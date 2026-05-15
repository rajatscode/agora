# Agora — Governed Operational Ontology Control Plane

A single binary that lets agents safely propose, generate, evolve, and audit shared business concepts — without forking reality.

Agora is a **governed operational ontology and control plane**. It sits above a serious storage substrate (Postgres for M0) and gives both offline data and live operational data a shared, evolvable structure that agents and humans can build against — without a committee bottleneck and without schema-free sprawl.

The full thesis lives in `investigation/gpt-deep-research-report.md`. The script the demo follows beat-by-beat is `DEMO.md`. The stack rationale is `STACK.md`.

---

## The five critical proofs

1. **Agents can propose** new concepts with semantic intent — not just shape diffs.
2. **The system detects reuse vs. duplication vs. refinement** — first-class, not aspirational.
3. **Artifacts are generated** from a proposal — DDL, `.proto`, HTTP handler, policy spec, all on disk and consumable.
4. **Safe changes auto-approve; risky changes are blocked, and the agent revises into approval** — the closed loop.
5. **Objects are discoverable, auditable, replayable, and permissioned** — including drift detection for anything that bypasses the control plane.

---

## Architecture (text layout)

```
┌────────────────────────────────────────────────────────────────────────┐
│  F4  Browser UI — maud + HTMX, served at http://localhost:3030/        │
│      All 8+1 beats live in one tab. No reloads, no demo-mode toggle.   │
└─────────────────────────────────┬──────────────────────────────────────┘
                                  │ /ui/* HTMX endpoints
┌─────────────────────────────────▼──────────────────────────────────────┐
│  F-DAEMON  agorad — Axum HTTP daemon on :3030                          │
│      JSON control plane: POST /proposals · POST /proposals/{id}/check  │
│      POST /entities/{type} · GET /verify · GET /concepts/{fqn}         │
│      POST /agent/run                                                   │
│      Wraps the same library functions the `agora` CLI uses.            │
└─────────────────────────────────┬──────────────────────────────────────┘
                                  │ library calls
┌─────────────────────────────────▼──────────────────────────────────────┐
│  F1  Propose       llm::author_proposal       (live Anthropic LLM)     │
│  F2  Check         check::check               (7-axis risk gate)       │
│  F3  Write/Verify  entity_write + verify      (mutation log + drift)   │
│  F5  Policy        policy::evaluate           (allow/deny + DenyAttempt│
│                                                logged)                 │
│  F6  Agent loop    agent::agent_loop          (closed-loop revision)   │
│  F8  Second domain Customer / LoyaltyTier     (proves generalisation)  │
│  Explorer          explorer::explorer         (owner/lineage/history)  │
└─────────────────────────────────┬──────────────────────────────────────┘
                                  │ sqlx
┌─────────────────────────────────▼──────────────────────────────────────┐
│  Postgres  bank_integrations · accounts · customers · loyalty_tiers    │
│            mutation_log (append-only, SHA-256 checksums)               │
│            policy_denials                                              │
└────────────────────────────────────────────────────────────────────────┘
```

CLI front door (`agora propose | check | write | verify | explorer`) and the JSON daemon front door call the **same library functions** — there is no business logic in the handlers.

---

## How to run (< 5 minutes from clone)

**Prerequisites:** Postgres 14+, Rust toolchain.

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

Optional: export `ANTHROPIC_API_KEY` for live LLM authoring/revision. Without it, F1 and F6 fall back to a deterministic offline path; the author-mode pill on every proposal card reports which mode is in effect.

> **Critical operational rule:** **After each merge or rebuild, restart the daemon.** `agorad` caches the seed concept catalog and migrations at boot; a running daemon will not pick up code changes, new migrations, or new seed concepts. Kill it (`Ctrl-C` or `kill $(lsof -ti:3030)`) and start it again. This is the issue that has bitten us four times in development — capturing it here so the runbook holds.

---

## How to demo (the 8+1 beats, in order)

The home page at `/` lays out every beat as its own card. Run them top-to-bottom. For each beat: click the button on the left, watch the slot on the right, narrate what you see.

