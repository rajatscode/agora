//! Controlled entity writes — the path that wires an entity-table insert
//! together with a mutation_log row inside one transaction.
//!
//! This is what an HTTP handler (F-DAEMON) calls; the CLI calls the same
//! function. The shared shape:
//!
//! ```text
//!   begin txn
//!     ↓
//!   INSERT INTO <entity table> ...
//!     ↓
//!   log_mutation_in_tx(...)     ← computes checksum, INSERTs into mutation_log
//!     ↓
//!   commit
//! ```
//!
//! If either insert fails the transaction rolls back, so the invariant
//! "every controlled entity row has a matching mutation_log entry" holds.
//!
//! Today we support the **BankIntegration** entity end-to-end — that's
//! Beat 7's happy-path write. Adding more entity types is a matter of
//! adding another `apply_*` function with the same shape; the daemon
//! workstream will dispatch to them by `entity_type`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

use crate::mutation_log::{
    self, MutationRecord, ACTOR_CLI, ACTOR_HTTP_HANDLER, OP_CREATE, OP_DENY_ATTEMPT, OP_UPDATE,
};
use crate::policy::{self, PolicyDecision, PolicyTuple, RELATION_OWNER};
use crate::seed::ConceptCard;

pub const TYPE_BANK_INTEGRATION: &str = "core.integrations.BankIntegration";
pub const TYPE_AUTHENTICATION_METHOD: &str = "core.integrations.AuthenticationMethod";
pub const TYPE_ACCOUNT: &str = "core.users.Account";
/// F8: second domain entity. The Customer 360 ontology lives under
/// `core.customer.*`; this constant is the FQN of the entity-table-backed
/// concept, used by daemon dispatch + the policy evaluator.
pub const TYPE_CUSTOMER: &str = "core.customer.Customer";
/// F9: third-domain entity (Compliance / GRC). Same dispatch pattern as
/// BankIntegration / Customer; the daemon routes POST /entities/AuditFinding
/// through `apply_create_audit_finding_authzed`.
pub const TYPE_AUDIT_FINDING: &str = "core.compliance.AuditFinding";

/// Where the write came from. The daemon's handler picks `HttpHandler`;
/// the `agora write` CLI picks `Cli`. Both end up in `mutation_log.actor`.
#[derive(Debug, Clone, Copy)]
pub enum WriteOrigin {
    HttpHandler,
    Cli,
}

impl WriteOrigin {
    pub fn actor(self) -> &'static str {
        match self {
            WriteOrigin::HttpHandler => ACTOR_HTTP_HANDLER,
            WriteOrigin::Cli => ACTOR_CLI,
        }
    }
}

/// Inputs the BankIntegration handler accepts. The HTTP layer will deserialize
/// this from JSON; the CLI builds it from --flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBankIntegrationCmd {
    pub entity_id: String,
    pub provider: String,
}

/// F8: Inputs for the Customer 360 entity write. `email` and `display_name`
/// are optional on purpose — the same nullability the seed catalog declares
/// (and the same nullability the risky "tighten email to required" proposal
/// is designed to test against).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCustomerCmd {
    pub entity_id: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub signup_source: Option<String>,
}

/// F9: Inputs for the AuditFinding entity write. `resolved_at` and `notes`
/// are optional — `resolved_at` tracks the open/investigating → resolved
/// lifecycle, and the risky-tighten proposal targets that nullability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAuditFindingCmd {
    pub entity_id: String,
    pub rule_id: String,
    pub severity: String,
    pub status: String,
    /// RFC3339 string when the finding was opened. The handler parses this
    /// once into a chrono::DateTime; tests use a fixed value.
    pub opened_at: String,
    #[serde(default)]
    pub resolved_at: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

/// What we return from a successful write. The HTTP layer renders this as
/// `201 Created` body. The CLI prints the JSON. Both match Beat 7 sub-step
/// 1's promise: client gets back mutation_seq + ontology_version as proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteOutcome {
    pub entity_type: String,
    pub entity_id: String,
    pub operation: String,
    pub ontology_version: i32,
    pub mutation_seq: i64,
    pub checksum: String,
    pub actor: String,
}

impl From<MutationRecord> for WriteOutcome {
    fn from(r: MutationRecord) -> Self {
        Self {
            entity_type: r.type_id,
            entity_id: r.entity_id,
            operation: r.operation,
            ontology_version: r.ontology_version,
            mutation_seq: r.seq,
            checksum: r.checksum,
            actor: r.actor,
        }
    }
}

