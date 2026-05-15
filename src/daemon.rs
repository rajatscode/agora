//! `agorad` HTTP control plane.
//!
//! Thin Axum server that wraps the F1/F2/F3 library entry points. The CLI
//! (`agora ...`) and this daemon are two front doors onto the same library
//! functions: there is no business logic in this file, just request parsing,
//! lib-call wiring, and JSON serialization.
//!
//! Endpoints (all return JSON):
//!   - GET  /health                                    — liveness + DB ping
//!   - POST /proposals                                 — F1 author_proposal
//!   - GET  /proposals                                 — list on disk
//!   - GET  /proposals/{id}                            — proposal + artifact paths
//!   - POST /proposals/{id}/check                      — F2 check::check
//!   - GET  /proposals/{id}/check_report               — cached CheckReport
//!   - POST /proposals/{id}/approve                    — verdict-based approval
//!   - POST /entities/{type}                           — F3 entity_write
//!   - GET  /verify                                    — F3 verify
//!   - GET  /concepts                                  — seed catalog list
//!   - GET  /concepts/{fqn}                            — F3 explorer

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::agent;
use crate::artifacts;
use crate::ast::OntologyChangeProposal;
use crate::check;
use crate::check_report::CheckReport;
use crate::db;
use crate::entity_write::{
    self, CreateAuditFindingCmd, CreateBankIntegrationCmd, CreateCustomerCmd, WriteOrigin,
    TYPE_AUDIT_FINDING, TYPE_BANK_INTEGRATION, TYPE_CUSTOMER,
};
use crate::explorer;
use crate::llm;
use crate::seed::{self, ConceptCard};
use crate::ui;
use crate::verify;

/// Process-wide handle: a connected pool (or `None` if no DB), the seed
/// concept catalog (cached at boot, never mutated at runtime), and the
/// directory under which generated proposal artifacts live.
#[derive(Clone)]
pub struct AppState {
    pub pool: Option<PgPool>,
    pub catalog: Arc<Vec<ConceptCard>>,
    pub generated_root: PathBuf,
}

impl AppState {
    pub fn new(pool: Option<PgPool>, generated_root: PathBuf) -> Self {
        Self {
            pool,
            catalog: Arc::new(seed::baseline_concepts()),
            generated_root,
        }
    }

    fn require_db(&self) -> Result<&PgPool, ApiError> {
        self.pool.as_ref().ok_or_else(|| {
            ApiError::service_unavailable(
                "database_unavailable",
                "agorad has no Postgres connection (set DATABASE_URL on launch)",
            )
        })
    }
}

