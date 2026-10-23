# Milestone 1: a type service for Parakeet

**Done.** `cuttlebone` is a long-lived process that speaks a small JSON
protocol on stdio ([docs/protocol.md](../protocol.md)). After a handshake
that checks the project's Inko version, it answers two questions:

- `typeAt(file, position)`: the type of the expression at a position.
- `membersAt(file, position)`: for `receiver.<placeholder>`, the methods,
  fields, constants and types available on the receiver, with signatures.

Every reply carries `status`: `fresh` or `unavailable` (with a reason code),
with `stale` reserved. Requests can carry unsaved buffers. Positions are a
line plus a UTF-16 column. A compiler panic is reported, and the process
keeps going.

87 tests, all passing under plain `cargo test` (about 10 s). Every behaviour
below has a test that fails when it breaks. I checked that by breaking each
one on purpose (19 mutations, from the synthetic-node rules to the patch's
overlay line), and each was caught.

## What was built, by task

### 1. Broken-code fixtures

`tests/fixtures/broken` is a valid 72-expression program. `tests/broken.rs`
breaks it in memory and sends the broken text as unsaved contents, so it
tests the overlay at the same time.

| Case | Expressions with a type | Query result |
|---|---|---|
| no errors | 72/72 | `fresh` |
| unknown method in a body | 70/72: the call, and the `to_string` that string interpolation wraps around it | `untyped` at the call, `fresh` everywhere else, including the call's receiver |
| undefined type in a signature | none that the compiler stores | `declarationErrors` everywhere |
| syntax error (in the file, or in a module it imports) | none (no HIR) | `noHir` |
| completion placeholder (`values.IntellijIdeaRulezzz`) | 70/72, as for an unknown method | `membersAt` lists `ref Array[Int]`'s members |

One spike number was misleading. The 3/56 that "had a type" after an error in
a signature were all types Cuttlebone reconstructs itself (`break` is
`Never`, `Stdout` as a receiver is `Stdout`), not types the compiler had
computed. Queries now detect that the type checker never reached method
bodies, and answer `declarationErrors` instead of a partial picture
(`expressions_pass_ran` in src/walker.rs).

### 2. Unsaved buffers

There's an in-memory overlay in the patch: a `Config.overlay` map, read by
`ModulesParser::parse`, the one place the compiler reads source files. That's
6 added lines (see below). It applies to the file asked about and to
everything it imports, so Parakeet can send every unsaved buffer.

I measured the alternative too. Creating and deleting a symlink mirror of a
std-sized project (222 files) takes about 33 ms per request, roughly 30% of
a typical check, and every path in every answer would need mapping back. The
overlay costs a hash lookup per module.

### 3. Positions

**HIR columns count extended grapheme clusters**, not bytes or characters.
The lexer advances the column by `value.graphemes(true).count()` per token
(ast/src/lexer.rs:518-520). For example, 👩‍👩‍👦 is 1 column, 8 UTF-16 units, 5
code points and 18 bytes. Lines and columns are 1-based, and end columns are
inclusive.

The protocol uses LSP's convention: a 0-based line, a 0-based UTF-16
`character`, and exclusive ends. `src/position.rs` converts using the same
`unicode-segmentation` version as the lexer, which a test pins through
Cargo.lock. Tests cover é, 😀 (a surrogate pair), e + U+0301, a ZWJ
family, CJK, tabs, CRLF line endings, and positions inside a cluster, all
against the real compiler on `tests/fixtures/unicode`. One theoretical
difference remains: the lexer segments per token and Cuttlebone per line, so
they'd disagree on a grapheme cluster that spans a token boundary (a string
literal that starts with a combining character).

### 4. Synthetic nodes

The walker marks the nodes that desugaring introduces as `synthetic`. The
rules are structural:

- names starting with `$`
- calls on those names
- calls whose range equals their receiver's
- the `match` and `break` that `while` and `for` generate
- the `nil` an `if` without an `else` gets
- the `false` in `!x`
- `recover` blocks the type checker infers

The rule for which expression a position means:

1. among the non-synthetic expressions containing the position,
2. take the innermost one,
3. and of several with the same range, the outermost.

docs/protocol.md has a table of what that gives for array literals,
interpolation, `for`, `while`, `if`, `!`, `async`/`await`, an inferred
`recover`, and `+=`, and `tests/type_at.rs` has a test for each. The golden
files now record the synthetic flag, so a change in which nodes are marked
shows up in their diff.

