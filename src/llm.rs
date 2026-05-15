//! Anthropic structured-output authoring.
//!
//! One Anthropic call per proposal (per spec). We use the tool-use
//! pattern as the structured-output mechanism: define a tool whose
//! `input_schema` matches `OntologyChangeProposal`, force the model
//! to call it, then deserialize `tool_use.input` directly into the
//! Rust type.
//!
//! Why tool-use rather than the newer `response_format` JSON-schema
//! mode: tool-use has been GA on Anthropic since 2024 and is supported
//! by every SDK version we might end up linking. Behaviour is identical
//! for our purposes (model returns JSON conforming to a schema).
//!
//! If `ANTHROPIC_API_KEY` is unset we fall back to a deterministic
//! heuristic author (`mock_proposal_from_prompt`) so the demo runs offline.

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde_json::{json, Value};

use crate::ast::{
    BackfillPlan, Change, CompatibilityClass, CompatibilityDeclaration, Field, MigrationPlan,
    OntologyChangeProposal, OntologyType, Ownership, PolicyClass, ProposalTest, ProtoType,
    Provenance, SemanticContract, TypeRef,
};
use crate::check_report::CheckReport;

const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Where this proposal was authored. Surfaced loudly to stderr AND embedded
/// in the outcome JSON so downstream workstreams (and demo audiences) cannot
/// mistake an offline-heuristic proposal for an LLM-derived one.
///
/// Why this matters: in `Live` mode, `compatibility.semantic` is **derived
/// by the LLM** from the proposal's meaning_before/meaning_after/invariants
/// — that's what makes Proof 4's semantic axis non-theatrical. In any
/// `Offline*` mode, those classifications are heuristic-defaults (additive
/// across the board) and downstream consumers MUST treat them as
/// low-confidence stand-ins.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthorMode {
    /// Anthropic structured-output call succeeded; semantic+compatibility
    /// classifications are LLM-derived from semantic_contract.
    Live,
    /// `ANTHROPIC_API_KEY` was unset — never attempted the call.
    OfflineNoKey,
    /// Key present, call attempted, network/API error → fell back.
    /// `error` carries a one-line summary for the demo.
    OfflineApiError { error: String },
}

impl AuthorMode {
    pub fn is_live(&self) -> bool {
        matches!(self, AuthorMode::Live)
    }
    pub fn label(&self) -> &'static str {
        match self {
            AuthorMode::Live => "live (LLM-derived)",
            AuthorMode::OfflineNoKey => "OFFLINE (no API key)",
            AuthorMode::OfflineApiError { .. } => "OFFLINE (API error)",
        }
    }
}

/// Public entry. Calls Anthropic once and returns a fully-populated proposal,
/// plus an `AuthorMode` so callers can prominently flag offline runs.
pub async fn author_proposal(
    user_prompt: &str,
    actor: &str,
) -> Result<(OntologyChangeProposal, AuthorMode)> {
    let proposal_id = generate_proposal_id(user_prompt);

    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        match call_anthropic(&api_key, user_prompt).await {
            Ok(mut prop) => {
                // Always overwrite id + provenance with locally trusted values.
                prop.id = proposal_id;
                prop.provenance = Provenance {
                    author: actor.to_string(),
                    source_prompt: user_prompt.to_string(),
                    model: std::env::var("ANTHROPIC_MODEL")
                        .unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
                    generated_at: Utc::now().to_rfc3339(),
                    trace_id: None,
                };
                return Ok((prop, AuthorMode::Live));
            }
            Err(e) => {
                let err_summary = format!("{e}");
                tracing::warn!(
                    "Anthropic call failed ({err_summary}); falling back to offline heuristic author"
                );
                let prop = mock_proposal_from_prompt(user_prompt, actor, proposal_id);
                return Ok((prop, AuthorMode::OfflineApiError { error: err_summary }));
            }
        }
    }
    tracing::warn!("ANTHROPIC_API_KEY unset; using offline heuristic author");
    Ok((
        mock_proposal_from_prompt(user_prompt, actor, proposal_id),
        AuthorMode::OfflineNoKey,
    ))
}

fn generate_proposal_id(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(Utc::now().to_rfc3339().as_bytes());
    h.update(b"|");
    h.update(seed.as_bytes());
    let digest = h.finalize();
    format!("prop_{}", hex::encode(&digest[..8]))
}

// ----- Anthropic call -----