/// Build the Axum router. Exposed so integration tests can mount the same
/// router against an `axum::serve(TcpListener, app)` without going through
/// `serve_forever`.
///
/// Mounts both the JSON control-plane API (`/proposals`, `/entities`, etc.)
/// and the Feature-4 browser UI (`/`, `/ui/*`, `/static/*`). The UI handlers
/// live in `crate::ui` and wrap the SAME library functions the JSON handlers
/// call; nothing is duplicated.
pub fn router(state: AppState) -> Router {
    Router::new()
        // JSON control-plane API
        .route("/health", get(health))
        .route("/proposals", post(create_proposal).get(list_proposals))
        .route("/proposals/:id", get(get_proposal))
        .route("/proposals/:id/check", post(run_check))
        .route("/proposals/:id/check_report", get(get_check_report))
        .route("/proposals/:id/approve", post(approve_proposal))
        .route("/entities/:type_name", post(write_entity))
        .route("/verify", get(run_verify))
        .route("/concepts", get(list_concepts))
        .route("/concepts/:fqn", get(get_concept))
        // F6 — closed-loop agentic revision
        .route("/agent/run", post(run_agent_loop))
        // Browser UI (Feature 4)
        .route("/", get(ui::home))
        .route("/ui/propose", post(ui::ui_propose))
        .route("/ui/proposals/:id/check", post(ui::ui_check))
        .route("/ui/proposals/:id/approve", post(ui::ui_approve))
        .route("/ui/risky-proposal", post(ui::ui_risky_proposal))
        .route("/ui/write", post(ui::ui_write))
        .route("/ui/tamper", post(ui::ui_tamper))
        .route("/ui/verify", get(ui::ui_verify))
        .route("/ui/concepts", get(ui::concepts_index))
        .route("/ui/concepts/:fqn", get(ui::concept_view_page))
        .route("/ui/agent", post(ui::ui_agent_run))
        .route("/static/agora.css", get(ui::css_handler))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Boot the daemon: connect to Postgres, run migrations, build state, listen
/// on `addr`. Used by the `agorad` bin entry point.
pub async fn serve_forever(
    addr: SocketAddr,
    db_url: Option<String>,
    generated_root: PathBuf,
    skip_migrate: bool,
) -> Result<()> {
    let pool = db::connect_optional(db_url.as_deref()).await?;
    if let Some(p) = &pool {
        if !skip_migrate {
            tracing::info!("running migrations");
            db::migrate(p).await.context("running migrations")?;
        }
    } else {
        tracing::warn!("DATABASE_URL not reachable — write/verify endpoints will return 503");
    }

    std::fs::create_dir_all(&generated_root)
        .with_context(|| format!("creating generated dir {}", generated_root.display()))?;

    let state = AppState::new(pool, generated_root);
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding to {addr}"))?;
    tracing::info!("agorad listening on http://{addr}");
    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

// ---------- handlers ----------

async fn health(State(state): State<AppState>) -> Json<Value> {
    let db = match &state.pool {
        Some(pool) => match sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool).await {
            Ok(_) => "connected",
            Err(_) => "unreachable",
        },
        None => "disconnected",
    };
    Json(json!({ "status": "ok", "db": db }))
}

#[derive(Debug, Deserialize)]
struct CreateProposalReq {
    prompt: String,
    #[serde(default = "default_actor")]
    actor: String,
}

fn default_actor() -> String {
    "agent://agorad-http".into()
}

#[derive(Debug, Serialize)]
struct CreateProposalResp {
    proposal: OntologyChangeProposal,
    author_mode: llm::AuthorMode,
    artifacts: artifacts::ArtifactManifest,
    proposal_path: String,
}

async fn create_proposal(
    State(state): State<AppState>,
    Json(req): Json<CreateProposalReq>,
) -> Result<Json<CreateProposalResp>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::bad_request("empty_prompt", "prompt must not be empty"));
    }

    let (proposal, author_mode) = llm::author_proposal(&req.prompt, &req.actor)
        .await
        .map_err(|e| ApiError::internal("author_failed", e))?;

    let manifest = artifacts::emit_all(&proposal, &state.generated_root)
        .map_err(|e| ApiError::internal("emit_artifacts_failed", e))?;

    let proposal_path = Path::new(&manifest.directory).join("proposal.json");
    let bytes = serde_json::to_vec_pretty(&proposal)
        .map_err(|e| ApiError::internal("serialize_failed", e))?;
    std::fs::write(&proposal_path, bytes)
        .with_context(|| format!("writing proposal to {}", proposal_path.display()))
        .map_err(|e| ApiError::internal("write_proposal_failed", e))?;

    Ok(Json(CreateProposalResp {
        proposal,
        author_mode,
        artifacts: manifest,
        proposal_path: proposal_path.to_string_lossy().into_owned(),
    }))
}

#[derive(Debug, Serialize)]
struct ProposalSummary {
    id: String,
    target: String,
    change_intent: String,
    has_check_report: bool,
    artifacts_dir: String,
}

async fn list_proposals(
    State(state): State<AppState>,
) -> Result<Json<Vec<ProposalSummary>>, ApiError> {
    let mut summaries = Vec::new();
    if !state.generated_root.exists() {
        return Ok(Json(summaries));
    }
    let read_dir = std::fs::read_dir(&state.generated_root)
        .with_context(|| format!("reading {}", state.generated_root.display()))
        .map_err(|e| ApiError::internal("read_generated_dir_failed", e))?;
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let proposal_path = path.join("proposal.json");
        if !proposal_path.exists() {
            continue;
        }
        match load_proposal(&proposal_path) {
            Ok(p) => summaries.push(ProposalSummary {
                id: p.id.clone(),
                target: p.target().fqn(),
                change_intent: p.change_intent.clone(),
                has_check_report: path.join("check_report.json").exists(),
                artifacts_dir: path.to_string_lossy().into_owned(),
            }),
            Err(e) => tracing::warn!("skipping unreadable proposal {}: {e}", proposal_path.display()),
        }
    }
    summaries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(summaries))
}

