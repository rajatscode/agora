//! Semantic axis — real LLM reasoning over the proposal's meaning.
//!
//! This is one of Feature 2's two load-bearing axes. Per the impl brief and
//! Proof 4: **this must be real LLM reasoning, not regex or template matching.**
//! We send Anthropic the proposal's change kind, target FQN, semantic
//! contract (meaning_before / meaning_after / invariants), and any relevant
//! catalog snippet, and let the model return a structured verdict via
//! tool-use.
//!
//! Fallback behavior: if `ANTHROPIC_API_KEY` is unset OR the call fails,
//! we emit a Low-confidence advisory verdict derived from the change kind
//! alone. The CheckRow's `source` field makes this transparent
//! (`anthropic:<model>` vs `offline-fallback`).
//!
//! We also honor the upstream proposal's authoring mode: if
//! `provenance.model` looks like an offline heuristic stand-in
//! (matches Feature 1's `OfflineNoKey` / `OfflineApiError` modes), we
//! downgrade confidence even when our own LLM call succeeds — the proposal's
//! `meaning_before`/`meaning_after` we're reasoning over were authored by a
//! template, not an LLM, so the semantic delta is constructed, not observed.

use crate::ast::{Change, OntologyChangeProposal};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use crate::seed::ConceptCard;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Instant;

const SEMANTIC_MODEL: &str = "claude-sonnet-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SemanticVerdict {
    /// "additive" | "refinement" | "breaking" | "dangerous"
    classification: String,
    /// 1-3 sentence explanation.
    reasoning: String,
    /// Does this proposal claim invariants the existing data may violate?
    invariants_at_risk: Vec<String>,
    /// Concept FQNs whose meaning overlaps with this proposal (for reuse cross-check).
    overlapping_concepts: Vec<String>,
}

pub async fn run(proposal: &OntologyChangeProposal, catalog: &[ConceptCard]) -> CheckRow {
    let started = Instant::now();

    // Detect "upstream offline" — Feature 1's heuristic author signs the
    // proposal with an `offline-*` model name. If that's true, even a
    // successful LLM call here is reasoning over a template-authored
    // semantic_contract, so we cap our confidence.
    let upstream_offline = is_upstream_offline(&proposal.provenance.model);

    let api_key = std::env::var("ANTHROPIC_API_KEY").ok();

    match api_key {
        Some(key) if !key.is_empty() => match call_anthropic(&key, proposal, catalog).await {
            Ok(verdict) => row_from_verdict(verdict, started, upstream_offline, /*live=*/ true),
            Err(e) => {
                tracing::warn!("Semantic LLM call failed: {e}; falling back to deterministic verdict");
                fallback_row(proposal, started, format!("anthropic-error: {e}"))
            }
        },
        _ => fallback_row(proposal, started, "no-api-key".into()),
    }
}

fn is_upstream_offline(model: &str) -> bool {
    let m = model.to_lowercase();
    m.starts_with("offline-") || m.contains("heuristic")
}

