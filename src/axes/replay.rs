//! Replay axis — can projections rebuild after the change?
//!
//! M0 fact-log is append-only with `ontology_version` stamped on every row.
//! A proposal threatens replay only when it changes how historical payloads
//! are interpreted:
//!   - DeprecateField: old rows still carry the field; projections must
//!     handle nulls or fallback (advisory)
//!   - TightenField: old rows may carry NULL for what is now a required
//!     field — projections that assume non-NULL will break (advisory)
//!   - ReclassifyField: visibility changes don't break replay shape
//!   - AddField/AddRelation/CreateType: no replay risk
//!
//! Beat 3 expects this axis to render real findings, not constant "pass".
//! When applicable we provide structured evidence so Beat 8's explorer can
//! show "this proposal would have required X for full replay".

use crate::ast::{Change, OntologyChangeProposal};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use serde_json::json;
use std::time::Instant;

pub fn run(proposal: &OntologyChangeProposal) -> CheckRow {
    let started = Instant::now();

    let (outcome, headline, note): (Outcome, &'static str, Option<String>) =
        match &proposal.change {
            Change::DeprecateField {
                type_ref,
                field_name,
            } => (
                Outcome::Advisory,
                "Deprecation: existing log rows still carry this field; projections must tolerate it.",
                Some(format!(
                    "Projections rebuilding from mutation_log will see `{}.{}` in historical rows; \
                     ensure consumers ignore-unknown rather than panic.",
                    type_ref.fqn(),
                    field_name
                )),
            ),
            Change::TightenField {
                type_ref,
                field_name,
                from_required,
                to_required,
            } if !from_required && *to_required => (
                Outcome::Advisory,
                "Tighten: historical log rows may carry NULL for the now-required field.",
                Some(format!(
                    "Replays of mutation_log will encounter rows where `{}.{}` was NULL when written. \
                     Projections must either backfill before tightening or be updated to handle NULL.",
                    type_ref.fqn(),
                    field_name
                )),
            ),
            _ => (
                Outcome::Pass,
                "No replay risk: change is purely additive or scoped to metadata.",
                None,
            ),
        };

    CheckRow {
        axis: Axis::Replay,
        outcome,
        findings: headline.to_string(),
        evidence: json!({ "note": note }),
        confidence: Confidence::High,
        source: "deterministic".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}
