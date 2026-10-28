# Milestone 15: what an indexing client still kept its own code for

**Done, but not released yet.** Everything for 0.15.0 is committed and
the release workflow's dry run passed on this report's commit (section
10); pushing the tag `v0.15.0`, which publishes it, was refused by the
session's permissions, and is left to you. It has:

1. **Quick fixes in files with other errors** (section 1): a fix is
   offered when its result has fewer of its diagnostic and no error the
   documents didn't have, not only when the file then has no error.
2. **Four more quick fixes** (section 2): `missing-field` (the field
   added to the call), `invalid-implementation` (every method the `impl`
   block lacks, stubbed), `missing-trait` (`impl R for T`, with its
   methods), `unreachable` (the code removed). Each still passes the
   oracle: the installed `inko check` on the project with it applied.
3. **Completion of names in code being edited, and keywords**
   (section 3): a declaration that differs, or has an error, refuses
   names only where it can bind or hide one, or change what's said of
   one; keywords where the grammar takes them.
4. **Document symbols that hold still** (section 4): a brace a method's
   body opens and nothing closes yet keeps the type it's in, and
   everything after it.
5. **Signature help for a type's constructor** (section 5), from its
   fields.
6. **`collapsedText` in folding ranges** (section 6).
7. **Workspace symbols for an index** (section 7): `impl` blocks, every
   symbol for an empty query, and every match of any query as partial
   results. Visibility has no standard place in a workspace symbol;
   docs/lsp.md says why, and points to document symbols' `detail`.

Everything is standard LSP 3.18: no new extension, and no patch change.
"Never a wrong answer" holds on the whole edit-replay corpus.

