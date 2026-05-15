//! Integration tests for the F6 agent loop.
//!
//! These tests exercise `agent::agent_loop` end-to-end through the same code
//! path the daemon's `POST /agent/run` handler hits — author → check →
//! revise → check → terminate. Postgres is NOT required: the offline LLM
//! mode is deterministic and the data-conformance axis treats a missing DB
//! plus a present `backfill_plan` as Advisory (mitigated).
//!
//! Three scenarios cover the loop's three terminal shapes:
//!   * Happy path        — additive prompt approves on attempt 1
//!   * Revision path     — tighten prompt blocks, revision adds backfill,
//!                         re-check approves on attempt 2
//!   * HTTP surface      — POST /agent/run returns the full AgentResult
//!
//! Existing 50 tests must still pass alongside these — see
//! `tests/{daemon_http,mutation_log_verify,risk_gate,ui_browser}.rs`.

use std::net::SocketAddr;
use std::time::Duration;

use agora::agent::{self, ActionTaken, FinalStatus};
use agora::daemon::{router, AppState};
use agora::db;
use agora::seed;
use serde_json::Value;
use tempfile::TempDir;

#[tokio::test]
async fn happy_prompt_approves_in_single_attempt() {
    std::env::remove_var("ANTHROPIC_API_KEY");
    let catalog = seed::baseline_concepts();

    let result = agent::agent_loop(
        "add an OAuth capability flag to authentication methods",
        &catalog,
        None,
    )
    .await
    .expect("agent_loop");

    assert_eq!(result.final_status, FinalStatus::Approved, "{:?}", result);
    assert_eq!(result.attempts.len(), 1);
    assert!(matches!(
        result.attempts[0].action_taken,
        ActionTaken::Authored
    ));
    assert!(result.attempts[0].check_report.auto_approval_eligible);
}

#[tokio::test]
async fn tighten_prompt_blocks_then_revision_adds_backfill_and_approves() {
    // The signature F6 demo arc: tighten Account.email → first attempt has
    // no migration plan → data_conformance blocks → revision adds
    // backfill_plan → DC flips to Advisory → approved on attempt 2.
    std::env::remove_var("ANTHROPIC_API_KEY");
    let catalog = seed::baseline_concepts();

    let result = agent::agent_loop(
        "tighten Account.email to required for compliance",
        &catalog,
        None,
    )
    .await
    .expect("agent_loop");

    assert_eq!(result.final_status, FinalStatus::Approved, "{:?}", result);
    assert_eq!(result.attempts.len(), 2);

    // Attempt 1: authored, blocked, no migration plan.
    let a1 = &result.attempts[0];
    assert!(matches!(a1.action_taken, ActionTaken::Authored));
    assert!(!a1.check_report.auto_approval_eligible);
    assert!(a1.proposal.migration.is_none());
    let block_reason = a1
        .check_report
        .block_reason
        .as_deref()
        .unwrap_or_default();
    assert!(
        block_reason.contains("data_conformance"),
        "expected data_conformance in block_reason; got: {block_reason}"
    );

    // Attempt 2: revised, approved, migration.backfill_plan populated.
    let a2 = &result.attempts[1];
    let reason = match &a2.action_taken {
        ActionTaken::Revised { reason } => reason.clone(),
        ActionTaken::Authored => panic!("attempt 2 should be Revised, was Authored"),
    };
    assert!(
        reason.contains("backfill_plan"),
        "revision reason should mention backfill_plan; got: {reason}"
    );
    assert!(a2.check_report.auto_approval_eligible, "{:?}", a2.check_report);
    let plan = a2
        .proposal
        .migration
        .as_ref()
        .and_then(|m| m.backfill_plan.as_ref())
        .expect("revised proposal carries migration.backfill_plan");
    assert!(!plan.strategy.is_empty());
    assert!(plan.idempotent);
}

#[tokio::test]
async fn revised_proposal_preserves_change_kind_and_target() {
    // Tightening Account.email — the revision must still be a TightenField
    // on the same target. Abandoning the change is not the agent's job; it
    // should add the mitigation, not pivot to a different proposal.
    std::env::remove_var("ANTHROPIC_API_KEY");
    let catalog = seed::baseline_concepts();

    let result = agent::agent_loop(
        "tighten Account.email to required",
        &catalog,
        None,
    )
    .await
    .expect("agent_loop");

    let a1 = &result.attempts[0];
    let a2 = &result.attempts[1];

    // Same change kind and same target FQN — only the migration plan should
    // have changed.
    assert_eq!(
        a1.proposal.signature(),
        a2.proposal.signature(),
        "revision must preserve change signature; before={} after={}",
        a1.proposal.signature(),
        a2.proposal.signature()
    );
    // ID is preserved so the artifact directory tracks one logical proposal.
    assert_eq!(a1.proposal.id, a2.proposal.id);
}

#[tokio::test]
async fn http_agent_run_returns_attempts_trail() {
    let tmp = TempDir::new().expect("tempdir");
    let pool = db::connect_optional(None).await.ok().flatten();
    let state = AppState::new(pool, tmp.path().to_path_buf());
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");

    std::env::remove_var("ANTHROPIC_API_KEY");

    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let body: Value = c
        .post(format!("{base}/agent/run"))
        .json(&serde_json::json!({
            "prompt": "tighten Account.email to required for compliance"
        }))
        .send()
        .await
        .expect("POST /agent/run")
        .json()
        .await
        .expect("agent run json");

    assert_eq!(body["final_status"], "approved");
    let attempts = body["attempts"].as_array().expect("attempts array");
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["action_taken"]["kind"], "authored");
    assert_eq!(attempts[1]["action_taken"]["kind"], "revised");
    assert!(attempts[0]["check_report"]["auto_approval_eligible"]
        .as_bool()
        .unwrap()
        == false);
    assert!(attempts[1]["check_report"]["auto_approval_eligible"]
        .as_bool()
        .unwrap());
}

#[tokio::test]
async fn empty_prompt_is_rejected_by_http_handler() {
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::new(None, tmp.path().to_path_buf());
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");

    let c = reqwest::Client::new();
    let resp = c
        .post(format!("{base}/agent/run"))
        .json(&serde_json::json!({ "prompt": "" }))
        .send()
        .await
        .expect("POST /agent/run");
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.expect("err json");
    assert_eq!(body["error"], "empty_prompt");
}
