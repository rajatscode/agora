# Agora

A governed operational ontology and control plane. One Rust binary, `agorad`, that sits between AI agents and a Postgres database and mediates every schema change and every write.

---

## What this is

`agorad` listens on `:3030` and does five things in sequence:

1. **PROPOSE.** An agent submits "we need to model X" in natural language. `agorad` calls Anthropic (or uses a deterministic offline path) and produces a typed `OntologyChangeProposal` plus four artifacts on disk — Protobuf schema, Postgres DDL, Rust HTTP handler skeleton, OpenFGA policy spec.
2. **CHECK.** The proposal flows through a 7-axis risk gate (composition, shape, semantic, policy, temporal, impact, replay) **plus a data-conformance axis that hits live Postgres**. A proposal to tighten `Account.email` to required is blocked with the exact count of violating rows (47), a sample of those rows, and the verbatim SQL that produced the count.
3. **AGENT LOOP.** A blocked proposal flows back to the LLM, which reads the structured rejection and revises — typically by adding a `migration.backfill_plan` that addresses the violation. The gate re-runs. The same agent code drives three different domains (bank integrations, customer 360, compliance findings) and produces three different domain-appropriate backfill strategies.
4. **WRITE + VERIFY.** Entity creation goes through `POST /entities/{type}`, which writes the row and an append-only `mutation_log` entry (SHA-256 checksum, actor identity, ontology version) in one transaction. An FGA policy can deny the write; denials are themselves logged as `DenyAttempt` rows. `agora verify` recomputes checksums and surfaces drift — including raw SQL `UPDATE`s that bypassed the daemon.
5. **EXPLORE.** `GET /concepts/{fqn}` returns owner team, fields, invariants, policy attachments, and the full version history from `mutation_log`.

All five flows live in one browser tab at `http://localhost:3030/`. The same library functions back the CLI (`agora propose | check | write | verify | explorer`) and the HTTP daemon — there is no separate "demo mode."

**The one-line claim.** Schema registries ask "are the bytes compatible?" Agora asks "does this change still tell the truth about the actual stored data?" — answered by a live SQL query, not a static type check, with a closed agent loop that revises blocked proposals until they are safe.

---

## The problem being solved

Three forces collide:

1. **Agentic development is fast and incoherent.** Each team and each agent invents its own data model, its own column names, its own meaning for the same concept. The result is duplicated meaning, silent semantic drift, and joins that produce wrong answers across product surfaces.
2. **The standard fixes are worse than the problem.** A central data team becomes a throughput bottleneck — every change waits behind every other change. Schemaless stores externalize coherence into tribal knowledge and post-hoc reconciliation. Neither scales to "thousands of agents touching the data layer."
3. **Compatibility checks at the schema layer are not enough.** "Adding a `NOT NULL` column is backward-compatible at the proto level" is true and useless if 47 production rows would violate the new constraint. The actual safety question is about the data, not the bytes.

The business requirement Agora targets: let many agents and humans propose meaning-changing data-layer changes in parallel, decide safely and automatically which ones can land, force the unsafe ones to either fix themselves or get escalated, and produce a tamper-evident audit trail of everything attempted — allowed, denied, or after-the-fact.

This is the layer above "the database" and below "the data catalog." Neither of those tools is shaped right for this job, as the next section details.

---

## Prior art and where it falls short

