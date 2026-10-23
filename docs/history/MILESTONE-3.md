# Milestone 3: exact locations

**Done.** Hover gives a type only as a display string, and a name doesn't
identify a symbol: two modules can each define `Error`. Cuttlebone now
answers three standard LSP requests with exact locations, found by the
compiler's own ids:

- **`textDocument/definition`:** where the symbol at a position is declared
- **`textDocument/typeDefinition`:** where the declared type of the
  expression at a position is, with ownership and type arguments stripped
- **`textDocument/references`:** every reference to the symbol across the
  project, honouring `includeDeclaration`

docs/lsp.md documents all three, with examples. Nothing in them assumes a
particular client. Parakeet, which will use definitions to resolve
`x.method`, is one client among many.

**The patch didn't grow.** The typed HIR already holds every declaration's
id next to its name's location, and every reference's id. The type
database's own locations weren't usable: they cover whole declarations
(`fn` to `}`), and traits get `Location::default()`
(compiler/src/type_check/define_types.rs, `define_trait`).

**Tests:** 175, up from 133, all passing under plain `cargo test`.
`scripts/mutate.py` now has 69 mutations, 27 of them new, and every one is
caught.
`scripts/check.sh --full` passed on the commit with this report, in
373 s (the 69 mutations are most of it; milestone 2 and the guardrails took
181 s for 42).

## What was built

### Symbols, references and declarations (src/walker.rs)

- **`SymbolId`** is one of the compiler's ids: a module, type, trait, type
  parameter, constant, method, field, constructor or variable. Two names are
  the same symbol only if their ids are equal. It compares by kind and
  number, because upstream's `ConstructorId` isn't `Hash`.
- **`SymbolId::canonical`** merges ids that stand for one declaration:
  - an enum constructor's static method (`Option.Some(42)` calls a method
    that `define_constructor_method` generates) is the constructor, which
    patterns (`case Some(x)`) refer to directly
  - a default method a type inherits from a trait without redefining it is
    a copy (`MethodSource::Inherited`), and becomes the trait's method
  - a type parameter copied for an `impl ... if T: Trait` bound becomes
    the original (`TypeParameterId::original`)
- **References.** The walker, which already visits every expression for
  hover, now also records every name that refers to a symbol, at the
  name's own location:
  - calls, including operators, setters (`a.b = c` calls `b=`) and static
    calls
  - variables, and assignments to them
  - fields: `@x`, `point.x`, field assignments, `x:` in `Point(x: 1)`, and
    field patterns
  - type names anywhere in signatures (arguments, returns, type parameter
    requirements, fields, constructor members, `impl` and `trait` headers,
    bounds), in casts, closures, `let` annotations, type arguments and
    `size_of`
  - constants, constructors (in calls and patterns), and types or modules
    used as receivers, with any module qualifier (`beta.Error`) as a
    separate reference to the module
  - imports: each segment of the path that names a checked module, and
    each imported symbol and its alias
  - named arguments, which refer to the method's argument
- **Desugaring adds no references.** Generated `$`-names, the calls
  `for` loops and string interpolation add, the `==` that `!x` becomes,
  and the `Some(value)` pattern of a `for` loop are left out. The
  variables in that pattern are kept: they're written in the source.
- **Declarations** (`walker::declarations`) come from each module's top
  level, without visiting method bodies: types, traits, type parameters,
  fields, constructors, constants, methods, and methods' arguments. The
  type checker defines arguments as variables in the HIR's order, so they
  pair up by index (and name, as a check). The walker adds the variables
  that patterns and closures bind.
- **`type_symbol`** gives the declaration of a type:
  `TypeRef::as_type_enum` strips ownership and the instance's type
  arguments. `Pointer[T]` is built into the compiler, so it has none.

### The queries (src/query.rs)

- **`Index`** maps each symbol to the file and name location of its
  declaration, from every module the check type checked, plus the walked
  file's local variables. Modules map to an empty range at their file's
  start.
- **`symbol_at`** picks the innermost name containing a position, among the
  walked file's references and declarations.
