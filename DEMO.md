# Agora Demo Script

**Status:** v2.0 — **Locked.** Adds Beat 6½ (closed-loop agent revision), strengthens Beat 7's verify with field-level tamper diffs, and ties Beat 8 into the verify report. All beats are wired in the F4 browser UI and run live against the F-DAEMON HTTP control plane. See Decision Log for the v2.0 changes.

**Audience:** Hackathon judges and reviewers. They have ~5 minutes of attention. They are skeptical of toy demos. They want to see real operational behavior, not screenshots.

**Date:** 2026-05-15

---

## What we are proving

Agora is **not a new database**. It is a **governed operational ontology and control plane** that sits above a serious storage substrate. It exists to solve one problem:

> Agentic product development degenerates into local schemas, duplicated concepts, and unreliable cross-product joins. Central core-service teams become bottlenecks. Schemaless stores externalize semantic coherence into tribal knowledge. Agora gives both **offline data** and **live operational data** a shared, evolvable structure that agents and humans can safely build against — without a committee bottleneck and without schema-free sprawl.

The demo must show that **a product or agent can propose a new concept, Agora detects reuse vs. duplication, generates operational artifacts, approves safe additive changes automatically, blocks meaning-changing or policy-sensitive changes, lets the agent read the rejection and revise into approval, produces objects that are discoverable, auditable, replayable, and permissioned from day one, and detects any out-of-band tampering with the data it governs.**

A demo that only shows schema exploration, or only code generation, undershoots the thesis and will not land.

---

## Where the demo runs

Everything happens in one browser tab against one binary.

- **Runtime:** `agorad` (single Rust binary) listening on **`http://localhost:3030`**.
- **UI:** F4 browser UI at `/` renders every beat as its own section. HTMX swaps each beat's response into a slot in place — no page reloads, no URL bar gymnastics, every prior beat stays visible.
- **No demo-mode toggle.** The `/ui/*` HTMX handlers wrap the same library functions (`llm::author_proposal`, `check::check`, `verify::verify`, `explorer::explorer`, `entity_write::*`, `agent::agent_loop`) that the JSON daemon and the `agora` CLI use. The judges see the same code path any caller would.
- **Live mode vs. offline mode** is determined by whether `ANTHROPIC_API_KEY` is set. Each beat surfaces an explicit author-mode pill (`live · LLM-derived` or `offline · …`). Honest framing is part of the demo, not hidden.

---

## The vertical slice we will demo

We model one concrete domain end-to-end: **bank integrations** (plus `Account` for the risky thread). The seed catalog includes:

- **`core.integrations.BankIntegration`** — canonical entity, owned by `integrations-platform`.
- **`core.integrations.AuthenticationMethod`** — canonical concept, pre-seeded. The reuse-detection target for Beat 2.
- **`core.users.Account`** — canonical entity, owned by `identity-platform`. Pre-seeded with **47 rows whose `email` field is NULL**. These rows are how Beat 6 fails honestly; they are not a demo prop.

Three threads run through the now-9 beats:

- **Happy thread:** a real LLM (Anthropic structured-output) authors a proposal to add a new `BankIntegrationCapability` concept tied to `BankIntegration`. Threads beats 1–5, 7, 8.
- **Risky thread (rejection):** a second proposal tightens `Account.email` from optional to required. Beat 6 is its moment.
- **Revision thread (the loop):** Beat 6½ picks the risky thread back up — the agent reads the rejection rationale and revises with a `migration.backfill_plan`, the gate re-runs, and the proposal clears. This is the closed-loop arc.

---

## The 9 beats

Each beat lists: **what happens on screen**, **what it proves**, and **what would defeat the proof** (so implementers know what *cannot* be faked).

### Beat 1 — A real LLM authors and submits a proposal

**On screen:** A developer types into the proposal form on `/`. The browser POSTs `/ui/propose`. Agora calls Anthropic with a structured-output schema; the LLM emits an `OntologyChangeProposal` JSON for a new `BankIntegrationCapability` concept. The proposal card renders with the author-mode pill, change intent, declared compatibility, semantic-contract before/after, and the four generated artifacts in a tab strip. Offline fallback emits a deterministic stub with the offline pill.

**What it proves:** The unit of change in Agora is a **semantic proposal**, not a raw DDL diff. *And it can be authored by an agent.* The Anthropic structured-output call is the realism marker. Agents are forced to declare intent and meaning, not just shape; that is what makes everything downstream possible.