| Category | Examples | What it does | What it misses for this problem |
|---|---|---|---|
| **Schema registries** | Confluent Schema Registry, Apicurio, Buf Schema Registry | Validates that a new schema version is compatible (backward / forward / full) with the old one at the bytes/types level. | Schema-against-schema, not schema-against-data. Cannot detect that the change would break 47 existing rows. No notion of intent, ownership, or policy. |
| **Data catalogs** | Backstage, DataHub, Amundsen, Atlan, Collibra, OpenMetadata | Describes datasets, owners, lineage, tags, sometimes quality scores. Searchable view of "what data exists." | Descriptive and after-the-fact. Sits beside the database, not in front of it. Cannot gate a write or block a proposal. Updated by ingest jobs, not consulted on the write path. |
| **Policy engines** | Open Policy Agent (Rego), AWS Cedar, OpenFGA, SpiceDB | Evaluate authorization decisions ("can user X do action Y on resource Z?"). | Enforcement only. Do not author changes, do not reason about whether the change is semantically safe, do not produce artifacts. Necessary primitive, not the whole story. |
| **Build-time contracts** | dbt contracts, Great Expectations, Soda, Monte Carlo | Test data shape and quality at build time or on a schedule. | Fire after the change has landed (or in CI). Do not gate the proposal itself. Do not produce a structured rejection an agent can read and revise against. |
| **Migration tools** | Flyway, Liquibase, Alembic, sqlx migrate | Apply ordered DDL changes to a database, track which have run. | Mechanical execution. No semantic check, no data-conformance check, no propagation to APIs or policies. Migrations are opaque SQL strings. |
| **Bitemporal / versioned stores** | XTDB, Datomic, TerminusDB | Built-in valid-time and transaction-time. Time-travel queries. | Storage substrates, not control planes. No agent loop, no policy/artifact propagation. Strong for "what did the data look like at T" but silent on "should this change be allowed." |
| **Enterprise metadata / MDM** | IBM Watson Knowledge Catalog, Informatica, Alation | Centralized data governance: glossaries, stewardship workflows, lineage. | Built for human committees on multi-week cycles. The bottleneck Agora is trying to remove. |
| **Service-meaning catalogs** | Backstage software templates, ServiceNow CMDB | Describe services and their owners. | Software-topology focused. Do not reach into the data layer. |

The gap Agora claims is the one no existing tool fills: a runtime control plane that **(a)** treats every change as a typed semantic proposal, **(b)** checks the proposal against live data, not just types, **(c)** allows an agent to revise blocked proposals via a structured rejection contract, and **(d)** produces operational artifacts (DDL, proto, handler, policy) from the same source so the catalog view and the runtime registry cannot drift.

---

## What Agora does differently

Five concrete moves none of the above prior art combines:

1. **Schema-against-data, not schema-against-schema.** The `data_conformance` axis runs a live `SELECT COUNT(*) FROM …` against Postgres, not a type-system inference. The block message contains the actual count, sample violating rows, and the verbatim SQL.
2. **One declaration, four artifacts.** A single `OntologyChangeProposal` emits `.proto`, `.sql`, an HTTP handler skeleton, and an FGA policy spec. The catalog view (`GET /concepts/{fqn}`) reads from the same source — descriptive metadata and runtime registry cannot drift apart because they are not separate stores.
3. **The agent loop is closed.** When the gate blocks, the rejection is structured (axis, finding, sample rows, suggested mitigation hint). The agent ingests that JSON and emits a revised proposal — typically adding a `migration.backfill_plan` block that addresses the conformance violation. The gate re-runs. The loop is bounded (`MAX_ATTEMPTS = 3`) and `Stalled` is a first-class outcome.
4. **Every mutation produces an audit row, including denials.** Writes commit an entity-table insert and a `mutation_log` row in one transaction. Policy denials log a `DenyAttempt` row with the rejection reason. Out-of-band SQL `UPDATE`s are caught by `agora verify` (checksum mismatch with a field-level diff). The audit trail is one append-only table.
5. **The framework, not the pipeline.** The same `agent.rs`, `check.rs`, `verify.rs`, `explorer.rs` code runs three domains with disjoint owners, policy classes, and backfill strategies. The agent generalizes its mitigation per domain — `derive_from_user_record` for accounts, `synthetic_placeholder_from_id` for customers, `synthetic_accept_open_findings` for audit findings — from the same prompt template.

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

## Alternative architectures considered, and why they were not taken

These are the load-bearing decisions and the tradeoff each one makes.

**Language: Rust instead of Python or TypeScript.** Python is the well-trodden choice for anything LLM-adjacent; TypeScript is the well-trodden choice for anything web-adjacent. Both produce a deployable artifact that is "a directory of files plus a runtime"; Rust produces one binary. For a control plane that has to be trustworthy at the write path, "one binary, one connection pool, one process" is structurally simpler. Type safety on the AST and diff types catches whole classes of bugs at compile time that would be unit tests elsewhere. Cost: longer compile times, smaller ecosystem of AI SDKs.