async fn call_anthropic(api_key: &str, user_prompt: &str) -> Result<OntologyChangeProposal> {
    let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let endpoint = std::env::var("ANTHROPIC_ENDPOINT")
        .unwrap_or_else(|_| "https://api.anthropic.com/v1/messages".into());

    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "tool_choice": { "type": "tool", "name": "emit_proposal" },
        "tools": [{
            "name": "emit_proposal",
            "description": "Emit a single OntologyChangeProposal in canonical Agora form.",
            "input_schema": proposal_input_schema()
        }],
        "system": SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": format!(
                "Author exactly one OntologyChangeProposal that captures the intent of \
                 the following request. Be concrete; pick a real namespace \
                 (`core.integrations` for bank/finance, `core.users` for users, etc). \
                 If the request is ambiguous, pick the most reasonable single change.\n\n\
                 REQUEST:\n{}",
                user_prompt
            )
        }]
    });

    let client = reqwest::Client::new();
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
        .find_map(|block| {
            if block.get("type")?.as_str()? == "tool_use" {
                block.get("input").cloned()
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("model did not emit a tool_use block"))?;

    let proposal: OntologyChangeProposal =
        serde_json::from_value(tool_input).context("tool_use.input did not match schema")?;
    Ok(proposal)
}

const SYSTEM_PROMPT: &str = "\
You are Agora's authoring agent. You receive a natural-language request from a \
product team (or another agent) and emit exactly one OntologyChangeProposal that \
makes the request concrete enough for the rest of Agora's pipeline (compiler, \
risk gate, runtime) to act on.

Hard rules:
1. Always call the `emit_proposal` tool exactly once. Never reply in prose.
2. Pick the SINGLE most plausible change kind: add_field, add_relation, \
   reclassify_field, deprecate_field, or create_type. If the request implies \
   several changes, pick the most central one and note the others in `rationale`.
3. Always populate `semantic_contract.meaning_before` and `meaning_after` even \
   when the field is brand new (use 'n/a' for before).
4. Be honest about `compatibility`: an additive optional field is `additive` \
   on shape, semantic, temporal, and storage axes; reclassifying PII upward is \
   `refinement` on policy; widening read scope is `dangerous` on policy.
4b. `semantic_contract.invariants` MUST contain at least two human-readable \
   invariants the change commits to (one about the new field/relation, one \
   about the relationship to the parent concept). The risk gate evaluates \
   these.
5. Prefer existing namespaces (`core.integrations`, `core.users`, \
   `core.payments`) — do NOT invent new top-level domains casually.
6. `ownership.team` should be a real team slug (`integrations-platform`, \
   `identity-platform`, `payments-platform`).";

// ----- JSON schema for the tool -----

fn proposal_input_schema() -> Value {
    // Hand-rolled JSON Schema — not auto-generated, because the Anthropic
    // tool-use validator only honours a subset of JSON Schema. Keep this
    // in lock-step with `OntologyChangeProposal` in `ast.rs`.
    json!({
        "type": "object",
        "required": [
            "id", "domain", "namespace", "change_intent", "rationale",
            "change", "semantic_contract", "compatibility", "ownership",
            "tests", "provenance"
        ],
        "properties": {
            "id":             { "type": "string", "description": "Will be overwritten by the CLI; supply any placeholder." },
            "domain":         { "type": "string", "description": "Coarse domain tag, e.g. 'integrations', 'users', 'payments'." },
            "namespace":      { "type": "string", "description": "Dotted namespace, e.g. 'core.integrations'." },
            "change_intent":  { "type": "string", "description": "One sentence summarising the change." },
            "rationale":      { "type": "string", "description": "Why this change matters. 1-3 sentences." },
            "change": {
                "type": "object",
                "description": "Discriminated union — set `kind` and the fields for that variant.",
                "required": ["kind"],
                "properties": {
                    "kind": { "type": "string", "enum": [
                        "add_field", "add_relation", "deprecate_field",
                        "reclassify_field", "create_type"
                    ] },
                    "type_ref":   { "$ref": "#/definitions/type_ref" },
                    "field":      { "$ref": "#/definitions/field" },
                    "relation":   { "$ref": "#/definitions/relation" },
                    "field_name": { "type": "string" },
                    "to":         { "$ref": "#/definitions/policy_class" },
                    "spec":       { "$ref": "#/definitions/ontology_type" }
                }
            },
            "semantic_contract": {
                "type": "object",
                "required": ["meaning_before", "meaning_after", "invariants"],
                "properties": {
                    "meaning_before": { "type": "string" },
                    "meaning_after":  { "type": "string" },
                    "justification":  { "type": "string" },
                    "invariants": {
                        "type": "array",
                        "minItems": 2,
                        "description": "At least two human-readable invariants the change commits to (e.g. 'Every active BankIntegration has ≥1 supported AuthenticationMethod').",
                        "items": { "type": "string" }
                    }
                }
            },
            "compatibility": {
                "type": "object",
                "required": ["shape", "semantic", "temporal", "policy", "api", "storage"],
                "properties": {
                    "shape":    { "$ref": "#/definitions/compat_class" },
                    "semantic": { "$ref": "#/definitions/compat_class" },
                    "temporal": { "$ref": "#/definitions/compat_class" },
                    "policy":   { "$ref": "#/definitions/compat_class" },
                    "api":      { "$ref": "#/definitions/compat_class" },
                    "storage":  { "$ref": "#/definitions/compat_class" }
                }
            },
            "ownership": {
                "type": "object",
                "required": ["team"],
                "properties": {
                    "team":              { "type": "string" },
                    "semantic_steward":  { "type": "string" }
                }
            },
            "tests": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["name", "kind", "assertion"],
                    "properties": {
                        "name":      { "type": "string" },
                        "kind":      { "type": "string", "enum": ["invariant", "compatibility", "smoke"] },
                        "assertion": { "type": "string" }
                    }
                }
            },
            "provenance": {
                "type": "object",
                "required": ["author", "source_prompt", "model", "generated_at"],
                "properties": {
                    "author":         { "type": "string" },
                    "source_prompt":  { "type": "string" },
                    "model":           { "type": "string" },
                    "generated_at":   { "type": "string" },
                    "trace_id":       { "type": "string" }
                }
            }
        },
        "definitions": {
            "type_ref": {
                "type": "object",
                "required": ["namespace", "name"],
                "properties": {
                    "namespace": { "type": "string" },
                    "name":      { "type": "string" }
                }
            },
            "policy_class": { "type": "string", "enum": ["Public","Internal","Sensitive","Pii"] },
            "compat_class": { "type": "string", "enum": ["additive","refinement","breaking","dangerous"] },
            "proto_type": {
                "description": "Either a primitive string or {Ref: 'fqn'}.",
                "oneOf": [
                    { "type": "string", "enum": ["String","Int64","Bool","Bytes","Timestamp"] },
                    { "type": "object", "required": ["Ref"], "properties": { "Ref": { "type": "string" } } }
                ]
            },
            "field": {
                "type": "object",
                "required": ["name","proto_type","proto_number","required","since_version"],
                "properties": {
                    "name":            { "type": "string" },
                    "proto_type":      { "$ref": "#/definitions/proto_type" },
                    "proto_number":    { "type": "integer", "minimum": 1 },
                    "required":        { "type": "boolean" },
                    "since_version":   { "type": "integer", "minimum": 1 },
                    "deprecated_in":   { "type": "integer" },
                    "classification":  { "$ref": "#/definitions/policy_class" },
                    "doc":             { "type": "string" }
                }
            },
            "relation": {
                "type": "object",
                "required": ["name","from","to","cardinality","since_version"],
                "properties": {
                    "name":          { "type": "string" },
                    "from":          { "$ref": "#/definitions/type_ref" },
                    "to":            { "$ref": "#/definitions/type_ref" },
                    "cardinality":   { "type": "string", "enum": ["one_to_one","one_to_many","many_to_many"] },
                    "since_version": { "type": "integer", "minimum": 1 }
                }
            },
            "ontology_type": {
                "type": "object",
                "required": ["namespace","name","version","ownership"],
                "properties": {
                    "namespace":   { "type": "string" },
                    "name":        { "type": "string" },
                    "version":     { "type": "integer", "minimum": 1 },
                    "fields":      { "type": "array", "items": { "$ref": "#/definitions/field" } },
                    "relations":   { "type": "array", "items": { "$ref": "#/definitions/relation" } },
                    "invariants":  { "type": "array", "items": { "type": "string" } },
                    "ownership": {
                        "type": "object",
                        "required": ["team"],
                        "properties": {
                            "team":             { "type": "string" },
                            "semantic_steward": { "type": "string" }
                        }
                    },
                    "policy_class": { "$ref": "#/definitions/policy_class" },
                    "locality":     { "type": "string" },
                    "doc":          { "type": "string" }
                }
            }
        }
    })
}