/// Canonical projection of a `bank_integrations` row to JSON. This is the
/// payload we log AND the payload `verify` reconstructs from the live row.
/// Columns that are server-set and orthogonal to identity / semantics
/// (e.g. `created_at`) are deliberately excluded — they'd otherwise force
/// the checksum to depend on insertion time, defeating drift detection.
pub fn project_bank_integration(entity_id: &str, provider: &str) -> Value {
    json!({
        "id": entity_id,
        "provider": provider,
    })
}

pub fn project_authentication_method(
    entity_id: &str,
    integration_id: &str,
    kind: &str,
) -> Value {
    json!({
        "id": entity_id,
        "integration_id": integration_id,
        "kind": kind,
    })
}

pub fn project_account(entity_id: &str, email: Option<&str>, display_name: Option<&str>) -> Value {
    json!({
        "id": entity_id,
        "email": email,
        "display_name": display_name,
    })
}

/// F8: Canonical projection for the Customer entity. Same rules as
/// `project_bank_integration`: include identity + semantic fields, exclude
/// server-set wallclock columns (`created_at`).
pub fn project_customer(
    entity_id: &str,
    email: Option<&str>,
    display_name: Option<&str>,
    signup_source: Option<&str>,
) -> Value {
    json!({
        "id":            entity_id,
        "email":         email,
        "display_name":  display_name,
        "signup_source": signup_source,
    })
}

/// F9: Canonical projection for the AuditFinding entity. Includes the
/// status-tracking timestamps so a change to `resolved_at` shows up in
/// the canonical-JSON checksum (and therefore in F3 verify drift output).
/// Excludes `created_at` for the same reason as the other projections.
///
/// Timestamps are serialised as RFC3339 strings rather than chrono values
/// so the projection is identical to what the seed migration would
/// produce on read-back, and so the canonical_json serialiser doesn't have
/// to special-case datetimes.
pub fn project_audit_finding(
    entity_id: &str,
    rule_id: &str,
    severity: &str,
    status: &str,
    opened_at: &str,
    resolved_at: Option<&str>,
    notes: Option<&str>,
) -> Value {
    json!({
        "id":          entity_id,
        "rule_id":     rule_id,
        "severity":    severity,
        "status":      status,
        "opened_at":   opened_at,
        "resolved_at": resolved_at,
        "notes":       notes,
    })
}

/// Authoring intent + F5 outcome carriers.

/// F5: Policy denial. `entity_write` returns this as a typed error so the
/// HTTP layer can map it to 403 (with the structured trace) without losing
/// the per-tuple detail to a `format!()` string.
#[derive(Debug, Error)]
#[error("policy_denied: {reason}")]
pub struct PolicyDeniedError {
    pub actor: String,
    pub relation: String,
    pub object: String,
    pub reason: String,
    pub considered: Vec<PolicyTuple>,
    /// The mutation_log row recording the denial attempt. Present iff the
    /// denial was successfully logged (the audit trail is independent from
    /// whether the entity row landed).
    pub logged_seq: Option<i64>,
}

/// Apply a CreateBankIntegration mutation: atomic INSERT + log.
///
/// Used by both the CLI (`agora write bank-integration ...`) and the future
/// HTTP handler. The ontology_version is a parameter, not a constant — the
/// caller (router/registry) decides which version a given write was authored
/// under, mirroring DEMO.md Beat 7's "ontology_version: 2" claim.
///
/// F5: backward-compatible alias of `apply_create_bank_integration_authzed`
/// that wires the historical actor (HTTP handler / CLI) up to the policy
/// evaluator as `team:integrations-platform`. New code should call the
/// authzed variant directly.
pub async fn apply_create_bank_integration(
    pool: &sqlx::PgPool,
    cmd: &CreateBankIntegrationCmd,
    ontology_version: i32,
    origin: WriteOrigin,
) -> Result<WriteOutcome> {
    apply_create_bank_integration_authzed(
        pool,
        cmd,
        ontology_version,
        origin,
        "team:integrations-platform",
        None,
    )
    .await
    .map_err(|e| match e {
        WriteError::Other(err) => err,
        // The legacy entry point can't surface PolicyDenied — it always
        // injects the owner team. Anyone hitting this branch is a bug.
        WriteError::PolicyDenied(d) => anyhow::anyhow!(
            "legacy entry point should never see PolicyDenied: {}",
            d.reason
        ),
    })
}

