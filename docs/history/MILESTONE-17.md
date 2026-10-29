# Milestone 17: what a client needs to drop its own index of declarations

**Done.** Everything for 0.17.0 is in this report's commit; section 9
says how its release went.

Parakeet keeps its own index of Inko's declarations
(`InkoDefinitionIndex`, read by `InkoSourceDefinitions`), because
Cuttlebone 0.15.0 gave no module's file by its name, nor a declaration's
signature, doc comment, or whether it's static, public or a trait's
required method, with its workspace symbol (Parakeet's
docs/history/SESSION-late-edits-and-slimming-3.md, "Not done" and
"Requests for Cuttlebone"). This milestone gives all of it in standard
fields, with no new extension and no patch line (0.17.0):

1. **Modules as workspace symbols** (section 1): kind Module, named as an
   import names them, at the start of their files.
2. **Where each part of a declaration is** (section 2): docs/lsp.md's
   table, "What a client that indexes declarations gets, and where", with
   the rules for `pub` and `static` held to the type database for all of
   std. That found std's builtin types counted as private (section 3).
3. **What it costs, and cheaper** (section 4): document symbols of a
   module the index has read come from its kept outline, 2 to 4 times
   faster for a client that asks for every module's; limits for the
   whole index and for a hover at a declaration's name.
4. **Answers in a file of an installed std** (section 5): a real bug the
   measurements found: such a file was checked as a module named `main`,
   so most answers there (the hover a client asks for a doc comment,
   among them) were `null` after a compiler panic.
5. **The rest of the report's requests** (section 6), each checked
   against 0.16.0 before building anything: all answered by 0.16.0
   already.

## 1. Modules as workspace symbols

`workspace/symbol` lists every module the workspace can import, besides
its declarations: kind Module (2), `name` the module's name as an import
writes it (`std.string`, `app.util`; a test module by its path in
`test/`), no `containerName`, and `location` the start of its file with
an empty range, as definition locates a module. A module whose text no
copy of parses is listed too: its file is there. Matching and order are
as for any symbol, so a query of a module's whole name has it first; an
import's path resolves with one request and no knowledge of where
modules live (std's directory, `src/`, `test/`, `dep/<url>/v<major>/src`).

A declaration's module is the module symbol with the same `uri`:
`containerName` (`std.string.String`) can't say where the module's name
ends. That answers the report's "not the module of a declaration whose
container is a type".

Tests (tests/symbols.rs, `workspace_symbols_of_modules`, both encodings):
the project's, a test module, a dependency's, std's (`std.string`,
`std.net.http`), and a module of 20 lines that don't parse, each the
first result for its name; `every_workspace_symbol_for_an_index`: every
`.inko` file of std is a module symbol named by its path.

## 2. Where each part of a declaration is

| What | Where | How |
|---|---|---|
| every module, its name and its file | `workspace/symbol` | kind Module (2), `name`, `location.uri` |
| every declaration, its kind, name, place, and what it's in | `workspace/symbol` | `kind`, `name`, `location`, `containerName` |
| a declaration's module | `workspace/symbol` | the module symbol with the same `uri` |
| its signature as written | document symbols of its module | `detail`, on one line |
| whether it's public | the same | `pub` after its first keyword in `detail`, or a `type builtin` of std's other than `CheckedIntResult`; a constructor as its type |
| whether a method is static | the same | `static` among the words between `fn` and its name |
| whether a trait's method is required | the same | no ` { ... }` at the end of `detail` (0.16.0) |
| a type's fields, constructors and methods; a trait's methods | the same | `children` |
| its doc comment | hover at its name | the last section |

The two symbols are the same declaration when the workspace symbol's
`location` is the document symbol's `selectionRange` in the same `uri`.

