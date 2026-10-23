# Guardrails: making the standing rules fail loudly

**Done.** Earlier sessions followed their rules only because their prompts
said to. Now a test, clippy or a check script fails when one is broken,
and a pre-commit hook runs them. This session adds no features, and
behaviour didn't change: all 126 earlier tests still pass. Only their import paths and
lint allows changed. All 32 earlier mutations are still caught, along
with 10 new ones for the rules.

The rules themselves are in [AGENTS.md](../../AGENTS.md), the open
agent-instructions format, so any coding agent reads them. The steps for
writing a report like this one are an Agent Skill:
[.agents/skills/milestone-report/SKILL.md](../../.agents/skills/milestone-report/SKILL.md).

## What's enforced, and how

| Rule | Enforced by | Proven by the mutation |
|---|---|---|
| Layering: `position`, `walker`, `check` use nothing in the crate; `query` only those; `worker` adds `query`; `lsp` on top; `dump` is a side module. Nothing imports from the crate root. | `modules_use_only_the_layers_below_them` (tests/guardrails.rs) reads every `crate::` path in src/, including grouped `use crate::{a, b}` and inline paths. It names the file and the forbidden module, and fails for a module that isn't in its table. | "query uses lsp", "position uses walker" |
| No editor or client is named or assumed outside docs/, editors/ and tests/ | `no_code_names_an_editor_or_client` matches 19 editor and client names as whole words, in every file git tracks or would track. README.md and AGENTS.md count as docs, and paths into editors/ (as in .gitignore) are ignored. | "an editor named in code" |
| Nothing named after Inko | `nothing_is_named_after_inko`: the package, binaries and features in Cargo.toml, the VS Code extension's name, display name and publisher, and the Neovim config's file name | "a binary named after Inko" |
| Every patch line justified | `patches_readme_accounts_for_every_line_of_the_patch`: patches/README.md's table must give each patched file's exact added and removed line counts. A new line in the patch fails until the table, and so the text below it, is updated. | "an unjustified patch line" |
| The patch applies to the pinned release, and is what the build uses | scripts/check-patch.sh exports the `v0.21.1` tag from `.upstream/inko` into a temporary directory and runs `git apply --check` there. It also fails if `.upstream/inko`'s diff isn't byte-for-byte `patches/upstream.patch`. | "a patch that doesn't apply to the release" |
| No paths from one machine | `no_paths_from_one_machine`: no tracked file may contain a macOS home directory path, or the `Developer` folder in a home directory. AGENTS.md and `.agents/` may not contain any home-relative path (a tilde and a slash). The test file spells out the patterns. | "a path from one machine" |
| No `unsafe` | `#![forbid(unsafe_code)]` in src/lib.rs and both binaries, and `unsafe_code = "forbid"` in Cargo.toml's `[lints.rust]`. Removing one still leaves the other. | "unsafe code" |
| Clippy is clean, and nothing prints to stdout or stderr by accident | `[lints.clippy]`: `all` denied, plus `print_stdout` and `print_stderr`. scripts/check.sh runs clippy twice, with `-D warnings`: once on all targets, and once on the lib and binaries without the tests' features (the release feature set). | "printing to stdout in the library" |
| No fault injection in release builds | The `fault-injection` cargo feature. scripts/check-release.sh builds the release server and fails if `CUTTLEBONE_FAULT` is in it. | "fault injection in release builds" |
| Test helpers stay out of the server | src/dump.rs, `#[doc(hidden)]`. Nothing in the server uses it, so the linker drops it. scripts/check-release.sh fails if the release server contains the pinned std path that `dump::upstream_std` embeds. | none (see below) |
| Every behaviour's test is proven | scripts/mutate.py now exits with status 1 unless every mutation is caught, so `check.sh --full` fails too. A mutation can name `sh:<command>` as its catcher, for rules that clippy or a script enforces. Each such verdict quotes the first error line, so a failure for the wrong reason shows. | all 42 |
| Checks run before every commit | .githooks/pre-commit runs scripts/check.sh. `scripts/install-hooks.sh` sets `core.hooksPath` once per clone (README.md says so). I checked that it refuses a badly formatted commit. | none |

### What changed to make that possible

- **src/lib.rs split.** It now only declares modules. The rule as stated
  ("query uses only position and walker") didn't match the code: `query`
  imported `Target`, `run` and `project_root` from lib.rs. Those, the
  version check and the version constants moved to **src/check.rs**, a
  base module. AGENTS.md's layering lists it. `dump_types`'s and the tests'
  helpers (`check`, `Checked`, `cross_checks`, `format_expr`,
  `upstream_std`, `Status` and `Expr::status`) moved to **src/dump.rs**.
  The crate root no longer re-exports anything, so the tests import from
  the defining modules.
- **Fault injection** is behind `fault-injection`. The crate is its own
  dev-dependency with that feature on, so `cargo test` builds the library
  *and the binaries the tests run* with it. Plain and release builds lack
  it. `strings target/release/cuttlebone | grep -c CUTTLEBONE_FAULT` is 0;
  in the test build it's 1. docs/lsp.md says so.
- **`take_if`.** The three places in src/lsp.rs that checked
  `self.running.as_ref().is_some_and(..)` and then did
  `self.running.take().unwrap()` now use
  `if let Some(..) = self.running.take_if(..)` or `let .. else`. The
  `child.stdin.take().unwrap()` calls in worker.rs were already clear, so
  I left them.