/// F5 error wrapper for the authzed write path. Lets the daemon
/// distinguish "real failure" from "policy denied — return 403".
#[derive(Debug)]
pub enum WriteError {
    PolicyDenied(PolicyDeniedError),
    Other(anyhow::Error),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::PolicyDenied(d) => write!(f, "{d}"),
            WriteError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<anyhow::Error> for WriteError {
    fn from(e: anyhow::Error) -> Self {
        WriteError::Other(e)
    }
}

/// F5 entrypoint — same write, plus policy evaluation against `actor`
/// before the INSERT lands. `policy_card` is the ConceptCard for the
/// target type; the policy spec is built from it via
/// `policy::spec_for_concept`. Pass `None` to skip the policy check (used
/// by tests that don't want to exercise authorization).
pub async fn apply_create_bank_integration_authzed(
    pool: &sqlx::PgPool,
    cmd: &CreateBankIntegrationCmd,
    ontology_version: i32,
    origin: WriteOrigin,
    actor: &str,
    policy_card: Option<&ConceptCard>,
) -> std::result::Result<WriteOutcome, WriteError> {
    let data = project_bank_integration(&cmd.entity_id, &cmd.provider);

    // -------- F5: policy check --------
    if let Some(card) = policy_card {
        let spec = policy::spec_for_concept(card);
        let object = policy::object_id("bank_integration", &cmd.entity_id);
        let decision = policy::evaluate(&spec, actor, RELATION_OWNER, &object);
        if let PolicyDecision::Deny { reason, considered } = decision {
            // Log the denial attempt — auditable, separate from the entity
            // table. The mutation_log row records the *attempted* payload so
            // a reviewer can see what they tried to write.
            let logged_seq =
                log_deny_attempt(pool, &cmd.entity_id, &data, ontology_version, actor, &reason)
                    .await
                    .map_err(WriteError::Other)?;
            return Err(WriteError::PolicyDenied(PolicyDeniedError {
                actor: actor.to_string(),
                relation: RELATION_OWNER.to_string(),
                object,
                reason,
                considered,
                logged_seq: Some(logged_seq),
            }));
        }
    }
    // -------- /F5 --------

    let mut tx = pool.begin().await.context("begin bank_integration write")?;

    sqlx::query(
        "INSERT INTO bank_integrations (id, provider) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET provider = EXCLUDED.provider",
    )
    .bind(&cmd.entity_id)
    .bind(&cmd.provider)
    .execute(&mut *tx)
    .await
    .context("inserting bank_integration row")?;

    // Distinguish CREATE vs UPDATE based on whether the row existed before.
    // For Beat 7's "happy path then tampering" demo we only need CREATE; we
    // still pick UPDATE if the upsert collided so version history is honest.
    let op = if sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mutation_log WHERE type_id = $1 AND entity_id = $2",
    )
    .bind(TYPE_BANK_INTEGRATION)
    .bind(&cmd.entity_id)
    .fetch_one(&mut *tx)
    .await
    .context("counting existing mutation_log rows")?
        > 0
    {
        OP_UPDATE
    } else {
        OP_CREATE
    };

    // F5: prefer the caller-supplied actor (team:foo) over the generic
    // origin actor when an actor was passed. This keeps the audit trail
    // honest about *who* did the allowed write.
    let logged_actor = if actor.is_empty() { origin.actor() } else { actor };
    let rec = mutation_log::log_mutation_in_tx(
        &mut tx,
        TYPE_BANK_INTEGRATION,
        &cmd.entity_id,
        op,
        &data,
        ontology_version,
        logged_actor,
    )
    .await
    .map_err(WriteError::Other)?;

    tx.commit().await.context("commit bank_integration write")?;
    Ok(WriteOutcome::from(rec))
}

/// Insert a DenyAttempt mutation_log row in its own short transaction. The
/// entity table is NOT touched. Returns the seq number for the trace.
///
/// F8: generalized — accepts the target `type_id` so the same helper logs
/// denials for any concept (BankIntegration, Customer, etc.). The legacy
/// helper below preserves the old call-site signature.
async fn log_deny_attempt_for(
    pool: &sqlx::PgPool,
    type_id: &str,
    entity_id: &str,
    attempted_payload: &Value,
    ontology_version: i32,
    actor: &str,
    reason: &str,
) -> Result<i64> {
    let mut tx = pool.begin().await.context("begin deny-attempt log")?;
    let rec = mutation_log::log_mutation_with_denial_in_tx(
        &mut tx,
        type_id,
        entity_id,
        OP_DENY_ATTEMPT,
        attempted_payload,
        ontology_version,
        actor,
        Some(reason),
    )
    .await?;
    tx.commit().await.context("commit deny-attempt log")?;
    Ok(rec.seq)
}

