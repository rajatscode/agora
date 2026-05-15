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
    OntologyType, Ownership, PolicyClass, ProposalTest, ProtoType, Provenance, SemanticContract,
    TypeRef,
};

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
