# Agora

A governed operational ontology and control plane. One Rust binary, `agorad`, that sits between AI agents and a Postgres database and mediates every schema change and every write.

---

## What this actually is

`agorad` listens on `:3030` and does five things in sequence:

1. **PROPOSE.** An agent says "we need to model X" in natural language. `agorad` calls Anthropic (or uses a deterministic offline path) and produces a typed `OntologyChangeProposal` plus four artifacts on disk — Protobuf schema, Postgres DDL, Rust HTTP handler skeleton, OpenFGA policy spec.
2. **CHECK.** The proposal flows through a 7-axis risk gate (composition, shape, semantic, policy, temporal, impact, replay) **plus a data-conformance axis that hits live Postgres**. If you propose tightening `Account.email` to required and 47 existing rows have `email IS NULL`, the gate blocks with the exact count and sample violating rows.
3. **AGENT LOOP.** If the gate blocks, the LLM reads the structured rejection and revises the proposal — typically by adding a `migration.backfill_plan` that addresses the violation. The gate re-runs. The same agent code drives three different domains (bank integrations, customer 360, compliance findings) and produces three different domain-appropriate backfill strategies.
4. **WRITE + VERIFY.** Entity creation goes through `POST /entities/{type}`, which writes the row and an append-only `mutation_log` entry (SHA-256 checksum, actor identity, ontology version) in one transaction. An FGA policy can deny the write; denials are themselves logged as `DenyAttempt` rows. `agora verify` recomputes checksums and surfaces drift — including raw SQL `UPDATE`s that bypassed the daemon.
5. **EXPLORE.** `GET /concepts/{fqn}` returns owner team, fields, invariants, policy attachments, and the full version history from `mutation_log`.

All five flows are usable from one browser tab at `http://localhost:3030/`. The same library functions back the CLI (`agora propose | check | write | verify | explorer`) and the HTTP daemon — there is no separate "demo mode."

**The one-line claim.** Schema registries ask "are the bytes compatible?" Agora asks "does this change still tell the truth about the actual stored data?" — answered by a live SQL query, not a static type check, with a closed agent loop that revises blocked proposals until they are safe.

---

## The problem being solved

Three forces collide:

1. **Agentic development is fast and incoherent.** Every team and every agent invents its own data model, its own column names, its own meaning for the same concept. The result is duplicated meaning, silent semantic drift, and joins that produce wrong answers across product surfaces.
2. **The standard fixes are worse than the problem.** A central data team becomes a throughput bottleneck (every change waits behind every other change). Schemaless stores externalize coherence into tribal knowledge and post-hoc reconciliation. Neither scales to "thousands of agents touching the data layer."
3. **Compatibility checks at the schema layer are not enough.** "Adding a `NOT NULL` column is backward-compatible at the proto level" is true and useless if 47 production rows would violate the new constraint. The actual safety question is about the data, not the bytes.

The business requirement Agora targets is: let many agents and humans propose meaning-changing data-layer changes in parallel, decide safely and automatically which ones can land, force the unsafe ones to either fix themselves or get escalated, and produce a tamper-evident audit trail of everything attempted — allowed, denied, or after-the-fact.

This is the layer above "your database" and below "your data catalog." Neither of those tools is shaped right for this job, which is the next section.

---

## Prior art and where it falls short

| Category | Examples | What it does | What it misses for this problem |
|---|---|---|---|
| **Schema registries** | Confluent Schema Registry, Apicurio, Buf Schema Registry | Validates that a new schema version is compatible (backward / forward / full) with the old one at the bytes/types level. | Schema-against-schema, not schema-against-data. Cannot tell you that the change would break 47 existing rows. No notion of intent, ownership, or policy. |
| **Data catalogs** | Backstage, DataHub, Amundsen, Atlan, Collibra, OpenMetadata | Describes datasets, owners, lineage, tags, sometimes quality scores. Searchable browsable view of "what data exists." | Descriptive and after-the-fact. Sits beside the database, not in front of it. Cannot gate a write or block a proposal. The catalog is updated by ingest jobs, not consulted on the write path. |
| **Policy engines** | Open Policy Agent (Rego), AWS Cedar, OpenFGA, SpiceDB | Evaluate authorization decisions ("can user X do action Y on resource Z?"). | Enforcement only. Don't author changes, don't reason about whether the change is semantically safe, don't produce artifacts. Necessary primitive, not the whole story. |
| **Build-time contracts** | dbt contracts, Great Expectations, Soda, Monte Carlo | Test data shape and quality at build time or on a schedule. | Fire after the change has landed (or in CI). Don't gate the proposal itself. Don't produce a structured rejection an agent can read and revise against. |
| **Migration tools** | Flyway, Liquibase, Alembic, sqlx migrate | Apply ordered DDL changes to a database, track which have run. | Mechanical execution. No semantic check, no data-conformance check, no propagation to APIs or policies. Treats migrations as opaque SQL strings. |
| **Bitemporal / versioned stores** | XTDB, Datomic, TerminusDB | Built-in valid-time and transaction-time. Time-travel queries. | Storage substrates, not control planes. No agent loop, no policy/artifact propagation. Strong for "what did the data look like at T" but silent on "should this change be allowed." |
| **Enterprise metadata / MDM** | IBM Watson Knowledge Catalog, Informatica, Alation | Centralized data governance: glossaries, stewardship workflows, lineage. | Built for human committees on multi-week cycles. The bottleneck Agora is trying to remove. |
| **Service-meaning catalogs** | Backstage software templates, ServiceNow CMDB | Describe services and their owners. | Software-topology focused. Don't reach into the data layer. |

