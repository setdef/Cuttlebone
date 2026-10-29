# Cuttlebone

[![Certified Shovelware](https://justin.searls.co/img/shovelware.svg)](https://justin.searls.co/shovelware/)

Cuttlebone, a language server for Inko.

It links the upstream compiler crates from a pinned release with a small
patch (see [patches/](patches/README.md)), type checks with them, and answers
questions about types over standard LSP, for any editor with an LSP client
([docs/lsp.md](docs/lsp.md)): hover shows the type at a position and the
declaration there with its doc comment, document and workspace symbols list
declarations from the syntax alone, completion after `.` lists members with
their signatures, and completion of names lists what's in scope and what
to import, with the import, and go to
definition, go to type definition and find references locate symbols by the
compiler's own ids. It publishes the same diagnostics as `inko check`,
highlights a symbol's uses in a file, shows the signature of the call being
typed, and shows inferred types and parameter names as inlay hints. It
folds blocks, comments and imports, formats exactly as `inko fmt` does,
and offers quick fixes for its diagnostics, each checked to remove its
diagnostic and add no error. It finds a trait's implementations, the trait method a method
declares, call and type hierarchies (all from the type database), and
all of a document's as code lenses in one request, expands selections from
the syntax, rewrites imports when files move, and answers pull diagnostics
for the whole project, also past a module that doesn't parse. Where a
declaration has an error, it type checks the method bodies anyway, and
answers wherever the error can't matter. In code being edited that
doesn't parse, it answers from a repaired copy of the documents or from
the last version that type checked, wherever that answer still holds.

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
- [MILESTONE-7.md](docs/history/MILESTONE-7.md): what an answer covers,
  versions, LSP 3.18, and a hole in the guardrails.
- [MILESTONE-8.md](docs/history/MILESTONE-8.md): answers when a
  declaration has an error.
- [MILESTONE-9.md](docs/history/MILESTONE-9.md): cheaper answers while
  typing: questions with no answer kept, and cancelling without killing.
- [MILESTONE-10.md](docs/history/MILESTONE-10.md): prebuilt binaries
  on GitHub releases, and a manifest for clients.
- [MILESTONE-11.md](docs/history/MILESTONE-11.md): a license, a release
  index, and a client's requests (ids for declaration errors, missing
  modules, compound assignments, saves, references outside the project).
- [MILESTONE-12.md](docs/history/MILESTONE-12.md): documentation in
  hover, document and workspace symbols, and completion of names.
- [MILESTONE-13.md](docs/history/MILESTONE-13.md): answers after an undo
  and a closed file changed on disk, folding ranges, formatting, quick
  fixes, and the release tier in CI.
- [MILESTONE-14.md](docs/history/MILESTONE-14.md): implementations,
  declarations, call and type hierarchies, selection ranges, file
  renames, pull diagnostics, and latency limits scaled per runner.
- [MILESTONE-15.md](docs/history/MILESTONE-15.md): quick fixes in files
  with other errors and four more, names in code being edited and
  keywords, steadier document symbols, constructors' signature help,
  folds' collapsed text, and workspace symbols for an index.
- [MILESTONE-16.md](docs/history/MILESTONE-16.md): diagnostics past a
  module that doesn't parse, whole-project answers past a declaration
  error elsewhere, code lenses, a status for every file rename, a
  selection step with doc comments, and a trait's default methods.
- [MILESTONE-17.md](docs/history/MILESTONE-17.md): modules as workspace
  symbols, where an index finds each part of a declaration, std's builtin
  types public, cheaper document symbols, and answers in an installed
  std's files.

Each milestone has its own version; what changed in each:
[CHANGELOG.md](CHANGELOG.md).

Setting it up in VS Code and Neovim: [editors/](editors/README.md). The rules
every change follows, for people and coding agents: [AGENTS.md](AGENTS.md).

## Installing

Each [release](https://github.com/setdef/Cuttlebone/releases) has a
prebuilt `cuttlebone` for macOS (15 or newer) and Linux (any
distribution), on AMD64 and ARM64. Each build works with one Inko version,
which its name says: pick the one for the `inko --version` you have.
For example, on Linux on AMD64 with Inko 0.21.1:

    version=0.17.0 inko=0.21.1 target=x86_64-unknown-linux-musl
    name=cuttlebone-$version-inko-$inko-$target
    curl -fLO https://github.com/setdef/Cuttlebone/releases/download/v$version/$name.tar.gz
    tar -xzf $name.tar.gz
    install $name/cuttlebone ~/.local/bin/    # or anywhere on PATH

The targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`. The Linux
binaries are static, so they run on glibc and musl distributions alike.
Each binary is self-contained: it needs Inko itself, and nothing else.
Each release's `manifest.json` lists its builds with their sizes and
SHA-256, for clients that install Cuttlebone for their users, and
`releases/latest/download/index.json` lists every release with its Inko
versions, for users of an older Inko ([docs/releases.md](docs/releases.md)).

## Building from source

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
- `src/syntax.rs`: what a module's text alone says, without a check: its
  declarations, with their signatures as written and doc comments as
  `inko doc` reads them, and its imports, also from a copy with lines
  blank where the text doesn't parse.
- `src/index.rs`: the declarations of every module a session can import
  (std's, the projects' and their dependencies'), kept between requests,
  for workspace symbols and completion's imports.
- `src/query.rs`: the checks kept for later questions about the same text,
  the type at a position, the members after a `.`
  (inserting a placeholder for completion), declarations and references,
  and catching compiler panics.
- `src/position.rs`: converting between the compiler's positions (1-based,
  grapheme clusters, lines ending at `\n`) and editors' (0-based, UTF-16 or
  UTF-8, lines ending at `\n`, `\r\n` or `\r`).
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
  cancellation and debouncing, requests answered for the text they were about, and diagnostics' own queue.
- `src/dump.rs`: helpers for dump-types and the tests only.

The core (`check.rs`, `edits.rs`, `units.rs`, `query.rs`, `walker.rs`, `syntax.rs`, `index.rs`, `position.rs`) doesn't know
about LSP. tests/guardrails.rs enforces the layering (see AGENTS.md).

## Tests

    scripts/install-hooks.sh          # once per clone: run scripts/check.sh before each commit
    scripts/check.sh                  # every commit: formatting, clippy, tests, the patch, the release build
    scripts/check.sh --milestone REV  # milestone reports: the whole corpus, no kept checks, mutations since REV
    scripts/check.sh --release        # CI, before each release: the same, without latency limits or mutations
    scripts/check.sh --latency        # CI, before each release: the latency limits, scaled by the runner's measured baseline (scripts/latency.sh)
    scripts/mutate.py --shard K/16    # CI, before each release: a 16th of every mutation (and --shard dist)
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
| `tests/broken.rs` | answers for code with errors: an unknown method, a syntax error, the placeholder; errors in declarations, in the file, an import, an importer and an unrelated module, for every kind of answer, with the changes since the last good check, and the passes after each kind of error, without a panic or after one |
| `tests/overlay.rs` | unsaved contents replacing files on disk |
| `tests/panic.rs` | a compiler panic is reported, and the next request works |
| `tests/crosscheck.rs` | reconstructed types agree with what the compiler stores |
| `tests/citations.rs` | the upstream lines the code cites still say what it mirrors |
| `tests/conformance.rs` | LSP 3.18: the checks every test over LSP makes of each message it receives (tests/common/spec.rs: its shape against the spec's meta model, in tests/fixtures/lsp, and what the client's capabilities allow), and what a client with no optional capabilities gets |
| `tests/lsp.rs` | the real binary as an LSP server over stdio: `--version`, initialize, exiting when the client's process is gone, encodings, documents, hover, completion, status, panics, timeouts, aborts, cancellation (a running check cancelled finishes in its worker, and is killed only after a query waits for it, or at the timeout), debouncing, side effects |
| `tests/definition.rs` | definition, type definition and references on every kind of name, same-named symbols in different modules, broken code, both encodings |
| `tests/navigation.rs` | the same over LSP, in UTF-8 and UTF-16, with status reasons, an installed std that isn't `.upstream/` (and answers in its files), and a package in `dep/` |
| `tests/corpus_definition.rs` | every reference in std/src selects its own symbol, and goes to a declaration of the same id and name |
| `tests/diagnostics.rs` | the oracle: diagnostics match the installed `inko check --format json` exactly (file, lines, columns, level, ID, message, order) for every project in tests/fixtures/diagnostics, which between them produce every diagnostic `inko check` can, for std, and for unsaved text; past a module that doesn't parse, `inko check` of the project without it and what imports it |
| `tests/publish.rs` | diagnostics over LSP: every file published without opening it, ranges in both encodings, every fixture project, edits that add and fix errors (and clear them), a module that doesn't parse leaving the others', the debounce, queries first, superseded and hung checks, a compiler panic, what's a target, turning them off |
| `tests/highlight.rs` | document highlights over LSP in both encodings: bindings and assignments as writes, fields, same-named symbols, methods, types, closure arguments, `null`, broken code |
| `tests/signature.rs` | signature help over LSP in both encodings: positional and named arguments (in any order), nested calls, type arguments, unclosed argument lists while typing, a type's constructor and its fields, `null` outside parentheses, for operators and unresolved methods |
| `tests/inlay.rs` | inlay hints over LSP in both encodings: every rule for type and parameter hints, and the requested range |
| `tests/latency.rs` | latency limits (scaled by the machine's load in the check of every commit, as written in the milestone tier: AGENTS.md, "Checks") for every query (each a check of its own), alone and while a diagnostics check of the same project runs, a burst of 1,000 definitions (one check), the cost of a worker process, a question after a cancelled one (the worker keeps its checks), and a second question in code that doesn't parse (a lookup). With `CUTTLEBONE_TYPING_REPLAY` set, a replay of typing at 7 characters a second, which prints what it measured |
| `tests/golden.rs` | `dump-types` output for a few std modules and fixtures, compared with `tests/golden/*.txt`. Run `UPDATE_GOLDEN=1 cargo test` to rewrite them. |
| `tests/editors.rs` | the Neovim smoke check (below) under `cargo test`, if `nvim` is on `PATH` |
| `tests/guardrails.rs` | the layering of src/, no editor named in code, nothing named after Inko, no machine-specific paths, patches/README.md accounting for every patch line, a CHANGELOG.md entry for the version, and docs showing it |
| `tests/repaired.rs` | fresh answers in code that doesn't parse, from a repaired copy, over LSP: the rest of the file, completion, before the edit in the method being edited, a placeholder type the blank lines could fill in, a changed declaration, and references never |
| `tests/stale.rs` | answers from the last good check over LSP (with repaired copies off, a fault only tests have): which requests give them (and which never do), positions and ranges mapped through edits in both encodings, a changed declaration, diagnostics checks and unsaved text when opened, hints in the method being edited and the ranges they cover, a check of documents edited since it was asked for |
| `tests/replay.rs` | the edit-replay corpus, the oracle for code being edited: real files edited as people do, every query asked at many positions after each edit (and inlay hints for each line and the whole file, and document symbols), and each answer compared with the fresh one at the end, hints within the ranges the answer says it covers, where they must be exactly the final ones, and none outside them. Fails on any wrong answer, or less coverage than measured. Every commit replays a sample; `CUTTLEBONE_CORPUS_FULL=1` replays all of it. `-- --nocapture` prints the tables |
| `tests/cache.rs` | checks kept between questions: questions about the same text share one, with the same answers (fresh, repaired and stale); an edit, a file changed, created or deleted on disk (also outside the editor, over LSP) is never answered from one; what may have changed during a check isn't kept; the bound and least recently used order; completion's copies and panicked checks not kept; checks with nothing to answer from (code that doesn't parse) kept apart, and never past a change to any source file; `cached` in the status; the worker replaced when it dies, stopped at `shutdown` |
| `tests/names.rs` | completion of names over LSP in both encodings: variables, arguments, closures' and cases' bindings in scope and not out of it, `self`'s methods, fields after `@`, types in a signature, symbols to import with their edits (not private, not a test module's), modules and symbols in an `import`, keywords, and code being edited, past declarations that can't change the names |
| `tests/hover.rs` | hover over LSP in both encodings: the type, the declaration as written and where it is, and its doc comment, for the project, a dependency and std, an implementation with its trait's doc comment, a module, a declaration's own name, plain text, and a stale hover after its doc comment changed |
| `tests/syntax.rs` | outlines from the syntax alone: names and ranges against the compiler's declarations in every module of std and the fixtures, and doc comments against the installed `inko doc` |
| `tests/symbols.rs` | document and workspace symbols over LSP in both encodings: every kind of declaration, `impl` blocks, every symbol for an empty query and as partial results, modules, code that doesn't parse, an unclosed brace in a method, documents that aren't files, a dependency, files changed on disk (also after the index read them), clients without hierarchy or the newer kinds |
| `tests/corpus_symbols.rs` | every declaration document symbols report, in std and the fixtures, is found by workspace symbols at the same place, and is its own definition; every trait's required and default methods, and whether each declaration is public and static, are the type database's |
| `tests/folding.rs` | folding ranges over LSP in both encodings: blocks, comment lines, imports, the text shown collapsed, code that doesn't parse, `\r\n` and lone `\r`, clients without kinds, collapsed text or with a limit; every declaration's body and doc comment in std folds |
| `tests/formatting.rs` | formatting against the installed `inko fmt` over every module of std and the fixtures, as they are and flattened; over LSP in both encodings, with nothing written, and text that doesn't parse |
| `tests/selection.rs` | selection ranges over LSP in both encodings: tokens, chains, elements, statements, brackets, declarations, code that doesn't parse; every declaration around every name in std is selected, nested, and again with its doc comment |
| `tests/hierarchy.rs` | implementations, declarations, and call and type hierarchies over LSP in both encodings, across the project, a dependency and std; complete or none in broken code, and past an error in a declaration elsewhere where it can't hide a part of them; code lenses, each its own request's answer, their coverage and their command |
| `tests/corpus_hierarchy.rs` | over all of std, against the type database: every implementation it records is found from its trait (or trait's method), and nothing else; every type's and trait's supertypes and subtypes; outgoing calls are their callees' incoming calls |
| `tests/rename_files.rs` | `workspace/willRenameFiles` over LSP in both encodings: imports and uses by name, aliases, directories, moves out of `src/`; the project checks with the edits applied and the file moved; a status for every rename, naming its files |
| `tests/pull.rs` | pull diagnostics over LSP: the same as the core's for every fixture project, unchanged reports, files not open, a long poll, no pushes to a client that pulls |
| `tests/actions.rs` | quick fixes over LSP in both encodings, each checked by the installed `inko check` on a copy of the project with it applied: imports, unused imports, variables and results, `mut` for variables and fields, missing fields, methods and traits, unreachable code; in files with other errors; none that would add an error, for other kinds, or for clients without literals |
| `tests/undo.rs` | answers after edits are undone over LSP, in both encodings: incremental and whole-text changes, several in one notification, closing and opening again, with and without a save between, a module changed on disk and restored; every answer from the documents as they are is a new server's, at every step |
| `tests/corpus.rs` | every expression in std (src/ and test/) has a type, except for the allow-listed kinds |

`scripts/mutate.py` breaks behaviours and rules on purpose, one at a time,
and checks that a test (or clippy, or a check script) fails for each,
running several mutations' tests at once, in copies of the tree. Before
any, it runs every test unmutated in such a copy, and stops if one fails
there (`--control`): a test that fails anyway would count its mutations as
caught. `--since REV` runs only those a
change since REV could affect, and `scripts/check-selection.py` shows that
this selection would have run every mutation guarding code the last three
milestones changed.

`scripts/check.sh` also checks that `patches/upstream.patch` applies to a
fresh copy of the pinned release and matches `.upstream/inko`
(`scripts/check-patch.sh`), and that the release server contains no
test-only code (`scripts/check-release.sh`). Tests build with the
`fault-injection` feature (docs/lsp.md, "Testing faults"); release builds
don't.

The milestone tier also checks the release tooling with this machine's
archive (`scripts/check-dist.sh`, [docs/releases.md](docs/releases.md)):
it builds and verifies it, publishes it to a `file://` URL with a manifest
and an index and checks them from there, and checks that each step
refuses what it should.

`editors/check.sh` starts the server from VS Code and Neovim headlessly and
checks that a hover and a go to definition work, and that a diagnostic
appears for an error and clears when it's fixed (see
[editors/](editors/README.md)).

## License

Cuttlebone is licensed under the Mozilla Public License 2.0
([LICENSE](LICENSE)), the same as Inko, whose compiler it links and
patches. The patch (patches/upstream.patch) modifies MPL-2.0 files, so it
is MPL-2.0 too ([patches/README.md](patches/README.md)). Each release
archive holds Cuttlebone's LICENSE, the compiler's own (as
`LICENSE-MPL-2.0`), and the licenses of everything else the binary
contains ([docs/releases.md](docs/releases.md#archives)).
