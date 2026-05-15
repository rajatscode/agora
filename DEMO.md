# Agora Demo Script

**Status:** v1.0 — **Locked by PM (2026-05-14). This is the north star for implementation.** All open questions from v0.1 are resolved (see Decision Log at the bottom). Living-doc rules still apply if implementation surfaces a forced change.

**Audience:** Hackathon judges and reviewers. They have ~5 minutes of attention. They are skeptical of toy demos. They want to see real operational behavior, not screenshots.

**Date:** 2026-05-14

---

## What we are proving

Agora is **not a new database**. It is a **governed operational ontology and control plane** that sits above a serious storage substrate. It exists to solve one problem:

> Agentic product development degenerates into local schemas, duplicated concepts, and unreliable cross-product joins. Central core-service teams become bottlenecks. Schemaless stores externalize semantic coherence into tribal knowledge. Agora gives both **offline data** and **live operational data** a shared, evolvable structure that agents and humans can safely build against — without a committee bottleneck and without schema-free sprawl.

The demo must show that **a product or agent can propose a new concept, Agora detects reuse vs. duplication, generates operational artifacts, approves safe additive changes automatically while blocking meaning-changing or policy-sensitive changes, produces objects that are discoverable, auditable, replayable, and permissioned from day one, and detects any out-of-band tampering with the data it governs.**

A demo that only shows schema exploration, or only code generation, undershoots the thesis and will not land.

---

## The vertical slice we will demo

We model one concrete domain end-to-end: **bank integrations**. The pre-seeded registry contains:

- **`BankIntegration`** — canonical entity, owned by `integrations-platform`.
- **`AuthenticationMethod`** — canonical concept, already present in the registry. The reuse-detection target.
- **`Account`** — canonical entity, used for the risky-proposal thread. Pre-seeded with **47 rows whose `email` field is NULL**. This is not a demo prop — those rows are how Beat 6 fails honestly.

Two threads run through the 8 beats:

- **Happy-path thread:** a real LLM (Anthropic structured-output CLI) authors a proposal to add a new `BankIntegrationCapability` concept tied to `BankIntegration`. This thread runs beats 1–5, 7, 8.
- **Risky thread:** a second proposal tightens `Account.email` from optional to required. Beat 6 is its moment. It is intentionally a *different* proposal, with a *different* concept, so we can show the boundary between safe and unsafe in the same demo.

---

## The 8 beats

Each beat lists: **what happens on screen**, **what it proves**, and **what would defeat the proof** (so implementers know what *cannot* be faked).

### Beat 1 — A real LLM authors and submits a proposal

**On screen:** A developer asks an LLM (Anthropic structured-output CLI) something like "we need to model what each bank integration can do — supported features, rate limits, etc." The LLM emits a real `OntologyChangeProposal` JSON for a new `BankIntegrationCapability` concept. The proposal carries: domain, namespace, target ontology version, change intent, rationale, ownership, classifications, compatibility declarations (multi-axis), migration plan, tests, and provenance. The proposal is submitted to Agora.

**What it proves:** The unit of change in Agora is a **semantic proposal**, not a raw DDL diff. *And it can be authored by an agent.* The Anthropic structured-output call is the realism marker — the proposal is generated in front of the judges, not pre-baked. Agents are forced to declare intent and meaning, not just shape; that's what makes the rest of the workflow possible.

**Cannot be faked:** The LLM call must happen live. The output must be a real artifact the rest of the pipeline consumes. Not a slide. Not a hand-edited JSON.

---

### Beat 2 — Agora detects reuse vs. duplication

**On screen:** Agora's ontology lookup scans the proposal and finds that `BankIntegrationCapability` overlaps semantically with the existing canonical `AuthenticationMethod` concept (auth-related capabilities are a subset). The system surfaces the overlap and forces the proposer to decide: **refine** the existing concept, **deprecate-and-replace**, or **namespace-extend**. The chosen decision is recorded as part of the proposal lineage.

**What it proves:** Agora is **not just a registry** — it actively prevents the agentic-chaos failure mode where every team invents its own near-duplicate concept. Reuse detection is first-class.

**Cannot be faked:** `AuthenticationMethod` must already exist in the registry when the demo starts (seeded). The link/conflict must be machine-detected from the proposal, not hardcoded for the demo.

---

### Beat 3 — Multi-axis checks run on the proposal

