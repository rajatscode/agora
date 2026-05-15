//! Browser demo UI — Feature 4 (impl-f4).
//!
//! Server-rendered HTML via maud + interactivity via HTMX (loaded from CDN).
//! There is **no JavaScript framework, no bundler, no Node toolchain**:
//! every byte the browser sees is either Rust-emitted HTML, the single
//! `static/agora.css` file, or `htmx.org` from unpkg.
//!
//! This file is the UI counterpart to `src/daemon.rs`. The JSON daemon
//! handlers stay; the `/ui/*` handlers below wrap the SAME library
//! functions (`llm::author_proposal`, `check::check`, `verify::verify`,
//! `explorer::explorer`, `entity_write::*`) and render their results as
//! HTMX-friendly HTML fragments. Beats 1-8 of `DEMO.md` map onto the
//! routes registered here:
//!
//!   GET  /                             → home (all 8 beats laid out)
//!   POST /ui/propose                   → Beats 1+2 (proposal card, reuse hits, artifact tabs)
//!   POST /ui/proposals/{id}/check      → Beat 3 (7-axis check report)
//!   POST /ui/proposals/{id}/approve    → Beat 5 (auto-approval verdict)
//!   POST /ui/risky-proposal            → Beat 6 (pre-baked risky thread; live block)
//!   POST /ui/write                     → Beat 7a (write a BankIntegration)
//!   POST /ui/tamper                    → Beat 7b (raw-SQL tamper, DEMO-ONLY)
//!   GET  /ui/verify                    → Beat 7c (drift detection report)
//!   GET  /ui/concepts                  → Beat 8 (concept list)
//!   GET  /ui/concepts/{fqn}            → Beat 8 (full ConceptView)
//!   GET  /static/agora.css             → embedded stylesheet
//!
//! All HTML is rendered with `maud::html!` macros so escaping is automatic.
//! Inline strings that need to be inserted as raw HTML (rare; one tiny
//! tab-switching script in the home <head>) use `PreEscaped`.
//!
//! Why fragments and not full pages for the action endpoints: HTMX swaps
//! by id. Each beat owns a `#beat-N-slot` div on the home page; the
//! response replaces only that slot. The page never reloads, the
//! presenter never touches the URL bar, and every beat stays visible the
//! whole way through the demo.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Form,
};
use maud::{html, Markup, PreEscaped, DOCTYPE};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent::{self, ActionTaken, AgentResult, Attempt, FinalStatus};
use crate::artifacts;
use crate::ast::OntologyChangeProposal;
use crate::check;
use crate::check_report::{Axis, CheckReport, DataConformance, Outcome, SampleViolation};
use crate::daemon::AppState;
use crate::entity_write::{self, CreateBankIntegrationCmd, WriteOrigin, WriteOutcome};
use crate::explorer::{self, ConceptView};
use crate::llm::{self, AuthorMode};
use crate::reuse::{self, ReuseReport};
use crate::verify::{self, VerifyReport, VerifyStatus};

/// HTMX v1.9.10 bytes baked into the binary so the demo works on any
/// network. Loading from unpkg.com would silently fail behind venue
/// firewalls — buttons render, page looks fine, clicks do nothing. Inlining
/// makes agorad a true single-binary distribution.
const HTMX_JS: &str = include_str!("../static/htmx.min.js");

/// Stylesheet bytes baked into the binary. Served verbatim by `css_handler`.
/// `include_str!` is relative to *this file's directory*, so the path goes
/// up one level out of `src/` then into `static/`.
const AGORA_CSS: &str = include_str!("../static/agora.css");

/// Tiny vanilla-JS helper that wires the artifact tab strip. Six lines, no
/// framework. Inlined into the home page <head> so subsequent HTMX-injected
/// fragments can simply emit `.tab` / `.tab-panel` markup and have it work.
///
/// IMPORTANT: the listener attaches to `document`, NOT `document.body`. The
/// script runs from <head> before <body> is parsed, so `document.body` is
/// `null` at parse time — using it throws `TypeError: Cannot read
/// properties of null` and the handler never registers, leaving the
/// artifact tabs frozen on `.proto`. Event delegation off `document`
/// works because `document` exists from the start of HTML parsing and
/// click events bubble up to it from any descendant.
const TAB_SCRIPT: &str = r#"
function agoraSelectTab(group, name) {
  document.querySelectorAll('[data-tab-group="'+group+'"]').forEach(function(el){
    el.classList.toggle('active', el.dataset.tabName === name);
  });
}
document.addEventListener('click', function(e) {
  var t = e.target.closest('[data-tab-trigger]');
  if (!t) return;
  agoraSelectTab(t.dataset.tabGroup, t.dataset.tabName);
});
"#;

// ============================================================================
// Static asset
// ============================================================================

pub async fn css_handler() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        AGORA_CSS,
    )
        .into_response()
}

// ============================================================================
// Page-level routes
// ============================================================================