**HTTP/JSON, not gRPC.** Agents and browsers both talk JSON natively. `tonic` would buy `.proto`-defined RPC at the cost of friction for the browser UI and curl-debuggability. Protobuf-encoded entity payloads are still accepted on ingest paths (decoded with `prost`), so the artifact emission story is honest — gRPC just is not the transport.

**JSON policy spec + in-Rust evaluator, not Rego/OPA.** OPA is the obvious choice for policy and is also a separate process with its own DSL and debugger story. The evaluator here is ~250 lines of Rust over five JSON policy files with a four-predicate DSL (`diff_kind`, `classification_delta_direction`, `affects_field_with_class`, `type_level_field_changed`). Swapping to OPA later is one engineer-week — the JSON specs round-trip to Rego. The structural claim — "policy is a first-class axis of the check report with cited rule IDs" — survives the swap.

**Append-only Postgres `mutation_log`, not Kafka.** The audit trail's structural requirements are append-only, monotonic sequence, and consumable by drift detection. Postgres satisfies all three with one fewer system to operate. Kafka would buy downstream fan-out that M0 does not need. The row schema is shaped so a future producer-side swap is mechanical.

**Single-node Postgres, not Spanner / CockroachDB / multi-region.** The locality field on `OntologyType` exists in the AST and propagates through artifacts, but the M0 substrate is one Postgres instance. The architectural piece is the control plane; the substrate is replaceable, and the AST already carries the metadata a multi-region substrate would consume. M1 is a storage-driver swap, not a control-plane rewrite.

**Append-only `mutation_log` with checksums, not full bitemporality.** XTDB or Datomic would provide valid-time + transaction-time natively. The mutation log provides transaction-time (when did the daemon see this write) and an `ontology_version` snapshot per row. Valid-time is deferred — the audit story does not require it, and treating bitemporality as a sidequest preserved time for the agentic loop instead.

**maud + HTMX, not React or Next.** The browser UI renders server-state-shaped views (a check report, a verify panel, a concept page) — HTMX swaps fragments into slots without a build step. No Node, no bundler, no client framework. The HTMX bytes are inlined into the binary so the UI works without an internet connection.

**Seed catalog as the source of truth, not registry-bootstrap-from-artifacts.** The four generated artifacts (`.proto`, `.sql`, handler, `.fga.json`) are real files written to `generated/{proposal_id}/`, but the canonical concept registry comes from `src/seed.rs`. The round-trip from emitted artifacts back into the registry is not built — the artifact-emit path is one-way for M0. The thesis (one declaration, many operational artifacts) still lands; the symmetric ingest is M1.

**No GraphQL surface.** The browser UI is HTMX; the agent surface is JSON-over-HTTP. Nothing benefits from a GraphQL gateway at this scope.

---

## Running locally

Prerequisites: Postgres 14+, Rust toolchain.

```bash
# Postgres up (macOS / Homebrew)
brew services start postgresql@14

# Database
createdb agora_dev

# Boot the daemon (runs migrations on startup)
DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad

# Open the UI
open http://localhost:3030
```

Optional: `ANTHROPIC_API_KEY` exported before booting enables live LLM authoring. Without it, the propose flow and the revise step of the agent loop use a deterministic offline path; the author-mode pill on every proposal card reports which mode is in effect. Offline mode is the validated default — every flow works end-to-end without an API key. Live mode is the same architecture with slightly different latency and a non-zero API-error risk; the author-mode pill flips to orange `offline · API error` and the offline fallback fires automatically on any error.

**Critical operational rule.** After any `cargo build` or `git pull`, the daemon must be restarted. The process caches seed concepts and migrations at boot — a running daemon will not pick up code, migration, or seed changes. This bit the build four times; the recovery procedure is in the failure-modes section below.

```bash
# Restore demo baselines (47 NULL accounts.email, 5 NULL customers.email,
# 4 NULL audit_findings.resolved_at, etc.)
curl -X POST localhost:3030/admin/reset
```

Health check:

```bash
curl -s localhost:3030/health
# expected: {"status":"ok","db":"connected"}
```

---

## The walk-through

The home page at `/` renders every flow as its own card, top to bottom. Each step below names the action, what shows up, what it proves, and the falsifiability check — the thing that would defeat the proof if it could be faked.

### Setup