#[derive(Debug, Serialize)]
struct ProposalDetail {
    proposal: OntologyChangeProposal,
    artifacts_dir: String,
    artifact_files: Vec<ArtifactFile>,
    check_report: Option<CheckReport>,
}

#[derive(Debug, Serialize)]
struct ArtifactFile {
    name: String,
    path: String,
    contents: String,
}

async fn get_proposal(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ProposalDetail>, ApiError> {
    let dir = state.generated_root.join(&id);
    let proposal_path = dir.join("proposal.json");
    if !proposal_path.exists() {
        return Err(ApiError::not_found(
            "proposal_not_found",
            format!("no proposal at {}", proposal_path.display()),
        ));
    }
    let proposal = load_proposal(&proposal_path)
        .map_err(|e| ApiError::internal("load_proposal_failed", e))?;

    let mut artifact_files = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(&dir) {
        for entry in read_dir.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            // proposal.json + check_report.json travel in their own fields.
            if name == "proposal.json" || name == "check_report.json" {
                continue;
            }
            match std::fs::read_to_string(&p) {
                Ok(contents) => artifact_files.push(ArtifactFile {
                    name,
                    path: p.to_string_lossy().into_owned(),
                    contents,
                }),
                Err(e) => tracing::warn!("could not read artifact {}: {e}", p.display()),
            }
        }
    }
    artifact_files.sort_by(|a, b| a.name.cmp(&b.name));

    let check_report = load_check_report(&dir)
        .map_err(|e| ApiError::internal("load_check_report_failed", e))?;

    Ok(Json(ProposalDetail {
        proposal,
        artifacts_dir: dir.to_string_lossy().into_owned(),
        artifact_files,
        check_report,
    }))
}

