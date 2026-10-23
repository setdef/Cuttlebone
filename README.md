# Cuttlebone

Cuttlebone, a language server for Inko.

It links the upstream compiler crates from a pinned release with a small
patch (see [patches/](patches/README.md)), type checks with them, and answers
questions about types over standard LSP, for any editor with an LSP client
([docs/lsp.md](docs/lsp.md)): hover shows the type at a position, and
completion after `.` lists members with their signatures.

- [SPIKE.md](SPIKE.md): the feasibility spike.
- [MILESTONE-1.md](MILESTONE-1.md): the type service (with a custom protocol,
  since replaced).
- [MILESTONE-2.md](MILESTONE-2.md): the language server.
- [editors/](editors/README.md): setting it up in VS Code and Neovim.

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
- `src/lib.rs`: running a check, with no side effects.
- `src/worker.rs`: running one query in a child process.
- `src/lsp.rs`: the LSP front end: documents, the queue, timeouts,
  cancellation and debouncing.

The core (`lib.rs`, `query.rs`, `walker.rs`, `position.rs`) doesn't know
about LSP.

## Tests

    cargo test

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
| `tests/corpus.rs` | every expression in std (src/ and test/) has a type, except for the allow-listed kinds |

`scripts/mutate.py` breaks behaviours on purpose, one at a time, and checks
that a test fails for each.

`editors/check.sh` starts the server from VS Code and Neovim headlessly and
checks that a hover works (see [editors/](editors/README.md)).