Open `http://localhost:3030/`. The page should show nine section headers (`01/02`, `03`, `05`, `06`, `06½`, `07`, `08`) plus an intro card titled "Eight beats. One control plane. Real data." Devtools console should be clean. If anything looks stale, `curl -X POST localhost:3030/admin/reset` restores baselines.

### Beat 1+2 — Propose + Reuse Detection

**Action.** The Propose form's default prompt (`we need to model what each bank integration can do — supported features, rate limits, etc.`) is submitted via the `Propose →` button.

**On screen.** A proposal card with `prop_xxxx…`, target `draft.model.What`, an author-mode pill (`live · LLM-derived` or `offline · no API key`), a reuse-detection table with three candidates (best `core.integrations.BankIntegration` at ~0.08, classified `New`), and an artifacts strip with four tabs: `.proto` (active by default), `.sql`, `_handler.rs`, `.fga.json`.

**What it proves.** The unit of change in Agora is a semantic proposal, not a raw DDL diff — and it can be authored by an agent. The Anthropic structured-output call is the realism marker. Reuse detection runs in the same pass; identical concepts are caught before duplication.

**Falsifiability.** The proposal must round-trip through `llm::author_proposal` and `artifacts::emit_all` — the same code path the CLI uses. The author-mode pill must reflect actual API key state. The four artifact tabs must contain distinct content sized roughly 300–1500 bytes each.

### Beat 4 — Generated Artifacts (within the proposal card)

**Action.** Clicking each tab in the artifact strip cycles `.proto` → `.sql` → `_handler.rs` → `.fga.json`.

**On screen.** `.proto` shows a typed schema. `.sql` shows DDL with the proposal_id in the comment. `_handler.rs` shows an HTTP handler skeleton. `.fga.json` shows a policy spec referencing the new concept FQN.

**What it proves.** One proposal compiles to four operational artifacts from the same source.

**Falsifiability.** Each artifact must be a real file on disk at `generated/{proposal_id}/`. Beat 4 has no standalone section on the home page — the artifact strip inside the proposal card is the entire Beat 4.

### Beat 3 — Multi-axis check

**Action.** `Run multi-axis check →` on the proposal card.

**On screen.** A banner reading `All axes clean — auto-approval eligible.` Below it, an 8-row table: composition / shape / semantic / policy / temporal / impact / replay / data_conformance — all `pass`. Six rows show `source = deterministic`. The `semantic` row shows `offline-fallback (no-api-key)` when no API key is set. The `data_conformance` row shows `not-applicable` for a brand-new concept that has no live table yet.

**What it proves.** Compatibility is multi-axis, not just shape-compatible vs. shape-breaking. Meaning, policy, and the actual state of the world need validation alongside types.

**Falsifiability.** The semantic axis's source must honestly report `offline-fallback` rather than masquerading as an LLM call. The other axes are deterministic and run against the proposal's diff.

### Beat 5 — Auto-approval

**Action.** `Submit for approval →`.

**On screen.** Green `Auto-approved` card with `status: approved`, `predicate: auto_approval::apply ⇒ all_axes_clean=true`, real `generated_at` timestamp.

**What it proves.** Every axis clean, so the predicate fires. The same predicate is what any caller — CLI, HTTP, or the agent loop — receives. No human in the loop because none was needed.

**Falsifiability.** The verdict is the output of `auto_approval::apply`. The CLI and the agent loop both consume it; divergence would surface as a test failure across the binary.

### Beat 6 — Risky proposal blocked by real data

**Action.** `Run risky proposal →` (separate section).

**On screen.** Red `Blocked` card with `data_conformance: 47 existing row(s) violate the proposed constraint`. The 8-axis table shows `data_conformance = fail`. A "Sample violations" table lists `acct_null_001 … acct_null_005`. The verbatim SQL string `SELECT COUNT(*)::BIGINT AS n FROM accounts WHERE email IS NULL` is rendered on the card.

**What it proves.** A proposal that tightens `Account.email` from optional to required is blocked because forty-seven rows in the live `accounts` table would violate. The block is grounded in real data, not a hypothetical lint.

**Falsifiability.** `psql postgres://localhost/agora_dev -c "SELECT COUNT(*) FILTER (WHERE email IS NULL) FROM accounts;"` returns `47`. Inserting or deleting a NULL-email row and re-running the check updates the count in lockstep.

