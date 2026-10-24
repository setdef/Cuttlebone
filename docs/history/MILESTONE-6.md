# Milestone 6: one check for many questions, and faster guardrails

**Done.** Two problems, both about time:

- **Every query ran its own type check**, about 100 ms on a std-sized
  project, even when nothing had changed. A worker now stays between
  queries and keeps its last 4 checks. Any question about the same text,
  with the same files on disk, is answered from one of them. **1,000
  definition requests sent at once on a std-sized project take 0.7 s,
  against 123.5 s before** (release builds). The same 2,051 definitions
  Find Usages might send across 132 of std's files take 7.8 s, against
  142.5 s.
- **The guardrails got slow.** They're now three tiers, written into
  AGENTS.md. **Every commit: about a minute (56 to 96 s), from 320 s.**
  Milestone and exhaustive tiers: about 15 minutes each, from 56, of which
  the mutations take 12.

Answers are the same with and without kept checks: every test, and the
whole edit-replay corpus, pass in both modes (below).

**Tests:** 259 (24 unit tests, 225 integration tests in one binary, 10
latency tests), up from 242. `scripts/mutate.py` has 197 mutations, 23 of
them new and 9 with new snippets, and every one is caught. **The patch didn't
change.**

## 1. Checks kept between questions

### Where the results live: a worker that stays

The compiler hands the typed HIR and its type database to a callback, and
drops both when the callback returns (the `check_with` hook,
patches/README.md). Everything an answer needs comes from those two:
types are formatted, members looked up and methods resolved through the
database. So I considered two places for a kept result:

- **Results sent back to the server.** The server would need every answer
  any later question could ask for, computed in advance and serialized:
  every expression's type, every reference, and completion's members for
  every receiver, which the worker only works out by asking the database.
  It would mean reimplementing answers as a precomputed index, the size of
  the database. I didn't build it.
- **A worker that keeps them** (built). Each check runs on a thread of its
  own and stays inside the callback, answering the questions sent to it,
  until the cache lets it go (`query::Cache`, `Checked`). The worker
  process stays between queries: the server sends each job as a line of
  JSON and reads the answer as a line, one job at a time.

Its timeout and kill behaviour are unchanged: the server kills the worker
when a query times out, is cancelled, or is superseded by an edit, and
starts a new one for the next query, which loses the kept checks with it.
A job that runs for `checkTimeoutMs` plus 1 s makes the worker exit by
itself, as before, and so does its stdin closing, so a worker never
outlives a killed server for longer. Diagnostics checks still each get a
worker of their own, which exits after the check.

Measured (Apple M1 Max, 64 GB; release build unless noted):

| | |
|---|---|
| A question answered from a kept check, in the worker (debug build, `cargo test`) | 0.45 ms, of which 0.3 ms is looking again at the 150 files and directories a std-sized check read |
| The same, over LSP, 1,000 at once | 0.7 ms each |
| What a worker adds to a check, over LSP (tests/latency.rs, debug) | 0 to 4 ms; it was 13 to 20 ms with a process per check (milestone 2) |
| A hover on a std-sized project, each a new check (tests/latency.rs, debug) | 90 ms; milestone 5: 112 ms |

### The key: the exact text, and what's on disk

A kept check answers a question when everything the question would check
is the same:

- **The target**: the file (or the project, for references, which check
  all of it), the project root, and the standard library.
- **The exact text of every open document**: the overlay the check used,
  compared in full, not by hash. Any edit, opening or closing a document
  makes it a different check. milestone 5's versions (`edits::Snapshots`)
  are what the last good check's documents are, so a stale answer's check
  is found by those documents' text in the same way.
- **The files on disk** (`check::Fingerprint`): each module the check read,
  the manifests, every directory under std, `src/`, `test/` and `dep/`,
  and the project directory. A file created or deleted changes its
  directory. Each is recorded by size, modification time, inode change
  time and inode. Before each answer the worker looks at all of them
  again, and checks again if any differs.

This covers all three kinds of answer, since each comes from a check of
its own text: the documents as they are, a repaired copy, and the last good
check's version. A burst of questions in broken code costs one fresh check
that fails at once (`noHir` takes about a millisecond), and lookups in the
kept repaired or stale check.

