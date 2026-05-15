# Feature 2: Multi-Axis Risk Gate

**Estimate:** 3h implementation  
**Demo beats:** 3 (happy path checks), 5 (auto-approval), 6 (risky proposal block)  
**Status:** Spec locked (ready for impl-f2)

---

## What Feature 2 does

Takes a proposal JSON (output from Feature 1's artifact generation) and:
1. Runs 7-axis compatibility checks
2. Queries actual database for data-conformance validation
3. Generates a structured CheckReport with findings
4. Applies auto-approval threshold logic
5. Blocks risky changes with structured explanation

---

## Input

A `Proposal` JSON structure from Feature 1 (see Feature 1's `proposal.json` artifacts):
```json
{
  "id": "prop_8d79a31f20ca7497",
  "concept_fqn": "core.integrations.BankIntegration",
  "change_intent": "additive|refinement|breaking",
  "rationale": "...",
  "owned_by": "integrations-platform",
  "compatibility": {
    "shape": "additive|refinement|breaking",
    "semantic": "additive|refinement|breaking",
    "policy": "...",
    "temporal": "...",
    "api": "...",
    "storage": "..."
  },
  "structural_changes": [...],
  "field_classifications": {...},
  ...
}
```

---

## Output: CheckReport

```json
{
  "proposal_id": "...",
  "status": "approved|blocked",
  "auto_approval_eligible": true|false,
  "checks": [
    {
      "axis": "composition",
      "passed": true|false,
      "findings": "...",
      "evidence": {...}
    },
    {
      "axis": "compatibility.shape",
      "passed": true|false,
      "findings": "..."
    },
    ...
  ],
  "data_conformance": {
    "violations_found": 0,
    "sample_violations": [],
    "query_time_ms": 42
  },
  "block_reason": "optional — if status=blocked",
  "generated_at": "...",
  "version": 1
}
```

---

## The 7 Axes (in order of evaluation)

### 1. Composition Check
**Question:** Does the ontology still compose after this change?

**What to check:**
- New foreign keys point to existing entities
- If removing a required field, no dependent concepts rely on it
- Circular dependencies don't form
- Type incompatibilities don't introduce errors

**Can fail:** Yes, if structural integrity breaks  
**Demo consequence:** Happy path passes (additive field, no breaking changes)  
**Evidence:** Link graph, field dependency tree

---

### 2. Compatibility: Shape Axis
**Question:** Is the data model shape change backward-compatible?

**What to check:**
- Additive: new columns, new optional fields → always safe
- Refinement: tightening constraints (nullable→required, but with data validation)
- Breaking: removing columns, changing cardinality fundamentally

**Can fail:** Yes, if breaking change detected  
**Demo consequence:** Happy path is additive (new field_flag on BankIntegration)  
**Evidence:** Field delta, old vs new schema

---

### 3. Compatibility: Semantic Axis
**Question:** Does this change redefine or refine the meaning of the concept?

**What to check:**
- Is a field being semantically refined? (e.g., `optional string email` → `required string email with email format`)
- Does this overlap with existing concepts? (Beat 2's reuse detection output flows here)
- Is a relation cardinality changing? (1:N → N:N)
- Are partitioning or residency semantics changing?

**Can fail:** Yes, semantic changes that don't match the proposal's declared intent  
**Demo consequence:**
- Happy path: namespace-extend of BankIntegration (no semantic redefinition)
- Risky path: Email field going from optional to required is a semantic refinement that breaks existing data

**Evidence:** Semantic delta, overlapping concept list, cardinality change

---

### 4. Compatibility: Policy Axis
**Question:** Are access control boundaries changing?

**What to check:**
- Does field visibility expand? (Internal → external, or reads_by expanding)
- Does ownership change?
- Do field classifications change? (INTERNAL → PII or vice versa)

**Can fail:** Yes, if expanding visibility without explicit approval  
**Demo consequence:** Happy path: no policy change (additive field keeps internal classification)  
**Evidence:** Policy diff, classification changes

---

### 5. Compatibility: Temporal Axis
**Question:** Are time semantics (valid-time, transaction-time) changing?

**What to check:**
- Is a time-series relation being collapsed or expanded?
- Are historical semantics being reinterpreted?
- Does this affect audit trail correctness?

**Can fail:** Yes, if temporal reinterpretation breaks replaceability  
**Demo consequence:** Happy path: no temporal change  
**Evidence:** Temporal semantic diff

---

### 6. Compatibility: Impact Axis
**Question:** Which downstream artifacts/concepts are affected?

**What to check:**
- Query the registry's lineage: what else references this concept?
- List affected HTTP handlers, API contracts, projections
- Count downstream consumers (could be many — useful for risk scoring)

**Cannot fail** (it's informational), but flags high-impact changes  
**Demo consequence:** Shows downstream artifacts affected by the change  
**Evidence:** Real lineage query results from registry DB

---

### 7. Data Conformance Check (Feature 2's key proof)
**Question:** Can existing data survive this proposal?

**This is the critical one for Beat 6.**

**What to check:**
- If proposal tightens a constraint (e.g., nullable→required):
  - Query the actual database table for violations
  - `SELECT COUNT(*) FROM Account WHERE email IS NULL`
  - If count > 0, the proposal fails data-conformance
- Generate a structured violation report with sample rows
- Flag whether proposal needs a backfill migration plan

**Can fail:** Yes — this is how Beat 6's account.email NULL rows block the proposal  
**Demo consequence:**
- Happy path: proposal adds new field (no conformance risk)
- Risky path: Account.email NULL rows exist → propose `tighten email from optional to required` → query returns 47 rows → block with explanation

**Evidence:** Actual row counts, sample violating rows, query time

---

## Auto-Approval Threshold Logic (Beat 5)

A proposal is eligible for auto-approval if:
- All 7 axes pass (or are informational)
- Data-conformance violations: 0
- No semantic refinement that tightens invariants
- Policy axis didn't expand visibility
- Temporal axis didn't reinterpret history

**Output:** `auto_approval_eligible: true` → proposal auto-merges, generates artifacts, publishes new ontology version

**Demo consequence:** Happy path gets auto-approved and published; risky path does not.

---

## Implementation Notes

### Database Integration
- Connect to Postgres (via sqlx)
- For data-conformance: run actual SQL queries against target tables
- Pre-seed Account table with 47 NULL rows in email field before demo

### Error Handling
- If schema doesn't exist yet: graceful no-op (data-conformance passes trivially)
- If Postgres unreachable: report error clearly (don't block demo)

### Performance
- Aim for <500ms total check time for demo fluidity
- Cache registry lineage if possible
- Parallelize axis checks where safe

### Integration with Feature 1
- Consume `proposal.json` artifacts from Feature 1's `generated/` directory
- Use structural changes and compatibility markers from the proposal
- Emit CheckReport as JSON to stdout (or to `generated/{proposal_id}/check_report.json`)

### Integration with Feature 3
- Feature 3's Explorer will consume CheckReports and display them
- CheckReport becomes a first-class registry artifact with version history

---

## Test Coverage (for val-f2)

**Happy path:**
- Propose additive field → all axes pass → auto-approval eligible → report emitted
- Verify proposal.json is consumed correctly
- Verify CheckReport structure is valid

**Risky path:**
- Propose semantic refinement (Account.email optional→required)
- Query returns 47 NULL rows
- Proposal blocked with clear explanation
- CheckReport shows violation samples

**Edge cases:**
- Proposal references non-existent schema (gracefully handled)
- Empty database (no violation, passes)
- Malformed proposal JSON (error reported)

---

## Artifacts & Outputs

### Code Structure (impl-f2's responsibility)
- `src/check.rs` — 7-axis check engine (core logic)
- `src/data_conformance.rs` — SQL query logic for violations
- `src/auto_approval.rs` — threshold logic
- `src/check_report.rs` — CheckReport struct and serialization
- `src/main.rs` — CLI entry point: `agora check <proposal.json>`

### Generated Artifacts
- `generated/{proposal_id}/check_report.json` — structured check findings

---

## Success Criteria for val-f2

1. Happy path proposal → report shows all axes passing
2. Risky proposal → report shows data-conformance failure with violation count
3. CheckReport is valid JSON with all required fields
4. Evidence fields contain real data (not stubs or placeholders)
5. Time to complete: <500ms even with DB query

---

## Unknowns / Architecture Clarifications Needed

1. **Database init:** Does impl-f2 set up Account/BankIntegration/AuthenticationMethod schema, or is it pre-seeded?
2. **Postgres access:** Connection string / .env handling?
3. **Lineage storage:** Where is the registry's lineage graph stored? (Memory? Registry DB?)
4. **Violation samples:** How many sample rows to include in violation report?

(Coordinator is asking Architecture Lead for database setup sequencing.)

---

## Timeline
- Implementation: 3h
- Validation: 1h (happy + risky paths, edge cases)
- Integration: 30m
- Total: 4h 30m

**Ready to start once Feature 1 integrates.**