// ----- offline fallback -----

/// Heuristic author so the CLI works without an API key (and so unit tests
/// don't pay for tokens). Routes prompts through one of three buckets:
///   - matches an existing concept   → AddField on that concept (Refinement)
///   - matches "user/customer"       → AddField on `core.users.User`
///   - matches NOTHING in the catalog → CreateType under a fresh namespace
///                                      derived from the prompt (New)
///
/// The third bucket is what makes "cosmic ray sensors" classify as `New`
/// rather than getting force-fit onto BankIntegration.
///
/// As soon as `ANTHROPIC_API_KEY` is set the real model takes over.
pub fn mock_proposal_from_prompt(
    prompt: &str,
    actor: &str,
    proposal_id: String,
) -> OntologyChangeProposal {
    let lower = prompt.to_lowercase();
    let mentions_biometric = lower.contains("biometric")
        || lower.contains("face id")
        || lower.contains("touch id")
        || lower.contains("passkey");
    let mentions_login = lower.contains("login")
        || lower.contains("auth")
        || lower.contains("sign in")
        || lower.contains("signin");
    let mentions_bank = lower.contains("bank") || lower.contains("integration");
    let mentions_user = lower.contains("user") || lower.contains("customer");

    // Beat 6 / F6 case: "tighten Account.email to required" → TightenField.
    // We surface this *before* the additive heuristics because the keyword
    // "tighten" plus an email mention is a strong, narrow signal — it's the
    // canonical refinement case Agora's data-conformance axis is built to
    // catch, and the F6 agent loop needs to be able to produce it from a
    // bare prompt when no API key is set.
    if lower.contains("tighten")
        || (lower.contains("required") && (lower.contains("email") || lower.contains("account")))
    {
        if let Some((namespace, type_name, field_name)) = guess_tighten_target(&lower) {
            return author_tighten_field_on(
                &namespace,
                &type_name,
                &field_name,
                prompt,
                actor,
                proposal_id,
            );
        }
    }

    if mentions_biometric || (mentions_login && mentions_bank) {
        return author_add_field_on(
            "core.integrations",
            "AuthenticationMethod",
            if mentions_biometric { "biometric_enrolled" } else { "supports_oauth" },
            PolicyClass::Internal,
            "Add a new authentication-method capability flag to BankIntegration.",
            "integrations-platform",
            prompt,
            actor,
            proposal_id,
        );
    }
    if mentions_user {
        return author_add_field_on(
            "core.users",
            "User",
            "preferred_login_method",
            PolicyClass::Internal,
            "Track which login method a user prefers.",
            "identity-platform",
            prompt,
            actor,
            proposal_id,
        );
    }
    // Nothing matched → propose a brand-new type. This makes novel prompts
    // ("cosmic ray sensors", "telemetry probes", etc.) classify as `New`
    // by both the classifier (no exact-match in catalog) AND the spec
    // (the change kind is CreateType).
    author_create_type_from(prompt, actor, proposal_id)
}

