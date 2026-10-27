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

## Versions

- Every milestone has its own version, `0.<milestone>.0` in Cargo.toml.
  Its first commit bumps the version and adds the version's entry to
  CHANGELOG.md, linking to the milestone's report (tests/guardrails.rs
  fails if the version has no entry). Examples in docs show the current
  version.

## Spec

- Cuttlebone follows LSP 3.18. Whenever the spec's standard requests,
  responses, error codes, capabilities or fields can solve a problem, use
  them instead of an extension. An extension is allowed only where the
  spec has no way to do it, and docs/lsp.md ("Extensions") must say why
  for each one.
- Send only what the client's capabilities say it supports. Every message
  the tests receive is checked against the 3.18 meta model and the
  client's capabilities (tests/common/spec.rs).

## Tests

- Every behaviour needs a test that fails when it breaks. Prove it with a
  mutation in scripts/mutate.py, and run it (`scripts/mutate.py WORD`).
- Behaviour-neutral changes must keep every mutation caught.

## Checks

Two tiers run locally, and a third in CI before every release. Run the
one the step calls for.

- **Every commit (locally):** `scripts/check.sh` (formatting, clippy,
  the tests with a sample of the edit-replay corpus, the patch, the
  release build), about a minute. The pre-commit hook runs it; set the
  hook up once with `scripts/install-hooks.sh`. On macOS, add the
  terminal under System Settings, Privacy & Security, Developer Tools, or
  each new test binary is scanned on its first run.
- **Never bypass the hook:** no `git commit --no-verify` (or `-n`), no
  disabling or editing the hook to get a commit through. If the hook
  fails, fix the cause, or make the check itself robust, in a commit that
  passes it.
- **Latency:** in the check of every commit, tests/latency.rs scales each
  latency limit by the machine's load (the last minute's load average per
  core, when it's over 1), so other processes can't fail a commit. The
  limits as written (`CUTTLEBONE_LATENCY_STRICT`) are advisory locally:
  the milestone tier runs them and reports a failure without failing.
  The release tier in CI holds every query to them, failing. Ratios
  between two measurements taken in the same run (a question after a
  cancel, a second lookup) aren't scaled in any tier. Don't loosen a
  written limit to get past a loaded machine.
- **Milestone reports (locally):** `scripts/check.sh --milestone [REV]`,
  where REV is the milestone's first commit (by default, the last commit
  that changed a milestone report). It adds the whole corpus, the latency
  limits as written (advisory), every test again without kept checks,
  the release tooling, and the mutations whose snippet, tests or checked
  command is in a file changed since REV, or whose snippet moved. Every
  one it selects must be caught. `scripts/check-selection.py` shows that
  this would have run every mutation guarding code the last three
  milestones changed.
- **Release (CI, on every tag push):** `.github/workflows/release.yml`
  runs the exhaustive tier: `scripts/check.sh --release` on macOS (the
  milestone tier's checks), `scripts/check.sh --latency` on a macOS
  runner of its own (the latency limits as written, failing, one test at
  a time), and every mutation, sharded across parallel jobs:
  `scripts/mutate.py --shard K/16` on 16 Linux runners, each running the
  unmutated control check first, and `--shard dist` (the release
  tooling's, which need macOS) on macOS. Publishing waits until all of
  them pass. Before tagging a milestone's release, run the workflow by
  hand on its last commit (workflow_dispatch: everything but publishing)
  as a dry run. Nothing local replaces this tier; `scripts/check.sh
  --exhaustive` runs it on one machine, if one is ever quiet enough.

`scripts/mutate.py --list [--since REV] [--shard K/N]` shows what would
run, and why. A new mutation goes in scripts/mutate.py, naming the test
file whose tests catch it.

## Reports

- Milestone and session reports go in docs/history/, linked from
  README.md. The milestone-report skill
  (.agents/skills/milestone-report/SKILL.md) has the steps.
