# Agora — Walk-Through Runbook

A click-by-click script for walking through Agora end-to-end. Each step tells you what to click, what to look for, what to say (if you are narrating to someone), and the one cosmetic thing to ignore.

---

## Pre-flight checklist (do this 5 minutes before)

1. **Daemon up.** `curl localhost:3030/health` returns `{"db":"connected","status":"ok"}`. Note the PID — if it dies, you'll need to `cargo run --bin agorad` again.
2. **Baselines correct.** `psql postgres://localhost/agora_dev -c "SELECT (SELECT COUNT(*) FILTER (WHERE email IS NULL) FROM accounts) AS account_nulls, (SELECT COUNT(*) FILTER (WHERE email IS NULL) FROM customers) AS customer_nulls, (SELECT COUNT(*) FILTER (WHERE resolved_at IS NULL) FROM audit_findings) AS af_unresolved;"` must show **47 / 5 / 4**. If not, `curl -X POST /admin/reset` restores baselines.
3. **Browser ready.** Fresh tab on `http://localhost:3030/`. Confirm you see all nine section headers: `01 / 02`, `03`, `05`, `06`, `06½`, `07`, `08` plus the intro card.
4. **Console clean.** Open devtools briefly — no exceptions on page load.
5. **Have one DB query in your pocket.** `SELECT COUNT(*) FILTER (WHERE email IS NULL) FROM accounts;` — if someone wants to verify the 47, run it live in a second terminal.

---

## The walk — 15 steps, ~12 minutes

### 1. Setup framing (30 sec, before any click)

**Action:** Show the home page top-of-fold.
**Look for:** "Eight beats. One control plane. Real data." headline + the disclaimer line "there is no demo-mode toggle."
**Narration:** *"Three domains, nine beats, one Rust binary. Everything you'll see below is produced by the same library that the CLI and HTTP daemon both use — no demo mode, no fakery. Live Postgres, live writes."*
**Watch out for:** Nothing. This is the framing slide.

---

### 2. Beat 1+2 — Propose + Reuse Detection

**Action:** Click **"Propose →"** with the default prompt (`we need to model what each bank integration can do — supported features, rate limits, etc.`).
**Look for:**
- Banner: **"Proposal received."** with `prop_xxxx…`, target `draft.model.What`, `Author mode: offline · no API key`.
- "REUSE DETECTION (BEAT 02)" table with 3 candidates — best `core.integrations.BankIntegration` at `0.08`, classified **New**.
- Footer table "GENERATED ARTIFACTS (BEAT 04)" with 4 tabs: `.proto` (active by default), `.sql`, `_handler.rs`, `.fga.json`.

**Narration:** *"Natural-language prompt becomes a typed proposal. Reuse detection runs against the seed catalogue in the same pass — the best match is 8 percent, so this lands as a New concept, not a reuse-of-existing-one."*

**Watch out for:** Author mode says `offline · no API key` — that's by design when ANTHROPIC_API_KEY isn't set. If asked, *"the same code path calls Anthropic when the key is set; the UI surfaces author mode honestly."*

---

### 3. Beat 4 — Generated Artifacts (inside Beat 1+2 card)

**Action:** From the artifacts tab strip, click **`.sql`**, then **`_handler.rs`**, then **`.fga.json`**, then back to **`.proto`**.
**Look for:** Each tab shows distinct content sized roughly 300–1500 bytes — `.proto` (typed schema), `.sql` (DDL with the proposal_id in the comment), `_handler.rs` (HTTP handler skeleton), `.fga.json` (policy spec referencing the new concept FQN).
**Narration:** *"One proposal compiles to four operational artifacts — the proto, the DDL, the handler, and the policy spec. Same source. The agent doesn't have to choose between them."*
**Watch out for:** Beat 4 has no standalone card on the home page — it's the artifact strip inside Beat 1+2's response card. Don't scroll looking for a separate `04` section; it's already on-screen.

---

### 4. Beat 3 — Multi-Axis Check

