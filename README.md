# Cuttlebone

Cuttlebone, a language server for Inko.

It links the upstream compiler crates from a pinned release with a small
patch (see [patches/](patches/README.md)), type checks with them, and answers
questions about types over standard LSP, for any editor with an LSP client
([docs/lsp.md](docs/lsp.md)): hover shows the type at a position,
completion after `.` lists members with their signatures, and go to
definition, go to type definition and find references locate symbols by the
compiler's own ids. It publishes the same diagnostics as `inko check`,
highlights a symbol's uses in a file, shows the signature of the call being
typed, and shows inferred types and parameter names as inlay hints. In
code being edited, which doesn't parse or has an error in a declaration, it
answers from a repaired copy of the documents or from the last version
that type checked, wherever that answer still holds.

Reports, in docs/history/:

- [SPIKE.md](docs/history/SPIKE.md): the feasibility spike.
- [MILESTONE-1.md](docs/history/MILESTONE-1.md): the type service (with a
  custom protocol, since replaced).
- [MILESTONE-2.md](docs/history/MILESTONE-2.md): the language server.
- [GUARDRAILS.md](docs/history/GUARDRAILS.md): turning the standing rules
  into checks.
- [MILESTONE-3.md](docs/history/MILESTONE-3.md): definition, type
  definition and references.
- [MILESTONE-4.md](docs/history/MILESTONE-4.md): diagnostics, document
  highlight, signature help and inlay hints.
- [MILESTONE-5.md](docs/history/MILESTONE-5.md): answers in code being
  edited, from repaired copies and the last good check.
- [MILESTONE-6.md](docs/history/MILESTONE-6.md): one check for many
  questions, and the guardrails in tiers.

Each milestone has its own version; what changed in each:
[CHANGELOG.md](CHANGELOG.md).

Setting it up in VS Code and Neovim: [editors/](editors/README.md). The rules
every change follows, for people and coding agents: [AGENTS.md](AGENTS.md).

## Building

You need Rust (rust-toolchain.toml pins the version) and LLVM 18, because the
upstream compiler crate links LLVM even though Cuttlebone never generates
code. `.cargo/config.toml` assumes Homebrew's `llvm@18`. Set
`LLVM_SYS_181_PREFIX` if yours is somewhere else.

    scripts/upstream.sh      # clone the pinned release into .upstream/ and patch it
    cargo build --release

To install `cuttlebone` and `dump-types` into Cargo's `bin` directory
(usually `~/.cargo/bin`):

    cargo install --locked --path .

`--locked` builds with the versions in Cargo.lock, the ones the tests ran
against. Without it, Cargo picks the newest versions Cargo.toml allows.

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
  which nodes desugaring introduced, cross-checks of the types it
  reconstructs, and every name that refers to or declares a symbol, by the
  compiler's ids.
- `src/query.rs`: the checks kept for later questions about the same text,
  the type at a position, the members after a `.`
  (inserting a placeholder for completion), declarations and references,
  and catching compiler panics.
- `src/position.rs`: converting between the compiler's positions (1-based,
  grapheme clusters) and editors' (0-based, UTF-16 or UTF-8).
- `src/check.rs`: running a check, or a full one for diagnostics, with no
  side effects.
- `src/edits.rs`: which text of an older version of a file is still in
  the current one, unchanged, from the editor's edits, and the documents
  of each file's last good check.
- `src/units.rs`: which units (methods and other items) of a check of
  older text still mean what they did, for answers in code being edited.
- `src/worker.rs`: running queries in a child process, which keeps its
  checks between them.
- `src/lsp.rs`: the LSP front end: documents, the queue, timeouts,
  cancellation and debouncing, and diagnostics' own queue.
- `src/dump.rs`: helpers for dump-types and the tests only.

The core (`check.rs`, `edits.rs`, `units.rs`, `query.rs`, `walker.rs`, `position.rs`) doesn't know
about LSP. tests/guardrails.rs enforces the layering (see AGENTS.md).

## Tests

    scripts/install-hooks.sh          # once per clone: run scripts/check.sh before each commit
    scripts/check.sh                  # every commit: formatting, clippy, tests, the patch, the release build
    scripts/check.sh --milestone REV  # milestone reports: the whole corpus, no kept checks, mutations since REV
    scripts/check.sh --exhaustive     # every mutation, when the runner or shared test code changes
    cargo test                        # the tests alone (two binaries: tests/main.rs and tests/latency.rs)

