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

use crate::mutation_log::{
    self, MutationRecord, ACTOR_CLI, ACTOR_HTTP_HANDLER, OP_CREATE, OP_UPDATE,
};

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

/// Apply a CreateBankIntegration mutation: atomic INSERT + log.
///
/// Used by both the CLI (`agora write bank-integration ...`) and the future
/// HTTP handler. The ontology_version is a parameter, not a constant — the
/// caller (router/registry) decides which version a given write was authored
/// under, mirroring DEMO.md Beat 7's "ontology_version: 2" claim.
pub async fn apply_create_bank_integration(
    pool: &sqlx::PgPool,
    cmd: &CreateBankIntegrationCmd,
    ontology_version: i32,
    origin: WriteOrigin,
) -> Result<WriteOutcome> {
    let data = project_bank_integration(&cmd.entity_id, &cmd.provider);
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

    let rec = mutation_log::log_mutation_in_tx(
        &mut tx,
        TYPE_BANK_INTEGRATION,
        &cmd.entity_id,
        op,
        &data,
        ontology_version,
        origin.actor(),
    )
    .await?;

    tx.commit().await.context("commit bank_integration write")?;
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
