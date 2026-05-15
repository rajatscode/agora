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
    Change, CompatibilityClass, CompatibilityDeclaration, Field, OntologyChangeProposal,
    Ownership, PolicyClass, ProposalTest, ProtoType, Provenance, Relation, SemanticContract,
    TypeRef,
};

const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Public entry. Calls Anthropic once and returns a fully-populated proposal.
/// Falls back to a deterministic offline author when no API key is available.
pub async fn author_proposal(
    user_prompt: &str,
    actor: &str,
) -> Result<OntologyChangeProposal> {
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
                return Ok(prop);
            }
            Err(e) => {
                tracing::warn!(
                    "Anthropic call failed ({e}); falling back to offline heuristic author"
                );
            }
        }
    } else {
        tracing::warn!("ANTHROPIC_API_KEY unset; using offline heuristic author");
    }

    Ok(mock_proposal_from_prompt(user_prompt, actor, proposal_id))
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
                "required": ["meaning_before", "meaning_after"],
                "properties": {
                    "meaning_before": { "type": "string" },
                    "meaning_after":  { "type": "string" },
                    "justification":  { "type": "string" }
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
/// don't pay for tokens). Looks at keywords in the prompt to pick a
/// best-effort proposal shape. Not clever — it doesn't have to be: as soon
/// as `ANTHROPIC_API_KEY` is set the real model takes over.
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

    let (namespace, type_name, field_name, classification, summary) =
        if mentions_biometric || (mentions_login && mentions_bank) {
            (
                "core.integrations",
                "AuthenticationMethod",
                if mentions_biometric { "biometric_enrolled" } else { "supports_oauth" },
                PolicyClass::Internal,
                "Add a new authentication-method capability flag to BankIntegration.",
            )
        } else if mentions_user {
            (
                "core.users",
                "User",
                "preferred_login_method",
                PolicyClass::Internal,
                "Track which login method a user prefers.",
            )
        } else {
            (
                "core.integrations",
                "BankIntegration",
                "feature_flag",
                PolicyClass::Internal,
                "Generic feature-flag field on BankIntegration.",
            )
        };

    let target = TypeRef {
        namespace: namespace.to_string(),
        name: type_name.to_string(),
    };

    let change = Change::AddField {
        type_ref: target.clone(),
        field: Field {
            name: field_name.to_string(),
            proto_type: ProtoType::Bool,
            proto_number: 17, // arbitrary unused number; compiler workstream renumbers.
            required: false,
            since_version: 2,
            deprecated_in: None,
            classification: classification.clone(),
            doc: Some(summary.to_string()),
        },
    };

    let _ = Relation {
        name: "_".into(),
        from: target.clone(),
        to: target.clone(),
        cardinality: crate::ast::Cardinality::OneToOne,
        since_version: 1,
    };

    OntologyChangeProposal {
        id: proposal_id,
        domain: namespace.split('.').nth(1).unwrap_or("misc").to_string(),
        namespace: namespace.to_string(),
        change_intent: summary.to_string(),
        rationale: format!(
            "Heuristic author: original request was: \"{}\". Selected an additive \
             boolean field on `{}.{}` because the request reads as a capability flag.",
            prompt, namespace, type_name
        ),
        change,
        semantic_contract: SemanticContract {
            meaning_before: format!("`{}.{}` did not record this capability.", namespace, type_name),
            meaning_after: format!(
                "`{}.{}` now records `{}` as an opt-in capability flag.",
                namespace, type_name, field_name
            ),
            justification: Some(
                "Field is additive and optional, defaulting to false; existing readers \
                 see no behavioural change."
                    .into(),
            ),
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
            team: if namespace.starts_with("core.users") {
                "identity-platform".into()
            } else {
                "integrations-platform".into()
            },
            semantic_steward: Some("core-ontology".into()),
        },
        tests: vec![
            ProposalTest {
                name: "field_default_false".into(),
                kind: "smoke".into(),
                assertion: format!(
                    "After migration, all existing rows have `{}` = false.",
                    field_name
                ),
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
    }
}
