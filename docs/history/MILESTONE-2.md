# Milestone 2: a general-purpose language server

**Done.** `cuttlebone` is now a standard LSP server
([docs/lsp.md](../lsp.md)) for any editor with an LSP client. Milestone
1's custom protocol and its tests are gone. Nothing in Cuttlebone assumes a
particular editor: Parakeet is one client among many.

What it does:

- **Hover:** the type at a position, as a Markdown code block.
- **Completion after `.`:** methods, fields, constants and types, with their
  signatures. The server inserts the placeholder itself, so clients send
  their buffer as it is.
- **Documents:** open, change (incremental), save and close keep unsaved
  buffers, which every check uses.
- **Positions:** UTF-16, or UTF-8 when the client prefers it.
- **Robustness:**
  - every check runs in its own worker process, with a timeout
  - hung, aborting or panicking checks don't take the server down
  - checks wait until typing pauses
  - edits and `$/cancelRequest` cancel checks they make pointless
- **No side effects:** a query never syncs `dep/`, touches the network, or
  writes anything.

**Tests:** 126, all passing under plain `cargo test` in about 10 s. Every
behaviour has a test that fails when it breaks. `scripts/mutate.py` breaks
32 of them on purpose, one at a time, and each is caught by the test meant
to catch it (the list is below). The editor checks run in real Neovim
(headless, also under `cargo test`) and real VS Code.

## What was built, by task

### 1. The LSP front end

- **What it's built on.** `src/lsp.rs` uses lsp-types 0.95 and lsp-server
  0.10, and runs one synchronous loop with no async runtime. That loop owns
  the open documents, a queue of hover and completion requests, the running
  check, and a timer for timeouts and debouncing. The other Rust SDKs, and
  why not them:
  - **tower-lsp:** async, with its own loop. It would get in the way of
    killing checks and debouncing.
  - **RustLSP:** dormant.
  - **lspf:** too new to rely on.
- **lsp-types 0.95, not 0.97.** 0.95's `url::Url` converts to and from file
  paths reliably. 0.97's URI type doesn't convert at all.
- **The core stays independent of the front end.** `lib.rs`, `query.rs`,
  `walker.rs` and `position.rs` know nothing about LSP. The core gained:
  - an `Encoding` (UTF-16 or UTF-8) on each request
  - several workspace roots
  - `query::complete`
  - serde derives, so a worker can send answers back

  `src/server.rs` and `docs/protocol.md` are deleted.

**initialize:**

- **It advertises only what works:** `positionEncoding`, incremental
  `textDocumentSync` with `includeText` on save, `hoverProvider`, and
  `completionProvider` with `.` as its trigger. A test fails if the set of
  capabilities changes. `experimental.cuttlebone` reports the Inko version
  and the standard library in use; it isn't a capability.
- **Position encoding:** the first of `utf-8` and `utf-16` in the client's
  order, else UTF-16. Neovim offers UTF-8 first and gets it; VS Code gets
  UTF-16.
- **The Inko version.** `inko --version` (from `initializationOptions.inko`,
  else `PATH`, with a 10 s timeout) must match the pinned release.
  Otherwise `initialize` gets an error whose message reads, for example,
  "Cuttlebone 0.1.0 works with Inko 0.21.1 only, but /usr/local/bin/inko is
  Inko 0.20.0. Install Inko 0.21.1, or use a build of Cuttlebone for Inko
  0.20.0." `data` has `code` (`inkoVersionMismatch` or `inkoNotFound`) and
  the versions. A client can send `initialize` again, for example after the
  user picks another toolchain.
- **Workspace roots.** The workspace folders are the project roots: a
  folder's `inko.pkg` and `dep/` apply to files in its `src/` and `test/`.

**Documents.** `didOpen`, `didChange` (full and incremental), `didSave`
(with text) and `didClose` keep each document's text. Every check sends the
text of all open documents that exist on disk as the overlay, so unsaved
edits to an imported module show up in its importers.

**Hover.** The type, as `` ```inko `` + the type + `` ``` ``, or plain text
for a client without Markdown support, plus the expression's range. `null`
when nothing is there or there's no answer.

**Completion.** `query::complete` inserts `cuttlebone_completion_placeholder`
at the cursor, in a copy of the document:

- `value.|` is checked as `value.cuttlebone_completion_placeholder`
- `value.si|` is checked as `value.sicuttlebone_completion_placeholder`

It then asks the members query from milestone 1. Each item has:

- `label`: the name
- `kind`: Method, Field, Constant or Class
- `detail`: the signature, with type arguments filled in
- `labelDetails.description`: the defining module