The gap Agora claims is the one no existing tool fills: a runtime control plane that **(a)** treats every change as a typed semantic proposal, **(b)** checks the proposal against live data, not just types, **(c)** allows an agent to revise blocked proposals via a structured rejection contract, and **(d)** produces operational artifacts (DDL, proto, handler, policy) from the same source so the catalog view and the runtime registry can't drift.

---

## What Agora does differently

Five concrete moves none of the above prior art combines:

1. **Schema-against-data, not schema-against-schema.** The `data_conformance` axis is a live `SELECT COUNT(*) FROM …` against Postgres, not a type-system inference. The block message contains the actual count, sample violating rows, and the verbatim SQL used to produce them.
2. **One declaration, four artifacts.** A single `OntologyChangeProposal` emits `.proto`, `.sql`, an HTTP handler skeleton, and an FGA policy spec. The catalog view (`GET /concepts/{fqn}`) reads from the same source — descriptive metadata and runtime registry cannot drift apart because they are not separate stores.
3. **The agent loop is closed.** When the gate blocks, the rejection is structured (axis, finding, sample rows, suggested mitigation hint). The agent ingests that JSON and emits a revised proposal — typically adding a `migration.backfill_plan` block that addresses the conformance violation. The gate re-runs. The loop is bounded (`MAX_ATTEMPTS = 3`) and `Stalled` is a first-class outcome.
4. **Every mutation produces an audit row, including denials.** Writes commit an entity-table insert and a `mutation_log` row in one transaction. Policy denials log a `DenyAttempt` row with the rejection reason. Out-of-band SQL `UPDATE`s are caught by `agora verify` (checksum mismatch with a field-level diff). The audit trail is one append-only table.
5. **The framework, not the pipeline.** The same `agent.rs`, `check.rs`, `verify.rs`, `explorer.rs` code runs three domains with disjoint owners, policy classes, and backfill strategies. The agent generalizes its mitigation strategy per domain — `derive_from_user_record` for accounts, `synthetic_placeholder_from_id` for customers, `synthetic_accept_open_findings` for audit findings — from the same prompt template.

---

## Architecture

```
┌────────────────────────────────────────────────────────────────────────┐
│  Browser UI — maud + HTMX, served at http://localhost:3030/            │
│  Every flow as a card. One tab. No reloads.                            │
└─────────────────────────────────┬──────────────────────────────────────┘
                                  │ /ui/* HTMX endpoints
┌─────────────────────────────────▼──────────────────────────────────────┐
│  agorad — Axum HTTP daemon on :3030                                    │
│  JSON control plane: POST /proposals · POST /proposals/{id}/check      │
│  POST /entities/{type} · GET /verify · GET /concepts/{fqn}             │
│  POST /agent/run · POST /admin/reset                                   │
└─────────────────────────────────┬──────────────────────────────────────┘
                                  │ library calls (same code as CLI)
┌─────────────────────────────────▼──────────────────────────────────────┐
│  llm::author_proposal       (live Anthropic + deterministic fallback)  │
│  check::check               (7-axis gate + live data_conformance)      │
│  agent::agent_loop          (block → read rejection → revise → retry)  │
│  entity_write::apply_*      (atomic insert + mutation_log + FGA)       │
│  verify::verify             (checksum drift + out-of-band detection)   │
│  explorer::explorer         (owner / fields / invariants / history)    │
│  artifacts::emit_all        (.proto · .sql · handler · .fga.json)      │
│  seed::*                    (canonical concept catalog)                │
└─────────────────────────────────┬──────────────────────────────────────┘
                                  │ sqlx
┌─────────────────────────────────▼──────────────────────────────────────┐
│  Postgres                                                              │
│  bank_integrations · accounts · customers · audit_findings · …         │
│  mutation_log (append-only, SHA-256 checksums, DenyAttempt rows)       │
└────────────────────────────────────────────────────────────────────────┘
```

