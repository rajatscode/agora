# Impl-F2 Brief: Multi-Axis Risk Gate

**Time:** Implementation starts after F1 integration (~02:00 ET). **3-hour window.**

---

## What You're Building

The risk gate: takes a `Proposal` JSON and runs **7-axis compatibility checks**, then returns a `CheckReport` that either:
1. Clears the proposal for auto-approval (all axes green, data-conformant), or
2. Blocks it with structured explanation (axis FAILED, violation count/samples)

**Critical insight:** The semantic axis is the hardest. See SEMANTIC-PRIMER (already sent to Researcher); TL;DR: shape-check says "fine", but optional→required breaks 47 rows of real data. You must query the DB.

---

## Spec You're Building From

`FEATURE-2-SPEC.md` is locked. Skim it for:
- Input: Proposal JSON (11 fields + `change` + structural changes)
- Output: CheckReport (status, checks[], data_conformance{}, block_reason)
- 7 axes: composition, shape, semantic, policy, temporal, impact, replay
- **Data-conformance:** Run actual SQL queries; report violation counts + samples
- **Auto-approval threshold:** All axes pass AND (no violations AND classification ∈ {additive, refinement})

---

## Database Setup (ASK FOR CLARIFICATION)

**Architecture Lead is answering this now.** Once they respond, you'll know:
- Are Account/BankIntegration/AuthenticationMethod tables pre-seeded?
- Do you need to create them or does F1's SQL generation handle it?
- Where is the ontology registry stored? (Same DB? Different DB? In-memory?)

**Don't get blocked.** If the answer is still unclear when you start, reach out to Architecture directly. They're a resource.

---

## Implementation Road Map (3h Budget)

### Phase 1: Check Engine Skeleton (30 min)
- `src/check.rs` — core struct: `pub fn check(proposal: &Proposal, db: &PgPool) -> Result<CheckReport>`
- `src/check_report.rs` — CheckReport struct with serde serialization
- Split the 7 axes into separate functions (one per file or module)
- **Entry point:** `cargo run -- check ./generated/prop_<id>/proposal.json` outputs JSON to stdout

**Deliverable:** Build succeeds, check compiles, stdin/stdout wired.

### Phase 2: Data-Conformance Axis (1h)
This is the proof point. **Build this first; it's the hardest and most critical.**

- `src/data_conformance.rs`:
  - Takes a Proposal + Db connection
  - If proposal has a "tighten" change (nullable→required, etc.), run a query:
    - `SELECT entity_id, <field> FROM <table> WHERE <field> IS NULL OR <condition>`
  - Count violations + collect sample rows
  - Return `DataConformanceCheck { passed: bool, violations: Vec<Violation>, query_time_ms: u64 }`

**Test case (critical):** Propose semantic refinement of Account.email (optional→required), query returns 47 rows, CheckReport shows violation count and sample entity_ids.

**Deliverable:** Real SQL queries against the DB, real violation counts, <500ms.

### Phase 3: Shape Axis (30 min)
- `src/shape_compat.rs`:
  - Parse the proposal's structural changes
  - Classify each as additive (add column), refinement (widen type), or breaking (remove column)
  - Return axis status + findings

**Simple heuristic:** 
- AddColumn → additive
- RemoveColumn → breaking
- ChangeType → refinement or breaking (depends on whether it narrows or widens)

**Deliverable:** Axis emits classified findings.

### Phase 4: Remaining Axes — Template Mode (1h)
- `src/composition.rs` — check: new entities don't clash, imports resolve
- `src/semantic_compat.rs` — check: overlapping concepts, invariant changes (feeds data-conformance findings)
- `src/policy_compat.rs` — check: visibility expanding, classifications changing
- `src/temporal_compat.rs` — check: time semantics reinterpreted (M0 is append-only, so mostly "no change")
- `src/impact_compat.rs` — check: downstream artifacts affected (lineage query)
- `src/replay_compat.rs` — check: projections can rebuild