AGENTS.md ("Checks") says which tier each step needs.

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
| `tests/conformance.rs` | LSP 3.18: the checks every test over LSP makes of each message it receives (tests/common/spec.rs: its shape against the spec's meta model, in tests/fixtures/lsp, and what the client's capabilities allow), and what a client with no optional capabilities gets |
| `tests/lsp.rs` | the real binary as an LSP server over stdio: `--version`, initialize, encodings, documents, hover, completion, status, panics, timeouts, aborts, cancellation, debouncing, side effects |
| `tests/definition.rs` | definition, type definition and references on every kind of name, same-named symbols in different modules, broken code, both encodings |
| `tests/navigation.rs` | the same over LSP, in UTF-8 and UTF-16, with status reasons, an installed std that isn't `.upstream/`, and a package in `dep/` |
| `tests/corpus_definition.rs` | every reference in std/src selects its own symbol, and goes to a declaration of the same id and name |
| `tests/diagnostics.rs` | the oracle: diagnostics match the installed `inko check --format json` exactly (file, lines, columns, level, ID, message, order) for every project in tests/fixtures/diagnostics, which between them produce every diagnostic `inko check` can, for std, and for unsaved text |
| `tests/publish.rs` | diagnostics over LSP: every file published without opening it, ranges in both encodings, every fixture project, edits that add and fix errors (and clear them), the debounce, queries first, superseded and hung checks, a compiler panic, what's a target, turning them off |
| `tests/highlight.rs` | document highlights over LSP in both encodings: bindings and assignments as writes, fields, same-named symbols, methods, types, closure arguments, `null`, broken code |
| `tests/signature.rs` | signature help over LSP in both encodings: positional and named arguments (in any order), nested calls, type arguments, unclosed argument lists while typing, `null` outside parentheses, for operators and unresolved methods |
| `tests/inlay.rs` | inlay hints over LSP in both encodings: every rule for type and parameter hints, and the requested range |
| `tests/latency.rs` | latency limits for every query (each a check of its own), alone and while a diagnostics check of the same project runs, a burst of 1,000 definitions (one check), and the cost of a worker process |
| `tests/golden.rs` | `dump-types` output for a few std modules and fixtures, compared with `tests/golden/*.txt`. Run `UPDATE_GOLDEN=1 cargo test` to rewrite them. |
| `tests/editors.rs` | the Neovim smoke check (below) under `cargo test`, if `nvim` is on `PATH` |
| `tests/guardrails.rs` | the layering of src/, no editor named in code, nothing named after Inko, no machine-specific paths, patches/README.md accounting for every patch line, a CHANGELOG.md entry for the version, and docs showing it |
| `tests/repaired.rs` | fresh answers in code that doesn't parse, from a repaired copy, over LSP: the rest of the file, completion, before the edit in the method being edited, a placeholder type the blank lines could fill in, a changed declaration, and references never |
| `tests/stale.rs` | answers from the last good check over LSP (with repaired copies off, a fault only tests have): which requests give them (and which never do), positions and ranges mapped through edits in both encodings, a changed declaration, diagnostics checks and unsaved text when opened, hints in the method being edited |
| `tests/replay.rs` | the edit-replay corpus, the oracle for code being edited: real files edited as people do, every query asked at many positions after each edit, and each answer compared with the fresh one at the end. Fails on any wrong answer, or less coverage than measured. Every commit replays a sample; `CUTTLEBONE_CORPUS_FULL=1` replays all of it. `-- --nocapture` prints the tables |
| `tests/cache.rs` | checks kept between questions: questions about the same text share one, with the same answers (fresh, repaired and stale); an edit, a file changed, created or deleted on disk (also outside the editor, over LSP) is never answered from one; what may have changed during a check isn't kept; the bound and least recently used order; completion's copies and panicked checks not kept; `cached` in the status; the worker replaced when it dies, stopped at `shutdown` |
| `tests/corpus.rs` | every expression in std (src/ and test/) has a type, except for the allow-listed kinds |

`scripts/mutate.py` breaks behaviours and rules on purpose, one at a time,
and checks that a test (or clippy, or a check script) fails for each,
running several mutations' tests at once. `--since REV` runs only those a
change since REV could affect, and `scripts/check-selection.py` shows that
this selection would have run every mutation guarding code the last three
milestones changed.

`scripts/check.sh` also checks that `patches/upstream.patch` applies to a
fresh copy of the pinned release and matches `.upstream/inko`
(`scripts/check-patch.sh`), and that the release server contains no
test-only code (`scripts/check-release.sh`). Tests build with the
`fault-injection` feature (docs/lsp.md, "Testing faults"); release builds
don't.

`editors/check.sh` starts the server from VS Code and Neovim headlessly and
checks that a hover and a go to definition work, and that a diagnostic
appears for an error and clears when it's fixed (see
[editors/](editors/README.md)).
