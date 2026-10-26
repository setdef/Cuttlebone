# Milestone 12: documentation in hover, symbols, and completion of names

**Done.** Cuttlebone 0.12.0 is published,
<https://github.com/setdef/Cuttlebone/releases/tag/v0.12.0>, built by the
release workflow and checked from outside (section 6). It answers the four
things Parakeet keeps its `inko doc` pipeline and its own indexes for, over
standard LSP only:

1. **Hover with documentation**: after the type at the position (as
   before), the declaration's signature as the source writes it, `Defined
   in` its module and type, and its doc comment as Markdown, the same text
   `inko doc` reads (section 1).
2. **`textDocument/documentSymbol`**: a hierarchical `DocumentSymbol[]`
   from the syntax alone, which answers in code that doesn't type check,
   and (from a copy with lines blank) in code that doesn't parse
   (section 2).
3. **`workspace/symbol`**: the project's, its dependencies' and std's
   declarations, matched as clients filter, kept between requests, 6 to
   35 ms a keystroke on a project that imports all of std (section 3).
4. **Completion of names**: variables and arguments in scope, what `self`
   has, fields after `@`, the module's own and imported symbols, types in
   a type's name, modules and symbols in an `import`, and public symbols
   of modules not imported yet, with an `additionalTextEdits` that adds the
   import as `inko fmt` would (section 4).

"Never a wrong answer" holds: the whole edit-replay corpus, now asking
document symbols at every edit and completion at the ends of names, has no
wrong answer in any mode (section 5). Every document symbol in std and the
fixtures is found by workspace symbols at the same place, and is its own
definition.

**Tests:** 342 (39 unit, 286 integration, 17 latency), up from 305.
`scripts/mutate.py` has 379 mutations, 76 of them new. **The patch didn't
change**, and there's no new extension.

This milestone was requested: built by a coding agent with little or no
review.

## 1. Hover with documentation

docs/lsp.md, "textDocument/hover", is the reference.

```
```inko
Counter
```

---

```inko
fn pub static new(
  label: String,
  count: Int,
) -> Counter
```

Defined in `main.Counter`

---