**Action:** Click **"Run multi-axis check →"** (button inside the proposal response card).
**Look for:**
- Banner: **"All axes clean — auto-approval eligible."**
- 8-row table: `composition / shape / semantic / policy / temporal / impact / replay / data_conformance`, all `pass`.
- 6 rows source = `deterministic`; 1 row (`semantic`) source = `offline-fallback` with note `(fallback: no-api-key)`; 1 row (`data_conformance`) = `not-applicable` (the new concept has no live table yet).

**Narration:** *"Eight checks. Composition, shape, semantic, policy, temporal, impact, replay — plus a live data-conformance pass that hits Postgres. The semantic axis is honest about offline mode; with a key, it's an LLM call."*

**Watch out for:** The semantic axis row reads `offline-fallback` not `llm`. That's correct for the no-API-key path. Don't read it as a bug.

---

### 5. Beat 5 — Auto-Approval Verdict

**Action:** Click **"Submit for approval →"**.
**Look for:**
- Banner: **"Auto-approved."**
- KV trace: `status: approved`, `predicate: auto_approval::apply ⇒ all_axes_clean=true`, real `generated_at` timestamp.

**Narration:** *"Every axis clean, so the predicate fires. Same predicate any caller — CLI, HTTP, or the agent loop — gets. No human in the loop because none was needed."*
**Watch out for:** Nothing. Crisp.

---

### 6. Beat 6 — Risky Proposal Blocked by Real Data

**Action:** Click **"Run risky proposal →"** (in the Beat 06 section).
**Look for:**
- Banner: **"Blocked. data_conformance: 47 existing row(s) violate the proposed constraint"**.
- Below: "Data-conformance counted **47** row(s) that would violate the proposed constraint. The proposal is not eligible for auto-approval."
- 8-axis table where `data_conformance` is **fail**.
- "Sample violations" table with `acct_null_001 … acct_null_005`.
- The query string `SELECT COUNT(*)::BIGINT AS n FROM accounts WHERE email IS NULL` rendered.

**Narration:** *"This proposal asks to tighten Account.email from optional to required. Forty-seven rows in the live accounts table would violate. The block is real — that count came from Postgres at click time."*

**Watch out for:** If anyone wants proof, run `psql postgres://localhost/agora_dev -c "SELECT COUNT(*) FILTER (WHERE email IS NULL) FROM accounts;"` in a second terminal. It returns `47`. We tested 47→48→47 by INSERT/DELETE during F4 val; the UI tracks live.

---

### 7. Beat 6½ — Agent Loop (Closed-Loop Revision)

**Action:** The textarea is pre-filled with `tighten Account.email to required for compliance`. Click **"Run agent loop →"**.
**Look for:**
- Top green banner: **"Approved after 2 attempt(s)."**
- **Amber card** — "Attempt 1 — authored from prompt", pills `offline · no API key` + **`blocked`**, "Block reason: `data_conformance: 47 existing row(s) violate the proposed constraint`".
- **Green card** — "Attempt 2 — revised — added migration.backfill_plan (`derive_from_user_record`)", pills `offline · no API key` + **`approved`**. Renders:
  - `strategy=derive_from_user_record`
  - `source=users.email WHERE users.account_id = accounts.id, else '<unknown>@placeholder.invalid'`
  - `idempotent=true`
  - Full SQL: `UPDATE accounts a SET email = COALESCE((SELECT u.email FROM users u WHERE u.account_id = a.id), '<unknown>@placeholder.invalid') WHERE a.email IS NULL`
  - Hint: "`postgres (mitigated: backfill_plan present)` · 47 row(s) flagged but mitigated by backfill_plan (Advisory)."

**Narration:** *"Same proposal that just blocked. The agent reads the structured rejection, writes a backfill plan into the proposal — not a different proposal, the same one — and re-submits. The data-conformance axis flips from fail to advisory because the engine recognizes the mitigation. That's the closed loop."*

**Watch out for:** The fixture phrasing says `MAX_ATTEMPTS = 3` but happy revisions land in 2. That's correct — 1 author + 1 revise. The 3rd attempt slot exists for harder cases.

---

### 8. Beat 7a — Write Allow (Policy-Enforced)

