# Changelog

Each milestone has its own version, `0.<milestone>.0`, reported by
`cuttlebone --version` and in `initialize`'s `serverInfo.version`. A
milestone's first commit bumps the version and adds its entry here, so a
build of work in progress already differs from the milestone before it;
the entry links to the milestone's report, which its last commit adds
(AGENTS.md, "Versions"). Milestones 1 to 6 were all built as 0.1.0; their
versions here were given afterwards.

## 0.17.0

Milestone 17: what a client needs to drop its own index of
declarations: modules as workspace symbols, and each declaration's
signature, doc comment, and whether it's static, public or a trait's
required method, from standard fields.
[Report](docs/history/MILESTONE-17.md).

## 0.16.0

Milestone 16: answers a client that relies on Cuttlebone lost:
diagnostics past a module that doesn't parse, implementations,
hierarchies and file renames past a declaration error elsewhere, code
lenses with every declaration's implementations and supertypes in one
request, a status for every file rename, a selection step with a
declaration's doc comment, and a trait's required and default methods.
[Report](docs/history/MILESTONE-16.md).

- **Diagnostics past a module that doesn't parse**: the rest of the
  project is checked without it and what imports it, as `inko check`
  checks a copy of the project without them; those keep `inko check`'s.
- **Implementations, incoming calls, supertypes, subtypes and file
  renames past a declaration error elsewhere**: refused only where a unit
  the error makes unsafe names what they're about, as references; so
  callers also in a document being edited.
- **Code lenses** (`textDocument/codeLens`, for a client with them):
  every declaration's implementations and supertypes, and each `impl`'s
  trait and its methods' trait's methods, in one request, with their
  locations in the command's arguments, and `covered` in the status.
- **A status for every file rename**, `fresh` where no module's name
  changes, naming the request's `files`.
- **A selection step with a declaration's doc comment.**
- **A trait's default methods** have ` { ... }` after the signature in
  document symbols' `detail`; required ones don't.
- For Inko 0.21.1, as before. Seven patch lines (a project check can leave files out), no new extension.

## 0.15.0

Milestone 15: what a client that indexes Inko itself still needed.
[Report](docs/history/MILESTONE-15.md).

- **Quick fixes in files with other errors**: a fix is offered when it
  removes its diagnostic and adds no error, not only when the file then
  has none. Imports are offered for the names in the requested range,
  also for a client that sends no diagnostics in the context.
- **Four more quick fixes**: `missing-field` (the field added with a
  value of its type), `invalid-implementation` (every method the `impl`
  block lacks, stubbed, in one action for all its diagnostics),
  `missing-trait` (`impl R for T` with its methods), `unreachable` (the
  code removed). Each still passes `inko check`.
- **Completion of names in code being edited** is refused only where a
  change or an error can bind or hide a name, not whenever any
  declaration differs; and **keywords**, where the grammar takes them.
- **Document symbols hold still**: a brace a method's body opens and
  nothing closes yet leaves out only that method, not the type it's in.
- **Signature help for a type's constructor** (`Point(x: 1)`), from its
  fields.
- **Folding ranges' `collapsedText`**, for clients that take it.
- **Workspace symbols for an index**: `impl` blocks, every symbol for an
  empty query, and every match of any query as partial results.
- For Inko 0.21.1, as before. No patch change, no new extension.

## 0.14.0

Milestone 14: implementations, declarations, call and type hierarchies,
selection ranges, imports rewritten when files move, pull diagnostics, and
latency limits scaled by each CI runner's measured speed.
[Report](docs/history/MILESTONE-14.md).

- **`textDocument/implementation`**: a trait's implementing types, and a
  trait's method's implementations (required, or a default redefined),
  from the type database, across the project, its dependencies and std.
- **`textDocument/declaration`**: a type's method that implements a
  trait's is declared by the trait's method; anything else as definition.
- **Call hierarchy** (`prepareCallHierarchy`, `incomingCalls`,
  `outgoingCalls`) and **type hierarchy** (`prepareTypeHierarchy`,
  `supertypes`, `subtypes`: traits a trait requires, traits a type
  implements), by the compiler's ids.
- **`textDocument/selectionRange`**: tokens, chains, elements,
  statements, brackets and declarations, from the syntax, also in code
  that doesn't parse.
