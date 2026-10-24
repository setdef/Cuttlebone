# Milestone 7: what an answer covers, versions, and LSP 3.18

**Done.** Four tasks, and one problem found on the way:

- **Coverage.** Every inlay hint status now says which ranges of the
  request the answer covers (`covered`). A client can tell "no hints here"
  from "no answer here". On the edit-replay corpus, whole-file hint
  requests mid-edit leave **17,180 hints of the final text uncovered** (in
  text that was there at the time), each of which a client used to read
  as "no hint". None of the covered ranges disagrees with the final text,
  and no answer has a hint outside them.
- **Versions.** This is 0.7.0. `cuttlebone --version` and `serverInfo`
  say so, [CHANGELOG.md](../../CHANGELOG.md) has an entry per milestone,
  and AGENTS.md and a test hold every milestone to a new version and an
  entry.
- **Installing:** README.md says `cargo install --locked --path .`.
- **LSP 3.18.** Every message the tests receive is now checked against
  the 3.18 meta model and the client's capabilities. Checking found eight
  ways Cuttlebone didn't conform, all fixed (below). Every remaining
  extension has its reason in docs/lsp.md, with evidence from VS Code's
  and Neovim's clients for the one the spec seemed to cover.
- **A hole in the guardrails.** Since milestone 6, ten tests failed in
  the mutation runner's copies of the tree with no mutation at all, so
  the 20 mutations they guard were reported caught without proof. Fixed,
  the runner now checks for it before every run, and the one real gap it
  hid has a test.

**Tests:** 276 (25 unit, 241 integration, 10 latency), up from 259.
`scripts/mutate.py` has 233 mutations: 197 from milestone 6, 2 of them
removed with the code they broke and 8 with new snippets, and 38 new.
**The patch didn't change.**

## 1. What an answer covers

### The problem

Milestone 5 answers an inlay hint request in code being edited from a
repaired copy or the last good check, leaving out the units whose answer
no longer holds (the method being edited, usually). The result is an
`InlayHint[]`, so a client sees `[]` for the method being edited and
can't tell it from a method with no hints. Parakeet, which has guesses
of its own, showed nothing there instead of them.

### The design

`cuttlebone/status` gains `covered`: the ranges of the request where the
answer's hints are all there are, in the negotiated encoding, each
including its end, as the request's range does (docs/lsp.md, "What an
answer covers").

- Every inlay hint status has it: `[request's range]` when fresh, `[]`
  when unavailable, and what holds when repaired or stale.
- What holds is decided by the same rule as which hints are given
  (`Guard::hint_at`): a position in text unchanged since the check, with
  the characters on either side, in a unit that still means what it did.
  The hints and the coverage can't disagree, because one function decides
  both.
- A client that ignores the field sees what it did.
- Other requests' answers are whole or none (docs/lsp.md says why for
  each), so they have no `covered`.

**No standard design fits** (task 4 asked for one if it did): an
`InlayHint[]` has nowhere to put it. I considered:
- partial results (`partialResultToken` and `$/progress`), which stream
  parts of a result that must still end up whole
- `workspace/inlayHint/refresh`, which asks the client to request every
  hint again, without saying where an answer was missing
- an error response, which is all or nothing
- a per-hint `data`, which can't mark a range that has no hints

### The test: the corpus

tests/replay.rs now:

- compares hints only inside the ranges the mid-edit answer says it
  covers, on both sides: there, its hints must be exactly the final
  text's (in text that existed in both)
- counts a hint outside those ranges as a wrong answer
- also asks for the whole file's hints after every edit, as clients do,
  besides each line's (`inlayHint, whole file` in the tables)

Whole-file hint requests mid-edit, full corpus, on the final code:

| Mode | Asked | Answered | Wrong |
|---|---|---|---|
| as the server answers | 443 | 443: 60 fresh, 306 repaired, 3 stale, and 74 fresh that differ at the end (the code meant something else then) | 0 |
| without older checks | 443 | 303 | 0 |
| without repaired copies | 443 | 443 | 0 |

Over every inlay hint question (per line and whole file), the answers
covering only part of what they were asked, and the final text's hints
in text that was there then where those answers don't cover:

| Mode | Partial answers | Final hints not covered |
|---|---|---|
| as the server answers | 379 | 17,180 |
| without older checks | 187 | 14,953 |
| without repaired copies | 379 | 23,451 |

Per line, 11,088 questions as before, with no wrong answers either. A
line is nearly always wholly in one unit, so only 70 line answers covered
part of their line, and none of those left out a final hint. The
whole-file requests, which are what clients send, are where coverage
matters.

