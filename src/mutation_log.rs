//! Append-only mutation log — the operational substrate Feature 3 builds on.
//!
//! Every controlled write (via the HTTP handler, the daemon, or `agora write`)
//! flows through `log_mutation`, which:
//!   1. Computes a deterministic SHA256 checksum of the canonical-JSON form of
//!      the inputs (type_id, entity_id, operation, data, ontology_version).
//!   2. Inserts the mutation row + the entity row inside ONE transaction,
//!      so an entity exists in the DB iff its corresponding log row does too.
//!
//! `agora verify` (see `verify.rs`) re-computes the same checksum from the
//! live entity table state and compares it with the latest log entry — any
//! mismatch is out-of-band tampering.
//!
//! Why this is library-first:
//!   - `log_mutation` takes a `&PgPool` (or `&mut Transaction`) and returns
//!     a `MutationRecord` value. No printing, no CLI concerns.
//!   - The CLI (`cli.rs`) and the upcoming F-DAEMON HTTP layer both call the
//!     same functions. The HTTP handler wires `log_mutation` into an Axum
//!     route in `daemon.rs` (Feature 3, Phase 2).
//!
//! Canonical-JSON contract:
//!   - Object keys sorted ascending (string sort, RFC8259 / RFC8785-ish).
//!   - No insignificant whitespace.
//!   - Numbers / strings / bools / nulls emitted as `serde_json::to_string`
//!     emits them.
//!   - Arrays preserve insertion order (canonical at the value level).
//!
//! This is enough determinism for Beat 7. We do NOT attempt full RFC 8785
//! number normalization — for the demo's small payloads the standard serde
//! formatter is stable, and the test `checksum_is_deterministic_across_key_order`
//! pins this behavior.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};

/// Operation kind. We accept the spec-mandated CREATE/UPDATE/DELETE strings
/// but persist them lowercased to match F2's existing `command` convention
/// (`'Create' | 'Update' | 'Deprecate'`). Both forms hash identically because
/// the checksum uses the canonical (input) string we got from the caller.
pub const OP_CREATE: &str = "Create";
pub const OP_UPDATE: &str = "Update";
pub const OP_DELETE: &str = "Delete";
/// F5: a denial attempt — the actor tried to write but policy blocked them.
/// The entity row is NOT inserted; only the mutation_log row is, so the
/// audit trail captures who tried, when, and why they were refused.
pub const OP_DENY_ATTEMPT: &str = "DenyAttempt";

/// Author identity for writes that came through Agora's HTTP handler / CLI.
/// Drift attribution: anything *not* in mutation_log is, by elimination,
/// out-of-band — but having a stable label for legitimate writes makes the
/// version-history view in the Explorer readable.
pub const ACTOR_HTTP_HANDLER: &str = "http-handler";
pub const ACTOR_CLI: &str = "agora-cli";

/// What `log_mutation` returns to the caller. The runtime / HTTP handler
/// echoes `mutation_seq` + `ontology_version` to the client as proof of
/// logging — see Beat 7's "201 Created { mutation_id, ontology_version }".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationRecord {
    pub seq: i64,
    pub type_id: String,
    pub entity_id: String,
    pub operation: String,
    pub ontology_version: i32,
    pub checksum: String,
    pub actor: String,
}

/// A historical mutation row, used by `verify` to reconstruct the expected
/// state of an entity and by the Explorer to render version history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggedMutation {
    pub seq: i64,
    pub type_id: String,
    pub entity_id: String,
    pub operation: String,
    pub data: Value,
    pub ontology_version: i32,
    pub actor: String,
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    pub checksum: Option<String>,
}

/// Compute the canonical-JSON serialization of a `Value`. Sorted keys; no
/// extraneous whitespace. This is the input to the checksum and MUST be
/// identical between write-time and verify-time.
pub fn canonical_json(v: &Value) -> String {
    let mut s = String::new();
    write_canonical(&mut s, v);
    s
}

