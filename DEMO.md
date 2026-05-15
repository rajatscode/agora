# Agora Demo Script

**Status:** Draft v0.1 — living document. Updates expected once PM finalizes features and Architecture Lead confirms stack.

**Audience:** Hackathon judges and reviewers. They have ~5 minutes of attention. They are skeptical of toy demos. They want to see real operational behavior, not screenshots.

**Date:** 2026-05-14

---

## What we are proving

Agora is **not a new database**. It is a **governed operational ontology and control plane** that sits above a serious storage substrate. It exists to solve one problem:

> Agentic product development degenerates into local schemas, duplicated concepts, and unreliable cross-product joins. Central core-service teams become bottlenecks. Schemaless stores externalize semantic coherence into tribal knowledge. Agora gives both **offline data** and **live operational data** a shared, evolvable structure that agents and humans can safely build against — without a committee bottleneck and without schema-free sprawl.

The demo must show that **a product or agent can propose a new concept, Agora detects reuse vs. duplication, generates operational artifacts, approves safe additive changes automatically while blocking meaning-changing or policy-sensitive changes, and produces objects that are discoverable, auditable, replayable, and permissioned from day one.**

A demo that only shows schema exploration, or only code generation, undershoots the thesis and will not land.

---

## The vertical slice we will demo

We model one concrete domain end-to-end: **bank integrations**. A `BankIntegration` is a canonical concept owned by the `integrations-platform` team. The proposing actor is an agent (`agent://schema-broker-1`) acting on behalf of a product team that wants to extend the concept with `AuthenticationMethod`.

The 8 beats below take the same proposal from "agent has an idea" to "operational reality with history, policy, and discoverability."

---

## The 8 beats

Each beat lists: **what happens on screen**, **what it proves**, and **what would defeat the proof** (so implementers know what *cannot* be faked).

### Beat 1 — An agent proposes a new concept or extension

**On screen:** An agent submits an `OntologyChangeProposal` to add an `AuthenticationMethod` relation to `BankIntegration`. The proposal carries: domain, namespace, target ontology version, change intent, rationale, ownership, classifications, compatibility declarations, migration plan, tests, and provenance.

**What it proves:** The unit of change in Agora is a **semantic proposal**, not a raw DDL diff. Agents are forced to declare *intent and meaning*, not just shape. This is what makes the rest of the workflow possible.

**Cannot be faked:** The proposal must be a real artifact the rest of the pipeline consumes. Not a slide. Not a mock JSON in the README.

---

### Beat 2 — Agora links to existing concepts (reuse vs. duplication)

**On screen:** Agora's ontology lookup shows the proposal references a possibly related canonical type. The system either (a) reuses an existing `AuthenticationMethod` concept, or (b) flags overlap with `ProviderAuthFlag` and asks the proposer to decide: refine, deprecate, or namespace-extend. The decision is recorded.

**What it proves:** Agora is **not just a registry** — it actively prevents the agentic-chaos failure mode where every team invents its own near-duplicate concept. Reuse detection is first-class.

**Cannot be faked:** A second, near-duplicate concept must already exist in the ontology when the demo starts. The link/conflict must be machine-detected, not hand-curated for the demo.

---

### Beat 3 — Compatibility, policy, and impact checks run

**On screen:** A check report is generated. It lists:
- **Composition:** Does the ontology still compose? Do generated APIs and events stay internally coherent?
- **Compatibility:** Shape, semantic, temporal, policy, API, and storage axes — each classified (additive / refinement / breaking / dangerous).
- **Semantic:** Does this overlap or change meaning of an existing concept?
- **Policy:** Does read/write visibility expand? Do field sensitivity classifications change?
- **Temporal:** Does this reinterpret history or valid-time semantics?
- **Impact:** Which APIs, projections, jobs, services, and consumers are affected?
- **Replay:** Can projections rebuild from events/history after the change?

**What it proves:** Compatibility is **multi-axis**, not just shape-compatible / shape-breaking. This is the central insight from the investigation: shape checks aren't enough; meaning and policy need validation.

**Cannot be faked:** The impact list must come from real lineage data — actual downstream artifacts in the registry, not a hardcoded list.

---

### Beat 4 — Agora generates operational artifacts

**On screen:** From the (now-approved) proposal, Agora generates:
- Storage schema (DDL / PROTO column changes)
- Protobuf message and gRPC command/read services
- GraphQL schema additions
- Policy bindings (ReBAC tuples + OPA/Rego rules where relevant)
- Explorer metadata (ownership, invariants, lineage edges)

**What it proves:** Agora doesn't replace the database — it **compiles** ontology changes into storage, contracts, APIs, and policy. One semantic change → many coherent operational artifacts. This is the "shared business meaning with operational consequences" thesis.

**Cannot be faked:** Generated code/schemas must be real and usable — the next beat depends on them.

---

### Beat 5 — Low-risk additive change is auto-approved and published

