//! The integration tests, as one test binary, so that a build links one
//! test binary instead of one per file (each links the compiler), and the
//! tests run in parallel. Run one file's tests with its module's name as
//! the filter: `cargo test --test integration lsp::`.
//!
//! tests/latency.rs is a binary of its own, so that it measures latency
//! without the rest of the tests loading the machine.
mod common;

mod broken;
mod cache;
mod citations;
mod complete;
mod conformance;
mod corpus;
mod corpus_definition;
mod crosscheck;
mod definition;
mod diagnostics;
mod editors;
mod golden;
mod guardrails;
mod highlight;
mod inlay;
mod lsp;
mod manifest;
mod members;
mod navigation;
mod overlay;
mod panic;
mod publish;
mod repaired;
mod replay;
mod signature;
mod stale;
mod type_at;