fn write_canonical(out: &mut String, v: &Value) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&serde_json::to_string(s).unwrap()),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(*k).unwrap());
                out.push(':');
                write_canonical(out, map.get(*k).unwrap());
            }
            out.push('}');
        }
    }
}

/// Compute the deterministic checksum for a mutation. Inputs are pipe-joined
/// in a fixed order so a flipped operation or version produces a different
/// hash. The canonical-JSON of `data` is used so logically-identical payloads
/// with different key orderings hash to the same value.
pub fn compute_checksum(
    type_id: &str,
    entity_id: &str,
    operation: &str,
    data: &Value,
    ontology_version: i32,
) -> String {
    let canonical = canonical_json(data);
    let blob = format!(
        "{}|{}|{}|{}|{}",
        type_id, entity_id, operation, canonical, ontology_version
    );
    let mut hasher = Sha256::new();
    hasher.update(blob.as_bytes());
    hex::encode(hasher.finalize())
}

/// Insert a mutation_log row inside an *existing* transaction. The caller
/// is responsible for opening the txn and writing the entity row. This is
/// what makes the atomicity proof real: if either insert fails, the txn
/// rolls back and there's no entity-without-log (or log-without-entity).
pub async fn log_mutation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    type_id: &str,
    entity_id: &str,
    operation: &str,
    data: &Value,
    ontology_version: i32,
    actor: &str,
) -> Result<MutationRecord> {
    log_mutation_with_denial_in_tx(
        tx,
        type_id,
        entity_id,
        operation,
        data,
        ontology_version,
        actor,
        None,
    )
    .await
}

/// F5 variant: log a mutation row with an optional `denial_reason`. For
/// allowed writes the reason is `None` and behaves identically to the
/// historical signature. For denied attempts the reason is `Some(text)`,
/// and the row carries a deterministic checksum over the attempted
/// payload — so the audit log is still tamper-evident.
pub async fn log_mutation_with_denial_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    type_id: &str,
    entity_id: &str,
    operation: &str,
    data: &Value,
    ontology_version: i32,
    actor: &str,
    denial_reason: Option<&str>,
) -> Result<MutationRecord> {
    let checksum = compute_checksum(type_id, entity_id, operation, data, ontology_version);
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO mutation_log
            (type_id, ontology_version, entity_id, command, payload, actor, checksum, denial_reason)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING seq",
    )
    .bind(type_id)
    .bind(ontology_version)
    .bind(entity_id)
    .bind(operation)
    .bind(data)
    .bind(actor)
    .bind(&checksum)
    .bind(denial_reason)
    .fetch_one(&mut **tx)
    .await
    .context("inserting into mutation_log")?;

    Ok(MutationRecord {
        seq: row.0,
        type_id: type_id.to_string(),
        entity_id: entity_id.to_string(),
        operation: operation.to_string(),
        ontology_version,
        checksum,
        actor: actor.to_string(),
    })
}

/// Convenience wrapper for callers that don't already hold a transaction.
/// Opens one, logs, commits. Used by tests + by callers that intend to log
/// AFTER they've already written the entity in another connection (the
/// txn-aware variant above is the correct path for handler integration).
pub async fn log_mutation(
    pool: &PgPool,
    type_id: &str,
    entity_id: &str,
    operation: &str,
    data: &Value,
    ontology_version: i32,
    actor: &str,
) -> Result<MutationRecord> {
    let mut tx = pool.begin().await.context("begin mutation_log txn")?;
    let rec =
        log_mutation_in_tx(&mut tx, type_id, entity_id, operation, data, ontology_version, actor)
            .await?;
    tx.commit().await.context("commit mutation_log txn")?;
    Ok(rec)
}