async fn log_deny_attempt(
    pool: &sqlx::PgPool,
    entity_id: &str,
    attempted_payload: &Value,
    ontology_version: i32,
    actor: &str,
    reason: &str,
) -> Result<i64> {
    log_deny_attempt_for(
        pool,
        TYPE_BANK_INTEGRATION,
        entity_id,
        attempted_payload,
        ontology_version,
        actor,
        reason,
    )
    .await
}

// ============================================================================
// F8 — second domain: Customer 360 entity writes.
//
// `apply_create_customer_authzed` is the analog of
// `apply_create_bank_integration_authzed`: same atomic INSERT + log shape,
// same F5 policy check, different table + projection. The presence of two
// uniform variants is the whole proof of generalization — daemon dispatch
// reads `type_name`, picks the right `apply_*`, and the rest is identical.
// ============================================================================

pub async fn apply_create_customer_authzed(
    pool: &sqlx::PgPool,
    cmd: &CreateCustomerCmd,
    ontology_version: i32,
    origin: WriteOrigin,
    actor: &str,
    policy_card: Option<&ConceptCard>,
) -> std::result::Result<WriteOutcome, WriteError> {
    let data = project_customer(
        &cmd.entity_id,
        cmd.email.as_deref(),
        cmd.display_name.as_deref(),
        cmd.signup_source.as_deref(),
    );

    // -------- F5 policy check (uniform across concepts) --------
    if let Some(card) = policy_card {
        let spec = policy::spec_for_concept(card);
        let object = policy::object_id("customer", &cmd.entity_id);
        let decision = policy::evaluate(&spec, actor, RELATION_OWNER, &object);
        if let PolicyDecision::Deny { reason, considered } = decision {
            let logged_seq = log_deny_attempt_for(
                pool,
                TYPE_CUSTOMER,
                &cmd.entity_id,
                &data,
                ontology_version,
                actor,
                &reason,
            )
            .await
            .map_err(WriteError::Other)?;
            return Err(WriteError::PolicyDenied(PolicyDeniedError {
                actor: actor.to_string(),
                relation: RELATION_OWNER.to_string(),
                object,
                reason,
                considered,
                logged_seq: Some(logged_seq),
            }));
        }
    }

    let mut tx = pool.begin().await.context("begin customer write")?;

    sqlx::query(
        "INSERT INTO customers (id, email, display_name, signup_source)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (id) DO UPDATE SET
            email = EXCLUDED.email,
            display_name = EXCLUDED.display_name,
            signup_source = EXCLUDED.signup_source",
    )
    .bind(&cmd.entity_id)
    .bind(&cmd.email)
    .bind(&cmd.display_name)
    .bind(&cmd.signup_source)
    .execute(&mut *tx)
    .await
    .context("inserting customer row")?;

    let op = if sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mutation_log WHERE type_id = $1 AND entity_id = $2",
    )
    .bind(TYPE_CUSTOMER)
    .bind(&cmd.entity_id)
    .fetch_one(&mut *tx)
    .await
    .context("counting existing customer mutation_log rows")?
        > 0
    {
        OP_UPDATE
    } else {
        OP_CREATE
    };

    let logged_actor = if actor.is_empty() { origin.actor() } else { actor };
    let rec = mutation_log::log_mutation_in_tx(
        &mut tx,
        TYPE_CUSTOMER,
        &cmd.entity_id,
        op,
        &data,
        ontology_version,
        logged_actor,
    )
    .await
    .map_err(WriteError::Other)?;

    tx.commit().await.context("commit customer write")?;
    Ok(WriteOutcome::from(rec))
}

// ============================================================================
// F9 — third domain: Compliance / GRC entity writes.
//
// Structurally identical to apply_create_customer_authzed: same atomic
// INSERT + log shape, same F5 policy check (different owner team), same
// WriteError::PolicyDenied carrying full trace. The presence of three
// uniform variants is the load-bearing proof of N-domain generalization
// — daemon dispatch reads `type_name`, picks the apply_*, the rest is
// identical control plane.
// ============================================================================

