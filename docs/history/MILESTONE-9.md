# Milestone 9: cheaper answers while typing

**Done.** Parakeet replayed real typing against Cuttlebone 0.7.0 (its
session report, docs/history/SESSION-typing-replay.md in Parakeet's own
repository, "For Cuttlebone") and found two costs paid on nearly every
keystroke in code being edited. Both are fixed:

- **Questions with no answer are kept.** In code that doesn't parse, a
  second question about the same text checked again whenever nothing
  answered it: the check of the documents as they are, which finds
  nothing to answer from, was never kept. Now it is, in a list of its own
  that never pushes out a check holding compiler state. A second
  unanswered question on a std-sized project takes **9.7 to 11 ms**
  instead of **30 to 31 ms** (section 1).
- **Cancelling is cheap.** `$/cancelRequest` on a running check killed its
  worker, and every check the worker kept. In code being typed, the next
  question then checked the last good text again. Now the request is
  answered at once, and its check finishes in the worker, which keeps
  it. A question after a cancelled one takes **54 to 70 ms** instead of
  **128 to 154 ms**, against 27 to 45 ms with no cancel. A hung check is
  still killed (section 2).
- **On a replay of typing at 7 characters a second**, the query worker
  was restarted at least 119 to 123 times while a new method was typed into std's
  json.inko, and 10 to 16 of 382 requests got an answer. Now one worker
  serves all of it, and 105 to 132 are answered (section 3).

Every answer is the same as before: an answer from a kept check is the
one a new check gives, and a cancelled check's answer goes nowhere. The
milestone tier replays the whole edit-replay corpus with and without
kept checks, and runs every test without them (section 5).

**Tests:** 297 (26 unit, 258 integration, 13 latency), up from 286.
`scripts/mutate.py` has 271 mutations, 14 of them new and 5 with new
snippets. All are caught (section 5). **The patch didn't change.**

This milestone was requested: built by a coding agent with little or no
review.

## 1. Questions with no answer

### What caused it

docs/lsp.md said a repaired copy "is checked like the documents as they
are, and kept like them". That held, but it wasn't the whole story. A
question in code that doesn't parse takes up to three checks:

1. the documents as they are, which can't be type checked (`noHir`)
2. a repaired copy, with the lines in the way blank
3. the last good check's version