Three domains are seeded so the framework's generalization is visible: `core.integrations.*` (BankIntegration, AuthenticationMethod), `core.users.Account`, `core.customer.*` (Customer, LoyaltyTier, PurchaseHistory), `core.compliance.AuditFinding`.

---

## Alternative architectures considered, and why we did not take them

These are the load-bearing decisions and the tradeoff each one makes.

**Language: Rust instead of Python or TypeScript.** Python is the well-trodden choice for anything LLM-adjacent; TypeScript is the well-trodden choice for anything web-adjacent. Both produce a deployable artifact that is "a directory of files plus a runtime"; Rust produces one binary. For a control plane that has to be trustworthy at the write path, "one binary, one connection pool, one process" is structurally simpler. Type safety on the AST and diff types also catches whole classes of bugs at compile time that would be unit tests in Python. Cost: longer compile times, smaller ecosystem of AI SDKs.

**HTTP/JSON, not gRPC.** Agents and browsers both talk JSON natively. `tonic` would buy us `.proto`-defined RPC at the cost of friction for the browser UI and curl-debuggability. We do still accept Protobuf-encoded entity payloads on ingest paths (decoded with `prost`), so the artifact emission story is honest. We just don't gate it behind a gRPC transport.

**JSON policy spec + in-Rust evaluator, not Rego/OPA.** OPA is the obvious choice for policy. It is also a separate process with its own DSL, its own debugger story, and a learning curve. The M0 evaluator is ~250 lines of Rust over five JSON policy files with a four-predicate DSL (`diff_kind`, `classification_delta_direction`, `affects_field_with_class`, `type_level_field_changed`). The M1 migration path is one engineer-week to swap evaluator implementations; the JSON specs round-trip to Rego. The structural claim — "policy is a first-class axis of the check report with cited rule IDs" — survives the swap.

**Append-only Postgres `mutation_log`, not Kafka.** The audit trail's structural requirements are append-only, monotonic sequence, and consumable by drift detection. Postgres satisfies all three with one fewer system to operate. Kafka would buy us downstream fan-out we do not need at M0. The row schema is shaped so a future producer-side swap is mechanical.

**Single-node Postgres, not Spanner / CockroachDB / multi-region.** The locality field on `OntologyType` exists in the AST and propagates through artifacts, but the M0 substrate is one Postgres instance. The architectural piece is the control plane; the substrate is replaceable, and the AST already carries the metadata a multi-region substrate would consume. M1 is a storage-driver swap, not a control-plane rewrite.

**Append-only `mutation_log` with checksums, not full bitemporality.** XTDB or Datomic would give us valid-time + transaction-time natively. The mutation log gives us transaction-time (when did the daemon see this write) and an `ontology_version` snapshot per row. Valid-time is deferred — the demo's audit story does not need it, and treating bitemporality as a sidequest let us prove the agentic loop instead.

**maud + HTMX, not React or Next.** The browser UI needs to render server-state-shaped views (a check report, a verify panel, a concept page) — HTMX swaps fragments into slots without a build step. There is no Node, no bundler, no client framework, no shadow DOM diff layer. The HTMX bytes are inlined into the binary so the UI works without an internet connection.

**Seed catalog as the source of truth, not registry-bootstrap-from-artifacts.** The four generated artifacts (`.proto`, `.sql`, handler, `.fga.json`) are real files written to `generated/{proposal_id}/`, but the canonical concept registry comes from `src/seed.rs`. We did not build the round-trip from emitted artifacts back into the registry — the artifact-emit path is one-way for M0. The thesis (one declaration, many operational artifacts) still lands; the symmetric ingest is M1.

**No GraphQL surface.** The browser UI is HTMX; the agent surface is JSON-over-HTTP. Nothing benefits from a GraphQL gateway at this scope. REST and HTMX endpoints cover every read path the demo needs.

---

## How to run

Prerequisites: Postgres 14+, Rust toolchain.

```bash
# 1. Postgres up (macOS / Homebrew)
brew services start postgresql@14

# 2. Database
createdb agora_dev

# 3. Boot the daemon (runs migrations on startup)
DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad

# 4. Open the UI
open http://localhost:3030
```

