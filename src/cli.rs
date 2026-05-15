//! `agora` CLI entry points (clap-derive). Today: just `propose`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::artifacts::{self, ArtifactManifest};
use crate::ast::OntologyChangeProposal;
use crate::check;
use crate::check_report::CheckReport;
use crate::db;
use crate::entity_write::{self, CreateBankIntegrationCmd, WriteOrigin, WriteOutcome};
use crate::explorer;
use crate::llm::{self, AuthorMode};
use crate::reuse::{self, ReuseReport};
use crate::seed;
use crate::verify;

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
    /// Run the multi-axis risk gate on an existing proposal JSON.
    Check(CheckArgs),
    /// Apply a controlled entity write (atomic INSERT + mutation_log). Today
    /// the only supported entity is `bank-integration`; the daemon workstream
    /// will add the rest.
    Write(WriteArgs),
    /// Drift detection: compare live entity tables to mutation_log checksums.
    Verify(VerifyArgs),
    /// Discovery: render a concept's metadata, lineage, policy and history.
    Explorer(ExplorerArgs),
}

#[derive(Debug, clap::Args)]
pub struct WriteArgs {
    #[command(subcommand)]
    pub kind: WriteKind,
}

#[derive(Debug, Subcommand)]
pub enum WriteKind {
    /// Create or update a BankIntegration row + log the mutation.
    BankIntegration(WriteBankIntegrationArgs),
}

#[derive(Debug, clap::Args)]
pub struct WriteBankIntegrationArgs {
    /// Stable entity id (e.g. "bi_acme").
    #[arg(long)]
    pub entity_id: String,
    /// Provider name (e.g. "plaid", "mx").
    #[arg(long)]
    pub provider: String,
    /// Ontology version under which this write is authored.
    #[arg(long, default_value_t = 2)]
    pub ontology_version: i32,
    /// Where the write originated.
    #[arg(long, default_value = "cli")]
    pub origin: String,
    /// Postgres connection string. Falls back to DATABASE_URL.
    #[arg(long, env = "DATABASE_URL")]
    pub db: Option<String>,
    /// Skip running migrations on startup.
    #[arg(long)]
    pub skip_migrate: bool,
}

#[derive(Debug, clap::Args)]
pub struct VerifyArgs {
    /// Postgres connection string. Falls back to DATABASE_URL.
    #[arg(long, env = "DATABASE_URL")]
    pub db: Option<String>,
    /// Skip running migrations on startup.
    #[arg(long)]
    pub skip_migrate: bool,
}

#[derive(Debug, clap::Args)]
pub struct ExplorerArgs {
    /// Concept FQN, e.g. `core.integrations.BankIntegration`.
    pub fqn: String,
    /// Postgres connection string. Optional — without a DB the explorer
    /// falls back to the offline seed catalog (no version history).
    #[arg(long, env = "DATABASE_URL")]
    pub db: Option<String>,
    /// Skip running migrations on startup.
    #[arg(long)]
    pub skip_migrate: bool,
}

#[derive(Debug, clap::Args)]
pub struct CheckArgs {
    /// Path to the proposal JSON (typically `generated/<id>/proposal.json`).
    pub proposal: PathBuf,

    /// Postgres connection string for the data-conformance axis.
    /// Falls back to `DATABASE_URL` env var; if both are absent the
    /// data-conformance axis is skipped (informational).
    #[arg(long, env = "DATABASE_URL")]
    pub db: Option<String>,

    /// Optional path to write the CheckReport JSON. Defaults to
    /// `<proposal-dir>/check_report.json`.
    #[arg(long)]
    pub report_out: Option<PathBuf>,

    /// Skip running migrations on startup (useful if the DB is already set up).
    #[arg(long)]
    pub skip_migrate: bool,

    /// Print the full CheckReport JSON to stdout (default). Tracing always
    /// goes to stderr.
    #[arg(long, default_value_t = true)]
    pub json: bool,
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
    /// Where the proposal came from. `Live` = LLM-derived (so
    /// `proposal.compatibility.semantic` is the LLM's verdict on the
    /// semantic delta). Anything else = heuristic stand-in; downstream
    /// consumers must NOT trust the compatibility classifications as
    /// independent semantic checks.
    pub author_mode: AuthorMode,
    pub proposal: OntologyChangeProposal,
    pub reuse: ReuseReport,
    pub artifacts: ArtifactManifest,
    pub api_post_status: Option<String>, // None = not attempted; Some("201 Created") | Some("error: ...")
}

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Propose(args) => run_propose(args).await,
        Command::Check(args) => run_check(args).await,
        Command::Write(args) => run_write(args).await,
        Command::Verify(args) => run_verify(args).await,
        Command::Explorer(args) => run_explorer(args).await,
    }
}

