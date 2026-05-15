//! F9 — third-domain (Compliance / GRC) integration tests.
//!
//! Mirrors `tests/customer_domain.rs`. Proves that the same plumbing that
//! generalized to a second domain (F8) generalizes to a third without
//! touching agent.rs / check.rs / explorer.rs.
//!
//! Five scenarios:
//!   1. The catalog registers AuditFinding + ComplianceRule.
//!   2. The explorer returns a real ConceptView for AuditFinding (DB-gated).
//!   3. Owner-team writes succeed and log Create rows (DB-gated).
//!   4. Non-owner-team writes are denied with DenyAttempt logged (DB-gated).
//!   5. verify() iterates the audit_findings table — seeded rows surface
//!      as out-of-band; handler-written rows reconcile cleanly; denied
//!      attempts poison neither bucket (DB-gated).

use std::net::SocketAddr;
use std::time::Duration;

use agora::daemon::{router, AppState};
use agora::db;
use agora::entity_write::TYPE_AUDIT_FINDING;
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
        .bind(TYPE_AUDIT_FINDING)
        .bind(entity_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM audit_findings WHERE id = $1")
        .bind(entity_id)
        .execute(pool)
        .await;
}

// ============================================================================
// Pure (DB-free) tests
// ============================================================================

#[test]
fn compliance_domain_concepts_are_in_baseline_catalog() {
    let cards = seed::baseline_concepts();
    let fqns: Vec<&str> = cards.iter().map(|c| c.fqn.as_str()).collect();
    assert!(fqns.contains(&"core.compliance.AuditFinding"));
    assert!(fqns.contains(&"core.compliance.ComplianceRule"));

    let af = cards
        .iter()
        .find(|c| c.fqn == "core.compliance.AuditFinding")
        .unwrap();
    // resolved_at is intentionally optional — the risky-tighten proposal target.
    let resolved = af
        .spec
        .fields
        .iter()
        .find(|f| f.name == "resolved_at")
        .expect("AuditFinding.resolved_at present");
    assert!(!resolved.required, "resolved_at must be optional in baseline");
    assert_eq!(af.spec.ownership.team, "compliance-platform");

    let rule = cards
        .iter()
        .find(|c| c.fqn == "core.compliance.ComplianceRule")
        .unwrap();
    assert_eq!(rule.spec.ownership.team, "compliance-platform");
}

// ============================================================================
// DB-requiring tests (skip when DATABASE_URL is unset)
// ============================================================================