Optional: export `ANTHROPIC_API_KEY` before booting for live LLM authoring. Without it, F1 (propose) and the revise step of the agent loop use a deterministic offline path; the author-mode pill on every proposal card honestly reports which mode is in effect.

**Critical operational rule.** After any `cargo build` or `git pull`, restart the daemon. The process caches seed concepts and migrations at boot — a running daemon will not pick up code, migration, or seed changes. Kill it (`Ctrl-C` or `kill $(lsof -ti:3030)`) and start it again. This is failure card #9 in `OPS-PLAYBOOK.md`; capturing it here too because it bit the build four times.

```bash
# Restore demo baselines (47 NULL accounts.email, 5 NULL customers.email,
# 4 NULL audit_findings, etc.):
curl -X POST localhost:3030/admin/reset
```

---

## How to walk through it

The home page at `/` lays out every flow as its own card. Click top-to-bottom.

| Card | Click | Look for |
|---|---|---|
| **Propose + Reuse** | `Propose →` (default prompt) | Proposal card with author-mode pill, four artifact tabs (`.proto` / `.sql` / `_handler.rs` / `.fga.json`), and a reuse-detection table showing the best catalog match. |
| **Multi-axis check** | `Run multi-axis check →` | 8-row table: composition · shape · semantic · policy · temporal · impact · replay · data_conformance. The hint line shows `data_conformance source = …` with the live count. |
| **Auto-approval** | `Submit for approval →` | Green "Auto-approved" card. `predicate ⇒ all_axes_clean=true`. |
| **Risky proposal** | `Run risky proposal →` | Red "Blocked" card. `data_conformance` reports **47** existing `Account.email IS NULL` rows with the verbatim SQL query — verifiable in psql. |
| **Agent loop** | `Run agent loop →` | Attempt 1 = `Authored` → blocked. Attempt 2 = `Revised — added migration.backfill_plan` → approved. The agent emits a domain-appropriate backfill strategy from the rejection rationale. |
| **Write (allow)** | `Write a BankIntegration →` (actor `team:integrations-platform`) | Green "Write committed" with `entity_id`, `mutation_seq`, SHA-256 `checksum`. |
| **Write (deny)** | Switch actor to `team:marketing`, click again | Red "Policy denied" panel with the rejected tuple, and a `DenyAttempt` row in `mutation_log`. |
| **Tamper** | `Tamper this row out-of-band →` | Amber card showing the raw SQL `UPDATE` that bypassed the handler. |
| **Verify** | `Run agora verify →` | Red "Drift detected" with the field-level diff (`provider: plaid → evil_corp_tampered`) on the row you just tampered, plus a list of pre-existing rows the control plane never saw. |
| **Explorer** | Click `core.integrations.BankIntegration` | Owner, fields with classifications, invariants, policy attachments, version history from `mutation_log`. Then `Customer` and `AuditFinding` for the same view on a different domain. |

`DEMO-RUNBOOK.md` has the verbatim narration per beat and the falsifiability check for each.

---

## Honest scope and limitations

Three M0 limitations are surfaced in-UI on the relevant cards. Calling them out once here too:

- **Actor is asserted, not authenticated.** The actor dropdown supplies the policy subject. The FGA-style evaluator (wildcards, considered-tuple introspection, deny logging) is real. The identity-issuance step is not — in production, an auth proxy or JWT decoder hands the actor to the same evaluator unchanged.
- **Offline mode trusts `backfill_plan` presence, not correctness.** When `ANTHROPIC_API_KEY` is unset, the revision step is a deterministic heuristic that emits a structurally valid `migration.backfill_plan`. The gate downgrades `data_conformance` from `fail` to `advisory` because the plan exists, not because the SQL inside has been simulated against the 47 rows. The loop is real; the correctness check on the backfill SQL is M1.
- **The seed catalog is the spec.** Concepts live in `src/seed.rs`. The four generated artifacts on disk are real, but the daemon does not re-ingest them as registry state. Production would slot in a real schema registry (Backstage, OpenMetadata, Confluent) at this point; the integration surface is the `ConceptCard` type.

Out of scope by design, with the rationale in `investigation/gpt-deep-research-report.md`:

- Full bitemporality (valid-time + transaction-time). Append-only mutation log is sufficient.
- Git-branch-merge over the registry (TerminusDB-style). Proposal JSON on disk is sufficient.
- Multi-region storage and locality routing. Single Postgres is the M0 substrate.
- gRPC and GraphQL service surfaces. HTTP + HTMX cover every demo read and write path.
- A descriptive catalog (Backstage-style explorer). The maud/HTMX explorer covers the same surface area; full catalog integration is M1.

