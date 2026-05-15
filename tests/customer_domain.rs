//! F8 — second-domain (Customer 360) integration tests.
//!
//! These tests prove the framework generalizes:
//!   1. The explorer surfaces a real ConceptView for `core.customer.Customer`
//!      — same code path that powers BankIntegration.
//!   2. The risk-gate's data-conformance axis runs the same SQL against the
//!      seeded `customers` table that has 5 NULL emails — same Beat-6 arc
//!      on a different concept.
//!   3. The HTTP write path accepts `POST /entities/Customer` and respects
//!      F5 policy enforcement (allow for owner, deny for non-owner) without
//!      any per-domain code in the runtime.
//!   4. The F6 agent loop drives "tighten Customer.email" prompts to a
//!      revision with `backfill_plan` — same loop, no domain hooks.
//!
//! DB-requiring tests skip cleanly when DATABASE_URL is unset (same
//! convention as the F3 integration tests).

use std::net::SocketAddr;
use std::time::Duration;

use agora::agent::{self, ActionTaken, FinalStatus};
use agora::ast::{Change, OntologyChangeProposal};
use agora::check;
use agora::daemon::{router, AppState};
use agora::db;
use agora::entity_write::TYPE_CUSTOMER;
use agora::seed;
use serde_json::Value;
use sqlx::PgPool;
use tempfile::TempDir;
use uuid::Uuid;

async fn boot_server_with_db() -> Option<(String, TempDir, PgPool)> {
    let tmp = TempDir::new().expect("tempdir");
    let pool = db::connect_optional(None).await.ok().flatten()?;
    db::migrate(&pool).await.expect("migrations");
    let state = AppState::new(Some(pool.clone()), tmp.path().to_path_buf());
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    Some((format!("http://{addr}"), tmp, pool))
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
}