The list is complete and unfiltered (`isIncomplete: false`), so the client
filters as the user types. `null` when the cursor isn't after a call's `.`
or there's no answer. Never a guess.

**Status.** LSP results can't say why there's no answer, so a client can
opt in with `initializationOptions.statusNotifications: true`. It then gets
a `cuttlebone/status` notification just before each hover and completion
response, with:

- the request's `id` and `method`
- `status`: `fresh` or `unavailable` (`stale` is reserved)
- `reason`: `code` and `message`
- `elapsedMs`

I chose a notification over `data` because hover results have no `data`
field, and an empty completion list has no items to carry one. Opt-in
keeps normal clients on plain LSP. The reason codes are milestone 1's, plus
`notAFile`, `timeout`, `checkCrashed` and `checkFailed`. docs/lsp.md lists
them all.

### 2. No side effects on queries

Milestone 1's checks called the compiler's `prepare()`. That:

- checks the Inko version the manifests require
- calls `sync_if_needed`, which *always* creates
  `<data directory>/inko/packages` and takes a file lock in it, and syncs
  `dep/` (downloading packages) if `inko.pkg` changed since the last sync
- reads `inko.pkg` relative to the working directory, which is why
  milestone 1 changed directories behind a lock

Now the hook starts after `prepare()`, which stays in `check` (details in
the patch section below). Cuttlebone repeats only the version check
(`check_required_version` in src/lib.rs), with absolute paths, for the
project's and its dependencies' manifests. The working-directory switch and
the lock are gone, so checks don't depend on the directory the server
starts in.

Tests:

- **`queries_never_sync_dependencies_or_touch_the_data_directory`** starts
  the real server with a fresh `HOME` and `XDG_DATA_HOME`, in a project
  whose `inko.pkg` requires a package that doesn't exist and whose `dep/`
  was never synced. It runs a hover and a completion, and asserts that
  nothing was created: no `dep/`, no `build/`, and nothing in the home
  directory. Putting `prepare()` back in the hook makes it fail.
- **tests/manifest.rs** covers the version check: a newer Inko required by
  the project or by a dependency, an older one, and an invalid manifest.

### 3. Robustness

**The worker.** Each check runs in a worker process, `cuttlebone --worker`,
the same binary. The server writes the job as JSON to the worker's stdin.
The worker writes the answer to stdout and exits. A reader thread per
worker sends the output back to the loop.

| Failure | What the client sees | Test |
|---|---|---|
| A check runs past `checkTimeoutMs` (default 5000) | The worker is killed. The answer is `null`, with reason `timeout`. | `a_hung_check_times_out_is_killed_and_the_server_keeps_going` (also asserts the worker is gone, and the next hover works) |
| The worker aborts or crashes | `null`, with reason `checkCrashed` and the signal | `an_aborting_check_is_unavailable_and_the_server_keeps_going` |
| The compiler panics | Caught in the worker as before: `null`, with reason `compilerPanic` and the message | `a_compiler_panic_is_unavailable_and_the_server_keeps_going` |
| The server is SIGKILLed during a hung check | Each worker exits by itself at the timeout plus 1 s | `a_hung_worker_exits_by_itself_if_the_server_dies` |
| `inko --version` or `inko print std` hangs | They have a 10 s timeout | none (see the unknowns) |

**Fault injection.** No real input is known to hang or abort the compiler.
The parser even caps nesting at 64, so deep nesting can't overflow the
stack. So the tests inject faults: with `CUTTLEBONE_FAULT=hang:<suffix>` or
`abort:<suffix>`, a worker checking a matching file hangs or calls
`abort()`. It's documented as test-only.

**Debouncing.** A query on a document edited less than `debounceMs`
(default 50) ago waits until that much time has passed since the edit.

**Cancellation.**

- A `didChange` cancels queued and running queries on that document, which
  get `ContentModified`. A running check is killed. Queries on other
  documents carry on.
- `$/cancelRequest` gives `RequestCancelled`, queued or running.
- `shutdown` cancels everything.

Tests cover each case, including that a superseded query never starts a
worker, and that killed workers are gone (checked with `pgrep -P`).

**In-thread versus child process,** measured below: a worker adds 13 to
20 ms to a 100 ms check, of which about 5.5 ms is starting the 17.6 MB
binary. Kept.

### 4. Tests

tests/lsp.rs has 33 end-to-end tests. Each drives the real binary over
stdio with a small LSP client (tests/common/client.rs):