**Cannot be faked:** The proposal must round-trip through `llm::author_proposal` and `artifacts::emit_all` — same code path the CLI uses. The author mode pill must reflect actual API key state.

---

### Beat 2 — Agora detects reuse vs. duplication

**On screen:** Inside the same `/ui/propose` response, the proposal card includes a **Reuse detection** subsection. `reuse::classify` runs against the seed catalog and surfaces hits ranked by score (Jaccard, cosine, layer). For the `BankIntegrationCapability` proposal, the existing `AuthenticationMethod` concept appears as a top hit; the class is `Refinement` or `Reuse` depending on overlap. The proposer's chosen extension semantics are encoded in the proposal itself.

**What it proves:** Agora is **not just a registry** — it actively prevents the agentic-chaos failure mode where every team invents its own near-duplicate concept. Reuse detection is first-class.

**Cannot be faked:** `AuthenticationMethod` must really be in the seed catalog. The hits must come from the live reuse classifier scored against the live proposal payload — not a hardcoded list.

---

### Beat 3 — Multi-axis check report

**On screen:** "Run multi-axis check →" POSTs `/ui/proposals/{id}/check`. `check::check` runs eight checks in parallel: composition, shape, semantic (LLM), policy, temporal, impact, replay, plus a live **data-conformance** query against Postgres. The table that renders shows axis · outcome · findings · source · elapsed-ms. A hint line surfaces `data_conformance source = …` and `live count = …` so a reviewer can mutate the DB and watch the count change on the next click.

For the `BankIntegrationCapability` proposal every axis comes back clean. `auto_approval_eligible = true`.

**What it proves:** Compatibility is **multi-axis**, not just shape-compatible vs. shape-breaking. Shape checks aren't enough; meaning, policy, and *the actual state of the world* need validation. The "live count" line is the falsifiability marker — judges can perturb the DB and watch the check update.

**Cannot be faked:** The impact list must come from real registry lineage. The data-conformance count must come from a real query. The elapsed-ms columns must be wall-clock from the run that just happened.

---

### Beat 4 — Agora generates four real operational artifacts

**On screen:** The proposal card's artifact tab strip exposes **four real files** sitting under `generated/{proposal_id}/`:

1. **`.proto`** — protobuf message defining the typed contract.
2. **`.sql`** — Postgres DDL for the new table and FK to `BankIntegration`.
3. **`_handler.rs`** — Rust axum HTTP handler for the new entity.
4. **`.fga.json`** — declarative policy spec (field classifications, read/write rules).

GraphQL and gRPC service stubs are **explicitly cut for the hackathon**.

**What it proves:** Agora doesn't replace the database — it **compiles** ontology changes into storage, contracts, APIs, and policy. One semantic change → four coherent operational artifacts. Scoped honestly: four real outputs, not a parade of stubs.

**Cannot be faked:** All four files must be on disk, real, and consumed downstream. The tabs render the actual file contents via `read_or_placeholder` — a missing file shows the error verbatim. The DDL must apply; the handler must be the same path Beat 7 calls; the policy spec must be inspectable.

---

### Beat 5 — The additive proposal is auto-approved and published

**On screen:** "Submit for approval →" POSTs `/ui/proposals/{id}/approve`. The handler reads the cached `check_report.json`, applies the `auto_approval::apply` predicate, and renders an approval card showing `predicate ⇒ all_axes_clean=true`, the report id, and `generated_at`. The verdict comes from the same predicate the CLI sees.

**What it proves:** Agora **removes the central-team bottleneck for safe changes**. Agents ship without waiting on humans, *because the protocol forced enough semantic discipline upfront to make automated approval trustworthy*.

**Cannot be faked:** The predicate is a one-liner in `auto_approval.rs`; reviewers can read it. The approval card surfaces the predicate string verbatim, not a "demo mode" stub.

---

### Beat 6 — A risky proposal is blocked because real data won't survive it

**On screen:** "Run risky proposal →" POSTs `/ui/risky-proposal`. Agora loads `fixtures/beat6_tighten_account_email.json` (tighten `Account.email` from optional to required) and runs `check::check` against it. The **data-conformance** axis fires `SELECT COUNT(*) … WHERE email IS NULL` against the live `Account` table and reports **47 existing rows would violate the proposed constraint**. The card shows the failing axes, the live count, sample violations, and the verbatim query that produced the count. `auto_approval_eligible = false`. There is no "Submit for approval" button on this card — by design.

