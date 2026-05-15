//! `agora verify` — drift detection between the live entity tables and the
//! append-only mutation_log.
//!
//! For each known entity table we read every row, project it to its canonical
//! JSON form, look up the most recent mutation_log entry for that
//! (type_id, entity_id) pair, and compare checksums:
//!
//!   - **clean**: the canonical-JSON projection of the live row reproduces the
//!     same checksum that was logged at write time. No drift.
//!   - **drift**: there *is* a log entry but the live checksum doesn't match
//!     it. The row was modified out-of-band (raw SQL UPDATE, manual fix,
//!     migration that bypassed the handler). This is Beat 7's proof.
//!   - **created_out_of_band**: a row exists in the entity table but there's
//!     no mutation_log entry at all — it was INSERTed without going through
//!     the control plane.
//!
//! Library-first contract: `verify` returns a `VerifyReport` value. The CLI
//! command in `cli.rs` serializes it to stdout as JSON. The future daemon
//! HTTP layer can mount the same function under `GET /verify`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

use crate::entity_write::{
    project_account, project_authentication_method, project_bank_integration, project_customer,
    TYPE_ACCOUNT, TYPE_AUTHENTICATION_METHOD, TYPE_BANK_INTEGRATION, TYPE_CUSTOMER,
};
use crate::mutation_log::{self, compute_checksum, LoggedMutation};

/// Top-level verify outcome. `verify_status` collapses everything into a
/// single boolean-ish for clients that only care "is anything wrong?".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub verify_status: VerifyStatus,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub entities_checked: u64,
    pub tampered_entities: Vec<DriftFinding>,
    pub outofband_entities: Vec<OutOfBandFinding>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerifyStatus {
    Clean,
    Tampered,
}

/// A row whose live state doesn't match its most recent mutation_log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftFinding {
    pub entity_type: String,
    pub entity_id: String,
    pub issue: String,
    pub fields_changed: Vec<String>,
    pub detected_via: String,
    pub last_logged_mutation_seq: i64,
    pub last_logged_at: chrono::DateTime<chrono::Utc>,
    pub last_logged_actor: String,
    pub logged_checksum: Option<String>,
    pub current_checksum: String,
    pub current_state: Value,
    pub logged_state: Value,
}

/// A row that exists in the entity table but never went through the control
/// plane. The mutation_log has zero entries for this (type_id, entity_id).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutOfBandFinding {
    pub entity_type: String,
    pub entity_id: String,
    pub issue: String,
    pub current_state: Value,
    pub current_checksum: String,
}

/// Snapshot of a live entity row, normalized to the canonical-JSON shape
/// `entity_write` would have logged.
struct LiveRow {
    entity_type: &'static str,
    entity_id: String,
    data: Value,
}

