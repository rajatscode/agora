//! Data-conformance axis — can existing data survive this proposal?
//!
//! THIS IS BEAT 6'S PROOF. The semantic axis says "tightening email is a
//! refinement"; this axis runs a real SQL query against the Account table
//! and returns the actual count of rows that violate the proposed constraint
//! (47, per the seed in `migrations/002_seed_accounts.sql`).
//!
//! Failure modes the impl brief calls out:
//!   - DATABASE_URL unset / Postgres unreachable → Skipped (advisory, not block)
//!   - Target table doesn't exist → Skipped (advisory, not block)
//!   - Target field doesn't exist → Skipped (advisory, not block)
//!   - Query succeeds with N>0 → Fail with sample rows
//!   - Query succeeds with N=0 → Pass
//!
//! Auto-approval is only allowed when this axis is Pass *or* `applicable=false`
//! (e.g. proposal can't possibly invalidate data, like adding an optional field).

use crate::ast::{Change, OntologyChangeProposal, TypeRef};
use crate::check_report::{DataConformance, Outcome, SampleViolation};
use sqlx::{PgPool, Row};
use std::time::Instant;

const MAX_SAMPLES: usize = 5;

/// Map an ontology concept FQN to the live Postgres table name.
/// This is the M0 manual mapping — a real registry would have an
/// `ontology_types` row carrying the storage binding.
fn map_table(target: &TypeRef) -> Option<&'static str> {
    match target.fqn().as_str() {
        "core.users.Account" => Some("accounts"),
        "core.integrations.BankIntegration" => Some("bank_integrations"),
        "core.integrations.AuthenticationMethod" => Some("authentication_methods"),
        _ => None,
    }
}

pub async fn run(proposal: &OntologyChangeProposal, db: Option<&PgPool>) -> DataConformance {
    // Step 1 — is this proposal even capable of invalidating data?
    let Some((table, field, query)) = candidate_query(proposal) else {
        return DataConformance {
            applicable: false,
            outcome: Outcome::Pass,
            violations_found: 0,
            sample_violations: vec![],
            query: None,
            query_time_ms: 0,
            source: "not-applicable".into(),
        };
    };

    // Step 2 — do we have a DB to query?
    let Some(pool) = db else {
        return DataConformance {
            applicable: true,
            outcome: Outcome::Skipped,
            violations_found: 0,
            sample_violations: vec![],
            query: Some(query),
            query_time_ms: 0,
            source: "skipped: no DB connection".into(),
        };
    };

    // Step 3 — schema present? If not, treat as informational pass.
    if !table_exists(pool, table).await {
        return DataConformance {
            applicable: true,
            outcome: Outcome::Skipped,
            violations_found: 0,
            sample_violations: vec![],
            query: Some(query),
            query_time_ms: 0,
            source: format!("skipped: table `{}` not present", table),
        };
    }

    // Step 4 — count violations + collect samples.
    let started = Instant::now();
    let count_q = format!(
        "SELECT COUNT(*)::BIGINT AS n FROM {} WHERE {} IS NULL",
        table, field
    );
    let n: i64 = match sqlx::query(&count_q).fetch_one(pool).await {
        Ok(row) => row.try_get::<i64, _>("n").unwrap_or(0),
        Err(e) => {
            return DataConformance {
                applicable: true,
                outcome: Outcome::Skipped,
                violations_found: 0,
                sample_violations: vec![],
                query: Some(query),
                query_time_ms: started.elapsed().as_millis() as u64,
                source: format!("skipped: query error: {}", e),
            };
        }
    };

    let mut samples: Vec<SampleViolation> = Vec::new();
    if n > 0 {
        let sample_q = format!(
            "SELECT id::TEXT AS entity_id FROM {} WHERE {} IS NULL ORDER BY id LIMIT {}",
            table, field, MAX_SAMPLES
        );
        if let Ok(rows) = sqlx::query(&sample_q).fetch_all(pool).await {
            for r in rows {
                if let Ok(eid) = r.try_get::<String, _>("entity_id") {
                    samples.push(SampleViolation {
                        entity_id: eid,
                        reason: format!("{}.{} IS NULL but proposal requires non-null", table, field),
                    });
                }
            }
        }
    }
    let elapsed = started.elapsed().as_millis() as u64;

    let outcome = if n > 0 { Outcome::Fail } else { Outcome::Pass };

    DataConformance {
        applicable: true,
        outcome,
        violations_found: n,
        sample_violations: samples,
        query: Some(count_q),
        query_time_ms: elapsed,
        source: "postgres".into(),
    }
}

/// Returns (table, field, illustrative-query-string) when the proposal could
/// invalidate existing data. Today only `TightenField` (optional→required)
/// has a non-trivial check; everything else is "not applicable".
fn candidate_query(p: &OntologyChangeProposal) -> Option<(&'static str, String, String)> {
    match &p.change {
        Change::TightenField {
            type_ref,
            field_name,
            from_required,
            to_required,
        } if !from_required && *to_required => {
            let table = map_table(type_ref)?;
            let q = format!(
                "SELECT COUNT(*) FROM {} WHERE {} IS NULL",
                table, field_name
            );
            Some((table, field_name.clone(), q))
        }
        // AddField with required=true on an existing concept would, after
        // migration, leave existing rows with NULL — but the column doesn't
        // exist YET so we can't query. The compiler workstream's backfill
        // step will gate that separately. We mark it not-applicable here.
        _ => None,
    }
}

async fn table_exists(pool: &PgPool, table: &str) -> bool {
    let q = "SELECT to_regclass($1) IS NOT NULL AS exists";
    match sqlx::query(q).bind(table).fetch_one(pool).await {
        Ok(row) => row.try_get::<bool, _>("exists").unwrap_or(false),
        Err(_) => false,
    }
}