pub async fn home() -> Markup {
    page_layout("Agora — demo", html! {
        (intro_block())

        (beat_section(
            "01 / 02",
            "Propose a concept (LLM-authored)",
            "An agent describes a new concept in natural language. Agora calls Anthropic with a structured-output schema and the proposal arrives typed. Reuse detection runs against the seed catalog in the same pass.",
            html! {
                form
                    hx-post="/ui/propose"
                    hx-target="#beat-1-slot"
                    hx-swap="innerHTML"
                    hx-disabled-elt="find button"
                {
                    textarea
                        name="prompt"
                        rows="3"
                        placeholder="we need to model what each bank integration can do — supported features, rate limits, etc." {
                        "we need to model what each bank integration can do — supported features, rate limits, etc."
                    }
                    div.row.right {
                        button type="submit" { "Propose →" }
                    }
                    p.hint { "Live LLM call when ANTHROPIC_API_KEY is set; deterministic offline author otherwise. The author mode is shown on the response card." }
                }
                div id="beat-1-slot" {}
            },
        ))

        (beat_section(
            "03",
            "Multi-axis check report",
            "Eight checks run against the proposal: composition, shape, semantic (LLM), policy, temporal, impact, replay, and a live data-conformance query. Each row carries an outcome, evidence source, and elapsed time.",
            html! {
                div.hint { "After proposing above, run the checks against the resulting proposal." }
                div id="beat-3-slot" {}
            },
        ))

        (beat_section(
            "05",
            "Auto-approval verdict",
            "If every axis is clean (advisory and skipped count as non-failure), the proposal is auto-approval-eligible. The verdict comes from the same predicate any caller — CLI or HTTP — would see.",
            html! {
                div.hint { "Available once the check report has been produced." }
                div id="beat-5-slot" {}
            },
        ))

        (beat_section(
            "06",
            "Risky proposal blocked by real data",
            "A second, pre-baked proposal asks to tighten Account.email from optional to required. Agora's data-conformance axis runs a real query against the live Account table; the count it surfaces is the count that exists right now.",
            html! {
                form
                    hx-post="/ui/risky-proposal"
                    hx-target="#beat-6-slot"
                    hx-swap="innerHTML"
                    hx-disabled-elt="find button"
                {
                    div.row {
                        button type="submit" { "Run risky proposal →" }
                        span.hint { "Loads " code { "fixtures/beat6_tighten_account_email.json" } " and runs " code { "check::check" } " against it." }
                    }
                }
                div id="beat-6-slot" {}
            },
        ))

        (beat_section(
            "06½",
            "Agent loop — revise on rejection (F6)",
            "The vision driver. Beat 6 showed Agora blocking a risky proposal. Here the agent reads the structured rejection, revises the proposal with a migration plan, and re-submits — closing the loop. Each attempt is rendered as its own card with the verdict and the action taken; both successes and failures are visible so the audience can see what happened.",
            html! {
                form
                    hx-post="/ui/agent"
                    hx-target="#beat-65-slot"
                    hx-swap="innerHTML"
                    hx-disabled-elt="find button"
                {
                    textarea
                        name="prompt"
                        rows="2"
                        placeholder="tighten Account.email to required for compliance" {
                        "tighten Account.email to required for compliance"
                    }
                    div.row.right {
                        button type="submit" { "Run agent loop →" }
                    }
                    p.hint { "Up to " code { "MAX_ATTEMPTS = 3" } " attempts. Author→check→(revise→check){0..2}. Live LLM revision when " code { "ANTHROPIC_API_KEY" } " is set; deterministic offline heuristic that adds " code { "migration.backfill_plan" } " otherwise." }
                }
                div id="beat-65-slot" {}
            },
        ))

        (beat_section(
            "07",
            "Write → policy → tamper → verify",
            "A controlled write creates a BankIntegration row and a mutation_log entry inside one transaction — but only after the FGA policy evaluator clears the actor on the `owner` relation. Pick `team:integrations-platform` to see the allow path; pick `team:marketing` to see the deny path with full policy trace. Both attempts are logged: the denial gets its own DenyAttempt row so it's auditable.",
            html! {
                form
                    hx-post="/ui/write"
                    hx-target="#beat-7a-slot"
                    hx-swap="innerHTML"
                    hx-disabled-elt="find button"
                {
                    div.row {
                        label for="actor" { "Actor " }
                        select name="actor" style="max-width:280px" {
                            option value="team:integrations-platform" selected { "team:integrations-platform (owner — allow)" }
                            option value="team:marketing" { "team:marketing (not owner — deny)" }
                        }
                    }
                    div.row {
                        label for="provider" { "Provider " }
                        input type="text" name="provider" value="plaid" style="max-width:200px" {}
                        button type="submit" { "Write a BankIntegration →" }
                    }
                    p.hint { "The entity_id is generated server-side. The policy evaluator runs " code { "policy::evaluate(...)" } " against the FGA spec for the concept; on allow the response carries " code { "mutation_seq" } " and the SHA-256 checksum, on deny the response carries the full denial trace and the DenyAttempt row's seq." }
                }
                div id="beat-7a-slot" {}
                div id="beat-7b-slot" {}
                div id="beat-7c-slot" {}
            },
        ))

        (beat_section(
            "08",
            "Explorer — owner, invariants, lineage, policy, history",
            "Discovery as a first-class output of the control plane. Every field below is derived from the registry plus the live mutation_log; nothing is hand-painted for the demo.",
            html! {
                p.hint { "Open a concept to see the explorer view." }
                ul.concept-list {
                    li {
                        a href="/ui/concepts/core.integrations.BankIntegration" {
                            span.fqn { "core.integrations.BankIntegration" }
                            span.team { "owner: integrations-platform" }
                        }
                    }
                    li {
                        a href="/ui/concepts/core.integrations.AuthenticationMethod" {
                            span.fqn { "core.integrations.AuthenticationMethod" }
                            span.team { "owner: integrations-platform" }
                        }
                    }
                    li {
                        a href="/ui/concepts/core.users.Account" {
                            span.fqn { "core.users.Account" }
                            span.team { "owner: identity-platform" }
                        }
                    }
                    li {
                        a href="/ui/concepts" { "All concepts →" }
                    }
                }
            },
        ))
    })
}

pub async fn concepts_index(State(state): State<AppState>) -> Markup {
    let items: Vec<Markup> = state
        .catalog
        .iter()
        .map(|c| {
            let fqn = c.fqn.clone();
            html! {
                li {
                    a href=(format!("/ui/concepts/{fqn}")) {
                        span.fqn { (fqn) }
                        span.team { "owner: " (c.spec.ownership.team) " · v" (c.spec.version) }
                    }
                }
            }
        })
        .collect();

    page_layout("Concepts — Agora", html! {
        (nav_back("/", "← back to demo"))
        div.beat {
            div.beat-head {
                span.beat-num { "08" }
                h2.beat-title { "Registered concepts" }
            }
            p.beat-sub { (items.len()) " concepts in the seed catalog. Click any to open the explorer." }
            ul.concept-list {
                @for item in &items { (item) }
            }
        }
    })
}

pub async fn concept_view_page(
    State(state): State<AppState>,
    AxumPath(fqn): AxumPath<String>,
) -> Result<Markup, UiError> {
    let view = explorer::explorer(state.pool.as_ref(), &fqn)
        .await
        .map_err(|e| UiError::internal(format!("explorer failed: {e}")))?;
    let view = view.ok_or_else(|| UiError::not_found(format!("no concept named {fqn:?}")))?;

    Ok(page_layout(&format!("{fqn} — Agora"), html! {
        (nav_back("/ui/concepts", "← all concepts"))
        (concept_view_markup(&view, state.pool.is_some()))
    }))
}

// ============================================================================
// HTMX fragment endpoints
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ProposeForm {
    pub prompt: String,
}

pub async fn ui_propose(
    State(state): State<AppState>,
    Form(form): Form<ProposeForm>,
) -> Result<Markup, UiError> {
    if form.prompt.trim().is_empty() {
        return Err(UiError::bad_request("prompt is empty"));
    }

    let actor = "agent://agora-browser-demo";
    let (proposal, mode) = llm::author_proposal(&form.prompt, actor)
        .await
        .map_err(|e| UiError::internal(format!("author_proposal failed: {e}")))?;

    let manifest = artifacts::emit_all(&proposal, &state.generated_root)
        .map_err(|e| UiError::internal(format!("artifact emit failed: {e}")))?;

    // Persist the proposal alongside its artifacts so /check can read it.
    let proposal_path = Path::new(&manifest.directory).join("proposal.json");
    let bytes = serde_json::to_vec_pretty(&proposal)
        .map_err(|e| UiError::internal(format!("serialize proposal failed: {e}")))?;
    std::fs::write(&proposal_path, &bytes).map_err(|e| {
        UiError::internal(format!(
            "writing {}: {e}",
            proposal_path.display()
        ))
    })?;

    let reuse_report = reuse::classify(&proposal, state.catalog.as_slice());

    Ok(proposal_card(&proposal, &mode, &manifest, &reuse_report))
}

