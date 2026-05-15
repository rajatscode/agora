# Agora — Pitch & Judge FAQ

> One-page pitch for judges, plus the questions we expect and the honest answers we have ready.

---

## The pitch

**The problem.** Agentic product development is fast and incoherent. Every team — every *agent* — invents its own data model, its own tables, its own column names for the same concepts. The result is duplicated meaning, silent semantic drift, and unreliable cross-product joins. The standard responses are worse than the problem: a central team that becomes a throughput bottleneck, or schemaless stores that externalize semantic coherence into tribal knowledge and post-hoc cleanup.

**What Agora is.** A **governed operational ontology and control plane** that sits above a serious storage substrate. Agora doesn't replace the database; it *compiles* ontology changes into storage schema, contracts, APIs, policy bindings, and explorer metadata. Agents propose changes as semantic artifacts (not DDL diffs). The control plane gates every change against a multi-axis check report. Safe changes auto-approve. Risky changes block with a structured rationale the agent can read and revise against. Writes flow through generated commands. Out-of-band tampering is caught field-by-field.

**The five proofs the demo lands.** (1) Agents can author semantic proposals (live Anthropic structured output). (2) The system detects reuse vs. duplication vs. refinement against the seed catalog. (3) Four real operational artifacts (`.proto`, `.sql`, HTTP handler, `.fga.json`) are emitted from one semantic change. (4) Safe changes auto-approve; risky changes block; *the agent revises into approval — the closed loop*. (5) Objects are discoverable, auditable, replayable, and permissioned — and the rows Agora never saw are visible too.

**The killer demonstrations.**
- **Beat 6 + 6½ + 7c, together, prove the system thinks.** Beat 6: the gate blocks a "tighten `Account.email` to required" proposal because 47 existing rows have `email = NULL` — block grounded in *real data*, not a hypothetical lint. Beat 6½: the agent reads the structured rejection, revises the *same* proposal with `migration.backfill_plan`, the gate re-runs, `data_conformance` downgrades `fail → advisory`, and the proposal auto-approves. Beat 7c: a write commits, a raw SQL `UPDATE` bypasses the handler, and `agora verify` catches the drift field-by-field (`provider: plaid → evil_corp_tampered`). Gate → revision → write → tamper-detection, all live.
- **F8 generalizes the framework.** The same control plane runs Customer 360 — a second domain with a different owner (`customer-platform`), a different policy class (`Pii`), and a different backfill strategy. `agent.rs`, `check.rs`, `verify.rs`, and `explorer.rs` contain *zero* Customer-specific code (verified by grep). Same code paths, different semantic territory.

**The thesis, in one line.** Agora is what you put between an army of agents and a database when you want both *speed* and *meaning*.

---

## Judge Q&A — anticipated questions, honest answers

The three through-lines: **calibrated scope** (M0 is narrow on purpose), **honesty about offline mode**, and **the structural property is real even when the M0 implementation is loose**.

### On the policy and trust model

**Q1. What's stopping someone from claiming `actor: team:integrations-platform`?**

Nothing. The actor is *stated*, not authenticated. F5 deliberately scopes to "policy enforcement on the claimed actor" — the FGA-style evaluator is real (wildcards, considered-tuple introspection, `DenyAttempt` audit rows), but identity verification is the next architectural layer. Production wires JWT/session → actor through the same evaluator unchanged. We frame F5 as "the policy step fires pre-INSERT and is auditable," not "this is authorization." That distinction is on the demo cards and in the README.

**Q2. Where does the policy spec actually come from?**

At runtime the FGA spec is derived from the seed catalog's `ownership.team`. F1 *does* emit `.fga.json` artifacts to disk under `generated/{proposal_id}/`, but the evaluator currently reads from the in-memory `ConceptCard`, not those files. The structural property — *policy as a first-class artifact emitted by ontology change* — is real; the artifact-on-disk → evaluator loop is the next-step integration we deliberately scoped out for M0.

