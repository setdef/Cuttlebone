# Milestone 5: answers in code being edited

**Done, except one investigation that ended in a write-up.** Cuttlebone used
to answer nothing in a file with a syntax error, and nothing in any method
body anywhere after a declaration error. It now answers in broken code
from two other checks, and never with a wrong answer on the corpus built
to catch them:

- **A repaired copy** (new, `fresh` with `"repaired": true`): the lines in
  the way are blank in a copy of the documents, which is checked instead.
- **The last good check** (`stale`, which milestone 1 reserved): the
  documents of each file's last check that typed method bodies, with every
  edit since, positions mapped back and ranges forward.

Both answer only where the answer holds for the documents as they are, by
one rule (units, below). Completion may use a repaired copy but never an
older check; references use neither; diagnostics always come from the
documents as they are.

On the corpus, **the questions asked mid-edit that get an answer went from
31% to 56% for hover** (and similarly for the other position requests),
and **none of the 21,873 answers from a repaired copy or an older check
is wrong.**

Task 3's second part, letting the compiler's passes go on past a
declaration error, **isn't done**: it needs more than a small patch (see
[The compiler's passes after a declaration error](#the-compilers-passes-after-a-declaration-error)).
**The patch didn't grow.**

**Tests:** 242, up from 212, all passing under plain `cargo test`.
`scripts/mutate.py` has 174 mutations, 52 of them new, and every one is
caught. `scripts/check.sh --full` passed on the commit with this report,
in 3,355 s (56 minutes; milestone 4's took 1,000 s for 122 mutations: about 20 of the new ones run the corpus, at about 100 s each).

## 1. The oracle: the edit-replay corpus

tests/replay.rs replays edits on real files and asks every query after
each edit, through `query::ask_with`, the same function the worker runs
(every single query is now a batch of one: `query::ask` answers many
questions with one check, which is what makes the corpus affordable).

**The edits.** Sixteen scenarios, on fixtures and on std's own modules and
tests, each generated from the file's text so that it ends where it
began:

| Scenario | Edits |
|---|---|
| features: retype a `let`; retype a call; navigation: retype a `let`; std test_set: retype a `let`; std test_int: retype a closure | delete a statement, then type it character by character |
| features: delete and retype lines; std range: the same; std test_array: delete and retype pushes | delete lines one at a time, last first, then type them back one line at a time |
| features: retype a method; std set: the same; navigation: retype a method in an import | delete a method, type its signature character by character, then its body line by line |
| features: break an argument's type, a field's type; std set: break a return type; navigation: break an import, break a return type in an import | delete a name in a declaration character by character, then type it again |

Two scenarios (test_int, test_array) are there because an earlier array
gets its type from the statement being typed; the first version of the
repaired rule gave wrong answers there (below). Three scenarios edit a
module the queried file imports.

**The questions.** After each edit, up to 40 positions where a name starts
(all of those within three lines of the edit, and the rest spread over the
file): hover, definition, type definition and highlight at each, and inlay
hints for each position's line. Completion after a `.` and signature help
after a `(` or `, ` at two positions each, after every third edit (each is
a check of its own). Only positions whose text is the same at the end of
the scenario are asked about, since only those can be compared.

**The comparison.** Each answer's ranges are mapped to the final text
through the changes between the two (`edits::Changes`), and compared with
the fresh answer at the mapped position in the final text. Lists
(highlights, hints) are compared on the text that exists in both. The
server's memory of older checks is simulated with the server's own
`Snapshots`: a check when the files open, then each step's documents kept
when its check typed method bodies.

**Wrong** means an answer that isn't from the documents as they are
(`repaired` or `stale`) and disagrees with the final fresh one. The test
fails on any. A **fresh** answer can disagree too, because the code meant
something else then: after a `let` is deleted, its variable is undefined
and a later use has no definition. Those are counted apart ("fresh,
differs at the end"); I looked at samples of each kind and they're all of
that sort. They're a property of the code, not of Cuttlebone, so they
don't fail the test.

The corpus runs every scenario three times, in parallel: as the server
does, without older checks (as after the server starts, or for a file that
was never good), and without repaired copies. It takes about 105 s under
`cargo test`, and fails if any mode has a wrong answer or answers fewer
questions than measured (`COVERAGE`, per request and mode).

## 2. Results

Questions asked mid-edit, and how many got an answer. "Before" is what
milestone 4 answered: the fresh answers, which are unchanged.

| Request | Asked | Before | After | Without older checks | Without repaired copies | Wrong |
|---|---|---|---|---|---|---|
| hover | 19,902 | 6,100 (31%) | 11,068 (56%) | 8,819 (44%) | 9,088 (46%) | 0 |
| definition | 19,902 | 6,421 (32%) | 11,475 (58%) | 9,190 (46%) | 9,409 (47%) | 0 |
| typeDefinition | 19,902 | 6,100 (31%) | 11,154 (56%) | 8,869 (45%) | 9,088 (46%) | 0 |
| completion | 394 | 88 (22%) | 110 (28%) | 104 (26%) | 88 (22%) | 0 |
| signatureHelp | 442 | 137 (31%) | 353 (80%) | 258 (58%) | 269 (61%) | 0 |
| inlayHint | 11,088 | 3,373 (30%) | 6,334 (57%) | 5,061 (46%) | 5,527 (50%) | 0 |
| documentHighlight | 19,902 | 6,421 (32%) | 10,019 (50%) | 8,310 (42%) | 8,584 (43%) | 0 |

As the server answers them ("After"), by source:

| Request | Fresh | Fresh, repaired | Stale | No answer | Fresh, differs at the end |
|---|---|---|---|---|---|
| hover | 5,924 | 4,754 | 214 | 8,834 | 176 |
| definition | 6,261 | 4,840 | 214 | 8,427 | 160 |
| typeDefinition | 6,080 | 4,840 | 214 | 8,748 | 20 |
| completion | 87 | 22 | 0 | 284 | 1 |
| signatureHelp | 137 | 215 | 1 | 89 | 0 |
| inlayHint | 3,272 | 2,757 | 204 | 4,754 | 101 |
| documentHighlight | 6,111 | 3,426 | 172 | 9,883 | 310 |

What each approach gained:

- **Older checks alone** (step 2) took hover from 31% to 46%: everything in
  units the edits can't affect.
- **Repaired copies alone** take it to 44% with no memory at all, which
  matters for a file opened broken or a server just started.
- **Together** they reach 56%. Repair answers most of what older checks
  could (with the current text of everything outside the blank lines), and
  adds what's before the edit in the method being edited, which an older
  check can't safely answer. The older check still answers 214 hovers
  where no copy parses or the blank lines are about more names than the
  edit is.
- **Signature help** gains most (31% to 80%): its `)` insertion already
  handled the call being typed, and now the rest of the file can be
  broken too.
- **Completion** gains little (22% to 28%) on the corpus, because the
  corpus asks only at `.`s that existed before the edit, mostly next to
  it, in the method being edited. Completion at the cursor being typed (the
  usual case) is changed text the corpus can't compare; tests/repaired.rs
  checks that it works there when the rest of the file is broken.

What stays dark is mostly the method being edited after the edit point,
and, for "retype a method" (2,106 of its 2,604 hovers), everything that
mentions a name the new method's unfinished text contains.

Latency, Apple M1 Max, 64 GB, `cargo test` (dependencies optimized),
each test alone, median of 7, from the client, on the std-sized project
(tests/latency.rs):

| | |
|---|---|
| hover, fresh | 112 ms |
| hover answered from a repaired copy | 135 ms |
| hover where nothing may answer: the fresh check, a repaired copy and the older version, three checks | 241 ms |

Run together, as `cargo test` runs them, while another session loaded
the machine, every query test stayed between 125 and 200 ms (references,
which check all of std, 442 ms), and the repaired hover 203 ms.

A document that doesn't parse is found in about a millisecond, so the
fresh check of a broken file costs nearly nothing. All are held to the
existing 500 ms limit.

## 3. What was built

### Results from the last good check (src/edits.rs, src/lsp.rs, src/query.rs)

- **`edits::Changes`**: the runs of unchanged text between an old version
  and the current one, in bytes, updated by each edit. `didChange` with
  ranges gives exact edits; a change without a range, a save with other
  text and closing with unsaved changes become one edit between the common
  start and end. A range maps forward only if it and the byte on either
  side of it are unchanged, so an identifier typed onto doesn't map.
- **`edits::Snapshots`**: for each file, the open documents at its last
  check that typed method bodies (a query's, or a diagnostics check's,
  which also counts for its target, so files opened later have one), and
  every edit since. Versions are shared between files. A document opened
  after a version was taken, with text other than what's on disk, counts
  as an edit of the disk text the version read.
- **The job** carries the file's version (`Fallback`), and the worker
  checks it when the fresh check fails with `noHir` or
  `declarationErrors`, and maps positions back and ranges forward.
- **Memory, bounded:** at most 4 versions of the open documents, plus the
  documents of up to 4 running checks (killed checks are forgotten), and
  at most 1,024 runs of unchanged text per changed file per version
  (24 bytes each; past that the shortest count as changed, which is
  always safe). docs/lsp.md documents it.

### Which answers still hold (src/units.rs)

A file splits into units: each method, and each other item (a type apart
from its methods, a trait, an `impl` block, a constant, an import), from
the `ast` crate's parse of the checked text. A method's types depend on
its text and the declarations of what it uses, never on another method's
body. So a change only affects:

- its own unit, if it's inside a method's body
- otherwise, units that mention a name it's about: the method, field or
  type it's in, the names it removed, and the names it added, as prefixes
  (they may be unfinished). Whole items (a method typed in full, or blank
  in a copy) are about only what they declare. Comments and blank lines
  between items are about nothing.

A unit mentions a name if its text does, or the formatted type of one of
its expressions does (so a value of a changed type counts without naming
it). On top of that:

- a unit with an expression the check couldn't type gets no answers (it
  may name something that didn't exist then), except completion's own
  placeholder
- highlights must have no use in changed text or in a unit without
  answers, where the check may have missed one
- inlay hints are filtered to units that hold, and a range with none is
  unavailable

The corpus found each of these: the first version had 24 wrong
definitions, 20 wrong inlay hints and 65 wrong highlights, from snapshots
of a check after a method was deleted (its callers were untyped), from
highlights missing uses typed since, and from uses in broken units.

### Fresh answers from a repaired copy (src/query.rs)

Where a document doesn't parse, a copy with blank lines, the first that
parses of:

1. the lines changed since the last good check
2. plus the line of a bracket nothing closes, found by a scan that skips
   strings and comments. The parser reports an unclosed `(` at the next
   statement and an unclosed `{` at the end of the file, so the line it
   reports is rarely the one to blank.
3. plus each line the parser stops at, up to eight times

For a declaration error, the copy blanks the units of the changes outside
method bodies. Blank lines keep every other line's number, and consecutive
ones are one change, so a whole method blank is one method.

The repaired copy's answers go through the same guard as stale ones, with
one addition: in a method with blank lines, what ends before the first
one means the same once the blank text is back, if all of it is fully
inferred. Text added later can fill in a placeholder but not change a
complete type. "Fully inferred" is checked on the type itself
(`walker::inferred`): my first version looked for `?` in the formatted
type, and the corpus's test_int scenario and tests/repaired.rs showed that
an unassigned placeholder for a type parameter prints as the parameter's
name (`Array[T]`), which gave a wrong `Array[T]` where the final text says
`Array[Int]`. The same rule is not sound for an older check, where a
complete type may have come from a later statement that changed since;
a mutation that allows it there produces wrong answers on the corpus.

### The server and the protocol

- `cuttlebone/status` reports `stale`, and `"repaired": true` on a fresh
  answer from a repaired copy.
- No new capability, request or option.
- `CUTTLEBONE_FAULT=norepair:<suffix>` (test builds only, like the other
  faults) turns repaired copies off, so tests/stale.rs can reach stale
  answers over LSP.

## 4. Which request may give which answer

docs/lsp.md, "Answers in code being edited", has the table:

| Request | Answers from |
|---|---|
| hover, definition, type definition, document highlight, signature help, inlay hints | the documents as they are, or a repaired copy, or the last good check |
| completion | the documents as they are, or a repaired copy: its items are inserted into the code, so never from an older check |
| references | the documents as they are only: a rename relies on them, and a blank line may hold a use |
| diagnostics | the documents as they are, always |

Definition and type definition count as read-only: they move the cursor,
and every location they give is mapped to text that's unchanged.

## The compiler's passes after a declaration error

I tried the smallest change that would let the type checker go on to
method bodies after an error in a declaration, and didn't keep it. In
`Compiler::check_types` (compiler/src/compiler.rs), the 21 passes are
chained with `&&`, and each `run_all` returns whether any error has been
reported so far. A config flag that makes each pass's result count as a
success (`(Pass::run_all(state, modules) || all)` on each line, plus the
flag: about 25 changed lines, all mechanical) does make every pass run.

Run on the twelve tests/fixtures/diagnostics projects (one per pass that
can fail), ten got through to typed method bodies, and **one panicked**:
`traits`, at `node.method_id.unwrap()` in `define_instance_method`
(compiler/src/type_check/expressions.rs:856). A method in an `impl` block
that the trait doesn't have is never defined, so it has no id. That's the
general shape: a pass that rejects a declaration skips it, and later
passes `unwrap` the ids it would have had. Making the passes continue
safely means finding every such `unwrap` across the passes, and either
defining something for rejected declarations or skipping them later. Each
is a change to the type checker, which the patch has never touched, and
there's no way to know they're all found but to fuzz.

What it would gain is also small now. With an older check, a declaration
error is already answered by a repaired copy (the changed declaration
blank) or the older check, and the same guard would still have to keep
answers out of units that use the broken declaration. The gain is
declaration errors in a file with no good check at all, such as a project
opened broken: those stay dark (repair can't tell which declaration to
blank without an older version).

## Every patch line added, and why

None. patches/upstream.patch is unchanged, and the experiment above was
reverted by re-applying the patch to a clean checkout (`scripts/upstream.sh`;
`scripts/check-patch.sh` passes).

## Tests

| File | What |
|---|---|
| tests/replay.rs (1) | The corpus, in three modes, with coverage floors per request and mode. |
| tests/stale.rs (8) | Over LSP, with repaired copies off: every read-only request answers stale with positions and ranges mapped in both encodings (after non-ASCII text, a line inserted above); completion and references don't; nothing in the method being edited; a changed declaration leaves out the units that mention it; a diagnostics check is a good check; a whole-text change; unsaved text when opened; no older check, no answer; hints in the method being edited left out; a signature help copy isn't a good check; definitions into an edited import mapped. |
| tests/repaired.rs (5) | Over LSP: the rest of the file fresh after an edit that doesn't parse, with completion; references never; before the edit in the method being edited (with no older check, and an unclosed bracket the parser reports later), and nothing after it; a placeholder type the blank lines could fill in; a changed declaration blank while another method's change since is answered fresh; the changed line blank first, where blanking from the parser's line would take in the next method. |
| tests/latency.rs (+1) | A hover from a repaired copy, and the three-check worst case, under 500 ms on the std-sized project. |
| src/edits.rs (6), src/units.rs (8), src/position.rs (+1) | Unit tests: mapping, margins, insertions at the start, the bounds; snapshots following edits, opened files, versions; units, effects of each kind of change, unfinished and whole items. |
| tests/definition.rs, tests/navigation.rs, tests/highlight.rs | Three tests asserted no answer for broken code at positions a repaired copy now answers correctly. They now assert the repaired answer there, and the status reasons at positions nothing can answer. |

## The mutations

All 174 are caught: milestone 4's 122, and 52 new.

- **The mapping and memory:** a split run's tail, the margins, text
  added at the start, the bounds on runs, versions and running checks,
  unused versions kept, byte offsets as positions, answers out of order in
  a batch, edits, whole-text changes and opened documents not recorded,
  diagnostics checks not kept (for files or their target).
- **Stale answers:** completion or references answering from older
  results, none at all, positions not mapped back, ranges (in the file or
  another) not mapped forward, the position's unit or the hints' units not
  checked, touched units, units mentioning changed names or with errors
  counting as safe, highlights missing uses in changed text or unsafe
  units, hints for a range with nothing safe, added names not prefixes, a
  body change changing other methods, a copy's check counting as the
  documents', the stale status, the fallback not sent (or not from the
  target's check).
- **Repaired copies:** none at all, references from them, each step
  of the repair (changed lines, the bracket scan and a mismatched bracket,
  the parser's lines, declaration units, body changes blanked for
  declaration errors, blank lines as separate changes), answers before the
  edit (none, or also from an older check, which the corpus catches as
  wrong answers), unfinished types or placeholders in type arguments
  counting as inferred, completion's placeholder as an error, whole items
  changing every name, the repaired status, the `norepair` fault ignored.
- **Moved code:** three existing mutations got new snippets.

Mutations I removed, because the code they broke was behaviour-neutral,
and I simplified the code instead:

- falling back only for `noHir` and `declarationErrors`: every other
  failure (`untyped`, an invalid position) is in a unit the guard refuses
  anyway
- treating `mut fn_name` operands, which are never typed, as typed: now a
  unit with one gets no answers from other checks
- a rule for what's between items: only comments and blank lines are
- checking the unit of every range in an answer: only inlay hints need it
- counting only expressions that end before a blank line: counting all
  that start before it is stricter, and changed nothing
- blanking the whole units of changes for syntax errors: the bracket scan
  and the parser's lines reach a copy that parses anyway (it moved 16
  hovers from repaired to stale, with the same total)

The first run of the new mutations caught 25 of 34, and each one that
survived either got a test or turned out neutral, as above. The first
`scripts/check.sh --full` caught 172 of 174: one mutation of milestone 4
no longer built (the status gained a field), and blanking the changed
lines first had no test that only it passed (the bracket scan found the
same line). Both are fixed, and the second has a test of its own. I also
tried blanking the line before each line the parser stops at, in case an
unfinished expression (`let x = 1 +`) was reported a line late, and
dropped it: Inko parses that as one expression with the next line, so
the text is fresh, not repaired.

## What clients should know

1. **`stale`** means the answer comes from an earlier version of the
   documents, the last that type checked, because the code as it is can't
   be. It holds for the documents as they are: every position and range is
   in text that hasn't changed, mapped to where it is now, in code no edit
   since could make mean something else. A client that doesn't know
   `stale` treats it as no answer (docs/lsp.md always said so); one that
   does may show it as it shows a fresh answer, or mark it.
2. **Only hover, definition, type definition, document highlight,
   signature help and inlay hints** are ever stale. Completion and
   references never are, and diagnostics are always from the current text.
3. **`"repaired": true`** on a `fresh` status means the answer comes from
   the documents as they are with some lines blank, outside anything the
   answer depends on. Completion may be repaired; references never are.
4. **Inlay hints** in broken code cover only the parts that hold: the
   method being edited has none after the edit point. A client that caches
   hints per range should ask again when the code parses.
5. **Incremental sync helps.** Edits with ranges are mapped exactly; a
   client that sends whole texts gets one change from the first difference
   to the last, which leaves more of the file dark.
6. **Latency** in broken code is up to three checks: about 240 ms on a
   std-sized project, against 110 ms for a fresh answer.

## Decisions you may want to revisit

- **Units are methods.** Inside the method being edited, only a repaired
  copy answers, and only before the edit. Finer units (statements,
  closures) would answer more of it, but shadowing and inference make
  "after the edit" unsafe in general.
- **Names, not ids, decide what a change affects.** Over-blocking a unit
  that mentions a name a change is about (say `Int`, in a signature being
  typed between items) is safe but costs coverage. Unfinished names as
  prefixes cost more, briefly, while a short prefix is typed.
- **Repaired copies before older checks.** Repair answers with the
  current text, so it's tried first; it costs a check where it then
  refuses, before the older check runs.
- **A diagnostics check is a good check** for every open file of its
  target, and for the target, so files opened later have one.
- **Definition and type definition may be stale.** They only navigate, and
  their locations are mapped; a client that edits at a definition's
  location should ask again once the code parses.
- **The fresh check's reason is kept** when neither other check answers:
  `noHir` or `declarationErrors` say why the code couldn't be checked, not
  why the guard refused.

## Remaining unknowns

1. **The compiler's passes after a declaration error**: not done (above).
   A project opened with a declaration error has no answers until it's
   fixed once.
2. **Wrong answers the corpus can't see.** Zero on 16 scenarios and
   21,873 answers from other checks (47,257 over the three modes) isn't
   proof. The rule's argument (method bodies are
   independent, names are what connect units) has known gaps: a change
   that alters method resolution through a trait without naming anything
   the unit mentions, and a new item whose name is a prefix of nothing yet
   but that shadows an import. I found no such case.
3. **Big projects**: memory grows with the open documents (4 versions),
   and a broken file costs up to three checks of its imports.
4. **Many open files in one project**: each file's version is its own last
   good check; a file whose version was dropped (more than 4 versions
   needed) has no stale answers until its next good check.
5. **From earlier milestones, still open:** lone `\r` line endings,
   Windows, packaging, and files changed outside the editor.