pub async fn ui_check(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Markup, UiError> {
    let dir = state.generated_root.join(&id);
    let proposal_path = dir.join("proposal.json");
    if !proposal_path.exists() {
        return Err(UiError::not_found(format!(
            "proposal {id:?} has no proposal.json on disk"
        )));
    }
    let proposal = load_proposal(&proposal_path)
        .map_err(|e| UiError::internal(format!("load proposal: {e}")))?;

    let report = check::check(&proposal, state.catalog.as_slice(), state.pool.as_ref())
        .await
        .map_err(|e| UiError::internal(format!("check::check failed: {e}")))?;

    // Cache for /check_report and /approve to read.
    if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
        let _ = std::fs::write(dir.join("check_report.json"), bytes);
    }

    Ok(check_report_panel(&report, &proposal.id, false))
}

pub async fn ui_approve(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Markup, UiError> {
    let dir = state.generated_root.join(&id);
    let report_path = dir.join("check_report.json");
    if !report_path.exists() {
        return Err(UiError::bad_request(
            "run the check before approving — no check_report.json on disk",
        ));
    }
    let raw = std::fs::read_to_string(&report_path)
        .map_err(|e| UiError::internal(format!("read report: {e}")))?;
    let report: CheckReport = serde_json::from_str(&raw)
        .map_err(|e| UiError::internal(format!("parse report: {e}")))?;
    Ok(approval_panel(&report))
}

pub async fn ui_risky_proposal(
    State(state): State<AppState>,
) -> Result<Markup, UiError> {
    // Load the pre-baked TightenField proposal from fixtures, copy it into
    // the generated root so subsequent /check & /approve calls work, then
    // run the check live. The 47-violation count comes from
    // axes::data_conformance::run, which runs SELECT COUNT(*) ... IS NULL
    // against the real Account table.
    let fixture_path = Path::new("fixtures").join("beat6_tighten_account_email.json");
    let raw = std::fs::read_to_string(&fixture_path).map_err(|e| {
        UiError::internal(format!(
            "reading fixture {}: {e}",
            fixture_path.display()
        ))
    })?;
    let proposal: OntologyChangeProposal = serde_json::from_str(&raw)
        .map_err(|e| UiError::internal(format!("parse fixture: {e}")))?;

    let dir = state.generated_root.join(&proposal.id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| UiError::internal(format!("mkdir {}: {e}", dir.display())))?;
    std::fs::write(dir.join("proposal.json"), &raw)
        .map_err(|e| UiError::internal(format!("persist risky proposal: {e}")))?;

    let report = check::check(&proposal, state.catalog.as_slice(), state.pool.as_ref())
        .await
        .map_err(|e| UiError::internal(format!("check::check failed: {e}")))?;

    if let Ok(bytes) = serde_json::to_vec_pretty(&report) {
        let _ = std::fs::write(dir.join("check_report.json"), bytes);
    }

    Ok(html! {
        (check_report_panel(&report, &proposal.id, true))
    })
}

#[derive(Debug, Deserialize)]
pub struct AgentRunForm {
    pub prompt: String,
}

/// F6 handler — runs the closed-loop revision and renders every attempt as
/// its own card. The first attempt's card is amber/red if blocked, green if
/// approved; subsequent (revision) cards display the action taken
/// ("added migration.backfill_plan …") so the audience can see *what
/// changed* between attempts.
pub async fn ui_agent_run(
    State(state): State<AppState>,
    Form(form): Form<AgentRunForm>,
) -> Result<Markup, UiError> {
    if form.prompt.trim().is_empty() {
        return Err(UiError::bad_request("prompt is empty"));
    }

    let result = agent::agent_loop(&form.prompt, state.catalog.as_slice(), state.pool.as_ref())
        .await
        .map_err(|e| UiError::internal(format!("agent_loop failed: {e}")))?;

    // Persist final-attempt artifacts so the rest of the demo (e.g. /ui/concepts)
    // can link to the resulting proposal.
    if let Some(final_attempt) = result.attempts.last() {
        let dir = state.generated_root.join(&final_attempt.proposal.id);
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&final_attempt.proposal) {
                let _ = std::fs::write(dir.join("proposal.json"), bytes);
            }
            if let Ok(bytes) = serde_json::to_vec_pretty(&final_attempt.check_report) {
                let _ = std::fs::write(dir.join("check_report.json"), bytes);
            }
            if let Ok(bytes) = serde_json::to_vec_pretty(&result) {
                let _ = std::fs::write(dir.join("agent_run.json"), bytes);
            }
        }
    }

    Ok(agent_result_panel(&result))
}

#[derive(Debug, Deserialize)]
pub struct WriteForm {
    pub provider: String,
    /// F5: actor performing the write. The form ships a dropdown with two
    /// options — `team:integrations-platform` (owner, allowed) and
    /// `team:marketing` (denied). An unrecognised value still gets policy-
    /// checked; we don't try to canonicalise here.
    #[serde(default)]
    pub actor: Option<String>,
}