/// Maps a lowercased prompt onto a (namespace, TypeName, field_name) for
/// the Tighten case. Narrow on purpose — recognizes the canonical Beat 6
/// targets only. Anything outside this set falls back to the additive/
/// create-type heuristics.
fn guess_tighten_target(lower: &str) -> Option<(String, String, String)> {
    if lower.contains("email") && (lower.contains("account") || lower.contains("user")) {
        // Prefer Account because that's the seed catalog's optional-email
        // concept (the one that has 47 NULL rows). User has email already
        // required in the seed, so tightening it is a no-op.
        if lower.contains("account") {
            return Some((
                "core.users".into(),
                "Account".into(),
                "email".into(),
            ));
        }
        return Some((
            "core.users".into(),
            "User".into(),
            "email".into(),
        ));
    }
    // Heuristic: phrase like "tighten Foo.bar". Pull X and Y from the dotted
    // reference if present in the prompt.
    let toks: Vec<&str> = lower.split_whitespace().collect();
    for t in toks {
        if let Some((lhs, rhs)) = t.split_once('.') {
            if lhs.chars().all(|c| c.is_ascii_alphabetic())
                && rhs.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
                && !lhs.is_empty()
                && !rhs.is_empty()
            {
                // Catalog lookup first: if any seed concept's terminal name
                // case-insensitively matches `lhs`, prefer its real
                // namespace + PascalCase name. This handles compound names
                // like "AuditFinding" / "BankIntegration" that naïve
                // pascal_case mishandles.
                if let Some((ns, nm)) = lookup_concept_by_lowername(lhs) {
                    return Some((ns, nm, rhs.to_string()));
                }
                // Fallback for tokens not in the seed catalog: best-guess
                // namespace from lhs and naïve PascalCase.
                let type_name = pascal_case(lhs);
                let namespace = if lhs == "account" || lhs == "user" {
                    "core.users".into()
                } else if lhs.contains("integration") || lhs.contains("bank") {
                    "core.integrations".into()
                } else {
                    format!("core.{}", lhs)
                };
                return Some((namespace, type_name, rhs.to_string()));
            }
        }
    }
    None
}

/// Catalog lookup that doesn't require threading `catalog` through the
/// heuristic-author call chain. Stays narrow on purpose — pulls the seed
/// concept list once and case-insensitively matches the terminal name
/// (`spec.name`). Returns `(namespace, name)` for the first match.
///
/// Why this is OK: `mock_proposal_from_prompt` is the offline fallback path;
/// it already implicitly assumes the seed catalog. The real LLM path doesn't
/// need this — Anthropic gets the catalog summary in the system prompt and
/// emits the FQN directly.
fn lookup_concept_by_lowername(lower_name: &str) -> Option<(String, String)> {
    use crate::seed::baseline_concepts;
    for card in baseline_concepts() {
        if card.spec.name.to_lowercase() == lower_name {
            return Some((card.spec.namespace.clone(), card.spec.name.clone()));
        }
    }
    None
}

fn author_tighten_field_on(
    namespace: &str,
    type_name: &str,
    field_name: &str,
    prompt: &str,
    actor: &str,
    proposal_id: String,
) -> OntologyChangeProposal {
    let target = TypeRef {
        namespace: namespace.to_string(),
        name: type_name.to_string(),
    };
    OntologyChangeProposal {
        id: proposal_id,
        domain: namespace.split('.').nth(1).unwrap_or("misc").to_string(),
        namespace: namespace.to_string(),
        change_intent: format!(
            "Tighten {}.{}.{} from optional to required.",
            namespace, type_name, field_name
        ),
        rationale: format!(
            "Heuristic author: request \"{}\" reads as a refinement — tightening an existing \
             optional field to required. Recorded as a `tighten_field` so the data-conformance \
             axis can verify against live rows.",
            prompt
        ),
        change: Change::TightenField {
            type_ref: target.clone(),
            field_name: field_name.to_string(),
            from_required: false,
            to_required: true,
        },
        semantic_contract: SemanticContract {
            meaning_before: format!(
                "`{}.{}.{}` is optional; existing rows may have NULL.",
                namespace, type_name, field_name
            ),
            meaning_after: format!(
                "Every `{}.{}` row carries a non-null `{}`.",
                namespace, type_name, field_name
            ),
            justification: Some(
                "Refines the meaning of the concept by strengthening an invariant. Existing \
                 NULL rows must be backfilled before this can ship."
                    .into(),
            ),
            invariants: vec![
                format!("Every `{}.{}` has a non-null `{}`.", namespace, type_name, field_name),
                format!(
                    "No new `{}.{}` may be created with `{}` = NULL.",
                    namespace, type_name, field_name
                ),
            ],
        },
        compatibility: CompatibilityDeclaration {
            shape: CompatibilityClass::Refinement,
            semantic: CompatibilityClass::Refinement,
            temporal: CompatibilityClass::Refinement,
            policy: CompatibilityClass::Additive,
            api: CompatibilityClass::Refinement,
            storage: CompatibilityClass::Refinement,
        },
        ownership: Ownership {
            team: if namespace == "core.users" {
                "identity-platform".into()
            } else if namespace == "core.integrations" {
                "integrations-platform".into()
            } else {
                "core-ontology".into()
            },
            semantic_steward: Some("core-ontology".into()),
        },
        tests: vec![
            ProposalTest {
                name: format!("no_null_{}_post_migration", field_name),
                kind: "invariant".into(),
                assertion: format!(
                    "After migration, no `{}.{}` row has `{}` = NULL.",
                    namespace, type_name, field_name
                ),
            },
            ProposalTest {
                name: format!("create_rejects_null_{}", field_name),
                kind: "compatibility".into(),
                assertion: format!(
                    "Creating a `{}.{}` with `{}` omitted returns 400.",
                    namespace, type_name, field_name
                ),
            },
        ],
        provenance: Provenance {
            author: actor.to_string(),
            source_prompt: prompt.to_string(),
            model: "offline-heuristic-v0".into(),
            generated_at: Utc::now().to_rfc3339(),
            trace_id: None,
        },
        // Intentionally NO migration plan on the first author. The F6
        // agent loop revision is what populates this — the first attempt is
        // supposed to be reckless about backfill, so the gate has a real
        // reason to block.
        migration: None,
    }
}

