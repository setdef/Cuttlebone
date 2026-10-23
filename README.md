# Cuttlebone

Cuttlebone, a language server for Inko.

It links the upstream compiler crates from a pinned release with a small
patch (see [patches/](patches/README.md)), type checks with them, and answers
questions about types over standard LSP, for any editor with an LSP client
([docs/lsp.md](docs/lsp.md)): hover shows the type at a position, and
completion after `.` lists members with their signatures.

Reports, in docs/history/:

- [SPIKE.md](docs/history/SPIKE.md): the feasibility spike.
- [MILESTONE-1.md](docs/history/MILESTONE-1.md): the type service (with a
  custom protocol, since replaced).
- [MILESTONE-2.md](docs/history/MILESTONE-2.md): the language server.
- [GUARDRAILS.md](docs/history/GUARDRAILS.md): turning the standing rules
  into checks.

Setting it up in VS Code and Neovim: [editors/](editors/README.md). The rules
every change follows, for people and coding agents: [AGENTS.md](AGENTS.md).

## Building

You need Rust (rust-toolchain.toml pins the version) and LLVM 18, because the
upstream compiler crate links LLVM even though Cuttlebone never generates
code. `.cargo/config.toml` assumes Homebrew's `llvm@18`. Set
`LLVM_SYS_181_PREFIX` if yours is somewhere else.

    scripts/upstream.sh      # clone the pinned release into .upstream/ and patch it
    cargo build --release

## Binaries

- `cuttlebone`: the language server, on stdin and stdout
  ([docs/lsp.md](docs/lsp.md)). `cuttlebone --version` prints its version
  and the Inko version it links. It runs each check in a child process of
  itself (`cuttlebone --worker`).
- `dump-types`: prints every expression in a project or file with its type,
  for debugging and the golden tests.

      target/release/dump-types path/to/project          # or path/to/file.inko
      target/release/dump-types --summary path/to/project

## Layout

- `src/walker.rs`: walks the typed HIR, recording each expression's type,
  which nodes desugaring introduced, and cross-checks of the types it
  reconstructs.
- `src/query.rs`: the type at a position, and the members after a `.`
  (inserting a placeholder for completion), and catching compiler panics.
- `src/position.rs`: converting between the compiler's positions (1-based,
  grapheme clusters) and editors' (0-based, UTF-16 or UTF-8).
- `src/check.rs`: running a check, with no side effects.
- `src/worker.rs`: running one query in a child process.
- `src/lsp.rs`: the LSP front end: documents, the queue, timeouts,
  cancellation and debouncing.
- `src/dump.rs`: helpers for dump-types and the tests only.

The core (`check.rs`, `query.rs`, `walker.rs`, `position.rs`) doesn't know
about LSP. tests/guardrails.rs enforces the layering (see AGENTS.md).

## Tests

    scripts/install-hooks.sh   # once per clone: run scripts/check.sh before each commit
    scripts/check.sh           # formatting, clippy, tests, the patch, the release build
    scripts/check.sh --full    # and the mutation runner (several minutes)
    cargo test                 # the tests alone

Dependencies (the compiler) are optimized even in debug builds, so this is
also fast enough for the latency limits.

| Test | Checks |
|---|---|
| `tests/type_at.rs` | which expression a position means (desugared array literals, string interpolation, `for` and `while` loops, ...), UTF-16 positions after non-ASCII text |
| `tests/members.rs` | the members of `receiver.name`: visibility, static and instance methods, type arguments, modules, type parameters |
| `tests/complete.rs` | completion's own placeholder, and converting positions to byte offsets |
| `tests/manifest.rs` | the Inko version a project's manifests require, and never syncing `dep/` |
| `tests/broken.rs` | answers for code with errors: an unknown method, an undefined type in a signature, a syntax error, the placeholder |
| `tests/overlay.rs` | unsaved contents replacing files on disk |
| `tests/panic.rs` | a compiler panic is reported, and the next request works |
| `tests/crosscheck.rs` | reconstructed types agree with what the compiler stores |
| `tests/citations.rs` | the upstream lines the code cites still say what it mirrors |
| `tests/lsp.rs` | the real binary as an LSP server over stdio: initialize, encodings, documents, hover, completion, status, panics, timeouts, aborts, cancellation, debouncing, side effects |
| `tests/latency.rs` | latency limits, and the cost of a worker process |
| `tests/golden.rs` | `dump-types` output for a few std modules and fixtures, compared with `tests/golden/*.txt`. Run `UPDATE_GOLDEN=1 cargo test` to rewrite them. |
| `tests/guardrails.rs` | the layering of src/, no editor named in code, nothing named after Inko, no machine-specific paths, and patches/README.md accounting for every patch line |
| `tests/corpus.rs` | every expression in std (src/ and test/) has a type, except for the allow-listed kinds |

`scripts/mutate.py` breaks behaviours and rules on purpose, one at a time,
and checks that a test (or clippy, or a check script) fails for each.

`scripts/check.sh` also checks that `patches/upstream.patch` applies to a
fresh copy of the pinned release and matches `.upstream/inko`
(`scripts/check-patch.sh`), and that the release server contains no
test-only code (`scripts/check-release.sh`). Tests build with the
`fault-injection` feature (docs/lsp.md, "Testing faults"); release builds
don't.

`editors/check.sh` starts the server from VS Code and Neovim headlessly and
checks that a hover works (see [editors/](editors/README.md)).
