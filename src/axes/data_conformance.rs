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
use crate::seed::ConceptCard;
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

pub async fn run(
    proposal: &OntologyChangeProposal,
    catalog: &[ConceptCard],
    db: Option<&PgPool>,
) -> DataConformance {
    // Step 1 — is this proposal even capable of invalidating data?
    let Some(candidate) = candidate_query(proposal) else {
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
    let Candidate {
        table,
        field,
        target_fqn,
    } = candidate;

    // Step 2 — field-name validation. Postgres identifiers can't be bound as
    // SQL parameters, so we allowlist by looking the field up in the catalog
    // (the same source of truth `composition.rs` uses). A regex shape check is
    // belt-and-braces; without it a future catalog entry with a quote in its
    // name would still poison the SQL.
    if !is_valid_identifier(&field) {
        return DataConformance {
            applicable: true,
            outcome: Outcome::Skipped,
            violations_found: 0,
            sample_violations: vec![],
            query: None,
            query_time_ms: 0,
            source: format!("skipped: field `{}` is not a valid identifier", field),
        };
    }
    if !field_in_catalog(catalog, &target_fqn, &field) {
        return DataConformance {
            applicable: true,
            outcome: Outcome::Skipped,
            violations_found: 0,
            sample_violations: vec![],
            query: None,
            query_time_ms: 0,
            source: format!(
                "skipped: field `{}` not present on `{}` in registry catalog",
                field, target_fqn
            ),
        };
    }

    // Both `table` (allowlist via map_table) and `field` (catalog + regex
    // validated) are now safe to interpolate as identifiers. The query is
    // built ONCE here so the report's `query` field is literally what ran.
    let count_q = format!(
        "SELECT COUNT(*)::BIGINT AS n FROM {} WHERE {} IS NULL",
        table, field
    );

    // Step 3 — do we have a DB to query?
    let Some(pool) = db else {
        return DataConformance {
            applicable: true,
            outcome: Outcome::Skipped,
            violations_found: 0,
            sample_violations: vec![],
            query: Some(count_q),
            query_time_ms: 0,
            source: "skipped: no DB connection".into(),
        };
    };

    // Step 4 — schema present? If not, treat as informational skip.
    if !table_exists(pool, table).await {
        return DataConformance {
            applicable: true,
            outcome: Outcome::Skipped,
            violations_found: 0,
            sample_violations: vec![],
            query: Some(count_q),
            query_time_ms: 0,
            source: format!("skipped: table `{}` not present", table),
        };
    }

    // Step 5 — count violations + collect samples.
    let started = Instant::now();
    let n: i64 = match sqlx::query(&count_q).fetch_one(pool).await {
        Ok(row) => row.try_get::<i64, _>("n").unwrap_or(0),
        Err(e) => {
            return DataConformance {
                applicable: true,
                outcome: Outcome::Skipped,
                violations_found: 0,
                sample_violations: vec![],
                query: Some(count_q),
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

/// What the axis needs to know to run a check, once we've decided the
/// proposal *could* invalidate existing rows.
struct Candidate {
    table: &'static str,
    field: String,
    target_fqn: String,
}

/// Returns a `Candidate` when the proposal could invalidate existing data.
/// Today only `TightenField` (optional→required) has a non-trivial check;
/// everything else is "not applicable".
fn candidate_query(p: &OntologyChangeProposal) -> Option<Candidate> {
    match &p.change {
        Change::TightenField {
            type_ref,
            field_name,
            from_required,
            to_required,
        } if !from_required && *to_required => {
            let table = map_table(type_ref)?;
            Some(Candidate {
                table,
                field: field_name.clone(),
                target_fqn: type_ref.fqn(),
            })
        }
        // AddField with required=true on an existing concept would, after
        // migration, leave existing rows with NULL — but the column doesn't
        // exist YET so we can't query. The compiler workstream's backfill
        // step will gate that separately. We mark it not-applicable here.
        _ => None,
    }
}

/// `^[a-z_][a-z0-9_]{0,62}$` — Postgres identifier shape (case-insensitive
/// in practice; we keep snake_case to match our DDL emitter). Bounds the
/// length at 63 (Postgres NAMEDATALEN-1) so absurdly long inputs short-circuit.
fn is_valid_identifier(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    let mut chars = s.chars();
    let first = chars.next().expect("non-empty already checked");
    if !(first.is_ascii_lowercase() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Looks the field up on the named concept in the seed catalog. This is the
/// real allowlist: a proposal cannot drive the data-conformance axis at a
/// column unless the ontology registry already knows about it.
fn field_in_catalog(catalog: &[ConceptCard], target_fqn: &str, field_name: &str) -> bool {
    catalog
        .iter()
        .find(|c| c.fqn == target_fqn)
        .map(|c| c.spec.fields.iter().any(|f| f.name == field_name))
        .unwrap_or(false)
}

async fn table_exists(pool: &PgPool, table: &str) -> bool {
    let q = "SELECT to_regclass($1) IS NOT NULL AS exists";
    match sqlx::query(q).bind(table).fetch_one(pool).await {
        Ok(row) => row.try_get::<bool, _>("exists").unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_accepts_real_columns() {
        assert!(is_valid_identifier("email"));
        assert!(is_valid_identifier("display_name"));
        assert!(is_valid_identifier("_internal"));
        assert!(is_valid_identifier("id1"));
    }

    #[test]
    fn identifier_rejects_injection_attempts() {
        // The smoke-test cases the deslop review flagged.
        assert!(!is_valid_identifier("email; DROP TABLE accounts; --"));
        assert!(!is_valid_identifier("email\""));
        assert!(!is_valid_identifier("email OR 1=1"));
        assert!(!is_valid_identifier(""));
        assert!(!is_valid_identifier("1starts_with_digit"));
        assert!(!is_valid_identifier("Email")); // uppercase rejected — we keep snake_case
        assert!(!is_valid_identifier(&"a".repeat(64)));
    }
}
