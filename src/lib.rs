//! Agora — governed operational ontology control plane (single crate).
//!
//! Workstream-A modules in this build:
//!   - `ast`        : shared ontology types (per STACK.md Artifact 1)
//!   - `cli`        : `agora` command-line entry points
//!   - `llm`        : Anthropic structured-output authoring
//!   - `reuse`      : three-layer reuse detection (exact / Jaccard / embeddings)
//!   - `artifacts`  : `.proto`, DDL, HTTP-handler, and OpenFGA tuple emitters
//!   - `seed`       : seed concept catalog (so the CLI can detect reuse offline)
//!
//! Other workstreams will fill in `api/`, `diff`, `policy`, `runtime`.

pub mod ast;
pub mod cli;
pub mod llm;
pub mod reuse;
pub mod artifacts;
pub mod seed;
