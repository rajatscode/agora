# Agora Demo Dry-Run Checklist

**Target:** All 8 beats flowing end-to-end with real data, real outputs, no stubs.  
**Duration:** ~5m 30s (from DEMO.md time budget)  
**When:** 07:30 ET (after all features integrate, before final push)

---

## Pre-Demo Setup (Must be ready before Beat 1 starts)

### Database & Registry State
- [ ] PostgreSQL running locally (or connection string set)
- [ ] Ontology registry tables created and initialized
- [ ] Pre-seeded concepts in registry:
  - [ ] `core.integrations.BankIntegration` (canonical entity)
  - [ ] `core.integrations.AuthenticationMethod` (canonical concept, for reuse detection)
  - [ ] `core.Account` with schema (entity_id, email NOT NULL should fail, for Beat 6)
- [ ] Account table seeded with **47 rows where email IS NULL** (critical for Beat 6)
- [ ] Mutation log table created (empty initially)
- [ ] Connection pool configured in Agora app

### Agora Services
- [ ] `cargo build` succeeds (all 3 features integrated)
- [ ] Binary location: `target/release/agora` (or dev target)
- [ ] Environment variables set:
  - [ ] `ANTHROPIC_API_KEY` (if real Anthropic call, otherwise offline fallback)
  - [ ] `DATABASE_URL` pointing to the seeded Postgres
  - [ ] `REGISTRY_PATH` or database connection for ontology registry

### CLI Commands Available
- [ ] `agora propose "<request>"` — Feature 1 (CLI working, LLM reasoning)
- [ ] `agora check <proposal.json>` — Feature 2 (Check engine)
- [ ] `agora verify` — Feature 3 (Tampering detection)
- [ ] `agora explorer <concept_fqn>` — Feature 3 (Discovery UI)

### Generated Artifacts Directory
- [ ] `./generated/` directory writable
- [ ] Old proposal artifacts cleaned out (or versioned with proposal IDs)

---

## Beat 1: A real LLM authors and submits a proposal (60s)

**Setup:**
- [ ] Dev has a terminal open, ready to run CLI
- [ ] Anthropic API key is valid (or offline fallback confirmed)

**Actions:**
1. [ ] Run: `cargo run -- propose "we need to model what each bank integration can do — supported features, rate limits, etc."`
2. [ ] Wait for LLM call (~10-15s, or instant with offline fallback)
3. [ ] Observe real `OntologyChangeProposal` JSON generated in terminal
4. [ ] Proposal should classify as `Refinement` or `Additive` against `AuthenticationMethod`

**Outputs to verify:**
- [ ] Proposal JSON with all 11 fields (id, domain, namespace, target_version, change_intent, rationale, owned_by, classifications, compatibility, structural_changes, field_classifications)
- [ ] Terminal shows: "Proposal ID: prop_<hash>" and file path to generated proposal.json
- [ ] Proposal JSON is valid (can parse it for next beat)

**Proof point:** Real LLM call, not pre-baked. ✓

---

## Beat 2: Agora detects reuse vs. duplication (30s)

**Input:** proposal.json from Beat 1

**Actions:**
1. [ ] System auto-runs reuse detection when proposal is ingested
2. [ ] Terminal/output shows: "Concept overlap detected: `BankIntegrationCapability` overlaps with `AuthenticationMethod` (similarity: 0.74)"
3. [ ] Proposer decision shown: "**Decision: namespace-extend** — `BankIntegrationCapability` extends core.integrations namespace without redefining `AuthenticationMethod`"

**Outputs to verify:**
- [ ] Overlap is machine-detected (not hardcoded)
- [ ] Similarity score is computed (from embeddings or Jaccard)
- [ ] Overlap is recorded in proposal's lineage
- [ ] Decision (namespace-extend vs refine vs deprecate) is persisted

**Proof point:** Reuse detection is automatic and enforced. ✓

---

## Beat 3: Multi-axis checks run on the proposal (45s)

**Input:** proposal.json (with decision from Beat 2)

**Actions:**
1. [ ] Run: `cargo run -- check ./generated/prop_<id>/proposal.json`
2. [ ] Check report is generated and displayed (or written to file)

**Check report must show:**
- [ ] **Composition:** ✓ (ontology still composes)
- [ ] **Compatibility.Shape:** Additive (new field, no existing columns removed)
- [ ] **Compatibility.Semantic:** Additive (no meaning change on existing fields)
- [ ] **Compatibility.Policy:** Additive (policy classifications unchanged)
- [ ] **Compatibility.Temporal:** No change (M0 append-only, no temporal reinterpretation)
- [ ] **Impact:** Shows downstream artifacts affected (HTTP handlers, policy specs)
- [ ] **Replay:** ✓ (projections can rebuild)

