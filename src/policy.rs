//! F5 — minimal FGA-style policy evaluator.
//!
//! The mutation_log invariant is "every controlled write is auditable". F5
//! extends that: every controlled write is also **authorised**. Before the
//! INSERT lands, `entity_write` calls `policy::evaluate(...)` against an
//! FGA-shaped policy spec for the concept. Allow → write proceeds. Deny →
//! the write is refused, a `DenyAttempt` row goes in the mutation_log
//! (so denials are auditable), and the caller gets a 403.
//!
//! Why we hand-roll the evaluator instead of pulling in regorus/openfga:
//!   * The brief is strict — ~100 lines, no new dependencies.
//!   * Our policy surface is intentionally narrow at M0: per-concept
//!     `owner` relation with a single owning team. Tuple matching is
//!     enough; we do not need rewrites, computed-userset, intersections,
//!     unions, or graph traversal.
//!   * The artifact shape (`generated/<prop>/<type>.fga.json`) is fixed by
//!     F1, so the evaluator's input format is stable.
//!
//! Tuple match rule (Allow iff there exists a tuple T in `spec.tuples`
//! such that):
//!   1. T.relation == requested relation
//!   2. T.user == requested actor                 (exact)        OR
//!      T.user == "team:*" (full wildcard)                       OR
//!      T.user ends with ":*" and actor starts with same prefix
//!   3. T.object == requested object              (exact)        OR
//!      T.object ends with ":*" and requested object starts with
//!      the same prefix (per-instance wildcard for concept-scoped grants)
//!
//! Anything else → Deny. The decision carries a human-readable reason so
//! the UI / mutation_log can quote it verbatim ("owner requires
//! team:integrations-platform; got team:marketing").

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::seed::ConceptCard;

/// Owner relation — the one F5 enforces on BankIntegration writes.
pub const RELATION_OWNER: &str = "owner";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow {
        /// Verbatim trace of *which* tuple matched. Surfaced in UI for the
        /// allow-side narrative ("matched owner ← team:integrations-platform").
        matched_tuple: PolicyTuple,
    },
    Deny {
        /// One-line reason, demo-grade ("owner requires team:integrations-platform;
        /// got team:marketing"). The mutation_log persists this verbatim.
        reason: String,
        /// The set of tuples that *could* have allowed this (for the trace UI).
        considered: Vec<PolicyTuple>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyTuple {
    pub object: String,
    pub relation: String,
    pub user: String,
}

impl PolicyDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, PolicyDecision::Allow { .. })
    }
    pub fn reason(&self) -> &str {
        match self {
            PolicyDecision::Allow { .. } => "allowed",
            PolicyDecision::Deny { reason, .. } => reason,
        }
    }
}

/// Walk `spec.tuples` for a row that matches (actor, relation, object).
/// Wildcards `team:*` and `<type>:*` are supported. See module docs for the
/// exact match rule.
pub fn evaluate(
    spec: &Value,
    actor: &str,
    relation: &str,
    object: &str,
) -> PolicyDecision {
    let tuples = extract_tuples(spec);
    let considered: Vec<PolicyTuple> = tuples
        .iter()
        .filter(|t| t.relation == relation)
        .cloned()
        .collect();

    for t in &considered {
        if relation_matches(&t.relation, relation)
            && object_matches(&t.object, object)
            && user_matches(&t.user, actor)
        {
            return PolicyDecision::Allow {
                matched_tuple: t.clone(),
            };
        }
    }

    // Build a useful denial trace: prefer the actual owner the spec names.
    let owner_users: Vec<String> = considered
        .iter()
        .filter(|t| t.relation == relation)
        .map(|t| t.user.clone())
        .collect();
    let reason = if owner_users.is_empty() {
        format!(
            "no `{relation}` tuple in policy spec; actor `{actor}` denied on `{object}`"
        )
    } else {
        format!(
            "`{relation}` requires {}; got `{actor}` on `{object}`",
            owner_users.join(" | ")
        )
    };
    PolicyDecision::Deny {
        reason,
        considered,
    }
}

fn relation_matches(spec_rel: &str, req_rel: &str) -> bool {
    spec_rel == req_rel
}

fn object_matches(spec_obj: &str, req_obj: &str) -> bool {
    if spec_obj == req_obj {
        return true;
    }
    // `bank_integration:*` matches any `bank_integration:<id>`.
    if let Some(prefix) = spec_obj.strip_suffix(":*") {
        if let Some(req_prefix) = req_obj.split(':').next() {
            return prefix == req_prefix;
        }
    }
    false
}

fn user_matches(spec_user: &str, actor: &str) -> bool {
    if spec_user == actor {
        return true;
    }
    // `team:*` matches any team.
    if spec_user == "team:*" {
        return actor.starts_with("team:");
    }
    // Generic `<prefix>:*` wildcard.
    if let Some(prefix) = spec_user.strip_suffix(":*") {
        return actor.starts_with(prefix) && actor[prefix.len()..].starts_with(':');
    }
    false
}