I also added variable bindings as answerable nodes: the name in
`let x = ...`, in a `match` or `for` pattern, and closure arguments. Without
them, hovering a `let` name would have answered `Nil`, the type of the whole
`let`. Closure arguments' types live on the `ClosureId`, not in the HIR. All
of std's bindings have types (the corpus test covers them).

### 5. The protocol

docs/protocol.md covers transport, status, reason codes, positions, the
handshake, `typeAt`, `membersAt`, `shutdown`, freshness and robustness.

- **Handshake:** `initialize` runs `inko --version` (from the `inko` path the
  client sends, or `PATH`) and compares it with UPSTREAM_VERSION. On a match
  it runs `inko print std` to find the std to check against. On a mismatch
  it replies `inkoVersionMismatch` with `installed`, `supported` and
  `cuttlebone`. A failed handshake leaves the server uninitialized, even
  after an earlier success.
- **membersAt** keeps a candidate name only if the compiler's own
  `TypeEnum::lookup_method` accepts it, so visibility and static/instance
  rules come from upstream. On top of that, it applies the compiler's
  `check_mutability` rule (no `fn mut` or `fn move` methods on a `ref`
  receiver) and its field visibility rule. Signatures are formatted by the
  compiler's formatter, with type arguments filled in:
  - the receiver's own (`get(index: Int) -> Result[ref Int, OutOfBounds]` on
    `Array[Int]`)
  - those of bounded `impl ... if T: Equal` blocks
  - those of default methods from generic traits (`map` on `IntoIter[Int]`)

### 6. Robustness

Each check runs on its own thread with a 64 MiB stack, under
`catch_unwind`, and a panic hook records the message and location. The
reply is `compilerPanic` with that message, and the next request works.
Every check builds its own compiler state, so a panic can't leave anything
half-updated.

I found two real panics:

- **In type checking:** a project file named `src/std/map.inko` shadows
  std's module, and the type checker panics with "The type std.map.Map
  isn't defined" (types/src/lib.rs:6330). This is the test input
  (`tests/fixtures/panic`). It works in-process (`tests/panic.rs`) and over
  stdio (`tests/e2e.rs`).
- **In MIR lowering:** a `break` inside a closure inside a loop. The type
  checker accepts it, and MIR lowering hits an `unwrap` on `None`
  (compiler/src/mir/passes.rs:1836); `inko check` panics the same way.
  Queries no longer lower to MIR (see the patch), so this can't reach
  Cuttlebone.

I chose `catch_unwind` over a child process. It handles panics, which is
what the compiler does on bad input, without per-request IPC or respawning.
It can't handle an abort, a stack overflow beyond 64 MiB, or a hang. Parakeet
already owns the process, so the protocol asks clients to time out each
request and restart the process if it exits or stalls (see below). I didn't
find any input that hangs or aborts the type checker.

### 7. Reconstructed types

The walker records a cross-check wherever the compiler stores something the
reconstructed type must agree with:

- **`receiver`:** a type or module used as a receiver, compared with the
  method's receiver, the method's module, the constant's or type's module,
  or the setter's call.
- **`let`:** a derived value bound by `let x = ...` without an annotation,
  compared with the variable's type.
- **`scope`:** a derived last expression of a scope, compared with the
  scope's type, as an owned value.

`tests/crosscheck.rs` asserts on these:

- In std/src, every one of the 3,844 Symbol-origin types is cross-checked,
  and all agree. The test fails if any Symbol-origin type lacks a
  cross-check.
- Of the 1,110 Derived types, 161 can be cross-checked (85 loops, 76
  `try`s), and all agree. The other 949 are mostly `break` (342) and `next`
  (26), which are always `Never`, and loops and `try`s whose value isn't
  bound or returned from a scope.
- `tests/fixtures/crosscheck` covers every kind (loop as `Nil` and `Never`,
  `break`, `next`, and `try` in methods returning `Result` or `Option` and
  in a closure whose return type is inferred) and every receiver shape
  (type, module, `module.Type`, `module.CONSTANT`, `module.Type(...)`).

The rules that can't be cross-checked cite the type checker lines they
mirror. `tests/citations.rs` lists each citation, with text that must still
appear in the cited lines. It fails if a citation moves or the cited code
changes, and it also fails if the code gains a citation the test doesn't
list. That turns "diff the cited lines on a rebase" into a failing test.

