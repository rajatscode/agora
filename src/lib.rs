//! Agora — governed operational ontology control plane (single crate).
//!
//! Module map:
//!   - `ast`            : shared ontology types (per STACK.md Artifact 1)
//!   - `cli`            : `agora` command-line entry points (propose, check)
//!   - `llm`            : Anthropic structured-output authoring (Feature 1)
//!   - `reuse`          : three-layer reuse detection (Feature 1)
//!   - `artifacts`      : `.proto`, DDL, HTTP-handler, OpenFGA emitters (Feature 1)
//!   - `seed`           : seed concept catalog
//!   - `check`          : multi-axis risk gate orchestrator (Feature 2)
//!   - `check_report`   : CheckReport types serialized to disk (Feature 2)
//!   - `axes`           : individual axis implementations (Feature 2)
//!   - `auto_approval`  : auto-approval threshold (Feature 2)
//!   - `db`             : Postgres connection + migrations (Feature 2)
//!
//! Other workstreams will fill in `api/`, `diff`, `policy`, `runtime`.

pub mod ast;
pub mod cli;
pub mod llm;
pub mod reuse;
pub mod artifacts;
pub mod seed;

pub mod check;
pub mod check_report;
pub mod axes;
pub mod auto_approval;
pub mod db;
