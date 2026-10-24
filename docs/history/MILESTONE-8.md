# Milestone 8: answers when a declaration has an error

**Done.** One error in a declaration anywhere (an undefined type in a
signature, an import of something that doesn't exist, an `impl` of an
undefined trait) used to stop the type checker before any method body,
in every module of the check. Then a file had no answers, and references
went dark project-wide. Now:

- **The passes run on.** A setting in the patch runs every type checking
  pass after an error. `inko check` doesn't set it, so diagnostics are
  unchanged. Four `unwrap`s that a rejected declaration reached become
  skips. The patch grows by 43 lines added and 32 removed (21 of each
  are the chain's passes), and no typing rule changes (section 2).
- **Answers where the error can't matter.** While a declaration has an
  error, an answer from the documents as they are is given only in units
  (methods, or other items) that neither the error nor what changed in
  declarations since the last good check can affect. These are the rules
  older checks already follow. References are given only when no such
  unit mentions the name, in any module of the project.
- **Never a wrong answer**, on four new corpus scenarios and the old
  ones: an error in the file, in an import, in a module that imports the
  file, and in an unrelated one, with references asked now too.
  **References mid-edit: 929 to 1,249 answered**, 320 of them past a
  declaration error. **Without older checks** (a project opened broken),
  hover goes from 9,350 to 10,350 answered (section 3).

**Tests:** 286 (26 unit, 250 integration, 10 latency), up from 276.
`scripts/mutate.py` has 257 mutations, 24 of them new and 6 with new
snippets. All 257 are caught. The exhaustive tier passed, with one
mutation run again after a timeout under load (section 5).

## 1. What stops, and why (measured before changing anything)

### The pass chain

`Compiler::check_types` (compiler/src/compiler.rs:591-622) runs the type
checker's 21 passes chained with `&&`:

    DefineTypes, CollectExternImports, DefineModuleMethodNames,
    DefineImportedTypes, InsertPrelude, DefineTypeParameters,
    DefineTypeParameterRequirements, DefineTraitRequirements,
    CheckTraitRequirements, ImplementTraits, CheckTraitImplementations,
    CheckTypeParameters, DefineConstructors, DefineFields,
    check_recursive_types, DefineMethods, CheckMainMethod,
    ImplementTraitMethods, define_constants, Expressions,
    check_unused_imports

Every pass returns `!state.diagnostics.has_errors()` (or `true`, for
the two that can't fail). That covers every module, so one error
anywhere stops the chain after the pass that reported it, and
`Expressions`, the pass that types method bodies, never runs. No body is
typed in any module of the check, including the standard library's. The
chain is the only reason. Each declaration pass already records a
declaration it rejects in a way later code can use: an undefined type
becomes `TypeRef::Error` and the method, field or argument is still
defined (compiler/src/type_check/mod.rs:206-412).

### Which passes stop, in practice

I measured with a probe built against a copy of the tree (never
`.upstream/` itself). It shortens each name in each declaration line of
13 files by one character, the state a name is in while it's typed, as
the corpus's "break a name" scenarios do. It also duplicates or deletes
each whole method, field, constructor and type. The files are the
fixtures features, navigation (main, alpha, geometry/shapes), members
(main, shapes) and edges, and std's set, range, option, iter, fmt and
fs/path, with up to 120 names each. Of 905 shortened names, 414 gave a
declaration error. The first failing pass:

| Pass | Cases | Diagnostic |
|---|---|---|
| DefineMethods | 183 | `invalid-symbol` (an undefined type in a signature), `invalid-type`, `duplicate-symbol` |
| ImplementTraits | 80 | `invalid-symbol` (an `impl` of an undefined trait or type) |
| ImplementTraitMethods | 68 | `invalid-method` (a method the trait doesn't have), `invalid-implementation` (one it requires, missing), `invalid-symbol` |
| DefineImportedTypes | 46 | `invalid-symbol` (an import of an undefined symbol) |
| DefineFields | 22 | `invalid-symbol` |
| DefineTypeParameterRequirements | 14 | `invalid-symbol` |
| DefineConstructors | 1 | `invalid-symbol` |

The other 491 were errors only in bodies (387, which already get
answers) or no error at all (104, such as a renamed argument nothing
used). The whole-item mutations gave 424 more declaration errors,
including duplicate types and methods (`DefineTypes`, `DefineMethods`).

### Continuing the chain

The change milestone 5 tried, making each pass count as a success, runs
every pass. On the 414 declaration errors it panicked in **142** (34%),
at five places:

| Where | Cases | Why |
|---|---|---|
| define_types.rs:482, 483 (`CheckTraitImplementations::implement_trait`) | 90 | an `impl` whose trait or type didn't resolve has no `type_instance` or `trait_instance` |
| expressions.rs:971 (`verify_required_traits`) | 32 | a trait's required trait didn't resolve, so it isn't a trait instance |
| expressions.rs:856 (`define_instance_method`) | 19 | a method in an `impl` that the trait doesn't have is never defined, so it has no `method_id` (milestone 5's panic) |
| then methods.rs:1224 (`ImplementTraitMethods::implement_trait`) | 90 | the same `impl` as the first, one pass later |
| types/src/lib.rs:6330 | 1 | `type Option` renamed in std.option itself: the compiler requires it |

The first four are the same kind: a pass `unwrap`s an id that an earlier
pass would have set, had it not rejected the declaration. Each needs a
skip there (`let Some(..) else { return }`, or `continue`), only on a path
that upstream never takes, because upstream stops before it. With four
of them (in three files), the 414 name cases, the 424 item cases and
the twelve tests/fixtures/diagnostics projects (one per pass that can
fail) run every pass without a panic, except the one that renames
`Option` in std.option. A panic is caught either way, and then the check
falls back to stopping where it does now.

### Are the bodies typed right?

The concern is that a method typed against a broken declaration gets
other types than it will once the declaration is fixed. So for each case
that didn't panic, I compared every expression's type, and every
reference's declaration, with the check of the unbroken text. The
comparison was unit by unit (units.rs: a method, or another item without
its methods), with the guard Cuttlebone already uses for older checks
and repaired copies:

- a unit is refused if a declaration error is in it, or if it has an
  expression the check couldn't type
- and if it mentions (in its text, or in its expressions' types) a name a
  declaration error is about: the item the error is in and the items
  that contain it (a method and its type, a field and its type, an
  `impl`'s type and trait), and every name in an import with an error

| | Name cases | Item cases |
|---|---|---|
| declaration errors, no panic | 413 | 423 |
| units answered | 6,826 | 7,828 |
| of those, differing from the unbroken check | **0** | **0** |
| expressions in answered units compared and equal | 70,401 of 70,401 | 82,577 of 83,365 (the rest are in the changed lines) |
| units refused: the error's own | 360 | 315 |
| units refused: an untyped expression | 251 | 72 |
| units refused: a name | 2,513 | 3,835 |

Without the guard, the refused units differ often, so it isn't vacuous:
in the name cases, 231 of the 360 with the error in them, 240 of the 251
with an untyped expression, and 6 of those refused for a name. With it,
none does.

### The alternative without touching the compiler

Blanking the broken declaration in a repaired copy, as milestone 5 does
for syntax errors, needs no patch line. The errors' locations say what
to blank. But it has three costs, and the chain doesn't:

- **Rounds.** The passes stop at the first that fails, so only its errors
  are known. Blanking them can reveal more in a later pass (an import
  blank makes the `impl` of what it imported fail), so it takes one
  check per round, on top of the check that failed.
- **References stay dark.** A repaired copy isn't the documents as they
  are, and references must be fresh. With the chain continued, the check
  *is* of the documents as they are, so references can be answered when
  no declaration error could hide one.
- **Less in the affected module.** A whole item is blank, where the chain
  keeps everything the guard doesn't refuse.

### The smallest change, and the decision

- **compiler/src/compiler.rs:** the chain gets `|| all` on each pass, where
  `all` is a new config flag, and the result is whether any error was
  reported, which is what it already was. `inko check` doesn't set the flag,
  so it runs exactly as before, and so do Cuttlebone's diagnostics (which
  must match it). The `check_with` hook also passes the diagnostics
  reported so far to `inspect`, so Cuttlebone knows where the declaration
  errors are while it answers from the typed HIR.
- **compiler/src/config.rs:** the flag.
- **type_check/define_types.rs, methods.rs, expressions.rs:** four skips
  of what an error left without an id. No typing rule changes: each only
  turns an `unwrap` that would panic into skipping the declaration, on a
  path upstream never reaches.

That is about 45 added and 35 removed lines, all of them mechanical or a
`let ... else`. It's small and changes no type-checking logic, so I'm
building it. Rebasing costs one hunk per site. If a release adds a pass,
it needs its `|| all`. If it adds an `unwrap` that a rejected
declaration reaches, that's a new panic, which falls back to today's
behaviour (no answer), not a wrong answer. The corpus and the tests
(below) would show it as lost coverage.

## 2. What was built

### The patch

As decided above: `check_after_errors` in `Config`, `|| all` on each of
the 21 passes, the result still an error if any pass reported one, and
the four skips. The `check_with` hook also passes the diagnostics
reported so far to `inspect`: Cuttlebone holds `inspect` open while it
keeps a check, so that's the only time it can read them.
patches/README.md has every line and why.

### Which answers hold (src/units.rs, src/query.rs)

A check now keeps its errors (`Checked::declaration_errors`). Each error
is placed in its file's outline (`units::error_effect`):

- **in a method's body:** nothing outside the method depends on it, so it
  counts for nothing, as before
- **in a declaration:** like a change of that item, touching its unit and
  about the names of the item and the items it's in (and every name of
  an import)
- **nowhere** (no item contains it): nothing is answered

If any error is in a declaration, the check is *guarded*. Every answer
from it goes through the same `Guard` that answers from older checks and
repaired copies use, with the errors as changes, and the old and new text
the same. On top of that come the names of what changed outside method
bodies since the file's last good check, if there is one
(`declaration_changes`). The error may come from them, as when a type is
renamed and its uses no longer resolve, and fixing it may undo them.
Where the guard refuses, the answer is `null` with reason
`declarationErrors`, and the repaired copy and the last good check are
tried as before.

**References** (`hidden`): the position's unit must be safe, and no
unsafe unit in any module of the project may mention the symbol's name.
A reference is always a name in the text, so an unsafe unit that doesn't
mention it can't hide one. Building a guard per module costs an outline
per file. The walks are the ones references do anyway.

**Operators.** A highlight or reference of an operator (`+`) has no
identifier to look for, so `Guard::typed_since` used to find nothing to
refuse for it. Now an operator is looked for as text in unsafe units,
and an operator highlight from an older check is refused if anything in
the file changed. That was a gap in milestone 5's stale highlights too:
an `@count + 1` typed since in another method is a use the older check
never saw (tests/stale.rs).

**Status.** These answers are `fresh`: they come from the documents as
they are. Inside, they're `Source::Guarded`, so that the corpus holds
them to the final answers. A guarded check isn't a good check for later
stale answers (`typed` is false), so older checks keep their meaning
("the last that type checked").

### When a pass after an error panics

The fuzzing found one case left (std.option's `Option` renamed in std
itself). For any such case, `start_check` checks again with the passes
stopping at the error, as `inko check` does. The answer is then
`declarationErrors` as before, not `compilerPanic`.

### A fault for the old path

Repaired copies and the last good check still answer what a guarded
check refuses, and everything after a panic. Their tests for declaration
errors need a check that stops, so `CUTTLEBONE_FAULT=stop:<suffix>`
gives one (test builds only, like the other faults). Faults can now be
combined with commas. tests/stale.rs runs with `norepair` and `stop`.

## 3. Corpus results, before and after

The corpus has 20 scenarios, up from 16:

| New scenario | The error is |
|---|---|
| navigation: break a module that imports the file | in main.inko, which imports alpha.inko, asked about |
| navigation: break an unrelated module | in beta.inko, while qualified.inko is asked about |
| navigation: rename a type | `type pub Point` itself, so its uses fail |
| navigation: break an impl's trait in an import | `impl Area for Circle` in geometry/shapes.inko |

The scenarios that were already there cover an error in the file asked
about ("features: break an argument's type", "... a field's type", "std
set: break a return type") and in an import ("navigation: break a return
type in an import", "navigation: break an import"). References are now
asked in every fixture scenario, at up to 10 positions per edit (one
check of the project each). They're compared like highlights: every
reference in text that exists both mid-edit and at the end must be
there. std's scenarios don't ask them: a check of all of std at every
edit costs too much.

"Before" is milestone 7's code on the same 20 scenarios: the copy of the
tree I measured in, with its continued chain off and a placeholder for
the new source, so the new replay.rs compiles. Answered questions (any
source), Apple M1 Max:

| Request | Asked | As the server runs: before | after | of which past a declaration error | Without older checks: before | after | Without repaired copies: before | after | Wrong |
|---|---|---|---|---|---|---|---|---|---|
| hover | 21,047 | 11,878 | 11,904 | 902 | 9,350 | 10,350 | 9,896 | 9,896 | 0 |
| definition | 21,047 | 12,285 | 12,311 | 902 | 9,721 | 10,721 | 10,217 | 10,217 | 0 |
| typeDefinition | 21,047 | 11,964 | 11,990 | 902 | 9,400 | 10,400 | 9,896 | 9,896 | 0 |
| completion | 420 | 115 | 115 | 0 | 109 | 109 | 93 | 93 | 0 |
| signatureHelp | 471 | 382 | 382 | 38 | 272 | 311 | 298 | 298 | 0 |
| inlayHint | 11,618 | 6,640 | 6,652 | 496 | 5,226 | 5,784 | 5,831 | 5,831 | 0 |
| inlayHint, whole file | 483 | 483 | 483 | 52 | 328 | 386 | 483 | 483 | 0 |
| documentHighlight | 21,047 | 10,787 | 10,789 | 714 | 8,838 | 9,616 | 9,351 | 9,351 | 0 |
| references | 3,334 | 929 | 1,249 | 320 | 929 | 1,261 | 929 | 1,249 | 0 |

What it says:

- **References gain most:** a third more answered, all of it past
  declaration errors. Per scenario, references went from 8 to 43 of 80 in
  "break a module that imports the file", from 9 to 72 of 108 in "break
  an unrelated module", and from 10 to 90 of 120 in "break a return type
  in an import". Before, any of these made them unavailable.
- **Without older checks** (a project opened broken, or a server just
  started), hover gains 1,000 answers: a declaration error no longer
  needs an older version to blank.
- **As the server runs,** the totals barely move. Answers past the error
  replace what repaired copies (blanking the changed declaration) and
  older checks already answered: the same rule refuses the same units.
  In "navigation: break a return type in an import", 286 repaired hovers
  became 260 guarded and 26 repaired. Without repaired copies, the
  guarded answers exactly replace stale ones.
- **No coverage was lost** for any request in any mode, and **no answer
  is wrong**: 4,326 answers past a declaration error (with older checks)
  and 4,392 (without), each equal to the final fresh answer.

The coverage floors in tests/replay.rs are the "after" columns, and the
sample's are measured the same way. The sample now has "navigation:
break a module that imports the file", so every commit asks references
past an error.

## 4. Diagnostics

Unchanged: `diagnose` doesn't set the new setting, and tests/diagnostics.rs
still compares every fixture with `inko check --format json` exactly. The
mutation that sets it for diagnostics is caught there, since the passes
after an error then report errors in bodies that `inko check` doesn't.

## Every patch line added, and why

patches/README.md has each one. In short:

| File | Lines | What |
|---|---|---|
| compiler/src/compiler.rs | +23 -22 | `|| all` on 21 passes, the result, the hook's third argument and its doc comment |
| compiler/src/config.rs | +4 | the setting |
| compiler/src/type_check/define_types.rs | +5 -2 | skip an `impl` that didn't resolve |
| compiler/src/type_check/methods.rs | +5 -2 | the same, a pass later |
| compiler/src/type_check/expressions.rs | +6 -6 | skip a method never defined, and a required trait that didn't resolve |

The patch is now 549 lines added and 511 removed, 469 of each in
hir.rs. **Cost per release:** each skip is a hunk in type-checking code,
and the chain is 21 lines that a new pass changes. Most releases should
apply as they are, or need these re-made by hand (patches/README.md,
"Rebasing"). A release can also add an `unwrap` that a rejected
declaration reaches. That panics, Cuttlebone checks again stopping at the
error, and the loss shows as coverage. tests/broken.rs runs every
diagnostics fixture (between them, every diagnostic) past its errors and
fails on a panic.

## Tests

| File | What |
|---|---|
| tests/broken.rs (+8, 1 changed) | Errors in declarations through `query`: one in an import leaves out only what uses it (and isn't a good check); references with an error in an importer and in an unrelated module, given and refused; an operator's highlights; an error in a body counts for nothing; the changes since the last good check count; the panic after std's `Option` is renamed; every pass after each diagnostics fixture's errors, with the result still an error; each of the four skips. Changed: an undefined type in a signature still stops dump-types, as it does `inko check`, but not queries. |
| tests/replay.rs | Four scenarios, references asked and compared, answers past declaration errors held to the final ones, the new floors. |
| tests/stale.rs (+1) | An operator typed since has no highlights from the last good check. Runs with the `stop` fault too. |
| tests/repaired.rs | The repaired-copy test for a changed declaration runs with the `stop` fault. |
| src/units.rs (+1) | Where an error is, and what it's about. |
| tests/citations.rs | Three upstream lines moved. |

## 5. The mutations

24 new, all caught (`scripts/mutate.py` on them, and on the six whose
snippets moved: 34 of 34 caught, 582 s, plus "an error's item names not
counted", which the corpus catches):

- **The patch:** the passes stopping at the first error, a check past
  errors that succeeds, the hook without diagnostics, and each of the four
  skips undone.
- **Checks:** diagnostics running every pass, queries stopping at the
  first error, queries without the errors.
- **Where errors are:** errors in bodies counted as declarations', the
  error's item's names and an import's names not counted.
- **The guard:** declaration errors not guarded, a guarded check as a good
  check, guarded answers not marked, references not checked for hidden
  uses (or in the file only), operators not looked for (as names, as
  text, or in changed text), the changes since the last good check not
  counted.
- **Robustness:** a panic after an error not checked again, the `stop`
  fault ignored.

Moved: "no required-version check", "other files' ranges use the
request's lines", "query uses lsp", "warnings dropped", "the position's
unit not checked", "highlights missing uses in unsafe units".

One branch has no mutation: an error that no item contains answers
nothing (`ErrorEffect::Unknown`). No input I know of reaches it (the
compiler's errors outside items are parse errors and a missing `Main`,
which stop before), so no test can fail when it breaks. src/units.rs's
test checks that it's what `error_effect` says for such a range.

**The tiers.** scripts/mutate.py changed (its list of mutations), so the
milestone tier said the exhaustive tier must pass too (AGENTS.md). That
tier runs everything the milestone tier does, so I ran it in its place, on
d7fce99, the commit that first added this report. Only this paragraph
changed since. It took 2,299 s in all, with the machine loaded by another
session throughout (load averages 140 to 320): the checks of every commit
59 s, the whole corpus 98 s, every test without kept checks 154 s, the
selection check 4 s, and the mutations 1,984 s (with the runner's control,
225 s). **256 of the 257 mutations were caught. The other, "the batch's
answers out of order", timed out at the runner's 900 s, under that load,
rather than surviving.** Run again alone (`scripts/mutate.py "the batch's
answers out of order"`), it was caught by the corpus in 575 s. It's
milestone 6's, and this milestone didn't change its code.

## Measurements

Apple M1 Max, 64 GB, `cargo test` (dependencies optimized). Another
session loaded the machine heavily for much of this milestone (load
averages up to 318), so the latency tests failed three of my first
commits' hooks. The timings below are from quieter moments.

| | |
|---|---|
| Fuzzing (section 1): 905 broken names and 696 items duplicated or deleted, in 13 files, each checked twice | about 8 minutes for each set, 6 at a time |
| The whole corpus, before (milestone 7's code) | 92 s |
| The whole corpus, after | 96 s |
| The corpus sample, every commit | 6.6 s (was 3.5 s: references, and a scenario more) |

Latency: a guarded check costs what a check that stopped did, plus the
passes after the error, which a check without the error runs anyway. So
a hover in code with a declaration error costs about what one in code
without it does. tests/latency.rs's limits are unchanged and pass.

## What clients should know

1. **Answers while a declaration has an error.** Hover, definition, type
   definition, highlights, signature help, inlay hints, completion and
   references now answer, `fresh`, in code that a declaration's error
   can't affect, including in a project opened with one. Elsewhere they're
   `null` with reason `declarationErrors`, which now means "the error may
   change this", not "nothing was typed".
2. **References no longer go dark** project-wide for a declaration error.
   They're given when no unit that the error may affect mentions the
   name, in any module. Otherwise they're `null`, as before.
3. **Inlay hints** past a declaration error cover only the units that hold,
   and `covered` says so.
4. **Diagnostics** are unchanged: still exactly `inko check`'s.

## Decisions you may want to revisit

- **Skips in the type checker.** Four `let ... else` in type_check/ are the
  price of running every pass. The repair alternative (section 1) needs
  none, but rounds of checks, and references would stay dark.
- **Names, not ids,** decide what an error affects, as for older checks:
  an error in `Counter.add`'s signature refuses every unit that mentions
  `Counter`. Measured, nearly everything refused for a name would have been
  right (63,027 of 63,346 expressions in the item fuzz), so a finer rule
  (ids, or only the broken signature's own name) would answer much more.
- **The changes since the last good check count** past a declaration
  error, which refuses more than the error alone would. Without them, a
  renamed type's new name would answer as it is now, which differs once
  the rename is undone.
- **A guarded check isn't a good check.** Counting it would give stale
  answers in more places, with its own errors guarded. I kept the old
  meaning: the last check with nothing in the way.
- **Units with an untyped expression get nothing** past a declaration
  error, even if the expression has nothing to do with it. That's the
  older checks' rule, and safe.

## Remaining unknowns

1. **Panics the fuzzing didn't reach.** 838 declaration errors from names
   and whole items, in 13 files, and every diagnostic's fixture. Other
   shapes (type parameter bounds, `extern` items, constants that refer to
   each other) may reach other `unwrap`s. Those fall back to stopping, so
   they can cost answers but not give wrong ones.
2. **Wrong answers the corpus can't see.** Zero of 4,326, plus 0 of 14,654
   answered units in the fuzzing. That isn't proof, and the argument
   (types flow between units only through names) has the gaps milestone
   5 listed. The rename case is the one I found and closed.
3. **References in std-sized projects** past an error: the corpus asks
   them only in the fixtures. The rule is the same; the cost is an
   outline per module.
4. **From earlier milestones, still open:** Windows, and packaging beyond
   `cargo install`.