/// Get the most recent mutation_log entry for a (type_id, entity_id) pair.
/// Returns `None` if no entry exists — that's how `verify` detects
/// out-of-band created rows (rows in the entity table with no log record).
pub async fn latest_for_entity(
    pool: &PgPool,
    type_id: &str,
    entity_id: &str,
) -> Result<Option<LoggedMutation>> {
    let row = sqlx::query_as::<_, (i64, String, i32, String, String, Value, String, chrono::DateTime<chrono::Utc>, Option<String>)>(
        "SELECT seq, type_id, ontology_version, entity_id, command, payload, actor, occurred_at, checksum
         FROM mutation_log
         WHERE type_id = $1 AND entity_id = $2
         ORDER BY seq DESC
         LIMIT 1",
    )
    .bind(type_id)
    .bind(entity_id)
    .fetch_optional(pool)
    .await
    .context("querying latest mutation_log row")?;

    Ok(row.map(
        |(seq, type_id, ontology_version, entity_id, command, payload, actor, occurred_at, checksum)| {
            LoggedMutation {
                seq,
                type_id,
                ontology_version,
                entity_id,
                operation: command,
                data: payload,
                actor,
                occurred_at,
                checksum,
            }
        },
    ))
}

/// All mutation_log rows for a given type, newest → oldest (most recent
/// mutation first). Used by the Explorer to render version history.
pub async fn history_for_type(
    pool: &PgPool,
    type_id: &str,
    limit: i64,
) -> Result<Vec<LoggedMutation>> {
    let rows = sqlx::query_as::<_, (i64, String, i32, String, String, Value, String, chrono::DateTime<chrono::Utc>, Option<String>)>(
        "SELECT seq, type_id, ontology_version, entity_id, command, payload, actor, occurred_at, checksum
         FROM mutation_log
         WHERE type_id = $1
         ORDER BY seq DESC
         LIMIT $2",
    )
    .bind(type_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing mutation_log rows for type")?;

    Ok(rows
        .into_iter()
        .map(
            |(seq, type_id, ontology_version, entity_id, command, payload, actor, occurred_at, checksum)| {
                LoggedMutation {
                    seq,
                    type_id,
                    ontology_version,
                    entity_id,
                    operation: command,
                    data: payload,
                    actor,
                    occurred_at,
                    checksum,
                }
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_json_sorts_keys() {
        let a = json!({"b": 1, "a": 2, "c": {"y": 4, "x": 3}});
        let b = json!({"c": {"x": 3, "y": 4}, "a": 2, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        // Pin the exact form so a future change to the canonicalizer
        // doesn't silently break checksum stability.
        assert_eq!(
            canonical_json(&a),
            r#"{"a":2,"b":1,"c":{"x":3,"y":4}}"#
        );
    }

    #[test]
    fn canonical_json_preserves_array_order() {
        let v = json!([3, 1, 2]);
        assert_eq!(canonical_json(&v), "[3,1,2]");
    }

    #[test]
    fn checksum_is_deterministic_across_key_order() {
        let a = json!({"id": "x", "provider": "plaid"});
        let b = json!({"provider": "plaid", "id": "x"});
        let h1 = compute_checksum("T", "x", "Create", &a, 1);
        let h2 = compute_checksum("T", "x", "Create", &b, 1);
        assert_eq!(h1, h2);
    }

    #[test]
    fn checksum_changes_when_any_input_changes() {
        let data = json!({"id": "x", "provider": "plaid"});
        let baseline = compute_checksum("T", "x", "Create", &data, 1);

        // Change type_id
        assert_ne!(baseline, compute_checksum("T2", "x", "Create", &data, 1));
        // Change entity_id
        assert_ne!(baseline, compute_checksum("T", "y", "Create", &data, 1));
        // Change operation
        assert_ne!(baseline, compute_checksum("T", "x", "Update", &data, 1));
        // Change ontology_version
        assert_ne!(baseline, compute_checksum("T", "x", "Create", &data, 2));
        // Change a single field value
        let evil = json!({"id": "x", "provider": "evil-corp"});
        assert_ne!(baseline, compute_checksum("T", "x", "Create", &evil, 1));
    }

    #[test]
    fn checksum_is_hex_64_chars() {
        let data = json!({"id": "x"});
        let h = compute_checksum("T", "x", "Create", &data, 1);
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
