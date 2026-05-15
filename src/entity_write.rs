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
async fn log_deny_attempt(
    pool: &sqlx::PgPool,
    entity_id: &str,
    attempted_payload: &Value,
    ontology_version: i32,
    actor: &str,
    reason: &str,
) -> Result<i64> {
    let mut tx = pool.begin().await.context("begin deny-attempt log")?;
    let rec = mutation_log::log_mutation_with_denial_in_tx(
        &mut tx,
        TYPE_BANK_INTEGRATION,
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