`Cache::with` kept a check only if `start_check` returned one with a
fingerprint, and for a check that found nothing to answer from
(`noHir`, or the compiler's own error), it returned only the reason.
So check 1 ran again for every question, whatever came after it. Checks
2 and 3 were kept, and answered from a lookup when they had an answer.

So both of Parakeet's guesses were partly right. The unanswered questions
were at places no repair answers: in the replays below, every one is in
the lines being typed, which the repaired copy has blank and the last
good check didn't have. The repaired copies didn't fail; the inlay hints
asked right after each such highlight came from the same repaired copy,
kept (`inlayHint fresh repaired cached`, 24 times in the features
replay). What wasn't remembered was the check of the documents
themselves, which every question pays first. A repaired copy whose own
check found nothing to answer from (an import of a module that doesn't
exist yet, say) wasn't kept either, for the same reason.

"The repaired copies tried again each time" are parses: which lines to
blank is worked out from the text each time (about 2 ms in all on the
std-sized project), and the copy's check is then a lookup.

### The fix (src/query.rs, src/check.rs)

- `start_check` returns a fingerprint with `noHir` and `compilerError`
  too, so `Cache::with` keeps them like any other check. A compiler panic
  still isn't kept.
- **What it read.** A check that doesn't parse never reaches the callback
  that says which modules it read. So its fingerprint
  (`Fingerprint::take_all`) covers every `.inko` file under std, `src/`,
  `test/` and `dep/`, as well as the directories and manifests every
  fingerprint has. A module fixed on disk is never answered from it. On
  the std-sized project that's about 1.9 ms to look at again.
- **Kept apart.** These hold no compiler state, only the text they
  checked and the fingerprint, so they're kept in a list of their own, up
  to 8 (`KEPT_FAILURES`), and don't count against the 4 checks. Otherwise
  typing in broken code, which makes a new one at every edit, would push
  out the last good check that every edit uses. Checks stopped by a
  declaration error (which were kept already) moved to that list too.
- **`cached` for no answer.** An `unavailable` answer used to be `cached`
  if the check of the documents as they are was kept, even if the
  repaired copy's or the last good version's check after it ran again.
  Now it's `cached` only if every check it took was kept.

### Measured

`a_second_question_in_code_that_doesnt_parse_is_a_lookup`: a std-sized
project (a file that imports all of std), with a `(` left open in a line
of its own, and a hover on that line, asked twice per edit, seven edits.
Neither a repaired copy nor the last good check answers there. The
worker's time (`elapsedMs`), medians:

| | first | second | second `cached` |
|---|---|---|---|
| before | 30.7, 33.1 ms | 30.3, 31.3 ms | never |
| after | 30.1 to 37.2 ms | 9.7 to 11.4 ms | always |

The limit: the second at most 0.6 of the first (measured 0.31 to 0.33).
A share rather than a time, because the machine was loaded (load
averages 25 to 390) throughout, and both scale with it. The 10 ms left
are lookups: the documents' kept failure (1.9 ms), working out the
repaired copy again (2 ms), its kept check (3 ms) and the last good one
(3.1 to 3.6 ms), each of which runs its guard over the file.

## 2. Cancelling

### What caused it

`Server::cancel` killed a running check's worker, as docs/lsp.md said.
The worker holds the kept checks, so they went with it. Parakeet measured
the next check at 77 ms instead of 39 (median, std).

I measured first whether a new process itself costs that, and it
doesn't: a hover on a new text in a fresh worker took 96.8 ms, and 96.5
ms in a worker that had checked before (`a check after a cancelled one`,
a version of the test below that edits to a new text each time; before
the fix, two runs). What costs is the kept checks. In code being typed,
each question uses one kept from before: the last good check, or a
repaired copy whose text is the same as an earlier one's. After a kill,
the next question checks those again.

### The fix (src/lsp.rs)

- **A queued request** (one waiting out the debounce, or behind another)
  never reached the worker, and still doesn't: it's taken out of the
  queue and answered `RequestCancelled`.
- **A running check** is answered `RequestCancelled` at once, and becomes
  `Abandoned`: its worker finishes it, keeps it as it would any check, and
  goes back to waiting for a job. Its answer is dropped, and its
  documents don't count as a good check, as when it was killed.
- **The next query waits** for it, but for at most `ABANDONED_GRACE`
  (250 ms) after it's ready to start. Then the worker is killed, as it
  always was, and a new one starts. A check that takes longer than that
  costs the wait and then the kill: at most 250 ms more than before, for
  checks that are already that slow.
- **The timeout** applies to a cancelled check as to a running one: it's
  killed at `checkTimeoutMs` from its start, with nothing waiting too.
  So a hung check is still killed, and the worker's own deadline
  (`checkTimeoutMs` plus 1 s) is unchanged.
- **Diagnostics** don't start while a cancelled check runs, as they don't
  while a query runs ("queries first"): it uses the machine as much.
- **`shutdown`** kills it, as it does a running check.

Why 250 ms: in the replays, checks of std's json.inko took 40 to 75 ms
(medians) and a std-sized project's 100 to 120 ms. So a cancelled check
nearly always finishes within the grace, and what it saves (one to three
checks, 30 to 300 ms) is more than the wait. No grace kill happened in
any replay. The grace is half of hover's latency limit (500 ms), so a
query after a cancel still meets it.

### Measured

`a_question_after_a_cancelled_one_is_no_slower`: the std-sized project,
its code made not to parse at each edit, so that each hover comes from
the repaired copy, which is the last good text, kept. Seven rounds of a
hover alone, and seven of a hover about an edit cancelled while it's
checked, then a hover about the next edit. The client's times, medians,
in four runs:

| | without a cancel | after a cancelled one | ratio |
|---|---|---|---|
| before | 26.7 to 45.2 ms | 122.7 to 153.7 ms | 3.4 to 4.7 |
| after | 27.1 to 40.9 ms | 54.0 to 70.1 ms | 1.7 to 2.0 |

After, a question after a cancel waits for the cancelled check to
finish, which is about one question's time, and is then a lookup. The
limit is 2.5 times a question without a cancel: the two, and a quarter
for noise.

## 3. A replay of typing

tests/latency.rs's `typing_replay`, run with `CUTTLEBONE_TYPING_REPLAY`
set, types into a file over LSP, one character every 143 ms (about 7 a
second), with the real server. After each keystroke it asks what an
editor would: document highlights and the definition at the cursor,
inlay hints for the 50 lines around it, and completion after a `.`. Each
edit first cancels every request without an answer yet, as a client
that cancels what an edit made outdated does (VS Code's, and Parakeet's
since its replay). Three typings:

- **std json: a new method**: 5 lines (125 characters) typed into
  `Json`, after `query`. The code doesn't parse for most of it.
- **std json: a line in a method**: `to_pretty_string`'s body deleted and
  typed again.
- **features: a line in a method**: `counter.add(amount: 1, times: count)`
  in tests/fixtures/features, deleted and typed again.

"Before" is 0051c81's server (0.9.0 with milestone 8's code), "after" is
this milestone's, run alternately, three times each, Apple M1 Max, 64 GB,
`cargo test` builds, at load averages of 95 to 155. The new method's
"after" lines were lost from those runs' log, so it has two runs after
(and five before) from a second set. Worker processes are those seen
running at a keystroke, so the counts before are at least that:

| Replay | requests | answered: before | after | cancelled: before | after | worker processes: before | after |
|---|---|---|---|---|---|---|---|
| std json: a new method | 382 | 10 to 16 | 105 to 132 | 366 to 372 | 250 to 277 | 119 to 123 | 1 |
| std json: a line in a method | 158 | 44 to 66 | 64 to 66 | 92 to 114 | 92 to 94 | 31 to 39 | 1 |
| features: a line in a method | 112 | 42 to 44 | 112 | 68 to 70 | 0 | 24 to 25 | 1 |

What it says:

- **Before, typing a new method was a spiral.** Once the code stopped
  parsing, each keystroke's highlight was still being checked when the
  next keystroke cancelled it. That killed the worker, so the next one
  had all three checks to do again, which took longer than a keystroke:
  for 18 s, almost nothing was answered.
- **After, one worker serves every replay**, and none of its checks was
  killed for waiting. In the features replay, everything is answered
  before the next keystroke, so nothing needs cancelling, and the 24
  definitions asked where the highlight just had no answer are lookups
  (`unavailable`, `cached`), where each was a check before.
- **In the new method, fix 1 doesn't show.** The highlight is the first
  question about each text, so it's checked (70 ms in the worker, the
  documents as they are and a new repaired copy), and the definition and
  hints behind it are cancelled by the next keystroke before they start.
  Those cancels cost nothing now.
- **Latency of what's answered** didn't change much (medians 91 to 159 ms
  before, 90 to 143 after, from the request to its response, debounce
  included).
  Before, few requests were answered at all in the slow replays, so their
  times describe the lucky ones.

The worker's total time isn't comparable between the two: before, the
checks that were killed never reported theirs.

## 4. Answers stay the same

- **Kept failures** answer what a new check would: the same text and
  files give the same failure. The corpus runs `query::ask_with` with the
  worker's cache, so in the milestone tier it replays every scenario with
  failures kept, and again with no kept checks (`CUTTLEBONE_NO_CACHE`),
  and every answer is held to the final fresh one both times.
- **Cancelling** changes no answer: a cancelled request was answered
  `RequestCancelled` before and is now, and what a cancelled check kept
  answers later questions as a new check would. A cancelled check isn't
  a good check for later stale answers, as before.

## 5. Tests and mutations