**Pragmatic approach:** For each axis, implement the "happy path" (no changes, axis passes) + the "risky path" that matters for the demo:
- Composition: happy = pass
- Shape: additive = pass, breaking = fail
- **Semantic:** data-conformance violation = fail (you already built this)
- Policy: visibility-expand = fail
- Temporal: mostly pass (M0 deferral)
- Impact: informational (doesn't fail)
- Replay: informational (doesn't fail)

**Deliverable:** All 7 axes emit CheckReport rows.

### Phase 5: Auto-Approval Threshold (15 min)
- `src/auto_approval.rs`:
  ```rust
  pub fn is_eligible_for_auto_approval(report: &CheckReport) -> bool {
    report.checks.iter().all(|c| c.passed) &&
    report.data_conformance.violations_found == 0 &&
    ["additive", "refinement", "no_change"].contains(&proposal.classification)
  }
  ```

**Deliverable:** CheckReport includes `auto_approval_eligible: bool`.

### Phase 6: Integration + Fallbacks (15 min)
- Main entry point handles: missing DB connection (graceful error), malformed proposal JSON (structured error), schema doesn't exist (no-op, pass)
- Tracing/logging goes to stderr (don't contaminate JSON output)
- All errors are recoverable; never `.unwrap()` or `.panic!()`

**Deliverable:** Full CLI integration, `agora check <proposal.json>` works end-to-end.

---

## Test Coverage (for val-f2)

The validator will test:
1. **Additive proposal** → all axes pass, auto_approval_eligible = true
2. **Semantic refinement** → data-conformance violation (47 rows), block with explanation
3. **Each axis individually** → at least 3-4 of 7 probed separately
4. **Performance** → <500ms including DB queries
5. **Edge cases** → missing schema (graceful), empty DB (pass), malformed JSON (error)

**Your job:** Implement so these tests all pass. Validator will be thorough.

---

## Resources

- **FEATURE-2-SPEC.md** — full specification (locked)
- **SEMANTIC-PRIMER** (from Researcher) — understand why semantic ≠ shape
- **qa-lead's test fixtures** — 47-row Account.email seeding, falsification tests
- **test-reviewer's validation strategy** — what falsification means

---

## Questions to Ask

- **Architecture:** Where is the ontology registry stored? Do you create the tables or are they pre-seeded?
- **Researcher:** If you hit temporal questions (valid-time, reinterpretation), they're available
- **qa-lead:** If you need test data or data-conformance query help

---

## Red Flags (When to Ask for Help)

- "How do I query for violations?" → qa-lead has examples
- "What does 'semantic refinement' mean?" → Researcher has the primer
- "Should this axis block or warn?" → Spec says (review FEATURE-2-SPEC.md)
- "Stuck on time" → Let me know ASAP so we can cut scope surgically

---

## Success Criteria

1. ✓ `agora check` CLI runs on a proposal JSON
2. ✓ Data-conformance axis queries the DB and reports 47-row violations
3. ✓ All 7 axes emit findings (happy path: pass, risky path: fail correctly)
4. ✓ Auto-approval threshold is clear in the output
5. ✓ Build succeeds, no `.unwrap()` in production paths

---

## Known Constraints

- **Database setup TBD** (Architecture answering)
- **Lineage graph storage TBD** (for impact axis, probably manual for now)
- **No bitemporality** (M0 append-only, not M1)

---

## Starting Point

1. Make sure Feature 1 has integrated (F1 artifacts + handlers exist)
2. Confirm database setup answer from Architecture
3. Clone the repo (main branch will have F1 integrated)
4. `cargo new` a new feature branch or use existing structure
5. Start with Phase 2 (data-conformance) first — it's the hardest and most critical

**Go build.** You have the spec, the brief, and the team support. 3h is tight but achievable if you stay focused on the 7 axes (happy + demo-critical risky paths) and skip over-optimization.
