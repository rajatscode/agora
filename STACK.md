# Agora Stack — Locked Decisions

**Status:** Locked for hackathon build. Update only by Architecture sign-off.
**Date:** 2026-05-14
**Window:** 9h build; 8 person team across 4 workstreams.

This document is the single source of truth for what we are building and the contracts each workstream commits to. Read it before writing code. Reference it during integration.

---

## Stack at a glance

| Layer | Choice | Notes |
|---|---|---|
| Language | Rust 2021 | Single crate, no workspace split (cargo compile-time discipline) |
| HTTP server | `axum` 0.7 | All APIs are HTTP/JSON. **No gRPC at runtime.** |
| Async runtime | `tokio` 1 (full) | — |
| Database | PostgreSQL 16 + `pgvector` | Single instance, docker-compose. Registry, fact-log, embeddings all in one DB. |
| DB driver | `sqlx` 0.7 (postgres, json, uuid, chrono) | Compile-time checked queries where practical. |
| Protobuf decode (runtime) | `prost` 0.12 + `prost-types` 0.12 | We **accept** real `.proto`-encoded requests on ingest paths and decode them. We do **not** serve gRPC. |
| Protobuf build-time | `prost-build` 0.12 | Compiles canonical `.proto` files (emitted by our compiler workstream) into Rust types used by ingest handlers. |
| Authorization | OpenFGA in Docker | Real ReBAC container. Model authored by WS-B from ontology classifications; enforced by WS-D. |
| Policy engine | **JSON policy spec + in-Rust evaluator** | See "Policy spec format" below. No Rego, no `regorus`. |
| LLM | `anthropic-sdk` 0.1 | Used by WS-A (authoring) and WS-C LLM-axis (semantic, temporal, reuse verdict). Single shared client wrapper. |
| Explorer UI | Server-rendered HTML from Axum + HTMX | Per team-lead stack pick. Concept graph via static SVG if interactive becomes a fight. |
| Logging | `tracing` + `tracing-subscriber` | — |
| Errors | `anyhow` (app) + `thiserror` (lib boundaries) | — |

### What we explicitly do not use

- ❌ `tonic` (gRPC server) — APIs are HTTP/Axum; no consumer for tonic at runtime
- ❌ `tonic-build` — useless without `tonic`
- ❌ `regorus` / Rego — replaced by JSON policy spec + in-Rust evaluator
- ❌ Kafka / Pub/Sub — fact-log is an append-only Postgres table; `LISTEN/NOTIFY` if we need pushes
- ❌ GraphQL / Apollo / Hive — REST is enough for the explorer
- ❌ TerminusDB, XTDB, Spanner, CockroachDB — Postgres carries M0; M1 migration paths documented separately
- ❌ Confluent Schema Registry — `buf` CLI handles lint + breaking-change checks against canonical `.proto` files in repo

---

## The four locked artifacts

These are the contracts every workstream commits to at hour 0:30. **Do not change them without an all-hands sync.** Diffs to these break the build.

### Artifact 1 — Ontology AST

Owned by: **WS-B (AST→diff lead)**. Consumed by all workstreams.

