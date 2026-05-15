//! CheckReport — Feature 2's output artifact.
//!
//! A CheckReport is the structured verdict the risk gate produces for a
//! single proposal. Seven axes (composition / shape / semantic / policy /
//! temporal / impact / replay) each emit a row; a separate data-conformance
//! block carries violation counts + sample rows. The combined picture decides
//! whether the proposal is auto-approval-eligible.
//!
//! Beat 3 of the demo renders this report. Beat 5 reads `auto_approval_eligible`
//! and merges. Beat 6 reads `data_conformance.violations_found` and blocks
//! with `block_reason` cited verbatim.

use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Composition,
    Shape,
    Semantic,
    Policy,
    Temporal,
    Impact,
    Replay,
    DataConformance,
}

impl Axis {
    pub fn as_str(&self) -> &'static str {
        match self {
            Axis::Composition => "composition",
            Axis::Shape => "shape",
            Axis::Semantic => "semantic",
            Axis::Policy => "policy",
            Axis::Temporal => "temporal",
            Axis::Impact => "impact",
            Axis::Replay => "replay",
            Axis::DataConformance => "data_conformance",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Axis passed cleanly.
    Pass,
    /// Axis surfaced advisory information but does not block.
    Advisory,
    /// Axis blocks the proposal.
    Fail,
    /// Axis was skipped (e.g. no DB available); does not block.
    Skipped,
}

impl Outcome {
    pub fn is_failure(&self) -> bool {
        matches!(self, Outcome::Fail)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Verdict is grounded in deterministic logic or in a live LLM call.
    High,
    /// Verdict is heuristic — e.g. LLM call failed and we fell back to rules,
    /// or the upstream proposal was authored offline. Downstream consumers
    /// must NOT treat as authoritative.
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRow {
    pub axis: Axis,
    pub outcome: Outcome,
    /// Short, human-readable headline (one line).
    pub findings: String,
    /// Free-form structured evidence; varies per axis.
    #[serde(default)]
    pub evidence: serde_json::Value,
    /// How confident we are in this verdict — see Confidence.
    pub confidence: Confidence,
    /// Where the verdict came from. "deterministic" | "anthropic:<model>" |
    /// "offline-fallback" | "n/a".
    pub source: String,
    /// Milliseconds spent on this axis. Helps demo + perf budgets.
    pub elapsed_ms: u64,
}

impl CheckRow {
    pub fn pass(axis: Axis, findings: impl Into<String>) -> Self {
        Self {
            axis,
            outcome: Outcome::Pass,
            findings: findings.into(),
            evidence: serde_json::Value::Null,
            confidence: Confidence::High,
            source: "deterministic".into(),
            elapsed_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataConformance {
    /// Whether the proposal even *could* invalidate existing data (e.g. a
    /// pure additive AddField never does). If false the rest of the fields
    /// are zeros and `outcome` is `Pass`.
    pub applicable: bool,
    /// Outcome of the check (Pass | Fail | Skipped).
    pub outcome: Outcome,
    /// Number of rows that would violate the proposed constraint.
    pub violations_found: i64,
    /// Sample of violating rows. Capped at 5 to keep the report compact.
    pub sample_violations: Vec<SampleViolation>,
    /// The SQL we ran to check (so the audit trail is reproducible).
    pub query: Option<String>,
    /// Wall-clock time of the actual DB query.
    pub query_time_ms: u64,
    /// Source of the count: "postgres" if real, "skipped:<reason>" otherwise.
    pub source: String,
}

impl Default for DataConformance {
    fn default() -> Self {
        Self {
            applicable: false,
            outcome: Outcome::Pass,
            violations_found: 0,
            sample_violations: vec![],
            query: None,
            query_time_ms: 0,
            source: "n/a".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleViolation {
    pub entity_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckReport {
    pub proposal_id: String,
    /// "approved" | "blocked"
    pub status: String,
    pub auto_approval_eligible: bool,
    pub block_reason: Option<String>,
    pub checks: Vec<CheckRow>,
    pub data_conformance: DataConformance,
    /// When the report was generated (RFC3339).
    pub generated_at: String,
    /// Total wall-clock time spent producing the report.
    pub elapsed_ms: u64,
    pub version: u32,
}

impl CheckReport {
    pub fn new(proposal_id: impl Into<String>) -> Self {
        Self {
            proposal_id: proposal_id.into(),
            status: "approved".into(),
            auto_approval_eligible: false,
            block_reason: None,
            checks: Vec::new(),
            data_conformance: DataConformance::default(),
            generated_at: Utc::now().to_rfc3339(),
            elapsed_ms: 0,
            version: 1,
        }
    }

    /// Returns true iff every axis row + the data-conformance block passed
    /// (advisory / skipped count as non-failure).
    pub fn all_axes_clean(&self) -> bool {
        let axes_clean = self.checks.iter().all(|c| !c.outcome.is_failure());
        let dc_clean = !self.data_conformance.outcome.is_failure();
        axes_clean && dc_clean
    }
}
