//! HTTP integration tests for the `agorad` control plane.
//!
//! Boots an in-process axum server on a random port, then drives it with
//! reqwest through the full Beat 4–8 flow:
//!
//!   propose → check → approve → write → verify → explorer
//!
//! Postgres-backed steps (write/verify/explorer-with-history) are skipped when
//! DATABASE_URL is unset — same convention as the Feature-3 integration tests.

use std::net::SocketAddr;
use std::time::Duration;

use agora::daemon::{router, AppState};
use agora::db;
use agora::entity_write::TYPE_BANK_INTEGRATION;
use serde_json::Value;
use sqlx::PgPool;
use tempfile::TempDir;
use uuid::Uuid;

async fn boot_server() -> (String, TempDir, Option<PgPool>) {
    let tmp = TempDir::new().expect("tempdir");
    let pool = db::connect_optional(None).await.ok().flatten();
    if let Some(p) = &pool {
        db::migrate(p).await.expect("migrations");
    }
    let state = AppState::new(pool.clone(), tmp.path().to_path_buf());
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let base = format!("http://{addr}");
    // Tiny grace period so axum::serve can register before the first request.
    tokio::time::sleep(Duration::from_millis(50)).await;
    (base, tmp, pool)
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap()
}

#[tokio::test]
async fn health_endpoint_responds() {
    let (base, _tmp, _pool) = boot_server().await;
    let c = client();
    let resp = c
        .get(format!("{base}/health"))
        .send()
        .await
        .expect("health request");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.expect("health json");
    assert_eq!(body["status"], "ok");
    assert!(body["db"].is_string());
}

#[tokio::test]
async fn list_concepts_returns_seed_catalog() {
    let (base, _tmp, _pool) = boot_server().await;
    let body: Value = client()
        .get(format!("{base}/concepts"))
        .send()
        .await
        .expect("get /concepts")
        .json()
        .await
        .expect("concepts json");
    let arr = body.as_array().expect("array");
    assert!(!arr.is_empty(), "seed catalog should not be empty");
    let fqns: Vec<&str> = arr.iter().filter_map(|c| c["fqn"].as_str()).collect();
    assert!(fqns.contains(&"core.integrations.BankIntegration"));
}

#[tokio::test]
async fn get_concept_unknown_is_404() {
    let (base, _tmp, _pool) = boot_server().await;
    let resp = client()
        .get(format!("{base}/concepts/core.unknown.Mystery"))
        .send()
        .await
        .expect("get unknown concept");
    assert_eq!(resp.status(), 404);
    let body: Value = resp.json().await.expect("err json");
    assert_eq!(body["error"], "concept_not_found");
}