**Which kind an answer is doesn't change.** A kept check's answer goes
through the same pipeline as a new one: the same rules for which request
may answer from which kind of check (`allows`), the same guard on units
(`Guard`), the same mapping of positions and ranges. `cached: true` in the
status is the only addition, for an answer whose check (the one its kind
names) was kept from an earlier question.

**Diagnostics checks aren't reused.** A diagnostics check lowers to MIR,
which consumes the typed HIR the queries would need, in the same callback.
Keeping it would mean stopping before MIR (another check for the
diagnostics) or a patch to the compiler, so it isn't cheaper than a
second check of the same text. It still counts as a good check for stale
answers (milestone 5), whose own check of those documents is then kept.

### Changes on disk

Cuttlebone doesn't watch files, and `workspace/didChangeWatchedFiles`
depends on the client sending it (and on its glob patterns). The
fingerprint needs neither: a file changed outside the editor (a `git
checkout`, a formatter, another editor's save) is never answered from an
older check. tests/cache.rs changes an imported module that isn't open,
on disk, to text of the same length, and checks that the next answer is a
new check with the new type, in-process and over LSP; and that a file
created in a subdirectory, and a file deleted, are noticed.

Two races, and what closes them:

- **A file written while a check runs** may have been read before the
  write; its stamp would then describe text the check never saw. A check
  is kept only if every path it read last changed before it started
  (`Fingerprint::settled`). The inode change time can't be set by tools,
  so a tool that sets the modification time back is still noticed (a test
  does this).
- **A file system that records whole seconds** can't tell a change during
  the same second. A time with no sub-second part gets a 2 s margin
  (FAT's times are two seconds apart). APFS and ext4 record nanoseconds.

### Memory

The worker keeps at most 4 checks, the least recently used going first.
Measured with a std-sized file, editing it between hovers so each is a new
check: the worker holds 48 MB with one check kept, 81, 113 and 146 MB with
two to four, and levels off at 182 MB once a fifth pushes the oldest out.
A check of all of std (references) is larger. Not kept at all:

- a check that didn't parse (`noHir`, about a millisecond anyway): with no
  modules, its fingerprint would have nothing to cover
- completion's copy with its placeholder, asked about once: it would only
  push out a check that answers more
- a check the compiler panicked in. A panic while answering from a kept
  check (which is only read) leaves it as it was: the same question is
  `compilerPanic` again, as from a new check. No input is known to cause
  one, so only the first case has a test.

docs/lsp.md, "Kept checks", documents all of it.

## 2. Many queries at once

Measured over LSP from a client sending requests as fast as it can, then
reading the answers (small scripts outside the repository), release
builds:

| | Milestone 5 | Now |
|---|---|---|
| 1,000 definition requests at once, std-sized project, one file | 123.5 s (123.5 ms each) | 0.71 s, one check |
| 2,051 definitions at every `size` in std's 132 files that have one, std as the project, one at a time | 142.5 s | 7.8 s, 132 checks |

tests/latency.rs (`a_burst_of_definitions_costs_one_check`) sends exactly
the first burst, under `cargo test`, and requires that exactly one answer
needed a check, that every answer is fresh, and that the burst takes under
**3 s**. It takes 1.0 to 1.1 s there (the debug build of Cuttlebone's own
code, which is slower than release's 0.7 s, with nothing else running);
3 s leaves room for a slower machine, and is "a few seconds" for 1,000.
The test runs alone among the latency tests, as the others that load the
machine do: with them, it took up to 4 s, and its load made the worker's
cost test fail.

### Across files: tried, measured, not kept

A burst across files still costs a check per file. One check of the
project could answer for every file in it, but only where it gives what the
file's own check would. The rule for that turned out to be narrow: a
file's own check has the modules it imports (and `std.init`), and any
other module may implement a trait for one of their types, or add methods
to it, which a check of the project then sees and the file's doesn't. So a
project check may answer for a file only if no module outside the file's
imports does either. I built that rule, and a check of the whole project
after misses in three files. On the std burst above it answered 45 of the
132 files, and took 8.9 s instead of 7.8 s: std implements most traits in
the trait's module (`std.fmt` implements `Format` for `Array`), so most
files fail the rule, and the project check (bigger, and pushed out by the
files' own) cost more than it saved. I reverted it.

## 3. The guardrails, in tiers

AGENTS.md, "Checks":

- **Every commit:** `scripts/check.sh`.
- **Milestone reports:** `scripts/check.sh --milestone REV`.
- **Exhaustive:** `scripts/check.sh --exhaustive`, only when the mutation
  runner or shared test infrastructure changes.

### Every commit

Measured after a one-line change to src/query.rs:

| Step | Milestone 5's code | Now |
|---|---|---|
| clippy, all targets | 26 s | 8 to 14 s |
| tests | 257 s | 49 to 65 s |
| the rest | 37 s | 11 to 19 s |
| **total** | **320 s** | **68 to 96 s** |

Three changes did it:

- **macOS scans each new binary the first time it runs**, about 3.3 s for
  a test binary (0.08 s afterwards). Every commit relinked 33 binaries,
  so most of the 257 s was the scan, not the tests (they took 38 s on a
  second run). The integration tests are now one binary (tests/main.rs
  declares each tests/*.rs file a module; `autotests = false`), beside
  tests/latency.rs, which stays apart so that nothing loads the machine
  under it, and the two programs have no test binaries of their own. One
  link instead of 30, and the tests run in parallel. Adding the terminal
  under System Settings, Privacy & Security, Developer Tools turns the
  scan off; AGENTS.md says so.
- **A sample of the edit-replay corpus.** Every edit is still applied, but
  six of the sixteen scenarios replay, one of each kind of edit (including
  one in an imported module, and one in std), two of them asking after
  every eighth edit only. 3.1 s, with fresh, repaired and stale answers
  checked, and coverage floors of its own. The whole corpus
  (`CUTTLEBONE_CORPUS_FULL=1`) runs in the milestone tier, and for every
  mutation.
- **Kept checks** make the whole corpus itself faster: 44 s, from about
  105 s, since stale and repaired checks repeat across edits.

### Milestone reports

`scripts/check.sh --milestone REV` adds the whole corpus, every test again
without kept checks, `scripts/check-selection.py`, and the mutations a
change since REV could affect (`scripts/mutate.py --since REV`): those
whose snippet is in a changed file, whose tests are, or whose checked
command names one, and any whose snippet isn't found any more (it moved).

**The selection is safe for the last three milestones.**
`scripts/check-selection.py` takes each milestone's own mutations and diff,
and finds the mutations that guard what it changed: those whose snippet is
on a line it added or changed, and those it added or edited. It then checks
that the selection by file includes all of them:

```
milestone 3 (96855a1..a928962): 69 mutations, 28 guard changed code, 60 selected by file, 0 of those missed
milestone 4 (a928962..4c4322a): 122 mutations, 61 guard changed code, 121 selected by file, 0 of those missed
milestone 5 (4c4322a..a19f986): 174 mutations, 63 guard changed code, 173 selected by file, 0 of those missed
```

It also shows the selection doesn't save much: each milestone changed
src/query.rs or src/lsp.rs, where most mutations are. The tier is fast
because the runner is.

### Exhaustive, and the runner

`scripts/mutate.py` used to apply a mutation, build it and run its tests in
the tree, and restore it, one at a time: 56 minutes for milestone 5's 174.
Running the tests of one mutation while the next was built, in one tree,
still took 46 minutes for 197: the serial part was the build (6 to 11 s
with tests running beside it) and the new test binary's first run (4.5 to
6 s, macOS again). So now:

- **Several copies of the tree build at once** (target/mutate/trees; a
  third of the cores, at most 3). Each is a copy of the files git tracks,
  with `.upstream` linked to the tree's own and a copy-on-write clone of
  the tree's build, so only Cuttlebone is built again: 12 s for a copy's
  first build, then 3 to 4 s a mutation. The clone took 140 s at first:
  `target/debug/deps` had 753,000 files, Cuttlebone's own leftovers from
  every build and from the 30 test binaries that no longer exist. Cleaned,
  it takes 4 s.
- **A pool runs the tests** (half the cores, at most 5), from copies of each
  mutation's test binary and `cuttlebone`. Each mutation's tests get a
  directory of their own for the files they make
  (`CUTTLEBONE_TEST_TMPDIR`) and run its copy of the server
  (`CUTTLEBONE_TEST_BIN`); tests/common reads both, falling back to
  cargo's.
- **In the tree itself:** `sh:` commands, and tests that read the tree with
  git (guardrails, citations; in a copy, which isn't a repository, they
  found nothing to check, and a mutation went uncaught, so they moved).
  The mutation of .upstream, which every copy links to, and tests/latency.rs
  run last, alone.
- **Each mutation runs exactly the tests its entry names**, as before:
  the runner lists the binary's tests and runs those in the named file
  whose names contain the filter, with `--exact`. A filter that matches no
  test is an error. My first version prefixed the filter with the module
  (`lsp::plain_text`), which matched nothing for filters that aren't the
  start of a test's name, and three mutations came out "not caught"; the
  listing makes that impossible.

Not catching fewer: every mutation runs the same tests it did before, with
the whole corpus, and all 197 are caught. Each verdict names the tests that
failed, which are in the mutation's own test file by construction; the
latency tests, the only ones whose limits a loaded machine could break, run
alone. I didn't run the old serial runner on the final set to compare
verdict by verdict (it would take about an hour); the first parallel run
caught 194 of 197, and the three it missed were snippets that had moved
and a replacement that no longer compiled, all fixed.

Timings, on 8a90266 (the code of the commit with this report). Both tiers
passed there; the milestone tier noted that the runner and shared test
infrastructure changed, so the exhaustive tier was required too. This
milestone changed nearly every file, so its milestone tier selected 178 of
197 and took as long as the exhaustive one; going by the last three
milestones (above), a typical one would too.

| Tier | Before | Now |
|---|---|---|
| Every commit | 320 s after a change to one source file (milestone 5: about 6 minutes) | 56 to 96 s |
| Milestone (`--milestone a19f986`) | 56 minutes (milestone 5's `--full`) | 924 s: the same checks, and the 178 mutations this milestone's changes select, 734 s |
| Exhaustive | the same 56 minutes | 891 s: the per-commit checks 53 s, the whole corpus 44 s, every test without kept checks 86 s, all 197 mutations 704 s |

## 4. Tests and mutations

### The same answers with and without kept checks

With `CUTTLEBONE_NO_CACHE` set (test builds only, like `CUTTLEBONE_FAULT`;
scripts/check-release.sh checks that release builds don't have it), a
worker keeps nothing, so every question gets a check of its own, as in
milestone 5. The milestone tier runs every test, end-to-end and corpus
included, in both modes. Both passed, on the commit with this report:
the whole corpus has the same counts and no wrong answers either way. The
existing latency tests now run with kept checks off, so that each still
measures a check: after an edit, a query's text is always new. The tests
about kept checks (tests/cache.rs over LSP, and the burst) keep them on in
either mode.

The corpus prints a table per scenario and mode: how many answers were
fresh, repaired, stale, missing, differing at the end, and wrong. Run with
and without kept checks, on the final code, the tables are identical,
every count in all 16 scenarios and 3 modes (44 s and 65 s).

### New tests

| File | What |
|---|---|
| tests/cache.rs (15) | Questions about the same text share one check, with the same answers, source and `typed` as a new check, for hover, definition, type definition, highlights and hints, and references their own; an edit is a different check, and undoing it finds the old one; a file changed on disk (to text of the same length), deleted, or created in a subdirectory is never answered from a kept check; a file changed during a check, or with times the check can't trust, isn't kept; the change time catches a modification time set back; the bound and least-recently-used order; completion's copies, and checks that panicked, aren't kept; repaired and stale answers from kept checks; over LSP: `cached` in the status, an edit outside the editor, the tests' switch, a worker that died while waiting is replaced, `shutdown` stops the waiting worker. |
| tests/latency.rs (+1) | The burst. |
| src/check.rs (1) | A time in whole seconds gets the margin. |
| tests/replay.rs | The sample, with its own floors; the whole corpus with `CUTTLEBONE_CORPUS_FULL`. |

Three tests changed because a worker now waits between queries: one counted
no workers after a superseded request (it now checks that the waiting
worker wasn't killed, which a running check would have been), and two
counted the diagnostics worker alone.

### The mutations

All 197 are caught: milestone 5's 174, 9 of them with new snippets because
the code they break moved, and 23 new:

- **Kept checks:** found by target without the text; used whatever changed
  on disk; fingerprints that compare nothing, or leave out directories or
  the change time; kept while what they read changed; the start time or
  the coarse margin ignored; more than the capacity kept; not moved to the
  end when used; completion's copies kept; repaired or stale answers
  marked by the fresh check; no `cached` status.
- **The worker:** the queries' worker not kept; workers keeping nothing; a
  dead waiting worker sent a job; no deadline between jobs; `shutdown`
  leaving the waiting worker; the tests' switch ignored, or in release
  builds.
- **The tiers:** the whole corpus replaying the sample; the selection
  ignoring changed snippets' files.

Removed, because the code it broke was behaviour-neutral or unreachable:
a check that panicked in a lookup being dropped (no input reaches it; the
code is gone, and the check is kept, as above), and an `is_alive` check
before sending a job, which `Worker::run` already makes.

## How the cache stays correct

1. **An answer comes from a check of exactly the text the question is
   about.** The key compares every open document's text in full, and the
   target. Nothing about the answer is computed differently: the same
   `inspect`, guard and mapping run, in the kept check's thread.
2. **Nothing on disk changed since.** Every file and directory the check
   could have read is looked at again before each answer, and a check
   whose files may have changed while it ran is never kept.
3. **Nothing is half-done.** A check is kept only after it typed the HIR,
   and is only read afterwards.
4. **The tests run both ways.** Every test, and the whole corpus, pass with
   and without kept checks, and tests/cache.rs compares kept answers with
   new ones directly.

## Every patch line added, and why

None. patches/upstream.patch is unchanged.

## What clients should know

1. **Bursts are cheap.** Questions about text that hasn't changed cost a
   lookup, under a millisecond in the worker, after the first check: ask
   about every occurrence a search finds, every visible line's hover, or
   the same position again. A burst across many files costs a check per
   file (about 30 to 100 ms each, then lookups), so sending a file's
   requests together is better than interleaving many files.
2. **For find usages, `textDocument/references`** is still the one request
   that answers for the whole project: one check of all of it (about
   0.3 s for std), with every use by the compiler's ids.
3. **An edit makes the next question a new check**, as before: kept checks
   help questions about unchanged text, not typing.
4. **`cached: true`** in `cuttlebone/status` says an answer's check was
   kept. Its status is otherwise exactly what a new check would give, and
   clients can ignore the field.
5. **Changes on disk are noticed without `workspace/didChangeWatchedFiles`.**
   Clients don't need to send it.
6. **Memory:** the queries' worker stays, with up to about 180 MB for a
   std-sized project (more for a whole-project check). It's freed when the
   server shuts down, or kills the worker for a timeout or a cancellation.
7. **Cancelling a running query kills the worker**, and the next question
   checks again. A client that cancels requests it no longer needs during a
   burst should expect that.

## Decisions you may want to revisit

- **4 kept checks.** Enough for the documents as they are, a repaired
  copy, the last good check's version and a project check. More would
  serve clients that jump between many files, at about 33 MB each.
- **Looking at every file before every answer.** 0.3 ms of a 0.7 ms answer
  for a std-sized check. Trusting a check for a few milliseconds after the
  last look would halve it, but an edit outside the editor in that window
  would be answered from the old check.
- **Cancellation kills the worker** with its checks. A job that's only a
  lookup could be left to finish instead; the timeout and kill behaviour
  were to stay unchanged.
- **The selection is by file.** It's safe for the last three milestones,
  but selects nearly everything when a milestone changes src/query.rs or
  src/lsp.rs. Selecting by changed lines would run fewer, and the proof
  above doesn't show that would be safe.
- **The latency tests run kept checks off**, measuring the check after an
  edit; the burst test measures the kept path.

## Remaining unknowns

1. **Bursts across many files** cost a check per file (above).
2. **A panic while answering from a kept check** has no input known to
   cause it, so the code that answers it is untested by a real panic.
3. **File systems with coarse times** are handled by a margin; only APFS
   was measured.
4. **Projects much bigger than std:** each kept check grows with the
   modules it checked.
5. **From earlier milestones, still open:** the compiler's passes after a
   declaration error, lone `\r` line endings, Windows, and packaging.