**Q3. What stops me from bypassing the daemon and writing directly to Postgres?**

Nothing — and that's exactly the threat model. `agora verify` catches it. Out-of-band `INSERT` → no mutation_log row → `created_out_of_band`. Out-of-band `UPDATE` → checksum mismatch → `tampered`. Beat 7c is the live demonstration. The 56 out-of-band rows the verify panel surfaces include the 47 seed `Account` rows from migration 002 — same property. We don't pretend the database can prevent privileged users from issuing SQL; we make every such mutation *visible*.

### On the agent loop

**Q4. Does the gate actually verify the backfill is correct in offline mode?**

No. Offline mode trusts `backfill_plan` *presence*, not *correctness*. The gate-blocking, rationale-reading, and revision-emitting are all real. The final clearance is offline-loose **by design** — proving the *loop* (block → read rationale → revise → re-check → clear) is the goal; proving the *backfill is sound* is out of scope for M0. The Beat 6½ UI card hints at this with the "row(s) flagged but mitigated by backfill_plan (Advisory)" line. Production-grade would simulate the backfill in a rollback'd transaction and re-count violations.

**Q5. What if the LLM hallucinates a backfill_plan?**

The plan schema is structured: `strategy`, `source`, `idempotent`, `rationale`, `backfill_query`. Production would simulate before committing — see Q4. The demo proves the *loop*: the gate emits a structured rejection, the agent ingests it, the agent emits a structured revision that addresses the cited problem. The agent isn't waiting for a human to translate the rejection.

**Q6. What if the agent loop never converges (Stalled)?**

It's bounded — `MAX_ATTEMPTS = 3`. `FinalStatus::Stalled` is a first-class outcome; the UI renders every attempt card whether the run succeeded or stalled. Failure is part of the audit trail, not a hidden path.

**Q7. Can the LLM be replaced with a deterministic engine?**

Yes. The structured-output schema *is* the integration boundary. Anything that emits valid `OntologyChangeProposal` JSON works. Our offline mode already proves this — the deterministic offline author emits the same schema; the gate doesn't know the difference.

### On scale and substrate

**Q8. Can this scale?**

Today: single Postgres instance, no sharding, no replicas, no failover. That's the M0 substrate, called out in `STACK.md` and `README.md`. **The control plane is the architectural piece; the substrate is replaceable.** The investigation report (`investigation/gpt-deep-research-report.md`) identifies Spanner + Protobuf as the M1 promotion path; the control plane code doesn't change between them — only the storage driver.

**Q9. How does `agora verify` scale on a billion rows?**

Today it's a full table scan with canonical-JSON SHA-256 checksums. Production would: (i) compute checksums incrementally per-mutation, (ii) sample-verify on a cadence, (iii) drift-only by partition. The *structural* property — every entity has a checksum trail in `mutation_log` — is what makes any of those optimizations possible. We chose to prove the property first.

**Q10. How do you handle race conditions on concurrent writes?**

Entity writes are atomic transactions (one txn per `apply_create_*` call covering both the entity-table insert and the `mutation_log` append). The mutation log is append-only with a monotonic `mutation_seq`. Concurrent migrations are serialized by `pg_advisory_lock(0xA607A)` around `db::migrate` (added during F8 to deflake concurrent test invocations).

### On positioning

**Q11. Why not just use Confluent Schema Registry?**

Schema-against-schema vs. schema-against-data. Confluent says "shapes are compatible." Agora says "this proposal would invalidate 47 existing rows in the live `accounts` table." Different question. The multi-axis check in Beat 3 (`data_conformance` axis) is the part Confluent can't do — and the part the demo's most-cited moment hinges on.

**Q12. How is this different from Backstage or DataHub?**

Both describe at-rest data and software topology *after the fact*. They're descriptive catalogs. Agora mediates the *write path*: proposals are gated before changes land, writes are policy-checked before they commit, drift is caught after. The control plane is prescriptive, not retrospective. Beat 8's explorer view is what a descriptive catalog looks like *when it's also the runtime registry*.

