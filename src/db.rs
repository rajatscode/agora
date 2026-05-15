//! Postgres connection + migrations.
//!
//! Hackathon stance: one Postgres instance, one pool, migrations applied
//! eagerly at CLI startup. Idempotent (every migration uses IF NOT EXISTS /
//! ON CONFLICT DO NOTHING).
//!
//! The `connect` helper returns `Option<PgPool>` rather than failing: F2's
//! data-conformance axis is supposed to gracefully skip when there's no
//! reachable DB (per Feature-2 spec), and the CLI shouldn't crash just
//! because someone runs it without Postgres running.

use anyhow::{Context, Result};
use sqlx::postgres::{PgPool, PgPoolOptions};

/// Embedded SQL — kept inline so `cargo run` works without any external
/// migration tooling. If we later switch to sqlx-cli these are the same
/// files on disk (migrations/001_*, 002_*).
const MIGRATION_001: &str = include_str!("../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_seed_accounts.sql");
const MIGRATION_003: &str = include_str!("../migrations/003_mutation_log_checksum.sql");
const MIGRATION_004: &str = include_str!("../migrations/004_policy_denial.sql");
const MIGRATION_005: &str = include_str!("../migrations/005_customer_domain.sql");

/// Connect to Postgres. Returns Ok(None) if DATABASE_URL is unset OR the
/// connection attempt fails — caller is expected to fall back to a
/// "skipped: no DB" verdict on data-conformance.
pub async fn connect_optional(database_url: Option<&str>) -> Result<Option<PgPool>> {
    let url = match database_url {
        Some(u) => u.to_string(),
        None => match std::env::var("DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                tracing::warn!("DATABASE_URL unset; data-conformance axis will skip");
                return Ok(None);
            }
        },
    };
    match PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(&url)
        .await
    {
        Ok(pool) => Ok(Some(pool)),
        Err(e) => {
            tracing::warn!(
                "could not connect to Postgres ({e}); data-conformance axis will skip"
            );
            Ok(None)
        }
    }
}

/// Run M0 migrations. Idempotent — safe to call on every CLI invocation.
///
/// We serialize concurrent invocations behind a Postgres advisory lock.
/// Without it, parallel test runs (each calling `migrate()` against the
/// same DB) deadlock on the seed `INSERT … ON CONFLICT DO NOTHING`
/// statements: row-level locks acquired by competing inserts can form a
/// cycle even when the final outcome is a no-op. The advisory lock takes
/// O(ms) and is released on session/connection drop, so this is harmless
/// for the CLI and decisive for the test suite.
///
/// The magic number is just an app-specific identifier — pg_advisory_lock
/// keys are 64-bit ints in any namespace. `0xA607A` is "AGORA" in leet.
const MIGRATION_LOCK_KEY: i64 = 0xA607A;

pub async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(pool)
        .await
        .context("acquiring migration advisory lock")?;

    let result = run_migrations_locked(pool).await;

    // Best-effort unlock; we don't promote an unlock error over a real
    // migration error.
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .execute(pool)
        .await;

    result
}

async fn run_migrations_locked(pool: &PgPool) -> Result<()> {
    sqlx::raw_sql(MIGRATION_001)
        .execute(pool)
        .await
        .context("running migrations/001_init.sql")?;
    sqlx::raw_sql(MIGRATION_002)
        .execute(pool)
        .await
        .context("running migrations/002_seed_accounts.sql")?;
    sqlx::raw_sql(MIGRATION_003)
        .execute(pool)
        .await
        .context("running migrations/003_mutation_log_checksum.sql")?;
    sqlx::raw_sql(MIGRATION_004)
        .execute(pool)
        .await
        .context("running migrations/004_policy_denial.sql")?;
    sqlx::raw_sql(MIGRATION_005)
        .execute(pool)
        .await
        .context("running migrations/005_customer_domain.sql")?;
    Ok(())
}