The cross-checks caught one drift right away. **The spike's loop rule was
wrong.** It assumed closures stop a `break` from reaching the enclosing loop,
but the type checker walks through closure scopes
(compiler/src/type_check/expressions.rs:2940-2968), so
`loop { fn { break } }` is `Nil`, not `Never`. That's fixed, cross-checked
by `let a = loop { ... }`, and tested. I also made `try` mirror the type
checker's accepted combinations: `try` on an Option in a method returning a
Result is now `untyped`, as the compiler has it, rather than the inner type.

### 8. End-to-end tests

`tests/e2e.rs` launches the real binary and drives it over stdio (13 tests):

- the handshake: success, version mismatch, missing `inko`, protocol
  mismatch, and re-initializing
- `typeAt` and `membersAt` with unsaved contents
- unavailable answers
- a compiler panic and recovery
- malformed JSON and unknown methods
- `shutdown`, and exiting when stdin ends
- two latency limits, and the real installed `inko` (skipped if it isn't
  0.21.1)

A fake `inko` script stands in for the toolchain, so the tests don't depend
on what's installed.

The latency tests use a 500 ms limit on the median of 5 requests, measured
from the client:

| Test | Modules checked | Median |
|---|---|---|
| a project whose main module imports all of std/src | 116 | 106–110 ms |
| a position in std.net.http.server | 90 | 84–89 ms |

Dependencies are optimized in debug builds (`[profile.dev.package."*"]`), so
these numbers are the same under `cargo test` and `cargo test --release`.

## Measurements

Apple M1 Max, release build.

| | |
|---|---|
| `typeAt`/`membersAt` on the test fixtures (53–57 modules each) | 30–40 ms |
| `typeAt` on a file importing all of std/src (116 modules) | ~110 ms |
| `typeAt` in std.net.http.server (90 modules) | ~85 ms |
| Process start plus `initialize` with the real `inko` (two subprocesses) | ~31 ms |
| Mirroring a 222-file project to a temp directory (the alternative to the overlay) | ~33 ms per request |
| Expressions and bindings in std/src / with a type | 68,976 / 68,956 (the 20 without one are the allow-listed `mut fn_name` operands, as in the spike) |
| The same for std/src and std/test | 202,138 / 202,118 |
| Binary size | 15.8 MB |

## Every patch line added, and why

patches/README.md has the full diff of every line with its justification.
The non-mechanical part grew from 13 to 27 added lines.