async fn run_check(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<CheckReport>, ApiError> {
    let dir = state.generated_root.join(&id);
    let proposal_path = dir.join("proposal.json");
    if !proposal_path.exists() {
        return Err(ApiError::not_found(
            "proposal_not_found",
            format!("no proposal at {}", proposal_path.display()),
        ));
    }
    let proposal = load_proposal(&proposal_path)
        .map_err(|e| ApiError::internal("load_proposal_failed", e))?;

    let report = check::check(&proposal, &state.catalog, state.pool.as_ref())
        .await
        .map_err(|e| ApiError::internal("check_failed", e))?;

    // Persist next to the proposal so GET /check_report can serve a cache.
    let report_path = dir.join("check_report.json");
    let bytes = serde_json::to_vec_pretty(&report)
        .map_err(|e| ApiError::internal("serialize_failed", e))?;
    if let Err(e) = std::fs::write(&report_path, bytes) {
        tracing::warn!("could not persist CheckReport to {}: {e}", report_path.display());
    }

    Ok(Json(report))
}

async fn get_check_report(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<CheckReport>, ApiError> {
    let dir = state.generated_root.join(&id);
    let report_path = dir.join("check_report.json");
    if !report_path.exists() {
        return Err(ApiError::not_found(
            "check_report_not_found",
            format!("no cached report at {}", report_path.display()),
        ));
    }
    let raw = std::fs::read_to_string(&report_path)
        .with_context(|| format!("reading {}", report_path.display()))
        .map_err(|e| ApiError::internal("read_check_report_failed", e))?;
    let report: CheckReport =
        serde_json::from_str(&raw).map_err(|e| ApiError::internal("parse_check_report_failed", e))?;
    Ok(Json(report))
}

#[derive(Debug, Serialize)]
struct ApprovalResp {
    proposal_id: String,
    approved: bool,
    auto_approval_eligible: bool,
    block_reason: Option<String>,
    status: String,
}

async fn approve_proposal(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ApprovalResp>, ApiError> {
    let dir = state.generated_root.join(&id);
    let report_path = dir.join("check_report.json");
    if !report_path.exists() {
        return Err(ApiError::bad_request(
            "check_required",
            "run POST /proposals/{id}/check before approving",
        ));
    }
    let raw = std::fs::read_to_string(&report_path)
        .with_context(|| format!("reading {}", report_path.display()))
        .map_err(|e| ApiError::internal("read_check_report_failed", e))?;
    let report: CheckReport =
        serde_json::from_str(&raw).map_err(|e| ApiError::internal("parse_check_report_failed", e))?;

    let approved = report.auto_approval_eligible;
    Ok(Json(ApprovalResp {
        proposal_id: id,
        approved,
        auto_approval_eligible: report.auto_approval_eligible,
        block_reason: report.block_reason.clone(),
        status: if approved { "approved".into() } else { "blocked".into() },
    }))
}

#[derive(Debug, Deserialize)]
struct WriteEntityReq {
    entity_id: String,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default = "default_ontology_version")]
    ontology_version: i32,
    /// F5: optional actor — defaults to the owner team so legacy clients
    /// (e.g. existing daemon_http.rs test) keep working without changes.
    #[serde(default)]
    actor: Option<String>,
    // F8: Customer 360 fields. Optional on purpose — see CreateCustomerCmd.
    // Only consumed when the dispatched type is Customer.
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    signup_source: Option<String>,
    // F9: Compliance / GRC fields. Only consumed when the dispatched
    // type is AuditFinding. Required fields (`rule_id`, `severity`,
    // `status`, `opened_at`) are explicitly required by the handler;
    // `resolved_at` and `notes` are optional and Option<String>-shaped.
    #[serde(default)]
    rule_id: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    opened_at: Option<String>,
    #[serde(default)]
    resolved_at: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

fn default_ontology_version() -> i32 {
    2
}

async fn write_entity(
    State(state): State<AppState>,
    AxumPath(type_name): AxumPath<String>,
    Json(req): Json<WriteEntityReq>,
) -> Result<Json<entity_write::WriteOutcome>, ApiError> {
    let pool = state.require_db()?;

    // Accept either the bare PascalCase type ("BankIntegration", "Customer",
    // "AuditFinding") or the fully-qualified name. Anything else → 400.
    let resolved = match type_name.as_str() {
        "BankIntegration" | TYPE_BANK_INTEGRATION => TYPE_BANK_INTEGRATION,
        // F8: Customer 360 domain.
        "Customer" | TYPE_CUSTOMER => TYPE_CUSTOMER,
        // F9: Compliance / GRC domain.
        "AuditFinding" | TYPE_AUDIT_FINDING => TYPE_AUDIT_FINDING,
        other => {
            return Err(ApiError::bad_request(
                "unsupported_entity_type",
                format!(
                    "unsupported entity type {other:?}; supported: BankIntegration, Customer, AuditFinding"
                ),
            ));
        }
    };

    if req.entity_id.trim().is_empty() {
        return Err(ApiError::bad_request("missing_entity_id", "entity_id is required"));
    }

    // F9: AuditFinding dispatch — same shape as Customer, different policy
    // card + apply_* function.
    if resolved == TYPE_AUDIT_FINDING {
        // Required-field validation. We surface 400s for missing required
        // fields instead of letting Postgres reject the INSERT with a 500.
        let rule_id = req.rule_id.clone().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            ApiError::bad_request("missing_rule_id", "rule_id is required for AuditFinding")
        })?;
        let severity = req.severity.clone().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            ApiError::bad_request("missing_severity", "severity is required for AuditFinding")
        })?;
        let status = req.status.clone().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            ApiError::bad_request("missing_status", "status is required for AuditFinding")
        })?;
        let opened_at = req.opened_at.clone().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            ApiError::bad_request(
                "missing_opened_at",
                "opened_at (RFC3339) is required for AuditFinding",
            )
        })?;

        let cmd = CreateAuditFindingCmd {
            entity_id: req.entity_id,
            rule_id,
            severity,
            status,
            opened_at,
            resolved_at: req.resolved_at.filter(|s| !s.trim().is_empty()),
            notes: req.notes.filter(|s| !s.trim().is_empty()),
        };
        let actor = req
            .actor
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("team:compliance-platform");
        let policy_card = state.catalog.iter().find(|c| c.fqn == TYPE_AUDIT_FINDING);
        let outcome = entity_write::apply_create_audit_finding_authzed(
            pool,
            &cmd,
            req.ontology_version,
            WriteOrigin::HttpHandler,
            actor,
            policy_card,
        )
        .await
        .map_err(|e| match e {
            entity_write::WriteError::PolicyDenied(denial) => ApiError::policy_denied(denial),
            entity_write::WriteError::Other(err) => ApiError::internal("write_failed", err),
        })?;
        return Ok(Json(outcome));
    }

    // F8: Customer dispatch — same shape as BankIntegration, different
    // policy card + apply_* function. The agent loop / risk gate / verify
    // don't have to know which branch was taken.
    if resolved == TYPE_CUSTOMER {
        let cmd = CreateCustomerCmd {
            entity_id: req.entity_id,
            email: req.email.filter(|s| !s.trim().is_empty()),
            display_name: req.display_name.filter(|s| !s.trim().is_empty()),
            signup_source: req.signup_source.filter(|s| !s.trim().is_empty()),
        };
        let actor = req
            .actor
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("team:customer-platform");
        let policy_card = state.catalog.iter().find(|c| c.fqn == TYPE_CUSTOMER);
        let outcome = entity_write::apply_create_customer_authzed(
            pool,
            &cmd,
            req.ontology_version,
            WriteOrigin::HttpHandler,
            actor,
            policy_card,
        )
        .await
        .map_err(|e| match e {
            entity_write::WriteError::PolicyDenied(denial) => ApiError::policy_denied(denial),
            entity_write::WriteError::Other(err) => ApiError::internal("write_failed", err),
        })?;
        return Ok(Json(outcome));
    }

    if resolved == TYPE_BANK_INTEGRATION {
        let provider = req
            .provider
            .ok_or_else(|| ApiError::bad_request("missing_provider", "provider is required for BankIntegration"))?;
        if provider.trim().is_empty() {
            return Err(ApiError::bad_request("missing_provider", "provider is required for BankIntegration"));
        }
        let cmd = CreateBankIntegrationCmd {
            entity_id: req.entity_id,
            provider,
        };

        // F5: resolve actor. Default to the owning team so existing tests
        // (and any client that hasn't been updated yet) keep getting Allow.
        let actor = req
            .actor
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("team:integrations-platform");
        // Find the policy card for BankIntegration in the catalog. Always
        // present in the seed catalog; if not, the policy check is skipped
        // (None) and we keep historical behaviour.
        let policy_card = state
            .catalog
            .iter()
            .find(|c| c.fqn == TYPE_BANK_INTEGRATION);

        let outcome = entity_write::apply_create_bank_integration_authzed(
            pool,
            &cmd,
            req.ontology_version,
            WriteOrigin::HttpHandler,
            actor,
            policy_card,
        )
        .await
        .map_err(|e| match e {
            entity_write::WriteError::PolicyDenied(denial) => {
                ApiError::policy_denied(denial)
            }
            entity_write::WriteError::Other(err) => ApiError::internal("write_failed", err),
        })?;
        return Ok(Json(outcome));
    }

    unreachable!("type_name was validated above")
}

