//! `agorad` — the Agora HTTP control plane.
//!
//! Starts an Axum server, runs migrations, and exposes the F1/F2/F3 library
//! entry points over HTTP. The Browser Demo (F4) is the intended client.
//!
//! Configuration is via env / flags:
//!   --port (or AGORAD_PORT)            default 3030
//!   --bind (or AGORAD_BIND)            default 127.0.0.1
//!   --db   (or DATABASE_URL)           Postgres URL; if absent some endpoints 503
//!   --generated-root                   where proposal artifacts live (default ./generated)
//!   --skip-migrate                     skip running migrations on startup

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "agorad", about = "Agora HTTP control plane", version)]
struct Args {
    /// TCP port to listen on.
    #[arg(long, env = "AGORAD_PORT", default_value_t = 3030)]
    port: u16,

    /// Bind address.
    #[arg(long, env = "AGORAD_BIND", default_value = "127.0.0.1")]
    bind: String,

    /// Postgres connection URL. Falls back to DATABASE_URL.
    #[arg(long, env = "DATABASE_URL")]
    db: Option<String>,

    /// Where proposal artifacts are emitted / read from.
    #[arg(long, env = "AGORAD_GENERATED_ROOT", default_value = "generated")]
    generated_root: PathBuf,

    /// Skip running migrations on startup.
    #[arg(long)]
    skip_migrate: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,sqlx=warn")),
        )
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .init();

    let args = Args::parse();
    let ip: IpAddr = args
        .bind
        .parse()
        .with_context(|| format!("parsing --bind {:?} as an IP address", args.bind))?;
    let addr = SocketAddr::new(ip, args.port);

    agora::daemon::serve_forever(addr, args.db, args.generated_root, args.skip_migrate).await
}