**Why these, and no extension.** A `WorkspaceSymbol` has `name`, `kind`,
`tags`, `containerName`, `location` and `data`. `SymbolTag` has only
`Deprecated`. `data` is the server's, kept between `workspace/symbol` and
`workspaceSymbol/resolve`, not something a client is meant to read. And
`workspaceSymbol/resolve` fills in only fields a `WorkspaceSymbol` has;
Cuttlebone already sends every one, ranges included, so resolve would add
nothing and isn't advertised. `DocumentSymbol.detail` is the declaration
as written, which says `pub` and `static` as the code does; there's no
documentation field on any symbol, and hover is the spec's place for a
doc comment. So a client asks for document symbols once per module, not
per declaration, and for a doc comment when it shows one.

**Held to the compiler.** tests/corpus_symbols.rs now reads, for every
declaration in std (6,739) and four fixtures, `pub` and `static` from
`detail` as a client would by the documented rules, and compares them
with the type database's `is_public` and `is_static` for the symbol at
its name; and the syntax's own `public` (which completion's imports use)
too.

## 3. std's builtin types are public

That comparison found the 14 `type builtin` declarations of std
(`String`, `Int`, `Array`, `Tuple1` to `Tuple8`, ...) written without
`pub`, and counted private by the syntax, while the compiler makes 13 of
them public: a builtin type is the type database's own, created before
std is read, and `define_types` takes it as it is
(compiler/src/type_check/define_types.rs:69-81); the database makes all
but `CheckedIntResult` public (types/src/lib.rs:6258-6264). The syntax
mirrors that now, with both cited (tests/citations.rs). For a client:
completion of names imports `Tuple2` (`import std.tuple (Tuple2)`, which
`inko check` accepts) and never `CheckedIntResult`
(tests/names.rs, `symbols_to_import`, both encodings).

## 4. What it costs, and cheaper

Measured on the reference machine, an Apple M1 Max, over LSP against the
release build, for all of the installed std (124 modules, about 7,500
symbols), at load averages of 40 to 360 from other work the whole
session (so upper bounds):

| Step | Before | After |
|---|---|---|
| every workspace symbol, as partial results | 96 to 280 ms | the same |
| every module's document symbols, one request each | 120 to 543 ms | 76 to 214 ms |
| a hover at a declaration's name in std (median of 41 across std) | 16 of 41 answered | 38 to 67 ms, 41 of 41 |

Document symbols of a file the index has read (by any workspace symbol
request), with the same text, come from the outline it keeps instead of
parsing it again: the same function of the same text, compared before
use. tests/symbols.rs (`document_symbols_after_the_index_read_older_text`)
changes a module the index has read on disk, then opens it with other
text, then closes it, asking each time.

tests/latency.rs (`indexing_every_declaration_with_its_signature_is_fast_enough`)
asks, on the std-sized project, for every workspace symbol as partial
results and then each module's document symbols, three times, and for a
hover at every 150th declaration of std's: the index is held to 1 s
(median) and the hovers to 250 ms (median). Under `cargo test` at a load
of 36 per core (not strict), they took 4.8 s and 352 ms; the strict step
of the milestone tier is in section 8.

## 5. Answers in a file of an installed std

Inko installs std as `lib/inko/std/std/float.inko`. A query in such a
file (a client that went to a definition in std and hovers there) checks
it as the main module; the compiler names a main module from its path
relative to `src/` or `test/`, or a `src` or `test` in its path
(compiler/src/compiler.rs:51-73), and an installed std has none, so it
was `main`: not std, so `_INKO` intrinsics were errors, and the walker
then panicked looking for `std.float` in the type database. Answers were
`null` with a compiler panic as the reason. Measuring hovers at std's
declarations found it: 16 of 41 answered before, 41 of 41 after.
`.upstream/`'s std, which the tests used, has `src/` in its path, which
is why nothing caught it.