fn row_from_verdict(
    v: SemanticVerdict,
    started: Instant,
    upstream_offline: bool,
    live_call: bool,
) -> CheckRow {
    let cls = v.classification.to_lowercase();
    let outcome = match cls.as_str() {
        "additive" | "refinement" => Outcome::Pass,
        "breaking" | "dangerous" => Outcome::Fail,
        _ => Outcome::Advisory,
    };
    // If reasoning surfaces invariants_at_risk, escalate Pass → Advisory.
    let outcome = if matches!(outcome, Outcome::Pass) && !v.invariants_at_risk.is_empty() {
        Outcome::Advisory
    } else {
        outcome
    };

    let confidence = if upstream_offline {
        Confidence::Low
    } else if live_call {
        Confidence::High
    } else {
        Confidence::Low
    };

    let source = if live_call {
        format!("anthropic:{}", SEMANTIC_MODEL)
    } else {
        "offline-fallback".into()
    };

    let upstream_note = if upstream_offline {
        " (upstream proposal was authored offline; verdict downgraded to low-confidence)"
    } else {
        ""
    };

    CheckRow {
        axis: Axis::Semantic,
        outcome,
        findings: format!(
            "{}: {}{}",
            cls,
            v.reasoning,
            upstream_note,
        ),
        evidence: json!({
            "classification": cls,
            "reasoning": v.reasoning,
            "invariants_at_risk": v.invariants_at_risk,
            "overlapping_concepts": v.overlapping_concepts,
            "upstream_offline": upstream_offline,
        }),
        confidence,
        source,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

fn fallback_row(proposal: &OntologyChangeProposal, started: Instant, why: String) -> CheckRow {
    // Deterministic stand-in. Marked Low confidence + offline-fallback source
    // so the demo audience can see this isn't the real LLM verdict.
    let (cls, reasoning) = match &proposal.change {
        Change::AddField { field, .. } if !field.required => (
            "additive",
            "Optional field addition; existing rows unaffected (offline classification).",
        ),
        Change::AddField { .. } => (
            "refinement",
            "Adding a required field; existing rows need backfill (offline classification).",
        ),
        Change::AddRelation { .. } => (
            "additive",
            "New relation; existing entities remain valid (offline classification).",
        ),
        Change::CreateType { .. } => (
            "additive",
            "New type; no historical interpretation changes (offline classification).",
        ),
        Change::DeprecateField { .. } => (
            "refinement",
            "Deprecation enters retirement window (offline classification).",
        ),
        Change::ReclassifyField { .. } => (
            "refinement",
            "Reclassification narrows or widens visibility (offline classification).",
        ),
        Change::TightenField {
            from_required,
            to_required,
            ..
        } if !from_required && *to_required => (
            "refinement",
            "Tightens an optional field to required; existing NULL rows would violate. \
             Defer to data-conformance axis (offline classification).",
        ),
        Change::TightenField { .. } => (
            "refinement",
            "Tightens a field constraint (offline classification).",
        ),
    };

    let outcome = match cls {
        "breaking" | "dangerous" => Outcome::Fail,
        _ => Outcome::Pass,
    };

    CheckRow {
        axis: Axis::Semantic,
        outcome,
        findings: format!("{}: {} (fallback: {})", cls, reasoning, why),
        evidence: json!({
            "classification": cls,
            "reasoning": reasoning,
            "fallback_reason": why,
        }),
        confidence: Confidence::Low,
        source: "offline-fallback".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

async fn call_anthropic(
    api_key: &str,
    proposal: &OntologyChangeProposal,
    catalog: &[ConceptCard],
) -> Result<SemanticVerdict> {
    let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| SEMANTIC_MODEL.into());
    let endpoint = std::env::var("ANTHROPIC_ENDPOINT")
        .unwrap_or_else(|_| "https://api.anthropic.com/v1/messages".into());

    let user_msg = build_user_prompt(proposal, catalog);

    let body = json!({
        "model": model,
        "max_tokens": 1024,
        "tool_choice": { "type": "tool", "name": "emit_semantic_verdict" },
        "tools": [{
            "name": "emit_semantic_verdict",
            "description": "Emit a semantic compatibility verdict for an ontology change proposal.",
            "input_schema": verdict_schema()
        }],
        "system": SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": user_msg
        }]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let resp = client
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("anthropic request failed")?;

    let status = resp.status();
    let raw: Value = resp.json().await.context("anthropic response not JSON")?;
    if !status.is_success() {
        return Err(anyhow!("anthropic {} → {}", status, raw));
    }

    let content = raw
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("anthropic response missing `content`: {raw}"))?;
    let tool_input = content
        .iter()
        .find_map(|b| {
            if b.get("type")?.as_str()? == "tool_use" {
                b.get("input").cloned()
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("model did not emit tool_use block"))?;
    let verdict: SemanticVerdict =
        serde_json::from_value(tool_input).context("tool_use.input did not match schema")?;
    Ok(verdict)
}

const SYSTEM_PROMPT: &str = "\
You are Agora's semantic-compatibility judge. You receive a single \
OntologyChangeProposal and the catalog of canonical concepts it might \
overlap with. Your job: reason about whether the change preserves, refines, \
or breaks the *meaning* of the affected concepts and any of its declared \
invariants.

You MUST call the emit_semantic_verdict tool exactly once. Do not reply in \
prose. Be specific in `reasoning`: cite the meaning_before/meaning_after \
delta, name the invariants you considered, and identify any catalog concept \
whose meaning overlaps. If the change tightens a constraint (e.g. optional → \
required) that existing data could violate, classify as `refinement` and list \
the at-risk invariants — the data-conformance axis will do the actual row \
count.\n\n\
Classification rubric:\n\
- additive:   adds new fields/types; existing rows + readers unaffected.\n\
- refinement: tightens or restructures existing meaning; needs data check.\n\
- breaking:   removes meaning consumers depend on; can't auto-approve.\n\
- dangerous:  expands policy/visibility OR redefines an invariant silently.";

fn build_user_prompt(proposal: &OntologyChangeProposal, catalog: &[ConceptCard]) -> String {
    let catalog_summary: String = catalog
        .iter()
        .map(|c| format!("- {}: {}", c.fqn, c.summary))
        .collect::<Vec<_>>()
        .join("\n");

    let change_summary = match &proposal.change {
        Change::AddField { type_ref, field } => format!(
            "AddField on {} — field `{}` ({:?}), required={}, class={:?}",
            type_ref.fqn(),
            field.name,
            field.proto_type,
            field.required,
            field.classification
        ),
        Change::AddRelation { relation } => format!(
            "AddRelation `{}` from {} to {} ({:?})",
            relation.name,
            relation.from.fqn(),
            relation.to.fqn(),
            relation.cardinality
        ),
        Change::DeprecateField {
            type_ref,
            field_name,
        } => format!("DeprecateField {}.{}", type_ref.fqn(), field_name),
        Change::ReclassifyField {
            type_ref,
            field_name,
            to,
        } => format!(
            "ReclassifyField {}.{} → {:?}",
            type_ref.fqn(),
            field_name,
            to
        ),
        Change::CreateType { spec } => format!("CreateType {}.{}", spec.namespace, spec.name),
        Change::TightenField {
            type_ref,
            field_name,
            from_required,
            to_required,
        } => format!(
            "TightenField {}.{}: required {}→{}",
            type_ref.fqn(),
            field_name,
            from_required,
            to_required
        ),
    };

    format!(
        "PROPOSAL: {pid}\n\
         INTENT: {intent}\n\
         CHANGE: {change}\n\
         meaning_before: {before}\n\
         meaning_after:  {after}\n\
         invariants: {invariants:?}\n\n\
         CATALOG (canonical concepts):\n{catalog}\n\n\
         Emit your semantic verdict via the tool now.",
        pid = proposal.id,
        intent = proposal.change_intent,
        change = change_summary,
        before = proposal.semantic_contract.meaning_before,
        after = proposal.semantic_contract.meaning_after,
        invariants = proposal.semantic_contract.invariants,
        catalog = catalog_summary,
    )
}

fn verdict_schema() -> Value {
    json!({
        "type": "object",
        "required": ["classification", "reasoning", "invariants_at_risk", "overlapping_concepts"],
        "properties": {
            "classification": {
                "type": "string",
                "enum": ["additive", "refinement", "breaking", "dangerous"]
            },
            "reasoning": {
                "type": "string",
                "description": "1-3 sentences citing the meaning delta and any catalog overlap."
            },
            "invariants_at_risk": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Invariants existing data may violate; empty if none."
            },
            "overlapping_concepts": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Catalog FQNs whose meaning overlaps with this proposal; empty if none."
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn tighten_proposal() -> OntologyChangeProposal {
        OntologyChangeProposal {
            id: "prop_test_tighten".into(),
            domain: "users".into(),
            namespace: "core.users".into(),
            change_intent: "tighten email".into(),
            rationale: "compliance".into(),
            change: Change::TightenField {
                type_ref: TypeRef {
                    namespace: "core.users".into(),
                    name: "Account".into(),
                },
                field_name: "email".into(),
                from_required: false,
                to_required: true,
            },
            semantic_contract: SemanticContract {
                meaning_before: "email may be null".into(),
                meaning_after: "every account has an email".into(),
                justification: None,
                invariants: vec!["every account has an email".into()],
            },
            compatibility: CompatibilityDeclaration::default(),
            ownership: Ownership {
                team: "identity-platform".into(),
                semantic_steward: None,
            },
            tests: vec![],
            provenance: Provenance {
                author: "test".into(),
                source_prompt: "test".into(),
                model: "offline-heuristic-v0".into(),
                generated_at: "now".into(),
                trace_id: None,
            },
        }
    }

    #[tokio::test]
    async fn fallback_when_no_api_key() {
        std::env::remove_var("ANTHROPIC_API_KEY");
        let p = tighten_proposal();
        let row = super::run(&p, &[]).await;
        // Fallback path emits offline-fallback source + Low confidence.
        assert_eq!(row.source, "offline-fallback");
        assert!(matches!(row.confidence, Confidence::Low));
        let cls = row.evidence["classification"].as_str().unwrap_or("");
        assert_eq!(cls, "refinement");
    }
}
