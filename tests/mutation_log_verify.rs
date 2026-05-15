//! Integration tests for Feature 3 — mutation log + verify + explorer.
//!
//! These tests REQUIRE a reachable Postgres (DATABASE_URL set). When the env
//! var is missing each test prints a skip message and passes — same pattern
//! the F2 data-conformance integration tests use. The runtime cost is small
//! (each test creates / cleans up a single fixture row).
//!
//! What these tests prove:
//!   1. `apply_create_bank_integration` writes the entity row AND a matching
//!      mutation_log row inside one transaction (Beat 7 happy path).
//!   2. A raw SQL UPDATE bypassing the handler is reported by `verify` with
//!      field-level drift detail (Beat 7 tampering proof).
//!   3. `verify` reports "clean" after a fresh write (no drift, no
//!      out-of-band for the entity we just wrote).
//!   4. `explorer` returns real owner / version-history data from the
//!      registry + mutation_log (Beat 8 proof).
//!
//! We use unique fixture entity_ids per test (test_<name>_<rand>) so tests
//! don't collide if run in parallel against the same DB.

use agora::db;
use agora::entity_write::{
    apply_create_bank_integration, CreateBankIntegrationCmd, WriteOrigin, TYPE_BANK_INTEGRATION,
};
use agora::explorer;
use agora::mutation_log;
use agora::verify::{verify, VerifyStatus};
use sqlx::PgPool;
use uuid::Uuid;

fn db_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

async fn connect_and_migrate() -> Option<PgPool> {
    let pool = db::connect_optional(None).await.ok().flatten()?;
    db::migrate(&pool).await.expect("migrations");
    Some(pool)
}

fn fixture_id(label: &str) -> String {
    format!("bi_test_{}_{}", label, Uuid::new_v4().simple())
}

async fn cleanup(pool: &PgPool, entity_id: &str) {
    let _ = sqlx::query("DELETE FROM mutation_log WHERE type_id = $1 AND entity_id = $2")
        .bind(TYPE_BANK_INTEGRATION)
        .bind(entity_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM bank_integrations WHERE id = $1")
        .bind(entity_id)
        .execute(pool)
        .await;
}

#[tokio::test]
async fn write_creates_entity_and_log_row_with_matching_checksum() {
    if db_url().is_none() {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    }
    let pool = connect_and_migrate().await.expect("pool");
    let entity_id = fixture_id("happy");

    let outcome = apply_create_bank_integration(
        &pool,
        &CreateBankIntegrationCmd {
            entity_id: entity_id.clone(),
            provider: "plaid".into(),
        },
        2,
        WriteOrigin::HttpHandler,
    )
    .await
    .expect("write");

    // Entity row exists.
    let provider: (String,) = sqlx::query_as("SELECT provider FROM bank_integrations WHERE id=$1")
        .bind(&entity_id)
        .fetch_one(&pool)
        .await
        .expect("entity row");
    assert_eq!(provider.0, "plaid");

    // Log row exists with the same checksum the outcome reported.
    let latest = mutation_log::latest_for_entity(&pool, TYPE_BANK_INTEGRATION, &entity_id)
        .await
        .expect("latest")
        .expect("some log row");
    assert_eq!(latest.checksum.as_deref(), Some(outcome.checksum.as_str()));
    assert_eq!(latest.operation, "Create");
    assert_eq!(latest.ontology_version, 2);
    // F5: the legacy `apply_create_bank_integration` entry point now
    // delegates to the authzed variant with default actor =
    // team:integrations-platform (the policy-permitted owner) so the
    // historical "http-handler" label has been promoted to a real actor.
    assert_eq!(latest.actor, "team:integrations-platform");

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn raw_sql_update_is_detected_as_drift() {
    if db_url().is_none() {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    }
    let pool = connect_and_migrate().await.expect("pool");
    let entity_id = fixture_id("drift");

    apply_create_bank_integration(
        &pool,
        &CreateBankIntegrationCmd {
            entity_id: entity_id.clone(),
            provider: "plaid".into(),
        },
        2,
        WriteOrigin::HttpHandler,
    )
    .await
    .expect("seed write");

    // Out-of-band tampering — the verify path's reason for existing.
    sqlx::query("UPDATE bank_integrations SET provider=$1 WHERE id=$2")
        .bind("evil_corp")
        .bind(&entity_id)
        .execute(&pool)
        .await
        .expect("tamper update");

    let report = verify(&pool).await.expect("verify");
    let drift = report
        .tampered_entities
        .iter()
        .find(|d| d.entity_id == entity_id)
        .expect("drift finding for our entity");

    assert_eq!(drift.entity_type, TYPE_BANK_INTEGRATION);
    assert_eq!(drift.fields_changed, vec!["provider".to_string()]);
    assert_eq!(drift.detected_via, "checksum mismatch");
    assert_ne!(drift.logged_checksum.as_deref(), Some(drift.current_checksum.as_str()));
    assert_eq!(report.verify_status, VerifyStatus::Tampered);

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn write_then_verify_reports_no_drift_for_this_entity() {
    if db_url().is_none() {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    }
    let pool = connect_and_migrate().await.expect("pool");
    let entity_id = fixture_id("clean");

    apply_create_bank_integration(
        &pool,
        &CreateBankIntegrationCmd {
            entity_id: entity_id.clone(),
            provider: "mx".into(),
        },
        2,
        WriteOrigin::Cli,
    )
    .await
    .expect("write");

    let report = verify(&pool).await.expect("verify");
    // Our specific entity should NOT show drift.
    assert!(
        !report
            .tampered_entities
            .iter()
            .any(|d| d.entity_id == entity_id),
        "entity {} unexpectedly reported as drift",
        entity_id
    );
    // And NOT as out-of-band either.
    assert!(
        !report
            .outofband_entities
            .iter()
            .any(|o| o.entity_id == entity_id),
        "entity {} unexpectedly reported as out-of-band",
        entity_id
    );

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn explorer_returns_history_including_our_write() {
    if db_url().is_none() {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    }
    let pool = connect_and_migrate().await.expect("pool");
    let entity_id = fixture_id("explorer");

    let outcome = apply_create_bank_integration(
        &pool,
        &CreateBankIntegrationCmd {
            entity_id: entity_id.clone(),
            provider: "finicity".into(),
        },
        2,
        WriteOrigin::HttpHandler,
    )
    .await
    .expect("write");

    let view = explorer::explorer(Some(&pool), TYPE_BANK_INTEGRATION)
        .await
        .expect("explorer")
        .expect("known fqn");

    // Real registry data.
    assert_eq!(view.fqn, TYPE_BANK_INTEGRATION);
    assert_eq!(view.ownership.team, "integrations-platform");
    assert!(!view.invariants.is_empty(), "invariants must be populated");
    assert!(!view.fields.is_empty(), "fields must be populated");
    assert!(
        view.policy_examples.iter().any(|p| p.relation == "owner"),
        "policy examples must include owner"
    );

    // Our write shows up in version history.
    let entry = view
        .version_history
        .iter()
        .find(|h| h.mutation_seq == outcome.mutation_seq)
        .expect("our write in history");
    assert_eq!(entry.entity_id, entity_id);
    // F5: see mutation_log_verify.rs:~95 — default actor is now the
    // owner team, not the generic "http-handler" label.
    assert_eq!(entry.actor, "team:integrations-platform");
    assert_eq!(entry.checksum.as_deref(), Some(outcome.checksum.as_str()));

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn explorer_returns_none_for_unknown_fqn_offline() {
    let r = explorer::explorer(None, "core.unknown.Mystery")
        .await
        .expect("explorer offline");
    assert!(r.is_none());
}