Makes one.
```

- **Sections**, each that there is, separated by `\n\n---\n\n`: the type
  (as before, the first code block wherever the position is in an
  expression), the declaration, and the doc comment. Plain-text clients
  get the same sections without fences, separated by `\n\n`.
- **Which declaration:** the symbol definition finds at the name (by the
  compiler's ids), if it's a module, type, trait, method, field,
  constructor or constant. Variables and type parameters show their type
  only. On a declaration's own name (`describe` in `fn pub describe`),
  where there was no hover before, the declaration alone, with the name's
  range.
- **The signature as written** comes from the declaring file's syntax
  (`syntax::Declaration::signature`): the text up to the body, with the
  first line's indentation taken off the others. `Defined in` is the
  declaring module and what it's in; a method of an `impl` block is in
  its type.
- **Doc comments, as `inko doc` reads them.** A check doesn't parse
  comments (`ModulesParser::new`), so the HIR's `documentation` is empty
  outside `inko doc`. Rather than patch that, src/syntax.rs parses with
  comments and mirrors upstream's rule exactly (`Comments`,
  compiler/src/hir.rs:39-90, and the module's,
  compiler/src/hir.rs:1320-1340, both in tests/citations.rs). An
  implementation of a trait's method without its own has the trait's, as
  `MethodId::documentation` gives it (types/src/lib.rs:3249-3262), through
  `original_method` in the type database.
- **From another check** (repaired copy, last good check, past a
  declaration's error), the text the declaration was read from (its doc
  comment and signature, and the trait method's) must be unchanged, with
  the line before it: a line typed right above a doc comment may be a
  comment.

**Tests:** tests/hover.rs over LSP in both encodings: the project, a
dependency, std (`Stdout`), a constant, a field, a constructor, a trait's
default method, an implementation with its trait's doc comment, a module,
a declaration's own name, a variable (type only), a position after
non-ASCII text, plain text, and a stale hover refused once the doc comment
it read changed. tests/syntax.rs runs the installed `inko doc --private
--dependencies` on a copy of a fixture and compares every documented
symbol's doc comment, 3,438 of them across the fixture and the std
modules it imports.

## 2. Document symbols

src/syntax.rs reads a module's declarations from the upstream parser's
syntax tree: types (Class), enums (Enum), traits (Interface), `impl`
blocks (Object), methods (Method), module methods (Function), fields
(Field), constructors (EnumMember) and constants (Constant), nested, with
the declaration's range, its name as `selectionRange` (a field's with its
`@`, as definition gives it), and its signature on one line as `detail`.
The server answers at once, without the worker. Clients without
hierarchical support get `SymbolInformation[]` with `containerName`; those
without the kinds past Array get Class and Constructor instead.

**In code that doesn't parse**, the outline is a copy's with lines blank:
the line of a bracket nothing closes, then in turn the line the parser
stops at or the line before it (whichever lets it get further; the one it
stops at on a tie), until a copy parses. Bytes are moved back to the text
by line, so ranges are the text's own. The corpus showed that a copy can
nest things differently from the text (a type's first line blank made its
methods module methods), so a symbol from a copy is given only where the
copy can't have changed it:

- not one whose signature has a blank line
- not a top-level one whose lines have another depth of braces in the copy
  than in the text, relative to its first line, or that starts deeper in
  the text
- not a top-level one after a blank top-level line that starts a type,
  trait or `impl` block (`type pub Foo` without its `{` yet)

The status says `repaired`. If no copy parses, the result is `null` with
`noHir`.

**Tests:** tests/symbols.rs over LSP in both encodings (every kind, ranges
sliced from the text, non-ASCII, a file not open, a statement and a
signature half typed, a document that isn't a file, `\r\n`, text no copy
parses, clients without hierarchy or kinds). tests/syntax.rs checks names
and ranges against the compiler's own declarations
(`walker::declarations`) in every module of std and three fixtures.

## 3. Workspace symbols

src/index.rs keeps every module the projects can import: std's, each
project's `src/` and `test/`, and each dependency's `src/` in `dep/` as the
project's `inko.pkg` says the compiler finds it (`Packages::source_directory`,
compiler/src/state.rs:98-135). Before each answer it stats every file and
reads again only what changed (or is open with other text), and drops
deleted files. Matching is the query's characters in order, ignoring case;
ranking is whole name, start, run, then the rest, same case first, then the
projects' own, dependencies', std's. At most 500 results, each located at
its name, with `containerName` its module and what it's in.

`Manifest::load` opens the manifest with `create(true)`: the first version
wrote an empty `inko.pkg` into every project without one. A test caught it
(empty files appeared in the fixtures); only an existing manifest is read
now, and a test checks no manifest appears.

**Tests:** tests/symbols.rs (a project copied with a dependency and a test
module: ranking, case, fuzzy, fields, constructors, std; unsaved methods,
files created, changed and deleted; clients without kinds), a unit test of
re-reading, and tests/corpus_symbols.rs: every document symbol in std and
four fixtures (over 3,000) is found by workspace symbols at the same place,
and definition at its name goes to it.

## 4. Completion of names

Completion away from a `.` inserts the same placeholder after what's
typed, and the check says what's in scope there (`query::names_in`), as
the compiler resolves a name without a receiver (`identifier` and
`constant`, compiler/src/type_check/expressions.rs:2544-2772):

- **Variables**, from the walker, which now records where each can be
  referred to: a `let`'s from after it to the end of its block, a method's
  or closure's arguments in it, a `match` case's bindings in that case.
- **What `self` has**: `members` on the method's receiver, as `self`:
  instance methods (traits' too, private ones), a static method's static
  methods, a module method's module.
- **What the module binds**: its symbols, imports and the prelude's.
- **After `@`**: the fields of the method's type, only.
- **In a type's name**: types, traits, modules, and the type parameters of
  the method and its type. The placeholder in a signature is an undefined
  type, a declaration error: errors at a placeholder don't count, and the
  method's unit (whose body no longer types) may answer names there.
- **Not code** (a comment, a string): `null`. An expression or type name
  must start where the name does.

The server adds the **symbols to import**: public types, traits, methods
and constants of modules the file can import and doesn't, matching what's
typed, at most 100, each with `additionalTextEdits` that import it as
`inko fmt` orders imports (compiler/src/format.rs:578-620): into the
module's list in order (`self` first), `(self, Name)` after an import of
the module alone, or a new line in the first group by path, or before the
first declaration and its doc comment. A project's `test/` modules only
from its tests, a dependency's only from its project. `isIncomplete` is
`true` for names. `@` is a new trigger character.

**In an `import`**, the server answers from the index at once: the next
name of importable modules, or in a symbol list, `self` and the module's
public symbols not listed yet.

**In code being edited**, consistent with milestones 5 and 8: names come
from the documents as they are; from a repaired copy or past a
declaration's error only where the guard allows (as members) *and* every
change and error is inside a method's body, since any other may bind
another name or none; never from the last good check (items are inserted
into code). docs/lsp.md has the table.

**Tests:** tests/names.rs over LSP in both encodings, unit tests for
import edits and import contexts, and the corpus.

## 5. The corpus

The whole edit-replay corpus (20 scenarios), Apple M1 Max, after this
milestone. Document symbols are asked after every edit; completion of
names at name ends (2 per edit, every third edit).

| Request | Asked | As the server runs | Without older checks | Without repaired copies | Wrong |
|---|---|---|---|---|---|
| hover | 21,047 | 11,902 (11,904 before) | 10,355 (10,358) | 9,892 (9,896) | 0 |
| completion, names | 497 | 265 | 214 | 148 | 0 |
| documentSymbol | 483 | 457 (217 repaired) | 457 | 457 | 0 |

The other requests are as milestone 11 measured them.

- **No wrong answer** in any mode. A mid-edit document symbol is wrong if
  the final text has no symbol with its kind, names around it, detail and
  ranges, where its name's text survived. Symbols a repaired copy leaves
  out are counted apart: 384 in all.
- **Hover lost 2 to 4 answers per mode**: where an edit touched the line
  right before a declaration, the declaration's text check refuses (a line
  typed there may be a comment). The floors were lowered to those counts.
- Before the nesting rules in section 2, the corpus found 9 wrong document
  symbols: methods given as module methods after a type's first line was
  blank.

## 6. The 0.12.0 release

The tag `v0.12.0` on b16c379 ran the workflow (run 40300858987): each of
the four builds in 2 to 3 minutes, publish 13 s, each check of the
published release 14 to 19 s. All passed.

| Target | Archive (0.11.0) | Archive (0.12.0) |
|---|---|---|
| `aarch64-apple-darwin` | 5,470,184 bytes | 5,607,052 bytes |
| `x86_64-apple-darwin` | 5,730,947 bytes | 5,877,363 bytes |
| `x86_64-unknown-linux-musl` | 15,863,260 bytes | 16,297,659 bytes |
| `aarch64-unknown-linux-musl` | 13,445,044 bytes | 13,873,568 bytes |

**Checked from outside**, on this Mac, from the latest URLs alone:

- `scripts/dist-check-index.py .../latest/download/index.json --newest
  0.12.0`: three releases, 0.12.0, 0.11.0 and 0.10.0, each with Inko
  0.21.1, each manifest agreeing.
- `scripts/dist-check-release.py .../latest/download/manifest.json
  --inko` (the real Inko 0.21.1): every archive downloaded with its size
  and SHA-256; the ARM64 macOS build ran with the real Inko, both Linux
  builds in Alpine 3.22 and Ubuntu 24.04. The Intel macOS build can't run
  here (no Rosetta), as in milestones 10 and 11, so that one was checked
  by size and SHA-256 (`--partial`); CI's `macos-15-intel` runner ran it.
- The published ARM64 macOS binary, from a scratch directory (nothing
  installed), answered the new requests on a copy of the symbols fixture:
  document symbols, workspace symbols, a hover with the multi-line
  signature and doc comment, and completion with an import edit.

docs/releases.md's example manifest shows the published ARM64 macOS build.

## 7. Measurements

Apple M1 Max, under `cargo test` (dependencies optimized). This machine
was loaded by other work for most of the milestone (load averages 20 to
180 on 10 cores), so these are upper bounds; the milestone tier's strict
latency step passed at lower load.

| What | Measured | Limit (tests/latency.rs) |
|---|---|---|
| document symbols, std's largest module (110 KB) | 30 to 41 ms | 150 ms |
| the same with a line that doesn't parse | 80 to 85 ms | 150 ms |
| workspace symbols, first request (reads and parses all of std) | 220 to 230 ms | 500 ms |
| workspace symbols, each keystroke after | 6 to 35 ms | 50 ms |
| completion of a name, std-sized project, with imports | 150 to 170 ms (median) | 500 ms |

Positions for many ranges are converted with a line table
(`position::Positions`): converting each from the start of the text made
document symbols of the largest module take 165 ms.

## 8. Tests and mutations

| File | What |
|---|---|
| tests/syntax.rs (new) | outlines against the compiler's declarations (std, fixtures) and `inko doc`'s doc comments |
| tests/symbols.rs (new) | document and workspace symbols over LSP |
| tests/corpus_symbols.rs (new) | document symbols are workspace symbols and their own definitions |
| tests/hover.rs (new) | hover with declarations over LSP |
| tests/names.rs (new) | completion of names over LSP |
| tests/replay.rs | document symbols and completion of names in the corpus, hover with declarations |
| tests/latency.rs | four new limits |
| src/syntax.rs, src/index.rs, src/position.rs | unit tests: repairs, nesting, doc comments, import edits and contexts, owners, re-reading, positions |
| tests/common/spec.rs | conformance gates for document and workspace symbols |

76 new mutations: document symbols and outlines (26), workspace symbols
(14), hover (9), completion of names (26), and converting positions (1). Running them found four
mutations no test caught, and four tests were made sharper:

- a stale hover's text check and the doc comment's place in it: the
  first test edited a comment inside a type, which the general guard
  refused anyway; it now edits a constant's doc comment in words no code
  mentions
- names from a copy with a declaration blank: the first case blanked a
  field of a type the method mentions; it now blanks a new method's
  signature no code mentions
- a placeholder's own error: the mutation was equivalent (the placeholder
  is only ever in the asked file), and now keeps every error
- the stale completion mutation now adds `Question::Complete`, which
  completion asks since this milestone

Two brace-depth rules and the doc comment rules each got unit tests after
their mutations survived.

**The tiers.** scripts/mutate.py, tests/main.rs and tests/common/spec.rs
changed, so the exhaustive tier is required (AGENTS.md); it runs
everything the milestone tier does. It runs on this report's first
commit, and the next commit says how it went.

## Every patch line added, and why

None: the patch is unchanged, and patches/README.md with it.

## What clients can stop doing

1. **Running `inko doc` for documentation.** Hover gives the signature as
   written, where it's defined, and the doc comment (inherited from a
   trait where `inko doc` would), for std, dependencies and the project,
   with no build, in unsaved text, and in code that doesn't type check
   where the guard allows. A client that renders its own documentation
   can split the hover's Markdown at `\n\n---\n\n`.
2. **Keeping an index of definitions for navigation and search.**
   `workspace/symbol` covers the project, its dependencies and std, fresh
   with disk and open documents, and works when the project doesn't type
   check or parse.
3. **Parsing files for an outline or structure view.**
   `textDocument/documentSymbol` gives nested declarations with exact
   ranges and signatures, also in code being typed (`repaired` says it
   came from a copy).
4. **Its own completion of names and auto-import.** Completion lists
   what's in scope, never what isn't, and imports with an edit as `inko
   fmt` would order it; `import` lines complete modules and symbols.
5. **Mapping its own module names to files** for imports: the index and
   completion do it, including dependencies by `inko.pkg`.

What a client still needs: rendering (Markdown to its own UI), and
anything the server leaves out of scope: formatting, folding, code
actions, rename, hierarchies.

## Decisions you may want to revisit

- **Hover's layout** puts the type first, so a client reading the first
  code block keeps working. Parakeet's current parser takes the whole
  value, and will need the first section.
- **Doc comments come from the syntax, not the patch.** Mirroring
  upstream's rule needs no patch line; tests/syntax.rs compares with `inko
  doc` itself, and tests/citations.rs pins the rule's lines.
- **Document symbols in broken code are partial on purpose**: typing `if
  x {` without its `}` leaves out the enclosing type until it's closed.
  Right, but an outline view will flicker.
- **Workspace symbols include private declarations** of std and
  dependencies (the corpus check requires every document symbol to be
  found), and rank the project's first.
- **Names in code being edited** are refused whenever any declaration
  differs, even far away. Narrower rules (only names a change binds) would
  answer more; this one is easy to argue.
- **Symbols to import**: public only (private ones in the same root
  namespace could be imported too), at most 100, none until a character
  is typed.
- **`import` completion** is line-based: an import's path must be on the
  line that starts with `import`.

## Remaining unknowns

1. **Very large workspaces.** The index holds every module's text and
   outline in the server; a workspace with many projects and large
   dependencies wasn't measured.
2. **Completion of names in a type's name outside a method** (a field's
   type) lists no type parameters, since only methods record them.
3. **A closure argument of a call that doesn't resolve** has no variable
   id, so it's not listed (never wrongly).
4. **The strict latency limits** were checked on a busy machine; a quiet
   run would give better numbers for docs/lsp.md.
