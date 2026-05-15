//! `agora` CLI binary.
//!
//! Today this dispatches `agora propose ...`. The other workstreams will
//! add `agora serve`, `agora migrate`, etc. as their pods land.

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .compact()
        .init();

    let cli = agora::cli::Cli::parse();
    agora::cli::run(cli).await
}

/// Shared HTTP application state — referenced by *generated* handler skeletons
/// (see `src/artifacts.rs::render_handler`). Kept here so generated code has a
/// real type to import; runtime workstream (WS-D) will replace this with a
/// proper `AppState` carrying the registry + fact-log handles.
#[derive(Clone, Default)]
#[allow(dead_code)]
pub struct AppState;