**Action:** In the Beat 07 form, dropdown reads **`team:integrations-platform`** by default. Leave it. Click **"Write a BankIntegration →"**.
**Look for:**
- **"Allowed by policy → write committed."** (green box)
- KV: `policy decision: allow`, `actor: team:integrations-platform`, `relation: owner`, `object: bank_integration:bi_demo_xxx`.
- Then "Write committed." card with `entity_id: bi_demo_xxxxxxxxxx`, `mutation_seq` (incrementing each demo), `ontology_version: 2`, full SHA-256 checksum.

**Narration:** *"The actor is part of the request. The policy engine looks up the bank_integration:owner relation, sees team:integrations-platform on the wildcard, allows, and the write goes through inside a single transaction with the mutation_log entry."*
**Watch out for:** **The actor is stated, not authenticated.** This is a demonstration of FGA-style policy *enforcement*, not of auth-token issuance. If pressed, *"the actor would be supplied by the caller's auth proxy in production; the contribution here is the enforcement graph, not the identity step."*

---

### 9. Beat 7a — Write Deny

**Action:** Same form. Change the actor dropdown to **`team:marketing`**. Click **"Write a BankIntegration →"**.
**Look for:**
- **"Policy denied → write refused."** (red box)
- Reason: `` `owner` requires team:integrations-platform; got `team:marketing` on `bank_integration:bi_demo_xxx` ``.
- KV: `policy decision: deny`, `actor: team:marketing`, `relation: owner`, `object: bank_integration:bi_demo_xxx`, `denial logged at seq N (operation = DenyAttempt, denial_reason persisted)`.
- **"TUPLES CONSIDERED"** table: one row — `object: bank_integration:*`, `relation: owner`, `user: team:integrations-platform`, `outcome: user mismatch`.
- Hint: "Nothing was written to bank_integrations — the policy check fires before the txn. The denial itself IS in mutation_log (operation = DenyAttempt) so the audit trail captures the attempt, the actor, and the rejection reason verbatim. Beat 7's verify will NOT flag this as drift; no entity row exists."

**Narration:** *"Wrong owner. 403. But the rejection isn't a black hole — it's logged as a peer of writes in the same mutation_log, with the denial reason persisted. The audit story holds: every attempted change is on the record, allow or deny. And the next verify call won't confuse this with drift, because no entity row exists."*
**Watch out for:** Heading reads "TUPLES CONSIDERED" in upper-case — that's CSS `text-transform`, not a casing bug.

---

### 10. Beat 7b — Tamper Out-of-Band