async fn run_verify(
    State(state): State<AppState>,
) -> Result<Json<verify::VerifyReport>, ApiError> {
    let pool = state.require_db()?;
    let report = verify::verify(pool)
        .await
        .map_err(|e| ApiError::internal("verify_failed", e))?;
    Ok(Json(report))
}

#[derive(Debug, Serialize)]
struct ConceptSummary {
    fqn: String,
    namespace: String,
    name: String,
    version: u32,
    team: String,
}

async fn list_concepts(State(state): State<AppState>) -> Json<Vec<ConceptSummary>> {
    let summaries = state
        .catalog
        .iter()
        .map(|c| ConceptSummary {
            fqn: c.fqn.clone(),
            namespace: c.spec.namespace.clone(),
            name: c.spec.name.clone(),
            version: c.spec.version,
            team: c.spec.ownership.team.clone(),
        })
        .collect();
    Json(summaries)
}

#[derive(Debug, Deserialize)]
struct AgentRunReq {
    prompt: String,
}

/// F6 — run the closed-loop agentic revision against `prompt`. Returns the
/// full `AgentResult` with every attempt's proposal + CheckReport so callers
/// can render the revision trace.
async fn run_agent_loop(
    State(state): State<AppState>,
    Json(req): Json<AgentRunReq>,
) -> Result<Json<agent::AgentResult>, ApiError> {
    if req.prompt.trim().is_empty() {
        return Err(ApiError::bad_request("empty_prompt", "prompt must not be empty"));
    }
    let result = agent::agent_loop(&req.prompt, state.catalog.as_slice(), state.pool.as_ref())
        .await
        .map_err(|e| ApiError::internal("agent_loop_failed", e))?;

    // Persist the final-attempt proposal + report next to the others so the
    // existing /proposals listing surfaces them, and so the UI can deep-link.
    if let Some(final_attempt) = result.attempts.last() {
        let dir = state.generated_root.join(&final_attempt.proposal.id);
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&final_attempt.proposal) {
                let _ = std::fs::write(dir.join("proposal.json"), bytes);
            }
            if let Ok(bytes) = serde_json::to_vec_pretty(&final_attempt.check_report) {
                let _ = std::fs::write(dir.join("check_report.json"), bytes);
            }
            // Also persist the full attempts trail for audit/replay.
            if let Ok(bytes) = serde_json::to_vec_pretty(&result) {
                let _ = std::fs::write(dir.join("agent_run.json"), bytes);
            }
        }
    }

    Ok(Json(result))
}