| Beat | Click | Look for |
|---|---|---|
| **1+2** Propose | `Propose →` (default prompt is fine) | Proposal card with author-mode pill, four artifact tabs (.proto / .sql / handler / .fga.json), and a **Reuse detection** subsection showing `AuthenticationMethod` as a top hit. |
| **3** Multi-axis check | `Run multi-axis check →` (button appears on the proposal card) | 8-row table: composition · shape · semantic · policy · temporal · impact · replay · data_conformance. The hint line below shows `data_conformance source = …` and a live count — that is your falsifiability marker. |
| **5** Auto-approval | `Submit for approval →` | Green "Auto-approved" card. Note the `predicate ⇒ all_axes_clean=true` line — the verdict is the same predicate the CLI sees. |
| **6** Risky proposal | `Run risky proposal →` (Beat 06 section) | Red "Blocked" card. Data-conformance reports **47** existing `Account.email IS NULL` rows. The verbatim SQL query that produced the count is shown — judges can replicate in psql. |
| **6½** Agent loop | `Run agent loop →` (Beat 06½ section) | Stack of attempt cards. Attempt 1 = `Authored` → blocked. Attempt 2 = `Revised — added migration.backfill_plan` → approved. Banner above: `Approved after 2 attempt(s)`. |
| **7a** Write | `Write a BankIntegration →` (actor defaults to `team:integrations-platform` — owner, allow) | Green "Write committed" card with `entity_id`, `mutation_seq`, and SHA-256 `checksum`. Switching the actor dropdown to `team:marketing` and clicking again shows the **deny** path: a red panel + a `DenyAttempt` row logged in `mutation_log`. |
| **7b** Tamper | `Tamper this row out-of-band →` (button on the write card) | Amber card showing the raw SQL `UPDATE bank_integrations SET provider = 'evil_corp_tampered' WHERE id = …` that bypassed the handler. |
| **7c** Verify | `Run agora verify →` | Red "Drift detected" panel. **Point at the `Tampered rows` section** — that's your live drift, with a field-level diff (`provider: plaid → evil_corp_tampered`). Below it, `Created out-of-band` lists 56 pre-existing rows the control plane never saw — *honest noise we want visible*, sets up Beat 8. |
| **8** Explorer | Click `core.users.Account` in the Beat 08 section | Full ConceptView: namespace, owner, fields, invariants, lineage, policy attachments, version history from `mutation_log`. The 56 out-of-band rows from Beat 7 **include the 47 NULL-email Accounts from Beat 6** — same row sets, two lenses. |

Time budget: ~6m 15s. See `DEMO.md` for the verbatim narration and the cannot-be-faked check per beat.

---

## What's in scope (M0) and known limitations

We deliberately scoped narrow so every demoed claim is honest. The three honesty notes are also called out inline on the relevant UI cards:

- **Actor is asserted, not authenticated.** F5's allow/deny path evaluates the policy on the actor string the caller passes. There is no auth handshake — the actor is the demo's stand-in for an authenticated principal. The policy *evaluator* is real; the *binding from request to identity* is not.
- **Offline mode trusts `backfill_plan` presence, not correctness.** In Beat 6½, when `ANTHROPIC_API_KEY` is unset, the revision step is a deterministic heuristic that adds a `migration.backfill_plan` block. The gate downgrades `data_conformance` from `fail` to `advisory` because the plan *exists* — not because the SQL inside it has been validated against the 47 rows. The card hints at this with "row(s) flagged but mitigated by backfill_plan (Advisory)". The *loop is real*; the *backfill correctness check* is out of scope.
- **Concept specs come from the seed catalog, not on-disk artifacts.** The four generated artifacts (`.proto`, `.sql`, handler, `.fga.json`) are real files written under `generated/{proposal_id}/`. But the seed catalog (`src/seed.rs`) is the source of truth for what concepts exist in the demo registry — we do not re-ingest the artifacts back into the catalog. The artifact-emit path is one-way for M0.

What is **not** scoped for the hackathon, by design, with the rationale in the investigation report:

- Full bitemporality (XTDB-style valid-time + transaction-time). Append-only mutation log is sufficient for Beat 7.
- TerminusDB / Git-branch-merge over the registry. Proposal JSON on disk is sufficient.
- Multi-region storage. Single-node Postgres is sufficient for M0.
- GraphQL or gRPC service surfaces. HTTP handler is the command surface.
- Backstage-style explorer UI. The maud + HTMX explorer covers Beat 8.

---

## Repo layout

```
src/
  bin/agorad.rs              — daemon entry point
  daemon.rs                  — JSON HTTP control plane
  ui.rs                      — maud + HTMX browser UI (F4)
  cli.rs                     — `agora` CLI subcommands
  llm.rs                     — F1: live + offline proposal authoring
  reuse.rs                   — Beat 2 reuse classifier
  check.rs, check_report.rs  — F2 check orchestration
  axes/                      — the 8 risk axes (one file each)
  artifacts.rs               — F4 codegen (proto / sql / handler / fga.json)
  auto_approval.rs           — F5 verdict predicate (single source of truth)
  agent.rs                   — F6 closed-loop revision
  entity_write.rs            — F3 + F5 write path (mutation log + policy)
  verify.rs                  — F3 drift detection
  explorer.rs                — Beat 8 ConceptView
  seed.rs                    — canonical concept catalog (BankIntegration,
                               AuthenticationMethod, Account, Customer,
                               LoyaltyTier, ...)
migrations/                  — 001 init · 002 seed accounts · 003 checksums
                               · 004 policy denials · 005 customer domain
fixtures/                    — pre-baked proposals (beat 6 risky, happy add,
                               malicious, customer tighten)
DEMO.md                      — the demo script (v2.0, north star)
STACK.md                     — the stack and decision log
investigation/               — the original deep-research report
generated/                   — F4 artifact output (gitignored)
```

---

## Status

- **Built features (8):** F1 (propose), F2 (check), F3 (write/verify/explorer), F4 (browser UI), F-DAEMON (HTTP control plane), F5 (policy enforcement on writes + DenyAttempt audit), F6 (agent loop), F8 (Customer 360 second domain).
- **Tests:** 78 passing (`cargo test`).
- **Build:** single `cargo build --bin agorad` produces the entire demo binary.
- **Deadline:** 2026-05-15, 8:00 ET.

For the demo narrative beat-by-beat, see `DEMO.md`. For the stack rationale and decision log, see `STACK.md`. For the original problem framing, see `investigation/gpt-deep-research-report.md`.
