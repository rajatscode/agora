# Agora — Handoff

The build is done. This file orients you in under a minute. Read it first.

---

## 1. Status

- **Nine flows wired end-to-end** — propose · 7-axis check · write/verify/explorer · browser UI · policy enforcement · agent loop · Customer 360 · Compliance/GRC · HTTP control plane.
- The end-to-end walk runs clean from a cold start against a fresh `agora_dev` database.
- **Daemon:** should still be running on `http://localhost:3030`. If it isn't, see step 2.

The build ran in **offline mode** the whole time. No `ANTHROPIC_API_KEY` was set; the system worked fully without it. That is the validated default posture.

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
| First | `README.md` | What Agora is, the problem it solves, prior art, alternative architectures, FAQ, repo layout. |
| Before walking through it | `DEMO-RUNBOOK.md` | Click-by-click narration with the falsifiability check per step. |
| Have open while walking through it | `OPS-PLAYBOOK.md` | Nine failure cards with paste-able recovery commands. |
| Deeper context | `DEMO.md` | Full beat-by-beat narrative with the "cannot be faked" note per beat. |
| Deeper context | `STACK.md` | Locked stack decisions and what we explicitly do not use. |

If you have ten minutes: skim README end-to-end and then DEMO-RUNBOOK.

---

## 4. API key status

The whole build was validated in **offline mode**. The author-mode pill on every proposal card honestly reports `offline · no API key`. The deterministic offline path emits structurally valid proposals; the gate, the agent loop, and verify all behave identically.

If you want live LLM, set `ANTHROPIC_API_KEY` **before booting the daemon** (the mode is cached at boot) — see the "LLM mode + quota" section at the top of `OPS-PLAYBOOK.md`. Plan for ~2 Anthropic calls per walk-through. Default to offline unless live is specifically required.

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

You've got everything you need.