pub async fn ui_write(
    State(state): State<AppState>,
    Form(form): Form<WriteForm>,
) -> Result<Markup, UiError> {
    let pool = state.require_pool()?;
    let provider = form.provider.trim();
    if provider.is_empty() {
        return Err(UiError::bad_request("provider is required"));
    }

    let entity_id = format!("bi_demo_{}", &Uuid::new_v4().simple().to_string()[..10]);
    let cmd = CreateBankIntegrationCmd {
        entity_id: entity_id.clone(),
        provider: provider.to_string(),
    };

    // F5: resolve actor + policy card; render allow OR deny panel.
    let actor = form
        .actor
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("team:integrations-platform");
    let policy_card = state
        .catalog
        .iter()
        .find(|c| c.fqn == entity_write::TYPE_BANK_INTEGRATION);

    match entity_write::apply_create_bank_integration_authzed(
        pool,
        &cmd,
        2,
        WriteOrigin::HttpHandler,
        actor,
        policy_card,
    )
    .await
    {
        Ok(outcome) => Ok(write_panel_with_actor(&outcome, actor)),
        Err(entity_write::WriteError::PolicyDenied(denial)) => Ok(deny_panel(&denial)),
        Err(entity_write::WriteError::Other(e)) => {
            Err(UiError::internal(format!("write failed: {e}")))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TamperForm {
    pub entity_id: String,
}

pub async fn ui_tamper(
    State(state): State<AppState>,
    Form(form): Form<TamperForm>,
) -> Result<Markup, UiError> {
    let pool = state.require_pool()?;
    let entity_id = form.entity_id.trim();
    if entity_id.is_empty() {
        return Err(UiError::bad_request("entity_id is required"));
    }

    // ============================================================
    // DEMO-ONLY: this path issues a raw SQL UPDATE that intentionally
    // bypasses entity_write (and therefore the mutation_log). It exists
    // so Beat 7 can show that even an out-of-band mutation is caught
    // by `agora verify`. NEVER EXPOSE THIS HANDLER OUTSIDE OF THE DEMO.
    // ============================================================
    let new_provider = "evil_corp_tampered";
    let res = sqlx::query("UPDATE bank_integrations SET provider = $1 WHERE id = $2")
        .bind(new_provider)
        .bind(entity_id)
        .execute(pool)
        .await
        .map_err(|e| UiError::internal(format!("tamper SQL failed: {e}")))?;

    if res.rows_affected() == 0 {
        return Err(UiError::bad_request(format!(
            "no bank_integrations row with id={entity_id:?}; write one first"
        )));
    }

    Ok(tamper_panel(entity_id, new_provider))
}

pub async fn ui_verify(
    State(state): State<AppState>,
) -> Result<Markup, UiError> {
    let pool = state.require_pool()?;
    let report = verify::verify(pool)
        .await
        .map_err(|e| UiError::internal(format!("verify failed: {e}")))?;
    Ok(verify_panel(&report))
}

// ============================================================================
// Markup helpers
// ============================================================================

fn page_layout(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width,initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/static/agora.css";
                script { (PreEscaped(HTMX_JS)) }
                script { (PreEscaped(TAB_SCRIPT)) }
            }
            body {
                header.topbar {
                    span.mark {}
                    span.wordmark { "agora" }
                    span.tagline { "Governed operational ontology — demo control plane" }
                    nav.nav {
                        a href="/" { "Home" }
                        a href="/ui/concepts" { "Concepts" }
                        a href="/health" { "Health" }
                    }
                }
                main.container { (body) }
                footer { "agora · feature-4-browser-demo-ui" }
            }
        }
    }
}

fn intro_block() -> Markup {
    html! {
        div.beat {
            h1 style="margin:0 0 6px; font-size:24px; letter-spacing:-0.01em;" {
                "Eight beats. One control plane. Real data."
            }
            p style="color:var(--text-muted); margin:0; font-size:14px;" {
                "Run each beat in order. Every artifact below is produced live by the same library functions that power the agorad daemon and the agora CLI — there is no demo-mode toggle."
            }
        }
    }
}

fn beat_section(num: &str, title: &str, sub: &str, body: Markup) -> Markup {
    html! {
        section.beat {
            div.beat-head {
                span.beat-num { (num) }
                h2.beat-title { (title) }
            }
            p.beat-sub { (sub) }
            div.beat-body { (body) }
        }
    }
}

fn nav_back(href: &str, label: &str) -> Markup {
    html! {
        p style="margin:0 0 16px;" {
            a href=(href) { (label) }
        }
    }
}

fn proposal_card(
    proposal: &OntologyChangeProposal,
    mode: &AuthorMode,
    manifest: &artifacts::ArtifactManifest,
    reuse_report: &ReuseReport,
) -> Markup {
    let tab_group = format!("artifacts-{}", proposal.id);
    html! {
        div.box.info style="margin-top:18px" {
            span.strong { "Proposal received." }
            div.body {
                "Authored by " span.id { (proposal.provenance.author) }
                " · target " span.id { (proposal.target().fqn()) }
                " · intent: " (proposal.change_intent)
            }
        }
        dl.kv {
            dt { "Proposal ID" }
            dd { span.id { (proposal.id) } }
            dt { "Author mode" }
            dd { (author_mode_pill(mode)) }
            dt { "Compatibility (declared)" }
            dd {
                span.mono {
                    "shape=" (format!("{:?}", proposal.compatibility.shape).to_lowercase())
                    " · semantic=" (format!("{:?}", proposal.compatibility.semantic).to_lowercase())
                    " · policy=" (format!("{:?}", proposal.compatibility.policy).to_lowercase())
                }
            }
            dt { "Meaning before" }
            dd { (proposal.semantic_contract.meaning_before) }
            dt { "Meaning after" }
            dd { (proposal.semantic_contract.meaning_after) }
        }

        h4 style="margin:18px 0 4px; font-size:13px; text-transform:uppercase; letter-spacing:0.05em; color:var(--text-muted);" { "Reuse detection (Beat 02)" }
        (reuse_block(reuse_report))

        h4 style="margin:18px 0 4px; font-size:13px; text-transform:uppercase; letter-spacing:0.05em; color:var(--text-muted);" { "Generated artifacts (Beat 04)" }
        p.hint { "Four real files written under " code { (manifest.directory) } "." }
        (artifact_tabs(&tab_group, manifest))

        div.row.right style="margin-top:18px" {
            button
                hx-post=(format!("/ui/proposals/{}/check", proposal.id))
                hx-target="#beat-3-slot"
                hx-swap="innerHTML"
                hx-disabled-elt="this"
            { "Run multi-axis check →" }
        }
    }
}

fn reuse_block(report: &ReuseReport) -> Markup {
    let class_label = format!("{:?}", report.class);
    let class_pill = match class_label.as_str() {
        "Duplicate" => "fail",
        "Refinement" => "warn",
        "Reuse" => "advisory",
        _ => "muted",
    };
    html! {
        div {
            span class=(format!("pill {class_pill}")) { (class_label) }
            span style="margin-left:10px; color:var(--text-muted); font-size:13px;" { (report.explanation) }
        }
        @if !report.top_hits.is_empty() {
            table.checks {
                thead { tr { th { "Existing concept" } th { "Layer" } th { "Score" } th { "Jaccard" } th { "Cosine" } } }
                tbody {
                    @for hit in &report.top_hits {
                        tr {
                            td.findings { span.id { (hit.fqn) } }
                            td.source { (hit.layer) }
                            td.elapsed { (format_score(hit.score)) }
                            td.elapsed { (format_score(hit.jaccard)) }
                            td.elapsed { (format_score(hit.cosine)) }
                        }
                    }
                }
            }
        }
    }
}