**What it proves:** Agora **catches the failure mode that breaks every shared-data system in practice** — the change that compiles cleanly, passes shape checks, and would silently turn 47 records into invariant violations. The block is grounded in *the actual state of the world*, not in a hypothetical lint rule.

**Cannot be faked:** The 47 NULL `email` rows must be real rows in the `Account` table. The card surfaces the verbatim SQL `query` that produced the count — judges can copy-paste it into psql and replicate.

---

### Beat 6½ — Agent loop: revise on rejection (F6 — the vision driver)

**On screen:** "Run agent loop →" POSTs `/ui/agent`. `agent::agent_loop` runs up to `MAX_ATTEMPTS = 3` author→check→(revise→check){0..2} iterations. Each attempt renders as its own card stacked top-down so the audience reads them as a conversation:

1. **Attempt 1** — `Authored` from prompt → check report → **blocked** (47 rows in data-conformance). The block reason and per-axis evidence are visible.
2. **Attempt 2** — `Revised`: the agent reads the block_reason and the failing axes' findings, then emits a new proposal *with the same target* but now carrying `migration.backfill_plan` (strategy, source, idempotent, rationale) plus a backfill query. The check re-runs; the data-conformance axis downgrades from `fail` to `advisory`; `auto_approval_eligible` flips true; the proposal is **approved**.

A banner above the cards shows `Approved after 2 attempt(s)` or `Stalled after 3 attempt(s)`. Both outcomes are first-class — failure cards stay visible so the audience can see *what* failed.

**Locked narration (verbatim from nemesis):**

> "Agent proposes tightening Account.email. Gate blocks: 47 historical rows violate. Agent reads the rejection rationale and revises — same proposal, same target, but now with a backfill_plan that derives email from the users table. Gate re-runs. The 47 rows are still there — but the proposal now carries a mitigation. The data_conformance axis downgrades from fail to advisory and the proposal auto-approves. The agent didn't just retry; it addressed the actual problem. That's the loop."