- **Reports** moved to docs/history/, with links updated in README.md,
  docs/lsp.md and the reports themselves. MILESTONE-1.md's link to
  `docs/protocol.md` was already dead (milestone 2 deleted that file), and
  it still is.
- **patches/README.md's table** now gives the patch's own counts
  (`+16 -1`). The old notation was ambiguous: "+16, 1 changed" meant 16
  added lines, one of which replaced a removed line, but read as 17. The
  numbers didn't change.

## What the new lints flagged, and the fixes

Clippy's default groups, now denied, flagged nothing: the code was already
clean. The print lints were never actually enabled before. The two
`#[allow(clippy::print_*)]` attributes in the code were dormant. Enabling
them flagged 19 calls:

- **dump-types: 12 calls.** Printing is its job, and it isn't an LSP
  server. It's allowed for the whole binary, with a comment saying so.
- **Tests: 7 `eprintln!` calls** in tests/latency.rs (measurements),
  tests/lsp.rs and tests/editors.rs (skip notices). These are allowed per
  file, with a comment.
- The dormant allows on `cuttlebone`'s `main` (`--version`, usage errors,
  a worker's own failure) and on `lsp::log` (the server's one writer to
  stderr) are now live, and got comments. So did the existing
  `#[allow(deprecated)]` for `rootUri`/`rootPath`.

The new tests flagged:

- **Two editor names in src/:**
  - `/// Bytes, as in Neovim.` in position.rs became "LSP's `utf-8`
    position encoding"
  - "Clients such as VS Code's pass --stdio" in bin/cuttlebone.rs became
    "Some clients pass --stdio"
- **scripts/mutate.py** naming an editor in its own new mutation. The name
  is now split in the source.
- **The patch table's ambiguity** (above).

## How long it takes

Apple M1 Max, with the dependencies (the upstream compiler) already built.

| | |
|---|---|
| `scripts/check.sh`, nothing changed | 17 s |
| `scripts/check.sh` after a change to src/ (the usual pre-commit case) | 28 to 40 s |
| `scripts/check.sh --full` | 181 s, of which the 42 mutations are about 150 s |
| Mutation runner before this session (32 mutations) | 147 s |

The mutation runner takes more than 2 minutes, so it's behind `--full`.
AGENTS.md and the milestone-report skill require `--full` before any
milestone report. The release build in check.sh is about 19 s of the
28–40 s after a change. It's there because it's the only way to see what a
release build contains.

## What CI would take

There's no CI yet. It would need:

1. **LLVM 18.**
   - On macOS runners: `brew install llvm@18`. .cargo/config.toml already
     points there.
   - On Linux: apt.llvm.org's `llvm-18-dev` and `libpolly-18-dev`, with
     `LLVM_SYS_181_PREFIX=/usr/lib/llvm-18` (the config doesn't force its
     default).
2. **The pinned upstream.** `scripts/upstream.sh` clones `v0.21.1` from
   GitHub and applies the patch. Cache `.upstream/` and `target/` by
   `UPSTREAM_VERSION`, `Cargo.lock` and the patch's hash. A cold build
   compiles the upstream compiler and links LLVM, which is the slow part.
3. **The toolchain** from rust-toolchain.toml (1.94, with rustfmt and
   clippy).
4. **`scripts/check.sh` on every push, and `--full` nightly or on PRs.**
   The runner edits files in place, so it needs its own checkout.
5. **Optional, for coverage that's skipped today:**
   - `nvim` 0.11+ on `PATH` for tests/editors.rs
   - Inko 0.21.1 on `PATH` for the installed-`inko` test
   - VS Code and `xvfb-run` on Linux for `editors/check.sh`'s VS Code half
6. **Windows** isn't supported by the tests (`pgrep`, `kill -0`, signal
   6), so CI would be macOS and Linux only.

## Limits of these checks

- **The pre-commit hook checks the working tree**, not just what's staged.
  Unstaged changes can make it pass or fail a commit wrongly.
  `git commit --no-verify` skips it, as with any hook.
- **The layering test reads `crate::` paths.** It doesn't follow `super::`
  out of a top-level module, or a macro that expands to a crate path.
  Neither occurs today.
- **The editor list is finite** (19 names). A client it doesn't list would
  pass.
- **The no-local-paths test looks only for macOS home directories and a
  home directory's `Developer` folder.** A Linux `/home/<name>/` path would
  pass. Docs use `file:///home/me/...` as an example URI.
- **The patch check exports the tag from `.upstream/inko`'s own clone**
  rather than cloning GitHub again, so it works offline. If that clone
  were tampered with, the check wouldn't notice.
- **No mutation removes `dump`'s separation.** The server doesn't use it,
  and check-release.sh would catch `upstream_std` being linked in. I
  didn't add a mutation for it, because any mutation that makes the
  server use `dump` already breaks the layering test first.
- **Nothing can enforce "never modify Inko's own repository" or "never
  open issues there".** AGENTS.md states them.
- **The macOS linker prints a warning** about Homebrew's `libunwind` on
  every link. It's harmless, comes from the local LLVM install, and isn't
  turned into an error: clippy doesn't link, and tests don't use
  `-D warnings`.

## Verification

`scripts/check.sh --full` passed on this report's commit: formatting,
clippy (both feature sets), 133 tests, the patch against a fresh
`v0.21.1`, the release build, and 42 of 42 mutations caught.