- **`definition`** looks the symbol up in the index. On a declaration's own
  name, that's the declaration itself.
- **`type_definition`** takes the expression hover would pick, and its
  `type_symbol`. Where no expression is, a type name (as in a signature) is
  its own type.
- **`references`** checks the **whole project** (its `src/` and `test/`),
  not just the file and what it imports, so modules that never import the
  file are searched. It walks each project module and collects the names
  with the same `SymbolId`. Dependencies and std aren't searched, and a
  declaration there isn't included even with `includeDeclaration`. Named
  arguments mean even a variable can be referred to from another module,
  so variables are searched for everywhere too.
- **`Scope`** replaces the five arguments every query's closure took, and
  adds the checked modules and a cache of other files' text. A location in
  another file is converted with that file's lines, in the request's
  encoding.

Everything else is shared with hover: the same check, the same worker,
timeouts, cancellation, debouncing, and the same `Unavailable` reasons. So
broken code gives `null` with the `cuttlebone/status` reasons of milestone 2
(`noHir`, `declarationErrors`, `untyped`, ...).

### The server (src/lsp.rs, src/worker.rs)

- `initialize` advertises `definitionProvider`, `typeDefinitionProvider` and
  `referencesProvider`, all `true`.
- The worker has three new `Query` variants, and two new `Outcome`s (one
  place, or many).
- Definition and type definition answer one `Location` or `null`.
  References answer an array, `[]`, or `null`.
- References read `context.includeDeclaration`. A request without a
  context gets `false`.
- `cuttlebone/status` is sent for all five queries.

### Locations into std and dependencies

A location's file is the path the compiler read the module from, so it's
always what the user compiles against:

- std files under the directory `inko print std` printed (the installed
  Inko's)
- dependencies under the project's `dep/`

Nothing points into `.upstream/`. Two tests prove it over LSP:

- **`locations_in_std_are_in_the_installed_std`** copies std into a
  temporary "installed" Inko, with a fake `inko` that prints the copy.
  Definitions and type definitions into std land in files under the copy,
  which exist and cover the right name.
- **`locations_in_dependencies_are_in_the_projects_dep_directory`** builds a
  project with an `inko.pkg` that requires a package, and the package in
  `dep/<hash>/v1/`, where the compiler looks. Definitions land there.

### Editors

Both smoke checks (`editors/check.sh`) now also go to a definition: from
the `e` in a new fixture line, `let f = '日本' + e`, to the `e` that `let e`
binds on line 12. `日本` is 6 bytes but 2 UTF-16 units. In the wrong
encoding, the position is past the end of the line and there's no
definition. Neovim reads the `Location`'s range in UTF-8 bytes. VS Code asks
its own `vscode.executeDefinitionProvider`, and accepts a `Location` or a
`LocationLink`. editors/README.md says what to press by hand (F12 and ⇧F12;
`<C-]>` and `grr`).

Both pass: Neovim 0.12.6 (headless, and under `cargo test`) negotiates
UTF-8, VS Code 1.127 UTF-16. I checked that each fails when it should: both
with a wrong expected line, and Neovim with a position that has no
definition (`neovim: FAIL: no definition`).

## Tests

| File | What |
|---|---|
| tests/definition.rs (30) | The core API on the navigation fixture: every kind of name (including a qualified constant), std, inherited methods, named arguments, a declaration's own name, `null` cases; type definitions (ownership, type arguments, type parameters, type names, tuples, closures); references (same-named types and methods in different modules, variables, constructors in patterns and calls, fields, std symbols, modules that don't import the file, `null`); broken code; both encodings. |
| tests/navigation.rs (9) | The same over LSP, with the real binary: every definition case and every type definition case in UTF-16 and in UTF-8 (including a position and a range after non-ASCII text, and a range after non-ASCII text in another file), references with and without `includeDeclaration` and without a context, the status reasons for broken code for all three methods, the installed std, and `dep/`. |
| tests/corpus_definition.rs (1) | The corpus test (below). |
| tests/latency.rs (+2) | Definition and type definition on a std-sized project, and references across all of std, against hover's 500 ms limit. |
| tests/lsp.rs | The exact capabilities now include the three providers. The unknown-method test uses `textDocument/implementation`. |
| tests/editors.rs | The Neovim check's definition step. |
| tests/citations.rs | Three new upstream citations. |