---

## FAQ

**What stops someone from claiming `actor: team:integrations-platform`?**
Nothing at the demo's identity layer — the actor is stated, not authenticated. The FGA-style evaluator is real; what is missing is the proxy that turns a JWT into an actor string. Production wires that proxy in front of the same evaluator unchanged.

**What stops me from bypassing the daemon and writing directly to Postgres?**
Nothing — and that is exactly the threat model. `agora verify` catches it. An out-of-band `INSERT` shows up as `created_out_of_band` (no `mutation_log` row). An out-of-band `UPDATE` shows up as `tampered` (checksum mismatch with a field-level diff). The control plane does not pretend the database can prevent privileged shells; it makes every such mutation visible.

**Does the gate actually verify the backfill is correct?**
In offline mode, no — it trusts `backfill_plan` presence. The blocking, the structured rejection, the agent revision, and the re-check are all real; the final clearance is offline-loose by design. Production-grade would simulate the backfill in a rollback'd transaction and re-count violations. The Beat 6½ card hints at this with "row(s) flagged but mitigated by `backfill_plan` (Advisory)."

**What if the agent loop never converges?**
It is bounded — `MAX_ATTEMPTS = 3`. `FinalStatus::Stalled` is a first-class outcome; the UI renders every attempt card whether the run succeeded or stalled. Failure is part of the audit trail, not a hidden path.

**Can the LLM be replaced with a deterministic engine?**
Yes — the structured-output schema is the integration boundary. Anything that emits a valid `OntologyChangeProposal` works. The offline mode already proves this.

**How does Agora differ from Confluent Schema Registry?**
Schema-against-data, not schema-against-schema. Confluent says "the shapes are compatible." Agora says "this change would invalidate 47 existing rows." Different question, answered by a live SQL query.

**How does Agora differ from Backstage or DataHub?**
Those are descriptive catalogs — they describe at-rest data after the fact, updated by ingest jobs. Agora mediates the write path: proposals are gated before they land, writes are policy-checked before they commit, drift is caught after. The control plane is prescriptive, not retrospective.

**Why not XTDB or Datomic for bitemporality?**
Deferred. Append-only mutation log with `ontology_version` per row covers the audit story. Valid-time + transaction-time is a P1 spike. We prioritized the agentic + audit + verify properties on a substrate that boots in under five minutes on any laptop.

**Can this scale?**
Today: single Postgres, no sharding, no replicas. The control-plane code does not change for a substrate swap — the AST carries `locality`, the policy evaluator is per-request, the mutation log row shape is consumer-agnostic. M1 is a storage-driver change, not a rewrite.

**How does `agora verify` scale on a billion rows?**
Today it is a full table scan with canonical-JSON SHA-256 checksums. Production would compute checksums incrementally per-mutation, sample-verify on a cadence, and drift-only by partition. The structural property — every entity has a checksum trail in `mutation_log` — is what makes any of those optimizations available.

---

## Repo layout

```
src/
  bin/agorad.rs              — daemon entry point
  daemon.rs                  — JSON HTTP control plane
  ui.rs                      — maud + HTMX browser UI
  cli.rs                     — `agora` CLI subcommands
  llm.rs                     — live + offline proposal authoring
  reuse.rs                   — reuse-detection classifier
  check.rs, check_report.rs  — check orchestration
  axes/                      — the 8 risk axes (one file each)
  artifacts.rs               — codegen (proto / sql / handler / fga.json)
  auto_approval.rs           — verdict predicate (single source of truth)
  agent.rs                   — closed-loop revision
  entity_write.rs            — write path (mutation log + policy)
  verify.rs                  — drift detection
  explorer.rs                — ConceptView
  seed.rs                    — canonical concept catalog
migrations/                  — 001 init · 002 seed accounts · 003 checksums
                               · 004 policy denials · 005 customer domain
                               · 006 audit findings
fixtures/                    — pre-baked proposals (risky, additive, malicious,
                               customer tighten, audit-finding tighten)
DEMO.md                      — full beat-by-beat narrative
DEMO-RUNBOOK.md              — verbatim narration + falsifiability checks
OPS-PLAYBOOK.md              — failure-mode cards
HANDOFF.md                   — one-page orientation
STACK.md                     — locked stack decisions
investigation/               — original deep-research report
generated/                   — emitted artifacts (gitignored)
```

For the original thesis and full prior-art survey, see `investigation/gpt-deep-research-report.md`.
