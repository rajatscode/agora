//! Browser-UI integration tests for Feature 4.
//!
//! Boots an in-process axum server that mounts both the JSON daemon API and
//! the new HTML UI, then exercises the home page, the static stylesheet,
//! the proposal-card fragment, and (when DATABASE_URL is set) the verify
//! fragment. Postgres-backed assertions are skipped without a DB — same
//! convention as `tests/daemon_http.rs`.

use std::net::SocketAddr;
use std::time::Duration;

use agora::daemon::{router, AppState};
use agora::db;
use sqlx::PgPool;
use tempfile::TempDir;

async fn boot() -> (String, TempDir, Option<PgPool>) {
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
async fn home_page_renders_all_eight_beats() {
    let (base, _tmp, _pool) = boot().await;
    let resp = client().get(format!("{base}/")).send().await.expect("home");
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("text/html"),
        "expected HTML content-type, got {ct:?}"
    );
    let body = resp.text().await.expect("body");

    // The wordmark, the HTMX CDN script, and the embedded stylesheet link.
    assert!(body.contains("agora"), "expected agora wordmark");
    assert!(body.contains("htmx.org"), "expected HTMX CDN script tag");
    assert!(
        body.contains("/static/agora.css"),
        "expected stylesheet link"
    );

    // All eight beat numbers must be visible at first paint.
    for (num, label) in [
        ("01 / 02", "Propose"),
        ("03", "check"),
        ("05", "approval"),
        ("06", "Risky"),
        ("07", "tamper"),
        ("08", "Explorer"),
    ] {
        assert!(
            body.contains(num),
            "expected beat number {num:?} in home page"
        );
        assert!(
            body.to_lowercase().contains(&label.to_lowercase()),
            "expected beat label {label:?} in home page"
        );
    }

    // HTMX endpoints must be referenced from the home form/buttons.
    assert!(body.contains("/ui/propose"));
    assert!(body.contains("/ui/risky-proposal"));
    assert!(body.contains("/ui/write"));

    // And the slot divs must exist for HTMX to swap into.
    for slot in [
        "beat-1-slot",
        "beat-3-slot",
        "beat-5-slot",
        "beat-6-slot",
        "beat-7a-slot",
        "beat-7b-slot",
        "beat-7c-slot",
    ] {
        assert!(body.contains(slot), "expected slot id {slot:?} in home page");
    }
}

#[tokio::test]
async fn static_css_is_served_with_correct_mime() {
    let (base, _tmp, _pool) = boot().await;
    let resp = client()
        .get(format!("{base}/static/agora.css"))
        .send()
        .await
        .expect("css");
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(ct.starts_with("text/css"));
    let body = resp.text().await.expect("body");
    assert!(body.contains("--accent"));
    assert!(body.contains(".beat"));
}

#[tokio::test]
async fn ui_propose_returns_html_fragment_with_artifacts_and_check_button() {
    let (base, _tmp, _pool) = boot().await;
    // Use a prompt that the offline heuristic author can handle so we don't
    // need ANTHROPIC_API_KEY for this test.
    let resp = client()
        .post(format!("{base}/ui/propose"))
        .form(&[("prompt", "add biometric authentication option to bank integrations")])
        .send()
        .await
        .expect("propose");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("body");

    // The fragment includes the proposal id with `prop_` prefix.
    assert!(body.contains("prop_"), "expected proposal id in fragment");
    // Reuse-detection block.
    assert!(body.to_lowercase().contains("reuse detection"));
    // Artifact tabs (proto/sql/handler/policy).
    assert!(body.contains(".proto"));
    assert!(body.contains(".sql"));
    assert!(body.contains("_handler.rs"));
    assert!(body.contains(".fga.json"));
    // The "Run multi-axis check" CTA, which posts back to /ui/proposals/.../check.
    assert!(body.contains("/ui/proposals/"));
    assert!(body.contains("/check"));
}

#[tokio::test]
async fn ui_concepts_index_lists_seed_catalog() {
    let (base, _tmp, _pool) = boot().await;
    let resp = client()
        .get(format!("{base}/ui/concepts"))
        .send()
        .await
        .expect("concepts");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("body");
    assert!(body.contains("core.integrations.BankIntegration"));
    assert!(body.contains("core.users.Account"));
}

#[tokio::test]
async fn ui_concept_view_unknown_returns_404_html() {
    let (base, _tmp, _pool) = boot().await;
    let resp = client()
        .get(format!("{base}/ui/concepts/core.unknown.Mystery"))
        .send()
        .await
        .expect("unknown concept");
    assert_eq!(resp.status(), 404);
    let body = resp.text().await.expect("body");
    assert!(body.contains("no concept named"));
}