**Tests:** 420 (52 unit, 344 integration, 24 latency, one of which only
measures). `scripts/mutate.py` has 521 mutations, 51 of them new (and
one removed: it guarded that `impl` blocks weren't workspace symbols).

This milestone was requested: built by a coding agent with little or no
review.

## 1. Quick fixes in files with other errors

docs/lsp.md, "textDocument/codeAction", is the reference.

`query::fixes` checked each fix on a copy of the documents and gave it
only if that copy had no error at all (MILESTONE-13.md: "In a file with
two undefined symbols, neither import is offered"). Now it gives a fix
when the copy has fewer of the diagnostic it fixes, and each error,
counted by file, ID and message (positions move with the edits), at most
as often as before, less the one fixed (`fixed_without_errors`, with a
unit test of its counting). So a file being typed, which nearly always
has another error, gets the fixes that break nothing else.

The names to import came only from the client's `context.diagnostics`.
A client that shows diagnostics of its own sends none, so now every name
in the requested range (from the start of the word it starts in to the
end of the one it ends in) is a candidate too; the worker offers only
the imports of names its own check says are undefined.

The oracle in tests/actions.rs changed with the rule: `inko check` on the
project with the fix applied reports fewer of the diagnostic than on the
project as it is, and no error it didn't report before.

**What a check can't see.** The compiler stops after a pass that
reported an error. A fix for an error of an early pass (`missing-trait`
is reported where types are defined, before bodies are checked) lets
later passes report what they find, which counts as adding errors if the
documents have any. Such a fix is offered once those are fixed; that's
conservative, never wrong.

## 2. Four more quick fixes

| ID | Fix | How |
|---|---|---|
| `missing-field` | `Add x: v` | the field's type from the type's declaration, found by definition at the call's type name; `0`, `0.0`, `''`, `false`, `[]`, `Option.None` for `Int`, `Float`, `String`, `Bool`, `Array[...]`, `Option[...]`, and `x: x` if the file has the name elsewhere. Each is checked, so only those that type check are offered. |
| `invalid-implementation` | `Implement m`, or `Implement the missing methods` | the compiler's own signature of each missing method is in its message (`the method 'fn pub mut next -> Option[T]' must be implemented for 'Numbers'`); each gets a body of `panic('not implemented')`, at the end of the block (or as its body, if it had none). A trait's type parameters are replaced by the block's type arguments, from the trait's declaration (`T` by `Int` in `impl Iter[Int] for Numbers`). |
| `missing-trait` | `Implement R for T` | `impl R for T {}` after the block that requires it, checked once to learn what it lacks, then stubbed as above |
| `unreachable` | `Remove the unreachable code` | a `case` arm's lines, or the statement, the rest of its block (`syntax::block_end`, from the tokens) and the blank lines before it |

The compiler reports each missing method as a diagnostic of its own at
the block, so the one fix that stubs them all would have been an action
for each. A fix that's the same for several diagnostics (title and
edits) is now one action whose `diagnostics` has them all, checked once.

Definition gives nothing at a trait's name in an `impl` header with an
error (the unit is the error's), so the trait's declaration is found
from the syntax: in the file, or in the module the file imports it from
(std's, or the project's `src/` and `test/`).

## 3. Completion of names in code being edited, and keywords

docs/lsp.md, "In code being edited", is the reference.

Names were refused from a repaired copy or past an error in a
declaration whenever any declaration differed. Now each change outside a
method's body, and each error in a declaration, refuses them only if
(`Guard::names_hold`):

- it's in the file asked about, outside every type, trait and `impl`
  block: an import, a module's method or constant, a declaration being
  typed may bind a name anywhere in the module (`Effect::top`);
- it declares a name the answer lists, binds (`Names::bound`, which
  decides what to import) or mentions in a `detail`
  (`Effect::declares`: the names it changed, and the item it's in, but
  not the containers it's merely inside); or
- it's inside the type or trait of the method the position is in, or a
  trait whose methods that type has (`Effect::within` against
  `Names::depends`, from the type database): a member added there is in
  scope through `self`.

As for every answer, the position's own method must not mention what an
error is about; that rule (MILESTONE-8.md) is what refuses most names in
the corpus, so the corpus's names coverage is unchanged (274 answered of
529, none wrong): its edits are in the method being completed. The new
tests in tests/names.rs show what the narrower rule answers and refuses:
an error in another type's method's signature, another type's method
half typed (a repaired copy), an error in another module's declaration
the file doesn't import (answered, the same as with no error); the same
in the method's own type, in a trait the type implements, in a
declaration the answer lists, or at the top of the file (refused).

**Keywords** (`query::keywords_at`), as Keyword items after the names:
where an expression starts, those the parser's `value` takes; in a
type's name, those `type_reference` takes, or only `fn` right after
`ref`, `mut`, `uni` or `move` (`reference_type`). The type checker may
still refuse one there, as `break` outside a loop.

## 4. Document symbols that hold still

docs/lsp.md, "textDocument/documentSymbol", is the reference.

Typing `if x {` without its `}` blanked a line, and every declaration
whose lines had another depth of braces in the copy than in the text was
left out: the type the method is in, and everything after it. But no
body can hold a declaration, so a brace a method's body opens closes in
that method. Where every blank line that opens a brace is in a method's
body (and none closes one), every symbol of the copy is given now, but
those methods: one may end at another `}` once the brace is closed
(`Outline::nested_as_in`).

Which `{` is left open isn't always the innermost: `if x {` at the end
of a method makes the method's `}` close the `if`, and the type's close
the method, so the brace left open at the end is the type's. The opener
of the first `}` that starts a line indented otherwise than its `{` is
tried too, and that copy is the answer if it keeps every symbol the
other does, and more (`unclosed_or_misclosed`, `Outline::keeps`). Using
it for every repaired copy, type checks too, cost hover coverage in the
corpus, so it's only for outlines.

The corpus leaves out 378 symbols with lines blank, from 385, with no
wrong answer.

## 5. Signature help for a type's constructor

The walker records the type a constructor call makes
(`CallKind::TypeInstance`, which the compiler sets even with fields
missing). Its parameters are the type's fields in the order the type
declares them, labelled as the type with the call's type arguments:
`Box[String](value: String, size: Int)`. Arguments match them by
position or name, also while the call is typed (tests/signature.rs).

## 6. Folding ranges' collapsed text

For a client with `foldingRange.foldingRange.collapsedText`: a comment's
first line, `import ...` for the imports, and a block's brackets around
`...`, the placeholders the client used before. The tests' check of
every message (tests/common/spec.rs) refuses `collapsedText` for a client
without it, and tests/folding.rs has one.

## 7. Workspace symbols for an index

- **`impl` blocks** are symbols: named by their header (`impl ToString
  for Counter`), of the kind document symbols give them, at the type's
  name.
- **Every symbol** for an empty query: no limit. A query that isn't
  empty still gets the best 500, which is what a client typing into a
  search wants; an exact name is always among them, since the whole
  name ranks first.
- **Partial results**: a client that sends a `partialResultToken` gets
  every match of any query in `$/progress`, 500 at a time, and an empty
  response, as the spec says partial results end.
- **Public or not**: LSP has no place for it in a workspace symbol.
  `SymbolTag` has only `Deprecated`, `WorkspaceSymbol` has no `detail`,
  and `containerName` is the qualifier clients show. Rather than an
  extension, docs/lsp.md points to document symbols' `detail`, which is
  the declaration as written, `pub` and signature included.

## 8. Measurements

Apple M1 Max (the reference machine), under `cargo test`, at load
averages of 130 to 600 from other work on the machine, so upper bounds:

| What | Measured | Limit (tests/latency.rs) |
|---|---|---|
| every workspace symbol (over 5,000), std-sized project, one response | 180 to 660 ms | 400 ms |
| the same as partial results, until the empty response | 80 to 120 ms | 400 ms |
| workspace symbols while a query is typed (unchanged) | 2 to 73 ms | 50 ms |

The 660 ms was at a load average of 440, where the limit scales by the
load; at 350, 180 to 290 ms. In the milestone tier's strict step this
Mac's baseline measured 375 ms (scale 1.34), and every limit held. A
quick fix's cost is unchanged: one full check of the documents, and one
for each fix (at most 8), plus one more for a missing trait's methods.

## 9. Tests and mutations

| File | What |
|---|---|
| tests/actions.rs | the oracle as "fewer of the diagnostic, no new error"; fixes in files with other errors, a fix that would add one, names from the range; missing fields, methods (in a block with a body and without, a trait's type parameter, a trait imported from std, several in one action), traits, unreachable statements and arms |
| tests/names.rs | the narrower rule, answered and refused (another type, another module, the method's own type, a trait it implements, an imported name, the top of the file); keywords where expressions and types start |
| tests/symbols.rs | an unclosed brace in a method keeps the type; `impl` blocks; every symbol for an empty query and as partial results |
| tests/signature.rs | a type's constructor: positional, named in any order, unfinished, generic |
| tests/folding.rs | collapsed text, a client without it, a lone `\r` |
| tests/latency.rs | every workspace symbol, in one response and as partial results |
| tests/common/spec.rs | `collapsedText` only for a client that takes it |
| tests/lsp.rs | the test of a cancelled check kept by its worker cancels once a worker runs it (it raced on a loaded machine) |
| src/query.rs, src/units.rs, src/syntax.rs | unit tests: the counting rule for fixes, what a change declares for names, braces opened in and outside bodies, which brace is left open |

**The milestone tier, locally**: `scripts/check.sh --milestone 1fa7e3d`
on f58fef0, at load averages of 130 to 500: every check passed
(formatting 1 s, clippy 14 s, tests 137 s, the patch 0 s, the release
build 31 s, the whole corpus 93 s, no kept checks 295 s, the selection
13 s, the release tooling 73 s), and the strict latency step (advisory)
passed, 72 s. Of its 449 selected mutations, 441 were caught and 8
weren't (158 minutes in all). Each of the 8 needed a test case of its
own (0e6db2d): with those, all 8 are caught (17 minutes).

## Every patch line added, and why

None: the patch is unchanged. Constructor calls' `TypeInstanceInfo`, the
types' fields and implemented traits are public upstream.

## What Parakeet can now delete

Each item of the request, and the fallback it replaces (Parakeet's
docs/history/SESSION-slimming.md, "Requests for Cuttlebone" and
"Fallbacks kept"):

1. **Quick fixes that don't need the whole file to type check** →
   Parakeet's import, unused-import, `_x` and `let _ =` fixes
   (`InkoImportFix`, `RemoveUnusedImportFix`, `PrefixUnderscoreFix`,
   `DiscardResultFix`). A file with other errors gets them, and a client
   that sends no diagnostics in the context still gets imports.
2. **The fixes Parakeet has and Cuttlebone didn't** → `AddMissingFieldFix`,
   `InkoImplementMethodsFix` (and Implement Methods, ⌃I, in an `impl`
   block that lacks them), `ImplementMissingTraitFix`,
   `RemoveUnreachableFix`. Unlike Parakeet's, each result type checks: a
   missing field gets a value, a missing method a body.
3. **Completion of names in code being edited, and keywords** →
   Parakeet's own names completion and auto-import, and its keywords.
   Names are refused only where a change can bind or hide one; where the
   method being typed mentions what changed, they still are, so a client
   that wants an answer there regardless keeps a fallback for that.
4. **Stable document symbols** → `InkoOutline` for the Structure view and
   breadcrumbs: an unclosed brace in a method leaves out only that method.
5. **Signature help for a type's constructor** → parameter info's own
   reading of the type's fields for `Point(x: 1)`.
6. **`collapsedText` in folding ranges** → the placeholders Parakeet had
   before (the comment's first line, `import ...`, `{...}`), now from the
   server, for a client that declares `foldingRange.collapsedText`.
7. **`workspace/symbol` for an index** → with implementations and type
   hierarchy from milestone 14, `InkoDefinitionIndex` and
   `InkoSourceDefinitions`: `impl` blocks are symbols, an empty query has
   everything, and partial results page any query. Visibility and the
   signature aren't in a workspace symbol (LSP has no place for them);
   they're in document symbols' `detail`, per module.

Still Parakeet's own: rename and refactorings as code actions (the next
milestone).

## Decisions you may want to revisit

- **A fix counts errors by file, ID and message**, not position: two
  errors with the same message in a file count as two, and a fix may
  move one. Exact positions can't be compared once edits shift them.
- **A missing field's value** is a literal for six types, or a variable
  of the field's name; another type gets no fix rather than a guess.
- **Stubs panic** (`panic('not implemented')`), which type checks for any
  return type. Parakeet's left the body empty, which doesn't.
- **A missing trait's fix costs a check more** (to learn the methods the
  new block lacks), and a fix for an error of an early pass is refused
  while the documents have errors later passes would report.
- **The trait of an `impl` with an error is found from the syntax**
  (the file, or the module it's imported from), not the type database.
- **Names: the position's own method must still not mention what
  changed.** That's the general rule for answers, and it's what refuses
  most names in the corpus; the narrower rule doesn't relax it, since a
  variable's type in a `detail` may depend on what changed.
- **Keywords are the grammar's**, not the type checker's: `break`
  outside a loop is offered.
- **The method with the unclosed brace is left out** of document
  symbols, since its end is ambiguous; everything else stays.
- **The misindented brace's copy is used for outlines only**: for every
  repaired copy it cost hover coverage in the corpus (12,651 answered,
  fewer than the 12,673 measured).
- **A query that isn't empty is still capped at 500** without a
  `partialResultToken`; an exact name always ranks first.
- **Visibility isn't sent in workspace symbols**: no extension, and
  `containerName` stays the path.

## Remaining unknowns

1. **IntelliJ's handling of `collapsedText` and partial results** wasn't
   tried; the tests are against the spec, its meta model and the
   client's capabilities.
2. **The machine was heavily loaded** (load averages up to 600) the
   whole session; the latency measurements are upper bounds, and some
   timing-sensitive tests failed under it and passed alone (one was made
   robust: tests/lsp.rs, above).
3. **The corpus doesn't exercise the narrower names rule**: its edits
   are in the method being completed. The LSP tests cover each case.

## 10. The 0.15.0 release

**The dry run** on 0e6db2d (run 40678208743, workflow_dispatch, 42
minutes) passed every job: the release checks, the latency job (the
limits as written, scaled by its runner's baseline), all 16 mutation
shards and the dist shard (each control passing, every mutation
caught), and the four builds.

**A second dry run** on this report's commit, fa2f40d (run
40683946731, 39 minutes), passed every job too.

**Not released.** Pushing the tag `v0.15.0` (on fa2f40d) was refused by
this session's permission system as creating a public surface, so
nothing is published and nothing was checked from outside. To release:

    git tag -a v0.15.0 -m "Cuttlebone 0.15.0" fa2f40d
    git push origin v0.15.0

The workflow then runs the release tier again, publishes, and checks the
published release on four runners, as for 0.14.0.
