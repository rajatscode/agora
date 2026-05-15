//! Shape axis — classifies the structural delta as additive / refinement /
//! breaking / dangerous based on the `Change` variant.
//!
//! This is the cheap, deterministic axis. The classification is straightforward:
//! - AddField, AddRelation, CreateType   → additive
//! - DeprecateField, ReclassifyField     → refinement
//! - TightenField (optional→required)    → refinement (semantic axis decides
//!                                           whether it's *actually* safe
//!                                           given the data)
//!
//! We deliberately do NOT trust `proposal.compatibility.shape` from the
//! authoring step — that field is the LLM's claim, not a check. The shape
//! axis is the check.

use crate::ast::{Change, CompatibilityClass, OntologyChangeProposal};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use serde_json::json;
use std::time::Instant;

pub fn run(proposal: &OntologyChangeProposal) -> CheckRow {
    let started = Instant::now();
    let (class, headline) = classify(&proposal.change);

    let outcome = match class {
        CompatibilityClass::Additive | CompatibilityClass::Refinement => Outcome::Pass,
        CompatibilityClass::Breaking | CompatibilityClass::Dangerous => Outcome::Fail,
    };

    let declared = &proposal.compatibility.shape;
    let mismatch = declared != &class;

    let findings = if mismatch {
        format!(
            "Shape classified as {:?}; proposal declared {:?}. Trust the check.",
            class, declared
        )
    } else {
        headline.to_string()
    };

    CheckRow {
        axis: Axis::Shape,
        outcome,
        findings,
        evidence: json!({
            "classified_as": format!("{:?}", class).to_lowercase(),
            "proposal_declared": format!("{:?}", declared).to_lowercase(),
            "mismatch_with_declared": mismatch,
        }),
        confidence: Confidence::High,
        source: "deterministic".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

/// Returns the shape class and a one-line headline describing the change.
fn classify(change: &Change) -> (CompatibilityClass, &'static str) {
    match change {
        Change::AddField { field, .. } => (
            if field.required {
                // Adding a brand-new required field to an existing concept is
                // a refinement, not a pure additive — existing rows have no
                // value and need a default or backfill. Semantic axis catches
                // it again; the data-conformance axis won't because there's
                // no row to check yet (the column doesn't exist).
                CompatibilityClass::Refinement
            } else {
                CompatibilityClass::Additive
            },
            "Adds a new field (no existing rows are invalidated).",
        ),
        Change::AddRelation { .. } => (
            CompatibilityClass::Additive,
            "Adds a new relation (existing entities remain valid).",
        ),
        Change::CreateType { .. } => (
            CompatibilityClass::Additive,
            "Introduces a new type (no existing data affected).",
        ),
        Change::DeprecateField { .. } => (
            CompatibilityClass::Refinement,
            "Deprecates an existing field (retirement window before removal).",
        ),
        Change::ReclassifyField { .. } => (
            CompatibilityClass::Refinement,
            "Reclassifies a field's policy class (visibility/sensitivity shift).",
        ),
        Change::TightenField {
            from_required,
            to_required,
            ..
        } => {
            if !from_required && *to_required {
                (
                    CompatibilityClass::Refinement,
                    "Tightens a field (optional → required); data axis must clear.",
                )
            } else {
                (
                    CompatibilityClass::Refinement,
                    "Adjusts a field's required-ness.",
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TypeRef;

    fn type_ref() -> TypeRef {
        TypeRef {
            namespace: "core.users".into(),
            name: "Account".into(),
        }
    }

    #[test]
    fn add_optional_field_is_additive() {
        let p = test_proposal_add_optional();
        let row = run(&p);
        assert!(matches!(row.outcome, Outcome::Pass));
        let v = row.evidence;
        assert_eq!(v["classified_as"], "additive");
    }

    #[test]
    fn tighten_optional_to_required_is_refinement() {
        let p = test_proposal_tighten();
        let row = run(&p);
        assert!(matches!(row.outcome, Outcome::Pass));
        assert_eq!(row.evidence["classified_as"], "refinement");
    }

    fn test_proposal_add_optional() -> OntologyChangeProposal {
        use crate::ast::*;
        OntologyChangeProposal {
            id: "prop_test".into(),
            domain: "users".into(),
            namespace: "core.users".into(),
            change_intent: "add nickname".into(),
            rationale: "for display".into(),
            change: Change::AddField {
                type_ref: type_ref(),
                field: Field {
                    name: "nickname".into(),
                    proto_type: ProtoType::String,
                    proto_number: 9,
                    required: false,
                    since_version: 2,
                    deprecated_in: None,
                    classification: PolicyClass::Internal,
                    doc: None,
                },
            },
            semantic_contract: SemanticContract {
                meaning_before: "n/a".into(),
                meaning_after: "Account has an optional nickname.".into(),
                justification: None,
                invariants: vec!["nickname is optional".into(), "no policy change".into()],
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
                model: "test".into(),
                generated_at: "now".into(),
                trace_id: None,
            },
        }
    }

    fn test_proposal_tighten() -> OntologyChangeProposal {
        let mut p = test_proposal_add_optional();
        p.change = crate::ast::Change::TightenField {
            type_ref: type_ref(),
            field_name: "email".into(),
            from_required: false,
            to_required: true,
        };
        p
    }
}