#[tokio::test]
async fn get_proposal_unknown_is_404() {
    let (base, _tmp, _pool) = boot_server().await;
    let resp = client()
        .get(format!("{base}/proposals/prop_doesnotexist"))
        .send()
        .await
        .expect("get unknown proposal");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn check_before_propose_is_404() {
    let (base, _tmp, _pool) = boot_server().await;
    let resp = client()
        .post(format!("{base}/proposals/prop_nope/check"))
        .send()
        .await
        .expect("check unknown proposal");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn write_without_db_is_503() {
    // Force a no-DB state by overriding state directly.
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::new(None, tmp.path().to_path_buf());
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = client()
        .post(format!("http://{addr}/entities/BankIntegration"))
        .json(&serde_json::json!({"entity_id": "x", "provider": "plaid"}))
        .send()
        .await
        .expect("write request");
    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.expect("err json");
    assert_eq!(body["error"], "database_unavailable");
}

#[tokio::test]
async fn write_unknown_entity_type_is_400() {
    let (base, _tmp, pool) = boot_server().await;
    if pool.is_none() {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    }
    let resp = client()
        .post(format!("{base}/entities/UnknownType"))
        .json(&serde_json::json!({"entity_id": "x"}))
        .send()
        .await
        .expect("write request");
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.expect("err json");
    assert_eq!(body["error"], "unsupported_entity_type");
}

/// End-to-end Beat 4–8 flow over HTTP. Skipped without DATABASE_URL.
#[tokio::test]
async fn end_to_end_propose_check_approve_write_verify_explorer() {
    let (base, _tmp, pool) = boot_server().await;
    let Some(pool) = pool else {
        eprintln!("skipping end-to-end: DATABASE_URL not set");
        return;
    };
    let c = client();

    // 1. Propose. Use a happy-path additive prompt so the offline heuristic
    //    author lands on AuthenticationMethod (additive). LLM author also
    //    fine — both paths produce a valid proposal.
    let propose: Value = c
        .post(format!("{base}/proposals"))
        .json(&serde_json::json!({"prompt": "add biometric authentication option to bank integrations"}))
        .send()
        .await
        .expect("POST /proposals")
        .json()
        .await
        .expect("propose json");
    let proposal_id = propose["proposal"]["id"]
        .as_str()
        .expect("proposal id")
        .to_string();
    assert!(propose["artifacts"]["proto"].is_string());

    // 2. List should include the new proposal.
    let listed: Value = c
        .get(format!("{base}/proposals"))
        .send()
        .await
        .expect("list")
        .json()
        .await
        .expect("list json");
    let ids: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str())
        .collect();
    assert!(ids.contains(&proposal_id.as_str()));

    // 3. Get proposal detail with artifact contents.
    let detail: Value = c
        .get(format!("{base}/proposals/{proposal_id}"))
        .send()
        .await
        .expect("detail")
        .json()
        .await
        .expect("detail json");
    assert_eq!(detail["proposal"]["id"], proposal_id);
    assert!(!detail["artifact_files"].as_array().unwrap().is_empty());

    // 4. Run the check.
    let report: Value = c
        .post(format!("{base}/proposals/{proposal_id}/check"))
        .send()
        .await
        .expect("check")
        .json()
        .await
        .expect("check json");
    assert_eq!(report["proposal_id"], proposal_id);
    assert!(report["checks"].as_array().unwrap().len() >= 6);

    // 5. Cached check report endpoint.
    let cached: Value = c
        .get(format!("{base}/proposals/{proposal_id}/check_report"))
        .send()
        .await
        .expect("cached")
        .json()
        .await
        .expect("cached json");
    assert_eq!(cached["proposal_id"], proposal_id);

    // 6. Approve. We don't assert auto_approval_eligible == true because the
    //    LLM author may classify differently than the offline author; we just
    //    assert the endpoint reports a deterministic verdict from the report.
    let approval: Value = c
        .post(format!("{base}/proposals/{proposal_id}/approve"))
        .send()
        .await
        .expect("approve")
        .json()
        .await
        .expect("approve json");
    assert_eq!(approval["proposal_id"], proposal_id);
    assert_eq!(
        approval["approved"].as_bool().unwrap(),
        report["auto_approval_eligible"].as_bool().unwrap()
    );

    // 7. Write a BankIntegration and clean it up after the run.
    let entity_id = format!("bi_http_test_{}", Uuid::new_v4().simple());
    let outcome: Value = c
        .post(format!("{base}/entities/BankIntegration"))
        .json(&serde_json::json!({"entity_id": entity_id, "provider": "plaid"}))
        .send()
        .await
        .expect("write")
        .json()
        .await
        .expect("write json");
    assert_eq!(outcome["entity_id"], entity_id);
    assert_eq!(outcome["entity_type"], TYPE_BANK_INTEGRATION);
    assert!(outcome["mutation_seq"].as_i64().unwrap() > 0);
    assert!(outcome["checksum"].as_str().unwrap().len() == 64);

    // 8. Verify reports the table state.
    let verify: Value = c
        .get(format!("{base}/verify"))
        .send()
        .await
        .expect("verify")
        .json()
        .await
        .expect("verify json");
    assert!(verify["entities_checked"].as_u64().unwrap() >= 1);
    // The entity we just wrote should not appear as drift or out-of-band.
    let tampered: Vec<&str> = verify["tampered_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["entity_id"].as_str())
        .collect();
    assert!(!tampered.contains(&entity_id.as_str()));
    let oob: Vec<&str> = verify["outofband_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["entity_id"].as_str())
        .collect();
    assert!(!oob.contains(&entity_id.as_str()));

    // 9. Explorer for BankIntegration includes the row's mutation in history.
    let view: Value = c
        .get(format!("{base}/concepts/{TYPE_BANK_INTEGRATION}"))
        .send()
        .await
        .expect("explorer")
        .json()
        .await
        .expect("explorer json");
    assert_eq!(view["fqn"], TYPE_BANK_INTEGRATION);
    let history_entity_ids: Vec<&str> = view["version_history"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["entity_id"].as_str())
        .collect();
    assert!(
        history_entity_ids.contains(&entity_id.as_str()),
        "expected freshly-written {entity_id} in version_history; got {history_entity_ids:?}"
    );

    // Cleanup so re-runs are idempotent.
    let _ = sqlx::query("DELETE FROM mutation_log WHERE type_id = $1 AND entity_id = $2")
        .bind(TYPE_BANK_INTEGRATION)
        .bind(&entity_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM bank_integrations WHERE id = $1")
        .bind(&entity_id)
        .execute(&pool)
        .await;
}