- **`workspace/willRenameFiles`**: the edits to every import of a module
  (or a directory's modules) that moves, and to its uses by its own name,
  for clients that rename files.
- **Pull diagnostics** (`textDocument/diagnostic`, `workspace/diagnostic`)
  for clients that pull them, which then get no pushes: the same as
  `inko check`, for files open or not, with result ids and a long poll.
- **Fixed:** nothing that's in a release; the call hierarchy's first
  version panicked at an external function (`fn extern`), found by the
  std oracle before any release.
- The strict latency limits are scaled by each machine's measured
  baseline (scripts/latency.sh), and the latency job is a workflow of its
  own (`.github/workflows/latency.yml`). No patch change, no new
  extension.

## 0.13.0

Milestone 13: answers after an edit is undone, folding ranges, formatting,
quick fixes, and the exhaustive checks in CI before each release.
[Report](docs/history/MILESTONE-13.md).

- **Fixed: a document edited, closed, and then changed on disk** (or
  opened again with other text) had its edits paired with the file's new
  text: a worker crash (`checkCrashed`) if the file got shorter, and
  possibly wrong answers from the last good check if it kept its length.
  0.10.0 to 0.12.0 have it.
- **`textDocument/foldingRange`**: blocks, comment lines and imports,
  from the tokens alone, so also in code that doesn't parse. Kinds and
  `rangeLimit` as the client says.
- **`textDocument/formatting`**: exactly `inko fmt`'s text, from the
  upstream formatter, as one edit; nothing written.
- **`textDocument/codeAction`**: quick fixes by diagnostic ID (imports,
  unused imports, variables and results, `mut` for immutable variables
  and fields), each applied to a copy and checked in full, offered only
  if that leaves no error. For clients with `codeActionLiteralSupport`.
- **Definition at a refused assignment** (to an immutable variable or
  field) goes to what the compiler looked up.
- **Workspace symbols** do less per keystroke.
- **Releases** wait for the release tier in CI: every test, the latency
  limits as written, and every mutation.
- One patch change (+3 −3, the formatter's `Document` made public); no
  new extension.

## 0.12.0

Milestone 12: documentation in hover, document and workspace symbols, and
completion of names. [Report](docs/history/MILESTONE-12.md).

- **Hover shows the declaration**, after the type: its signature as the
  source writes it, `Defined in` its module and type, and its doc comment
  as Markdown, the text `inko doc` reads (a trait's for an implementation
  without its own). For std, dependencies and the project; on a
  declaration's own name too. Sections are separated by `---`; the type
  is still the first code block where there's an expression.
- **`textDocument/documentSymbol`**: types, traits, `impl` blocks,
  methods, fields, constructors and constants, nested, with their
  signatures as `detail`, from the syntax alone: in code that doesn't type
  check, and (from a copy with lines blank, `repaired`) in code that
  doesn't parse. `SymbolInformation[]` for clients without hierarchy.
- **`workspace/symbol`**: the same declarations of the project, its
  dependencies and std, matched as clients filter (the characters in
  order, any case), the project's first, at most 500. Fresh with every
  file on disk and open document; nothing is written.
- **Completion of names**: variables in scope, what `self` has, fields
  after `@`, what the module binds, types in a type's name; public
  symbols of modules not imported, with `additionalTextEdits` that add the
  import as `inko fmt` would; modules and symbols in an `import`. `@` is a
  trigger character. Names are `isIncomplete`. In code being edited, names
  come only where no declaration differs.
- No new extension, and the patch is unchanged.

## 0.11.0

Milestone 11: a license, a release index, and requests from a client's
reports. [Report](docs/history/MILESTONE-11.md).

- **License: MPL-2.0**, as Inko's. LICENSE is at the repository's root,
  and every release archive now holds it beside the compiler's
  `LICENSE-MPL-2.0`.
- **A release index**, `index.json`, with each release, and at
  `releases/latest/download/index.json`: every release's version, the
  Inko versions it has builds for, and its manifest's URL, so a client
  can find a build for an older Inko ([docs/releases.md](docs/releases.md)).
  The manifest is unchanged.
- **Declaration errors by ids.** An error in a declaration no longer
  refuses code that only mentions its name by another symbol's id: with
  an error in `A.size`'s signature, a method that only calls `B.size` is
  answered.
- **An import of a module that doesn't exist** (`noHir`) is blank in the
  repaired copy, so the rest of the file is answered, `fresh` and
  `repaired`, even before any good check. A whole import now counts only
  the names it binds, not its path's leading ones.
- **Definition at `c.value += 1`** answers the setter and the getter, as
  a `Location[]`. Every other position still answers one `Location`.
- **Saving keeps kept checks.** A save (in place, or by renaming a new
  file over the old) no longer makes the next question a whole check.
- **References at a declaration outside the project** (std's, or a
  dependency's in `dep/`) answer the project's uses, instead of
  `noHir` or the declaring library's own.

## 0.10.0

Milestone 10: prebuilt binaries.
[Report](docs/history/MILESTONE-10.md).

- **Prebuilt binaries** for macOS and Linux, on AMD64 and ARM64 (Linux's
  static, for musl and glibc distributions alike), on each GitHub
  release, built by its own workflow. Each build works with one Inko
  version, which its name says.
- **A manifest** with each release (`manifest.json`): per build, the Inko
  version, target, URL, size and SHA-256, so a client can pick the build
  for the user's platform and Inko without scraping
  ([docs/releases.md](docs/releases.md)).
- No change to the server's behaviour.

## 0.9.0

Milestone 9: cheaper answers while typing.
[Report](docs/history/MILESTONE-9.md).

- **Questions with no answer are kept.** In code that doesn't parse, a
  second question about the same text is a lookup, whatever the answer,
  `unavailable` included: the check that found nothing to answer from is
  kept too (status `cached`). An `unavailable` answer is `cached` only if
  no check ran for it.
- **Cancelling is cheap.** `$/cancelRequest` on a running query answers
  it at once, and its check finishes in the worker, which keeps it and
  everything it kept, instead of being killed. The next query waits for
  it for at most 250 ms. A hung check is still killed at
  `checkTimeoutMs`. A request cancelled while it waits out the debounce
  never reaches the worker, as before.

## 0.8.0

Milestone 8: answers when a declaration has an error.
[Report](docs/history/MILESTONE-8.md).

- **Errors in declarations.** An undefined type in a signature, an
  import of something that doesn't exist and the like no longer leave
  the project without answers. The type checker runs on past them (a
  setting in the patch, which `inko check` leaves off), and every query
  answers, `fresh`, in code the error can't affect. Elsewhere the answer
  is `null` with reason `declarationErrors`, which now means the error
  may change it.
- **References** past such an error are given when no code it may
  affect mentions the name, in any module, instead of never.
- **Diagnostics** are unchanged: exactly `inko check`'s.

## 0.7.0

Milestone 7: what an answer covers, versions, and LSP 3.18.
[Report](docs/history/MILESTONE-7.md).

- **Coverage.** Every inlay hint status says which ranges of the request
  the answer covers (`covered`), so a client can tell "no hints here"
  from "no answer here", such as in the method being edited.
- **LSP 3.18.** Every message the tests receive is checked against the
  3.18 meta model and the client's capabilities. Changes for clients:
  - a request is answered for the text it was about, even if an edit
    comes first, instead of `ContentModified`
  - requests pending at `shutdown` get `null` (reason `shutdown`), not
    `RequestCancelled`
  - a lone `\r` ends a line
  - Markdown hovers, completion's `labelDetails` and Constant kind,
    signature help's label offsets and the signature's
    `activeParameter`, and diagnostic tags only for clients that support
    them; `activeParameter: null` past the last parameter for clients
    with `noActiveParameterSupport`
  - the server exits when the client process that started it is gone,
    and accepts `--clientProcessId`
- **Versions.** This changelog, and a version per milestone.
- **Installing:** `cargo install --locked --path .`.

## 0.6.0

Milestone 6: one check for many questions, and the guardrails in tiers.
[Report](docs/history/MILESTONE-6.md).

- The worker stays between queries and keeps its last 4 checks: questions
  about the same text, with the same files on disk, are answered from one
  (`"cached": true` in the status).
- The guardrails run in three tiers, and mutations in parallel.

## 0.5.0

Milestone 5: answers in code being edited.
[Report](docs/history/MILESTONE-5.md).

- Answers from a repaired copy of the documents (`"repaired": true`), or
  from the last check that typed method bodies (`stale`), wherever they
  still hold.
- The edit-replay corpus, the oracle for both.

## 0.4.0

Milestone 4: diagnostics, document highlight, signature help and inlay
hints. [Report](docs/history/MILESTONE-4.md).

- Diagnostics, the same as `inko check`, from a full check in a worker of
  their own.
- `textDocument/documentHighlight`, `textDocument/signatureHelp` and
  `textDocument/inlayHint`.

## 0.3.0

Milestone 3: definition, type definition and references.
[Report](docs/history/MILESTONE-3.md).

- `textDocument/definition`, `textDocument/typeDefinition` and
  `textDocument/references`, by the compiler's ids.
- Before it, the standing rules became checks
  ([GUARDRAILS.md](docs/history/GUARDRAILS.md)).

## 0.2.0

Milestone 2: the language server. [Report](docs/history/MILESTONE-2.md).

- Standard LSP over stdio, replacing milestone 1's protocol: hover,
  completion after `.`, documents, UTF-8 and UTF-16 positions, a worker
  process per check with a timeout, debouncing and cancellation.

## 0.1.0

Milestone 1: a type service, with a custom protocol since replaced.
[Report](docs/history/MILESTONE-1.md).

- The type at a position, and the members after a `.`, from the upstream
  compiler with a small patch ([SPIKE.md](docs/history/SPIKE.md), the
  feasibility spike before it).
