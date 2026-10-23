# Milestone 4: diagnostics, and three features on data already there

**Done.** Cuttlebone now publishes the errors and warnings `inko check`
reports, and answers three more requests built on what the walker already
had:

- **Diagnostics** (`textDocument/publishDiagnostics`): the same as
  `inko check --format json`, diagnostic for diagnostic, for every file in
  the project, open or not, from a full check (including MIR) in a worker
  of its own, so queries never wait for one.
- **`textDocument/documentHighlight`:** a symbol's uses in the file, by
  compiler id, marked Read, Write or Text.
- **`textDocument/signatureHelp`:** the resolved method's signature, its
  parameters and the active one, named arguments included, while the
  argument list is still being typed.
- **`textDocument/inlayHint`:** inferred types on `let` bindings and closure
  arguments, and parameter names at call sites, with written rules for
  what's left out as noise.

docs/lsp.md documents every new request with an example, the diagnostics'
scheduling, every difference from `inko check`, and a section for clients
(such as JetBrains') that turn on every advertised capability. Nothing in
`src/` names a client.

**Tests:** 212, up from 175, all passing under plain `cargo test`.
`scripts/mutate.py` has 122 mutations, 53 of them new, and every one is
caught. `scripts/check.sh --full` passed on the commit with this report,
in 1000 s (milestone 3's took 373 s for 69 mutations; the diagnostics and
latency tests that several new mutations run take longer).

**The patch grew by 18 lines** (+11 −7): a four-line accessor for the
compiler's diagnostics, and seven `pub(crate)`s that become `pub`.

## How closely diagnostics match `inko check`

**Exactly, wherever both give an answer.** tests/diagnostics.rs is the
oracle. It runs the installed `inko check --format json` (which must be the
pinned 0.21.1; the test fails otherwise, rather than skip) and compares the
whole list with Cuttlebone's, **in order**: file, first and last line, first
and last column, level, ID and message. It does this for:

- **Twelve fixture projects** in tests/fixtures/diagnostics. The compiler
  stops after the first pass that reports an error, so each project
  targets one pass: syntax and missing modules, duplicate symbols, fields,
  types in signatures, implementing traits twice, traits a trait requires,
  methods a trait lacks or adds, methods that can't be defined, errors in
  method bodies, constants (evaluated while lowering to MIR), moves and
  exhaustiveness (MIR), and a project with only warnings. Between them they
  produce **all 23 IDs `inko check` can report**, and the test fails if one
  goes missing. The 24th, `missing-main`, is only reported by `inko build`:
  `inko check` never defines a main module.
- **std**, src/ and test/ as one project, which has no diagnostics. It
  proves there are no false positives in all of std, and that lowering all
  of it to MIR works.
- **Unsaved text:** Cuttlebone with an edited buffer (adding a move) against
  `inko check` on a copy with that text saved.

Non-ASCII text comes before diagnostics in most fixtures (`日本`, `é`,
`👩‍👩‍👦`), including a syntax error after a ZWJ family on its line, and the
lexer's message for a non-ASCII identifier (`a '�' is not allowed`).

tests/publish.rs then checks the LSP side: the same diagnostics for every
fixture project as published `Diagnostic`s, and ranges in UTF-8 and UTF-16
against positions computed independently from the text (the family emoji
is one compiler column, 8 UTF-16 units and 18 bytes).

### Every difference, and why

1. **A compiler panic.** Lowering some code to MIR panics (a `break` inside
   a closure inside a loop; compiler/src/mir/passes.rs:1836).
   `inko check` aborts and prints nothing. Cuttlebone catches the panic,
   publishes what the compiler reported before it (in the
   tests/fixtures/mir_panic fixture, a `borrow-value-type` warning), and
   reports status `compilerPanic`. Publishing nothing would hide real
   errors; publishing a partial list with a status that says so seemed
   more useful.
2. **No `dep/` sync.** `inko check` syncs `dep/` first if `inko.pkg`
   changed, which can download packages. Cuttlebone checks with whatever
   `dep/` holds (no side effects, as for every query), so in a project
   whose dependencies were never synced it reports the missing modules
   (`invalid-file`) that `inko check` would have downloaded.
3. **The standard library and dependencies aren't targets.** Opening a file
   in std or `dep/` doesn't check std or the package as a project. A
   diagnostic in such a file found while checking the user's project is
   still published.
4. **One list per file.** If two targets (say two lone files that import
   the same broken module) report diagnostics in one file, the file shows
   the latest check's.
5. **What a target is.** `inko check` checks the directory it's run in.
   Cuttlebone checks the project an open file belongs to (the innermost
   workspace folder, or the nearest directory with `src/`), and a file
   outside any `src/` or `test/` alone, like `inko check file.inko`.

Neither tool reads `--target` or `-i` here: both use the host's target and
the default source directories.

## What was built

### The patch

`Compiler::diagnostics()` returns an iterator over the diagnostics the
compiler collected, and `Diagnostic` with its `is_error`, `id`, `message`,
`file` and `location` become public. patches/README.md justifies each line,
and its table matches the patch (tests/guardrails.rs).

- The accessor is the last method of `impl Compiler`, after `prepare`, so
  no line Cuttlebone's code cites moved (tests/citations.rs). My first
  version put it next to `print_diagnostics` and shifted two citations.
- It returns an iterator, so the `Diagnostics` collection stays private,
  and spells out `crate::diagnostics::Diagnostic` to save a `use` line.
- Parsing the JSON presenter's output would have needed no patch lines,
  but means capturing stderr and parsing a format that isn't quite JSON
  (it escapes strings with Rust's `{:?}`).

### Diagnostics (src/check.rs, src/query.rs, src/lsp.rs)

- **`check::diagnose`** runs `check_with` with an `inspect` that continues
  to MIR, exactly as `Compiler::check` does, inside `catch_unwind`, and
  reads the compiler's diagnostics afterwards, even after a panic.
- **`query::diagnostics`** converts each location to a range in the
  request's encoding with the same `Lines` as everything else, reading the
  file's text from the open documents or disk. **`query::diagnostics_target`**
  says what a file's diagnostics come from: its project, the file alone, or
  nothing (std and dependencies).
- **The server** has a second worker slot for diagnostics (`Diagnoser`):
  - a target is checked when the server starts (each workspace folder that
    has `src/` or `test/`), the first time one of its files is opened, and
    `diagnosticsDebounceMs` (300) after an edit, a save with new text, or
    closing a document with unsaved changes
  - one diagnostics check runs at a time, with the same timeout and crash
    handling as queries, and starts only when no query is queued or running
  - an edit to a target being checked kills that check (status
    `superseded`) and schedules another
  - after a check, every file with diagnostics is published, and every file
    published last time that has none now gets `[]`
  - a check that doesn't finish leaves the last published diagnostics
  - with `statusNotifications`, a `cuttlebone/status` follows each check,
    with the target instead of a request id
  - `"diagnostics": false` turns all of it off;
    `experimental.cuttlebone.diagnostics` reports which
- **Tags:** `unused-symbol` and `unreachable` are tagged `Unnecessary`.
- **`source`** is `cuttlebone`, never named after Inko.

### Document highlight

The walker now records which references assign (`writes`): variables and
fields in `=`, `:=` and compound assignments, and setters. Highlights are
the walked file's references and declarations with the symbol's id:

- **Write:** a variable's binding (`let`, patterns, arguments of methods
  and closures) and assignments
- **Read:** every other reference, including named arguments and fields
  set in `Point(x: 1)`, which are initializations rather than assignments
- **Text:** the declaration of anything else (methods, types, fields,
  constants)

### Signature help

- The walker records each call written with parentheses
  (`ArgumentList`): the call's range, the name, the type arguments' end,
  the resolved method with the call's type arguments, and each argument's
  location, name (if named) and variable name (if it's just a variable).
- **Which call:** the innermost whose `(` is before the position and whose
  `)` is at or after it.
- **The label** is completion's `detail` formatter with the call's type
  arguments, so `map` on `Iter[ref Int]` shows `fn (ref Int) -> Int`.
  Parameter offsets are found in it as `(a: A, b: B)`, the formatter's own
  output.
- **The active parameter** counts commas after arguments before the
  position (read from the source between them); a named argument is its
  parameter in any order; an empty slot is the next positional parameter,
  or the first not yet named.
- **While typing**, `counter.add(` doesn't parse. If the file doesn't parse
  but does with a `)` inserted at the cursor, that copy is checked instead.
  The `ast` crate's parser decides, in a millisecond, which needed `ast` as
  a direct dependency. Calls with too few arguments still resolve, so this
  works from the first `(`.

### Inlay hints

`Untyped` records each `let` of a plain name without a type annotation and
each closure argument without one; `ArgumentList` already had the rest.
The rules, each with a test in tests/inlay.rs and written down in
docs/lsp.md:

- type hints after `let x` and `fn (x)` without annotations, unless the
  value says the type (an integer, float, string, `true`, `false` or `nil`
  literal; a cast; `Point(x: 1)`; a static method of a type that returns
  that type, such as `Counter.new` or `Option.Some(1)`), or the type isn't
  fully known
- parameter hints before positional arguments of calls written with
  parentheses, unless the argument is a variable or field named like the
  parameter, and never for operators or enum constructors (whose
  parameters are named `arg0` and so on)

Writing these found that **binary operators are calls with `parens` set**
in the HIR: `a + b` got an `other:` hint, and would have had signature help
on `b`. `ArgumentList` now leaves out calls whose name isn't an identifier.
It also found that `let _ = x` is a wildcard pattern, not a binding, so the
`_` rule I had first was dead code; a mutation showed it, and it's gone.

## Tests

| File | What |
|---|---|
| tests/diagnostics.rs (3) | The oracle, above. |
| tests/publish.rs (12) | Diagnostics over LSP: every file published without opening it (both of the `moves` fixture's files, the exact LSP diagnostic, tags); ranges in UTF-8 and UTF-16; every fixture project matches the core; an edit adds an error, its fix clears it with `[]`, closing goes back to disk; a clean project publishes nothing and clears its last diagnostic; the debounce; queries first (a hung query delays a check that became ready); a query isn't stuck behind a hung check; superseded and timed-out checks are killed; a panic publishes what came before; targets (project, lone file, not std or `dep/`); turning them off. |
| tests/highlight.rs (6) | Bindings and assignments as writes, from a use and from the binding; fields declared, read and written (`@x =`, `@x +=`, `a.x =`); same-named field, argument and variable kept apart; methods, types and closure arguments; `null`; a syntax error. Both encodings. |
| tests/signature.rs (6) | Positional arguments, between them and before the `)`; named arguments in and out of order; nested calls; type arguments; unclosed argument lists while typing; `null` on the name, before the `(`, after the `)`, without parentheses, for operators and for an undefined method. Both encodings. |
| tests/inlay.rs (7) | Every hint in the fixture in both encodings, and one test per rule: each obvious value, a static method of another type, closure arguments, parameter names (named, same-named variable, operators, constructors, no parentheses), unknown types, the range. |
| tests/latency.rs (+2) | Highlight, signature help and inlay hints on a std-sized project; and hover, definition, completion, highlight, signature help and inlay hints each while a full check of the same project runs, which must still be running at the answer. |
| tests/editors.rs | The Neovim check's diagnostics step, now with a timeout. |
| tests/manifest.rs (+1) | Diagnostics refuse a project that requires a newer Inko, as queries do. |
| tests/lsp.rs | The exact capabilities; the side-effects test also waits for a diagnostics check. Tests that count worker processes turn diagnostics off. |
| tests/citations.rs | Three new citations. Paths under `inko/` now count as upstream, so the new citation of inko/src/command/check.rs is checked too. |

The fixtures: tests/fixtures/diagnostics (twelve projects),
tests/fixtures/mir_panic, and tests/fixtures/features (highlights,
signature help and inlay hints), each with non-ASCII text before names.

### The editor checks

Both smoke checks (`editors/check.sh`) now insert `let g = '日本' + nope`
after line 12 without saving, wait for an `invalid-symbol` diagnostic whose
range covers exactly `nope` (in the wrong encoding it wouldn't), delete the
line, and wait for the diagnostic to go. Both pass: Neovim 0.12.6 headless
(UTF-8) and VS Code 1.127 (UTF-16). I checked that each fails with a wrong
expected text. VS Code's failure message includes the published
diagnostic: `"range":[{"line":12,"character":19},{"line":12,"character":23}]`.

Two robustness fixes came out of it. A Lua error in check.lua (here, the
new variables missing under `cargo test`) left headless Neovim waiting
forever, and `cargo test` with it: check.lua now runs in `pcall` and checks
its variables first, and tests/editors.rs kills Neovim after 120 s.

## Measurements

Apple M1 Max, 64 GB, release build, median of 7, from the client's side.
The std-sized project imports every std module that builds on macOS.

| | |
|---|---|
| Hover, std-sized project | 102 ms |
| Document highlight, same | 100 ms |
| Signature help, same | 104 ms |
| Inlay hints, whole file, same | 103 ms |
| Hover while a diagnostics check of the same project runs | 104 ms |
| Diagnostics check, std-sized project (in the worker) | 199 ms |
| `inko check`, same project | 206 ms |
| Diagnostics check, std's src/ and test/ | 424 ms |
| `inko check`, same | 427 ms |
| Diagnostics check, the `moves` fixture | 55 ms |
| From an edit to published diagnostics, std-sized (300 ms debounce) | 509 ms |

A full check costs about twice a query's type check, as the spike
predicted (MIR is the other half). Running it beside a query costs the
query almost nothing on this machine: the two use different cores. In
`cargo test`'s debug build of Cuttlebone (dependencies optimized), a check
of the std-sized project takes 210 to 230 ms, and every query stays under
200 ms beside it.

## The mutations

All 122 are caught: milestone 3's 69, and 53 new ones.

- **Diagnostics (20):** skipping the required-version check; skipping MIR;
  dropping warnings; ignoring unsaved
  text; ranges always in UTF-16; a panic discarding what came before; std,
  dependencies and lone files as the wrong targets; no check of workspace
  folders at start; files not cleared; closing not rechecking; no
  `Unnecessary` tags; warnings as errors; no supersession; no debounce;
  not waiting for queries; no timeout; not turning off; checking in the
  server's thread (caught by the latency test, because the check no longer
  overlaps the query).
- **Document highlight (7):** assignments to variables, fields and setters
  not writes; bindings not writes; matching by name instead of id; kinds
  swapped; no capability.
- **Signature help (9):** no `)` inserted; named arguments taken as
  positional; commas not counted; the outermost call winning; type
  arguments ignored; parameter offsets from the label's start; the `(` or
  what's after the `)` counting as inside; no capability.
- **Inlay hints (17):** each kind of obvious value not obvious; every or no
  static method obvious; annotated `let`s and closure arguments hinted;
  unknown types hinted; named arguments hinted; same-named variables
  hinted; operators as argument lists (for hints, and for signature help);
  constructors' generated names; hints past the range; the range's end not
  read; no padding; no capability.

Six existing mutations needed new snippets because the code they break
moved or now appears twice: the query timeout, `didClose`, `didSave`, the
documents sent with a query, the required-version check (which `diagnose`
also calls, so it has a mutation and a test of its own in
tests/manifest.rs), and the layering guardrail's import. The first full
run reported the last three as skipped or not building; the second
caught all 122.

Two mutations I wrote weren't caught, and both were behaviour-neutral, so I
removed them rather than the tests: inserting a `)` into code that already
parses (it never parses afterwards, so the early return only saves a
parse; it now says so), and the `_` rule above.

**A leak in the mutation runner.** Under the "no worker deadline" mutation,
`a_hung_worker_exits_by_itself_if_the_server_dies` fails as it should, but
left its hung worker running forever. Earlier runs had left several such
`cuttlebone --worker` processes (some 16 hours old) sleeping in the `hang`
fault; they're orphans of the test build, not of anything users run. The
test now kills its worker before failing. I didn't kill the old ones, as
they predate this session: `pkill -f 'target/debug/cuttlebone --worker'`
removes them.

## What clients should know

1. **Diagnostics come unasked.** There's no capability for them: turn them
   off with `"diagnostics": false` in `initializationOptions`.
   `experimental.cuttlebone.diagnostics` says whether they're on.
2. **They cover the whole project**, open files or not, and every file
   gets `[]` when its last diagnostic goes. Keep the diagnostics of files
   that aren't open.
3. **`code` is the compiler's ID** (`moved`, `invalid-symbol`, ...), and
   `source` is `cuttlebone`. `version` isn't set.
4. **Latency:** about 300 ms of debounce plus the check, so 0.5 s on a
   std-sized project, and longer for bigger ones.
5. **A failed check keeps the last diagnostics.** With status
   notifications, `cuttlebone/status` says why (`timeout`, `checkCrashed`,
   `compilerError`, `superseded`); `compilerPanic` publishes a partial list.
6. **Document highlight** is within one file, by id. Named arguments and
   `Point(x: 1)` fields are Read.
7. **Signature help** has one signature (no overloading in Inko), with
   `[start, end)` parameter offsets in the negotiated encoding, which for
   Inko's ASCII identifiers are the same in all. It works before the `)` is
   typed. It's `null` for constructors (`Point(x: 1)`) and closure calls.
8. **Inlay hints** are plain strings with `paddingRight` on parameter
   hints; no `resolve`, tooltips or edits.
9. **Clients that turn on every capability** (JetBrains' LSP support) must
   turn off on their side whatever another plugin already provides;
   docs/lsp.md has a section on it.

## Decisions you may want to revisit

- **Partial diagnostics after a panic**, rather than none (as `inko check`)
  or the previous ones.
- **Keeping the last diagnostics after a failed check**, rather than
  clearing them. A timeout on a huge project leaves stale errors on
  screen; clearing would make them flicker.
- **Queries first.** A diagnostics check doesn't start while any query is
  queued or running. Constant queries (hovering on every mouse move) could
  starve diagnostics.
- **Running a check beside queries**, rather than pausing it. On a machine
  with few cores the two would compete; a lower priority for the
  diagnostics worker would need a dependency (setting it is `unsafe` libc
  otherwise).
- **300 ms debounce.** Long enough not to check mid-word, short enough to
  feel prompt.
- **Named arguments and constructor fields as Read** in highlights; they
  give values, but aren't assignments.
- **Parameter hints for closures** (`func:` before `fn (n) { ... }`). Some
  servers leave those out.
- **Array literals get type hints**: `[1, 2]` doesn't say `Array[Int]`.

## Remaining unknowns

1. **Files changed outside the editor** (`git checkout`, a formatter) don't
   trigger a check until a document of the project changes: Cuttlebone
   doesn't watch files (`workspace/didChangeWatchedFiles`).
2. **Projects much larger than std:** a full check grows with the project,
   with nothing cached between checks.
3. **Diagnostics in two targets** (difference 4) and **std or dependency
   files** with errors of their own aren't tested beyond the targets
   themselves.
4. **`inko check` exit paths** I couldn't reach: an internal error mid-way
   (other than a newer Inko required, which gives status `compilerError`
   and no diagnostics, where `inko check` prints the error).
5. **Signature help on calls with type arguments** (`foo[Int](`) relies on
   the `(` coming right after the `]`; it's untested, because std has
   almost no such calls.
6. **From earlier milestones, still open:** lone `\r` line endings,
   `stale` answers, Windows, and packaging.