async fn run_write(args: WriteArgs) -> Result<()> {
    match args.kind {
        WriteKind::BankIntegration(a) => run_write_bank_integration(a).await,
    }
}

async fn run_write_bank_integration(args: WriteBankIntegrationArgs) -> Result<()> {
    eprintln!(
        "[agora] write: BankIntegration entity_id={} provider={}",
        args.entity_id, args.provider
    );
    let pool = db::connect_optional(args.db.as_deref()).await?;
    let Some(pool) = pool else {
        anyhow::bail!("agora write requires a reachable Postgres (set DATABASE_URL or --db)");
    };
    if !args.skip_migrate {
        db::migrate(&pool).await.context("running migrations")?;
    }

    let origin = match args.origin.as_str() {
        "cli" => WriteOrigin::Cli,
        "http-handler" | "http" => WriteOrigin::HttpHandler,
        other => anyhow::bail!("--origin must be 'cli' or 'http-handler' (got {other:?})"),
    };

    let cmd = CreateBankIntegrationCmd {
        entity_id: args.entity_id.clone(),
        provider: args.provider.clone(),
    };
    let outcome: WriteOutcome =
        entity_write::apply_create_bank_integration(&pool, &cmd, args.ontology_version, origin)
            .await
            .context("applying bank_integration write")?;

    eprintln!(
        "[agora] write: 201 Created entity_id={} mutation_seq={} ontology_version={} checksum={}",
        outcome.entity_id,
        outcome.mutation_seq,
        outcome.ontology_version,
        outcome.checksum
    );
    let json = serde_json::to_string_pretty(&outcome).context("serializing write outcome")?;
    println!("{json}");
    Ok(())
}

async fn run_verify(args: VerifyArgs) -> Result<()> {
    eprintln!("[agora] verify: scanning entity tables for drift");
    let pool = db::connect_optional(args.db.as_deref()).await?;
    let Some(pool) = pool else {
        anyhow::bail!("agora verify requires a reachable Postgres (set DATABASE_URL or --db)");
    };
    if !args.skip_migrate {
        db::migrate(&pool).await.context("running migrations")?;
    }

    let report = verify::verify(&pool).await.context("running verify")?;
    eprintln!(
        "[agora] verify: status={:?} entities_checked={} drift={} out_of_band={} ({} ms)",
        report.verify_status,
        report.entities_checked,
        report.tampered_entities.len(),
        report.outofband_entities.len(),
        report.elapsed_ms,
    );
    for finding in &report.tampered_entities {
        eprintln!(
            "[agora]   DRIFT {} {} fields={:?} logged_at={} actor={}",
            finding.entity_type,
            finding.entity_id,
            finding.fields_changed,
            finding.last_logged_at,
            finding.last_logged_actor,
        );
    }
    for finding in &report.outofband_entities {
        eprintln!(
            "[agora]   OUT-OF-BAND {} {}",
            finding.entity_type, finding.entity_id,
        );
    }
    let json = serde_json::to_string_pretty(&report).context("serializing verify report")?;
    println!("{json}");
    Ok(())
}

async fn run_explorer(args: ExplorerArgs) -> Result<()> {
    eprintln!("[agora] explorer: loading view for {}", args.fqn);
    let pool = db::connect_optional(args.db.as_deref()).await?;
    if let Some(p) = &pool {
        if !args.skip_migrate {
            db::migrate(p).await.context("running migrations")?;
        }
    } else {
        eprintln!("[agora] explorer: no DB — version history will be empty (offline mode)");
    }
    let view = explorer::explorer(pool.as_ref(), &args.fqn)
        .await
        .context("running explorer")?;
    match view {
        None => {
            eprintln!("[agora] explorer: concept {} not found in registry catalog", args.fqn);
            anyhow::bail!("concept not found: {}", args.fqn);
        }
        Some(view) => {
            let json = serde_json::to_string_pretty(&view)
                .context("serializing concept view")?;
            println!("{json}");
            Ok(())
        }
    }
}