async fn get_concept(
    State(state): State<AppState>,
    AxumPath(fqn): AxumPath<String>,
) -> Result<Json<explorer::ConceptView>, ApiError> {
    match explorer::explorer(state.pool.as_ref(), &fqn)
        .await
        .map_err(|e| ApiError::internal("explorer_failed", e))?
    {
        Some(view) => Ok(Json(view)),
        None => Err(ApiError::not_found(
            "concept_not_found",
            format!("no concept named {fqn:?} in registry"),
        )),
    }
}

// ---------- helpers ----------

fn load_proposal(path: &Path) -> Result<OntologyChangeProposal> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&raw)
        .with_context(|| format!("parsing proposal JSON at {}", path.display()))
}

fn load_check_report(dir: &Path) -> Result<Option<CheckReport>> {
    let p = dir.join("check_report.json");
    if !p.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
    let report: CheckReport = serde_json::from_str(&raw)
        .with_context(|| format!("parsing CheckReport at {}", p.display()))?;
    Ok(Some(report))
}

// ---------- error type ----------

/// Uniform `{ "error": code, "details": message }` response with an HTTP
/// status. `Internal` carries the chained anyhow source so logs are useful;
/// the wire body only includes the user-safe `details` string.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    details: String,
    /// F5: optional structured policy-denial trace. When present, the JSON
    /// body adds an `evidence` field carrying the actor / relation / object /
    /// considered-tuples so the UI can render the full denial story.
    policy_evidence: Option<serde_json::Value>,
}

impl ApiError {
    pub fn bad_request(code: &'static str, details: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
            details: details.into(),
            policy_evidence: None,
        }
    }
    pub fn not_found(code: &'static str, details: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code,
            details: details.into(),
            policy_evidence: None,
        }
    }
    pub fn service_unavailable(code: &'static str, details: impl Into<String>) -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code,
            details: details.into(),
            policy_evidence: None,
        }
    }
    pub fn internal(code: &'static str, err: impl std::fmt::Display) -> Self {
        let details = err.to_string();
        tracing::error!(error_code = code, %details, "agorad internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code,
            details,
            policy_evidence: None,
        }
    }

    /// F5: 403 with the full denial trace from the policy evaluator. The
    /// mutation_log seq is included so the client / UI can deep-link to the
    /// audit row.
    pub fn policy_denied(denial: entity_write::PolicyDeniedError) -> Self {
        let evidence = json!({
            "actor":          denial.actor,
            "relation":       denial.relation,
            "object":         denial.object,
            "reason":         denial.reason,
            "considered":     denial.considered,
            "logged_seq":     denial.logged_seq,
            "operation_logged": "DenyAttempt",
        });
        Self {
            status: StatusCode::FORBIDDEN,
            code: "policy_denied",
            details: denial.reason,
            policy_evidence: Some(evidence),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = match self.policy_evidence {
            Some(ev) => json!({
                "error":    self.code,
                "details":  self.details,
                "evidence": ev,
            }),
            None => json!({ "error": self.code, "details": self.details }),
        };
        (self.status, Json(body)).into_response()
    }
}