**Q13. Why not XTDB / Datomic for the bitemporal piece?**

Deferred. The investigation flags valid-time + transaction-time as a P1 spike. M0 is append-only mutation log with "as-of" reads, which is sufficient for Beat 7's history. We chose to prove the agentic + audit + verify properties first, on a substrate every reviewer can stand up in five minutes.

### On the demo itself

**Q14. The 56 out-of-band rows in Beat 7's verify panel — is that a bug?**

No. They are pre-existing seed rows (47 from `migrations/002_seed_accounts.sql`, plus 5–9 from `migrations/005_customer_domain.sql`) inserted *before* the mutation log existed for those rows. `agora verify` correctly classifies them as `created_out_of_band`. The narration explicitly tells the demoer to point at the `tampered_entities[]` array (the row you just touched, with field-level diff), not the top-level status. The 56 rows are honest noise we *want* visible — they tee up Beat 8's option-B punchline: every row Agora didn't see is flagged.

**Q15. What if the LLM API call hangs mid-demo?**

Offline mode is wired. Each proposal card surfaces the author-mode pill (`live · LLM-derived` or `offline · no API key` / `offline · API error`). If the live call stalls, the deterministic offline path keeps every beat structure identical — only the pill text changes. Both Beat 1 and Beat 6½ exercise the same fallback.

---

## Quick technical card

| | |
|---|---|
| **Stack** | Rust · Axum · maud · HTMX · sqlx · Postgres |
| **Binaries** | `agorad` (HTTP daemon, port 3030) · `agora` (CLI, same library) |
| **Lines of code** | ~12.3k Rust (`src/` + `tests/`), ~200 SQL across 5 migrations |
| **Tests** | **78 passing** (`cargo test`) — 35 lib + 5 agent_loop + 8 customer_domain + 8 daemon_http + 5 mutation_log_verify + 4 policy_enforcement + 5 risk_gate + 8 ui_browser |
| **Build** | `cargo build --bin agorad` — one binary, no Node, no bundler |
| **Runtime** | Single process, single Postgres. F4 UI inlines HTMX bytes so the demo works on venue wifi |
| **Demo length** | ~6m 15s end-to-end (`DEMO.md` v2.0) |
| **Domains shipped** | `core.integrations.*` (BankIntegration, AuthenticationMethod) · `core.users.Account` · `core.customer.*` (Customer, LoyaltyTier, PurchaseHistory) |
| **Features shipped** | F1 propose · F2 7-axis check · F3 write/verify/explorer · F4 browser UI · F5 policy enforcement · F6 agent loop · F8 second domain · F-DAEMON HTTP control plane |

---

## Where to look in the repo for each claim

- **The five proofs landing live:** `DEMO.md` v2.0 (commit `96f28d8`)
- **The runbook:** `README.md` (commit `9fc2e6a`)
- **The closed loop:** `src/agent.rs` (`MAX_ATTEMPTS = 3`, `Authored | Revised`, `Approved | Stalled`)
- **The honest offline-mode framing:** `src/llm.rs` (`AuthorMode::{Live, OfflineNoKey, OfflineApiError}`)
- **The structured rejection the agent reads:** `src/check_report.rs` (`block_reason`, per-axis `findings`)
- **The policy evaluator:** `src/policy.rs` (~250 lines, no new deps, 8 unit tests)
- **The drift detector:** `src/verify.rs` (`tampered_entities`, `outofband_entities`, `diff_top_level_fields`)
- **The proof of generalization:** `tests/customer_domain.rs` (`agent_loop_on_customer_email_tighten`)
- **The original thesis:** `investigation/gpt-deep-research-report.md`

---

## The closing line for the audience

The hard problem isn't storage. The hard problem is letting a thousand agents change a thousand things without losing the meaning. Agora is what you put between them and the database when you want both speed and meaning.
