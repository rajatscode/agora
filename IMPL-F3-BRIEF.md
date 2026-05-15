# Impl-F3 Brief: Explorer + Fact Log + Policy Enforcement

**Time:** Implementation starts after F2 integration (~06:30 ET). **2.5-hour window.**

**Note:** This is the final feature. The demo's operational proof depends on this being solid. No shortcuts on integrity (checksums, tampering detection) but we can lean hard on simplicity everywhere else.

---

## What You're Building

Three integrated systems:
1. **Mutation Log** — append-only writes recorded with ontology_version + checksum
2. **`agora verify`** — detects out-of-band tampering by comparing DB state to mutation log
3. **`agora explorer`** — CLI showing concept metadata, lineage, policy, version history

---

## Spec You're Building From

`FEATURE-3-SPEC.md` is locked. Skim for:
- Mutation_log schema: mutation_id, entity_type, entity_id, operation, data, ontology_version, written_at, written_by, checksum
- Write flow: HTTP handler calls `log_mutation()` atomically with the entity write
- Verify algorithm: for each entity, compute checksum, compare to mutation_log, report drift
- Explorer queries: registry metadata, lineage, policy, version history (all sources should be real)

---

## Database Schema (Pre-seeded or Yours to Create?)

**ASK Architecture.** You need to know:
- Does Account table exist? BankIntegration? AuthenticationMethod?
- Is mutation_log table pre-created or your responsibility?
- Where is the ontology registry (same Postgres? Different DB?)

If unclear when you start, ask Architecture — don't guess.

---

## Implementation Road Map (2.5h Budget)

### Phase 1: Mutation Log Schema + Insert Logic (30 min)

- `src/mutation_log.rs`:
  ```sql
  CREATE TABLE IF NOT EXISTS mutation_log (
    mutation_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    operation TEXT NOT NULL,  -- CREATE|UPDATE|DELETE
    data JSON NOT NULL,
    ontology_version INT NOT NULL,
    written_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    written_by TEXT NOT NULL,
    checksum TEXT,
    INDEX mutation_log_entity (entity_type, entity_id, written_at DESC)
  );
  ```
- Implement `async fn log_mutation(db: &PgPool, entity_type: &str, entity_id: &str, operation: &str, data: Value, ontology_version: i32, written_by: &str) -> Result<Uuid>`
- Checksum function: `fn compute_checksum(entity_type: &str, entity_id: &str, operation: &str, data: &Value, ontology_version: i32) -> String`
  - Use SHA256 of canonicalized JSON (sorted keys)
  - Deterministic: same inputs → same output, always

**Deliverable:** Mutation logging works end-to-end. Write to DB + log in same transaction (ACID).

### Phase 2: Write Flow Integration (45 min)

- Plumb mutation logging into Feature 1's HTTP handlers (generated `_handler.rs` files)
- Handlers now call `log_mutation()` before returning:
  ```rust
  let mutation_id = log_mutation(
    db, 
    "core.integrations.BankIntegration", 
    &cmd.entity_id, 
    "CREATE", 
    json!(entity), 
    2,  // ontology_version
    "http-handler"
  ).await?;
  
  Ok(Json(CreateResp { entity_id, mutation_id, ontology_version: 2 }))
  ```
- Response includes mutation_id and ontology_version (proof of logging)

**Test case (critical):** POST to HTTP handler → entity row appears in table + mutation_log row appears with matching checksum.

**Deliverable:** Happy-path write creates both entity and log entry atomically.

### Phase 3: `agora verify` — Tampering Detection (45 min)

- `src/verify.rs`:
  ```rust
  pub async fn verify(db: &PgPool) -> Result<VerifyReport> {
    let mut tampered_entities = Vec::new();
    
    // For each entity table (BankIntegration, Account, etc.):
    for row in get_all_entities(db).await? {
      let current_checksum = compute_checksum(&row.entity_type, &row.entity_id, &row.operation, &row.data, &row.ontology_version);
      
      if let Some(log_entry) = get_latest_mutation(db, &row.entity_type, &row.entity_id).await? {
        if current_checksum != log_entry.checksum {
          // DRIFT
          tampered_entities.push(TamperedEntity {
            entity_id: row.entity_id,
            issue: "drift",
            field_changed: identify_changed_fields(&log_entry.data, &row.data),
            detected_via: "checksum mismatch",
            last_logged_mutation: log_entry.mutation_id,
            current_state: row.data,
            logged_state: log_entry.data,
          });
        }
      } else {
        // NO LOG ENTRY
        tampered_entities.push(TamperedEntity {
          entity_id: row.entity_id,
          issue: "created_out_of_band",
          // ... rest of fields
        });
      }
    }
    
    Ok(VerifyReport {
      verify_status: if tampered_entities.is_empty() { "clean" } else { "tampered" },
      entities_checked: num_entities,
      tampered_entities,
    })
  }
  ```