**On screen:** A check report is generated. It lists:
- **Composition:** Does the ontology still compose? Do generated artifacts remain internally coherent?
- **Compatibility:** Shape, semantic, temporal, policy, API, and storage axes — each classified (additive / refinement / breaking / dangerous).
- **Semantic:** Does this overlap or change meaning of an existing concept? (Beat 2's overlap finding flows in here.)
- **Policy:** Does read/write visibility expand? Do field sensitivity classifications change?
- **Temporal:** Does this reinterpret history or valid-time semantics?
- **Impact:** Which downstream artifacts in the registry are affected?
- **Replay:** Can projections rebuild after the change?

For the `BankIntegrationCapability` proposal, every axis comes back additive/refinement/no-change. The proposal is eligible for auto-approval.

**What it proves:** Compatibility is **multi-axis**, not just shape-compatible vs. shape-breaking. This is the central insight from the investigation: shape checks aren't enough; meaning and policy need validation.

**Cannot be faked:** The impact list must come from real lineage data — actual downstream artifacts in the registry, not a hardcoded list. The report rows must be generated from the proposal + registry state, not templated.

---

### Beat 4 — Agora generates four real operational artifacts

**On screen:** From the (about-to-be-approved) proposal, Agora generates exactly **four real artifacts**, all on disk and consumable:

1. **DDL** — Postgres DDL for the new `BankIntegrationCapability` table and its foreign-key relation to `BankIntegration`. Runnable on the database.
2. **`.proto` definition** — protobuf message for `BankIntegrationCapability`. Real .proto file; defines the typed contract.
3. **HTTP handler** — Rust (axum) HTTP handler that accepts writes for `BankIntegrationCapability`. The command surface used in Beat 7.
4. **Policy spec** — declarative policy file (field classifications, read/write rules, ownership) that the verify step in Beat 7 will enforce.

GraphQL schema generation and gRPC service stubs are **explicitly cut for the hackathon**.

**What it proves:** Agora doesn't replace the database — it **compiles** ontology changes into storage, contracts, APIs, and policy. One semantic change → four coherent operational artifacts. This is the "shared business meaning with operational consequences" thesis, scoped honestly: four real outputs, not a parade of stubs.

**Cannot be faked:** All four artifacts must be on disk, real, and used downstream. The DDL must apply cleanly; the HTTP handler must accept the writes Beat 7 makes; the policy spec must be the same file Beat 7's verify step reads.

---

### Beat 5 — The additive proposal is auto-approved and published

**On screen:** Because the proposal was additive, namespaced, did not expand visibility, did not change partitioning/residency, and passed all checks, Agora's agent-approval threshold fires. The change merges, a new ontology version is published, and the four artifacts go live. No human touched it.

**What it proves:** Agora **removes the central-team bottleneck for safe changes**. Agents can ship without waiting on humans, *because the protocol forced enough semantic discipline upfront to make automated approval trustworthy*.

**Cannot be faked:** The decision must come from the threshold logic operating on the check report — not from a coin flip or a "demo mode" toggle. Reviewers should be able to see the predicate in the codebase.

---

### Beat 6 — A risky proposal is blocked because real data won't survive it

**On screen:** A second proposal arrives: tighten `Account.email` from optional to required. On surface it looks like a "make the schema tidier" refinement. Agora runs its checks. The **semantic** axis flags this as a refinement that strengthens an invariant. The **data-conformance** check (run via `agora verify` against the current `Account` table) reports: **47 existing rows have `email = NULL`**. The check fails. The proposal is blocked with a structured explanation: *what changed semantically*, *which existing data violates it*, *what would have to happen for this proposal to pass* (either a backfill plan, or a relaxation of the requirement).

**What it proves:** Agora **catches the failure mode that breaks every shared-data system in practice**: the change that compiles cleanly, passes shape checks, and would silently turn 47 records into invariant violations. The block is grounded in *the actual state of the world*, not in a hypothetical lint rule.

**Cannot be faked:** The 47 NULL `email` rows must be real rows in the `Account` table, not a number printed by a stub. The check failure must be produced by a real query against the table.

---

### Beat 7 — Writes flow through generated commands; tampering is caught

**On screen:** This beat has three sub-steps:

1. **Happy write.** A real write happens via the generated HTTP handler from Beat 4. A `BankIntegrationCapability` row is created. The mutation is recorded in the append-only log. A read confirms the row.
2. **Out-of-band tampering.** From a separate terminal, a raw SQL `UPDATE` is issued directly against the Postgres table, bypassing the HTTP handler entirely. The row's data now disagrees with what the control plane knows.
3. **`agora verify` catches it.** Running `agora verify` against the database surfaces the drift: it identifies the row whose state was modified outside the control plane, names the field, and points to the missing mutation-log entry that should have accompanied the change.

**What it proves:** Agora preserves **auditability and integrity by construction**. The control plane's guarantees aren't theoretical — they survive an actual bypass attempt. This is the demo beat that distinguishes Agora from "yet another schema registry."

**Cannot be faked:** The `UPDATE` must be a real SQL statement issued live; `agora verify` must really query both the data and the mutation log and report a real discrepancy. No demo-mode toggle.

---

### Beat 8 — Explorer shows owner, invariants, lineage, policy, version history

**On screen:** A developer (or another agent) opens the explorer and navigates to `BankIntegration`. In one view they see:
- Owner (`integrations-platform`) and semantic steward.
- Invariants on the entity.
- Lineage — the new `BankIntegrationCapability` relation from today's proposal, the existing `AuthenticationMethod` link, the HTTP handler and policy spec exposing the entity.
- Policy attachments (who can read which fields).
- Version history — today's proposal, who/what authored it (LLM, with attribution), the check report, the diff that landed.

**What it proves:** Discovery and trust are **first-class outputs of the control plane**, not someone's side-project wiki. An agent or human can find canonical concepts and APIs without human routing. The ontology, contracts, storage, policy, and history all share one navigable graph.

**Cannot be faked:** Every field shown must be backed by real registry data populated through beats 1–7.

---

## What ties the beats together (don't lose this)

The demo only works if the **two threads are kept clean**:

- **Happy thread:** Beat 1's `BankIntegrationCapability` proposal is the artifact Beat 2 detects reuse on, Beat 3 checks, Beat 4 compiles into four artifacts, Beat 5 approves, Beat 7 writes against, and Beat 8 shows in version history. If any beat is implemented as a standalone screen disconnected from the others, the thesis collapses.
- **Risky thread:** Beat 6's `Account.email` proposal is run against the *same* check engine as the happy thread, but the world (47 NULL rows) makes it fail. The contrast between the two threads in the same engine is what makes the multi-axis check claim credible.

Beat 7's tampering sub-step is the third arc: it proves the control plane keeps watching *after* a proposal has shipped.

---

## Time budget (target: 5 minutes)

| Beats | Time | Notes |
|---|---|---|
| 1 | 60s | LLM authors proposal live. The realism marker — let the model take the few seconds it needs. |
| 2 | 30s | Reuse detection → AuthenticationMethod surfaced; proposer picks namespace-extend. |
| 3 | 45s | Multi-axis check report. This is where judges learn what Agora *is*. |
| 4 | 30s | Show the four files appearing on disk. |
| 5 | 15s | "Auto-approval fired because every axis was clean." Short and punchy. |
| 6 | 60s | Risky proposal → 47 NULL emails → block with explanation. |
| 7 | 60s | Happy write, then live `UPDATE`, then `agora verify` catches drift. This is the dramatic beat. |
| 8 | 30s | Explorer tour. Land the "one navigable graph" point. |

Total: 5m 30s. If we need to trim, Beat 5's voiceover can shrink and Beat 8 can lose 10s. Beat 1's LLM latency is the only non-deterministic budget item — plan a fallback if the API call hangs (cached response file, switched live with one keystroke).

---

## Decision log (PM-locked, 2026-05-14)

| Open question (v0.1) | Resolution |
|---|---|
| Which domain do we model? | **Bank integrations.** Concepts in scope: `BankIntegration`, `AuthenticationMethod`, `BankIntegrationCapability`. |
| Beat 2's duplicate concept? | **`AuthenticationMethod`** is pre-seeded in the registry; reuse detection links the new `BankIntegrationCapability` to it. |
| Beat 6's risky-proposal category? | **Semantic refinement that breaks existing data:** `Account.email` optional→required, blocked by 47 pre-existing NULL rows. |
| Beat 7's history substrate? | Postgres append-only mutation log (M0 per investigation). Full bitemporality deferred. |
| Beat 8's explorer UI? | Deferred to Architecture Lead; CLI walkthrough is acceptable. Doesn't change the script. |
| Beat 1's authorship? | **Real LLM** (Anthropic structured-output CLI). The proposal is generated live. |
| Beat 4's artifact set? | **Four real artifacts:** DDL, `.proto`, HTTP handler, policy spec. **GraphQL and gRPC service stubs cut.** |
| New step: verify + tampering? | **Added as Beat 7 sub-steps.** `agora verify` runs after a live out-of-band `UPDATE`; must report real drift. |

---

## Living-doc rules

- This script is now the **north star**. Implementation must serve the beats; if a beat is implementable only by faking, raise it — don't quietly drift.
- If a beat's implementation detail changes, update the relevant section in place and add a row to the decision log.
- If we cut a beat for time, mark it `[CUT]` rather than deleting — keeps the rationale visible.
- Every commit that lands a beat should reference the beat number in the message.