tests/stale.rs checks `covered` over LSP: the request's range when fresh,
the rest of the file but not the method being edited when stale, clipped
to a smaller request's range, and `[]` when unavailable.

## 2. Versions

- Cargo.toml's version is `0.7.0`, which `--version`
  (`cuttlebone 0.7.0 (Inko 0.21.1)`), `serverInfo.version` and the
  version refusal's message and `data` report. A new test runs
  `--version`; nothing tested it before.
- [CHANGELOG.md](../../CHANGELOG.md) has an entry for each milestone,
  0.1.0 to 0.7.0, each linking to its report. Milestones 1 to 6 were all
  built as 0.1.0; the changelog says their versions were given
  afterwards.
- AGENTS.md, "Versions": a milestone's first commit bumps the version and
  adds its entry. That way a build of work in progress already differs
  from the milestone before it, and the entry's link to the report
  resolves once the report lands. The milestone-report skill now says to
  finish the entry.
- tests/guardrails.rs fails if CHANGELOG.md's newest entry isn't the
  version, if an entry doesn't link to its milestone's report, if a
  report it links to is missing (except the current milestone's), or if
  docs/lsp.md shows another version anywhere.

## 3. Installing

README.md, "Building": `cargo install --locked --path .`, and why
`--locked` matters: it builds with the versions in Cargo.lock, which the
tests ran against. editors/README.md points to it. I didn't run it.

## 4. LSP 3.18

### How it's checked

**Every message, in every test.** tests/common/spec.rs reads the 3.18
meta model (vendored, with its CC BY 4.0 attribution, in
tests/fixtures/lsp), and the test client checks every message it
receives against it:

- the result type of the request a response answers, or the base
  protocol's `ResponseError` (with `initialize`'s own `errorData`)
- a notification's params
- strictly: every required property is there, no property the type
  doesn't have, base types in range, enumerations' values