fn artifact_tabs(group: &str, manifest: &artifacts::ArtifactManifest) -> Markup {
    let entries: Vec<(&str, &str, String)> = vec![
        ("proto", ".proto", read_or_placeholder(&manifest.proto)),
        ("ddl", ".sql", read_or_placeholder(&manifest.ddl)),
        ("handler", "_handler.rs", read_or_placeholder(&manifest.handler)),
        ("policy", ".fga.json", read_or_placeholder(&manifest.openfga)),
    ];
    html! {
        div.tabs {
            @for (i, (name, label, _)) in entries.iter().enumerate() {
                button
                    type="button"
                    data-tab-trigger="1"
                    data-tab-group=(group)
                    data-tab-name=(*name)
                    class=(if i == 0 { "tab active" } else { "tab" })
                { (label) }
            }
        }
        @for (i, (name, _, body)) in entries.iter().enumerate() {
            div
                data-tab-group=(group)
                data-tab-name=(*name)
                class=(if i == 0 { "tab-panel active" } else { "tab-panel" })
            {
                pre.code { code { (body) } }
            }
        }
    }
}

fn check_report_panel(report: &CheckReport, proposal_id: &str, is_risky: bool) -> Markup {
    let dc = &report.data_conformance;
    let banner: Markup = if report.auto_approval_eligible {
        html! { div.box.success { span.strong { "All axes clean — auto-approval eligible." }
            div.body { "Total wall-clock " (report.elapsed_ms) " ms across " (report.checks.len()) " axes plus data-conformance." }
        } }
    } else {
        let reason = report.block_reason.clone().unwrap_or_else(|| "one or more axes failed".into());
        html! { div.box.error { span.strong { "Blocked." } " " (reason)
            div.body {
                @if dc.violations_found > 0 {
                    "Data-conformance counted "
                    span.strong { (dc.violations_found) }
                    " row(s) that would violate the proposed constraint. The proposal is not eligible for auto-approval."
                } @else {
                    "Review the per-axis rows below for details."
                }
            }
        } }
    };

    html! {
        (banner)
        // Falsifiability: render the timestamp + DC source verbatim so a
        // reviewer can mutate the DB, click again, and watch this line
        // change. The count comes from check::check, never from the template.
        p.hint style="margin-top:8px" {
            "Report generated at " span.mono { (report.generated_at) }
            " · " span.mono { (report.elapsed_ms) " ms total"}
            " · data_conformance source = " span.mono { (dc.source) }
            @if dc.applicable {
                " · live count = " span.mono { (dc.violations_found) }
            }
        }
        table.checks {
            thead {
                tr {
                    th { "Axis" }
                    th { "Outcome" }
                    th { "Findings" }
                    th { "Source" }
                    th { "Elapsed" }
                }
            }
            tbody {
                @for row in &report.checks {
                    tr {
                        td.axis { (axis_label(row.axis)) }
                        td.outcome { (outcome_pill(row.outcome)) }
                        td.findings { (row.findings) }
                        td.source { (row.source) }
                        td.elapsed { (row.elapsed_ms) " ms" }
                    }
                }
                tr {
                    td.axis { "data_conformance" }
                    td.outcome { (outcome_pill(dc.outcome)) }
                    td.findings { (data_conformance_findings(dc)) }
                    td.source { (dc.source) }
                    td.elapsed { (dc.query_time_ms) " ms" }
                }
            }
        }

        @if !dc.sample_violations.is_empty() {
            (sample_violations_block(&dc.sample_violations, dc.query.as_deref()))
        }

        @if !is_risky {
            div.row.right style="margin-top:18px" {
                button
                    hx-post=(format!("/ui/proposals/{proposal_id}/approve"))
                    hx-target="#beat-5-slot"
                    hx-swap="innerHTML"
                    hx-disabled-elt="this"
                { "Submit for approval →" }
            }
        }
    }
}

fn data_conformance_findings(dc: &DataConformance) -> Markup {
    if !dc.applicable {
        return html! { "Not applicable to this change kind." };
    }
    if matches!(dc.outcome, Outcome::Skipped) {
        return html! { "Skipped — " (dc.source) };
    }
    if dc.violations_found > 0 {
        html! {
            (dc.violations_found) " existing row(s) would violate the proposed constraint."
        }
    } else {
        html! { "No existing rows violate the proposed constraint." }
    }
}

fn sample_violations_block(samples: &[SampleViolation], query: Option<&str>) -> Markup {
    html! {
        div.box.warn style="margin-top:14px" {
            span.strong { "Sample violations (capped at " (samples.len()) ")" }
            table.data style="margin-top:8px" {
                thead { tr { th { "entity_id" } th { "reason" } } }
                tbody {
                    @for s in samples {
                        tr {
                            td.findings { span.id { (s.entity_id) } }
                            td.findings { (s.reason) }
                        }
                    }
                }
            }
            @if let Some(q) = query {
                p.hint style="margin-top:10px" { "Query that produced this count:" }
                pre.code { code { (q) } }
            }
        }
    }
}

fn approval_panel(report: &CheckReport) -> Markup {
    if report.auto_approval_eligible {
        html! {
            div.box.success {
                span.strong { "Auto-approved." }
                div.body {
                    "Every axis of " span.id { (report.proposal_id) }
                    " came back clean. The proposal is published; the four artifacts go live; ontology version increments. No human touched it."
                }
            }
            dl.kv {
                dt { "status" } dd { span.pill.pass { "approved" } }
                dt { "predicate" } dd { span.mono { "auto_approval::apply ⇒ all_axes_clean=true" } }
                dt { "report id" } dd { span.id { (report.proposal_id) } }
                dt { "generated_at" } dd { span.mono { (report.generated_at) } }
            }
        }
    } else {
        let reason = report.block_reason.clone().unwrap_or_else(|| "one or more axes failed".into());
        html! {
            div.box.error {
                span.strong { "Blocked." }
                div.body { (reason) }
            }
            dl.kv {
                dt { "status" } dd { span.pill.fail { "blocked" } }
                dt { "predicate" } dd { span.mono { "auto_approval::apply ⇒ all_axes_clean=false" } }
                dt { "report id" } dd { span.id { (report.proposal_id) } }
            }
        }
    }
}

/// Render the full AgentResult — one banner summarising the run outcome,
/// then one card per attempt. The cards intentionally stack vertically so
/// the demo audience reads them top-down as a conversation: "I proposed X
/// → blocked because Y → I revised to add Z → approved."
fn agent_result_panel(result: &AgentResult) -> Markup {
    let banner: Markup = match result.final_status {
        FinalStatus::Approved => html! {
            div.box.success {
                span.strong { "Approved after " (result.attempts.len()) " attempt(s)." }
                div.body {
                    "The agent loop closed: the final attempt is auto-approval-eligible. "
                    "All " (result.attempts.len()) " attempts are shown below — the revision trail is auditable."
                }
            }
        },
        FinalStatus::Stalled => html! {
            div.box.error {
                span.strong { "Stalled after " (result.attempts.len()) " attempt(s)." }
                div.body {
                    "No revision unblocked the gate within the attempt budget. "
                    "All attempts are shown below — the failures are the explanation."
                }
            }
        },
    };

    html! {
        (banner)
        p.hint style="margin-top:8px" {
            "Loop completed at " span.mono { (result.completed_at) }
            " · prompt = " span.mono { (result.prompt) }
        }
        @for attempt in &result.attempts {
            (attempt_card(attempt))
        }
    }
}