#[tokio::test]
async fn ui_verify_without_db_is_503() {
    // Force no-DB state.
    let tmp = TempDir::new().expect("tempdir");
    let state = AppState::new(None, tmp.path().to_path_buf());
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let resp = client()
        .get(format!("http://{addr}/ui/verify"))
        .send()
        .await
        .expect("verify no-db");
    assert_eq!(resp.status(), 503);
    let body = resp.text().await.expect("body");
    assert!(body.to_lowercase().contains("postgres"));
}

/// Beat 6 end-to-end: pre-baked risky proposal, live data-conformance count.
/// Requires DATABASE_URL with the seeded 47 NULL-email Account rows; otherwise
/// the data-conformance axis is skipped and the count won't be 47, so we
/// gracefully skip the assertion.
#[tokio::test]
async fn ui_risky_proposal_fragment_renders_block_with_violations() {
    let (base, _tmp, pool) = boot().await;
    if pool.is_none() {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    }
    let resp = client()
        .post(format!("{base}/ui/risky-proposal"))
        .send()
        .await
        .expect("risky");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("body");
    // Either way the fragment should mention data_conformance.
    assert!(body.contains("data_conformance"));
    // And it should render the "Blocked" banner since the account table has
    // NULL emails per migrations/002_seed_accounts.sql.
    assert!(
        body.contains("Blocked"),
        "expected Blocked banner; body excerpt:\n{}",
        &body[..body.len().min(800)]
    );
}

/// Beat 7 end-to-end: write a row through /ui/write, verify confirms clean,
/// then tamper, then verify reports drift. Skipped without DATABASE_URL.
#[tokio::test]
async fn ui_write_then_tamper_then_verify_reports_drift() {
    let (base, _tmp, pool) = boot().await;
    let Some(pool) = pool else {
        eprintln!("skipping: DATABASE_URL not set");
        return;
    };
    let c = client();

    // 1. Write.
    let write_html = c
        .post(format!("{base}/ui/write"))
        .form(&[("provider", "plaid")])
        .send()
        .await
        .expect("write")
        .text()
        .await
        .expect("write body");
    assert!(write_html.contains("Write committed"));
    // Extract the entity_id from the hidden tamper form input. This is the
    // same id the user's browser would post back from the rendered button.
    let entity_id = extract_hidden_entity_id(&write_html).expect("entity_id");
    assert!(entity_id.starts_with("bi_demo_"));

    // 2. Verify (should be clean for the row we just wrote — we don't
    //    require global clean because pre-existing rows may exist).
    let verify_html = c
        .get(format!("{base}/ui/verify"))
        .send()
        .await
        .expect("verify1")
        .text()
        .await
        .expect("verify1 body");
    assert!(!verify_html.contains(&format!("<td class=\"findings\"><span class=\"id\">{entity_id}</span></td>")) || !verify_html.contains("Tampered rows"));

    // 3. Tamper.
    let tamper_html = c
        .post(format!("{base}/ui/tamper"))
        .form(&[("entity_id", entity_id.as_str())])
        .send()
        .await
        .expect("tamper")
        .text()
        .await
        .expect("tamper body");
    assert!(tamper_html.contains("Out-of-band UPDATE issued"));

    // 4. Verify again — drift detected for our row, with field-level diff
    //    showing the actual logged-vs-current value (Gate 4 falsification).
    let verify2_html = c
        .get(format!("{base}/ui/verify"))
        .send()
        .await
        .expect("verify2")
        .text()
        .await
        .expect("verify2 body");
    assert!(verify2_html.contains("Drift detected"));
    assert!(
        verify2_html.contains(entity_id.as_str()),
        "expected tampered entity {entity_id} to appear in verify report"
    );
    // The logged value (what we wrote: "plaid") and the tampered current
    // value ("evil_corp_tampered") must BOTH be visible — this is the
    // expected-vs-actual proof reviewers will look for.
    assert!(
        verify2_html.contains("plaid"),
        "expected logged value 'plaid' in field-diff; body excerpt:\n{}",
        &verify2_html[..verify2_html.len().min(2000)]
    );
    assert!(
        verify2_html.contains("evil_corp_tampered"),
        "expected current value 'evil_corp_tampered' in field-diff"
    );
    // The provider field name must appear in the diff table.
    assert!(verify2_html.contains("provider"));

    // Cleanup.
    let _ = sqlx::query("DELETE FROM mutation_log WHERE entity_id = $1")
        .bind(&entity_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM bank_integrations WHERE id = $1")
        .bind(&entity_id)
        .execute(&pool)
        .await;
}

/// Cheap regex-free extraction of `name="entity_id" value="bi_demo_..."`.
fn extract_hidden_entity_id(html: &str) -> Option<String> {
    let needle = r#"name="entity_id" value=""#;
    let i = html.find(needle)?;
    let rest = &html[i + needle.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}
