//! Integration tests for the F2 risk gate.
//!
//! These tests exercise the orchestrator end-to-end without a database.
//! Postgres-backed tests live separately (require DATABASE_URL); see the
//! README. The two scenarios here cover:
//!   - Happy path (Beat 3/5): additive optional field → auto-approve
//!   - Risky path (Beat 6 no-DB shape): tighten Account.email → blocked
//!     by data-conformance (applicable but unverified)

use agora::ast::*;
use agora::check;
use agora::check_report::Outcome;
use agora::seed;

fn happy_additive() -> OntologyChangeProposal {
    OntologyChangeProposal {
        id: "prop_happy_test".into(),
        domain: "integrations".into(),
        namespace: "core.integrations".into(),
        change_intent: "Add optional flag".into(),
        rationale: "Track OAuth capability".into(),
        change: Change::AddField {
            type_ref: TypeRef {
                namespace: "core.integrations".into(),
                name: "AuthenticationMethod".into(),
            },
            field: Field {
                name: "supports_oauth".into(),
                proto_type: ProtoType::Bool,
                proto_number: 17,
                required: false,
                since_version: 2,
                deprecated_in: None,
                classification: PolicyClass::Internal,
                doc: None,
            },
        },
        semantic_contract: SemanticContract {
            meaning_before: "n/a".into(),
            meaning_after: "OAuth capability flagged on auth methods.".into(),
            justification: None,
            invariants: vec!["additive only".into(), "no class change".into()],
        },
        compatibility: CompatibilityDeclaration::default(),
        ownership: Ownership {
            team: "integrations-platform".into(),
            semantic_steward: None,
        },
        tests: vec![],
        provenance: Provenance {
            author: "user://test".into(),
            source_prompt: "test".into(),
            model: "claude-sonnet-4-5".into(),
            generated_at: "2026-05-15T00:00:00Z".into(),
            trace_id: None,
        },
    }
}

fn risky_tighten() -> OntologyChangeProposal {
    OntologyChangeProposal {
        id: "prop_risky_test".into(),
        domain: "users".into(),
        namespace: "core.users".into(),
        change_intent: "Tighten Account.email".into(),
        rationale: "Compliance requires non-null email".into(),
        change: Change::TightenField {
            type_ref: TypeRef {
                namespace: "core.users".into(),
                name: "Account".into(),
            },
            field_name: "email".into(),
            from_required: false,
            to_required: true,
        },
        semantic_contract: SemanticContract {
            meaning_before: "email may be null".into(),
            meaning_after: "every account has an email".into(),
            justification: None,
            invariants: vec!["non-null email".into(), "no new accounts without email".into()],
        },
        compatibility: CompatibilityDeclaration {
            shape: CompatibilityClass::Refinement,
            semantic: CompatibilityClass::Refinement,
            temporal: CompatibilityClass::Refinement,
            policy: CompatibilityClass::Additive,
            api: CompatibilityClass::Refinement,
            storage: CompatibilityClass::Refinement,
        },
        ownership: Ownership {
            team: "identity-platform".into(),
            semantic_steward: None,
        },
        tests: vec![],
        provenance: Provenance {
            author: "user://test".into(),
            source_prompt: "test".into(),
            model: "claude-sonnet-4-5".into(),
            generated_at: "2026-05-15T00:00:00Z".into(),
            trace_id: None,
        },
    }
}

#[tokio::test]
async fn happy_path_auto_approves_without_db() {
    std::env::remove_var("ANTHROPIC_API_KEY");
    let proposal = happy_additive();
    let catalog = seed::baseline_concepts();
    let report = check::check(&proposal, &catalog, None).await.expect("check");

    assert_eq!(report.status, "approved", "{:?}", report);
    assert!(report.auto_approval_eligible);
    assert!(report.block_reason.is_none());

    // Every axis row present (7 + the data_conformance block).
    let axis_count = report.checks.len();
    assert_eq!(axis_count, 7, "expected 7 axis rows, got {axis_count}");

    // No Fail outcomes anywhere.
    assert!(report.checks.iter().all(|c| !matches!(c.outcome, Outcome::Fail)));
    assert!(!matches!(report.data_conformance.outcome, Outcome::Fail));
    // Data-conformance is not-applicable for a pure additive optional field.
    assert!(!report.data_conformance.applicable);
}

#[tokio::test]
async fn risky_path_blocks_when_db_unavailable() {
    std::env::remove_var("ANTHROPIC_API_KEY");
    let proposal = risky_tighten();
    let catalog = seed::baseline_concepts();
    let report = check::check(&proposal, &catalog, None).await.expect("check");

    // Without a DB we cannot prove the tighten is safe → must block.
    assert_eq!(report.status, "blocked", "{:?}", report);
    assert!(!report.auto_approval_eligible);
    let reason = report.block_reason.expect("block reason");
    assert!(
        reason.contains("data_conformance"),
        "expected data_conformance in block reason; got: {reason}"
    );

    // Data-conformance is applicable AND skipped.
    assert!(report.data_conformance.applicable);
    assert!(matches!(report.data_conformance.outcome, Outcome::Skipped));
}

#[tokio::test]
async fn report_is_valid_json() {
    std::env::remove_var("ANTHROPIC_API_KEY");
    let proposal = happy_additive();
    let catalog = seed::baseline_concepts();
    let report = check::check(&proposal, &catalog, None).await.expect("check");
    let s = serde_json::to_string_pretty(&report).expect("serialize");
    // Round-trip — exact same schema parseable back.
    let _: serde_json::Value = serde_json::from_str(&s).expect("parse");
}