A check of a file in std that isn't in the project's `src/` or `test/`
now names it from std's directory (`check::Target::config`).
tests/navigation.rs (`answers_in_a_file_of_an_installed_std`, both
encodings) lays std out as Inko installs it, with no `src/`, and hovers
in `std.float` (the type of an intrinsic's argument, and a declaration)
and in `std.net.http.server`.

## 6. The rest of the report's requests, against 0.16.0

| Request (SESSION-late-edits-and-slimming-3.md) | 0.16.0 already | Built now |
|---|---|---|
| 1. signature, doc comment, static, public, required with the symbol | `detail` (signature, `pub`, `static`), ` { ... }` for default methods, hover's doc comment | the table and rules, held to the type database; builtin types; doc comments in installed std's files |
| 2. a trait's default methods, and required ones before an `impl` exists | the trait's document symbols, `detail` marking defaults (milestone 16) | nothing |
| 3. modules as workspace symbols | no | section 1 |
| 4. implementations, hierarchies, renames past a declaration error; diagnostics past a module that doesn't parse; batch gutter answers; statuses for every rename and by request | all in 0.16.0 (milestone 16, sections 1 to 4) | nothing |

The report's open correctness failure (`testSavedChangesAreKnown`) is in
Parakeet's own index, which this milestone lets it delete. Cuttlebone's
index looks at each file's size and modification time on every request,
and reads it again when they change, so a write that lands late is read
when it lands, not kept stale.

## 7. Also fixed

- `scripts/check-patch.sh` failed under `git commit -a`, which gives its
  hooks `GIT_INDEX_FILE` (its own index): `git -C .upstream/inko diff`
  read Cuttlebone's index as Inko's. Every git command there now runs
  without the repository's environment.

## 8. Tests and mutations

| File | What |
|---|---|
| tests/symbols.rs | modules as workspace symbols, every std module among them; document symbols after the index read older text |
| tests/corpus_symbols.rs | `pub` and `static` from `detail`, and the syntax's `public`, against the type database for all of std |
| tests/names.rs | `Tuple2` imported, `CheckedIntResult` never |
| tests/navigation.rs | answers in a file of an installed std |
| tests/latency.rs | the whole index, and hovers at declarations' names |
| tests/citations.rs | three upstream citations |

New mutations (scripts/mutate.py), each caught: modules not workspace
symbols, only modules that parse listed, a module's symbol of kind File,
named by its file's last name; document symbols from the index's older
text; builtin types as their declarations say, and every builtin type
public; an installed std's file checked as `main`. A first run found the
module that "doesn't parse" in the test parsed once a line was blank, so
dropping modules without an outline was uncaught; it has 20 such lines
now, past what a copy blanks. One older mutation's pattern pointed at
code this milestone moved, and was pointed at the new code.

**The milestone tier, locally**: `scripts/check.sh --milestone 27c9a15`
on b193426, 1 hour 45 minutes at load averages of 200 to 990:
formatting 0 s, clippy 15 s, tests 147 s, the patch 0 s, the release
build 38 s, the whole corpus 98 s, every test without kept checks 238 s,
the selection 5 s, the release tooling 45 s, and the strict latency step
(advisory) passed, 76 s (this Mac's baseline measured 258 ms, so every
limit as written). Its mutation step selected 290: 276 were caught, and
14 got no verdict, because the disk filled up (the mutation trees in
`target/mutate` had grown to 203 GB, on a disk other work shared): six
reported a build error, eight none. With the trees removed, those 14
ran again on their own (`scripts/mutate.py` with their names): all 14 caught (15 minutes). So every one of the 290 is caught.

## What Parakeet can now delete

`InkoDefinitionIndex` and `InkoSourceDefinitions`, with
`InkoDefinitionIndexFileListener`, `InkoDefinitionIndexWarmer`, and the
`invalidate` calls in `InkoConfigurable`, `InkoIvm` and
`InkoDependencies`. Cuttlebone's index is kept up to date by itself, on
every request, from the files and the open documents. Each reader, and
what replaces it:

| Reader | What it read | What replaces it |
|---|---|---|
| `InkoResolver` (an import's module and symbols; names where Cuttlebone has no answer) | modules by name, top-level declarations of a module, members of a type by name | definition (from a check); otherwise `workspace/symbol` by name: the module symbol of that exact name, its declarations by `uri`, a type's members by `containerName` |
| `InkoDocumentation` (quick documentation) | a definition's signature and doc comment | hover at the reference (or at the declaration's name: its symbol's `location`), whose sections are the signature, where it is, and the doc comment, now in installed std's files too |
| `InkoImplementMethods` (Implement and Override Methods) | a trait's required and default methods, their signatures | definition at the `impl`'s trait name, then that file's document symbols: the trait's children, `detail` the signature, ` { ... }` marking a default one |
| `InkoGenerate` (Generate → Implement Trait) | every public trait (or this module's), its module, the file's module | `workspace/symbol` of kind Interface; `pub` in each one's document symbol `detail`; module symbols by `uri` for both modules |
| `InkoIntentions`, `InkoParameterInfo` (`InkoCalls.parameters`: argument names) | a callee's parameters, a type's fields | definition at the callee, then its document symbol's `detail` (the signature) or its type's Field children; or signature help at the call |
| `InkoCompletion`'s fallback (members by name after `.`) | methods and fields by name | `workspace/symbol` by the name typed, kind Method or Field, `containerName` the type, `detail` from the module's document symbols; Cuttlebone's own completion first, as now |
| `InkoExpressionType.writable` (Introduce Variable's type and its imports) | the module of a type or trait by name | `workspace/symbol` by that name, kind Class, Enum or Interface, its module by `uri`; or completion of names, whose item has the import edit |
| `InkoQualifiedNames.libraryModule` (Copy Reference outside `src/`) | a std or dependency file's module | the module symbol with the file's `uri` |
| `InkoProjectRoots` (warming the index) | | nothing: Cuttlebone reads its index at the first workspace symbol request |

`InkoDefinition`'s fields map to: `kind` (SymbolKind, with
`STATIC_METHOD` from `static` in `detail`), `name`, `module` (module
symbol by `uri`), `owner` (`containerName` after the module's name),
`file`/`line`/`column` (`location`), `signature` (`detail`),
`documentation` (hover), `required` (no ` { ... }`), `public` (`pub`, as
in section 2).

A client that wants all of it at once asks for every workspace symbol as
partial results and then each module's document symbols: 0.2 to 0.5 s
for all of std on the reference machine (section 4), and the doc comment
when it shows one.

## Decisions you may want to revisit

- **No extension for visibility or doc comments.** The spec has no place
  in a workspace symbol; `detail` and hover are its places. A client
  reads `pub` and `static` from `detail` by the rules in docs/lsp.md,
  with one exception it must know (builtin types).
- **Module symbols have no `containerName`**, and are named by their
  whole name (`std.net.http`, not `http` in `std.net`), so that a lookup
  by an import's path is exact. A client that shows them lists the whole
  name.
- **A module symbol's range is empty, at the file's start**, as
  definition locates a module, rather than the whole file.
- **Two workspace folders can each have a module of a name**; both are
  listed, the client's own project's first by the usual order, and the
  client picks by location.
- **Document symbols reuse the index's outline only for identical
  text**, not by stamps: a stale index can never answer, at the cost of
  comparing the text.
- **The fix for installed std names any file under std's directory from
  it**, wherever the project is, which is what its importers call it.

## Remaining unknowns

1. **The machine was heavily loaded** (load averages of 40 to 360) the
   whole session; the measurements are upper bounds, and the first hook
   runs of two commits failed latency tests on load (a code lens across
   std timed out; a lookup ratio), each passing alone, named in its
   commit.
2. **Parakeet hasn't been tried against 0.17.0**; the tests are against
   the spec, its meta model and the client's capabilities.
3. **The edit-replay corpus** doesn't ask workspace symbols; module
   symbols come from file names, and document symbols' reuse is guarded
   by text equality and tested directly.

## 9. The 0.17.0 release

0.16.0, tagged outside the last session, was published by its release
workflow (run 40792250726, 38 minutes) before this milestone ended, so
the index lists it before 0.17.0.

The dry run and the release of 0.17.0 are recorded below once they ran.