| File | What |
|---|---|
| tests/cache.rs (+4) | A check with nothing to answer from is kept, and answers as a new check does; a module fixed on disk (rewritten in place, so its directory doesn't change) is checked again; failures don't push out checks, and at most `KEPT_FAILURES` are kept; an answer with no result is `cached` only if no check ran for it, past a repaired copy and past the last good check. |
| tests/lsp.rs (+4, 1 changed) | A cancelled check's request is answered at once while its worker runs on, and a query waits `ABANDONED_GRACE` for a hung one, then has it killed; a cancelled hung check is killed at the timeout with nothing waiting; a cancelled check is kept by its worker (the same question is `cached`, in the same worker); diagnostics wait for a cancelled check; `shutdown` kills one. Changed: "cancelling a running check kills it" is now the first of these. |
| tests/latency.rs (+3) | A question after a cancelled one, at most 2.5 times one without; a second unanswered question in code that doesn't parse, at most 0.6 of the first; the typing replay, which prints what it measures and asserts nothing about time. |

The mutations (`scripts/mutate.py`), all caught when run on their own
during the milestone:

- **Kept failures (6):** failures not kept, their fingerprints without
  source files, kept with the checks, kept without a bound, and an
  unanswered question marked `cached` though a repaired copy, or the last
  good check, was checked again.
- **Cancelling (8):** cancelling kills the check (the old behaviour), a
  waiting query never kills it, or kills it at once, no wake-up at the
  end of the grace, cancelled checks never time out, diagnostics start
  beside one, its worker not kept afterwards, `shutdown` leaving it.
- **Moved (5):** kept checks found by target but not text, fingerprints
  leaving out directories, the cache keeping more than its capacity, kept
  checks not moved to the end when used, and the queries' worker not
  kept (its snippet now matched the cancelled check's too).

**The tiers.** TIERS-PENDING

## Every patch line added, and why

None: the patch is unchanged, and patches/README.md with it.

## What clients should know

1. **Cancel freely.** A request cancelled while it waits out the debounce
   never reached a worker, and still doesn't. One cancelled while it's
   checked is answered `RequestCancelled` at once, and costs what's left
   of its check, at most 250 ms, instead of the worker's kept checks. A
   client that cancels what each edit made outdated, as VS Code's does,
   loses nothing by it.
2. **Ask again.** In code that doesn't parse, asking the same text again
   (say, a definition where the highlight just had no answer) is a lookup,
   about 10 ms on a std-sized project, whatever the answer. Parakeet's
   choice to skip that prefetch still saves the lookup, but no longer a
   check.
3. **`cached` on an `unavailable` answer** now means no check ran for it.
   Before, it could say `true` when the repaired copy's check ran again.
4. **The first question about each new text** still costs its checks:
   in code that doesn't parse, the documents as they are (about 20 ms on a
   std-sized project, which parses every module before it fails) and a
   repaired copy of the new text. Asking a cheap question first, or once
   typing pauses, is still worth it.

## Decisions you may want to revisit

- **The grace is fixed at 250 ms**, not a setting. A project whose checks
  take seconds pays up to 250 ms after a cancel before the old kill. A
  setting, or a grace taken from the worker's recent check times, would
  fit such projects better; I had no such project to measure.
- **A cancelled check that finishes isn't a good check** for later stale
  answers, as when it was killed. Counting it would give stale answers
  more often, but its documents are ones the client stopped asking about,
  and it would make what's answered depend on timing.
- **Failures' fingerprints cover every source file.** That's about 1.9 ms
  per lookup on std, against the ~150 paths a check's fingerprint has. A
  patch could report which modules a failed parse reached, but the patch
  has no line for that now, and the cost is small.
- **Working out the repaired copy again** on every question (about 2 ms)
  could be kept with the copy's check. I left it: it's a fifth of a lookup
  and keying it on the documents and the last good version would add
  state to the cache for little.

## Remaining unknowns

1. **A loaded machine.** Another session kept the load average between 25
   and 390 throughout, so every time here has noise in it, and the
   replays' counts of answered and cancelled requests depend on how fast
   checks run. I ran before and after alternately, so both saw the same
   load, and set the latency limits as ratios for that reason.
2. **Whether IntelliJ's client sees the same.** The replay is mine, over
   LSP, with the cancelling Parakeet now does. It isn't Parakeet's
   replay in an IDE; Parakeet's session lists what to check by hand.
3. **Slow checks and the grace.** No replay reached the grace. A project
   whose checks take longer than 250 ms would show what the kill after
   it costs; none of the corpus's does.
4. **From earlier milestones, still open:** Windows, and packaging beyond
   `cargo install`.
