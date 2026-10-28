# Milestone 15: what an indexing client still kept its own code for

STATUS

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

TESTS

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

MEASUREMENTS

PATCH

PARAKEET

DECISIONS

UNKNOWNS

RELEASE