The fixture, tests/fixtures/navigation, has two modules that each define
`type pub Error` with `new` and `message`, a trait implemented by a type in
a nested module (`geometry.shapes`), an enum, fields, a constant, imports
of modules and symbols, closures, a `for` loop, a match, a tuple, and
non-ASCII text before names.

### The corpus test

`every_reference_in_std_goes_to_a_declaration_of_its_id` type checks all
of std/src as a project, walks each of its modules, and for every
reference with an id:

1. `symbol_at` at the reference's own start position must select that
   reference's symbol
2. the index must have a declaration of the same id
3. the text at the declaration's location must be the symbol's name
4. the text at the reference must be the symbol's name too, allowing for
   `@` on fields, `Self`, compound operators (`+=` calls `+`), setters
   (`a.b = c` calls `b=`) and import aliases

There are 51,435 references in std/src: 11,550 methods, 12,346 types,
11,459 variables, 5,225 fields, 4,245 constructors, 2,142 modules, 1,856
traits, 1,481 type parameters and 1,131 constants. Each kind must have
more than 500, so a kind the walker stops recording fails the test. **The
allow-list is empty:** every reference resolves.

The first runs found four bugs, now fixed:

- `Pointer[T]` went to `T` (987 wrong references, with the next two)
- the `Some(value)` pattern of a `for` loop was a reference
- the `==` call that `!x` becomes was a reference, at the whole `!x`
- (in the fixture tests) named arguments in `Point(x: 1)` weren't recorded

## Measurements

Apple M1 Max, 64 GB, release build, median of 7, from the client's side.

| | |
|---|---|
| Hover, file importing all of std/src (116 modules) | 118 ms |
| Definition, same file | 125 ms |
| Type definition, same file | 109 ms |
| Completion, same file | 121 ms |
| References across std as the project (src/ and test/) | 273 ms |
| A whole-std type check alone (dump-types, src/ and test/) | 202 ms |

Definition costs a few milliseconds more than hover, for the declaration
index over every checked module. References cost a whole-project check plus
a walk of each project module, about 55 ms of walking for std's 200,000
expressions. The walk formats every expression's type for hover, which
references don't need. Skipping that is the obvious optimization if large
projects need it. In `cargo test`'s debug build of Cuttlebone (with the
compiler optimized), references across std take 359 ms.

## The patch

**No lines were added or removed.** patches/README.md is unchanged.
Everything comes from what the patch already exposes: the HIR's
`type_id`, `trait_id`, `method_id`, `field_id`, `constructor_id`,
`constant_id`, `type_parameter_id` and `variable_id` fields next to each
name's `Location`, and the call, field and variable info the type checker
stores.

Three new upstream citations, pinned by tests/citations.rs:

- `compiler/src/type_check/methods.rs:1030-1094`: constructor methods
- `compiler/src/type_check/methods.rs:1283-1291`: inherited default methods
- `compiler/src/type_check/methods.rs:198-204`: arguments defined in order

## The mutations

`scripts/mutate.py` applies each, runs the tests named for it, and restores
the file. All 69 are caught: milestone 2's 32, the guardrails' 10, and these
27 new ones:

- **Canonical symbols:** constructor calls not treated as constructors;
  inherited methods not followed to the trait; bounded type parameters not
  followed to the original.
- **What isn't a reference:** `Pointer[T]` as `T`; a `for` loop's pattern;
  a negation's `==`.
- **What is a reference:** named arguments; import paths; imported
  symbols; field reads; type names; static receivers (`Point` in
  `Point.new`); a qualified constant pattern covering its qualifier
  (`libc.EAGAIN`).
- **Declarations:** pattern variables, closure arguments, method arguments,
  modules.
- **Type definition:** keeping the instance instead of its type; ignoring
  type names; answering with the definition instead (in the worker).
