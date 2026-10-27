//! The integration tests, as one test binary, so that a build links one
//! test binary instead of one per file (each links the compiler), and the
//! tests run in parallel. Run one file's tests with its module's name as
//! the filter: `cargo test --test integration lsp::`.
//!
//! tests/latency.rs is a binary of its own, so that it measures latency
//! without the rest of the tests loading the machine.
mod common;

mod actions;
mod broken;
mod cache;
mod citations;
mod complete;
mod conformance;
mod corpus;
mod corpus_definition;
mod corpus_symbols;
mod crosscheck;
mod definition;
mod diagnostics;
mod editors;
mod folding;
mod formatting;
mod golden;
mod guardrails;
mod highlight;
mod hover;
mod inlay;
mod lsp;
mod manifest;
mod members;
mod names;
mod navigation;
mod overlay;
mod panic;
mod publish;
mod repaired;
mod replay;
mod signature;
mod stale;
mod symbols;
mod syntax;
mod type_at;
mod undo;
