//! `agora` CLI entry points (clap-derive). Today: just `propose`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::artifacts::{self, ArtifactManifest};
use crate::ast::OntologyChangeProposal;
use crate::llm;
use crate::reuse::{self, ReuseReport};
use crate::seed;

#[derive(Debug, Parser)]
#[command(
    name = "agora",
    about = "Agora — governed operational ontology control plane",
    version
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Author + classify + post + emit-artifacts for a single proposal.
    Propose(ProposeArgs),
}

#[derive(Debug, clap::Args)]
pub struct ProposeArgs {
    /// Natural-language description of the change.
    /// Example: agora propose "users need biometric login on mobile"
    pub prompt: String,

    /// Identity to attribute the proposal to. Stored in `provenance.author`.
    #[arg(long, default_value = "agent://schema-broker-1")]
    pub actor: String,

    /// Where to write generated artifacts.
    #[arg(long, default_value = "generated")]
    pub out: PathBuf,

    /// Where to write the proposal JSON file (in addition to artifacts dir).
    #[arg(long)]
    pub proposal_out: Option<PathBuf>,

    /// Base URL of the Agora API. CLI POSTs `proposal` to `{api}/proposals`.
    /// If unset (or unreachable) the CLI completes anyway and warns.
    #[arg(long, env = "AGORA_API")]
    pub api: Option<String>,

    /// Print the full proposal JSON to stdout (in addition to the summary).
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
pub struct ProposeOutcome {
    pub proposal: OntologyChangeProposal,
    pub reuse: ReuseReport,
    pub artifacts: ArtifactManifest,
    pub api_post_status: Option<String>, // None = not attempted; Some("201 Created") | Some("error: ...")
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Propose(args) => run_propose(args).await,
    }
}

async fn run_propose(args: ProposeArgs) -> Result<()> {
    eprintln!("[agora] authoring proposal from: {:?}", args.prompt);
    let proposal = llm::author_proposal(&args.prompt, &args.actor)
        .await
        .context("authoring proposal")?;
    eprintln!(
        "[agora]   id={} target={} change_intent={:?}",
        proposal.id,
        proposal.target().fqn(),
        proposal.change_intent
    );

    eprintln!("[agora] running 3-layer reuse detection (exact → jaccard → embedding)");
    let report = reuse::classify(&proposal, &seed::baseline_concepts());
    eprintln!(
        "[agora]   verdict={:?} — {}",
        report.class, report.explanation
    );
    for h in &report.top_hits {
        eprintln!(
            "[agora]   hit: {} (layer={}, score={:.2}, jaccard={:.2}, cosine={:.2})",
            h.fqn, h.layer, h.score, h.jaccard, h.cosine
        );
    }

    eprintln!(
        "[agora] emitting 4 artifacts under {}/{}/",
        args.out.display(),
        proposal.id
    );
    let artifacts = artifacts::emit_all(&proposal, &args.out)?;
    eprintln!("[agora]   .proto    → {}", artifacts.proto);
    eprintln!("[agora]   DDL       → {}", artifacts.ddl);
    eprintln!("[agora]   handler   → {}", artifacts.handler);
    eprintln!("[agora]   OpenFGA   → {}", artifacts.openfga);

    // Always also write the proposal JSON next to the artifacts so other
    // workstreams can pick it up without parsing the manifest.
    let proposal_path = args
        .proposal_out
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from(&artifacts.directory).join("proposal.json"));
    std::fs::write(
        &proposal_path,
        serde_json::to_vec_pretty(&proposal).context("serialize proposal")?,
    )
    .with_context(|| format!("writing proposal to {}", proposal_path.display()))?;
    eprintln!("[agora]   proposal  → {}", proposal_path.display());

    let api_status = if let Some(api_root) = &args.api {
        match post_proposal(api_root, &proposal).await {
            Ok(s) => {
                eprintln!("[agora] POST {} /proposals → {}", api_root, s);
                Some(s)
            }
            Err(e) => {
                eprintln!(
                    "[agora] POST {}/proposals failed: {} (continuing — artifacts already on disk)",
                    api_root, e
                );
                Some(format!("error: {e}"))
            }
        }
    } else {
        eprintln!("[agora] AGORA_API unset — skipped POST /proposals");
        None
    };

    let outcome = ProposeOutcome {
        proposal,
        reuse: report,
        artifacts,
        api_post_status: api_status,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
    } else {
        // Compact one-screen summary for humans / demo.
        println!();
        println!("┌─ Agora proposal authored ───────────────────────────────");
        println!("│ id           : {}", outcome.proposal.id);
        println!("│ target       : {}", outcome.proposal.target().fqn());
        println!("│ change_intent: {}", outcome.proposal.change_intent);
        println!("│ reuse class  : {:?}", outcome.reuse.class);
        if let Some(top) = outcome.reuse.top_hits.first() {
            println!(
                "│ closest hit  : {} (score {:.2})",
                top.fqn, top.score
            );
        }
        println!("│ artifacts in : {}", outcome.artifacts.directory);
        if let Some(s) = &outcome.api_post_status {
            println!("│ api POST     : {}", s);
        }
        println!("└─────────────────────────────────────────────────────────");
    }

    Ok(())
}

async fn post_proposal(api_root: &str, proposal: &OntologyChangeProposal) -> Result<String> {
    let url = format!("{}/proposals", api_root.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let resp = client.post(&url).json(proposal).send().await?;
    Ok(format!(
        "{} {}",
        resp.status().as_u16(),
        resp.status().canonical_reason().unwrap_or("")
    ))
}
