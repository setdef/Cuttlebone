# Feasibility spike report

Question: can a Rust language server get Inko's inferred types by linking the
upstream compiler crates from a pinned release, with a small private patch
kept out of tree?

**Answer: yes. Go, with the conditions below.**

## What the patch does

`patches/upstream.patch`, against v0.21.1 (details and rebase recipe in
[patches/README.md](patches/README.md)):

| Part | Files | Lines |
|---|---|---|
| Hook: `pub mod hir`, plus `Compiler::check_with(file, inspect)`, which calls `inspect(&Database, &[hir::Module])` after type checking and before MIR, even when type checking fails. `check` now wraps it. | compiler.rs, lib.rs | +13 / −3 |
| Visibility: `pub(crate)` → `pub` throughout hir.rs (except `LowerToHir`) | hir.rs | 469 changed lines, mechanical |
| **Total** | **3 files** | **+483 / −471** |

Nothing in the type checker or the types crate changed. The patch applies
cleanly to a fresh v0.21.1 clone, and `scripts/upstream.sh` reapplies it.

## Measurements

Corpus: std at v0.21.1, 124 modules and 60,431 lines in src/, plus 98
modules and 32,346 lines in test/. Timings are on an Apple M1 Max with a
release build, as the median of 10 runs.

### Share of expressions with a type

| Corpus | Expressions | With a type | Without |
|---|---|---|---|
| std/src | 65,512 | 65,492 (99.969%) | 20 |
| std/src + std/test | 190,448 | 190,428 (99.989%) | 20 |

No type is partial (`?` or `<unknown>`) or an error. In std/src, the type
comes from three places:

- **Stored** (60,558): read from the HIR node, or from its `CallInfo`,
  `FieldInfo` or `VariableId`.