async fn run_check(args: CheckArgs) -> Result<()> {
    eprintln!("[agora] check: loading proposal from {}", args.proposal.display());
    let raw = std::fs::read_to_string(&args.proposal)
        .with_context(|| format!("reading proposal from {}", args.proposal.display()))?;
    let proposal: OntologyChangeProposal =
        serde_json::from_str(&raw).context("parsing proposal JSON")?;

    // Connect to Postgres (graceful: returns None if unset/unreachable).
    let pool = db::connect_optional(args.db.as_deref()).await?;
    if let Some(p) = &pool {
        if !args.skip_migrate {
            eprintln!("[agora] check: applying M0 migrations (idempotent)");
            db::migrate(p).await.context("running migrations")?;
        } else {
            eprintln!("[agora] check: --skip-migrate set; assuming schema is in place");
        }
    } else {
        eprintln!("[agora] check: no DB connection — data-conformance axis will skip");
    }

    let catalog = seed::baseline_concepts();

    let report = check::check(&proposal, &catalog, pool.as_ref())
        .await
        .context("running multi-axis risk gate")?;

    // Stderr summary for humans.
    eprintln!(
        "[agora] check: proposal {} → {} (auto_approval_eligible={})",
        report.proposal_id, report.status, report.auto_approval_eligible
    );
    for row in &report.checks {
        eprintln!(
            "[agora]   axis={:12} outcome={:8?} confidence={:?} src={} ({} ms) — {}",
            row.axis.as_str(),
            row.outcome,
            row.confidence,
            row.source,
            row.elapsed_ms,
            row.findings
        );
    }
    eprintln!(
        "[agora]   axis=data_conformance outcome={:?} violations={} ({} ms) src={}",
        report.data_conformance.outcome,
        report.data_conformance.violations_found,
        report.data_conformance.query_time_ms,
        report.data_conformance.source,
    );
    if let Some(br) = &report.block_reason {
        eprintln!("[agora]   block_reason: {}", br);
    }
    eprintln!("[agora]   total elapsed: {} ms", report.elapsed_ms);

    // Always write the report next to the proposal for downstream pods.
    let report_path = args.report_out.clone().unwrap_or_else(|| {
        args.proposal
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
            .join("check_report.json")
    });
    if let Some(parent) = report_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating dir {}", parent.display()))?;
        }
    }
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).context("serializing CheckReport")?,
    )
    .with_context(|| format!("writing CheckReport to {}", report_path.display()))?;
    eprintln!("[agora]   report   → {}", report_path.display());

    if args.json {
        let s = serde_json::to_string_pretty(&report).context("CheckReport to JSON")?;
        println!("{}", s);
    }

    Ok(())
}

#[derive(Debug, Serialize)]
pub struct CheckOutcome {
    pub report: CheckReport,
}

async fn run_propose(args: ProposeArgs) -> Result<()> {
    eprintln!("[agora] authoring proposal from: {:?}", args.prompt);
    let (proposal, author_mode) = llm::author_proposal(&args.prompt, &args.actor)
        .await
        .context("authoring proposal")?;

    // Loud, unmissable banner when offline — fig-leaf protection per Nemesis.
    if !author_mode.is_live() {
        eprintln!("[agora] ╔════════════════════════════════════════════════════════════╗");
        eprintln!("[agora] ║  ⚠  AUTHOR MODE: {:42}  ║", author_mode.label());
        eprintln!("[agora] ║  compatibility.* axes are heuristic stand-ins, NOT LLM-derived ║");
        eprintln!("[agora] ║  downstream consumers should treat them as low-confidence  ║");
        eprintln!("[agora] ╚════════════════════════════════════════════════════════════╝");
    } else {
        eprintln!("[agora]   author mode: live (LLM-derived compatibility classifications)");
    }
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
        author_mode: author_mode.clone(),
        proposal,
        reuse: report,
        artifacts,
        api_post_status: api_status,
    };

    if args.json {
        let json = serde_json::to_string_pretty(&outcome)
            .context("serializing propose outcome to JSON")?;
        println!("{}", json);
    } else {
        // Compact one-screen summary for humans / demo.
        println!();
        println!("┌─ Agora proposal authored ───────────────────────────────");
        println!("│ author mode  : {}", outcome.author_mode.label());
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
        if !outcome.author_mode.is_live() {
            println!("│ ⚠ NOTE      : compatibility.* are heuristic stand-ins (offline)");
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
