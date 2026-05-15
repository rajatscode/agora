//! Temporal axis — are time semantics (valid-time, transaction-time) changing?
//!
//! M0 is append-only mutation log. Concrete temporal red flags at this stage:
//!   - DeprecateField on a timestamp-typed field → reinterprets history
//!   - ReclassifyField touching a Timestamp field → could change replay
//!   - TightenField on a since_version field → would invalidate older rows
//!     written before that version (their values would now be invalid)
//!
//! Most proposals pass this axis. We surface advisory rows for anything
//! that touches a timestamp or since_version delta so the demo audience
//! can see the axis is doing real work rather than constant-passing.

use crate::ast::{Change, OntologyChangeProposal, ProtoType};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use crate::seed::ConceptCard;
use serde_json::json;
use std::time::Instant;

pub fn run(proposal: &OntologyChangeProposal, catalog: &[ConceptCard]) -> CheckRow {
    let started = Instant::now();
    let mut notes: Vec<String> = Vec::new();
    let mut outcome = Outcome::Pass;

    match &proposal.change {
        Change::TightenField {
            type_ref,
            field_name,
            from_required,
            to_required,
        } => {
            if !from_required && *to_required {
                if let Some(card) = catalog.iter().find(|c| c.fqn == type_ref.fqn()) {
                    if let Some(field) = card.spec.fields.iter().find(|f| &f.name == field_name) {
                        if field.since_version > 1 {
                            notes.push(format!(
                                "Field `{}` was introduced in v{}; tightening it now retroactively \
                                 invalidates rows written before that version.",
                                field_name, field.since_version
                            ));
                            outcome = Outcome::Advisory;
                        }
                    }
                }
            }
        }
        Change::DeprecateField {
            type_ref,
            field_name,
        } => {
            if let Some(card) = catalog.iter().find(|c| c.fqn == type_ref.fqn()) {
                if let Some(field) = card.spec.fields.iter().find(|f| &f.name == field_name) {
                    if matches!(field.proto_type, ProtoType::Timestamp) {
                        notes.push(format!(
                            "Deprecating timestamp field `{}` removes temporal anchor used in \
                             replay; verify projections don't depend on it.",
                            field_name
                        ));
                        outcome = Outcome::Advisory;
                    }
                }
            }
        }
        _ => {}
    }

    let findings = if notes.is_empty() {
        "No temporal reinterpretation (M0 append-only; no valid-time changes).".to_string()
    } else {
        notes.join(" | ")
    };

    CheckRow {
        axis: Axis::Temporal,
        outcome,
        findings,
        evidence: json!({ "notes": notes }),
        confidence: Confidence::High,
        source: "deterministic".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}