fn attempt_card(attempt: &Attempt) -> Markup {
    let eligible = attempt.check_report.auto_approval_eligible;
    let box_class = if eligible { "box success" } else { "box warn" };
    let action_label = match &attempt.action_taken {
        ActionTaken::Authored => "authored from prompt".to_string(),
        ActionTaken::Revised { reason } => format!("revised — {reason}"),
    };
    let target = attempt.proposal.target().fqn();
    let dc = &attempt.check_report.data_conformance;

    html! {
        div class=(box_class) style="margin-top:14px" {
            div.row style="justify-content:space-between; align-items:baseline" {
                div {
                    span.strong { "Attempt " (attempt.attempt_num) }
                    " — " (action_label)
                }
                div {
                    (author_mode_pill(&attempt.author_mode))
                    " "
                    @if eligible {
                        span.pill.pass { "approved" }
                    } @else {
                        span.pill.fail { "blocked" }
                    }
                }
            }
            dl.kv style="margin-top:10px" {
                dt { "Proposal" } dd { span.id { (attempt.proposal.id) } " · target " span.id { (target) } }
                dt { "Change intent" } dd { (attempt.proposal.change_intent) }
                @if let Some(mig) = &attempt.proposal.migration {
                    @if let Some(plan) = &mig.backfill_plan {
                        dt { "Backfill plan" }
                        dd {
                            span.mono { "strategy=" (plan.strategy) }
                            @if let Some(src) = &plan.source {
                                " · " span.mono { "source=" (src) }
                            }
                            " · " span.mono { "idempotent=" (plan.idempotent) }
                        }
                        @if let Some(rat) = &plan.rationale {
                            dt { "Backfill rationale" } dd { (rat) }
                        }
                        @if let Some(q) = &mig.backfill_query {
                            dt { "Backfill query" }
                            dd { pre.code style="margin:0" { code { (q) } } }
                        }
                    }
                }
                dt { "Block reason" }
                dd {
                    @if let Some(r) = &attempt.check_report.block_reason {
                        span.mono { (r) }
                    } @else {
                        "—"
                    }
                }
            }
            @if eligible {
                p.hint style="margin-top:8px" {
                    "All axes clean. data_conformance source = " span.mono { (dc.source) }
                    @if dc.applicable && dc.violations_found > 0 {
                        " · " span.mono { (dc.violations_found) }
                        " row(s) flagged but mitigated by backfill_plan (Advisory)."
                    }
                }
            } @else {
                p.hint style="margin-top:8px" {
                    "Gate blocked. The revision step gets this full report (block_reason + per-axis evidence) as input."
                }
            }
        }
    }
}

/// F5: write_panel + a small allow trace showing which policy tuple matched.
/// `actor` is what the form selected — used for the trace line. The
/// underlying mutation_log row already records the actor on `outcome`.
fn write_panel_with_actor(outcome: &WriteOutcome, actor: &str) -> Markup {
    html! {
        div.box.success {
            span.strong { "Allowed by policy → write committed." }
            div.body {
                "Actor " span.id { (actor) } " holds the "
                code { "owner" }
                " relation on " span.id { (outcome.entity_type) } "."
            }
        }
        dl.kv {
            dt { "policy decision" } dd { span.pill.pass { "allow" } }
            dt { "actor" } dd { span.mono { (actor) } }
            dt { "relation" } dd { span.mono { "owner" } }
            dt { "object" } dd { span.mono { "bank_integration:" (outcome.entity_id) } }
        }
        (write_panel(outcome))
    }
}

/// F5: deny panel. Red box + full policy trace + DenyAttempt seq so the
/// audience can see *who* tried, *what* policy rejected them, and *where*
/// in the audit log the denial lives. No tamper / verify buttons here —
/// nothing was written to the entity table; only the denial row exists.
fn deny_panel(denial: &entity_write::PolicyDeniedError) -> Markup {
    html! {
        div.box.error {
            span.strong { "Policy denied → write refused." }
            div.body { (denial.reason) }
        }
        dl.kv {
            dt { "policy decision" } dd { span.pill.fail { "deny" } }
            dt { "actor" } dd { span.mono { (denial.actor) } }
            dt { "relation" } dd { span.mono { (denial.relation) } }
            dt { "object" } dd { span.mono { (denial.object) } }
            @if let Some(seq) = denial.logged_seq {
                dt { "denial logged at seq" } dd { span.mono { (seq) } " (operation = " code { "DenyAttempt" } ", denial_reason persisted)" }
            }
        }
        @if !denial.considered.is_empty() {
            h4 style="margin:14px 0 4px; font-size:13px; text-transform:uppercase; letter-spacing:0.05em; color:var(--text-muted);" { "Tuples considered" }
            table.checks {
                thead { tr { th { "object" } th { "relation" } th { "user" } th { "outcome" } } }
                tbody {
                    @for t in &denial.considered {
                        tr {
                            td.findings { span.mono { (t.object) } }
                            td.findings { span.mono { (t.relation) } }
                            td.findings { span.mono { (t.user) } }
                            td.outcome {
                                @if t.user == denial.actor { span.pill.pass { "match user" } }
                                @else { span.pill.fail { "user mismatch" } }
                            }
                        }
                    }
                }
            }
        }
        p.hint style="margin-top:12px" {
            "Nothing was written to " code { "bank_integrations" } " — the policy check fires before the txn. "
            "The denial itself IS in " code { "mutation_log" } " (operation = " code { "DenyAttempt" } ") so the audit trail captures the attempt, the actor, and the rejection reason verbatim. Beat 7's verify will NOT flag this as drift; no entity row exists."
        }
    }
}

fn write_panel(outcome: &WriteOutcome) -> Markup {
    html! {
        div.box.success {
            span.strong { "Write committed." }
            div.body {
                "Row inserted into " span.id { (outcome.entity_type) }
                " plus an atomic mutation_log entry."
            }
        }
        dl.kv {
            dt { "entity_id" } dd { span.id { (outcome.entity_id) } }
            dt { "operation" } dd { span.mono { (outcome.operation) } }
            dt { "ontology_version" } dd { span.mono { (outcome.ontology_version) } }
            dt { "mutation_seq" } dd { span.mono { (outcome.mutation_seq) } }
            dt { "checksum (SHA-256)" } dd { span.mono style="word-break:break-all" { (outcome.checksum) } }
            dt { "actor" } dd { span.mono { (outcome.actor) } }
        }
        div.row style="margin-top:18px; gap:12px" {
            form
                hx-post="/ui/tamper"
                hx-target="#beat-7b-slot"
                hx-swap="innerHTML"
                hx-disabled-elt="find button"
            {
                input type="hidden" name="entity_id" value=(outcome.entity_id) {}
                button.danger type="submit" { "Tamper this row out-of-band →" }
            }
            button
                class="secondary"
                hx-get="/ui/verify"
                hx-target="#beat-7c-slot"
                hx-swap="innerHTML"
                hx-disabled-elt="this"
            { "Run agora verify" }
        }
        p.hint { "Tamper issues a raw SQL UPDATE that bypasses the handler entirely. Verify reproduces the canonical-JSON checksum from the live row and compares it to the logged one." }
    }
}