### Beat 6½ — Agent loop (closed-loop revision)

**Action.** The textarea pre-filled with `tighten Account.email to required for compliance` is submitted via `Run agent loop →`.

**On screen.** A top green banner reading `Approved after 2 attempt(s).` An amber card for Attempt 1 — `authored from prompt`, pills `offline · no API key` + `blocked`, block reason `data_conformance: 47 existing row(s) violate the proposed constraint`. A green card for Attempt 2 — `revised — added migration.backfill_plan (derive_from_user_record)`, pills `offline · no API key` + `approved`. The revised proposal renders `strategy=derive_from_user_record`, `source=users.email WHERE users.account_id = accounts.id, else '<unknown>@placeholder.invalid'`, `idempotent=true`, and the full SQL `UPDATE accounts a SET email = COALESCE((SELECT u.email FROM users u WHERE u.account_id = a.id), '<unknown>@placeholder.invalid') WHERE a.email IS NULL`. Hint line: `postgres (mitigated: backfill_plan present) · 47 row(s) flagged but mitigated by backfill_plan (Advisory)`.

**What it proves.** The same proposal that just blocked is read back by the agent as a structured rejection. The agent writes a backfill plan into the same proposal — not a different proposal — and re-submits. The data-conformance axis flips from `fail` to `advisory` because the engine recognizes the mitigation. That is the closed loop.

**Falsifiability.** Each attempt is a real call into `agent::agent_loop`. The bound `MAX_ATTEMPTS = 3` is a constant in `src/agent.rs`. `FinalStatus::Stalled` is a first-class outcome surfaced as its own banner if all three attempts block.

### Beat 7a — Write allow (policy-enforced)

**Action.** With the actor dropdown on `team:integrations-platform`, `Write a BankIntegration →`.

**On screen.** Green `Allowed by policy → write committed.` KV: `policy decision: allow`, `actor: team:integrations-platform`, `relation: owner`, `object: bank_integration:bi_demo_xxx`. Then a "Write committed" card with `entity_id: bi_demo_xxxxxxxxxx`, `mutation_seq` (increments per write), `ontology_version: 2`, full SHA-256 checksum.

**What it proves.** The actor flows into the policy engine, the `owner` relation is checked against the seeded ownership wildcard, and the write commits inside a single transaction alongside the `mutation_log` entry.

**Falsifiability.** A `SELECT * FROM mutation_log ORDER BY seq DESC LIMIT 1` shows the same `mutation_seq` and checksum the UI rendered. The actor is asserted, not authenticated — this is a property of the demo's identity layer, not the policy evaluator.

### Beat 7a — Write deny

**Action.** Same form, actor dropdown switched to `team:marketing`, `Write a BankIntegration →`.

**On screen.** Red `Policy denied → write refused.` Reason: `` `owner` requires team:integrations-platform; got `team:marketing` on `bank_integration:bi_demo_xxx` ``. KV: `policy decision: deny`, `actor: team:marketing`, `relation: owner`, `object: bank_integration:bi_demo_xxx`, `denial logged at seq N (operation = DenyAttempt, denial_reason persisted)`. A "TUPLES CONSIDERED" table with one row: `object: bank_integration:*`, `relation: owner`, `user: team:integrations-platform`, `outcome: user mismatch`.

**What it proves.** Wrong owner produces a 403, but the rejection is not a black hole — it is logged as a peer of writes in the same `mutation_log`, with the denial reason persisted. The next verify call will not flag this as drift because no entity row exists for a deny.

**Falsifiability.** `SELECT operation, denial_reason FROM mutation_log ORDER BY seq DESC LIMIT 1` returns the `DenyAttempt` row with the verbatim reason.

### Beat 7b — Tamper out-of-band

**Action.** Actor dropdown back to `team:integrations-platform`, `Write a BankIntegration →` once more to land a fresh row, then `Tamper this row out-of-band →`.

**On screen.** Amber `Out-of-band UPDATE issued.` The raw SQL is rendered: `UPDATE bank_integrations SET provider = 'evil_corp_tampered' WHERE id = 'bi_demo_xxx';`. Body text explains that the `mutation_log` was not updated and the control plane no longer agrees with the database.