- **Handshake:** the exact capabilities; encoding negotiation, five cases;
  a version mismatch, then a retry; a missing `inko`; the installed `inko`
  (skipped unless it's 0.21.1)
- **A whole session:** open, incremental edits, hover, completion after
  `.` with nothing after it, and after a typed prefix, then close and back
  to disk
- **Edits:** full-document changes; several changes in one notification;
  unsaved imported modules; save with text
- **Positions:** non-ASCII positions and ranges under UTF-16 and under
  UTF-8 (é, 😀😀, a ZWJ family, CJK), including an incremental edit after
  non-ASCII text
- **Status:** `fresh`, `noHir`, `declarationErrors`, `notAFile` and
  `invalidPosition`; no notifications without opting in
- **Hover format:** plain text for clients without Markdown
- **Workspace folders:** which folder's `inko.pkg` applies
- **Failures:** a panic, a timeout, an abort, an orphaned worker
- **Cancellation:** cancelling a running and a queued request;
  superseding edits; debouncing
- **Side effects:** none
- **Lifecycle:** `shutdown`/`exit` statuses, stdin closing, shutdown
  killing a running check; unknown methods, invalid params, and a second
  `initialize`

Elsewhere:

- **tests/latency.rs** keeps milestone 1's 500 ms limit (median of 7, from
  the client). It also measures a completion, and the worker's cost
  against an in-thread check, with a 50 ms limit on that cost.
- **tests/complete.rs** tests the placeholder: after `.`, after a prefix,
  mid-name, on disk, under a non-canonical path, in UTF-8, where nothing
  has members, past the end, and `byte_offset`.
- **tests/editors.rs** runs the Neovim smoke check (below).

Ported and deleted:

- **Ported from milestone 1's tests/e2e.rs:** the handshake, both queries
  over stdio, unavailable answers, the panic, invalid requests, stdin
  closing, the latency limits, and the installed `inko`.
- **Deleted:** the protocol-version test and the malformed-JSON test.
  lsp-server ends the session on malformed framing, and LSP clients don't
  send it.
- **Moved:** the manifest test from tests/overlay.rs to tests/manifest.rs.
- **Kept:** every core test (type_at, members, broken, overlay, panic,
  crosscheck, citations, golden, corpus). Only the API they call changed:
  `projects`, `encoding`, and `code` as a `String`.

### 5. Editors

`editors/check.sh` runs both smoke checks. Each starts `cuttlebone` through
the editor's own LSP client, opens `tests/fixtures/unicode/src/main.inko`,
and hovers `[3]` after `'中文'`. It expects `Array[Int]`. In the wrong
encoding the position lands on `size` (`Int`), so the check also proves the
negotiation.

- **Neovim**: `editors/neovim/lsp/cuttlebone.lua` is a config in
  nvim-lspconfig's format, for Neovim 0.11+'s `vim.lsp.config`. You add
  `vim.lsp.enable('cuttlebone')`. The check runs headless (`nvim --headless
  --clean`), and asserts that UTF-8 was negotiated. It passes with Neovim
  0.12.6. `cargo test` runs it too, and skips it if `nvim` isn't on `PATH`.
- **VS Code**: `editors/vscode` is a 30-line extension. It registers
  `.inko` as the `inko` language and starts `cuttlebone` with
  `vscode-languageclient` 11, with `cuttlebone.path` and `cuttlebone.inko`
  settings. The check uses `@vscode/test-electron` with the installed VS
  Code (1.127), a throwaway profile, and `--disable-extensions`. It asks VS
  Code's own hover provider for the hover, and asserts UTF-16. It passes.
  It can't be truly headless on macOS: it opens a VS Code window for about
  3 s and closes it, with nothing to click. It isn't in `cargo test` for
  that reason. On Linux it would run under `xvfb-run`.
- **By hand:** editors/README.md says what to click in VS Code (F5 in
  `editors/vscode`, set `cuttlebone.path`, hover `[3]`, type `out.`) and
  what to run in Neovim (`:checkhealth vim.lsp`, `K`, `<C-x><C-o>`).

I checked that each script fails when it should: with a wrong expected type
in both editors, and with a missing binary in Neovim.

## Measurements

Apple M1 Max, release build, medians, from the client's side unless noted.

| | |
|---|---|
| Hover, file importing all of std/src (116 modules) | 116–120 ms |
| Completion, same file | 117–145 ms |
| Hover in std.net.http.server (90 modules) | 94–100 ms |
| Hover / completion on the test fixtures (53–57 modules) | 38 / 40 ms |
| Process start plus `initialize` with the real `inko` | 44 ms (milestone 1 measured 31 ms, from a different harness) |
| **The same std-sized check in the server's thread** (milestone 1's approach) | **99–108 ms** |
| **Over LSP with a worker process** | **116–121 ms: +13 to +20 ms** |
| A worker that does no type checking (a hover on a missing file), round trip | 7.5 ms |
| Starting `cuttlebone --version`, minus starting `/usr/bin/true` | ≈ 5.5 ms |
| A worker's peak memory for the std-sized check | 52 MB |
| Binary size | 17.6 MB (milestone 1: 15.8 MB) |