```rust
pub struct OntologyType {
    pub namespace: String,            // "core.integrations"
    pub name: String,                 // "BankIntegration"
    pub version: u32,                 // monotonic per (namespace, name)
    pub fields: Vec<Field>,
    pub relations: Vec<Relation>,
    pub invariants: Vec<String>,      // human-readable; LLM-checked, not enforced
    pub ownership: Ownership,
    pub policy_class: PolicyClass,    // Public | Internal | Sensitive | Pii
    pub locality: Option<String>,     // partition-key field — M1 readiness signal
}

pub struct Field {
    pub name: String,
    pub proto_type: ProtoType,        // String | Int64 | Bytes | Timestamp | Ref(TypeRef)
    pub proto_number: u32,
    pub required: bool,
    pub since_version: u32,
    pub deprecated_in: Option<u32>,
    pub classification: PolicyClass,  // field-level; propagates through all artifacts
}

pub struct Relation {
    pub name: String,
    pub from: TypeRef,
    pub to: TypeRef,
    pub cardinality: Cardinality,
    pub since_version: u32,
}

pub enum Change {
    AddField { type_ref: TypeRef, field: Field },
    AddRelation { relation: Relation },
    DeprecateField { type_ref: TypeRef, field_name: String },
    ReclassifyField { type_ref: TypeRef, field_name: String, to: PolicyClass },
    CreateType { spec: OntologyType },
}

pub struct OntologyChangeProposal {
    pub id: ProposalId,
    pub author: String,               // "agent://..." or "user://..."
    pub change: Change,
    pub rationale: String,
    pub semantic_contract: SemanticContract,  // meaning_before, meaning_after
}
```

Lives in `src/ast.rs`. Serialization: `serde_json` (canonical JSON).

### Artifact 2 — Diff schema

Owned by: **WS-B (AST→diff lead)** and **WS-C (LLM-axis lead)** jointly. Frozen at hour 0:30. **This is the keystone contract.**

The diff is what the compiler emits and what the risk gate classifies. It must carry enough information for all five axes:

```rust
pub struct OntologyDiff {
    pub proposal_id: ProposalId,
    pub target: TypeRef,
    pub kind: DiffKind,                 // AddField | AddRelation | Deprecate | Reclassify | CreateType
    pub field_changes: Vec<FieldChange>,
    pub relation_changes: Vec<RelationChange>,
    pub type_level: TypeLevelChange,    // locality, ownership, policy_class
    pub semantic_contract: SemanticContract,
}

pub struct FieldChange {
    pub name: String,
    pub before: Option<Field>,          // None for additions
    pub after: Option<Field>,           // None for deletions
    pub classification_delta: Option<(PolicyClass, PolicyClass)>,  // for Reclassify
    pub type_delta: Option<(ProtoType, ProtoType)>,
    pub since_version_delta: Option<(u32, u32)>,
}
```

What each consumer needs from this diff:
- **Shape axis** (`buf breaking`): runs against emitted `.proto`, not this diff. The diff doesn't drive it.
- **Semantic axis** (LLM): reads `semantic_contract`, `field_changes`, `relation_changes`
- **Temporal axis**: reads `since_version_delta`, `deprecated_in` changes, type_delta
- **Policy axis** (JSON policy spec): reads `classification_delta`, `type_level.policy_class`
- **Operational axis**: reads `type_level.locality`, `type_level.ownership`

Lives in `src/diff.rs`.

### Artifact 3 — Classification-propagation contract

Owned by: **WS-B (diff→artifacts lead)** emits; **WS-D (runtime lead)** enforces.

For every value of `PolicyClass`, this is what gets emitted and enforced. The list is exhaustive — extending it requires sync.

| Field `classification` | `.proto` option emitted (B) | DDL column treatment (B) | OpenFGA tuple emitted (B) | Runtime enforcement (D) |
|---|---|---|---|---|
| `Public` | `[(agora.classification) = "PUBLIC"]` | Standard column | None — visible to all | None |
| `Internal` | `[(agora.classification) = "INTERNAL"]` | Standard column | `{type}:{id}#internal_viewer@team:*` | Reject anonymous calls |
| `Sensitive` | `[(agora.classification) = "SENSITIVE"]` | Standard column + security label comment | `{type}:{id}#sensitive_viewer@team:{owner}` | Check OpenFGA before return; cite rule in 403 |
| `Pii` | `[(agora.classification) = "PII"]` | Standard column + security label comment | `{type}:{id}#pii_viewer@role:dpo` | Check OpenFGA; mask in fact-log payloads; audit access |

The OpenFGA model is also generated by WS-B from the set of classifications in use (one relation per class). It is **not** hand-authored.

### Artifact 4 — `mutation_log` row schema (with `ontology_version`)

