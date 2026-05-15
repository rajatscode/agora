//! Composition axis — does the ontology still compose after this change?
//!
//! Checks:
//!   - For AddRelation: the `to` type exists in the seed catalog
//!   - For TightenField / Deprecate / Reclassify: target field actually exists
//!   - For CreateType: the new FQN doesn't collide with a catalog entry
//!
//! "Composes" is a deliberately conservative read here — at M0 the registry
//! catalog is the seed. The intent is to catch the obvious own-goal (rename
//! typo'd, FK points at nothing) without pretending we've done a full type
//! check across all generated artifacts.

use crate::ast::{Change, OntologyChangeProposal};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use crate::seed::ConceptCard;
use serde_json::json;
use std::time::Instant;

pub fn run(proposal: &OntologyChangeProposal, catalog: &[ConceptCard]) -> CheckRow {
    let started = Instant::now();
    let mut issues: Vec<String> = Vec::new();
    let target = proposal.target().fqn();

    let target_card = catalog.iter().find(|c| c.fqn == target);

    match &proposal.change {
        Change::AddRelation { relation } => {
            let from_fqn = relation.from.fqn();
            let to_fqn = relation.to.fqn();
            if !catalog.iter().any(|c| c.fqn == from_fqn) {
                issues.push(format!(
                    "Relation source `{}` not present in registry catalog.",
                    from_fqn
                ));
            }
            if !catalog.iter().any(|c| c.fqn == to_fqn) {
                issues.push(format!(
                    "Relation target `{}` not present in registry catalog.",
                    to_fqn
                ));
            }
        }
        Change::DeprecateField {
            type_ref,
            field_name,
        }
        | Change::TightenField {
            type_ref,
            field_name,
            ..
        }
        | Change::ReclassifyField {
            type_ref,
            field_name,
            ..
        } => {
            match catalog.iter().find(|c| c.fqn == type_ref.fqn()) {
                None => issues.push(format!(
                    "Target type `{}` not present in registry.",
                    type_ref.fqn()
                )),
                Some(card) => {
                    if !card.spec.fields.iter().any(|f| &f.name == field_name) {
                        issues.push(format!(
                            "Field `{}` does not exist on `{}`.",
                            field_name,
                            type_ref.fqn()
                        ));
                    }
                }
            }
        }
        Change::CreateType { spec } => {
            let new_fqn = format!("{}.{}", spec.namespace, spec.name);
            if catalog.iter().any(|c| c.fqn == new_fqn) {
                issues.push(format!(
                    "Type `{}` already exists in catalog; CreateType would collide.",
                    new_fqn
                ));
            }
        }
        Change::AddField { field, .. } => {
            match target_card {
                // Target concept must exist — adding a field to a phantom
                // concept is exactly the "agora waves through `core.fake.Foo`"
                // failure mode that erodes the canonical-concepts story.
                None => issues.push(format!(
                    "Target concept `{}` not in catalog; cannot add a field to a non-existent type.",
                    target
                )),
                Some(card) => {
                    if card.spec.fields.iter().any(|f| f.name == field.name) {
                        issues.push(format!(
                            "Field `{}` already exists on `{}`; should use TightenField or rename.",
                            field.name, target
                        ));
                    }
                }
            }
        }
    }

    let outcome = if issues.is_empty() {
        Outcome::Pass
    } else {
        Outcome::Fail
    };
    let findings = if issues.is_empty() {
        "Ontology composes after this change (no dangling references or collisions).".into()
    } else {
        issues.join(" | ")
    };

    CheckRow {
        axis: Axis::Composition,
        outcome,
        findings,
        evidence: json!({
            "target": target,
            "issues": issues,
            "catalog_size": catalog.len(),
        }),
        confidence: Confidence::High,
        source: "deterministic".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}
