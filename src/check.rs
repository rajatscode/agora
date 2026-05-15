//! Multi-axis risk gate — the orchestrator.
//!
//! Public entry point: `check(proposal, &catalog, db).await -> CheckReport`.
//! Runs every axis in `crate::axes`, packages the rows into a `CheckReport`,
//! applies the auto-approval threshold, and returns the verdict.
//!
//! Order of evaluation (matches FEATURE-2-SPEC.md):
//!   1. Composition           — does the ontology still compose?
//!   2. Shape                 — structural delta classification
//!   3. Semantic              — LLM reasoning over meaning_before/after
//!   4. Policy                — visibility / classification boundaries
//!   5. Temporal              — time-semantics changes
//!   6. Impact                — downstream artifacts affected (informational)
//!   7. Data-conformance      — real SQL against the live DB (Beat 6)
//!   8. Replay                — projection rebuildability (informational)
//!
//! The semantic axis runs asynchronously (LLM call). Everything else is
//! synchronous and fast. We don't parallelize today — the sequential trace
//! is helpful for the demo (each axis prints to stderr as it fires).

use crate::ast::OntologyChangeProposal;
use crate::auto_approval;
use crate::axes;
use crate::check_report::{Axis, CheckReport, Outcome};
use crate::seed::ConceptCard;
use anyhow::Result;
use sqlx::PgPool;
use std::time::Instant;

pub async fn check(
    proposal: &OntologyChangeProposal,
    catalog: &[ConceptCard],
    db: Option<&PgPool>,
) -> Result<CheckReport> {
    let started = Instant::now();
    let mut report = CheckReport::new(proposal.id.clone());

    tracing::info!("axis: composition");
    report.checks.push(axes::composition::run(proposal, catalog));

    tracing::info!("axis: shape");
    report.checks.push(axes::shape::run(proposal));

    tracing::info!("axis: semantic (LLM)");
    report.checks.push(axes::semantic::run(proposal, catalog).await);

    tracing::info!("axis: policy");
    report.checks.push(axes::policy::run(proposal, catalog));

    tracing::info!("axis: temporal");
    report.checks.push(axes::temporal::run(proposal, catalog));

    tracing::info!("axis: impact");
    report.checks.push(axes::impact::run(proposal, catalog));

    tracing::info!("axis: data-conformance");
    report.data_conformance = axes::data_conformance::run(proposal, catalog, db).await;

    tracing::info!("axis: replay");
    report.checks.push(axes::replay::run(proposal));

    // Apply auto-approval threshold and final status.
    auto_approval::apply(&mut report, proposal);

    report.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// Convenience for callers that don't want to import Outcome.
pub fn count_failures(report: &CheckReport) -> usize {
    let axes_failed = report
        .checks
        .iter()
        .filter(|c| matches!(c.outcome, Outcome::Fail))
        .count();
    let dc_failed = if matches!(report.data_conformance.outcome, Outcome::Fail) {
        1
    } else {
        0
    };
    axes_failed + dc_failed
}

/// Lookup helper: returns the axis row by axis kind, if present.
pub fn axis_row(report: &CheckReport, axis: Axis) -> Option<&crate::check_report::CheckRow> {
    report.checks.iter().find(|r| r.axis == axis)
}