**For BankIntegrationCapability proposal:**
- [ ] Every axis: Additive or Refinement (no breaking)
- [ ] Auto-approval threshold: ELIGIBLE

**Proof point:** Multi-axis compatibility checking, not just shape. ✓

---

## Beat 4: Agora generates four real operational artifacts (30s)

**Input:** proposal.json

**Actions:**
1. [ ] Check that artifacts exist on disk: `ls -la ./generated/prop_<id>/`

**Artifacts to verify (all 4 must exist):**
- [ ] **bank_integration.proto** — Real protobuf with message definition, inheritance, field metadata
- [ ] **bank_integration.sql** — Real DDL (ALTER TABLE or CREATE TABLE) with column definitions
- [ ] **bank_integration_handler.rs** — Real Axum HTTP handler with route, struct definitions, function signature
- [ ] **bank_integration.fga.json** — Real OpenFGA policy spec with relations and tuples

**Proof point:** All 4 artifacts are real, on-disk, and usable. ✓

---

## Beat 5: The additive proposal is auto-approved and published (15s)

**Input:** Check report (from Beat 3) with all axes passing

**Actions:**
1. [ ] Check report is consulted for auto-approval decision
2. [ ] System outputs: "Auto-approval threshold met. Proposal merging..."
3. [ ] New ontology version published (v2 or v3)
4. [ ] Artifacts moved from staged to published location (or marked as live)

**Proof point:** Safe changes auto-approve without human approval. ✓

---

## Beat 6: A risky proposal is blocked (60s)

**Setup:**
- [ ] Account table has 47 rows with email IS NULL (confirmed in database)

**Actions:**
1. [ ] Create a second proposal file (risky_proposal.json) that proposes: `Refine Account.email from optional to required`
2. [ ] Run: `cargo run -- check ./risky_proposal.json`
3. [ ] Check report is generated

**Check report must show:**
- [ ] **Semantic Axis:** Refinement (tightens invariant)
- [ ] **Data-Conformance:** ✗ FAIL (violations found)
  - [ ] Violation count: 47
  - [ ] Sample violations listed (entity_ids of rows with email IS NULL)
  - [ ] SQL query time: <500ms

**Terminal output:**
```
✗ PROPOSAL BLOCKED
Reason: Data-conformance violation
Semantic axis: Refinement (tightens invariant on Account.email)
Violations found: 47 rows in Account where email IS NULL
Example violating rows: [ba-123, ba-456, ...]
To proceed: backfill missing email values or revise constraint to optional
```

**Proof point:** Real DB query detects real data violation. ✓

---

## Beat 7: Writes flow through generated commands; tampering is caught (60s)

### Sub-step 1: Happy Write (20s)

**Setup:**
- [ ] HTTP server running (routes from Feature 1 handlers plumbed in)
- [ ] Terminal open with curl or similar

**Actions:**
1. [ ] POST to `/entities/bank_integration` with valid data:
   ```
   curl -X POST http://localhost:3000/entities/bank_integration \
     -H "Content-Type: application/json" \
     -d '{"entity_id":"bi-acme","provider_name":"ACME Corp"}'
   ```
2. [ ] Server returns 201 Created with mutation_id and ontology_version
3. [ ] Verify write in database: `SELECT * FROM core_integrations_bank_integration WHERE entity_id='bi-acme';`
4. [ ] Verify mutation_log entry created

**Outputs:**
- [ ] Response: `{ "entity_id": "bi-acme", "mutation_id": "mut-abc123", "ontology_version": 2 }`
- [ ] Database row exists
- [ ] mutation_log row exists with matching checksum

**Proof point:** Writes are logged with ontology version stamps. ✓

### Sub-step 2: Out-of-band Tampering (15s)

**Actions (from a separate terminal/connection):**
1. [ ] Issue raw SQL UPDATE, bypassing the HTTP handler:
   ```sql
   UPDATE core_integrations_bank_integration 
   SET provider_url = 'https://evil.bank' 
   WHERE entity_id = 'bi-acme';
   ```
2. [ ] Do NOT log this in mutation_log (intentional bypass)

**State after tampering:**
- [ ] Database row has modified provider_url
- [ ] mutation_log has NO entry for this change
- [ ] System doesn't know the change happened (yet)

### Sub-step 3: `agora verify` Catches It (25s)