Owned by: **WS-D (runtime lead)** for the table; **WS-B (diff→artifacts lead)** for what generated command handlers write.

```sql
CREATE TABLE mutation_log (
  seq               BIGSERIAL PRIMARY KEY,
  type_id           UUID NOT NULL REFERENCES ontology_types(id),
  ontology_version  INT NOT NULL,             -- snapshot of the type version at write time
  entity_id         TEXT NOT NULL,
  command           TEXT NOT NULL,            -- 'Create' | 'Update' | 'Deprecate'
  payload           JSONB NOT NULL,           -- canonical entity state after the command
  payload_proto_b64 TEXT,                     -- optional: protobuf wire if we accepted .proto input
  actor             TEXT NOT NULL,
  occurred_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ON mutation_log (type_id, occurred_at);
```

**Why `ontology_version` matters:** beat 8 of the demo ("replay as of T-5min") needs to reinterpret historical payloads against the ontology version that was active at write time. Without this column, history is uninterpretable after schema evolution.

Generated command handlers (WS-B output) **must** include the current `ontology_version` when inserting into this table. This is enforced by code review, not schema.

---

## Policy spec format

Replaces Rego entirely. Policies are JSON documents in `policies/*.json`. The evaluator is hand-rolled Rust in `src/policy.rs`. **Total: ~150 lines of Rust + N JSON files.**

### Schema

```json
{
  "id": "no_sensitivity_downgrade",
  "description": "Reclassifying a field to a less-restrictive class requires security-owner approval.",
  "scope": "diff",
  "match": {
    "diff_kind": "Reclassify",
    "classification_delta_direction": "downgrade"
  },
  "verdict": {
    "severity": "high",
    "decision": "block",
    "reason": "Cannot downgrade classification of {field_path} from {from_class} to {to_class} without explicit security-owner approval.",
    "escalate_to": "team:security-platform"
  }
}
```

### Evaluator contract (in-Rust)

```rust
pub fn evaluate(diff: &OntologyDiff, policies: &[Policy]) -> Vec<PolicyVerdict>;

pub struct PolicyVerdict {
    pub policy_id: String,
    pub axis: Axis,                    // always Policy for these
    pub severity: Severity,            // Low | Medium | High
    pub decision: Decision,            // Allow | Warn | Block | Escalate
    pub rendered_reason: String,       // template variables substituted
}
```

### Match predicates supported (M0)

- `diff_kind`: exact match on `DiffKind`
- `classification_delta_direction`: `upgrade` | `downgrade` | `any`
- `affects_field_with_class`: matches if any field-change involves this class
- `type_level_field_changed`: e.g. `locality`, `ownership`

That's the entire M0 policy DSL. Five JSON files cover the demo:

| File | What it gates |
|---|---|
| `no_sensitivity_downgrade.json` | Reclassify to less-restrictive class → block |
| `pii_requires_dpo_approval.json` | Any change involving `Pii` field → escalate to DPO |
| `locality_change_blocks.json` | `type_level.locality` change → block (would require data migration) |
| `additive_field_auto_approve.json` | `AddField` with `Internal`/`Public` class → allow |
| `ownership_change_warns.json` | Ownership change without security owner sign-off → warn |

WS-C (deterministic-axis lead) writes the evaluator + the five JSON files. WS-D surfaces the verdicts in the check report UI with the rule ID cited.

### Why JSON + Rust instead of Rego

- Faster to demo (no Rego learning curve in the room)
- Easier to debug (Rust stack traces, not Rego policy-eval traces)
- Easier to extend (add a match predicate = add a Rust match arm)
- Honest claim: "policy-as-data" not "policy-as-code." We don't pretend to be OPA.
- The 5-axis novelty claim (line 314) is preserved — policy is still a distinct axis with cited rules.

The M1 migration path is "swap evaluator for OPA HTTP eval; JSON specs become Rego rules." That migration is a week of work, not a hackathon ambition.

---

## Workstream → role mapping