| File | Added | Why |
|---|---|---|
| compiler/src/lib.rs | 1 changed | `pub mod hir` (unchanged since the spike) |
| compiler/src/hir.rs | 469 changed | `pub(crate)` → `pub`, mechanical (unchanged since the spike) |
| compiler/src/compiler.rs | +17, 1 changed | `check_with(file, inspect)`. New in this milestone: `inspect` returns a `bool`, and `false` skips lowering to MIR (5 more lines than the spike's hook). Types don't depend on MIR, MIR is more than half of a check's time, and MIR lowering is where the closure-`break` panic is. |
| compiler/src/config.rs | +4 | `pub overlay: IndexMap<PathBuf, Vec<u8>>` and its default, for unsaved buffers |
| compiler/src/modules_parser.rs | +5, 1 changed | read the overlay before falling back to disk |

The patch still applies cleanly to a fresh v0.21.1 clone, and nothing in the
type checker, the types crate or code generation changed.

## Decisions I made that you may want to revisit

- **Nothing is cached between requests.** Each query re-checks the file and
  its imports. At 30–110 ms that's well within the limit. Caching the last
  check by content hash would make `membersAt` right after `typeAt` free, but
  it needs file-modification tracking for every imported module to stay
  `fresh`.
- **`fresh` with a `null` result** means "computed, and nothing is there".
  `unavailable` means "couldn't compute": no HIR, declaration errors, an
  untyped expression, or a panic. A type containing `<error>` counts as
  `untyped`; a partially inferred type (`?`) is passed through as `fresh`.
- **Checks run with the project root as the working directory**, behind a
  lock, because the compiler reads `inko.pkg` from the working directory
  (compiler/src/pkg/manifest.rs:12, compiler/src/pkg/sync.rs:201-211). A
  query therefore behaves like `inko check` in the project: it enforces
  `require inko`, and **it syncs `dep/` if `inko.pkg` changed since the last
  sync, which can download packages**. That may be surprising for a type
  query. Avoiding it would need a few more patch lines to skip `prepare()`.
- **A position means the character at it** (LSP semantics), with no fallback
  to the character before. For a caret right after `foo`, Parakeet should
  send the position of the last character.
- **`membersAt` matches the call by its name's range**, so it works with any
  placeholder name, a typed prefix (`name.siIntellijIdeaRulezzz`), or a real
  method name.

## Remaining unknowns

1. **Hangs and aborts.** These are covered only by the client's timeout and
   restart. No hanging input is known, but the type checker hasn't been
   fuzzed.
2. **Code with declaration errors or syntax errors still gets nothing.** This
   is what `stale` is reserved for: a later milestone could keep the last
   good check per file and map positions across edits.
3. **`try` with mismatched Result error types.** The type checker types it as
   an error, but Cuttlebone reports the inner value. It only happens in code
   with an invalid-throw error, and it's documented at `try_type` in
   src/walker.rs.
4. **Members, not yet modelled:**
   - a field and a method with the same name (the compiler picks the field
     when the call has no arguments, and Cuttlebone lists only the method)
   - imported symbols re-exported through a module
   - `Self` in some trait signatures
5. **Performance on projects much larger than std** is unmeasured. Per-file
   checks scale with the import closure, not the project.
6. **Packaging** is unchanged since the spike: 15.8 MB, LLVM linked, Homebrew
   zstd and libunwind dynamic on macOS, and one build per Inko release.
7. **Upstream bugs found** (not reported, per the no-LLM policy): the MIR
   panic on `break` in a closure, and the type checker panic when a project
   module shadows a std module.

## What Parakeet will need

1. **Finding the binary and starting it.** Packaging is out of scope, so for
   now a setting for the `cuttlebone` path. Start one process per Inko
   project root (`Inko.projectRoot`), with stdin and stdout piped and stderr
   sent to the IDE log. Send `initialize` with `protocol: 1`, `inko`
   (`InkoProjectSettings.toolchain.executable()`) and `project`. Re-send
   `initialize` when the toolchain setting changes. On
   `inkoVersionMismatch`, show the versions it reports (for example in the
   existing missing-toolchain banner's style) and use heuristics.
2. **A client service** that serializes requests (Cuttlebone handles one at
   a time, in order), matches replies by `id`, applies a timeout per request
   (say 2 s), and restarts the process on a timeout or exit, with backoff.
   Anything other than `fresh` falls back to the current heuristics, and
   unknown fields must be ignored.
3. **Positions.** IntelliJ document offsets are already UTF-16:
   - `line = document.getLineNumber(offset)`
   - `character = offset - document.getLineStartOffset(line)`
   - ranges back: `getLineStartOffset(line) + character`, with an exclusive
     end
4. **Unsaved contents.** For each request, send
   `FileDocumentManager.getUnsavedDocuments()` for `.inko` files in the
   project as `contents` (path → text). For completion, send the completion
   copy's text for the current file.
5. **Completion** in `InkoCompletions.member()`:
   - Call `membersAt` with the original file's path, the completion file
     copy's text (`parameters.position.containingFile.text`, which contains
     IntelliJ's dummy identifier) as `contents`, and the position of
     `parameters.offset` in the copy.
   - Map members to lookup elements: `name` to the lookup string, `kind` to
     the icon, `signature` after the name to the tail text, `module` to the
     type text.
   - Keep the heuristics as the fallback on `unavailable` or a timeout, and
     call `ProgressManager.checkCanceled()` while waiting.
6. **Type display** from `typeAt`:
   - IntelliJ's `ExpressionTypeProvider` (*View → Type Info*, ⌃⇧P), which
     maps directly to `typeAt`'s type and range
   - types for locals in quick documentation
   - `InkoLocals`' receiver types for the heuristics
7. **A Parakeet bug this milestone surfaced.** `Inko.offsetOf` (used for
   `inko check` diagnostics) treats the compiler's columns as UTF-16
   offsets, but they count grapheme clusters, so diagnostics after an emoji,
   a combining character or a ZWJ sequence on the same line are shifted.
   It's fine for é and CJK, which are one unit and one cluster each. The fix
   is to walk the line by grapheme clusters (`BreakIterator` or ICU4J) when
   converting. I didn't change Parakeet.
