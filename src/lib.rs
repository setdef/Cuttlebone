//! Cuttlebone, a language server for Inko.
//!
//! It type checks a project with the upstream compiler (patched to expose
//! the typed HIR; see patches/README.md), walks the typed HIR, and answers
//! questions about the types of expressions.
//!
//! The core (`check`, `query`, `walker`, `syntax` and `position`) knows nothing about
//! editors or LSP. `worker` runs one query in a child process, and `lsp` is
//! the language server front end. `dump` holds helpers for dump-types and
//! the tests. tests/guardrails.rs enforces this layering.
#![forbid(unsafe_code)]

pub mod check;
#[doc(hidden)]
pub mod dump;
pub mod edits;
pub mod index;
pub mod lsp;
pub mod position;
pub mod query;
pub mod syntax;
pub mod units;
pub mod walker;
pub mod worker;
