# Feature 3: Explorer + Fact Log + Policy Enforcement

**Estimate:** 3.5h implementation  
**Demo beats:** 7 (write + tampering detection), 8 (explorer UI)  
**Status:** Spec locked (ready for impl-f3)

---

## What Feature 3 does

1. **Fact Log (append-only mutation log):** Records every write to entities with ontology version stamp (Beat 7)
2. **Write Handler Integration:** Routes entity writes through HTTP handlers (from Feature 1 artifacts) and logs mutations
3. **Tampering Detection (`agora verify`):** Compares actual DB state with fact log to detect out-of-band modifications (Beat 7)
4. **Explorer UI:** Navigable view of concepts, ownership, invariants, lineage, policy, version history (Beat 8)

---

## Part 1: Fact Log + Write Flow (Beat 7)

### Fact Log Schema

```sql
CREATE TABLE IF NOT EXISTS mutation_log (
  mutation_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  entity_type TEXT NOT NULL,              -- e.g., "core.integrations.BankIntegration"
  entity_id TEXT NOT NULL,
  operation TEXT NOT NULL,                -- "CREATE", "UPDATE", "DELETE"
  data JSON NOT NULL,                     -- full entity after mutation
  ontology_version INT NOT NULL,          -- which version of ontology this was written under
  written_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  written_by TEXT NOT NULL,               -- "system", agent ID, or "direct-sql" if tampering
  checksum TEXT,                          -- SHA256 of (entity_id, operation, data, ontology_version)
  INDEX mutation_log_entity (entity_type, entity_id, written_at DESC),
  INDEX mutation_log_version (ontology_version)
);
```

### Write Flow

When a write comes through the HTTP handler (from Feature 1 artifacts):
1. Handler receives a mutation (e.g., CreateBankIntegrationCmd)
2. **Construct the mutation:**
   - entity_id: from request
   - operation: "CREATE" | "UPDATE" | "DELETE"
   - data: serialized entity JSON
   - ontology_version: current ontology version (from request or header)
3. **Write to mutation_log** atomically with the actual entity write
4. **Return response** with mutation_id and ontology_version

**Demo consequence (Beat 7, sub-step 1):**
```
→ POST /entities/bank_integration { entity_id: "bi-acme", ... }
← 201 Created { mutation_id: "mut-abc123", ontology_version: 2 }
+ mutation_log row created with checksum
```

---

## Part 2: Tampering Detection (`agora verify`) (Beat 7, sub-steps 2-3)

### The Tampering Scenario

1. **Happy write** (via HTTP handler) → mutation logged with checksum
2. **Out-of-band tampering** (raw SQL `UPDATE` bypassing handler) → mutation_log has NO entry for this change
3. **Run `agora verify`** against the database

### `agora verify` Algorithm

```
For each entity table in the registry:
  For each row in that table:
    1. Compute current state checksum from row data
    2. Look up the most recent mutation_log entry for this entity_id
    3. Compute expected checksum from that log entry
    4. If checksums don't match → TAMPERING DETECTED
       - Report: entity_id, field that changed, mutation log gap
       - Output to stderr: structured JSON with drift
    5. If no mutation_log entry exists for this entity_id → CREATED OUT-OF-BAND
       - Report: entity_id, was created outside control plane
```

### Checksum Computation

```rust
checksum = SHA256(format!(
  "{entity_type}|{entity_id}|{operation}|{data_json}|{ontology_version}",
  data_json = canonical_json_sort(data)
))
```

The checksum ensures that if even one field changes without a corresponding mutation_log entry, it's detected.

### Output Format

```json
{
  "verify_status": "clean" | "tampered",
  "timestamp": "...",
  "entities_checked": 42,
  "tampered_entities": [
    {
      "entity_id": "ba-acme",
      "entity_type": "core.integrations.BankIntegration",
      "issue": "drift",
      "field_changed": "provider_url",
      "detected_via": "checksum mismatch",
      "last_logged_mutation": "mut-abc123",
      "last_logged_at": "2026-05-14T23:00:00Z",
      "current_state": {...},
      "logged_state": {...}
    }
  ],
  "outofband_entities": []
}
```

**Demo consequence (Beat 7):**
```
$ agora verify
✗ TAMPERING DETECTED
  Entity: ba-acme (BankIntegration)
  Field changed: provider_url
  Last mutation: mut-abc123 at 2026-05-14T23:00:00Z
  Current value: "https://evil.bank"
  Logged value: "https://acme.bank"
  Conclusion: mutation without log entry → drift
```

---

## Part 3: Explorer UI (Beat 8)

### What the Explorer Shows

When you navigate to a concept (e.g., `core.integrations.BankIntegration`):

```
┌─────────────────────────────────────────────────────┐
│ core.integrations.BankIntegration                   │
├─────────────────────────────────────────────────────┤
│ Owner: integrations-platform                        │
│ Semantic Steward: @schema-broker                    │
│ Status: Active (v2)                                 │
├─────────────────────────────────────────────────────┤
│ INVARIANTS                                          │
│  • entity_id: required, unique                      │
│  • provider_name: required, lowercase               │
├─────────────────────────────────────────────────────┤
│ LINEAGE (what touches this concept)                 │
│  • HTTP: POST /entities/bank_integration            │
│  • Linked to: AuthenticationMethod (1:N)            │
│  • Generated by: BankIntegrationCapability (feature)│
│  • Policy: bank_integration.fga.json               │
├─────────────────────────────────────────────────────┤
│ POLICY (who can read/write what)                    │
│  • internal_viewer: team:*                          │
│  • owner: team:integrations-platform               │
│  • sensitive_viewer: role:data-analyst             │
├─────────────────────────────────────────────────────┤
│ VERSION HISTORY                                     │
│  • v2 (2026-05-14 23:07) BankIntegrationCapability │
│       added by: agent://schema-broker               │
│       check_report: all axes additive               │
│  • v1 (2026-05-14 22:00) Initial BankIntegration    │
│       added by: human                               │
└─────────────────────────────────────────────────────┘
```

