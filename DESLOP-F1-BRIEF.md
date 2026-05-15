# Deslopper F1 Brief

**Role:** Review Feature 1's code for quality, maintainability, and architectural fit.

**Your job:** Identify AI-generated slop, over-abstraction, dead code, and coherence issues. Recommend specific fixes.

---

## What to Review

Feature 1 code lives at: `.claude/worktrees/feature-1-propose-cli/src/`

Key files:
- `main.rs` — CLI entry point
- `cli.rs` — argument parsing
- `llm.rs` — Anthropic SDK integration
- `reuse.rs` — 3-layer reuse detection (core logic)
- `ast.rs` — OntologyChangeProposal AST
- `artifacts.rs` — code generation for 4 artifacts
- `seed.rs` — pre-seeding canonical concepts
- `lib.rs` — library exports

---

## Red Flags for AI Slop

### Over-abstraction
- [ ] Too many trait definitions for what's needed?
- [ ] Helper functions that are called once?
- [ ] Unnecessary indirection (factory functions, builders) for simple types?
- [ ] Generic parameters that aren't actually generic?

**Example to watch for:**
```rust
// BAD: over-engineered
pub trait ProposalClassifier {
    fn classify(&self, p: &Proposal) -> Classification;
}

// GOOD: just a function
pub fn classify_proposal(p: &Proposal) -> Classification { ... }
```

### Dead Code
- [ ] Unused imports or dependencies?
- [ ] Private functions never called?
- [ ] Dead branches or unreachable code?
- [ ] Test-only code left in the binary?

### Unnecessary Comments
- [ ] Comments explaining WHAT instead of WHY?
- [ ] Comments on obvious code (e.g., "increment counter" over `i += 1`)?
- [ ] Multi-line docstrings that just paraphrase the function signature?

**Good comments explain non-obvious decisions or constraints:**
- "Hashed-bag-of-words instead of fastembed-rs due to TLS conflict — swap via default_embedder() trait"
- "Layer 1 exact-match must run first; it's O(1) and catches obvious dupes"

### Copy-Paste Patterns
- [ ] Similar code blocks that should be extracted?
- [ ] Repeated logic in different modules?
- [ ] Inconsistent error handling patterns?

### Type Safety
- [ ] Using `String` when an enum would be clearer?
- [ ] Using `Vec<Any>` or loose JSONs when types should be strict?
- [ ] Unsafe blocks without justification?

---

## Architectural Coherence

### Does it fit the stack?
- [ ] Uses Axum for HTTP? (Yes, it generates Axum handlers)
- [ ] Uses Anthropic SDK properly? (Yes, structured-output + fallback)
- [ ] Async/await used consistently? (Check `tokio` runtime)
- [ ] Serialization uses `serde`? (Check Proposal struct derives)

### Trait Design
- [ ] `Embedder` trait is clean and minimal? (Should allow swapping fastembed-rs)
- [ ] No unnecessary generic bounds?
- [ ] Trait methods match actual usage?

### Error Handling
- [ ] Errors are typed (using `thiserror` or `anyhow`)?
- [ ] Propagation vs recovery is clear?
- [ ] No `.unwrap()` or `.panic!()` outside tests?
- [ ] User-facing errors are clear?

### Testing
- [ ] Unit tests exist for core logic (reuse detection)?
- [ ] Tests are focused, not testing implementation details?
- [ ] Mocks/fixtures for LLM (since we don't want to call the API in tests)?

---

## Specific Things to Check

### `reuse.rs` (Critical)
- [ ] 3-layer classification is clear and well-commented
- [ ] Hashed-bag-of-words embedder is deterministic (check: no RNG, uses SHA256)
- [ ] Scoring logic (0.5 * jaccard + 0.5 * cosine) is intentional, not arbitrary
- [ ] Edge cases handled: empty catalogue, empty text, division by zero in cosine

### `artifacts.rs` (Critical)
- [ ] String templating for .proto, .sql, _handler.rs, .fga.json is clean
- [ ] No copy-paste between artifact generators (extract common patterns)
- [ ] Output files are written correctly (check: directory creation, file names, formatting)
- [ ] Generated code is readable (not minified, has comments)

### `llm.rs` (Critical)
- [ ] Anthropic API call is structured-output (tool-use, not prompt)
- [ ] Offline fallback is deterministic (not a random fake)
- [ ] Error handling: timeouts, rate limits, invalid responses
- [ ] Proposal JSON validation after LLM call

### `cli.rs`
- [ ] Argument parsing is straightforward?
- [ ] Help text is clear?
- [ ] Error messages guide the user?

---

## Design Questions to Ask

1. **Proposal JSON schema:** Is it locked? (Check: 11 required fields, all present)
2. **Concept catalogue:** Where does it come from? (Check: `seed.rs`, hard-coded, or registry?)
3. **Generated artifacts directory:** How are paths constructed? (Check: no path injection issues)
4. **Determinism:** Is reuse detection deterministic? (Important for demo repeatability)

---

## What NOT to Worry About (Out of Scope)

- Optimization (we care about correctness, not speed)
- Logging verbosity (feature-specific, not a slop issue)
- Edge cases beyond the demo (e.g., handling 10k concepts — M0 is small)

---

## Sign-Off Criteria

Sign off when:
- ✓ No obvious AI slop (over-abstraction, dead code, unnecessary comments)
- ✓ Error handling is consistent and user-facing
- ✓ Core logic (reuse detection, artifacts) is clean and maintainable
- ✓ Code fits the Agora stack (Axum, serde, anyhow/thiserror, async)
- ✓ Tests exist for critical functions
- ✓ Specific recommendations recorded (if any minor issues found)

---

## If You Find Issues

**Minor issues:** Document them; they don't block integration, but impl-f1 should know.
- "Unused import in artifacts.rs line 42"
- "Copy-paste between .proto and .sql generators could be extracted"
- "Error message for missing ANTHROPIC_API_KEY could be clearer"

**Major issues:** Block integration and ask impl-f1 to fix.
- Unwrap/panic in non-test code
- Significant over-abstraction
- Missing error handling on critical path
- Dead code that confuses the narrative

---

## Timeline

- val-f1 finishes: ~23:50 ET
- You start reviewing: ~23:50 ET (parallel with meta-val-f1)
- You finish: ~00:35 ET (45 min review window)
- Sign off or ask for fixes: 00:35 ET

**Go wide and shallow first** (scan all files for obvious issues), then **go deep on critical modules** (reuse.rs, artifacts.rs, llm.rs).

---

## How to Actually Do This

1. Clone/reference the worktree: `.claude/worktrees/feature-1-propose-cli/`
2. Open each `.rs` file and skim for the red flags above
3. For critical modules, read carefully
4. Make a list of issues (major vs minor)
5. Record your sign-off: "deslop-f1 validates code quality; issues: [list]"

That's it.