fn author_add_field_on(
    namespace: &str,
    type_name: &str,
    field_name: &str,
    classification: PolicyClass,
    summary: &str,
    team: &str,
    prompt: &str,
    actor: &str,
    proposal_id: String,
) -> OntologyChangeProposal {
    let target = TypeRef {
        namespace: namespace.to_string(),
        name: type_name.to_string(),
    };
    let change = Change::AddField {
        type_ref: target.clone(),
        field: Field {
            name: field_name.to_string(),
            proto_type: ProtoType::Bool,
            // Heuristic-author placeholder. Real proto field numbers must be
            // unique within the parent message, monotonic, and skip the
            // 19000–19999 reserved range — the compiler workstream (WS-B)
            // is responsible for renumbering against the live message
            // when it merges this partial proto. Picked 17 as a hex-pretty
            // value that's clearly synthetic.
            proto_number: 17,
            required: false,
            since_version: 2,
            deprecated_in: None,
            classification: classification.clone(),
            doc: Some(summary.to_string()),
        },
    };

    OntologyChangeProposal {
        id: proposal_id,
        domain: namespace.split('.').nth(1).unwrap_or("misc").to_string(),
        namespace: namespace.to_string(),
        change_intent: summary.to_string(),
        rationale: rationale_for_addfield(prompt, namespace, type_name, field_name),
        change,
        semantic_contract: SemanticContract {
            meaning_before: format!("`{}.{}` did not record this capability.", namespace, type_name),
            meaning_after: format!(
                "`{}.{}` now records `{}` as an opt-in capability flag.",
                namespace, type_name, field_name
            ),
            justification: Some(
                "Field is additive and optional; existing readers see no behavioural change."
                    .into(),
            ),
            invariants: vec![
                format!("If `{}` is true on a `{}`, the underlying provider must support it (validated downstream).", field_name, type_name),
                format!("Setting `{}` does not change the visibility class of `{}.{}`.", field_name, namespace, type_name),
            ],
        },
        compatibility: CompatibilityDeclaration {
            shape: CompatibilityClass::Additive,
            semantic: CompatibilityClass::Additive,
            temporal: CompatibilityClass::Additive,
            policy: CompatibilityClass::Additive,
            api: CompatibilityClass::Additive,
            storage: CompatibilityClass::Additive,
        },
        ownership: Ownership {
            team: team.to_string(),
            semantic_steward: Some("core-ontology".into()),
        },
        tests: vec![
            ProposalTest {
                name: "field_default_false".into(),
                kind: "smoke".into(),
                assertion: format!("After migration, all existing rows have `{}` = false.", field_name),
            },
            ProposalTest {
                name: "additive_back_compat".into(),
                kind: "compatibility".into(),
                assertion: "Existing readers ignoring unknown fields still parse rows.".into(),
            },
        ],
        provenance: Provenance {
            author: actor.to_string(),
            source_prompt: prompt.to_string(),
            model: "offline-heuristic-v0".into(),
            generated_at: Utc::now().to_rfc3339(),
            trace_id: None,
        },
        migration: None,
    }
}

fn author_create_type_from(
    prompt: &str,
    actor: &str,
    proposal_id: String,
) -> OntologyChangeProposal {
    let (domain, type_name) = pick_domain_and_type_name(prompt);
    let namespace = format!("draft.{}", domain);
    let team = format!("{}-platform", domain);

    let new_type = OntologyType {
        namespace: namespace.clone(),
        name: type_name.clone(),
        version: 1,
        fields: vec![
            Field {
                name: "id".into(),
                proto_type: ProtoType::String,
                proto_number: 1,
                required: true,
                since_version: 1,
                deprecated_in: None,
                classification: PolicyClass::Internal,
                doc: Some("Stable id".into()),
            },
            Field {
                name: "created_at".into(),
                proto_type: ProtoType::Timestamp,
                proto_number: 2,
                required: true,
                since_version: 1,
                deprecated_in: None,
                classification: PolicyClass::Internal,
                doc: Some("Wall-clock time the entity was created.".into()),
            },
        ],
        relations: vec![],
        invariants: vec![
            format!("`{}.{}` is owned by exactly one team.", namespace, type_name),
        ],
        ownership: Ownership {
            team: team.clone(),
            semantic_steward: Some("core-ontology".into()),
        },
        policy_class: PolicyClass::Internal,
        locality: None,
        doc: Some(format!("Heuristic-author-drafted type for: \"{}\".", prompt)),
    };

    let summary = format!("Draft a new `{}` type under `{}`.", type_name, namespace);

    OntologyChangeProposal {
        id: proposal_id,
        domain,
        namespace: namespace.clone(),
        change_intent: summary.clone(),
        rationale: rationale_for_createtype(prompt, &namespace, &type_name),
        change: Change::CreateType { spec: new_type },
        semantic_contract: SemanticContract {
            meaning_before: "n/a — type does not exist yet".into(),
            meaning_after: format!(
                "`{}.{}` is the canonical representation of `{}` for the {} domain.",
                namespace, type_name, type_name, prompt
            ),
            justification: Some(
                "No existing concept in the catalogue covers this; new type drafted under \
                 a `draft.*` namespace pending steward review."
                    .into(),
            ),
            invariants: vec![
                format!("Every `{}.{}` has a non-empty `id`.", namespace, type_name),
                format!("`{}.{}.created_at` is monotonic per id.", namespace, type_name),
            ],
        },
        compatibility: CompatibilityDeclaration::default(), // a brand-new type is purely additive
        ownership: Ownership {
            team,
            semantic_steward: Some("core-ontology".into()),
        },
        tests: vec![
            ProposalTest {
                name: "type_namespacing".into(),
                kind: "smoke".into(),
                assertion: format!(
                    "Inserting a `{}.{}` does not collide with any existing fully-qualified name.",
                    namespace, type_name
                ),
            },
            ProposalTest {
                name: "additive_to_registry".into(),
                kind: "compatibility".into(),
                assertion: "Existing concept lookups are unaffected by the new draft type."
                    .into(),
            },
        ],
        provenance: Provenance {
            author: actor.to_string(),
            source_prompt: prompt.to_string(),
            model: "offline-heuristic-v0".into(),
            generated_at: Utc::now().to_rfc3339(),
            trace_id: None,
        },
        migration: None,
    }
}

