//! POST /admin/reset integration test (DEMO-ONLY endpoint).
//!
//! Verifies that the reset endpoint:
//!   1. Wipes any handler-written entity rows from per-demo tables.
//!   2. Wipes the mutation_log (allowed + denied entries).
//!   3. Restores the seed counts so the next demo starts clean.
//!   4. Leaves the load-bearing Beat-6 seed (47 NULL-email accounts)
//!      intact — the `accounts` table only loses non-seed (`id NOT LIKE
//!      'acct_%'`) rows.
//!
//! DB-gated: skips cleanly when DATABASE_URL is unset.

use std::net::SocketAddr;
use std::time::Duration;

use agora::daemon::{router, AppState};
use agora::db;
use agora::entity_write::{TYPE_AUDIT_FINDING, TYPE_BANK_INTEGRATION, TYPE_CUSTOMER};
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

#[tokio::test]
async fn admin_reset_wipes_handler_writes_and_restores_seeds() {
    let Some((base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let c = client();

    // 1. Make some per-demo state we expect /admin/reset to clean.
    let bi_id = format!("bi_reset_test_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = c
        .post(format!("{base}/entities/BankIntegration"))
        .json(&serde_json::json!({
            "entity_id": bi_id,
            "provider": "plaid",
            "actor": "team:integrations-platform",
        }))
        .send()
        .await
        .expect("write bi");
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap_or_default());

    let cust_id = format!("cust_reset_test_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = c
        .post(format!("{base}/entities/Customer"))
        .json(&serde_json::json!({
            "entity_id": cust_id,
            "email": "reset@example.com",
            "actor": "team:customer-platform",
        }))
        .send()
        .await
        .expect("write cust");
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap_or_default());

    let af_id = format!("af_reset_test_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = c
        .post(format!("{base}/entities/AuditFinding"))
        .json(&serde_json::json!({
            "entity_id": af_id,
            "rule_id": "SOC2-CC6.1",
            "severity": "low",
            "status": "open",
            "opened_at": "2026-05-15T10:00:00Z",
            "actor": "team:compliance-platform",
        }))
        .send()
        .await
        .expect("write af");
    assert_eq!(resp.status(), 200, "{}", resp.text().await.unwrap_or_default());

    // Snapshot pre-reset: these rows DO exist.
    let bi_pre: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bank_integrations WHERE id = $1")
        .bind(&bi_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(bi_pre, 1);

    // 2. Trigger reset.
    let body: Value = c
        .post(format!("{base}/admin/reset"))
        .send()
        .await
        .expect("reset")
        .json()
        .await
        .expect("reset json");
    assert_eq!(body["reset"], true);
    assert!(body["elapsed_ms"].as_u64().is_some());

    // 3. Per-demo rows wiped.
    let bi_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM bank_integrations WHERE id = $1")
        .bind(&bi_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(bi_after, 0, "bank_integration {bi_id} should be wiped");
    let cust_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM customers WHERE id = $1")
        .bind(&cust_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(cust_after, 0);
    let af_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_findings WHERE id = $1")
        .bind(&af_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(af_after, 0);

    // 4. mutation_log fully truncated.
    let ml: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mutation_log")
        .fetch_one(&pool)
        .await
        .expect("ml count");
    assert_eq!(ml, 0, "mutation_log should be empty after reset");

    // 5. Seeds restored.
    let cust_seed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM customers WHERE id LIKE 'cust_%' AND id ~ '^cust_[0-9]+$'",
    )
    .fetch_one(&pool)
    .await
    .expect("cust seed count");
    assert_eq!(cust_seed, 20, "all 20 seeded customers should be present");

    let af_seed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_findings WHERE id LIKE 'af_%' AND id ~ '^af_[0-9]+$'",
    )
    .fetch_one(&pool)
    .await
    .expect("af seed count");
    assert_eq!(af_seed, 15, "all 15 seeded audit_findings should be present");

    // 6. Beat-6 anchor: the 47 NULL-email seeded accounts must STILL be there.
    // We don't drop the accounts seed because data-conformance demonstrates the
    // 47-row count and any change here would invalidate the Beat 6 narrative.
    let null_emails: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM accounts WHERE email IS NULL")
            .fetch_one(&pool)
            .await
            .expect("null email count");
    assert!(
        null_emails >= 47,
        "Beat-6 seed of 47 NULL-email accounts must remain; got {null_emails}"
    );

    // 7. Counts surfaced in the response are sane shapes.
    let truncated = &body["entities_truncated"];
    let restored = &body["seeds_restored"];
    assert!(truncated["mutation_log"].as_i64().unwrap() >= 3); // ≥ the 3 entities we wrote
    assert_eq!(restored["customers"].as_i64().unwrap(), 20);
    assert_eq!(restored["audit_findings"].as_i64().unwrap(), 15);
    assert_eq!(restored["mutation_log"].as_i64().unwrap(), 0);
}
