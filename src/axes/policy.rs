//! Policy axis — are access-control boundaries changing?
//!
//! Per STACK.md (Artifact 3: classification-propagation contract):
//!   Public < Internal < Sensitive < Pii    (in increasing restrictiveness)
//!
//! Rules at M0:
//!   - Downgrade (Pii → Internal, Sensitive → Public, etc.) → BLOCK
//!     (we cannot widen visibility without security sign-off)
//!   - Upgrade (Internal → Pii, etc.) → ADVISORY (more restrictive = safer)
//!   - AddField with class Pii or Sensitive → ADVISORY (data steward should
//!     be aware; downstream OpenFGA tuples will be emitted)
//!   - Anything else → PASS
//!
//! This mirrors `no_sensitivity_downgrade.json` in STACK.md's policy DSL —
//! the JSON evaluator pod (other workstream) will replace this in-Rust check
//! later. For F2 we keep it explicit and deterministic.

use crate::ast::{Change, OntologyChangeProposal, PolicyClass};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use crate::seed::ConceptCard;
use serde_json::json;
use std::time::Instant;

fn class_rank(c: &PolicyClass) -> u8 {
    match c {
        PolicyClass::Public => 0,
        PolicyClass::Internal => 1,
        PolicyClass::Sensitive => 2,
        PolicyClass::Pii => 3,
    }
}

pub fn run(proposal: &OntologyChangeProposal, catalog: &[ConceptCard]) -> CheckRow {
    let started = Instant::now();

    let (outcome, findings, evidence) = match &proposal.change {
        Change::ReclassifyField {
            type_ref,
            field_name,
            to,
        } => {
            // Look up the field's current class in the catalog.
            let card = catalog.iter().find(|c| c.fqn == type_ref.fqn());
            let from = card
                .and_then(|c| c.spec.fields.iter().find(|f| &f.name == field_name))
                .map(|f| f.classification.clone());
            match from {
                Some(from_class) => {
                    let from_rank = class_rank(&from_class);
                    let to_rank = class_rank(to);
                    if to_rank < from_rank {
                        (
                            Outcome::Fail,
                            format!(
                                "Reclassify {}.{} downgrades visibility: {:?} → {:?}. \
                                 Requires security-owner approval (rule: no_sensitivity_downgrade).",
                                type_ref.fqn(),
                                field_name,
                                from_class,
                                to
                            ),
                            json!({
                                "rule": "no_sensitivity_downgrade",
                                "field": format!("{}.{}", type_ref.fqn(), field_name),
                                "from": format!("{:?}", from_class),
                                "to":   format!("{:?}", to),
                                "direction": "downgrade",
                            }),
                        )
                    } else {
                        (
                            Outcome::Advisory,
                            format!(
                                "Reclassify {}.{} upgrades visibility: {:?} → {:?}.",
                                type_ref.fqn(),
                                field_name,
                                from_class,
                                to
                            ),
                            json!({
                                "rule": "no_sensitivity_downgrade",
                                "field": format!("{}.{}", type_ref.fqn(), field_name),
                                "from": format!("{:?}", from_class),
                                "to":   format!("{:?}", to),
                                "direction": "upgrade",
                            }),
                        )
                    }
                }
                None => (
                    Outcome::Advisory,
                    format!(
                        "Reclassify {}.{} → {:?} (prior class unknown; check unverified).",
                        type_ref.fqn(),
                        field_name,
                        to
                    ),
                    json!({
                        "rule": "no_sensitivity_downgrade",
                        "field": format!("{}.{}", type_ref.fqn(), field_name),
                        "to":   format!("{:?}", to),
                        "direction": "unknown",
                    }),
                ),
            }
        }
        Change::AddField { field, .. }
            if matches!(field.classification, PolicyClass::Pii | PolicyClass::Sensitive) =>
        {
            (
                Outcome::Advisory,
                format!(
                    "Adds a new {:?}-classified field; ensure DPO/security review of the artifact set.",
                    field.classification
                ),
                json!({
                    "rule": "pii_requires_dpo_approval",
                    "field": field.name.clone(),
                    "classification": format!("{:?}", field.classification),
                }),
            )
        }
        _ => (
            Outcome::Pass,
            "No policy-class boundary changes.".to_string(),
            json!({"rule": "additive_field_auto_approve"}),
        ),
    };

    CheckRow {
        axis: Axis::Policy,
        outcome,
        findings,
        evidence,
        confidence: Confidence::High,
        source: "deterministic".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}
