# Meta-Validator F1 Brief

**Role:** Validate that val-f1's validation was thorough and complete.

**Your job is NOT to re-test everything.** You're auditing the validator's approach.

---

## What to Read First

- val-f1's validation report (they'll provide this)
- impl-f1's code at `.claude/worktrees/feature-1-propose-cli/`
- FEATURE-1 implementation outline (below)

---

## Feature 1 Overview

Feature 1 (Proposal Compiler + LLM Authoring CLI) does:
1. **CLI entry point:** `agora propose "<user request>"`
2. **LLM call:** Uses Anthropic structured-output to author a semantic `OntologyChangeProposal` JSON
   - Fallback: offline deterministic proposal if API unavailable
3. **Reuse detection:** 3-layer (exact match → Jaccard similarity → hashed-bag-of-words embedding)
   - Classifies as New | Reuse | Refinement | Duplicate
4. **4 artifacts:** emits .proto, .sql, _handler.rs, .fga.json to `./generated/`

---

## What val-f1 Should Have Tested

**Happy path:**
- [ ] CLI runs: `cargo run -- propose "..."`
- [ ] LLM call succeeds (or offline fallback)
- [ ] Valid OntologyChangeProposal JSON with all 11 required fields
- [ ] Reuse detection runs and classifies against seeded concepts
- [ ] All 4 artifacts emit to `./generated/{proposal_id}/`

**Reuse detection specifics:**
- [ ] "biometric login" proposal → matches `AuthenticationMethod` (semantic similarity)
- [ ] Exact match on existing concepts (Layer 1)
- [ ] Novel proposals (Layer 2/3) still get classified
- [ ] Classification scores are reasonable (0.0-1.0 range)

**Artifacts validity:**
- [ ] .proto: valid protobuf syntax (can be linted with `buf`)
- [ ] .sql: runnable DDL (correct ALTER/CREATE syntax)
- [ ] _handler.rs: syntactically valid Rust (can compile)
- [ ] .fga.json: valid JSON with schema_version, type_definitions, tuples

**Edge cases:**
- [ ] Empty/nonsensical prompts → graceful behavior
- [ ] Malformed JSON parsing → proper error
- [ ] Missing environment variables → fallback or error message

---

## What YOU Should Validate (Meta-Level)

### Did val-f1 Actually Test These?

1. **CLI execution:** Did they actually run the command, or just read the code? Look for evidence of actual invocations (e.g., "I ran `cargo run -- propose ...` and got output X").

2. **LLM reasoning:** Did they test the real LLM call or just assume it works? If using offline fallback, did they verify the fallback behaves correctly?

3. **Artifacts exist and are readable:** Did they check all 4 artifact files exist AND can be read/parsed? Or just that the code compiles?

4. **Reuse detection depth:** Did they test:
   - Multiple proposals (not just one)?
   - Both exact matches and similarity-based matches?
   - Edge cases (empty catalogue, nonsense input)?
   - Or did they just run the test suite?

5. **Integration:** Did they actually run the CLI and verify the full pipeline (proposal → reuse detect → artifact generation) works end-to-end? Or did they just test individual functions?

---

## Red Flags (When to Send Validator Back)

- "I ran the tests" ← Good, but did you test the CLI manually?
- "Artifacts compile" ← OK, but are they valid (proto lints, SQL parses)?
- "No edge case tests" ← Ask them to test nonsense input, empty prompts, malformed JSON
- "Validation was just cargo test" ← Not enough. Real CLI usage is critical.
- "Reuse detection matches AuthenticationMethod" ← Great! But did they test other concepts too?

---

## What to Ask for in Validation Report

If val-f1's report is thin, ask them to clarify:

1. **Which specific proposals did you test?** (List the prompts)
2. **Did you verify all 4 artifacts were emitted?** (List the files)
3. **Did you test the offline fallback?** (How did you trigger it?)
4. **Did you check artifact validity?** (Proto linting, SQL syntax, Rust compilation, JSON parse)
5. **Any failures or unexpected behavior?** (Timeouts, crashes, invalid output)

---

## Sign-Off Criteria

You sign off on val-f1's work when:
- ✓ They tested the CLI end-to-end (not just unit tests)
- ✓ They verified reuse detection works (at least 2 different proposals)
- ✓ They confirmed all 4 artifacts are real, readable files
- ✓ They tested an edge case (malformed input, API unavailable, etc.)
- ✓ They provide a clear validation report with evidence (examples, outputs, file paths)

If any of these is missing, send them back with a specific ask.

---

## Timeline

- val-f1 finishes: ~23:50 ET
- You receive their report: ~23:52 ET
- You validate the validation: ~23:55 ET → 00:05 ET (15 min window)
- Sign off or send back: 00:05 ET
- deslop-f1 can run in parallel (they start when val-f1 finishes, don't wait for you)

**Go deep, but move fast.**

---

## How to Actually Do This

1. Read val-f1's report carefully
2. Check if they mention:
   - Actual CLI invocations (not just "the code looks right")
   - Specific artifact outputs (not just "artifacts were generated")
   - Edge case testing
3. If report is vague, ask specific clarifying questions
4. Once satisfied, record your sign-off: "meta-val-f1 validates that val-f1 tested [X, Y, Z] comprehensively."

That's it. You're not redoing their tests; you're validating they did it right.