**On screen:** Because the proposal was additive, namespaced, did not expand visibility, did not change partitioning/residency, and passed all checks, Agora's agentic-approval threshold fires. The change merges, a new ontology version is published, and the generated artifacts go live. No human touched it.

**What it proves:** Agora **removes the central-team bottleneck for safe changes**. Agents can ship without waiting on humans, *because the protocol forced enough semantic discipline upfront to make automated approval trustworthy*.

**Cannot be faked:** The decision must come from the policy/threshold logic — not from a coin flip or a "demo mode" toggle.

---

### Beat 6 — A risky change is blocked and escalated

**On screen:** A second proposal is submitted. It looks innocuous on the surface (e.g., renames a field, "tightens" a type, or quietly widens read visibility). Agora's checks classify it as either a **semantic break**, a **policy expansion**, or a **temporal reinterpretation**. The agentic-approval path refuses; the proposal is escalated to a named human reviewer with a clear explanation of *why* and *what would have to change* for it to pass.

**What it proves:** Agora **catches the failure mode that breaks every shared-data system in practice**: the change that compiles cleanly, passes shape checks, and silently changes meaning, history interpretation, or who-can-see-what.

**Cannot be faked:** The check must explain the *kind* of risk in human-readable terms, not just print "blocked."

---

### Beat 7 — Writes flow through generated commands; reads show state + history

**On screen:** A real write happens via the generated gRPC command. We then issue two reads:
1. Current state of the `BankIntegration` entity (shows the new `AuthenticationMethod` relation populated).
2. Historical "what was true as of last week / what did we know last week" — answering both valid-time and transaction-time questions.

**What it proves:** Agora preserves **auditability and replayability** by construction. History is not a bolt-on; it's a property of going through the control plane. This is the demo beat that distinguishes Agora from "yet another schema registry."

**Cannot be faked:** The historical read must reflect a real append-only event/mutation log or temporal store, not a stub.

---

### Beat 8 — Explorer shows owner, invariants, lineage, policy, version history

**On screen:** A developer (or another agent) opens the explorer, navigates to `BankIntegration`, and sees in one view:
- Owner (`integrations-platform`) and semantic steward (`core-ontology`).
- Invariants ("Every active `BankIntegration` has at least one supported `AuthenticationMethod`").
- Lineage (which APIs expose it, which projections feed off it, which events emit/consume it).
- Policy attachments (who can read which fields, who can issue which commands).
- Version history (today's change, who/what proposed it, the diff, the check report).

**What it proves:** Discovery and trust are **first-class outputs of the control plane**, not someone's side-project Confluence page. An agent or human can find canonical concepts and APIs without human routing. The ontology, contracts, storage, policy, and history all share one navigable graph.

**Cannot be faked:** Every field shown must be backed by real registry data populated through beats 1–7.

---

## What ties the beats together (don't lose this)

The demo only works if the **same proposal threads through every beat**. Beat 1's `OntologyChangeProposal` is the artifact Beat 3 checks, Beat 4 compiles, Beat 5 approves, Beat 7 writes against, and Beat 8 shows the history of. If any beat is implemented as a standalone screen disconnected from the others, the thesis collapses.

Beat 6 (the blocked proposal) is the second thread. It is intentionally a *different* proposal so we can demonstrate the boundary between safe and unsafe in the same demo, without contaminating the happy-path thread.

---

## Time budget (target: 5 minutes)

| Beats | Time | Notes |
|---|---|---|
| 1–2 | 60s | "Here's the proposal. Notice Agora caught the duplicate." |
| 3 | 60s | The multi-axis check report. This is where judges learn what Agora *is*. |
| 4–5 | 45s | Generated artifacts; auto-approval fires. |
| 6 | 60s | The blocked proposal. Show the explanation, not just the rejection. |
| 7 | 45s | Real write, current read, historical read. |
| 8 | 30s | Explorer tour. Land the "one navigable graph" point. |

---

## Open questions (Documenter → PM and Architecture Lead)

These need answers before this script can be locked:

1. **Which domain do we model?** Bank integrations is the placeholder from the investigation. PM: confirm or replace.
2. **What is the duplicate concept for Beat 2?** Needs to be seeded in the ontology before the demo runs.
3. **What is the risky proposal for Beat 6?** Should be one of: silent semantic break, visibility expansion, or temporal reinterpretation. PM: pick one based on which is most legible to judges.
4. **Substrate for Beat 7's history?** Spanner mutation log + projections? Postgres + append-only event table? XTDB spike? Architecture Lead decides; the script doesn't change but the implementation surface does.
5. **Explorer UI in Beat 8?** Backstage plugin, custom thin app, or CLI walkthrough? Affects scope significantly.

---

## Living-doc rules

- Update this file whenever a beat's implementation detail changes.
- If we cut a beat for time, mark it `[CUT]` rather than deleting — keeps the rationale visible.
- Every commit that lands a beat should reference the beat number in the message.