/// Verify the entire operational substrate. Returns a report; never panics
/// on a tampered row. Tracing goes to stderr; no `println!` in this fn.
pub async fn verify(pool: &PgPool) -> Result<VerifyReport> {
    let started = std::time::Instant::now();
    let rows = collect_live_rows(pool).await?;
    let entities_checked = rows.len() as u64;

    let mut tampered_entities = Vec::new();
    let mut outofband_entities = Vec::new();

    // Pull every latest-per-entity log row in one pass to avoid N round-trips.
    let latest = latest_per_entity(pool).await?;

    for row in rows {
        // Use the canonical JSON of the live row to derive the version we
        // would compare against. We need ontology_version + operation from
        // the log row to reconstruct the comparable checksum — the
        // checksum's input includes both. If no log row exists, this is
        // out-of-band.
        let key = (row.entity_type.to_string(), row.entity_id.clone());
        match latest.get(&key) {
            None => {
                let current_checksum = compute_checksum(
                    row.entity_type,
                    &row.entity_id,
                    "Unknown",
                    &row.data,
                    0,
                );
                tracing::warn!(
                    entity_type = row.entity_type,
                    entity_id = %row.entity_id,
                    "verify: row has no mutation_log entry — created_out_of_band"
                );
                outofband_entities.push(OutOfBandFinding {
                    entity_type: row.entity_type.to_string(),
                    entity_id: row.entity_id,
                    issue: "created_out_of_band".to_string(),
                    current_state: row.data,
                    current_checksum,
                });
            }
            Some(log) => {
                let current_checksum = compute_checksum(
                    &log.type_id,
                    &log.entity_id,
                    &log.operation,
                    &row.data,
                    log.ontology_version,
                );
                if Some(&current_checksum) != log.checksum.as_ref() {
                    let fields_changed = diff_top_level_fields(&log.data, &row.data);
                    tracing::warn!(
                        entity_type = row.entity_type,
                        entity_id = %row.entity_id,
                        last_seq = log.seq,
                        "verify: checksum mismatch — drift detected"
                    );
                    tampered_entities.push(DriftFinding {
                        entity_type: row.entity_type.to_string(),
                        entity_id: row.entity_id,
                        issue: "drift".to_string(),
                        fields_changed,
                        detected_via: "checksum mismatch".to_string(),
                        last_logged_mutation_seq: log.seq,
                        last_logged_at: log.occurred_at,
                        last_logged_actor: log.actor.clone(),
                        logged_checksum: log.checksum.clone(),
                        current_checksum,
                        current_state: row.data,
                        logged_state: log.data.clone(),
                    });
                }
            }
        }
    }

    let verify_status = if tampered_entities.is_empty() && outofband_entities.is_empty() {
        VerifyStatus::Clean
    } else {
        VerifyStatus::Tampered
    };

    Ok(VerifyReport {
        verify_status,
        timestamp: chrono::Utc::now(),
        entities_checked,
        tampered_entities,
        outofband_entities,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// Collect every row from every known entity table, projected to canonical
/// JSON. Adding a new entity type is two lines: another `fetch_*` call.
async fn collect_live_rows(pool: &PgPool) -> Result<Vec<LiveRow>> {
    let mut out = Vec::new();

    let bi: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, provider FROM bank_integrations ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("reading bank_integrations")?;
    for (id, provider) in bi {
        let data = project_bank_integration(&id, &provider);
        out.push(LiveRow {
            entity_type: TYPE_BANK_INTEGRATION,
            entity_id: id,
            data,
        });
    }

    let am: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT id, integration_id, kind FROM authentication_methods ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("reading authentication_methods")?;
    for (id, integration_id, kind) in am {
        let data = project_authentication_method(&id, &integration_id, &kind);
        out.push(LiveRow {
            entity_type: TYPE_AUTHENTICATION_METHOD,
            entity_id: id,
            data,
        });
    }

    let acc: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, email, display_name FROM accounts ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("reading accounts")?;
    for (id, email, display_name) in acc {
        let data = project_account(&id, email.as_deref(), display_name.as_deref());
        out.push(LiveRow {
            entity_type: TYPE_ACCOUNT,
            entity_id: id,
            data,
        });
    }

    // F8: Customer 360 — iterate the `customers` table the same way the
    // other entity tables are iterated. Rows that exist here without a
    // matching mutation_log entry surface as out-of-band; rows whose
    // canonical-JSON checksum no longer matches the logged checksum
    // surface as drift. Same code path as bank_integrations / accounts;
    // no domain-specific branching in `verify()` itself.
    let cust: Vec<(String, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT id, email, display_name, signup_source FROM customers ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .context("reading customers")?;
    for (id, email, display_name, signup_source) in cust {
        let data = project_customer(
            &id,
            email.as_deref(),
            display_name.as_deref(),
            signup_source.as_deref(),
        );
        out.push(LiveRow {
            entity_type: TYPE_CUSTOMER,
            entity_id: id,
            data,
        });
    }

    Ok(out)
}

/// Single SQL query, returning the latest log row for each (type_id, entity_id)
/// pair. Avoids the N+1 round-trip pattern when verifying many entities.
async fn latest_per_entity(
    pool: &PgPool,
) -> Result<HashMap<(String, String), LoggedMutation>> {
    // DISTINCT ON gives us the head of each (type_id, entity_id) group
    // ordered by seq DESC. We restrict to known entity types so we don't
    // pull historical garbage rows that no longer correspond to any table.
    let rows = sqlx::query_as::<_, (i64, String, i32, String, String, Value, String, chrono::DateTime<chrono::Utc>, Option<String>)>(
        "SELECT DISTINCT ON (type_id, entity_id)
            seq, type_id, ontology_version, entity_id, command, payload, actor, occurred_at, checksum
         FROM mutation_log
         WHERE type_id = ANY($1)
         ORDER BY type_id, entity_id, seq DESC",
    )
    .bind(&[
        TYPE_BANK_INTEGRATION.to_string(),
        TYPE_AUTHENTICATION_METHOD.to_string(),
        TYPE_ACCOUNT.to_string(),
        // F8: include Customer 360 entries so writes through the handler
        // (operation=Create/Update) match against live `customers` rows;
        // without this, every handler-written Customer would look
        // out-of-band because we'd never find its log row.
        TYPE_CUSTOMER.to_string(),
    ][..])
    .fetch_all(pool)
    .await
    .context("loading latest mutation_log per entity")?;

    let mut map = HashMap::new();
    for (seq, type_id, ontology_version, entity_id, command, payload, actor, occurred_at, checksum) in rows {
        map.insert(
            (type_id.clone(), entity_id.clone()),
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
            },
        );
    }
    Ok(map)
}

/// Identify which top-level fields differ between the logged state and the
/// live state. Best-effort: if the JSON isn't an object on either side, we
/// fall back to a synthetic "<root>". The list is purely informational —
/// the load-bearing signal is the checksum mismatch itself.
fn diff_top_level_fields(logged: &Value, current: &Value) -> Vec<String> {
    match (logged, current) {
        (Value::Object(l), Value::Object(c)) => {
            let mut keys: std::collections::BTreeSet<&String> = l.keys().collect();
            keys.extend(c.keys());
            keys.iter()
                .filter(|k| l.get(**k) != c.get(**k))
                .map(|s| (*s).clone())
                .collect()
        }
        _ => vec!["<root>".into()],
    }
}

/// Helper for `agora verify` CLI: read latest mutation row directly. Used
/// by Phase-5 edge-case tests so we don't have to expose the SQL twice.
pub async fn debug_latest(
    pool: &PgPool,
    type_id: &str,
    entity_id: &str,
) -> Result<Option<LoggedMutation>> {
    mutation_log::latest_for_entity(pool, type_id, entity_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diff_identifies_changed_fields() {
        let logged = json!({"id": "x", "provider": "plaid"});
        let current = json!({"id": "x", "provider": "evil"});
        let changed = diff_top_level_fields(&logged, &current);
        assert_eq!(changed, vec!["provider".to_string()]);
    }

    #[test]
    fn diff_handles_multiple_changes() {
        let logged = json!({"id": "x", "provider": "plaid", "extra": 1});
        let current = json!({"id": "y", "provider": "evil", "extra": 1});
        let mut changed = diff_top_level_fields(&logged, &current);
        changed.sort();
        assert_eq!(changed, vec!["id".to_string(), "provider".to_string()]);
    }

    #[test]
    fn diff_with_non_object_returns_root() {
        let logged = json!("string-payload");
        let current = json!("other-payload");
        assert_eq!(diff_top_level_fields(&logged, &current), vec!["<root>".to_string()]);
    }
}