**What it proves.** Raw SQL outside the daemon is the realistic threat model. The tamper button simulates that by issuing the UPDATE itself, then catches itself in the next step. This is an honesty point: the demo simulates the out-of-band write rather than pretending it came from a separate session.

**Falsifiability.** `SELECT provider FROM bank_integrations WHERE id = 'bi_demo_xxx'` returns `evil_corp_tampered`. `SELECT * FROM mutation_log WHERE entity_id = 'bi_demo_xxx'` does not include the tamper.

### Beat 7c — Verify (drift detection)

**Action.** `Run agora verify →`.

**On screen.** Red `Drift detected.` with summary `N tampered row(s) and M out-of-band row(s) across X entities.` A "TAMPERED ROWS" section lists the entity just tampered with `logged checksum: <hex>` ≠ `current checksum: <hex>` and a field-level diff table: `provider | plaid | evil_corp_tampered`. A "CREATED OUT-OF-BAND" section lists pre-existing seed rows that pre-date the mutation_log (`acct_null_001…047`, `cust_001…cust_020`, `af_001…af_015`, etc.).

**What it proves.** `agora verify` recomputes canonical-JSON checksums from the live row, compares to the logged checksum, and flags the specific field that changed — not "something is wrong" but `provider: plaid → evil_corp_tampered`. The 7a-deny entity is not in this list because no entity row exists for a deny.

**Falsifiability.** The classification is reproducible: rolling back the tamper UPDATE and re-running verify removes the row from the tampered list. The out-of-band list matches the count of rows inserted by SQL migrations that pre-date the mutation_log (47 + 20 + 15 = 82, plus whatever has accumulated).

### Beat 8 — Explorer (three domains)

**Action.** Clicking `core.integrations.BankIntegration` in the concept list opens `/ui/concepts/core.integrations.BankIntegration`. The same flow on `core.customer.Customer` and `core.compliance.AuditFinding` proves generalization.

**On screen, BankIntegration.** `namespace: core.integrations`, `name: BankIntegration`, `version: 3 (Active (v3))`, `owner: integrations-platform · semantic steward: core-ontology`, `policy class: Internal`, `HTTP route: POST /entities/bank_integration`, `storage table: core_integrations_bank_integration`, `proto: bank_integration.proto`, `policy spec: bank_integration.fga.json`. Fields table with `id`, `provider`. Invariants: `Every active BankIntegration has at least one supported AuthenticationMethod`. Policy attachments: `owner / team:integrations-platform / bank_integration:{id}`, `internal_viewer / team:* / bank_integration:{id}`. Version history populated from `mutation_log`.

**On screen, Customer.** Same UI, different domain. `namespace: core.customer`, `owner: customer-platform` (different team), `policy class: Pii` (different classification). Four fields including `email (Pii)` and `display_name (Pii)`.

**On screen, AuditFinding.** Third domain. `namespace: core.compliance`, `owner: compliance-platform`, `policy class: Pii`. Seven fields, one invariant: `status = 'resolved' requires resolved_at to be non-null`. Policy attachments include `pii_viewer / role:dpo / audit_finding:{id}` — the policy graph composes with role-based access, not just team membership.

**What it proves.** Owner, steward, policy class, every field's classification, invariants, the storage table, the policy artifact, and the live version history with checksums — all rendered for three semantically distinct domains by the same code paths. The 56 out-of-band rows surfaced in Beat 7c include the 47 NULL-email Accounts from Beat 6 — same row sets, two lenses.

**Falsifiability.** `grep -r "Customer\|BankIntegration\|AuditFinding" src/agent.rs src/check.rs src/verify.rs src/explorer.rs` returns zero matches. The framework code has no domain-specific branches.

### The three-strategy callout

The agent loop run on all three domains produces three different backfill strategies, byte-for-byte verifiable via `curl`:

- **`Account.email` → `derive_from_user_record`.** The agent inspects the schema, sees a related `users` table joined by `account_id`, and writes a `COALESCE` lookup with a placeholder fallback.
- **`Customer.email` → `synthetic_placeholder_from_id`.** No upstream join target. The agent generates `UPDATE customers SET email = lower(id) || '@placeholder.invalid' WHERE email IS NULL`.
- **`AuditFinding.resolved_at` → `synthetic_accept_open_findings`.** Compliance-aware. Not a blanket `SET resolved_at = now()` — also promotes `status` from `open` / `investigating` to `accepted_risk`, because that is the GRC-correct state for a finding being mass-closed by migration.

