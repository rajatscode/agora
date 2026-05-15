//! F5 integration tests — policy enforcement on entity writes.
//!
//! Three behavioural shapes covered:
//!   1. Allow path  — owner team writes succeed; mutation_log row has
//!                    operation=Create + actor=team:integrations-platform.
//!   2. Deny path   — non-owner gets 403 + structured evidence; mutation_log
//!                    row has operation=DenyAttempt + denial_reason populated;
//!                    entity table is NOT touched.
//!   3. Verify safety — verify() does NOT flag denied attempts as drift or
//!                      out-of-band (no entity row was created for them).
//!
//! Postgres is required (DATABASE_URL) for the in-DB checks. The pure
//! policy-evaluator behaviour is covered by `policy::tests` in the lib;
//! these tests prove the full HTTP → entity_write → mutation_log → verify
//! pipeline behaves correctly when policy fires.

use std::net::SocketAddr;
use std::time::Duration;

use agora::daemon::{router, AppState};
use agora::db;
use agora::entity_write::TYPE_BANK_INTEGRATION;
use serde_json::Value;
use sqlx::PgPool;
use tempfile::TempDir;
use uuid::Uuid;

async fn boot_server() -> Option<(String, TempDir, PgPool)> {
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
async fn owner_team_write_is_allowed_and_logged_as_create() {
    let Some((base, _tmp, pool)) = boot_server().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("bi_f5_allow_{}", &Uuid::new_v4().simple().to_string()[..10]);

    let resp = client()
        .post(format!("{base}/entities/BankIntegration"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "provider": "plaid",
            "actor": "team:integrations-platform",
        }))
        .send()
        .await
        .expect("write");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("write json");
    assert_eq!(body["entity_type"], TYPE_BANK_INTEGRATION);
    assert_eq!(body["operation"], "Create");
    assert_eq!(body["actor"], "team:integrations-platform");
    assert!(body["mutation_seq"].as_i64().unwrap() > 0);

    // mutation_log row exists with operation=Create + actor=team:...
    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT command, actor, denial_reason FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(TYPE_BANK_INTEGRATION)
    .bind(&entity_id)
    .fetch_one(&pool)
    .await
    .expect("query mutation_log");
    assert_eq!(row.0, "Create");
    assert_eq!(row.1, "team:integrations-platform");
    assert!(row.2.is_none(), "allowed write must not carry denial_reason");

    // Entity row exists.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bank_integrations WHERE id = $1")
        .bind(&entity_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn non_owner_team_write_is_denied_403_and_logs_deny_attempt() {
    let Some((base, _tmp, pool)) = boot_server().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("bi_f5_deny_{}", &Uuid::new_v4().simple().to_string()[..10]);

    let resp = client()
        .post(format!("{base}/entities/BankIntegration"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "provider": "plaid",
            "actor": "team:marketing",
        }))
        .send()
        .await
        .expect("write");
    assert_eq!(resp.status(), 403);
    let body: Value = resp.json().await.expect("err json");
    assert_eq!(body["error"], "policy_denied");
    let evidence = &body["evidence"];
    assert_eq!(evidence["actor"], "team:marketing");
    assert_eq!(evidence["relation"], "owner");
    assert_eq!(evidence["object"], format!("bank_integration:{entity_id}"));
    assert_eq!(evidence["operation_logged"], "DenyAttempt");
    let reason = evidence["reason"].as_str().unwrap();
    assert!(reason.contains("integrations-platform"), "reason: {reason}");
    assert!(reason.contains("marketing"), "reason: {reason}");
    let logged_seq = evidence["logged_seq"].as_i64().expect("logged_seq present");
    assert!(logged_seq > 0);

    // mutation_log row exists with operation=DenyAttempt + denial_reason set
    // + actor=team:marketing.
    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT command, actor, denial_reason FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(TYPE_BANK_INTEGRATION)
    .bind(&entity_id)
    .fetch_one(&pool)
    .await
    .expect("query mutation_log");
    assert_eq!(row.0, "DenyAttempt");
    assert_eq!(row.1, "team:marketing");
    let log_reason = row.2.expect("denial_reason persisted");
    assert!(log_reason.contains("marketing"), "log reason: {log_reason}");
    assert!(log_reason.contains("integrations-platform"), "log reason: {log_reason}");

    // Critically: the entity row was NOT inserted.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bank_integrations WHERE id = $1")
        .bind(&entity_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0, "denied write must not touch the entity table");

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn verify_does_not_flag_denied_attempt_as_drift() {
    let Some((base, _tmp, pool)) = boot_server().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("bi_f5_verify_{}", &Uuid::new_v4().simple().to_string()[..10]);

    // Issue a deny — adds a DenyAttempt row, no entity row.
    let resp = client()
        .post(format!("{base}/entities/BankIntegration"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "provider": "plaid",
            "actor": "team:marketing",
        }))
        .send()
        .await
        .expect("denied write");
    assert_eq!(resp.status(), 403);

    // Run verify. The denied entity_id must NOT appear as drift OR
    // out-of-band — verify only inspects entity-table rows, and there isn't
    // one for our denied attempt.
    let body: Value = client()
        .get(format!("{base}/verify"))
        .send()
        .await
        .expect("verify request")
        .json()
        .await
        .expect("verify json");
    let tampered: Vec<&str> = body["tampered_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["entity_id"].as_str())
        .collect();
    assert!(
        !tampered.contains(&entity_id.as_str()),
        "denied attempt should not show up as drift: {tampered:?}"
    );
    let oob: Vec<&str> = body["outofband_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["entity_id"].as_str())
        .collect();
    assert!(
        !oob.contains(&entity_id.as_str()),
        "denied attempt should not show up as out-of-band: {oob:?}"
    );

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn legacy_write_without_actor_defaults_to_owner_team_and_allows() {
    // Regression: pre-F5 clients (the existing daemon_http.rs test) send
    // {entity_id, provider} with no actor. They MUST keep working — F5
    // defaults the missing actor to team:integrations-platform.
    let Some((base, _tmp, pool)) = boot_server().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("bi_f5_legacy_{}", &Uuid::new_v4().simple().to_string()[..10]);

    let resp = client()
        .post(format!("{base}/entities/BankIntegration"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "provider": "plaid",
        }))
        .send()
        .await
        .expect("legacy write");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("write json");
    assert_eq!(body["operation"], "Create");
    // Default actor was applied — log row has it.
    assert_eq!(body["actor"], "team:integrations-platform");

    cleanup(&pool, &entity_id).await;
}