See team-lead's roster for actual assignments. This is the structural map:

| Workstream | Owns | Headcount | Critical-path? |
|---|---|---|---|
| WS-A — Authoring CLI | LLM proposal authoring; produces `OntologyChangeProposal` JSON | 1 | No (parallel to B) |
| WS-B — Compiler | (i) AST→diff; (ii) diff→artifacts (`.proto`, DDL, OpenFGA tuples, command-handler skeleton) | 2 | Yes (longest pod) |
| WS-C — Risk gate | (i) LLM axes (semantic, temporal, reuse); (ii) Deterministic axes (shape via `buf breaking`, policy via JSON evaluator, operational via AST inspection) | 2 | After B's diff lands |
| WS-D — Explorer + runtime | (i) HTMX UI + concept graph + check report + codegen panel; (ii) Fact-log writes + OpenFGA enforcement + ontology_version stamping | 2 | Final consumer |
| Integration + demo + seed | Contract tests at every boundary; seed ontology; demo CLI script; fallback recordings | 1 | Continuous |

### Hard sync points

| Time | Who | Purpose |
|---|---|---|
| 0:00–0:30 | All 8 | Lock the four artifacts above; confirm Cargo.toml; agree on directory layout |
| 0:30–0:45 | WS-A lead + WS-C LLM lead | Align on shared `anthropic-sdk` wrapper + structured-output prompt scaffold |
| 1:00 | WS-B (artifacts) + WS-D (runtime) | Confirm OpenFGA tuple format + model emission protocol |
| 3:00 | All | Checkpoint 1: stub flow — `GET /concepts/{ns}/{name}` renders in UI |
| 6:00 | All | Checkpoint 2: Proposal A flows end-to-end (auto-approve + codegen visible) |
| 8:00 | All | Checkpoint 3: Proposal B blocks visibly; full demo runs |

---

## Cross-cutting design rules

1. **Single crate.** No workspace split. `cargo check` for inner loop, `cargo build --release` only at integration moments. Rust compile times are the silent hackathon killer; minimize their surface.
2. **All shared types in `src/ast.rs` and `src/diff.rs`.** No workstream re-declares them.
3. **All HTTP handlers in `src/api/`.** Pod boundaries map to module boundaries, not crate boundaries.
4. **All generated artifacts written to `generated/{proposal_id}/`.** Persisted in `generated_artifacts` table too, so the UI can render them on demand.
5. **`X-Agora-Actor` header** is the stand-in for identity throughout the demo. No real auth.
6. **Classification metadata propagates everywhere.** If you add an artifact emitter (e.g. for OpenAPI later), it must read `PolicyClass` and emit corresponding constraints. The unified-permission claim depends on this discipline.

---

## What we ship vs. what we honestly defer (M1)

| M0 (this build) | M1 path (slide, not code) |
|---|---|
| Postgres registry + fact-log | Spanner canonical store; `locality` field consumed by interleaved tables |
| JSONB payloads | PROTO column type in Spanner; `prost`-decoded ingest stays the same |
| pgvector reuse detection | Same, with richer embeddings + dedicated vector store |
| In-Rust policy evaluator + JSON specs | OPA HTTP eval; JSON specs → Rego (a week of porting) |
| OpenFGA in Docker | OpenFGA managed or SpiceDB at scale; model emission stays automated |
| `mutation_log` Postgres table | Pub/Sub topic with same row shape; consumers swap producer side only |
| Single namespace | Namespace promotion workflow (M2 spike) |
| HTMX explorer | Same UX patterns, richer interactive graph (e.g., Cytoscape.js) if budget allows |

The honest claim is **"the control-plane code is portable; the substrate swap is real engineering, days-to-weeks per piece."** We do not claim "one-line config change to Spanner." We do not claim multi-region. We claim that the abstractions we built — the AST, the diff, the propagation contract, the policy spec — survive the substrate swap. That is the M0 thesis, and it is provable today.