- **Derived** (1,110): `loop`, `break`, `next` and `try`. The type checker
  computes these types but doesn't store them, so the walker applies the
  same rule (e.g. a loop is `Nil` if something breaks out of it, otherwise
  `Never`; `try` gives the Option or Result's inner type).
- **Symbol** (3,844): types and modules used as static receivers
  (`ByteArray.filled`, `Option.Some`, `libc.X`, `tls.Server`). The type
  checker returns `Owned(Type(id))` for these without storing it
  (type_check/expressions.rs:2579 and :2714), so the walker looks them up
  again in the module's symbol table.

The only expressions without a type are the 20 operands of
`mut fn_name` (all in std/net/tls.inko), which take a pointer to a C
function. The name is a method, not a value. The `Mut` node has the type
(`Pointer[Uint8]`) and the method is in `Mut.pointer_to_method`. These are
allow-listed in `tests/corpus.rs` as the `PointerToMethod` kind.

### Wall time

| Check | Modules type checked | Type check (in-process) | + MIR lowering | Process wall time |
|---|---|---|---|---|
| all of std/src | 124 | 120 ms | 270 ms | 346 ms |
| std/src + std/test | 222 | 235 ms | 517 ms | 735 ms |
| one module: std/cmp.inko | 53 | 29 ms | 62 ms | 74 ms |
| one module: std/string.inko | 53 | 36 ms | 68 ms | 81 ms |
| one module: std/net/http/server.inko | 90 | 73 ms | 163 ms | 195 ms |
| `inko check` (installed 0.21.1) on std, for comparison | | | | 504 ms |

Checking "one module" still type checks everything it imports, and that's
never less than the 53 modules std.init and the prelude pull in.

### Re-checking one changed module

The compiler can't re-check one module in place. `types::Database` is an
append-only arena (no `Clone`, no removal), and type checking is pass-major:
every module goes through `DefineTypes`, then all go through the next pass,
and so on, with a `&&` chain that stops at the first pass with errors.
`compiler/src/incremental.rs` only tracks dependencies for caching object
files.

What does work, without more patching:

- **The changed module's own types**: re-check the module and its imports
  (`check_with(Some(file))`), which takes 70–200 ms in std. The module's
  types don't depend on the modules that import it.
- **Diagnostics in modules that import it**: re-check the whole project in
  the background. That's under 0.3 s for something std's size.

Real incremental checking would need upstream redesign, which is off the
table. At these speeds, a full re-check after a debounce is good enough for
projects up to several times std's size.

### Code with errors (fixture outside the repo, 56 expressions)

| Error | Expressions with a type |
|---|---|
| none | 56/56 |
| unknown method in a method body | 55/56. Only the bad call lacks a type. |
| undefined type in a signature | 3/56. The pass chain stops before `Expressions`, so no method bodies anywhere get types. |
| syntax error | 0. HIR lowering bails, so there's no HIR at all. |

## JetBrains LSP API vs LSP4IJ

Parakeet declares `sinceBuild = 252` with no upper bound, and depends only
on `com.intellij.modules.platform`. That makes it installable in every
IntelliJ-based IDE from 2025.2 on, and its verifier targets IDEA Community
2025.2.6.3 and IDEA 2026.2.3.

- **JetBrains LSP API** (`com.intellij.modules.lsp`) is available in:
  - IntelliJ IDEA Ultimate, with or without a subscription since 2025.2
  - the unified IntelliJ IDEA (2025.3 and later), including the free tier
  - the commercial IDEs
  - unified PyCharm without Pro (2025.1 and later)

  It is **not** available in IDEA Community 2025.2 (a JetBrains staff
  answer: "For Community nothing changes and we don't provide LSP there"),
  in IntelliJ open-source builds, or in Android Studio. So it doesn't cover
  every IDE Parakeet targets, including one of its own verification
  targets.
- **LSP4IJ** (Red Hat, open source) closes that gap. The current release,
  0.21.0, supports 2024.2 and later in every product the Marketplace lists:
  IDEA, IDEA Community, PyCharm Community, Android Studio, RustRover,
  WebStorm, CLion, GoLand, Rider and others. Plugins declare servers through
  the `com.redhat.devtools.lsp4ij.server` extension point. The cost is that
  users have to install a second plugin.
- Your options:
  - LSP4IJ only: one client everywhere.
  - Native API, plus raise `sinceBuild` to 253 and accept losing Android
    Studio and open-source builds.
  - Both, each behind an optional `<depends>`. That's two integrations to
    maintain.
- A fourth option: Parakeet only needs types. It could call `cuttlebone`
  over a small JSON protocol of its own, the way it already calls `inko`,
  with no LSP client at all. LSP would then only matter for other editors.

## Remaining unknowns

1. **Code that doesn't parse or has declaration errors** gets no types at
   all. The server will have to keep the last good result for each file and
   map positions across edits. Making the compiler keep going after a
   failing pass would be a much bigger and riskier patch.
2. **Unsaved buffers.** `ModulesParser` reads source files from disk.
   Either mirror the project into a temp directory, as Parakeet does for
   `inko check`, or extend the patch with an in-memory overlay (likely about
   20 more lines in modules_parser.rs).
3. **Diagnostics.** `Diagnostic`'s accessors are `pub(crate)`. Making them
   public is a few more patch lines; the alternative is the JSON presenter.
   MIR lowering also reports errors (e.g. moves), so a full check still
   runs it.
4. **LLVM.** The compiler crate links LLVM 18 through inkwell, even though
   Cuttlebone never generates code. `dump-types` is 15 MB, and on macOS it
   links Homebrew's zstd and libunwind dynamically, so distribution needs
   per-platform builds like inko's own. Making inkwell optional would be a
   far larger patch.
5. **Version skew.** One Cuttlebone build corresponds to one Inko release,
   and its std must match. You'd need a build per Inko version (selected via
   ivm or `inko.pkg`) or a clear refusal on a mismatch.
6. **Positions.** I haven't checked whether HIR columns count bytes or
   characters. LSP uses UTF-16 code units.
7. **Synthetic nodes.** Desugaring (array literals, string interpolation,
   `for` loops) produces several nodes that share a source range. For
   example, `[1, 2]` yields a Scope, a `$array` variable and push calls.
   Hover will need a rule for picking the node the user means.
8. **Everything beyond types.** Go to definition and find usages should
   work from the ids the HIR stores (`method_id`, `CallInfo.id`,
   `VariableId`, `FieldInfo`), but I haven't measured them.
9. **Robustness.** Compiler panics on bad input would take down a
   long-lived server. Run checks under `catch_unwind` or in a child process.
10. **Larger projects.** The only corpus measured was std (about 93,000
    lines with tests).

## Recommendation

**Go.** The patch is small, mechanical where it's large, and touches no
compiler logic. Coverage of real code is effectively complete, and a full
check of std is faster than `inko check`. The hard part of the full server
isn't getting types. It's the error-recovery story (unknown 1) and
packaging (unknowns 4 and 5), so those should be the first milestones,
before building LSP features on top.