#[tokio::test]
async fn audit_finding_concept_in_explorer_returns_view_via_http() {
    let Some((base, _tmp, _pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let body: Value = client()
        .get(format!("{base}/concepts/core.compliance.AuditFinding"))
        .send()
        .await
        .expect("explorer request")
        .json()
        .await
        .expect("explorer json");
    assert_eq!(body["fqn"], "core.compliance.AuditFinding");
    assert_eq!(body["namespace"], "core.compliance");
    assert_eq!(body["name"], "AuditFinding");
    assert_eq!(body["ownership"]["team"], "compliance-platform");
    let fields: Vec<&str> = body["fields"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    for expected in [
        "id",
        "rule_id",
        "severity",
        "status",
        "opened_at",
        "resolved_at",
        "notes",
    ] {
        assert!(fields.contains(&expected), "missing field {expected}");
    }
}

#[tokio::test]
async fn audit_finding_write_allowed_for_owner_team() {
    let Some((base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("af_f9_allow_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = client()
        .post(format!("{base}/entities/AuditFinding"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "rule_id": "SOC2-CC6.1",
            "severity": "high",
            "status": "investigating",
            "opened_at": "2026-05-15T10:00:00Z",
            "actor": "team:compliance-platform",
        }))
        .send()
        .await
        .expect("write");
    assert_eq!(resp.status(), 200, "body: {}", resp.text().await.unwrap_or_default());
    let body: Value = client()
        .get(format!("{base}/concepts/core.compliance.AuditFinding"))
        .send()
        .await
        .expect("explorer")
        .json()
        .await
        .expect("explorer json");
    assert_eq!(body["fqn"], "core.compliance.AuditFinding");

    // mutation_log row exists with Create + owner actor + denial_reason NULL.
    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT command, actor, denial_reason FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(TYPE_AUDIT_FINDING)
    .bind(&entity_id)
    .fetch_one(&pool)
    .await
    .expect("query mutation_log");
    assert_eq!(row.0, "Create");
    assert_eq!(row.1, "team:compliance-platform");
    assert!(row.2.is_none());

    // Entity row exists.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_findings WHERE id = $1")
        .bind(&entity_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 1);

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn audit_finding_write_denied_for_non_owner_team_logs_deny_attempt() {
    let Some((base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let entity_id = format!("af_f9_deny_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = client()
        .post(format!("{base}/entities/AuditFinding"))
        .json(&serde_json::json!({
            "entity_id": entity_id,
            "rule_id": "GDPR-Art32",
            "severity": "critical",
            "status": "open",
            "opened_at": "2026-05-15T11:00:00Z",
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
    assert_eq!(ev["object"], format!("audit_finding:{entity_id}"));
    let reason = ev["reason"].as_str().unwrap();
    assert!(reason.contains("compliance-platform"), "reason: {reason}");

    // DenyAttempt logged with reason.
    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT command, actor, denial_reason FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2 ORDER BY seq DESC LIMIT 1",
    )
    .bind(TYPE_AUDIT_FINDING)
    .bind(&entity_id)
    .fetch_one(&pool)
    .await
    .expect("query deny log");
    assert_eq!(row.0, "DenyAttempt");
    assert_eq!(row.1, "team:marketing");
    assert!(row
        .2
        .expect("denial_reason persisted")
        .contains("compliance-platform"));

    // Entity table untouched.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_findings WHERE id = $1")
        .bind(&entity_id)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);

    cleanup(&pool, &entity_id).await;
}

#[tokio::test]
async fn verify_iterates_audit_findings_and_treats_seeded_rows_as_oob() {
    // The F8 send-back pattern, on the new domain: verify() must iterate
    // the audit_findings table; the 15 seeded rows (af_001..af_015) bypass
    // the handler so they MUST surface in outofband_entities.
    let Some((base, _tmp, pool)) = boot_server_with_db().await else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };

    // Handler-written allow row.
    let allow_id = format!("af_f9_oob_allow_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = client()
        .post(format!("{base}/entities/AuditFinding"))
        .json(&serde_json::json!({
            "entity_id": allow_id,
            "rule_id": "SOC2-CC8.1",
            "severity": "low",
            "status": "resolved",
            "opened_at": "2026-05-15T12:00:00Z",
            "resolved_at": "2026-05-15T13:00:00Z",
            "actor": "team:compliance-platform",
        }))
        .send()
        .await
        .expect("write");
    assert_eq!(resp.status(), 200, "body: {}", resp.text().await.unwrap_or_default());

    // Handler-denied write.
    let deny_id = format!("af_f9_oob_deny_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let resp = client()
        .post(format!("{base}/entities/AuditFinding"))
        .json(&serde_json::json!({
            "entity_id": deny_id,
            "rule_id": "PCI-DSS-3.4",
            "severity": "high",
            "status": "open",
            "opened_at": "2026-05-15T14:00:00Z",
            "actor": "team:marketing",
        }))
        .send()
        .await
        .expect("deny");
    assert_eq!(resp.status(), 403);

    let body: Value = client()
        .get(format!("{base}/verify"))
        .send()
        .await
        .expect("verify")
        .json()
        .await
        .expect("verify json");
    let tampered: Vec<&str> = body["tampered_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["entity_id"].as_str())
        .collect();
    let oob: Vec<(&str, &str)> = body["outofband_entities"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| Some((e["entity_type"].as_str()?, e["entity_id"].as_str()?)))
        .collect();

    // (a) allow row: in neither bucket.
    assert!(
        !tampered.contains(&allow_id.as_str()),
        "handler-written audit_finding must not be flagged as drift"
    );
    let allow_in_oob = oob.iter().any(|(_, id)| *id == allow_id);
    assert!(!allow_in_oob, "handler-written audit_finding must not be flagged oob");

    // (b) deny row: in neither bucket.
    assert!(!tampered.contains(&deny_id.as_str()));
    assert!(!oob.iter().any(|(_, id)| *id == deny_id));

    // (c) seeded af_001..af_015 rows: must show up in oob (bypass handler).
    let oob_af_ids: Vec<&str> = oob
        .iter()
        .filter(|(t, _)| *t == TYPE_AUDIT_FINDING)
        .map(|(_, id)| *id)
        .collect();
    let seeded_present = (1..=15)
        .map(|n| format!("af_{:03}", n))
        .filter(|sid| oob_af_ids.contains(&sid.as_str()))
        .count();
    assert!(
        seeded_present >= 10,
        "expected ≥10 of the 15 seeded audit_findings in oob; \
         found {seeded_present}. oob audit_finding ids: {oob_af_ids:?}"
    );

    cleanup(&pool, &allow_id).await;
    cleanup(&pool, &deny_id).await;
}
