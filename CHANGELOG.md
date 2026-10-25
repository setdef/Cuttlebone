# Changelog

Each milestone has its own version, `0.<milestone>.0`, reported by
`cuttlebone --version` and in `initialize`'s `serverInfo.version`. A
milestone's first commit bumps the version and adds its entry here, so a
build of work in progress already differs from the milestone before it;
the entry links to the milestone's report, which its last commit adds
(AGENTS.md, "Versions"). Milestones 1 to 6 were all built as 0.1.0; their
versions here were given afterwards.

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