fn extract_tuples(spec: &Value) -> Vec<PolicyTuple> {
    let Some(arr) = spec.get("tuples").and_then(|v| v.as_array()) else {
        return vec![];
    };
    arr.iter()
        .filter_map(|t| {
            let object = t.get("object")?.as_str()?.to_string();
            let relation = t.get("relation")?.as_str()?.to_string();
            let user = t.get("user")?.as_str()?.to_string();
            Some(PolicyTuple {
                object,
                relation,
                user,
            })
        })
        .collect()
}

/// Construct a minimal FGA-shaped policy spec for a concept, based on its
/// `ownership.team`. This is the M0 stand-in for "load the per-proposal
/// .fga.json from disk": for the demo the seed catalog carries the truth,
/// so we synthesise the spec from it. The resulting JSON is the same shape
/// the F1 artifact emits (type_definitions + tuples).
///
/// The owner tuple uses `<type>:*` so it grants the relation for any
/// instance of the type — appropriate for the per-concept `owner` grant
/// the demo enforces.
pub fn spec_for_concept(card: &ConceptCard) -> Value {
    let fga_type = snake_case(&card.spec.name);
    let owner_team = format!("team:{}", card.spec.ownership.team);
    json!({
        "concept_fqn": card.fqn,
        "model": {
            "type_definitions": [{
                "type": fga_type,
                "relations": {
                    "owner":            { "this": {} },
                    "internal_viewer":  { "this": {} },
                }
            }]
        },
        "tuples": [
            { "object": format!("{fga_type}:*"), "relation": "owner",           "user": owner_team },
            { "object": format!("{fga_type}:*"), "relation": "internal_viewer", "user": "team:*"   }
        ]
    })
}

/// `BankIntegration` → `bank_integration`. Local copy to avoid pulling the
/// artifacts module into the runtime path.
fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// FGA object id helper: `bank_integration:bi_abc123`.
pub fn object_id(fga_type: &str, entity_id: &str) -> String {
    format!("{}:{}", fga_type, entity_id)
}

/// Resolve a concept FQN to its FGA `type` slug (the snake_case of the
/// terminal name). Returns `None` if the concept isn't in the catalog.
pub fn fga_type_for_fqn(catalog: &[ConceptCard], fqn: &str) -> Option<String> {
    catalog
        .iter()
        .find(|c| c.fqn == fqn)
        .map(|c| snake_case(&c.spec.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bi_spec() -> Value {
        json!({
            "tuples": [
                { "object": "bank_integration:*", "relation": "owner",           "user": "team:integrations-platform" },
                { "object": "bank_integration:*", "relation": "internal_viewer", "user": "team:*" },
            ]
        })
    }

    #[test]
    fn owner_team_is_allowed() {
        let d = evaluate(
            &bi_spec(),
            "team:integrations-platform",
            "owner",
            "bank_integration:bi_abc",
        );
        assert!(d.is_allow(), "{:?}", d);
    }

    #[test]
    fn non_owner_team_is_denied() {
        let d = evaluate(
            &bi_spec(),
            "team:marketing",
            "owner",
            "bank_integration:bi_abc",
        );
        assert!(!d.is_allow());
        let reason = d.reason();
        assert!(reason.contains("integrations-platform"), "got: {reason}");
        assert!(reason.contains("marketing"), "got: {reason}");
    }

    #[test]
    fn wildcard_team_matches_any_team_actor() {
        let d = evaluate(
            &bi_spec(),
            "team:anybody",
            "internal_viewer",
            "bank_integration:bi_abc",
        );
        assert!(d.is_allow());
    }

    #[test]
    fn wildcard_team_does_not_match_role_actor() {
        let d = evaluate(
            &bi_spec(),
            "role:dpo",
            "internal_viewer",
            "bank_integration:bi_abc",
        );
        assert!(!d.is_allow());
    }

    #[test]
    fn object_type_mismatch_is_denied() {
        let d = evaluate(
            &bi_spec(),
            "team:integrations-platform",
            "owner",
            "account:acc_xyz",
        );
        assert!(!d.is_allow());
    }

    #[test]
    fn missing_relation_is_denied() {
        let d = evaluate(
            &bi_spec(),
            "team:integrations-platform",
            "approver",
            "bank_integration:bi_abc",
        );
        assert!(!d.is_allow());
        let r = d.reason();
        assert!(r.contains("no `approver` tuple"), "got: {r}");
    }

    #[test]
    fn snake_case_basic() {
        assert_eq!(snake_case("BankIntegration"), "bank_integration");
        assert_eq!(snake_case("Account"), "account");
        assert_eq!(snake_case("AuthenticationMethod"), "authentication_method");
    }

    #[test]
    fn spec_for_concept_builds_owner_tuple() {
        use crate::seed::baseline_concepts;
        let cards = baseline_concepts();
        let bi = cards
            .iter()
            .find(|c| c.fqn == "core.integrations.BankIntegration")
            .unwrap();
        let spec = spec_for_concept(bi);
        let d = evaluate(
            &spec,
            "team:integrations-platform",
            "owner",
            "bank_integration:bi_foo",
        );
        assert!(d.is_allow(), "{:?}", d);
        let d2 = evaluate(
            &spec,
            "team:marketing",
            "owner",
            "bank_integration:bi_foo",
        );
        assert!(!d2.is_allow());
    }
}