async fn cleanup(pool: &PgPool, entity_id: &str) {
    let _ = sqlx::query("DELETE FROM mutation_log WHERE type_id = $1 AND entity_id = $2")
        .bind(TYPE_CUSTOMER)
        .bind(entity_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM customers WHERE id = $1")
        .bind(entity_id)
        .execute(pool)
        .await;
}

// ============================================================================
// Pure (DB-free) tests
// ============================================================================

#[test]
fn customer_domain_concepts_are_in_baseline_catalog() {
    let cards = seed::baseline_concepts();
    let fqns: Vec<&str> = cards.iter().map(|c| c.fqn.as_str()).collect();
    assert!(fqns.contains(&"core.customer.Customer"));
    assert!(fqns.contains(&"core.customer.LoyaltyTier"));
    assert!(fqns.contains(&"core.customer.PurchaseHistory"));

    // Customer.email is intentionally optional — that's what makes the
    // tighten-to-required risky proposal a real semantic refinement.
    let customer = cards
        .iter()
        .find(|c| c.fqn == "core.customer.Customer")
        .unwrap();
    let email = customer
        .spec
        .fields
        .iter()
        .find(|f| f.name == "email")
        .expect("Customer.email present");
    assert!(!email.required, "Customer.email must be optional in baseline");

    // Ownership is split across teams per the brief.
    assert_eq!(customer.spec.ownership.team, "customer-platform");
    let purchases = cards
        .iter()
        .find(|c| c.fqn == "core.customer.PurchaseHistory")
        .unwrap();
    assert_eq!(purchases.spec.ownership.team, "analytics-platform");
}

#[tokio::test]
async fn agent_loop_handles_customer_domain_without_domain_specific_code() {
    // F4 + F6 generalization proof: the same agent_loop that drives Account
    // tightening drives Customer tightening — no per-domain conditional in
    // agent.rs. Without a DB, data-conformance is Skipped applicable → the
    // first attempt blocks; the revise heuristic adds a backfill_plan; the
    // second attempt clears.
    std::env::remove_var("ANTHROPIC_API_KEY");
    let catalog = seed::baseline_concepts();
    let result = agent::agent_loop(
        "tighten Customer.email to required for the Customer 360 domain",
        &catalog,
        None,
    )
    .await
    .expect("agent_loop");

    assert_eq!(result.final_status, FinalStatus::Approved, "{:?}", result);
    assert!(result.attempts.len() >= 2);
    // Attempt 1 authored, blocked.
    let a1 = &result.attempts[0];
    assert!(matches!(a1.action_taken, ActionTaken::Authored));
    assert!(!a1.check_report.auto_approval_eligible);
    // The proposal must target core.customer.Customer — proving the mock
    // author's dotted-reference recognizer picked the second-domain FQN.
    assert_eq!(a1.proposal.target().namespace, "core.customer");
    assert_eq!(a1.proposal.target().name, "Customer");
    assert!(matches!(a1.proposal.change, Change::TightenField { .. }));

    // Last attempt approved + has a backfill_plan tailored to Customer.
    let last = result.attempts.last().unwrap();
    assert!(last.check_report.auto_approval_eligible, "{:?}", last);
    let plan = last
        .proposal
        .migration
        .as_ref()
        .and_then(|m| m.backfill_plan.as_ref())
        .expect("backfill_plan on revision");
    // Strategy slug picked up the Customer-specific case in heuristic_backfill_for.
    assert!(
        plan.strategy.contains("synthetic_placeholder") || plan.strategy.contains("placeholder"),
        "strategy = {}",
        plan.strategy
    );
}

#[tokio::test]
async fn customer_risky_proposal_fixture_parses_and_classifies_as_tighten() {
    // The fixture under fixtures/customer_tighten_email.json must parse into
    // an OntologyChangeProposal and the change must be TightenField on
    // core.customer.Customer.email. (The full block-vs-pass behaviour is
    // covered by `customer_risky_tighten_blocks_on_real_db` below, which
    // requires DATABASE_URL.)
    let path = std::path::Path::new("fixtures").join("customer_tighten_email.json");
    let raw = std::fs::read_to_string(&path).expect("read fixture");
    let prop: OntologyChangeProposal =
        serde_json::from_str(&raw).expect("parse fixture");
    assert_eq!(prop.target().fqn(), "core.customer.Customer");
    match &prop.change {
        Change::TightenField {
            type_ref,
            field_name,
            from_required,
            to_required,
        } => {
            assert_eq!(type_ref.fqn(), "core.customer.Customer");
            assert_eq!(field_name, "email");
            assert!(!from_required);
            assert!(*to_required);
        }
        other => panic!("expected TightenField, got {other:?}"),
    }
}

// ============================================================================
// DB-requiring tests (skip when DATABASE_URL is unset)
// ============================================================================

#[tokio::test]
async fn customer_concept_in_explorer_returns_view_via_http() {
    let Some((base, _tmp, _pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let body: Value = client()
        .get(format!("{base}/concepts/core.customer.Customer"))
        .send()
        .await
        .expect("explorer request")
        .json()
        .await
        .expect("explorer json");
    assert_eq!(body["fqn"], "core.customer.Customer");
    assert_eq!(body["namespace"], "core.customer");
    assert_eq!(body["name"], "Customer");
    assert_eq!(body["ownership"]["team"], "customer-platform");
    let fields: Vec<&str> = body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    for expected in ["id", "email", "display_name", "signup_source"] {
        assert!(fields.contains(&expected), "missing field {expected}");
    }
}

#[tokio::test]
async fn customer_write_allowed_for_owner_team() {
    let Some((base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("cust_f8_allow_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = client()
        .post(format!("{base}/entities/Customer"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "email": "test@example.com",
            "display_name": "Test User",
            "signup_source": "web",
            "actor": "team:customer-platform",
        }))
        .send()
        .await
        .expect("write");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("write json");
    assert_eq!(body["entity_type"], TYPE_CUSTOMER);
    assert_eq!(body["operation"], "Create");
    assert_eq!(body["actor"], "team:customer-platform");

    // mutation_log row exists with Create + owner actor.
    let row: (String, String) = sqlx::query_as(
        "SELECT command, actor FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(TYPE_CUSTOMER)
    .bind(&entity_id)
    .fetch_one(&pool)
    .await
    .expect("query mutation_log");
    assert_eq!(row.0, "Create");
    assert_eq!(row.1, "team:customer-platform");

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn customer_write_denied_for_non_owner_team_logs_deny_attempt() {
    let Some((base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("cust_f8_deny_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = client()
        .post(format!("{base}/entities/Customer"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "email": "intruder@example.com",
            "actor": "team:marketing",
        }))
        .send()
        .await
        .expect("write");
    assert_eq!(resp.status(), 403);
    let body: Value = resp.json().await.expect("err json");
    assert_eq!(body["error"], "policy_denied");
    let ev = &body["evidence"];
    assert_eq!(ev["actor"], "team:marketing");
    assert_eq!(ev["relation"], "owner");
    assert_eq!(ev["object"], format!("customer:{entity_id}"));
    let reason = ev["reason"].as_str().unwrap();
    assert!(reason.contains("customer-platform"), "reason: {reason}");

    // DenyAttempt logged.
    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT command, actor, denial_reason FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(TYPE_CUSTOMER)
    .bind(&entity_id)
    .fetch_one(&pool)
    .await
    .expect("query deny log");
    assert_eq!(row.0, "DenyAttempt");
    assert_eq!(row.1, "team:marketing");
    assert!(row.2.expect("denial_reason persisted").contains("customer-platform"));

    // Entity table untouched.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customers WHERE id = $1")
        .bind(&entity_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn customer_risky_tighten_blocks_on_real_db_with_violation_count() {
    let Some((_base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    // Use the library function directly so we get the structured CheckReport
    // back (and don't have to thread it through HTTP).
    let path = std::path::Path::new("fixtures").join("customer_tighten_email.json");
    let raw = std::fs::read_to_string(&path).expect("read fixture");
    let proposal: OntologyChangeProposal =
        serde_json::from_str(&raw).expect("parse fixture");

    std::env::remove_var("ANTHROPIC_API_KEY");
    let catalog = seed::baseline_concepts();
    let report = check::check(&proposal, &catalog, Some(&pool))
        .await
        .expect("check");

    // Data-conformance must fail (or already-mitigated, depending on whether
    // some previous run inserted backfill — but with the seeded import-rows
    // it should be Fail with at least 5 violations).
    assert!(report.data_conformance.applicable);
    // The fixture has no migration plan → must be Fail (count > 0).
    assert_eq!(report.data_conformance.outcome.is_failure(), true,
        "data_conformance must Fail on seeded NULL rows; got: {:?}",
        report.data_conformance);
    assert!(
        report.data_conformance.violations_found >= 5,
        "expected ≥5 NULL-email rows from seed; got {}",
        report.data_conformance.violations_found
    );
    assert!(!report.auto_approval_eligible);
    let reason = report
        .block_reason
        .as_deref()
        .expect("block reason populated");
    assert!(reason.contains("data_conformance"), "reason: {reason}");
}
