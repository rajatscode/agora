//! The seven axes that make up Feature 2's multi-axis risk gate.
//!
//! Each module owns one axis. The orchestrator in `crate::check` calls every
//! axis on every proposal and collects the resulting `CheckRow`s into a
//! single `CheckReport`. Three of the axes are demo-load-bearing:
//!
//! - `shape`            — classifies the structural change (Beat 3, Beat 5)
//! - `semantic`         — LLM reasoning over meaning_before/after + invariants
//!                        (Beat 3, Beat 6: real LLM call, not regex)
//! - `data_conformance` — queries the live DB for rows that violate the
//!                        proposed constraint (Beat 6: 47 NULL emails)
//!
//! The remaining axes (composition / policy / temporal / impact / replay)
//! are deterministic and run in microseconds.

pub mod composition;
pub mod data_conformance;
pub mod impact;
pub mod policy;
pub mod replay;
pub mod semantic;
pub mod shape;
pub mod temporal;