### Data Sources for Explorer

1. **Concept metadata:** From registry (concept_fqn, owner, status, invariants)
2. **Lineage:** From registry's lineage graph (what other concepts/artifacts reference this)
3. **HTTP handlers:** From Feature 1's generated artifacts
4. **Policy:** From Feature 1's .fga.json artifacts
5. **Version history:** From mutation_log + CheckReports (joined with proposal history)

### Implementation Approach

**Option A: CLI explorer** (fastest, acceptable for hackathon)
- `agora explorer <concept_fqn>`
- Queries registry DB, formats as structured text
- Navigable with less/more
- ~1h to implement

**Option B: Web UI** (more impressive, ~2.5h)
- Simple HTTP server in Axum
- Query registry, render HTML/CSS
- Clickable navigation between concepts
- Might be tight on time but worth the polish

**Recommendation:** Start with CLI (fast, demoable), add web UI if time permits.

---

## Implementation Structure

### Part 1: Fact Log (shared infrastructure)
- `src/mutation_log.rs` — schema, insert logic, checksum computation
- Modify Feature 1's HTTP handlers to call `log_mutation()` before returning

### Part 2: Tampering Detection
- `src/verify.rs` — `agora verify` implementation
- `src/cli/verify.rs` — CLI command handler
- Reads mutation_log, computes checksums, reports drift

### Part 3: Explorer
- `src/explorer.rs` — data fetching (registry queries, lineage, policy)
- `src/explorer_cli.rs` OR `src/explorer_ui.rs` — rendering
  - If CLI: formatted text output
  - If web: Axum routes + static HTML/CSS

---

## Integration Points

### With Feature 1
- Feature 1's HTTP handlers need to log mutations
- Call: `log_mutation(entity_type, entity_id, operation, data, ontology_version).await?`
- This happens at startup when we plumb the handlers

### With Feature 2
- Explorer displays the CheckReport alongside the proposal
- CheckReport becomes a first-class registry artifact
- Explorer queries: `SELECT check_report FROM proposals WHERE id = ?`

### With Demo
- Beat 7: Write flow + tampering
  - Setup: seed an entity via HTTP handler
  - Action: issue raw SQL UPDATE directly to table
  - Result: `agora verify` detects the tampering
- Beat 8: Explorer
  - Navigate to BankIntegration, see full lineage and history
  - Click through to AuthenticationMethod relationship
  - View policy and who can access what fields

---

## Data Seeding for Demo

**Before demo runs:**
1. Create and initialize ontology registry tables
2. Create mutation_log table
3. Seed Account table with 47 rows where email IS NULL
4. Seed BankIntegration, AuthenticationMethod tables
5. Seed initial proposal history for version 1

This is coordination between features — likely Feature 3's validator or setup step.

---

## Test Coverage (for val-f3)

### Happy Path (Beat 7, sub-step 1)
- Write entity via HTTP handler
- Verify row appears in entity table
- Verify mutation_log entry exists with correct checksum
- Read the entity back

### Tampering Detection (Beat 7, sub-steps 2-3)
- Write entity via HTTP handler
- Issue raw SQL UPDATE directly (bypass handler)
- Run `agora verify`
- Verify it reports the specific field that changed
- Verify it shows the mutation_log gap

### Explorer (Beat 8)
- Query a concept (BankIntegration)
- Verify owner, invariants, lineage are present
- Verify policy shows correct access rules
- Verify version history shows the proposal that created it

### Edge Cases
- Entity created out-of-band (no mutation_log entry) → verify detects
- Multiple tamperings on same entity → verify reports all
- Explorer concept doesn't exist → graceful 404
- Mutation_log contains sensitive data → policy checks it properly

---

## Success Criteria for val-f3

1. Write via HTTP handler → entity + mutation_log row created
2. Tampering (raw SQL UPDATE) → `agora verify` detects and reports the change
3. Explorer displays all required fields (owner, invariants, lineage, policy, history)
4. `agora verify clean` exits cleanly after successful write
5. Performance: <1s for verify on ~50 entities

---

## Timeline
- Mutation log + write integration: 1h
- Tampering detection: 1h
- Explorer (CLI): 1.5h
- Validation + integration: 1h
- **Total: 4.5h**

If we have time, add web UI for Explorer (+1h).

---

## Unknowns / Dependencies

1. **Ontology registry DB schema:** Where is it? Same Postgres instance?
2. **Feature 1 integration:** Do handlers get plumbed into the main app in Feature 2 or Feature 3?
3. **Policy enforcement:** Does Feature 3 verify that the write is allowed under the policy, or just log it?
   - Recommendation: Just log + verify for hackathon (enforcement is Future Work™)
4. **Version history linkage:** How do proposals, check reports, and mutations link together in version history?

(Architecture Lead will clarify schema/registry location.)

---

## Nice-to-Have (if time permits)

- Web UI for Explorer (instead of CLI)
- Policy enforcement on writes (check FGA before accepting mutation)
- Full bitemporality (valid-time + transaction-time) — M1 (deferred to post-hackathon)
- Compression on mutation_log for large entities

---

## Why This Order

Feature 1 → Feature 2 → Feature 3 is deliberate:
1. **F1 generates artifacts** (proposals, handlers, policy specs)
2. **F2 validates proposals** (checks), gates approval
3. **F3 executes proposals** (writes via handlers, detects tampering, shows discovery)

The demo only works if all three flow together. Feature 3 is the "operational proof" that Agora actually works in practice.