**Actions:**
1. [ ] Run: `agora verify`
2. [ ] Tamper detection runs, comparing DB state to mutation_log

**Output:**
```
✗ TAMPERING DETECTED

Tampered Entity: bi-acme
  Type: core.integrations.BankIntegration
  Issue: Drift detected
  Field changed: provider_url
  
  Expected state (from mutation log mut-abc123):
    provider_url: "https://acme.bank"
    
  Current state (from database):
    provider_url: "https://evil.bank"
    
  Last logged mutation: mut-abc123 at 2026-05-14T23:07:00Z
  
Conclusion: Row was modified outside the control plane. Integrity compromised.
```

**Proof point:** Auditability and integrity are enforced by construction. ✓

---

## Beat 8: Explorer shows owner, invariants, lineage, policy, version history (30s)

**Actions:**
1. [ ] Run: `agora explorer core.integrations.BankIntegration`

**Explorer output shows all of:**
- [ ] **Owner:** `integrations-platform`
- [ ] **Semantic steward:** (if applicable)
- [ ] **Status:** Active
- [ ] **Invariants:** (if any defined)
- [ ] **Lineage:**
  - [ ] HTTP handler: `/entities/bank_integration` (POST)
  - [ ] Related concept: `AuthenticationMethod` (1:N relationship)
  - [ ] Generated by: BankIntegrationCapability proposal
  - [ ] Policy: `bank_integration.fga.json`
- [ ] **Policy Attachments:**
  - [ ] internal_viewer: team:*
  - [ ] owner: team:integrations-platform
  - [ ] (other classification-based rules)
- [ ] **Version History:**
  - [ ] v2: BankIntegrationCapability (proposal, 2026-05-14 23:07)
  - [ ] v1: Initial BankIntegration (2026-05-14 22:00)

**Proof point:** Discovery and trust are first-class outputs. ✓

---

## Post-Demo Checklist

- [ ] All 8 beats completed in ~5.5 minutes (within 5m 30s target)
- [ ] Demo was live (not pre-recorded) — no stubs or mock data
- [ ] All 5 critical proofs demonstrated:
  1. ✓ Agents propose semantic concepts
  2. ✓ System detects reuse vs duplication
  3. ✓ Artifacts generated automatically
  4. ✓ Safe changes auto-approve
  5. ✓ Objects are discoverable, auditable, replayable
- [ ] No judges/reviewers found fakery or placeholder behavior

---

## Troubleshooting / Fallbacks

| Issue | Fallback |
|-------|----------|
| Anthropic API timeout | Use offline LLM fallback (deterministic proposal) |
| Postgres unavailable | Use in-memory SQLite for demo (switch with env var) |
| Explorer takes too long | Use CLI output instead of web UI |
| Tampering detection slow | Pre-compute checksums, show cached result |
| Any beat blocked at > 1m | Skip that beat, move to next (honesty > perfection) |

---

## What Cannot Be Faked

(From DEMO.md — critical for integrity)

- Beat 1: LLM call must happen live (or clearly show fallback)
- Beat 2: Overlap detection must be from actual registry, not hardcoded
- Beat 3: Check report must be generated from proposal + registry state
- Beat 4: All 4 artifacts must be real files on disk
- Beat 6: 47 NULL rows must be actual data in Account table
- Beat 7: Tampering must be a real SQL UPDATE, `agora verify` must query real log
- Beat 8: All explorer data must come from real registry, not mock data

---

## Demo Success Criteria

**Minimal:** All 8 beats flow, no crashes, all 5 proofs demonstrated  
**Honest:** No faking; if something doesn't work, show the offline fallback or explain the limitation  
**Impressive:** Beats flow smoothly, data is real, judges see actual operational behavior  
**Time:** Complete in <6 minutes (we have 5:30 budget)

---

## Day-of Timeline

| Time | Action |
|------|--------|
| ~07:30 | All 3 features integrated, build succeeds |
| ~07:35 | Database state verified (47 NULL rows, tables exist) |
| ~07:40 | Dry-run starts — run through all 8 beats |
| ~07:45 | Dry-run complete, feedback collected |
| ~07:50 | Final fixes (if any) |
| ~08:00 | **DEADLINE** — Demo ready for judges |

---

## Notes for Implementers

- **Feature 1:** Proposal JSON structure must be exactly as specified (11 required fields)
- **Feature 2:** Check report must include evidence (actual violation counts, sample rows)
- **Feature 3:** Checksums must be deterministic (use sorted JSON + SHA256)
- **All:** Prefer honest errors (gracefully degrade) over fake success

This checklist is the north star for the dry-run. Every item must pass.
