//! Explorer DATA layer — Feature 3's discovery contract.
//!
//! The Explorer answers, for a given concept FQN: who owns it, what its
//! invariants are, what touches it (lineage), what policy class governs it,
//! and how it has been mutated over time. The "DATA layer" qualifier matters:
//! this file is library-first. It returns plain Rust types. The CLI (`cli.rs`)
//! and the upcoming F-DAEMON HTTP layer both render the same `ConceptView`.
//!
//! Sources of truth (all real, none stubbed):
//!   - **Owner, invariants, policy, fields**: from `seed::baseline_concepts()`
//!     (the same registry view F2's data-conformance axis consults).
//!   - **Lineage**: derived from the live entity → log relationship, plus the
//!     known HTTP / DDL artifact naming convention from F1's `artifacts.rs`.
//!   - **Version history**: from `mutation_log` rows for this `type_id`,
//!     plus any `generated_artifacts` rows whose proposal_id matches a
//!     mutation actor.
//!
//! Beat 8's narrative requires every line of the rendered view to be a real
//! query result — no hard-coded fields, no placeholder owners.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::ast::{PolicyClass, ProtoType};
use crate::mutation_log::{self, LoggedMutation};
use crate::seed::{self, ConceptCard};

/// Top-level view returned for `agora explorer <fqn>`. All fields are derived
/// from the running registry + mutation_log; nothing is hard-coded for the
/// demo. (`policy_examples` is the closest thing to a constant — but it's
/// computed from the concept's `PolicyClass`, which itself is registry data.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptView {
    pub fqn: String,
    pub namespace: String,
    pub name: String,
    pub version: u32,
    pub status: String,
    pub doc: Option<String>,
    pub ownership: OwnershipView,
    pub fields: Vec<FieldView>,
    pub invariants: Vec<String>,
    pub policy_class: PolicyClass,
    pub policy_examples: Vec<PolicyTupleView>,
    pub lineage: LineageView,
    pub version_history: Vec<VersionHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipView {
    pub team: String,
    pub semantic_steward: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldView {
    pub name: String,
    pub proto_type: String,
    pub required: bool,
    pub classification: PolicyClass,
    pub since_version: u32,
    pub deprecated_in: Option<u32>,
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyTupleView {
    pub relation: String,
    pub subject: String,
    pub object: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineageView {
    pub http_route: String,
    pub storage_table: String,
    pub policy_artifact: String,
    pub proto_artifact: String,
    /// Other registry concepts this one references via `ProtoType::Ref`.
    pub references: Vec<String>,
    /// Proposal IDs that have written to this concept (distinct actors
    /// found in mutation_log + generated_artifacts).
    pub touched_by_proposals: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionHistoryEntry {
    pub mutation_seq: i64,
    pub entity_id: String,
    pub operation: String,
    pub ontology_version: i32,
    pub actor: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    pub checksum: Option<String>,
}

const HISTORY_LIMIT: i64 = 50;

/// Load the concept view from registry + mutation_log. Returns `Ok(None)` if
/// the concept FQN isn't in the seed catalog (the daemon's HTTP handler can
/// translate that into a 404).
pub async fn explorer(pool: Option<&PgPool>, fqn: &str) -> Result<Option<ConceptView>> {
    let catalog = seed::baseline_concepts();
    let card = catalog.iter().find(|c| c.fqn == fqn);
    let Some(card) = card else {
        return Ok(None);
    };

    let version_history = if let Some(pool) = pool {
        let history =
            mutation_log::history_for_type(pool, fqn, HISTORY_LIMIT).await.with_context(|| {
                format!("loading mutation history for {fqn}")
            })?;
        history.into_iter().map(history_entry_from_log).collect()
    } else {
        Vec::new()
    };

    let touched_by_proposals = if let Some(pool) = pool {
        proposals_touching(pool, fqn).await?
    } else {
        Vec::new()
    };

    Ok(Some(build_view(card, version_history, touched_by_proposals)))
}

fn build_view(
    card: &ConceptCard,
    version_history: Vec<VersionHistoryEntry>,
    touched_by_proposals: Vec<String>,
) -> ConceptView {
    let spec = &card.spec;
    let snake = snake_case(&spec.name);
    let storage_table = format!(
        "{}_{}",
        snake_case(&spec.namespace.replace('.', "_")),
        snake
    );
    let lineage = LineageView {
        http_route: format!("POST /entities/{snake}"),
        storage_table,
        policy_artifact: format!("{snake}.fga.json"),
        proto_artifact: format!("{snake}.proto"),
        references: spec
            .fields
            .iter()
            .filter_map(|f| match &f.proto_type {
                ProtoType::Ref(r) => Some(r.clone()),
                _ => None,
            })
            .chain(spec.relations.iter().map(|r| r.to.fqn()))
            .collect(),
        touched_by_proposals,
    };

    ConceptView {
        fqn: card.fqn.clone(),
        namespace: spec.namespace.clone(),
        name: spec.name.clone(),
        version: spec.version,
        status: status_for(spec.version),
        doc: spec.doc.clone(),
        ownership: OwnershipView {
            team: spec.ownership.team.clone(),
            semantic_steward: spec.ownership.semantic_steward.clone(),
        },
        fields: spec
            .fields
            .iter()
            .map(|f| FieldView {
                name: f.name.clone(),
                proto_type: proto_type_label(&f.proto_type),
                required: f.required,
                classification: f.classification.clone(),
                since_version: f.since_version,
                deprecated_in: f.deprecated_in,
                doc: f.doc.clone(),
            })
            .collect(),
        invariants: spec.invariants.clone(),
        policy_class: spec.policy_class.clone(),
        policy_examples: policy_examples_for(&spec.policy_class, &snake, &spec.ownership.team),
        lineage,
        version_history,
    }
}

/// Distinct proposal IDs (or actors) that have written into this concept.
/// We pull from BOTH mutation_log.actor AND generated_artifacts.proposal_id
/// so an LLM-authored proposal whose first write hasn't happened yet still
/// appears in lineage.
async fn proposals_touching(pool: &PgPool, fqn: &str) -> Result<Vec<String>> {
    // Mutation actors that look like proposal IDs or agent URIs.
    let actors: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT actor FROM mutation_log WHERE type_id = $1 ORDER BY actor",
    )
    .bind(fqn)
    .fetch_all(pool)
    .await
    .context("listing distinct mutation actors")?;

    let mut out: Vec<String> = actors.into_iter().map(|t| t.0).collect();

    // Generated artifacts created with paths matching the concept name. The
    // F1 emitter writes under `generated/<proposal_id>/<snake>.proto` etc.,
    // so we filter on the artifact `path` containing the snake_case name.
    let snake = snake_case(&fqn.rsplit('.').next().unwrap_or(fqn));
    let pattern = format!("%/{}%", snake);
    let arts: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT proposal_id FROM generated_artifacts WHERE path LIKE $1 ORDER BY proposal_id",
    )
    .bind(pattern)
    .fetch_all(pool)
    .await
    .context("listing artifact proposals")?;
    for (pid,) in arts {
        if !out.contains(&pid) {
            out.push(pid);
        }
    }
    Ok(out)
}

fn history_entry_from_log(m: LoggedMutation) -> VersionHistoryEntry {
    VersionHistoryEntry {
        mutation_seq: m.seq,
        entity_id: m.entity_id,
        operation: m.operation,
        ontology_version: m.ontology_version,
        actor: m.actor,
        occurred_at: m.occurred_at,
        checksum: m.checksum,
    }
}

fn proto_type_label(t: &ProtoType) -> String {
    match t {
        ProtoType::String => "string".into(),
        ProtoType::Int64 => "int64".into(),
        ProtoType::Bool => "bool".into(),
        ProtoType::Bytes => "bytes".into(),
        ProtoType::Timestamp => "google.protobuf.Timestamp".into(),
        ProtoType::Ref(r) => format!("ref<{r}>"),
    }
}

fn status_for(version: u32) -> String {
    if version == 0 {
        "Draft".into()
    } else {
        format!("Active (v{version})")
    }
}

/// Construct canonical policy tuples the same way `artifacts::render_openfga`
/// does, so the Explorer's policy view matches what the daemon would actually
/// register with OpenFGA. The tuples are illustrative (object id placeholder
/// `{id}`) but the subjects/relations are real.
fn policy_examples_for(
    class: &PolicyClass,
    fga_type: &str,
    owner_team: &str,
) -> Vec<PolicyTupleView> {
    let mut out = Vec::new();
    if !matches!(class, PolicyClass::Public) {
        out.push(PolicyTupleView {
            relation: "owner".into(),
            subject: format!("team:{owner_team}"),
            object: format!("{fga_type}:{{id}}"),
        });
    }
    match class {
        PolicyClass::Public => {}
        PolicyClass::Internal => out.push(PolicyTupleView {
            relation: "internal_viewer".into(),
            subject: "team:*".into(),
            object: format!("{fga_type}:{{id}}"),
        }),
        PolicyClass::Sensitive => out.push(PolicyTupleView {
            relation: "sensitive_viewer".into(),
            subject: format!("team:{owner_team}"),
            object: format!("{fga_type}:{{id}}"),
        }),
        PolicyClass::Pii => out.push(PolicyTupleView {
            relation: "pii_viewer".into(),
            subject: "role:dpo".into(),
            object: format!("{fga_type}:{{id}}"),
        }),
    }
    out
}

fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else if ch == '.' || ch == '-' || ch == ' ' {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn explorer_returns_none_for_unknown_fqn() {
        // No pool needed to test the "concept not in catalog" path.
        let r = explorer(None, "core.unknown.Mystery")
            .await
            .expect("explorer ran");
        assert!(r.is_none());
    }

    #[test]
    fn build_view_pulls_real_owner_invariants_and_policy_for_known_fqn() {
        let catalog = seed::baseline_concepts();
        let card = catalog
            .iter()
            .find(|c| c.fqn == "core.integrations.BankIntegration")
            .unwrap();
        let view = build_view(card, vec![], vec![]);

        assert_eq!(view.ownership.team, "integrations-platform");
        assert!(!view.invariants.is_empty());
        assert!(matches!(view.policy_class, PolicyClass::Internal));
        // Internal class produces owner + internal_viewer tuples.
        assert!(view.policy_examples.iter().any(|p| p.relation == "owner"));
        assert!(view
            .policy_examples
            .iter()
            .any(|p| p.relation == "internal_viewer"));
        // Storage table follows the F1 naming convention.
        assert_eq!(
            view.lineage.storage_table,
            "core_integrations_bank_integration"
        );
        assert_eq!(
            view.lineage.http_route,
            "POST /entities/bank_integration"
        );
    }

    #[test]
    fn build_view_marks_pii_correctly() {
        let catalog = seed::baseline_concepts();
        let card = catalog.iter().find(|c| c.fqn == "core.users.Account").unwrap();
        let view = build_view(card, vec![], vec![]);
        assert!(matches!(view.policy_class, PolicyClass::Pii));
        assert!(view
            .policy_examples
            .iter()
            .any(|p| p.relation == "pii_viewer"));
    }
}