Same agent code. Three domain-shaped strategies. Reproducible:

```bash
curl -X POST -d '{"prompt":"tighten Account.email to required for compliance"}' \
  http://localhost:3030/agent/run | jq '.attempts[1].proposal.migration'
```

---

## Honest scope and limitations

Three M0 limitations are surfaced in-UI on the relevant cards and reproduced here:

- **Actor is asserted, not authenticated.** The actor dropdown supplies the policy subject. The FGA-style evaluator (wildcards, considered-tuple introspection, deny logging) is real. The identity-issuance step is not — in production, an auth proxy or JWT decoder hands the actor to the same evaluator unchanged.
- **Offline mode trusts `backfill_plan` presence, not correctness.** When `ANTHROPIC_API_KEY` is unset, the revision step emits a structurally valid `migration.backfill_plan`. The gate downgrades `data_conformance` from `fail` to `advisory` because the plan exists, not because the SQL has been simulated against the 47 rows. The loop is real; the correctness check on the backfill SQL is M1.
- **The seed catalog is the spec.** Concepts live in `src/seed.rs`. The four generated artifacts on disk are real, but the daemon does not re-ingest them as registry state. Production would slot in a real schema registry (Backstage, OpenMetadata, Confluent) at this point; the integration surface is the `ConceptCard` type.

Deferred by design (rationale in `investigation/gpt-deep-research-report.md`):

- Full bitemporality (valid-time + transaction-time).
- Git-branch-merge over the registry (TerminusDB-style).
- Multi-region storage and locality routing.
- gRPC and GraphQL service surfaces.
- A descriptive catalog as a separate system (Backstage-style explorer).

---

## Failure modes and recovery

The system is designed to surface degraded modes rather than hide them. Every fallback below corresponds to a UI signal Agora already gives — an orange author-mode pill, a `Skipped` axis, a `Stalled` banner, an entry in `tampered_entities[]`.

### 1. Daemon crashed

Symptoms: blank page, browser shows connection refused, every HTMX button is inert.
Diagnosis: `ps aux | grep '[a]gorad'` returns no row, or `curl -s localhost:3030/health` errors.
Recovery: `DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad` in a fresh terminal, then refresh the browser. Postgres state is intact; the daemon is stateless.

### 2. Postgres connection died

Symptoms: `/health` returns `{"db":"disconnected"}`; the `data_conformance` axis returns `Skipped`; writes 503.
Diagnosis: `brew services list | grep postgresql` shows stopped/error.
Recovery: `brew services restart postgresql@14`, then restart the daemon to re-establish the pool.

### 3. Browser tab stuck / spinner forever

Symptoms: HTMX button disabled, slot never updates.
Diagnosis: Devtools network tab shows the request hung or cancelled.
Recovery: Refresh the tab. HTMX is server-rendered; every prior beat re-renders from disk plus DB.

### 4. Anthropic rate-limited or unreachable

Symptoms: Author-mode pill renders orange `offline · API error` (or `offline · no API key` if the env var is unset).
Diagnosis: The pill is the diagnosis. The system surfaced it.
Recovery: None required — offline mode is the fallback by design. The deterministic offline author emits a structurally valid proposal; the gate does not know the difference.

### 5. Beat 6 shows a count other than 47

Symptoms: Blocked card says `46 existing row(s)` or `48 existing row(s)`.
Diagnosis: A prior run modified the `accounts` table — usually a successful Beat 6½ revision that backfilled rows, or a stray INSERT/DELETE in psql.
Recovery (preferred): `curl -X POST localhost:3030/admin/reset`. Or in psql: `\i migrations/002_seed_accounts.sql`.

### 6. Artifact tab strip frozen

