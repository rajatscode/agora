# Agora — Handoff

Good morning. The build is done. This file orients you in under a minute. Read it first.

---

## 1. Status as of 06:43 ET

- **9 features shipped** — F1 propose · F2 7-axis check · F3 write/verify/explorer · F4 browser UI · F5 policy enforcement · F6 agent loop · F8 Customer 360 · F9 Compliance/GRC · F-DAEMON HTTP control plane.
- **84 tests passing** (`cargo test`).
- **Demo verified clean from a cold start** — all 8+1 beats land end-to-end against a fresh `agora_dev` database.
- **Daemon:** should still be running on `http://localhost:3030`. If it isn't, see step 2.

You ran the entire build in **offline mode**. No `ANTHROPIC_API_KEY` was set; the system worked fully without it. That's the validated demo posture.

---

## 2. First steps (2 minutes)

```bash
# Is the daemon up?
curl -s localhost:3030/health
# expect: {"status":"ok","db":"connected"}

# If not, from repo root:
DATABASE_URL=postgres://localhost/agora_dev cargo run --bin agorad

# Then open the UI:
open http://localhost:3030/
```

If `/health` returns `{"db":"disconnected"}`, Postgres is the problem — `brew services restart postgresql@14`, then restart the daemon.

---

## 3. Read in this order

| When | File | What it gives you |
|---|---|---|
| **Before the demo** | `DEMO-RUNBOOK.md` | The on-stage script. 12-minute walk, click-by-click, beat-by-beat. |
| **Before the demo** | `PITCH.md` | The 1-pager and the 15 anticipated judge questions with honest answers. |
| **Have it open during the demo** | `OPS-PLAYBOOK.md` | What to do if anything breaks mid-demo. 9 failure cards, paste-able commands. |
| Background | `DEMO.md` | Full beat-by-beat narrative — why each beat exists, what cannot be faked. |
| Background | `README.md` | Architecture diagram + the five critical proofs + repo layout. |

If you have 10 minutes: read DEMO-RUNBOOK end-to-end and skim PITCH's Q&A headings. That's enough to take the stage.

---

## 4. API key status

The whole build, every dry-run, and the on-stage script were validated in **offline mode**. The author-mode pill on every proposal card honestly reports `offline · no API key`. The deterministic offline path emits structurally valid proposals; the gate, the agent loop, and verify all behave identically.

If you want live LLM for the judge run, set `ANTHROPIC_API_KEY` **before booting the daemon** (the mode is cached at boot) — see the "LLM mode + quota" section at the top of `OPS-PLAYBOOK.md`. Plan for 4–8 Anthropic calls per session. Default to offline unless live is specifically required.

---

## 5. If something looks off

```bash
# Restore demo baselines (47 NULL accounts.email, 5 NULL customers.email,
# 4 NULL audit_findings.email, 20 customers, 15 audit_findings,
# 3 BankIntegration rows):
curl -X POST localhost:3030/admin/reset
```

If state still seems stale after a reset, the daemon is probably on a stale binary — use the **daemon-restart procedure** in `OPS-PLAYBOOK.md` (failure card #9, "THE restart bug"). Caught four times during the build; the runbook now spells it out.

---

You've got everything you need. Good demo.