In debug builds, as `cargo test` uses them, the cost of a worker came out
at 1–10 ms, because the in-thread number is slower there.

**The child-process cost.** The 13–20 ms is about 5.5 ms of `exec` and
dynamic loading, plus a cold process: page faults in the compiler's code,
fresh allocator arenas, and std's sources read again (they're in the OS's
cache). In exchange, the server can kill a hung check, survives an abort or
a stack overflow, and frees each check's memory completely. Keeping a spare
worker started in advance would hide at most the 5.5 ms. It would also cost
an idle process, and make "no worker left behind" harder to test, so I
didn't build it. It's an easy addition later.

## Every patch line added, and why

**This milestone adds no lines to the patch. It removes one, and rewords
the hook's doc comment.** The rebuilt patch applies cleanly to a fresh
v0.21.1 clone. `inko check` behaves exactly as before.

| File | Milestone 1 | Milestone 2 | Change |
|---|---|---|---|
| compiler/src/compiler.rs | +17, 1 changed | +16, 1 changed | `self.prepare()?` stays in `check`, and `check_with` starts after it, so the hook doesn't sync `dep/`. The doc comment says so, in the same three lines. |
| compiler/src/config.rs | +4 | +4 | unchanged (the overlay field) |
| compiler/src/modules_parser.rs | +5, 1 changed | +5, 1 changed | unchanged (reading the overlay) |
| compiler/src/lib.rs | 1 changed | 1 changed | unchanged (`pub mod hir`) |
| compiler/src/hir.rs | 469 changed | 469 changed | unchanged (`pub(crate)` → `pub`) |

The diff of `check` now reads:

```diff
     pub fn check(&mut self, file: Option<PathBuf>) -> Result<(), CompileError> {
         self.prepare()?;
+        self.check_with(file, |_, _| true)
+    }
 
+    /// Type checks like `check` but without `prepare` (which syncs `dep/`),
+    /// and passes the typed HIR to `inspect` before MIR, even if type checking
+    /// fails. Returning `false` from `inspect` skips lowering to MIR.
+    pub fn check_with<F: FnOnce(&types::Database, &[hir::Module]) -> bool>(
```

patches/README.md justifies every line, including this one. It explains what
`prepare` does (the version check, which Cuttlebone repeats, and
`sync_if_needed`, which a query must not run) and names the test that fails
if it comes back. Five new upstream citations
(`compiler.rs:795-810`, `:812`, `pkg/manifest.rs:11-34`, `pkg/sync.rs:22-31`
and `pkg/util.rs:11-23`) are pinned by tests/citations.rs.

## The 32 mutations

`scripts/mutate.py` applies each one, runs the tests named for it, and
restores the file. All 32 were caught:

- **The patch:** `prepare()` back in the hook.
- **The version check:** no version check; dependency manifests ignored.
- **Encodings:** always UTF-16; UTF-8 counting characters instead of bytes;
  edit ranges always read as UTF-16; `byte_offset` not stripping `\r`.
- **Edits:** incremental edits treated as full replacements.
- **Timeouts and workers:** no timeout; a timeout that doesn't kill; no
  worker deadline; checks run in the server's thread; a crashed worker's
  output trusted.
- **Cancellation:** `$/cancelRequest` ignored; edits that don't supersede;
  edits that supersede every document; `shutdown` that doesn't cancel.
- **Debouncing:** no debounce.
- **Status:** notifications sent without opting in.
- **Hover:** always Markdown; no range.
- **The handshake:** a version mismatch accepted; the completion trigger
  missing; full sync advertised.
- **Completion:** no placeholder inserted; completion kinds swapped.
- **Documents:** `didSave` text ignored; `didClose` ignored; other open
  documents not sent.
- **Workspace folders:** ignored.
- **Lifecycle:** exit status ignoring `shutdown`; requests after `shutdown`
  answered.

Milestone 1's 19 core mutations cover code that didn't change, apart from
the API, so I didn't rerun them.

## Decisions you may want to revisit

