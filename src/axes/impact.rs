//! Impact axis — which downstream artifacts / concepts are affected?
//!
//! Informational (never blocks). Surveys the registry for things that
//! reference the proposal's target type:
//!   - Relations pointing TO the target (from other concepts)
//!   - Catalog cards whose summary mentions the target name
//!   - Generated artifacts under `generated/` for the target (if any)
//!
//! Beat 3 of the demo cites this output ("affects: BankIntegration, …") so
//! it has to look like real data, not a placeholder.

use crate::ast::{Change, OntologyChangeProposal};
use crate::check_report::{Axis, CheckRow, Confidence, Outcome};
use crate::seed::ConceptCard;
use serde_json::json;
use std::time::Instant;

pub fn run(proposal: &OntologyChangeProposal, catalog: &[ConceptCard]) -> CheckRow {
    let started = Instant::now();
    let target = proposal.target();
    let target_fqn = target.fqn();

    let mut referenced_by: Vec<String> = Vec::new();
    for card in catalog {
        if card.fqn == target_fqn {
            continue;
        }
        for rel in &card.spec.relations {
            if rel.to.fqn() == target_fqn || rel.from.fqn() == target_fqn {
                referenced_by.push(format!("{} (via relation `{}`)", card.fqn, rel.name));
            }
        }
        if card.summary.contains(&target.name.to_lowercase())
            && !referenced_by.iter().any(|s| s.starts_with(&card.fqn))
        {
            referenced_by.push(format!("{} (summary mention)", card.fqn));
        }
    }

    let kind_label = match &proposal.change {
        Change::AddField { .. } => "field added",
        Change::AddRelation { .. } => "relation added",
        Change::DeprecateField { .. } => "field deprecated",
        Change::ReclassifyField { .. } => "field reclassified",
        Change::CreateType { .. } => "type created",
        Change::TightenField { .. } => "field tightened",
    };

    let findings = format!(
        "Impact: {} on `{}`. {} concept(s) reference this target.",
        kind_label,
        target_fqn,
        referenced_by.len()
    );

    CheckRow {
        axis: Axis::Impact,
        outcome: Outcome::Advisory,
        findings,
        evidence: json!({
            "target_fqn": target_fqn,
            "referenced_by": referenced_by,
            "kind": kind_label,
        }),
        confidence: Confidence::High,
        source: "deterministic".into(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}