/// Pull a (domain, TypeName) pair out of free text. Trivial: pick the first
/// alphabetic word ≥4 chars as the domain, the first noun-ish word
/// as the type name (PascalCased). Real model overrides this anyway.
fn pick_domain_and_type_name(prompt: &str) -> (String, String) {
    let stop: &[&str] = &[
        "the", "a", "an", "to", "and", "or", "for", "of", "in", "on", "we",
        "need", "want", "should", "must", "with", "from", "have", "users",
        "user", "system", "agora",
    ];
    let toks: Vec<&str> = prompt
        .split(|c: char| !c.is_alphabetic())
        .filter(|t| t.len() >= 4 && !stop.contains(&t.to_lowercase().as_str()))
        .collect();

    let domain = toks
        .first()
        .map(|s| s.to_lowercase())
        .unwrap_or_else(|| "misc".into());
    let type_word = toks.get(1).copied().unwrap_or_else(|| toks.first().copied().unwrap_or("Concept"));
    let type_name = pascal_case(type_word);

    (domain, type_name)
}

fn pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = true;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if upper {
                for u in ch.to_uppercase() {
                    out.push(u);
                }
                upper = false;
            } else {
                out.push(ch.to_ascii_lowercase());
            }
        } else {
            upper = true;
        }
    }
    if out.is_empty() {
        out.push_str("Concept");
    }
    out
}

fn rationale_for_addfield(
    prompt: &str,
    namespace: &str,
    type_name: &str,
    field_name: &str,
) -> String {
    let lower = prompt.to_lowercase();
    let why = if lower.contains("compliance") || lower.contains("audit") {
        "compliance/audit signal"
    } else if lower.contains("security") || lower.contains("biometric") {
        "security-posture signal"
    } else if lower.contains("preference") || lower.contains("opt") {
        "user-preference signal"
    } else {
        "capability flag"
    };
    format!(
        "Heuristic author: request reads as a {why} (\"{}\"). Modeled as an additive \
         boolean field `{field_name}` on `{namespace}.{type_name}` so adoption is \
         opt-in and no historical row interpretation changes."
    , prompt)
}

fn rationale_for_createtype(prompt: &str, namespace: &str, type_name: &str) -> String {
    format!(
        "Heuristic author: request \"{}\" doesn't map onto any concept in the catalogue. \
         Drafting a new type `{}.{}` under the `draft.*` namespace so it doesn't claim \
         a stable canonical name before the semantic steward weighs in.",
        prompt, namespace, type_name
    )
}

// ============================================================================
// Feature 6 — revise_proposal: closed-loop revision in response to a block.
// ============================================================================
//
// The agent loop calls this when the multi-axis check rejects the previous
// attempt. The job here is to feed the **structured CheckReport** back to
// the LLM (or the offline heuristic) and emit a new proposal that addresses
// the rejection. The classic case the F6 brief targets: data_conformance
// blocked the previous attempt → the revision adds `migration.backfill_plan`
// so DC flips to Advisory and the gate clears it.
//
// We deliberately do NOT just "retry the prompt" — that wouldn't be closed
// loop. The revision call gets the full prior proposal AND the rejection
// rationale, so it can target the specific axis that failed.

const REVISE_SYSTEM_PROMPT: &str = "\
You are Agora's revision agent. You receive: (a) the original user request, \
(b) a previous OntologyChangeProposal that was BLOCKED by Agora's multi-axis \
risk gate, and (c) the structured CheckReport explaining which axes failed \
and why. Your job is to emit a single REVISED proposal that addresses the \
rejection.

Hard rules:
1. Always call the `emit_proposal` tool exactly once. Never reply in prose.
2. Preserve the same `change` (kind, target, field name) unless the rejection \
   makes that change fundamentally impossible. If the change is salvageable \
   with a migration plan, populate `migration.backfill_plan` rather than \
   abandoning the change.
3. If the rejection cites `data_conformance` (existing rows would violate the \
   proposed constraint), you MUST populate `migration.backfill_plan` with a \
   concrete strategy. Set `strategy` to a short slug, `source` to where the \
   values come from (a column, a constant, a derivation), and \
   `idempotent: true` if rerunning the backfill is safe. Also set \
   `migration.backfill_query` to the SQL UPDATE you would run.
4. If the rejection cites `policy` (PII visibility), tighten the field's \
   classification rather than widen it.
5. Carry forward the original `id`, `domain`, `namespace`, `ownership` unless \
   the rejection explicitly demands a different owner.
6. Keep `compatibility` honest about the post-revision shape — a refinement \
   with a backfill_plan is still `refinement` on shape/semantic, but the \
   semantic_contract.justification should mention the backfill commitment.";