- **One check at a time, in order.** A hover queued behind a slow
  completion waits for it, for at most the timeout. Checks could run in
  parallel workers (they're independent processes now), at 50 MB each.
- **A newer edit cancels completion too.** Typing `.` then `s` within the
  check's 120 ms cancels the first completion with `ContentModified`, as
  you asked. LSP clients are meant to drop such responses quietly. I
  haven't watched whether VS Code or Neovim ask again while you keep
  typing, or how it feels. Keeping a superseded completion
  alive when only a prefix was added would be the alternative.
- **The default timeout (5 s) and debounce (50 ms)** are guesses. Both can
  be set per client.
- **Status is opt-in, by notification.**
- **The fault-injection variable ships in the binary.** It does nothing
  unless set, and only in workers. It could move behind a cargo feature if
  the tests built the binary with that feature.
- **The `inko` language ID.** Hover code blocks use `` ```inko ``, and the
  VS Code extension registers the language ID `inko`. These identify the
  language for highlighting, and nothing is named after it: the extension,
  binary, settings and notification are all `cuttlebone`.
- **Version checking happens once, in `initialize`.** Switching toolchains
  mid-session needs a restart (or a new `initialize`, which LSP clients
  don't send).

## Remaining unknowns

1. **Lone `\r` line endings.** LSP counts a lone `\r` as a line break;
   Cuttlebone and the compiler don't. Positions after one are off. `\n` and
   `\r\n` are tested.
2. **Typing feel.** The debounce and cancellation are tested for
   correctness, not for feel in a real editor while typing fast.
3. **Hangs and aborts** are still only known from fault injection. The
   server now survives them, but no real input is known to cause them.
4. **Windows.** It's untested:
   - The tests use `pgrep` and `kill -0`.
   - The abort test expects signal 6.
   - The worker design itself is portable.
5. **Hanging `inko`.** A hang in `inko --version` during `initialize` blocks
   the server for up to 10 s. That's untested.
6. **Files that aren't on disk** (new unsaved files) can't be checked. The
   compiler finds modules on disk before reading the overlay.
7. **From milestone 1, still open:**
   - no answers in code with declaration or syntax errors (`stale` is
     reserved for that)
   - `try` with mismatched error types
   - the member gaps: a field and a method with the same name, re-exported
     symbols, and `Self` in some trait signatures
   - performance on projects much larger than std
   - packaging
   - the two upstream bugs I found and didn't report

## What any client needs to know

1. **Start** `cuttlebone` (or `cuttlebone --stdio`) with stdio pipes, one
   process per editor session. Pass the workspace folders. stderr is a log.
2. **Settings**, as `initializationOptions`:
   - `inko`: the toolchain's executable
   - optionally `checkTimeoutMs`, `debounceMs` and `statusNotifications`
3. **A version mismatch** comes back as a failed `initialize` with a
   message meant for people. Show it. `data.installed` and `data.supported`
   have the versions. Send `initialize` again after the user changes the
   toolchain.
4. **Offer `utf-8`** in `general.positionEncodings` if positions are byte
   offsets for you. Otherwise do nothing and get UTF-16.
5. **Sync documents normally**; incremental sync is preferred. Unsaved
   buffers are used automatically, and nothing needs to go with each
   request.
6. **Completion:** send the real buffer and cursor. Don't insert a dummy
   identifier. Filter the full list by the typed prefix yourself; it's
   never incomplete.
7. **Hover** is a `` ```inko `` code block. Declare plain text in
   `hover.contentFormat` to get the bare type.
8. **No answer** is `null`. If you have fallback heuristics, opt into
   `cuttlebone/status` to learn why (`unavailable` with a `reason.code`),
   and fall back on anything but `fresh`.
9. **Expect `ContentModified`** for requests overtaken by edits, and
   `RequestCancelled` for ones you cancelled. Both are normal.
10. **Don't time out checks** or restart the server for slowness. The server
    does that itself. Do restart it if it exits, as any LSP client does.

### For Parakeet in particular

Parakeet can use Cuttlebone through an LSP client (LSP4IJ, or IntelliJ's
own LSP API), like any other editor:

- **Drop the old client service.** The custom protocol, its client service,
  per-request timeouts, restarts, and sending unsaved documents with every
  request are all gone.
- **Drop `IntellijIdeaRulezzz`.** The server inserts its own placeholder
  on the real document.
- **Type info** (`ExpressionTypeProvider`) maps onto `textDocument/hover`.
  Declare `plaintext` hover support to get the bare type, or strip the code
  fence. The hover's range is the expression's range.
- **Fallback heuristics:** handle `cuttlebone/status` as a custom
  notification in the LSP client, to know when to use them.
- **A bug still in Parakeet** (from milestone 1): diagnostics from
  `inko check` treat grapheme-cluster columns as UTF-16 offsets.
