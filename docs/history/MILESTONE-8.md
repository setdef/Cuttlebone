# Milestone 8: answers when a declaration has an error

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