/// Public entry. Given the original prompt, the blocked proposal, and its
/// CheckReport, produce a revised proposal that targets the rejection.
///
/// Returns `(revised_proposal, mode, reason_summary)`. `reason_summary` is a
/// short one-liner the UI shows verbatim ("added backfill_plan:
/// derive_from_provider_config") so the demo audience can see exactly what
/// the revision changed.
pub async fn revise_proposal(
    original_prompt: &str,
    previous: &OntologyChangeProposal,
    check_report: &CheckReport,
    actor: &str,
) -> Result<(OntologyChangeProposal, AuthorMode, String)> {
    // We always preserve the previous proposal's id so the artifact directory
    // tracks one logical proposal across the revision arc.
    let preserved_id = previous.id.clone();

    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        match call_anthropic_revise(&api_key, original_prompt, previous, check_report).await {
            Ok(mut revised) => {
                revised.id = preserved_id;
                revised.provenance = Provenance {
                    author: actor.to_string(),
                    source_prompt: original_prompt.to_string(),
                    model: std::env::var("ANTHROPIC_MODEL")
                        .unwrap_or_else(|_| DEFAULT_MODEL.to_string()),
                    generated_at: Utc::now().to_rfc3339(),
                    trace_id: None,
                };
                let reason = summarize_revision(previous, &revised);
                return Ok((revised, AuthorMode::Live, reason));
            }
            Err(e) => {
                let err_summary = format!("{e}");
                tracing::warn!(
                    "Anthropic revision call failed ({err_summary}); falling back to deterministic heuristic"
                );
                let (revised, reason) =
                    heuristic_revise(original_prompt, previous, check_report, actor);
                return Ok((revised, AuthorMode::OfflineApiError { error: err_summary }, reason));
            }
        }
    }

    tracing::warn!("ANTHROPIC_API_KEY unset; using deterministic heuristic revision");
    let (revised, reason) = heuristic_revise(original_prompt, previous, check_report, actor);
    Ok((revised, AuthorMode::OfflineNoKey, reason))
}

async fn call_anthropic_revise(
    api_key: &str,
    original_prompt: &str,
    previous: &OntologyChangeProposal,
    check_report: &CheckReport,
) -> Result<OntologyChangeProposal> {
    let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let endpoint = std::env::var("ANTHROPIC_ENDPOINT")
        .unwrap_or_else(|_| "https://api.anthropic.com/v1/messages".into());

    let previous_json = serde_json::to_string_pretty(previous)
        .unwrap_or_else(|_| "{ /* serialize failed */ }".into());
    let report_json = serde_json::to_string_pretty(check_report)
        .unwrap_or_else(|_| "{ /* serialize failed */ }".into());

    let body = json!({
        "model": model,
        "max_tokens": 2048,
        "tool_choice": { "type": "tool", "name": "emit_proposal" },
        "tools": [{
            "name": "emit_proposal",
            "description": "Emit one REVISED OntologyChangeProposal that addresses the rejection.",
            // Reuse the authoring schema verbatim — same structure, with the
            // optional `migration` field that the revise system prompt
            // instructs the model to populate. The structurally-richer schema
            // is what makes data_conformance failures actually addressable.
            "input_schema": revise_input_schema()
        }],
        "system": REVISE_SYSTEM_PROMPT,
        "messages": [{
            "role": "user",
            "content": format!(
                "ORIGINAL REQUEST:\n{}\n\n\
                 PREVIOUS PROPOSAL (BLOCKED):\n```json\n{}\n```\n\n\
                 CHECK REPORT (explains the block):\n```json\n{}\n```\n\n\
                 Emit a single revised proposal that addresses the rejection. \
                 Preserve the original change kind and target unless impossible. \
                 If the rejection cites data_conformance, you MUST populate \
                 migration.backfill_plan.",
                original_prompt, previous_json, report_json
            )
        }]
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(&endpoint)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("anthropic revise request failed")?;

    let status = resp.status();
    let raw: Value = resp.json().await.context("anthropic revise response not JSON")?;

    if !status.is_success() {
        return Err(anyhow!("anthropic {} → {}", status, raw));
    }

    let content = raw
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow!("revise response missing `content`: {raw}"))?;

    let tool_input = content
        .iter()
        .find_map(|block| {
            if block.get("type")?.as_str()? == "tool_use" {
                block.get("input").cloned()
            } else {
                None
            }
        })
        .ok_or_else(|| anyhow!("revise model did not emit a tool_use block"))?;

    let proposal: OntologyChangeProposal =
        serde_json::from_value(tool_input).context("revise tool_use.input did not match schema")?;
    Ok(proposal)
}

/// JSON Schema for the revise call. Same as the authoring schema, plus an
/// optional top-level `migration` object so the model is told the field
/// exists and is the right answer for data_conformance failures.
fn revise_input_schema() -> Value {
    // Start with the authoring schema and graft the `migration` property
    // onto its `properties` map. This keeps the two in lock-step — if
    // proposal_input_schema() changes, the revise schema picks it up.
    let mut schema = proposal_input_schema();
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.insert(
            "migration".into(),
            json!({
                "type": "object",
                "description": "Migration commitment. Populate `backfill_plan` when the rejection cites data_conformance.",
                "properties": {
                    "backfill_plan": {
                        "type": "object",
                        "required": ["strategy"],
                        "properties": {
                            "strategy":   { "type": "string", "description": "Short slug, e.g. 'derive_from_provider_config'." },
                            "source":     { "type": "string", "description": "Where backfill values come from." },
                            "idempotent": { "type": "boolean" },
                            "rationale":  { "type": "string" }
                        }
                    },
                    "backfill_query":    { "type": "string", "description": "SQL the backfill would run." },
                    "dual_write_window": { "type": "string", "description": "How long the compatibility window lasts." }
                }
            }),
        );
    }
    schema
}

