//! Auto-approval threshold logic — Beat 5's gate.
//!
//! A proposal is `auto_approval_eligible` iff:
//!   1. Every axis row outcome is Pass or Advisory (no Fail).
//!   2. Data-conformance outcome is Pass (or not-applicable).
//!   3. The proposal's *classified* shape (from the shape axis, NOT the
//!      LLM author's claim) is Additive or Refinement.
//!   4. The semantic axis verdict is not Breaking/Dangerous.
//!   5. The policy axis didn't surface a downgrade (Fail covers this; the
//!      check above already catches it).
//!
//! When the proposal is NOT eligible, we populate `block_reason` with a
//! short, demo-grade explanation that the runtime + Beat 6 narrative can
//! quote verbatim.
//!
//! Note: Advisory rows do not block auto-approval — by design, they're
//! informational signals (Impact axis, e.g.). Only Fail blocks.

use crate::ast::OntologyChangeProposal;
use crate::check_report::{Axis, CheckReport, Outcome};

pub fn apply(report: &mut CheckReport, _proposal: &OntologyChangeProposal) {
    let mut blockers: Vec<String> = Vec::new();

    for row in &report.checks {
        if matches!(row.outcome, Outcome::Fail) {
            blockers.push(format!("{}: {}", row.axis.as_str(), row.findings));
        }
    }

    if matches!(report.data_conformance.outcome, Outcome::Fail) {
        let count = report.data_conformance.violations_found;
        blockers.push(format!(
            "data_conformance: {} existing row(s) violate the proposed constraint",
            count
        ));
    }
    // If the proposal could in principle invalidate data but we never got
    // a real query result, we MUST NOT auto-approve — silently waving a
    // tighten through without a DB check is exactly the Beat-6 failure mode.
    if report.data_conformance.applicable
        && matches!(report.data_conformance.outcome, Outcome::Skipped)
    {
        blockers.push(format!(
            "data_conformance: applicable but unverified ({})",
            report.data_conformance.source
        ));
    }

    if blockers.is_empty() {
        report.status = "approved".into();
        report.auto_approval_eligible = true;
        report.block_reason = None;
    } else {
        report.status = "blocked".into();
        report.auto_approval_eligible = false;
        report.block_reason = Some(blockers.join(" | "));
    }
}

/// Returns true iff the report indicates the proposal can auto-merge.
pub fn is_eligible(report: &CheckReport) -> bool {
    report.auto_approval_eligible
}

/// Convenience: shape axis says additive/refinement.
pub fn shape_classified_as(report: &CheckReport) -> Option<String> {
    report
        .checks
        .iter()
        .find(|c| c.axis == Axis::Shape)
        .and_then(|c| c.evidence.get("classified_as").cloned())
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}
