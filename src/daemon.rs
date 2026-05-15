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

use crate::artifacts;
use crate::ast::OntologyChangeProposal;
use crate::check;
use crate::check_report::CheckReport;
use crate::db;
use crate::entity_write::{self, CreateBankIntegrationCmd, WriteOrigin, TYPE_BANK_INTEGRATION};
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

    // Accept either the bare PascalCase type ("BankIntegration") or the
    // fully-qualified name ("core.integrations.BankIntegration"). Anything
    // else is a 400 with the supported set listed.
    let resolved = match type_name.as_str() {
        "BankIntegration" | TYPE_BANK_INTEGRATION => TYPE_BANK_INTEGRATION,
        other => {
            return Err(ApiError::bad_request(
                "unsupported_entity_type",
                format!(
                    "unsupported entity type {other:?}; supported: BankIntegration"
                ),
            ));
        }
    };

    if req.entity_id.trim().is_empty() {
        return Err(ApiError::bad_request("missing_entity_id", "entity_id is required"));
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
        let outcome = entity_write::apply_create_bank_integration(
            pool,
            &cmd,
            req.ontology_version,
            WriteOrigin::HttpHandler,
        )
        .await
        .map_err(|e| ApiError::internal("write_failed", e))?;
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
}

impl ApiError {
    pub fn bad_request(code: &'static str, details: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, code, details: details.into() }
    }
    pub fn not_found(code: &'static str, details: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, code, details: details.into() }
    }
    pub fn service_unavailable(code: &'static str, details: impl Into<String>) -> Self {
        Self { status: StatusCode::SERVICE_UNAVAILABLE, code, details: details.into() }
    }
    pub fn internal(code: &'static str, err: impl std::fmt::Display) -> Self {
        let details = err.to_string();
        tracing::error!(error_code = code, %details, "agorad internal error");
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, code, details }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({ "error": self.code, "details": self.details });
        (self.status, Json(body)).into_response()
    }
}