/// Deterministic offline fallback. Inspects the previous proposal + check
/// report; if the block cites data_conformance (or the previous change is a
/// TightenField that we know would fail), emit a revision that adds a
/// `migration.backfill_plan` targeting that field. Otherwise we re-emit the
/// previous proposal with a small bump to the rationale — which will still
/// fail the gate on the next attempt, surfacing as Stalled. That's
/// intentional: the offline path is honest about its narrow competence.
fn heuristic_revise(
    _original_prompt: &str,
    previous: &OntologyChangeProposal,
    check_report: &CheckReport,
    actor: &str,
) -> (OntologyChangeProposal, String) {
    let block_reason = check_report.block_reason.clone().unwrap_or_default();
    let dc_failed = block_reason.contains("data_conformance")
        || matches!(
            check_report.data_conformance.outcome,
            crate::check_report::Outcome::Fail | crate::check_report::Outcome::Skipped
        ) && check_report.data_conformance.applicable;

    // Case 1: data_conformance failure on a TightenField — add a backfill_plan.
    if dc_failed {
        if let Change::TightenField {
            type_ref,
            field_name,
            ..
        } = &previous.change
        {
            let (strategy, source, query) =
                heuristic_backfill_for(&type_ref.fqn(), field_name);
            let mut revised = previous.clone();
            revised.migration = Some(MigrationPlan {
                backfill_plan: Some(BackfillPlan {
                    strategy: strategy.clone(),
                    source: Some(source.clone()),
                    idempotent: true,
                    rationale: Some(format!(
                        "Backfill {}.{} from {} so the tightening is safe against the {} \
                         existing violating row(s) the data-conformance axis surfaced.",
                        type_ref.fqn(),
                        field_name,
                        source,
                        check_report.data_conformance.violations_found
                    )),
                }),
                backfill_query: Some(query),
                dual_write_window: Some("14d".into()),
            });
            revised.rationale = format!(
                "{} | Revised by agent loop: added backfill_plan to address {} existing \
                 row(s) flagged by data_conformance.",
                revised.rationale, check_report.data_conformance.violations_found
            );
            // Keep id stable across revisions; refresh provenance so the
            // audit trail shows the loop touched it.
            revised.provenance = Provenance {
                author: actor.to_string(),
                source_prompt: previous.provenance.source_prompt.clone(),
                model: "offline-revise-v0".into(),
                generated_at: Utc::now().to_rfc3339(),
                trace_id: None,
            };
            let reason = format!("added migration.backfill_plan ({strategy})");
            return (revised, reason);
        }
    }

    // Case 2: nothing we know how to fix deterministically. Re-emit the
    // previous proposal unchanged with a noted rationale — the loop will see
    // no progress and (eventually) Stall, which is the honest outcome.
    let mut unchanged = previous.clone();
    unchanged.provenance = Provenance {
        author: actor.to_string(),
        source_prompt: previous.provenance.source_prompt.clone(),
        model: "offline-revise-v0".into(),
        generated_at: Utc::now().to_rfc3339(),
        trace_id: None,
    };
    let reason = format!(
        "no deterministic revision available for block: {}",
        if block_reason.is_empty() {
            "(unspecified)".into()
        } else {
            block_reason
        }
    );
    (unchanged, reason)
}

/// Pick a sensible backfill strategy/source pair for the known (concept, field)
/// pairs in the seed catalog. The list is intentionally short — this is the
/// offline path; the live path delegates judgment to the LLM.
fn heuristic_backfill_for(target_fqn: &str, field_name: &str) -> (String, String, String) {
    match (target_fqn, field_name) {
        ("core.users.Account", "email") => (
            "derive_from_user_record".into(),
            "users.email WHERE users.account_id = accounts.id, else '<unknown>@placeholder.invalid'".into(),
            "UPDATE accounts a SET email = COALESCE((SELECT u.email FROM users u WHERE u.account_id = a.id), '<unknown>@placeholder.invalid') WHERE a.email IS NULL".into(),
        ),
        // F8: Customer 360 — the import-source rows that arrived without
        // emails get a synthetic placeholder so the tightening lands safely.
        // The lifecycle team can rerun a "real" backfill against their CRM
        // export later; this just satisfies the new invariant.
        ("core.customer.Customer", "email") => (
            "synthetic_placeholder_from_id".into(),
            "lower(customers.id) || '@placeholder.invalid' for import-source rows".into(),
            "UPDATE customers SET email = lower(id) || '@placeholder.invalid' WHERE email IS NULL".into(),
        ),
        // F9: Compliance / GRC. Tightening `AuditFinding.resolved_at` to
        // required would invalidate the open/investigating findings. The
        // compliance team's M0 stance: rather than synthetic-fill, mark
        // them as `accepted_risk` with `resolved_at = now()` so the
        // invariant holds AND the status reflects the operational truth.
        ("core.compliance.AuditFinding", "resolved_at") => (
            "synthetic_accept_open_findings".into(),
            "now() for findings still in status='open'/'investigating' (status promoted to accepted_risk)".into(),
            "UPDATE audit_findings SET resolved_at = now(), status = 'accepted_risk' WHERE resolved_at IS NULL".into(),
        ),
        _ => (
            "default_to_placeholder".into(),
            format!("constant: '<unknown>' for {}.{}", target_fqn, field_name),
            format!(
                "/* concrete table+column mapping not in heuristic catalog for {} — \
                 a real revision would consult the storage binding registry */",
                target_fqn
            ),
        ),
    }
}

/// Short, demo-friendly summary of *what* the revision changed.
fn summarize_revision(prev: &OntologyChangeProposal, revised: &OntologyChangeProposal) -> String {
    let added_backfill = prev.migration.as_ref().map(|m| m.has_backfill()).unwrap_or(false)
        == false
        && revised
            .migration
            .as_ref()
            .map(|m| m.has_backfill())
            .unwrap_or(false);
    if added_backfill {
        let strat = revised
            .migration
            .as_ref()
            .and_then(|m| m.backfill_plan.as_ref())
            .map(|b| b.strategy.clone())
            .unwrap_or_else(|| "unspecified".into());
        return format!("added migration.backfill_plan ({})", strat);
    }
    // Compatibility-class softening?
    if prev.compatibility.policy != revised.compatibility.policy {
        return format!(
            "narrowed policy classification ({:?} → {:?})",
            prev.compatibility.policy, revised.compatibility.policy
        );
    }
    "revised rationale/contract".into()
}