pub async fn apply_create_audit_finding_authzed(
    pool: &sqlx::PgPool,
    cmd: &CreateAuditFindingCmd,
    ontology_version: i32,
    origin: WriteOrigin,
    actor: &str,
    policy_card: Option<&ConceptCard>,
) -> std::result::Result<WriteOutcome, WriteError> {
    // Normalise the caller's RFC3339 timestamps through chrono so the
    // projection logged here is byte-identical to what `verify()` will
    // produce when it reads the TIMESTAMPTZ back and formats with
    // `.to_rfc3339()`. Otherwise the caller's "...Z" suffix vs chrono's
    // "...+00:00" would diverge → spurious drift.
    let opened_dt: chrono::DateTime<chrono::Utc> = cmd
        .opened_at
        .parse::<chrono::DateTime<chrono::FixedOffset>>()
        .map(|d| d.with_timezone(&chrono::Utc))
        .or_else(|_| cmd.opened_at.parse::<chrono::DateTime<chrono::Utc>>())
        .map_err(|e| WriteError::Other(anyhow::anyhow!("invalid opened_at: {e}")))?;
    let resolved_dt: Option<chrono::DateTime<chrono::Utc>> = match cmd.resolved_at.as_deref() {
        None => None,
        Some(s) => Some(
            s.parse::<chrono::DateTime<chrono::FixedOffset>>()
                .map(|d| d.with_timezone(&chrono::Utc))
                .or_else(|_| s.parse::<chrono::DateTime<chrono::Utc>>())
                .map_err(|e| WriteError::Other(anyhow::anyhow!("invalid resolved_at: {e}")))?,
        ),
    };
    let opened_canonical = opened_dt.to_rfc3339();
    let resolved_canonical = resolved_dt.map(|d| d.to_rfc3339());

    let data = project_audit_finding(
        &cmd.entity_id,
        &cmd.rule_id,
        &cmd.severity,
        &cmd.status,
        &opened_canonical,
        resolved_canonical.as_deref(),
        cmd.notes.as_deref(),
    );

    // -------- F5 policy check (uniform across concepts) --------
    if let Some(card) = policy_card {
        let spec = policy::spec_for_concept(card);
        let object = policy::object_id("audit_finding", &cmd.entity_id);
        let decision = policy::evaluate(&spec, actor, RELATION_OWNER, &object);
        if let PolicyDecision::Deny { reason, considered } = decision {
            let logged_seq = log_deny_attempt_for(
                pool,
                TYPE_AUDIT_FINDING,
                &cmd.entity_id,
                &data,
                ontology_version,
                actor,
                &reason,
            )
            .await
            .map_err(WriteError::Other)?;
            return Err(WriteError::PolicyDenied(PolicyDeniedError {
                actor: actor.to_string(),
                relation: RELATION_OWNER.to_string(),
                object,
                reason,
                considered,
                logged_seq: Some(logged_seq),
            }));
        }
    }

    let mut tx = pool.begin().await.context("begin audit_finding write")?;

    // Bind the parsed DateTime values directly — sqlx maps
    // chrono::DateTime<Utc> ↔ TIMESTAMPTZ natively. Going through
    // canonical chrono on both write and verify is what makes the
    // checksum stable across the round-trip.
    sqlx::query(
        "INSERT INTO audit_findings
            (id, rule_id, severity, status, opened_at, resolved_at, notes)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (id) DO UPDATE SET
            rule_id     = EXCLUDED.rule_id,
            severity    = EXCLUDED.severity,
            status      = EXCLUDED.status,
            opened_at   = EXCLUDED.opened_at,
            resolved_at = EXCLUDED.resolved_at,
            notes       = EXCLUDED.notes",
    )
    .bind(&cmd.entity_id)
    .bind(&cmd.rule_id)
    .bind(&cmd.severity)
    .bind(&cmd.status)
    .bind(opened_dt)
    .bind(resolved_dt)
    .bind(&cmd.notes)
    .execute(&mut *tx)
    .await
    .context("inserting audit_finding row")?;

    let op = if sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mutation_log WHERE type_id = $1 AND entity_id = $2",
    )
    .bind(TYPE_AUDIT_FINDING)
    .bind(&cmd.entity_id)
    .fetch_one(&mut *tx)
    .await
    .context("counting existing audit_finding mutation_log rows")?
        > 0
    {
        OP_UPDATE
    } else {
        OP_CREATE
    };

    let logged_actor = if actor.is_empty() { origin.actor() } else { actor };
    let rec = mutation_log::log_mutation_in_tx(
        &mut tx,
        TYPE_AUDIT_FINDING,
        &cmd.entity_id,
        op,
        &data,
        ontology_version,
        logged_actor,
    )
    .await
    .map_err(WriteError::Other)?;

    tx.commit().await.context("commit audit_finding write")?;
    Ok(WriteOutcome::from(rec))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_is_stable() {
        let p1 = project_bank_integration("bi_acme", "plaid");
        let p2 = project_bank_integration("bi_acme", "plaid");
        assert_eq!(p1, p2);
        assert_eq!(p1["id"], "bi_acme");
        assert_eq!(p1["provider"], "plaid");
        // Should NOT include created_at — that would couple the checksum to
        // wallclock time and break verify.
        assert!(p1.get("created_at").is_none());
    }
}