- **References:** `includeDeclaration` ignored; only searching what the
  file imports; searching std and dependencies; converting other files'
  ranges with the requested file's lines.
- **The server:** no `definitionProvider`; `includeDeclaration` not read;
  no status for definitions.

One mutation taught me something. I thought the walker's `tail` (trimming
`mod.` off a qualified constant's location) was dead code, because the
fixture tests still passed without it: the parser gives a qualified
constant *reference* its name's location. Removing it broke the corpus
test, because a qualified constant *pattern* (`case libc.EAGAIN`) does
cover the qualifier. It stays, and its mutation now names the corpus
test.

## What clients should know

1. **Definition** returns one `Location` (never `LocationLink`), whose range
   is the declared *name*, not the whole declaration. A module's range is
   empty, at line 0, character 0 of its file.
2. **Type definition strips ownership and type arguments.** `ref
   Option[String]`, `mut Option[String]` and `uni Option[String]` all go to
   `Option`'s declaration in std.option. What's inside the brackets is lost:
   to reach `String`, ask on an expression of that type. Hover still shows
   the full type.
3. **Type definition of other types:**
   - a type parameter goes to its declaration (`T` in `[T: Area]`), not to
     whatever it's bound to at a call site
   - a tuple goes to std.tuple's `Tuple2`, `Tuple3` and so on
   - a closure, `Never`, `Pointer[T]` and C types return `null`
   - a trait-typed value goes to the trait
   - a type name in a signature goes to that type
4. **Matching is by id.** Two modules' `Error`s never mix, in definitions or
   references. That includes distinct ids for one name in one place: a call
   through a bound `T: ToString` refers to `ToString.to_string`, and a call
   on a `String` to `String`'s own `to_string`.
5. **References search the project only** (`src/` and `test/`), not `dep/`
   or std, and a declaration outside the project isn't included even with
   `includeDeclaration`. They type check the whole project, so an error
   that stops the type checker anywhere in it (a syntax error, or an error
   in a declaration) makes them unavailable.
6. **Locations into std and dependencies** are real files: the installed
   std (`experimental.cuttlebone.std`) and the project's `dep/`. Open them
   as they are. They may not be open documents.
7. **`null` means no answer**, never a guess: nothing at the position, a
   name that doesn't resolve, or no answer at all, which `cuttlebone/status`
   explains with the same codes as hover.
8. **For Parakeet's `x.method`:** send `textDocument/definition` at the
   method name. The result is the method actually called, after the
   compiler's own resolution: the implementation for a concrete receiver,
   the trait's method for a type parameter's bound, and the trait's default
   for an inherited method. Compare locations, not names.

## Decisions you may want to revisit

- **One `Location`, not `LocationLink`.** A link could carry the
  reference's own range (`originSelectionRange`) and the whole declaration
  (`targetRange`). The data is there (the HIR's node locations). Clients
  that offer `linkSupport` would benefit.
- **References exclude std and dependencies entirely**, including a
  declaration there. The alternative reading of `includeDeclaration` is to
  add the declaration wherever it is.
- **A declaration's name is its own definition**, rather than `null`.
- **Trait methods and their implementations are separate symbols.** Finding
  every implementation is `textDocument/implementation`, which is out of
  scope.
- **References re-check the whole project every time**, as everything does.
  273 ms for std is fine; much larger projects would want caching, which
  is out of scope.

## Remaining unknowns

1. **Names without a stored id** have no definition. That's what the corpus
   allows as "with a stored id". I didn't count how many such names std
   has (a call the type checker couldn't resolve has none, by design).
2. **Overlapping names.** `symbol_at` picks the innermost name, but in std
   no two recorded names overlap: the corpus test's selection check passes
   for all 51,435. No mutation covers the tie-breaking for that reason.
3. **Type patterns** (`case { @a = x }` on a type) record the field names,
   but not a type name: the HIR's `TypePattern` has a type id without a
   name location.
4. **Projects much larger than std** for references, and **Windows**, as
   in milestone 2.
5. **From milestone 2, still open:** lone `\r` line endings, answers in code
   with declaration or syntax errors (`stale` is reserved), packaging.