fn tamper_panel(entity_id: &str, new_provider: &str) -> Markup {
    html! {
        div.box.warn {
            span.strong { "Out-of-band UPDATE issued." }
            div.body {
                "Row " span.id { (entity_id) } " had its " code { "provider" }
                " column changed to " code { (new_provider) }
                " via raw SQL — the mutation_log was NOT updated. The control plane no longer agrees with the database."
            }
        }
        pre.code style="margin-top:10px" { code {
            "UPDATE bank_integrations SET provider = '" (new_provider) "' WHERE id = '" (entity_id) "';"
        } }
        p.hint style="margin-top:6px" {
            "(SQL shown above is the logical statement; the handler binds the values via "
            code { "sqlx::query(...).bind(...)" }
            " — parameterized, not concatenated.)"
        }
        div.row.right style="margin-top:14px" {
            button
                hx-get="/ui/verify"
                hx-target="#beat-7c-slot"
                hx-swap="innerHTML"
                hx-disabled-elt="this"
            { "Run agora verify →" }
        }
    }
}

fn verify_panel(report: &VerifyReport) -> Markup {
    let ok = matches!(report.verify_status, VerifyStatus::Clean);
    let banner = if ok {
        html! { div.box.success { span.strong { "Clean." }
            div.body { "Checked " (report.entities_checked) " entities in " (report.elapsed_ms) " ms. No drift." }
        } }
    } else {
        let drift_n = report.tampered_entities.len();
        let oob_n = report.outofband_entities.len();
        html! { div.box.error { span.strong { "Drift detected." }
            div.body {
                (drift_n) " tampered row(s) and " (oob_n) " out-of-band row(s) across " (report.entities_checked) " entities. The control plane caught what raw SQL changed."
            }
        } }
    };
    html! {
        (banner)
        p.hint style="margin-top:8px" {
            "Live verify run at " span.mono { (report.timestamp.to_rfc3339()) } " — every click re-queries Postgres and recomputes checksums."
        }
        @if !report.tampered_entities.is_empty() {
            h4 style="margin:14px 0 4px; font-size:13px; text-transform:uppercase; letter-spacing:0.05em; color:var(--text-muted);" { "Tampered rows" }
            // For each tampered entity render a per-row card so the
            // expected-vs-actual values are visible field by field — that's
            // the proof reviewers need: not just "drift detected" but
            // exactly which value changed and what it changed from.
            @for d in &report.tampered_entities {
                div.box.error style="margin-top:10px" {
                    div {
                        span.strong { "Drift " }
                        span.id { (d.entity_type) } " · " span.id { (d.entity_id) }
                    }
                    dl.kv style="margin-top:8px" {
                        dt { "logged at" } dd { span.mono { (d.last_logged_at.to_rfc3339()) } " (seq " (d.last_logged_mutation_seq) ")" }
                        dt { "logged actor" } dd { span.mono { (d.last_logged_actor) } }
                        dt { "detected via" } dd { span.mono { (d.detected_via) } }
                        dt { "logged checksum" } dd { span.mono style="word-break:break-all" { (d.logged_checksum.clone().unwrap_or_else(|| "—".into())) } }
                        dt { "current checksum" } dd { span.mono style="word-break:break-all" { (d.current_checksum) } }
                    }
                    h5 style="margin:14px 0 6px; font-size:11.5px; text-transform:uppercase; letter-spacing:0.05em; color:var(--text-muted);" {
                        "Field-level diff (logged vs. live)"
                    }
                    table.data {
                        thead { tr { th { "field" } th { "logged value (in mutation_log)" } th { "current value (in row)" } } }
                        tbody {
                            @if d.fields_changed.is_empty() {
                                tr { td.findings colspan="3" { "(checksum mismatch but no top-level fields differ — payload-level drift)" } }
                            }
                            @for field in &d.fields_changed {
                                tr {
                                    td.findings { span.mono { (field) } }
                                    td.findings { span.mono { (json_field_or_dash(&d.logged_state, field)) } }
                                    td.findings { span.mono style="color:var(--error)" { (json_field_or_dash(&d.current_state, field)) } }
                                }
                            }
                        }
                    }
                }
            }
        }
        @if !report.outofband_entities.is_empty() {
            h4 style="margin:14px 0 4px; font-size:13px; text-transform:uppercase; letter-spacing:0.05em; color:var(--text-muted);" { "Created out-of-band" }
            table.data {
                thead { tr { th { "type" } th { "entity_id" } th { "issue" } th { "current checksum" } } }
                tbody {
                    @for o in &report.outofband_entities {
                        tr {
                            td.findings { span.id { (o.entity_type) } }
                            td.findings { span.id { (o.entity_id) } }
                            td.findings { (o.issue) }
                            td.findings { span.mono style="word-break:break-all" { (truncate_checksum(Some(&o.current_checksum))) } }
                        }
                    }
                }
            }
        }
    }
}