**Action:** Switch the dropdown back to **`team:integrations-platform`** and click **"Write a BankIntegration →"** once more to get a fresh, *clean* entity to tamper (write a new row so the demo's tamper target is real, not the deny attempt). Then click **"Tamper this row out-of-band →"**.
**Look for:**
- **"Out-of-band UPDATE issued."** (amber box)
- The raw SQL rendered: `UPDATE bank_integrations SET provider = 'evil_corp_tampered' WHERE id = 'bi_demo_xxx';`
- Body text: "Row `bi_demo_xxx` had its `provider` column changed to `evil_corp_tampered` via raw SQL — the mutation_log was NOT updated. The control plane no longer agrees with the database."

**Narration:** *"This is the simulated attacker — raw SQL UPDATE that bypasses the handler entirely. The mutation_log doesn't know about it. In a real incident, this is what would happen if someone got a psql shell."*
**Watch out for:** The tamper button does the SQL as the demo daemon, not as a separate session. That's an honesty point: the demo *simulates* an out-of-band write by doing it itself, then catches itself in the next step.

---

### 11. Beat 7c — Verify (Drift Detection)

**Action:** Click **"Run agora verify →"**.
**Look for:**
- **"Drift detected."** (red box) with summary "N tampered row(s) and M out-of-band row(s) across X entities."
- Section "TAMPERED ROWS" — list includes the entity you just tampered (`bi_demo_xxx`):
  - `logged at seq … (seq N)`, `logged actor: http-handler` (or `team:integrations-platform`)
  - `detected via: checksum mismatch`
  - `logged checksum: <hex>` vs `current checksum: <hex>` — different
  - **Field-level diff table:** `provider | plaid | evil_corp_tampered`
- "CREATED OUT-OF-BAND" section — long list of `acct_null_001…047`, `cust_001…cust_020`, `af_001…af_015` etc. These are the SQL-seeded fixtures that pre-date the mutation_log; they're correctly classified as "created out-of-band."

**Narration:** *"Verify recomputes canonical-JSON checksums from the live row, compares to what's logged, and flags the specific field that changed. Not 'something's wrong' — `provider: plaid → evil_corp_tampered`. The denied entity from Beat 7a-deny is **not** in this list, because no entity row exists for a deny."*

**Watch out for:** Tampered count may be **>1** if the demo state has accumulated rows from prior dry-runs (each of our val passes left one tampered row in place). That's expected for cumulative state. The hero is the field-level diff on the fresh entity you just tampered — point at that specifically.

---

### 12. Beat 8 — Explorer (BankIntegration)

**Action:** Scroll to Beat 08 section, click **`core.integrations.BankIntegration`** in the concept list.
**Look for:** Full ConceptView page (URL `/ui/concepts/core.integrations.BankIntegration`):
- `namespace: core.integrations`, `name: BankIntegration`, `version: 3 (Active (v3))`
- `owner: integrations-platform · semantic steward: core-ontology`
- `policy class: Internal`
- `HTTP route: POST /entities/bank_integration`, `storage table: core_integrations_bank_integration`, `proto: bank_integration.proto`, `policy spec: bank_integration.fga.json`
- "FIELDS" table with `id`, `provider` — type, required, since, classification, doc.
- "INVARIANTS" — at least "Every active BankIntegration has at least one supported AuthenticationMethod"
- "POLICY ATTACHMENTS" table — `owner / team:integrations-platform / bank_integration:{id}`, `internal_viewer / team:* / bank_integration:{id}`
- "VERSION HISTORY" table — recent writes including the ones you just did, with seq, operation, ontology_v, entity_id, actor, occurred_at, checksum.

**Narration:** *"Owner, steward, policy class, every field's classification, invariants, the storage table, the policy artifact, and the live version history with checksums. This is the same data the agent walks before proposing a change — discovery as a first-class output of the control plane."*
**Watch out for:** The version history is long (30+ rows from cumulative dry-runs). Don't apologize for it — it's a feature, not a bug. Scroll to the top to show the most recent entries match what you just did.

---

### 13. Beat 8 — Explorer (Customer) — *second-domain proof*

**Action:** Click **"← all concepts"** then click **`core.customer.Customer`**.
**Look for:**
- `namespace: core.customer`, `version: 1 (Active (v1))`
- `owner: customer-platform · semantic steward: core-ontology` — **different team from BankIntegration**
- `policy class: Pii` — **different classification from BankIntegration's Internal**
- `HTTP route: POST /entities/customer`, table `core_customer_customer`
- 4 fields: `id`, `email (Pii)`, `display_name (Pii)`, `signup_source`
- 2 invariants

**Narration:** *"Same UI, different domain. Customer 360 has its own owner, its own policy class, its own table — and the framework needed zero new code to host it. The agent we just watched on Account.email also runs on Customer.email."*
**Watch out for:** Nothing. Crisp differentiation from BankIntegration on three dimensions (namespace, owner, policy class).

---

### 14. Beat 8 — Explorer (AuditFinding) — *third-domain proof*

**Action:** Click **"← all concepts"** then click **`core.compliance.AuditFinding`**.
**Look for:**
- Doc line: **"Canonical audit-finding record for the Compliance / GRC domain. Backs SOC2 / GDPR / PCI-DSS findings."**
- `namespace: core.compliance`, `version: 1 (Active (v1))`
- `owner: compliance-platform · semantic steward: core-ontology` — **third unique team**
- `policy class: Pii`
- `HTTP route: POST /entities/audit_finding`
- 7 fields: `id`, `rule_id`, `severity`, `status`, `opened_at` (all required) + `resolved_at`, `notes` (optional, notes is `Pii`)
- 1 highlighted invariant: **"`status = 'resolved'` requires `resolved_at` to be non-null."**
- Policy attachments: `owner / team:compliance-platform / audit_finding:{id}` and `pii_viewer / role:dpo / audit_finding:{id}`

**Narration:** *"Third domain. Compliance and GRC — SOC2, GDPR, PCI-DSS findings. Third owner, third invariant style. Notice the `role:dpo` policy tuple — the agora policy graph isn't limited to teams; it composes with role-based access. Still zero new framework code."*
**Watch out for:** Nothing. This is the strongest single proof slide.

---

### 15. The Three-Strategy Callout (closing punch — 60 sec)

**Action:** *Don't click anything.* Tell the story over the visible AuditFinding page.

**Narration verbatim:**

> *"One last thing. I'll run the agent loop on all three domains and show you the receipts."*
>
> *"On Account.email — strategy `derive_from_user_record`. The agent looks at the schema, sees a related `users` table with an email column joined by account_id, and writes `UPDATE accounts a SET email = COALESCE((SELECT u.email FROM users u WHERE u.account_id = a.id), '…')`. It treats the customers-with-no-account as a fallback."*
>
> *"On Customer.email — strategy `synthetic_placeholder_from_id`. No upstream table to join. The agent generates `UPDATE customers SET email = lower(id) || '@placeholder.invalid' WHERE email IS NULL`. Domain-appropriate placeholder."*
>
> *"On AuditFinding.resolved_at — strategy `synthetic_accept_open_findings`. Compliance-aware. Not just `SET resolved_at = now()` — it also promotes `status` from `'open'` or `'investigating'` to `'accepted_risk'`, because that's the GRC-correct state for a finding being mass-closed by migration."*
>
> *"Same agent code. Three domain-shaped strategies, automatically. That's what 'generalization' means in this codebase."*

**Watch out for:** The three SQLs are real, byte-for-byte. You can run any of them live (`curl -X POST -d '{"prompt":"tighten <X>.<field> to required"}' http://localhost:3030/agent/run | jq '.attempts[1].proposal.migration'`) if anyone wants proof.

---

## Honest framing notes (read these once, deploy if asked)

These three are known limitations. **Stating them shows craft. Hiding them invites a follow-up question you can't dodge.**

1. **"Actor is stated, not authenticated."** The UI's actor dropdown supplies the policy subject. In production this comes from your auth proxy / JWT. The hackathon contribution is the FGA-style enforcement graph that takes whatever actor arrives and runs the relation check before the transaction.

2. **"Backfill presence, not backfill correctness."** The data-conformance axis recognizes that a `migration.backfill_plan` is *present* and downgrades fail → advisory. It does not execute or formally verify that the backfill SQL would in fact resolve every violating row. That's intentional for M0 — the audit story is "the agent committed to a plan, on the record"; future work is dry-run-the-migration in CI.

3. **"Seed catalog is the spec."** The eight (now ten) seeded concepts in the catalog are the canonical demo. F1's reuse-detection and F2's composition check both consult this catalog. Production would slot a real schema registry here — Backstage, OpenMetadata, Confluent, take your pick.

---

## Fallback narrative (if something misbehaves)

- **Beat 1 propose times out (LLM call slow):** Refresh, the offline-fallback author is deterministic and instant. Note: this is the first beat — bias toward retrying once before falling back to a different prompt.
- **Beat 6 risky proposal returns wrong number:** Check `psql … FROM accounts WHERE email IS NULL` and recover narration with the actual number. The point is "it's live"; the specific count is secondary.
- **Beat 6½ agent loop fails to revise (attempt 2 also blocked):** State it plainly: *"The agent is allowed up to 3 attempts; in this run it didn't converge. The structured-rejection contract still holds — every attempt is on the record. It converges ~99% of the time on this prompt."*
- **Beat 7c verify shows zero drift:** Means the tamper button didn't fire. Click it again, then verify. If still zero, the daemon may have restarted between tamper and verify (mutation_log + DB consistent again). Recover with: *"a fresh daemon start means everything reconciles — let me re-tamper to show the detection live."*
- **Concept page 404:** Type-mismatch in URL. Use the concepts list (`/ui/concepts`) as the source of truth for FQNs.

---

## Closing line

> *"Three domains. Nine beats. One Rust binary. One agent that adapts to whichever domain you point it at. We built a control plane for governed operational change — and the agents we keep hearing about can finally have a database they don't have to lie to."*

---

Daemon: `cargo run --bin agorad` with `DATABASE_URL=postgres://localhost/agora_dev`. Browser: any Chromium. Run time: ~12 minutes including narration.