**What it proves:** The agent isn't waiting on a human reviewer to translate the rejection. The control plane's structured rationale is *machine-readable enough* that the agent can ingest it, identify the actual problem (data won't survive the constraint), and propose a fix (backfill the data first). That's the closed-loop agentic story the rest of the demo sets up.

**Honest framing (offline mode caveat — included on the card itself):**

In live mode (`ANTHROPIC_API_KEY` set), the LLM rewrites the proposal. In offline mode, a deterministic heuristic adds `migration.backfill_plan`. **In both modes, the gate trusts the *presence* of a backfill plan, not its correctness.** This is by design for the hackathon — proving the *loop* is real (block → read rationale → revise → re-check → clear) is the goal; proving the *backfill is sound* is out of scope.

**What is real in this beat:**
- The gate-block is real (real 47 rows, real query).
- The rationale-read is real (the revision step receives the full `CheckReport` including `block_reason` and per-axis findings as input).
- The revision-emitting is real (a new proposal artifact is produced, persisted, and re-checked).
- The author-mode pill on each attempt card surfaces live vs. offline honestly.

**What is offline-loose by design:**
- The final clearance trusts `backfill_plan.presence`, not the SQL inside it. The card hints at this with the "row(s) flagged but mitigated by backfill_plan (Advisory)" line.

**Cannot be faked:** Every attempt card must be a real `Attempt` from `agent::agent_loop` with a real proposal, a real check report, and a real `ActionTaken` ("authored" vs. "revised — {reason}"). The loop is bounded by `MAX_ATTEMPTS = 3`; the `Stalled` outcome must be reachable when the heuristic fails (covered by unit tests).

---

### Beat 7 — Write → tamper → verify (with field-level diff)

**On screen:** Three sub-steps on the same beat slot:

1. **Happy write.** "Write a BankIntegration →" POSTs `/ui/write`. `entity_write::apply_create_bank_integration` opens a transaction, inserts the row into `bank_integrations`, appends an atomic entry to `mutation_log` with a canonical-JSON SHA-256 `checksum`, and commits. The card renders `entity_id`, `mutation_seq`, `checksum`, `actor`, and exposes two action buttons.
2. **Out-of-band tampering.** "Tamper this row out-of-band →" POSTs `/ui/tamper`. A raw `UPDATE bank_integrations SET provider = 'evil_corp_tampered' WHERE id = $1` runs via `sqlx::query(...).bind(...)` — bypassing the handler. The mutation_log is *not* updated. The tamper card shows the logical SQL plus the note that the parameterization is on the wire.
3. **`agora verify` catches it.** "Run agora verify →" GETs `/ui/verify`. `verify::verify` re-reads every entity row, recomputes its canonical-JSON checksum, and compares to the latest `mutation_log` entry. The panel renders:
   - A banner: "Drift detected" with counts of tampered rows and out-of-band rows.
   - A **Tampered rows** section showing one card per drifted entity with `entity_id`, `last_logged_at`, `last_logged_actor`, `detected_via`, logged-vs-current checksums, **and a field-level diff table** (`logged value` vs `current value`) so the audience sees `provider: plaid → evil_corp_tampered` in red.
   - A **Created out-of-band** section listing every row that exists without a matching `mutation_log` entry.

**Locked narration caveat (test-reviewer):** `verify_status: tampered` at the top of the report will also be true because **56 pre-existing seed rows from F2 (inserted before the mutation log was wired)** are correctly classified as `created_out_of_band`. **The narration points at the `tampered_entities[]` array specifically** — that is the row you just touched, with its field-level diff. The 56 out-of-band rows are honest noise we *want* visible; they tee up Beat 8.

**What it proves:** Agora preserves **auditability and integrity by construction**. The control plane's guarantees aren't theoretical — they survive an actual bypass attempt, and the proof is field-level (`provider: plaid → evil_corp_tampered`), not "drift detected, trust us."

**Cannot be faked:** The tamper is a real `UPDATE` (parameterized, on the wire). Verify queries Postgres live and recomputes checksums on every click — the timestamp on the panel updates with each request. The field-level diff comes from `diff_top_level_fields(logged_state, current_state)` — both JSON objects are surfaced in the response so reviewers can confirm.

---

### Beat 8 — Explorer: every row Agora didn't see is flagged

**On screen:** The Beat 8 slot on the home page lists the three canonical concepts. Clicking `core.users.Account` opens `/ui/concepts/core.users.Account` — the full ConceptView:

- Header card: namespace, name, version, owner team, semantic steward, policy class.
- **Fields** table: every field with proto type, required flag, since-version, classification, doc.
- **Invariants** list.
- **Lineage**: HTTP route, storage table, .proto artifact, .fga.json artifact, references, *proposals that touched this concept* (today's Beat 6 + Beat 6½ entries are visible here).
- **Policy attachments**: per-relation tuples (relation/subject/object).
- **Version history**: every `mutation_log` entry for entities of this type — seq, op, ontology_v, entity_id, actor, occurred_at, truncated checksum. If the DB is unavailable, the section says so explicitly rather than rendering a fake.

**The tie-in to Beat 7 (option B, from nemesis):** the 56 out-of-band rows surfaced in Beat 7's `outofband_entities[]` section **include the 47 NULL-email accounts** from Beat 6. Same row sets, two lenses:
- Beat 6's lens: data-conformance against a *proposed* constraint → "47 rows would violate."
- Beat 8's lens: presence against the mutation log → "every row Agora didn't see is flagged."

That coincidence is the demo's most under-appreciated punchline: Agora's value isn't catching one kind of violation — it's that everything outside the control plane is *visible*, by construction.

**What it proves:** Discovery and trust are **first-class outputs of the control plane**, not someone's side-project wiki. An agent or human can find canonical concepts and APIs without human routing. The ontology, contracts, storage, policy, and history all share one navigable graph — and the rows the control plane has never seen are visible too.

**Cannot be faked:** Every field shown must be backed by real registry data or real `mutation_log` queries. The "Touched by proposals" list must include the proposals authored today. The DB-unavailable branch must do its job, not paper over absence.

---

## What ties the beats together (don't lose this)

Three threads, kept clean:

- **Happy thread:** Beat 1's `BankIntegrationCapability` proposal threads through Beats 2, 3, 4, 5, 7, 8.
- **Risky-rejection thread:** Beat 6's `Account.email` tightening — same check engine as happy thread, but the world (47 NULL rows) makes it fail.
- **Revision thread:** Beat 6½ picks up the risky thread. Same target, same check engine, but the proposal now carries a `migration.backfill_plan` and clears. The contrast between Beat 6's red banner and Beat 6½'s green banner *on the same target concept* is the punchline.

Beat 7's tampering sub-step is the fourth arc: the control plane keeps watching *after* a proposal has shipped. Beat 8's out-of-band section is the fifth arc: it sees what the control plane never saw at all.

---

## Time budget (target: ~6m 15s)

| Beat | Time | Notes |
|---|---|---|
| 1 | 60s | LLM authors proposal live. Realism marker — let the model take a few seconds. |
| 2 | 30s | Reuse detection → `AuthenticationMethod` surfaced inside the same card. |
| 3 | 45s | Multi-axis check report. Falsifiability hint ("live count =") is the moment. |
| 4 | 30s | Click through the four artifact tabs. |
| 5 | 15s | "Auto-approval fired — predicate string is the proof." Short and punchy. |
| 6 | 60s | Risky proposal → 47 NULL emails → block with verbatim query. |
| **6½** | **45s** | **Agent loop — block, revise, clear. Read nemesis's narration verbatim while the second attempt card renders. Acknowledge offline-loose framing in one line.** |
| 7 | 60s | Happy write → live `UPDATE` → `agora verify` catches drift. Point at `tampered_entities[]` row's field-level diff. |
| 8 | 30s | Explorer tour. Land the option-B coincidence: "every row Agora didn't see is flagged — the 56 out-of-band rows include the 47 NULL-email accounts from Beat 6." |

**Total:** 6m 15s. Trimming options if needed: Beat 5 (15→10s, drop the predicate readout), Beat 4 (30→20s, click only the .proto and .sql tabs), Beat 8 (30→20s, skip Touched-by-proposals).

**Non-deterministic budget items:** Beat 1's LLM call and Beat 6½'s revision attempt(s). Mitigation: offline mode is wired (the `offline · no API key` pill is a feature, not a bug). If the live LLM stalls mid-demo, the offline path keeps the beat structure identical — only the pill text changes.

---

## Decision log

| Date | Change | Why |
|---|---|---|
| 2026-05-14 | v1.0 — domain, concepts, Beat 6 risky scenario, Beat 4 four-artifact set, gRPC/GraphQL cut, Beat 7 verify+tampering | PM-locked decisions; see v1.0 history below. |
| 2026-05-14 → 2026-05-15 | v0.1 open questions resolved | Domain = bank integrations; Beat 2 target = `AuthenticationMethod`; Beat 6 = `Account.email` optional→required (47 NULL rows); history = Postgres append-only mutation log; explorer = CLI-acceptable (later: F4 browser UI); Beat 1 = real LLM (Anthropic structured output); Beat 4 artifacts = DDL, `.proto`, HTTP handler, policy spec. |
| **2026-05-15** | **v2.0 — Beat 6½ added (closed-loop agent revision, F6)** | The vision driver. Beat 6 alone proves *Agora blocks*; Beat 6½ proves *the agent reads the block and addresses the actual problem*. Verbatim narration from nemesis pulled into the beat. Honest offline-loose framing inline so test-reviewer's note is preserved on screen. |
| **2026-05-15** | **v2.0 — Beat 7 strengthened with field-level tamper diff** | The verify panel now renders `logged value` vs `current value` per changed field (`diff_top_level_fields`). Proof is concrete, not "drift detected, trust us." |
| **2026-05-15** | **v2.0 — Beat 7 narration points at `tampered_entities[]` specifically** | 56 pre-existing seed rows from F2 are correctly classified as `created_out_of_band`, which makes top-level `verify_status: tampered` true even before any live tampering. Pointing at the `tampered_entities[]` array isolates the live drift from the seed-data noise. |
| **2026-05-15** | **v2.0 — Beat 8 ties into Beat 7's `outofband_entities[]`** | Option B from nemesis: "every row Agora didn't see is flagged — the 56 out-of-band rows include the 47 NULL-email accounts from Beat 6." Two lenses on the same row sets. |
| **2026-05-15** | **v2.0 — runtime documented** | `agorad` on `:3030`, F4 browser UI ships all 8(+1) beats live, no demo-mode toggle. |

---

## Living-doc rules

- This script is the **north star**. Implementation must serve the beats; if a beat is implementable only by faking, raise it — don't quietly drift.
- If a beat's implementation detail changes, update the relevant section in place and add a row to the decision log.
- If we cut a beat for time, mark it `[CUT]` rather than deleting — keeps the rationale visible.
- Every commit that lands or modifies a beat should reference the beat number in the message.
- **Honest framing rule:** every place the demo trusts something it can't fully verify (e.g. Beat 6½'s `backfill_plan.presence`), that fact lives in the beat itself, not in a footnote. The audience hears it from the script, not from a critic.
