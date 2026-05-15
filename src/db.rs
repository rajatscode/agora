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
pub async fn migrate(pool: &PgPool) -> Result<()> {
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
    Ok(())
}