Symptoms: Clicking `.sql` / `_handler.rs` / `.fga.json` does not change the panel below `.proto`.
Diagnosis: Browser devtools console shows `TypeError: Cannot read properties of null (reading 'addEventListener')`. The tab-switch script attached to `document.body` before `<body>` was parsed.
Recovery: Confirm the daemon is built from a commit at or after `3cf3eb6` (`fix(F4): tab handler must delegate off document, not document.body`), then restart. Fallback: `cat generated/{proposal_id}/{*.proto,*.sql,*_handler.rs,*.fga.json}` in a side-car terminal.

### 7. Agent loop stalls (three attempts, all blocked)

Symptoms: Three amber attempt cards, banner reads `Stalled after 3 attempt(s)`.
Diagnosis: Offline-heuristic revision did not recognize the field name → could not emit a backfill plan. Usually triggered by a non-canonical prompt.
Recovery: Re-run with the canonical prompt verbatim: `tighten Account.email to required for compliance` (or the Customer / AuditFinding variant). `Stalled` is the honest outcome — every attempt is on the record.

### 8. Policy deny does not trigger (`team:marketing` write succeeds)

Symptoms: 7a-deny path returns 200 + "Write committed" instead of 403.
Diagnosis: Daemon is serving a pre-F5 binary — the restart bug below.
Recovery: Apply the restart procedure from #9.

### 9. Daemon serving a stale binary (the restart bug — caught four times in this build)

Symptoms: Behavior does not match the latest commit. New endpoints 404. New seed concepts do not appear. New axes do not fire.
Diagnosis: `ps -o pid,lstart,command -p $(lsof -ti:3030)` and `git log -1 --format=%cI`. If the process start time is before the last commit, the binary is stale.
Recovery: `kill $(lsof -ti:3030) && cargo build --bin agorad && DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad`.

---

## FAQ

**What stops someone from claiming `actor: team:integrations-platform`?**
Nothing at the demo's identity layer — the actor is stated, not authenticated. The FGA-style evaluator is real; the proxy that turns a JWT into an actor string is what is missing. Production wires that proxy in front of the same evaluator unchanged.

**What stops a process from bypassing the daemon and writing directly to Postgres?**
Nothing — and that is exactly the threat model. `agora verify` catches it. An out-of-band `INSERT` shows up as `created_out_of_band` (no `mutation_log` row). An out-of-band `UPDATE` shows up as `tampered` (checksum mismatch with a field-level diff). The control plane does not pretend the database can prevent privileged shells; it makes every such mutation visible.

**Does the gate actually verify the backfill is correct?**
In offline mode, no — it trusts `backfill_plan` presence. The blocking, the structured rejection, the agent revision, and the re-check are real; the final clearance is offline-loose by design. Production-grade would simulate the backfill in a rollback'd transaction and re-count violations. The Beat 6½ card hints at this with "row(s) flagged but mitigated by `backfill_plan` (Advisory)."

**What if the agent loop never converges?**
It is bounded — `MAX_ATTEMPTS = 3`. `FinalStatus::Stalled` is a first-class outcome; the UI renders every attempt card whether the run succeeded or stalled. Failure is part of the audit trail, not a hidden path.

**Can the LLM be replaced with a deterministic engine?**
Yes — the structured-output schema is the integration boundary. Anything that emits a valid `OntologyChangeProposal` works. The offline mode already proves this.

**How does Agora differ from Confluent Schema Registry?**
Schema-against-data, not schema-against-schema. Confluent says "the shapes are compatible." Agora says "this change would invalidate 47 existing rows." Different question, answered by a live SQL query.

**How does Agora differ from Backstage or DataHub?**
Those are descriptive catalogs — they describe at-rest data after the fact, updated by ingest jobs. Agora mediates the write path: proposals are gated before they land, writes are policy-checked before they commit, drift is caught after. The control plane is prescriptive, not retrospective.

**Why not XTDB or Datomic for bitemporality?**
Deferred. Append-only mutation log with `ontology_version` per row covers the audit story. Valid-time + transaction-time is a P1 spike. The agentic + audit + verify properties were prioritized first.

**Can this scale?**
Single Postgres today, no sharding, no replicas. The control-plane code does not change for a substrate swap — the AST carries `locality`, the policy evaluator is per-request, the mutation log row shape is consumer-agnostic. M1 is a storage-driver change, not a rewrite.

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
STACK.md                     — locked stack decisions
investigation/               — original deep-research report
generated/                   — emitted artifacts (gitignored)
```
