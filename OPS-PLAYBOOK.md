# OPS-PLAYBOOK — What to do if the demo goes sideways

A one-card-per-failure-mode reference you can read **during** the demo. Every card lists **symptoms**, **diagnosis** (how to confirm in seconds), **fix** (specific commands you can paste), and **fallback narration** (what to say while you're typing).

The honest framing is the through-line: **calibration over recovery theatre**. If something degrades, name it. The system is designed to surface degraded modes rather than hide them — say so out loud.

Sidekick references: pre-demo checklist in `DEMO-RUNBOOK.md`; runtime rule in `README.md` ("After each merge or rebuild, restart the daemon.").

---

## LLM mode + quota (decide this BEFORE going live)

**Default is offline.** The demo flow works **fully** without `ANTHROPIC_API_KEY`. Every author-mode pill renders `offline · no API key` honestly; Beat 6½ revision plans come from the deterministic heuristic. This is the safer demo posture and the mode all dry-runs have been validated against.

**If the user wants live LLM for the judge run:**

1. **Set the key in the daemon's environment, then restart.** The daemon caches mode at boot — exporting after launch is too late.
   ```bash
   kill $(lsof -ti:3030)
   ANTHROPIC_API_KEY=sk-ant-... DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad
   ```
2. **Check quota first.** Each full demo run makes **~2 Anthropic tool-use calls** (Beat 1 proposal author + Beat 6½ revise step). Practice runs eat the same budget. **Plan for 4–8 calls per session** (3 practice runs + 1 judge run is realistic).
3. **Verify with one Beat 1 click.** If the proposal card's pill reads `live · LLM-derived`, you're good. If it reads `offline · API error`, check the key's permissions and tier before going on stage.
4. **Mid-demo rate limit is non-fatal.** The pill flips to orange `offline · API error` and the offline fallback fires automatically. This is **by design, not a failure** — narrate it as such (see failure card #4 below).

**Recommendation.** Run the demo in **offline mode** unless live-LLM is specifically required by the judges. Offline mode is faster, free, deterministic, and visibly honest. Live mode is the same architecture with slightly different latency and a non-zero API-error risk you have to monitor.

---

## Pre-demo (90 seconds, do this before the camera is on)

- Daemon running on the **latest** binary: `kill $(lsof -ti:3030) 2>/dev/null; cargo build --bin agorad && DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad &`
- `curl -s localhost:3030/health` → `{"status":"ok","db":"connected"}`
- Side-car terminal open with `psql agora_dev` ready
- Side-car terminal open with `curl` ready (paste-able templates in the runbook)
- Browser devtools **console** tab open in a separate window — the F4 tab bug (#6 below) only shows up there
- `ANTHROPIC_API_KEY` either exported (live mode) or knowingly unset (offline mode). **Both are fine.** Don't pretend.

---

## 1. Daemon crashed mid-demo

- **Symptoms:** blank page, browser tab shows "connection refused" or 502, every HTMX button does nothing.
- **Diagnosis:** `ps aux | grep '[a]gorad'` returns no row. Or `curl -s localhost:3030/health` errors out.
- **Fix:** Fresh terminal, paste:
  ```bash
  DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad
  ```
  Then refresh the browser. Postgres state is intact; the daemon is stateless.
- **Fallback narration:** *"Real systems crash. Watch us recover — single binary, restart in seconds, no data loss because everything is in Postgres. The mutation log will still have every prior write."*

## 2. Postgres connection died

- **Symptoms:** `curl -s localhost:3030/health` returns `{"db":"disconnected"}` or `"unreachable"`. Beat 3's `data_conformance` axis returns `Skipped`. Beat 7 writes 503.
- **Diagnosis:** `brew services list | grep postgresql` shows stopped/error.
- **Fix:**
  ```bash
  brew services restart postgresql@14
  # wait ~3 sec, then restart the daemon to re-establish the pool:
  kill $(lsof -ti:3030); DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad
  ```
- **Fallback narration:** *"Storage substrate is replaceable. The control plane is the architectural piece. Spanner instead of Postgres tomorrow — same code path."*

## 3. Browser tab drops connection / spinner forever

- **Symptoms:** HTMX button shows the disabled state but the slot never updates.
- **Diagnosis:** Devtools network tab shows the request hung or cancelled. Daemon log shows no incoming request (network blip) **or** shows the request completed (browser issue).
- **Fix:** Refresh the tab. HTMX is server-rendered — every prior beat re-renders from disk + DB. You can pick up exactly where you left off.
- **Fallback narration:** *"The daemon doesn't keep browser session state — let me refresh and we'll continue from where we were."*

## 4. Anthropic rate-limited or down (Beat 1 / Beat 6½)

- **Symptoms:** The author-mode pill on the proposal card renders **orange**: `offline · API error` (or `offline · no API key` if the env var is unset).
- **Diagnosis:** That pill *is* the diagnosis. The system told you.
- **Fix:** None needed — offline mode is the fallback by design. The deterministic offline author emits a structurally valid proposal; the gate doesn't know the difference.
- **Fallback narration (strong version):** *"And here is the offline fallback firing live. The system tells you when it's degraded instead of pretending. That's the calibration property F1 ships with — the same property you saw in the multi-axis check 'live count' line."*

## 5. Beat 6 shows count ≠ 47

- **Symptoms:** Blocked card says `46 existing row(s)` or `48 existing row(s)` instead of 47.
- **Diagnosis:** A prior run modified the `accounts` table — usually a successful Beat 6½ revision that backfilled some rows, or a stray INSERT/DELETE in psql.
- **Fix (preferred):** Use the admin reset endpoint:
  ```bash
  curl -X POST localhost:3030/admin/reset
  ```
  Or, in psql:
  ```sql
  -- Re-run the seed segment that creates the 47 NULL rows
  \i migrations/002_seed_accounts.sql
  ```
- **Fallback narration:** *"Whatever number you see on screen is correct — the gate reports the actual current state of the table, not a constant. Watch — I'll click again, the count updates. That's the falsifiability property."*

## 6. F4 tab strip frozen on `.proto` (the historical F4 bug)

- **Symptoms:** Clicking the `.sql` / `_handler.rs` / `.fga.json` tabs in Beat 4's artifact strip doesn't change the panel below.
- **Diagnosis:** Browser **devtools console** shows `TypeError: Cannot read properties of null (reading 'addEventListener')`. The tab-switch script attached to `document.body` before `<body>` was parsed.
- **Fix:** Confirm the daemon is built from a commit at or after `3cf3eb6` (`fix(F4): tab handler must delegate off document, not document.body`):
  ```bash
  git log --oneline | grep 3cf3eb6   # should appear
  kill $(lsof -ti:3030); cargo build --bin agorad && DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad
  ```
- **Fallback narration:** *"While I sort the tab strip — here are the four artifact files on disk directly,"* and `cat generated/{proposal_id}/{*.proto,*.sql,*_handler.rs,*.fga.json}` in your side-car terminal. Beat 4 still lands.

## 7. Agent loop stalls (3 attempts, all blocked) — Beat 6½

- **Symptoms:** Three amber attempt cards, banner reads `Stalled after 3 attempt(s)`.
- **Diagnosis:** The offline-heuristic revision didn't recognise the field name → couldn't emit a backfill plan. Usually triggered by a non-canonical prompt.
- **Fix:** Re-run with the canonical prompt verbatim:
  ```
  tighten Account.email to required for compliance
  ```
  (For F8: `tighten Customer.email to required for compliance`.)
- **Fallback narration:** *"`Stalled` is the honest outcome — the agent has three attempts, after which a human is paged. The structured-rejection contract still holds; every attempt is on the record. Let me re-run with the runbook-canonical prompt."*

## 8. Policy deny doesn't trigger (`team:marketing` succeeds with 200)

- **Symptoms:** Beat 7a deny path returns 200 + "Write committed" instead of 403 + red deny panel.
- **Diagnosis:** Daemon is serving a **pre-F5 binary**. This is the famously-caught daemon-restart issue (#9).
- **Fix:** Apply the restart rule:
  ```bash
  kill $(lsof -ti:3030); cargo build --bin agorad && DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad
  ```
- **Fallback narration:** *"Policy enforcement lives in the daemon's `POST /entities` handler — let me skip the deny sub-beat and narrate it: an actor outside the owner team gets a 403, and the deny attempt is logged as its own `DenyAttempt` row in `mutation_log` so the audit trail is complete. Beat 7c's verify won't flag it because no entity row exists."*

## 9. Daemon serving stale binary (THE restart bug — caught 4× in this build)

- **Symptoms:** Behaviour doesn't match the latest commit. New endpoints 404. New seed concepts don't appear. New axes don't fire.
- **Diagnosis:**
  ```bash
  ps -o pid,lstart,command -p $(lsof -ti:3030)
  git log -1 --format=%cI
  ```
  If the process start time is **before** the last commit, you're on the stale binary.
- **Fix (THE fix, memorise it):**
  ```bash
  kill $(lsof -ti:3030) && cargo build --bin agorad && \
    DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad
  ```
- **Fallback narration:** *"Live deploy after a merge — same procedure your CI would run. Single binary, restart in seconds."*

---

## When in doubt — the universal escape hatch

1. **Confess to the audience.** Name what you see. ("That count should be 47; it's 46 — a prior run modified the table. Let me reset.")
2. **Don't fight the demo.** If a beat truly won't recover in 30 seconds, narrate it from `DEMO.md` while moving to the next beat. The script's seven other beats land the thesis without any one of them.
3. **The system's job is to surface degraded modes, not hide them.** Every fallback above corresponds to a UI signal Agora already gives you (orange pill, `Skipped` axis, `Stalled` banner, `tampered_entities[]` array). Use them.
4. **If multiple things go wrong simultaneously,** abandon live demo, switch to narrating from `DEMO.md` v2.0 with `PITCH.md`'s killer demonstrations as the spine. The story still wins.

---

## Post-mortem hook (after the demo)

If anything fired here, jot a note in `STACK.md`'s decision log so the next run is smoother. Every cell that catches a real failure earns a row.