- CLI: `agora verify` runs the check and outputs JSON to stdout

**Test case (critical):** Issue raw SQL UPDATE outside the handler → `agora verify` detects the field changed + reports missing mutation_log entry.

**Deliverable:** Real tampering (raw SQL UPDATE) is detected and reported with field + checksum mismatch.

### Phase 4: `agora explorer` — Discovery UI (30 min)

- `src/explorer.rs`:
  - Query registry for concept metadata (owner, invariants, status)
  - Query mutation_log for version history
  - Construct the output view

- CLI: `agora explorer <concept_fqn>` displays:
  ```
  core.integrations.BankIntegration
  Owner: integrations-platform
  Status: Active (v2)
  
  Invariants:
    • entity_id: required, unique
    • provider_name: required, lowercase
  
  Lineage:
    • HTTP: POST /entities/bank_integration
    • Linked to: AuthenticationMethod (1:N)
    • Policy: bank_integration.fga.json
  
  Policy:
    • internal_viewer: team:*
    • owner: team:integrations-platform
  
  Version History:
    • v2 (2026-05-15 03:45) added field_flag (BankIntegrationCapability proposal)
    • v1 (2026-05-15 02:00) Initial BankIntegration
  ```

**Pragmatic:** Don't build a web UI. CLI that outputs formatted text is sufficient for the demo.

**Deliverable:** CLI shows owner, invariants, lineage, policy, version history for a concept.

### Phase 5: Edge Cases + Error Handling (20 min)

- Empty mutation_log (no writes yet): `agora verify` reports "clean"
- Non-existent concept: `agora explorer` returns clear error
- Concurrent mutations: each gets its own checksum (deterministic)
- Malformed data: JSON parse error reported cleanly

**Deliverable:** Graceful error handling; no `.unwrap()` outside tests.

---

## Test Coverage (for val-f3)

The validator will test:
1. **Happy write** → entity + mutation_log entry with matching checksum
2. **Checksums are deterministic** → same write, computed twice, identical result
3. **Tampering detection** → raw SQL UPDATE → `agora verify` reports the field changed
4. **Clean verify** → no tampering → `agora verify` reports clean
5. **Explorer shows real data** → all displayed fields cross-checked against registry

---

## Resources

- **FEATURE-3-SPEC.md** — full specification (locked)
- **test-reviewer's validation strategy** — falsification approach
- **Architecture:** Ask about database setup, registry location, concurrent writes
- **QA Lead:** Ask about test data seeding if you hit issues

---

## Questions to Ask

- **Architecture:** Where is the ontology registry? Concurrent write handling?
- **QA Lead:** How to seed test data for explorer output validation?
- **Test-Reviewer:** If mutation_log schema questions come up

---

## Success Criteria

1. ✓ Writes are logged atomically (entity + mutation_log in same transaction)
2. ✓ Checksums are deterministic (same inputs → same output)
3. ✓ `agora verify` detects out-of-band SQL UPDATEs and reports them
4. ✓ `agora verify` reports "clean" when there's no tampering
5. ✓ `agora explorer` shows real data (owner, lineage, policy, version history)
6. ✓ All errors are recoverable; no panics or `.unwrap()`

---

## Red Flags (When to Ask for Help)

- "How do I hash the JSON canonically?" → Use `serde_json` with sorted keys
- "Where do I get the concept metadata?" → It's in the registry (ask Architecture where that is)
- "Concurrent writes handling?" → Ask Architecture; also test-reviewer can help
- "Stuck on time" → Let me know ASAP, we can defer Explorer to CLI-only or even cut it

---

## Known Constraints

- **No web UI** — CLI output is acceptable (saves ~1h)
- **No policy enforcement on writes** — just log them. Enforcement is future work.
- **No full bitemporality** — M0 append-only. Temporal questions escalate to Researcher.

---

## Leaning Scope (if time is tight)

If you're running behind by 30+ minutes:
1. Skip elaborate error messages; keep them functional
2. Explorer: show only owner + version history (cut lineage/policy if needed)
3. Verify: focus on drift detection; cut out-of-band creation detection

**But DO NOT compromise:** mutation logging integrity, checksum determinism, tampering detection. Those are the proofs.

---

## Starting Point

1. Feature 2 has integrated (feature branch merged)
2. Database setup answer from Architecture
3. Mutation log table created
4. Clone the repo, branch for Feature 3
5. **Start with Phase 1 (checksums)** — it's the foundation for everything else

**Go build.** This is the final piece. The demo lives or dies on whether tampering detection actually works. Trust the spec, ask for help if needed, and keep the pace sustainable.