It also checks what depends on the client's capabilities (below), against
the capabilities the test client actually sent. A test fails on a message
that doesn't conform. Most tests send the capabilities of a client that
supports everything Cuttlebone uses (`capable_client`, itself checked
against the meta model's `ClientCapabilities`). tests/conformance.rs
covers a client that sends none, checks that the checks catch what they
should, and that the client fails on a fake server's non-conforming
message.

**Every request, response, notification and capability**, by reading the
spec (base protocol, lifecycle, document sync, each request Cuttlebone
answers, and the types): what follows is everything that didn't
conform, then what 3.18 changed.

### What didn't conform, and the fixes

| | What the spec says | Before | Now |
|---|---|---|---|
| Edits during a request | `ContentModified`: "A server should NOT send this error code if it detects a content change in its unprocessed messages. The result even computed on an older state might still be useful for the client." Implementation Considerations: servers "should not decide by themselves to cancel requests simply due to that fact that a state change notification is detected in the queue" | a `didChange` failed every query on the document with `ContentModified`, and killed the running check | each request is answered for the documents as they were when it arrived (`Frozen`, shared by every query queued at the same edit); a running check carries on. Such a check is never a good check for later stale answers, since the edits since its documents weren't recorded against them |
| `shutdown` | `RequestCancelled`: "The client has canceled a request" | pending queries got `RequestCancelled` | `null`, with reason `shutdown` |
| Line ends | `\n`, `\r\n` and a lone `\r` end lines | a lone `\r` didn't (docs/lsp.md said so) | positions convert through byte offsets: the compiler's lines end at `\n`, editors' at all three |
| Hover format | `contentFormat`: the client's formats, "the order describes the preferred format" | Markdown unless the client's list lacked it, including when it sent none | the first of `markdown` and `plaintext` in the client's order; plain text if it names neither |
| Completion | `labelDetails` needs `labelDetailsSupport`; kinds past Reference (18) need `completionItemKind.valueSet` | always sent; Constant (21) to every client | only with the capability; Value (12) for a constant otherwise |
| Signature help | label offsets need `labelOffsetSupport`; `SignatureInformation.activeParameter` needs `activeParameterSupport`; past the last parameter, `activeParameter` can be `null` only with 3.18's `noActiveParameterSupport`, and left out it means the first | offsets and the signature's `activeParameter` to every client; past the last parameter, left out (so the first was highlighted) | offsets or the parameter's text; the signature's only with the capability; `null` for clients that support it |
| Diagnostic tags | `tagSupport.valueSet` | `Unnecessary` to every client | only if the client lists it |
| The client's process | `processId`: "If the parent process is not alive then the server should exit"; servers should accept `--clientProcessId` | exited only when stdin closed; `--clientProcessId` was a usage error | if the client's process started the server and is gone, it exits, checking once a second (std's `parent_id`, no `unsafe`); the option is accepted |

**A mistake of mine the checks caught.** The spec's text puts
`noActiveParameterSupport` at `textDocument.signatureHelp`, while the
meta model has it in `signatureInformation`, which is where VS Code's
client (lib/common/signatureHelp.js:21) and Neovim
(runtime/lua/vim/lsp/protocol.lua:536) send it. I first read the text's
place, so neither client would ever have got `null`. Checking the tests'
own capabilities against the meta model would have caught it; that check
is a test now, and both places count.

**Kept as they were, and why they conform:**
- `initialize`'s refusal uses `RequestFailed`, since
  `InitializeErrorCodes` has only the deprecated `unknownProtocolVersion`.
- `ServerNotInitialized` before `initialize` and dropping notifications
  then is lsp-server's handling, which the spec's.
- Requests after `shutdown` get `InvalidRequest`. `$/` requests get
  `MethodNotFound`, and `$/` notifications are ignored, as the spec
  allows.
- A character past the end of its line means the end of the line.

### What 3.18 changes, for Cuttlebone

From the meta model's `since` fields, on the types Cuttlebone sends or
reads, and the change log:

| 3.18 | For Cuttlebone |
|---|---|
| `activeParameter: null`, with `noActiveParameterSupport` | used (above) |
| `Diagnostic.message` may be `MarkupContent`, with `markupMessageSupport` | not used: the compiler's messages are plain text |
| `MessageType.Debug` | not used: logs go to stderr, which docs/lsp.md says clients shouldn't parse |
| `CompletionList.applyKind` | not used: no `itemDefaults` |
| inline completion, `workspace/textDocumentContent`, folding range refresh, ranges formatting, snippets in edits, edit metadata, code action kind docs, command tooltips, relative patterns in filters | features Cuttlebone doesn't have |

**lsp-types stays at 0.95.1.** The newest release, 0.97.0 (June 2024),
predates 3.18 (June 2026) and has none of its changes that matter here.
Upgrading would only replace `url::Url` with another URI type throughout.
So Cuttlebone reads the client's capabilities as JSON (`Support::of`,
which also sees what lsp-types would drop) and builds signature help's
JSON itself (lsp-types can't send `null` there).

### The extensions, re-examined

docs/lsp.md, "Extensions", has a row for each, with why the spec's own
channels don't solve the problem:

- **`cuttlebone/status`: kept.** The spec's way to say why a request has
  no answer is an error, `RequestFailed` with `data`, which would tie the
  reason to its request. But the reason is routine (the code doesn't
  parse while it's typed), and clients show those errors to people:
  - **VS Code**: vscode-languageclient 11.0.0 (editors/vscode's), in
    lib/common/client.js. `handleFailedRequest` (lines 1854 to 1886)
    returns quietly only for `RequestCancelled`, `ServerCancelled` and
    `ContentModified`; for any other error it calls
    `this.error("Request <method> failed.", error, true)` (line 1884).
    With the default `revealOutputChannelOn` of `Error` (line 417),
    `error` (lines 854 to 861) shows it with `window.showErrorMessage`:
    a popup for each hover, definition, highlight, signature help or
    inlay hint request in broken code.
  - **Neovim** 0.12.6, runtime/lua/vim/lsp:
    - default handlers (`documentHighlight` among them) notify every
      error but `ContentModified` at `ERROR` level (handlers.lua:719-724)
    - signature help does too (buf.lua:372-376), and completion at `WARN`
      (completion.lua:741)
    - only hover (buf.lua:95), inlay hints (inlay_hint.lua:42) and
      definitions log it quietly

  So errors would mean popups while typing. Where an answer came from
  (`stale`, `repaired`, `cached`) and what it covers have no field in any
  result, and `publishDiagnostics` can't say a check didn't finish.
- **`experimental.cuttlebone`: kept.** It's the spec's place for what
  isn't in it; `serverInfo` has only a name and a version.
- **`initializationOptions`: kept.** It is the spec's field for settings,
  whose names are the server's. `workspace/configuration` would arrive
  only after `initialize`, which needs `inko` to accept or refuse the
  session.
- **The version refusal's extra `data` fields: kept**, beside `retry`
  (the spec's `InitializeError`), since `data` is where an error carries
  more. The test client's checks allow exactly these extra properties
  and no others.
- **`covered`: new**, and an extension for the reasons in section 1.

### Requests for their own text: the cost

A queued query now keeps the documents it was asked about if an edit
comes first: one shared copy of the open documents per edit that finds
queries waiting, and each file's last good version, both behind `Arc`s.
A client that doesn't cancel requests it no longer needs now gets
answers for older text, at a check each, where it used to get
`ContentModified` for free. VS Code's client cancels them (its
`staleRequestSupport` says `cancel: true`), and the debounce still means
a request right after an edit waits, so a cancel in that time saves the
check.

### Lone `\r` and speed

The first version of the new line scanning searched for `\n` and `\r`
together (`find(['\n', '\r'])`), char by char. The full corpus went from
about 60 s to over ten minutes. Two byte searches per line, and a fast
path for text with no `\r` at all, brought it back. The latency tests
hold every query to the same limits as before.

## 5. A hole in the guardrails, and the runner's control

Measuring this milestone's corpus time against milestone 6's code, in a
copy of the tree, milestone 6's corpus failed with 40 wrong answers. In a
copy, `.upstream` is a link to the tree's own (milestone 6's runner makes
its copies that way). So the tests' `upstream_std()` wasn't the
canonical path the compiler and the server use, and the tests' own
mapping of locations in an edited std file missed. **Ten tests**
(the corpus, five in tests/definition.rs, four in tests/navigation.rs,
and the std definition corpus) failed in every copy, mutation or not. A
mutation whose tests included one counted as caught whatever it did: 20
of milestone 6's 197. The milestone 6 report's tiers passed, but those 20
verdicts proved nothing. A mutation that only edits a comment came out
"caught" by the corpus (I checked, and the runner kept no output to show
why).

Fixed:

- `upstream_std()` is canonical (src/dump.rs, helpers for the tests).
- The runner first runs every test unmutated in a copy of the tree,
  built and run exactly as a mutation's are, and stops if any fails
  (`scripts/mutate.py --control`, 145 to 240 s). A mutation of the fix
  is caught by it.

With the fix, two mutations survived the first exhaustive run, and both
now have what catches them:

- **Milestone 5's "answers before an edit in an older check"** (older
  checks answering, like repaired copies, before the edit in the method
  being edited). The milestone 5 report says the corpus caught it with
  wrong answers. It no longer does: with it, the corpus answers 4,382
  stale hovers instead of 2,988, none wrong. The rule's reason still
  stands, so tests/stale.rs now tests it directly: `let values = []`
  whose type came from a `values.push(1)` being retyped, where the older
  check would say `Array[Int]`.
- **This milestone's "coverage ignores the margins"**: the margin
  condition in coverage could never change the result, since `safe`
  already excludes positions next to an edit in a unit, and hints are
  never between units. Hints and coverage now share one function
  (`hint_at`), which existing mutations break, and that mutation is gone.

## Every patch line added, and why

None. patches/upstream.patch is unchanged.

## Tests

| File | What |
|---|---|
| tests/common/spec.rs (new), tests/conformance.rs (new, 6) | The 3.18 checks of every message, and what a client with no optional capabilities gets: plain-text hovers, completion without `labelDetails` and with Value for constants, signature help with parameter text and no `null`, diagnostics without tags. The tests' own capabilities conform; the checks catch what they should; the client fails on a non-conforming server. |
| tests/replay.rs | Hints compared inside what an answer covers, none allowed outside, whole-file requests, and their floors. |
| tests/stale.rs (+3) | `covered` over LSP; a check of documents edited since isn't a good check; a query frozen by an edit still answers from the last good check; an older check doesn't answer before the edit. |
| tests/lsp.rs (+4, 4 changed) | `--version`; a lone `\r` ending lines (hover, a diagnostic's range, an incremental edit, both encodings); a request answered for its own text; exiting when the client's process is gone (`processId`, and `--clientProcessId`). Changed: an edit doesn't cancel a running check; the debounce test; `null` at shutdown; cancelling one of two queued requests; hover formats in the client's order. |
| tests/signature.rs (+1) | `null` past the last parameter, present rather than left out. |
| tests/guardrails.rs (+2) | The changelog entry; docs showing the version. |
| src/position.rs (+1) | Lone `\r` and `\r\n` both ways. |

## The mutations

All 233 are caught. **The exhaustive tier passed on 5656e6d** (the code
of the commit with this report), which it had to, since the mutation
runner and tests/common changed (AGENTS.md): 1,604 s in all, of which the
runner's control 116 s, the whole corpus 75 s, every test without kept
checks 124 s, and the 233 mutations 1,343 s. It includes everything the
milestone tier runs. The first exhaustive run, before the fixes in
section 5, caught 228 of 235: two not caught (section 5) and five whose
code had moved.

New, by task:
- **Coverage:** hints covering everything asked, coverage stopping at its
  first gap, not clipped to the request, no `covered` in the status, an
  unavailable answer covering the request, a fresh one covering nothing.
- **Versions:** a version without an entry, an entry without its report,
  docs showing another version, `--version` and `serverInfo` without it.
- **Conformance:** hover Markdown for every client, and ignoring the
  client's order; `labelDetails` and the Constant kind for every client,
  and the kind for none; label offsets, the signature's
  `activeParameter`, and `null` for every client; no `null`; parameter
  text off by one; `noActiveParameterSupport` read only where the spec's
  text puts it; diagnostic tags for every client; the client's conformance
  check off; lone `\r` not a line end, `\r\n` as two, a position between
  them, compiler lines ending at `\r`.
- **Requests for their own text:** edits not freezing queued queries,
  freezing without the fallback, edits killing the running check, frozen
  checks counting as good checks, `$/cancelRequest` cancelling the wrong
  request, shutdown answering with errors.
- **The client's process:** not watched, `processId` ignored,
  `--clientProcessId` ignored.
- **The runner:** `upstream_std()` not canonical, caught by `--control`.

Removed: "edits don't supersede" and "supersede cancels every document",
with the code they broke; "coverage ignores the margins" (above).
New snippets: eight, whose code moved ("always markdown", "byte_offset
ignores CR", "cancel ignored", "other open documents not sent", "hints
past the range", "inlay hints for a range with nothing safe", "no
fallback sent", "no fallback from the target's check").

## Measurements

Apple M1 Max, 64 GB, `cargo test` (dependencies optimized), with nothing
else heavy running:

| | |
|---|---|
| The whole corpus, milestone 6's code, in a copy of the tree | 58 to 65 s (failing, as above) |
| The whole corpus, this milestone's code, run alternately with it | 96 to 101 s: the whole-file requests, with coverage over every byte of the file each |
| The corpus sample, every commit | 3.5 s |
| The runner's control | 116 to 243 s |
| The exhaustive tier, on 5656e6d | 1,604 s |
| Every commit (`scripts/check.sh`) | 60 to 83 s |

## What clients should know

1. **`covered`** on every inlay hint status: where the hints are all
   there are. Outside it, show your own guesses, or nothing.
2. **No more `ContentModified`.** A request is answered for the text it
   was asked about, even after an edit. Cancel what you don't need.
3. **At `shutdown`**, pending requests get `null` with reason `shutdown`.
4. **Capabilities matter now.** Without `labelDetailsSupport`, completion
   has no module; without a `completionItemKind.valueSet` that has 21,
   constants are Value (12); without `labelOffsetSupport`, parameters are
   text; send `contentFormat` to get Markdown hovers.
5. **`activeParameter: null`** past the last parameter, for clients with
   `signatureInformation.noActiveParameterSupport`.
6. **Lone `\r`** ends a line in positions.
7. **The server exits** when the client process that started it is gone.
8. **Versions**: `serverInfo.version` is the milestone's (0.7.0).

## Decisions you may want to revisit

- **Freezing instead of `ContentModified`** follows the spec, and costs
  checks with clients that don't cancel. A client that wants the old
  behaviour can cancel on edit.
- **Plain-text hovers when the client names no format.** The spec gives
  no default; Markdown to a client that didn't say it could render it
  seemed the worse guess.
- **`covered` on inlay hints only.** Highlights and references are whole
  or nothing, so they don't need it; if they ever answer in part, they
  should carry it too.
- **The status stays an extension.** If clients ever show
  `RequestFailed` quietly, the reasons could move to error `data`.
- **The parent process is watched only if it's the server's parent**,
  since that needs only std. A client that starts the server through a
  wrapper relies on stdin closing.
- **Coverage is computed byte by byte** over the request's range. Cheap
  for a line, and the whole-file corpus requests cost most of the
  corpus's added time; computing it over units and runs would be faster.

## Remaining unknowns

1. **Other clients' handling of `RequestFailed`**: only VS Code's and
   Neovim's were read.
2. **Wrong answers the corpus can't see**, as before; the hole above
   shows the corpus can stop exercising a rule without anything failing.
   Rules it doesn't reach now need tests of their own.
3. **From earlier milestones, still open:** the compiler's passes after a
   declaration error, Windows, and packaging beyond `cargo install`.