fn concept_view_markup(view: &ConceptView, has_db: bool) -> Markup {
    html! {
        div.beat {
            div.beat-head {
                span.beat-num { "08" }
                h2.beat-title { (view.fqn) }
            }
            p.beat-sub { (view.doc.clone().unwrap_or_else(|| "—".into())) }

            dl.kv {
                dt { "namespace" } dd { span.mono { (view.namespace) } }
                dt { "name" } dd { span.mono { (view.name) } }
                dt { "version" } dd { span.mono { (view.version) " (" (view.status) ")" } }
                dt { "owner" } dd {
                    span.mono { (view.ownership.team) }
                    @if let Some(s) = &view.ownership.semantic_steward {
                        " · semantic steward: " span.mono { (s) }
                    }
                }
                dt { "policy class" } dd { span.pill.muted { (format!("{:?}", view.policy_class)) } }
            }
        }

        div.beat {
            div.explorer-section {
                h3 { "Fields" }
                table.data {
                    thead { tr { th { "name" } th { "type" } th { "required" } th { "since" } th { "classification" } th { "doc" } } }
                    tbody {
                        @for f in &view.fields {
                            tr {
                                td.findings { span.mono { (f.name) } }
                                td.findings { span.mono { (f.proto_type) } }
                                td.findings { @if f.required { "✓" } @else { "—" } }
                                td.elapsed { "v" (f.since_version) }
                                td.findings { span.pill.muted { (format!("{:?}", f.classification)) } }
                                td.findings { (f.doc.clone().unwrap_or_default()) }
                            }
                        }
                    }
                }
            }

            div.explorer-section {
                h3 { "Invariants" }
                @if view.invariants.is_empty() {
                    p.hint { "None declared." }
                } @else {
                    ul.invariants {
                        @for inv in &view.invariants { li { (inv) } }
                    }
                }
            }

            div.explorer-section {
                h3 { "Lineage" }
                dl.kv {
                    dt { "HTTP route" } dd { span.mono { (view.lineage.http_route) } }
                    dt { "storage table" } dd { span.mono { (view.lineage.storage_table) } }
                    dt { "proto" } dd { span.mono { (view.lineage.proto_artifact) } }
                    dt { "policy spec" } dd { span.mono { (view.lineage.policy_artifact) } }
                }
                @if !view.lineage.references.is_empty() {
                    h4 style="margin:12px 0 4px; font-size:12px; color:var(--text-muted); text-transform:uppercase; letter-spacing:0.05em;" { "References" }
                    ul.references {
                        @for r in &view.lineage.references { li { span.mono { (r) } } }
                    }
                }
                @if !view.lineage.touched_by_proposals.is_empty() {
                    h4 style="margin:12px 0 4px; font-size:12px; color:var(--text-muted); text-transform:uppercase; letter-spacing:0.05em;" { "Touched by proposals" }
                    ul.touched {
                        @for p in &view.lineage.touched_by_proposals { li { span.id { (p) } } }
                    }
                }
            }

            div.explorer-section {
                h3 { "Policy attachments" }
                @if view.policy_examples.is_empty() {
                    p.hint { "Public — no per-relation tuples." }
                } @else {
                    table.data {
                        thead { tr { th { "relation" } th { "subject" } th { "object" } } }
                        tbody {
                            @for p in &view.policy_examples {
                                tr {
                                    td.findings { span.mono { (p.relation) } }
                                    td.findings { span.mono { (p.subject) } }
                                    td.findings { span.mono { (p.object) } }
                                }
                            }
                        }
                    }
                }
            }

            div.explorer-section {
                h3 { "Version history" }
                @if !has_db {
                    div.box.warn {
                        span.strong { "DB unavailable." }
                        div.body { "Version history is read live from mutation_log; agorad has no DATABASE_URL set." }
                    }
                } @else if view.version_history.is_empty() {
                    p.hint { "No mutations recorded for this type yet." }
                } @else {
                    table.history {
                        thead { tr { th { "seq" } th { "operation" } th { "ontology_v" } th { "entity_id" } th { "actor" } th { "occurred_at (UTC)" } th { "checksum" } } }
                        tbody {
                            @for h in &view.version_history {
                                tr {
                                    td { (h.mutation_seq) }
                                    td { (h.operation) }
                                    td { "v" (h.ontology_version) }
                                    td { (h.entity_id) }
                                    td { (h.actor) }
                                    td { (h.occurred_at.to_rfc3339()) }
                                    td { (truncate_checksum(h.checksum.as_deref())) }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// Small utilities
// ============================================================================

fn outcome_pill(o: Outcome) -> Markup {
    let (cls, label) = match o {
        Outcome::Pass => ("pass", "pass"),
        Outcome::Advisory => ("advisory", "advisory"),
        Outcome::Fail => ("fail", "fail"),
        Outcome::Skipped => ("skipped", "skipped"),
    };
    html! { span class=(format!("pill {cls}")) { (label) } }
}

fn author_mode_pill(mode: &AuthorMode) -> Markup {
    match mode {
        AuthorMode::Live => html! { span.pill.live { "live · LLM-derived" } },
        AuthorMode::OfflineNoKey => html! { span.pill.offline { "offline · no API key" } },
        AuthorMode::OfflineApiError { error } => {
            html! { span.pill.offline title=(error) { "offline · API error" } }
        }
    }
}

fn axis_label(a: Axis) -> &'static str {
    a.as_str()
}

fn format_score(f: f32) -> String {
    format!("{:.2}", f)
}

fn truncate_checksum(c: Option<&str>) -> String {
    match c {
        Some(s) if s.len() > 12 => format!("{}…", &s[..12]),
        Some(s) => s.to_string(),
        None => "—".into(),
    }
}

/// Render the value of `field` from a JSON object as a compact string. Used
/// by the verify panel to surface logged-vs-current values for tampered
/// rows. Returns "—" for missing keys and JSON null; primitives render
/// without quotes; arrays/objects fall back to their compact JSON form so
/// nested drift is still legible.
fn json_field_or_dash(v: &serde_json::Value, field: &str) -> String {
    match v.get(field) {
        None => "—".into(),
        Some(serde_json::Value::Null) => "null".into(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

fn read_or_placeholder(path: &str) -> String {
    match std::fs::read_to_string(PathBuf::from(path)) {
        Ok(s) => s,
        Err(e) => format!("// could not read {path}: {e}"),
    }
}

fn load_proposal(path: &Path) -> Result<OntologyChangeProposal> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("parsing proposal JSON at {}", path.display()))
}

// ============================================================================
// Error handling — UI variant: returns HTML, not JSON.
// ============================================================================

#[derive(Debug)]
pub struct UiError {
    pub status: StatusCode,
    pub message: String,
}

impl UiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: msg.into() }
    }
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: msg.into() }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        let m = msg.into();
        tracing::error!("ui error: {m}");
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: m }
    }
    pub fn service_unavailable(msg: impl Into<String>) -> Self {
        Self { status: StatusCode::SERVICE_UNAVAILABLE, message: msg.into() }
    }
}

impl IntoResponse for UiError {
    fn into_response(self) -> Response {
        let body = html! {
            div.box.error {
                span.strong { "Error " (self.status.as_u16()) "." }
                div.body { (self.message) }
            }
        };
        (self.status, body).into_response()
    }
}

// Internal helper exposed to the UI module for require_pool. The daemon's
// `require_db` uses a JSON-formatted ApiError, so we redefine the predicate
// here producing a UiError instead.
trait AppStateUi {
    fn require_pool(&self) -> Result<&PgPool, UiError>;
}

impl AppStateUi for AppState {
    fn require_pool(&self) -> Result<&PgPool, UiError> {
        self.pool.as_ref().ok_or_else(|| {
            UiError::service_unavailable(
                "agorad has no Postgres connection (set DATABASE_URL on launch)",
            )
        })
    }
}
