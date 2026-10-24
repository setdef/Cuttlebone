# Standing rules for Cuttlebone

Read README.md, docs/lsp.md and patches/README.md before changing anything.
Tests or scripts/check.sh enforce most of these rules. Don't weaken a check
to get past it.

## Upstream

- Never open issues or PRs against Inko, and never send it changes: it has a
  strict no-LLM policy.
- Never modify any checkout of Inko's own repository. The pinned release
  lives in `.upstream/` (scripts/upstream.sh), and changes to it go only
  through `patches/upstream.patch` (scripts/refresh-patch.sh).
- Keep the patch minimal. patches/README.md justifies every line, and its
  table must match the patch's line counts (tests/guardrails.rs).

## Names

- Never name anything after Inko (trademark). The binary, package, settings
  and notifications are `cuttlebone`. The `inko` language ID is fine.

## Design

- Cuttlebone is a general-purpose LSP server. No code may assume a
  particular editor or client, or name one outside docs/, editors/ and
  tests/ (tests/guardrails.rs).
- Layering (tests/guardrails.rs has the table):
  - `position`, `walker`, `check`, `edits`: nothing in the crate
  - `units`: `position` and `edits`
  - `query`: only those
  - `worker`: `query` and below
  - `lsp`: on top
  - `dump`: helpers for dump-types and the tests only
  The core never depends on `lsp` or `worker`. Import from the defining
  module (`crate::check::run`), never from the crate root.
- Queries have no side effects: no network, no `dep/` sync, no writes
  anywhere.
- No `unsafe`. Fix what clippy flags; allow a lint only with a comment
  saying why.
- `CUTTLEBONE_FAULT` exists only in builds with the `fault-injection`
  feature, which only the tests enable.
- Use repository-relative paths in code, scripts and docs, never paths
  from one machine.

## Tests

- Every behaviour needs a test that fails when it breaks. Prove it with a
  mutation in scripts/mutate.py.
- Behaviour-neutral changes must keep every mutation caught.

## Before committing

- Run `scripts/check.sh`. The pre-commit hook runs it; set the hook up once
  with `scripts/install-hooks.sh`.
- Run `scripts/check.sh --full` (adds the mutation runner, a few minutes)
  before any milestone report.

## Reports

- Milestone and session reports go in docs/history/, linked from
  README.md. The milestone-report skill
  (.agents/skills/milestone-report/SKILL.md) has the steps.
