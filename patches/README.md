# The upstream patch

`upstream.patch` applies to the upstream compiler release named in
`../UPSTREAM_VERSION`. It's a modification of MPL-2.0 files, so it's
MPL-2.0 too. Publishing it here, next to the release tag it applies to,
makes the source of every modified file available.

No typing rule, nothing in the types crate, and nothing in code generation
changes. It adds one way into the compiler (a hook), one input (an overlay
of source text), and makes the typed HIR, the diagnostics and the
formatter's own document usable from outside. It also adds a setting that
runs every type checking pass after an error, which `inko check` leaves off, with four places in the passes that then
skip a declaration an earlier pass rejected, where they would panic. The counts are the patch's own
(`git apply --numstat`); tests/guardrails.rs fails if they disagree, so a
line can't be added without updating this table and justifying it below.

| File | Lines (added, removed) | Why |
|---|---|---|
| compiler/src/lib.rs | +1 -1 | `mod hir` becomes `pub mod hir`, so Cuttlebone can read the typed HIR. |
| compiler/src/hir.rs | +469 -469 | Every `pub(crate)` becomes `pub`, except `struct LowerToHir`. Mechanical. |
| compiler/src/compiler.rs | +43 -23 | The `check_with` hook, which doesn't sync `dep/` and passes the diagnostics too, the `diagnostics` accessor, and the pass chain running on after an error when asked. |
| compiler/src/diagnostics.rs | +7 -7 | `Diagnostic` and the accessors Cuttlebone reads become `pub`. |
| compiler/src/config.rs | +8 | The overlay field, and the setting to run every pass. |
| compiler/src/modules_parser.rs | +5 -1 | Reading the overlay. |
| compiler/src/type_check/define_types.rs | +5 -2 | Skipping an `impl` whose trait or type didn't resolve. |
| compiler/src/type_check/methods.rs | +5 -2 | The same, one pass later. |
| compiler/src/type_check/expressions.rs | +6 -6 | Skipping a method that was never defined, and a required trait that didn't resolve. |
| compiler/src/format.rs | +3 -3 | `Document`, its `new` and its `format` become `pub`, so formatting is `inko fmt`'s own, without its files or stdin. |

## Every line, and why

### compiler/src/lib.rs

```diff
-mod hir;
+pub mod hir;
```

The typed HIR is where the type checker stores each expression's type.
Without this, Cuttlebone can't name `hir::Module` or any of its nodes.

### compiler/src/hir.rs

`sed 's/pub(crate)/pub/g'`, then `struct LowerToHir` goes back to
`pub(crate)`, so the crate doesn't warn about a private type in a public
signature. The node structs' fields (`resolved_type`, `kind`, `location`, ...)
are what Cuttlebone reads. Making them public one by one would be a
hand-picked diff of about the same size that's harder to redo on a rebase.

### compiler/src/compiler.rs

```diff
     pub fn check(&mut self, file: Option<PathBuf>) -> Result<(), CompileError> {
         self.prepare()?;
+        self.check_with(file, |_, _, _| true)
+    }
 
+    /// Type checks like `check` but without `prepare` (which syncs `dep/`),
+    /// and passes the typed HIR and diagnostics to `inspect` before MIR, even if
+    /// type checking fails. Returning `false` from `inspect` skips lowering to MIR.
+    pub fn check_with<F: FnOnce(&types::Database, &[hir::Module], &[&crate::diagnostics::Diagnostic]) -> bool>(
+        &mut self,
+        file: Option<PathBuf>,
+        inspect: F,
+    ) -> Result<(), CompileError> {
         let start = Instant::now();
```

`check` keeps its behaviour (`inko check` is unchanged): it calls `prepare`
as before, then `check_with` with an `inspect` that does nothing and
continues to MIR. The signature and doc comment are the hook's interface.
`check_with` is the rest of `check`'s old body, so it does the same parsing,
lowering and passes.

The hook starts *after* `self.prepare()?`, which stays in `check`. This costs
no added line (milestone 1's hook started before it, and had one more
line), and it's what keeps queries free of side effects. `prepare` does two
things:

- It refuses a project whose manifests require a newer Inko. Cuttlebone
  repeats this check itself (`check_required_version` in src/check.rs), with
  absolute paths, because `prepare` reads `inko.pkg` relative to the working
  directory.
- It calls `sync_if_needed`. That function first creates
  `<data directory>/inko/packages` and takes a file lock in it, even when
  nothing needs syncing. It then downloads packages into `dep/` if
  `inko.pkg` changed since the last sync. A hover must not write to the
  user's data directory, wait on a lock that `inko pkg sync` may be
  holding, or touch the network. Dependencies are whatever the last
  `inko pkg sync` (or `inko build`) put in `dep/`.

Because of this, checks no longer depend on the working directory, so
Cuttlebone no longer changes it or serializes checks behind a lock.
`tests/lsp.rs` (`queries_never_sync_dependencies_or_touch_the_data_directory`)
runs the server in a project whose `dep/` was never synced, with a fresh
home directory, and fails if anything is created.

```diff
         let mut hir = self.compile_hir(ast)?;
+        let typed = self.check_types(&mut hir);
-        self.check_types(&mut hir)?;
+        if !inspect(&self.state.db, &hir, &self.state.diagnostics.iter().collect::<Vec<_>>()) {
+            return typed;
+        }
+
+        typed?;
```

- `let typed = ...` instead of `?`: `inspect` must also run when type
  checking fails, because a file with an error in one method body still has
  types everywhere else, and that's the normal state of a file being edited.
- `inspect(&self.state.db, &hir, ...)`: the type database and the typed
  HIR are everything a type answer needs, and both are dropped once
  lowering to MIR consumes the HIR. Passing them by reference keeps the
  compiler's ownership as it was.
- The third argument, the diagnostics reported so far: where the errors
  are, while Cuttlebone answers from the HIR (it holds `inspect` open for
  as long as it keeps the check). With every pass run after an error
  (below), some of what the HIR says depends on a declaration with an
  error, and Cuttlebone answers only where that can't matter (docs/lsp.md,
  "Errors in declarations"). `Compiler::diagnostics` can't be called
  during `inspect`, which borrows the compiler. Collecting the references
  into a `Vec` keeps `Diagnostics` and its iterator type `pub(crate)`.
- `if !inspect(...) { return typed; }`: types never depend on MIR, and MIR
  lowering is more than half of a check's time (spike: 120 ms of 270 ms for
  std). It also panics on some input the type checker accepts (a `break`
  inside a closure inside a loop; compiler/src/mir/passes.rs:1836), so
  skipping it takes that crash off Cuttlebone's path. `dump-types` and the
  golden tests still lower to MIR by returning `true`.
- `typed?;`: the original early return, for when `inspect` continues.

```diff
+
+    pub fn diagnostics(&self) -> impl Iterator<Item = &crate::diagnostics::Diagnostic> {
+        self.state.diagnostics.iter()
+    }
 }
```

Every error and warning a check reported, for `textDocument/publishDiagnostics`
(docs/lsp.md, "Diagnostics"). The compiler keeps them in `State`, which is
private, and only prints them (`print_diagnostics`, through the text or JSON
presenter). Reading them directly is the smallest way out; the alternative,
setting the JSON presenter and parsing what it prints to stderr, would mean
capturing the process's stderr and a parser for a format that isn't quite
JSON (it escapes strings with Rust's `{:?}`).

- It's the last method of `impl Compiler`, after `prepare`, so no line that
  Cuttlebone's code cites (tests/citations.rs) moves.
- It returns an iterator rather than `&Diagnostics`, so the `Diagnostics`
  collection and its builder methods stay `pub(crate)`.
- The full path, `crate::diagnostics::Diagnostic`, saves a `use` line.

```diff
-        let res = DefineTypes::run_all(state, modules)
-            && CollectExternImports::run_all(state, modules)
+        let all = state.config.check_after_errors;
+        let res = (DefineTypes::run_all(state, modules) || all)
+            && (CollectExternImports::run_all(state, modules) || all)
```
and the same `(... || all)` for each of the other 19 passes, then
```diff
-        if res { Ok(()) } else { Err(CompileError::Invalid) }
+        if res && !state.diagnostics.has_errors() { Ok(()) } else { Err(CompileError::Invalid) }
```

`Compiler::check_types` chains its 21 passes with `&&`, and each returns
whether no error has been reported yet, in any module (or `true`, for the
two that can't report one). So one error in a declaration anywhere stops
the chain before `Expressions`, the pass that types method bodies, and
no body in the check gets types. With `check_after_errors`, every pass
runs. The result is still an error if any pass reported one:
`res && !has_errors()` is what `res` was without the setting, since every
pass that can fail returns `!has_errors()`, and errors are never removed.
`inko check` doesn't set it, and neither do Cuttlebone's diagnostics,
which must match `inko check`'s exactly. Only Cuttlebone's queries do.

Running the passes after an error is safe for the passes as they are,
except where one takes an id that an earlier pass would have set, had it
not rejected the declaration (the four places below). Every declaration
pass already records what it rejects: an undefined type becomes
`TypeRef::Error`, and the method, field or argument is still defined.
docs/history/MILESTONE-8.md has the measurements: 838 declaration errors
made by breaking names and items in fixtures and std, with 142 panics
before the skips and one after (`type Option` renamed in std.option,
which the compiler requires). A panic is caught, and Cuttlebone then
checks again without the setting.

### compiler/src/config.rs (the setting)

```diff
+
+    /// Run every type checking pass, even after one reports an error.
+    pub check_after_errors: bool,
```
```diff
+            check_after_errors: false,
```

The default is `inko`'s behaviour.

### compiler/src/type_check: skipping what an error left undefined

Each of these turns an `unwrap` that panics into skipping the
declaration. Upstream never reaches them with the id missing, because the
pass that would leave it missing reports an error, and the chain stops.
No typing rule changes. A skipped declaration has an error already, and
Cuttlebone gives no answer that depends on it.

```diff
     fn implement_trait(&mut self, node: &hir::ImplementTrait) {
-        let type_ins = node.type_instance.unwrap();
-        let trait_ins = node.trait_instance.unwrap();
+        let (Some(type_ins), Some(trait_ins)) =
+            (node.type_instance, node.trait_instance)
+        else {
+            return;
+        };
```

define_types.rs, `CheckTraitImplementations`: an `impl` of a trait or
for a type that doesn't resolve (`ImplementTraits` reported it) has
neither instance.

```diff
     fn implement_trait(&mut self, node: &mut hir::ImplementTrait) {
-        let trait_ins = node.trait_instance.unwrap();
+        let (Some(trait_ins), Some(type_ins)) =
+            (node.trait_instance, node.type_instance)
+        else {
+            return;
+        };
         let trait_id = trait_ins.instance_of();
-        let type_ins = node.type_instance.unwrap();
```

methods.rs, `ImplementTraitMethods`: the same `impl`, one pass later.

```diff
-        let method = node.method_id.unwrap();
+        let Some(method) = node.method_id else { return };
```

expressions.rs, `define_instance_method`: a method in an `impl` that the
trait doesn't have is never defined (`ImplementTraitMethods` reported
it), so its body isn't typed. Milestone 5 found this one.

```diff
-            let req_id = req
-                .resolved_type
-                .as_trait_instance(self.db())
-                .unwrap()
-                .instance_of();
+            let Some(req_ins) = req.resolved_type.as_trait_instance(self.db())
+            else {
+                continue;
+            };
+            let req_id = req_ins.instance_of();
```

expressions.rs, `verify_required_traits`: a trait's required trait that
doesn't resolve isn't a trait instance. It's left out of the check for
conflicting methods between required traits, which it has none of.

### compiler/src/diagnostics.rs

```diff
-pub(crate) enum DiagnosticId {
+pub enum DiagnosticId {
```
```diff
-pub(crate) struct Diagnostic {
+pub struct Diagnostic {
```
```diff
-    pub(crate) fn is_error(&self) -> bool {
+    pub fn is_error(&self) -> bool {
```
and the same for `id`, `message`, `file` and `location`.

These are what `inko check --format json` prints (compiler/src/presenters.rs,
`JsonPresenter::to_json`): the ID, error or warning, the file, and the
location. `DiagnosticId` must be public because `id` returns it; Cuttlebone
only formats it with its `Display`, which gives the ID as `inko check`
prints it (`invalid-symbol`). The fields stay private, `new` and `kind`
stay `pub(crate)` (`is_error` says the same as `kind`), and so do
`Diagnostics` and `DiagnosticType`.

### compiler/src/config.rs (the overlay)

```diff
     pub generate_code: bool,
+
+    /// Source code to parse instead of these files' contents on disk.
+    pub overlay: IndexMap<PathBuf, Vec<u8>>,
 }
```
```diff
             generate_code: true,
+            overlay: IndexMap::new(),
         })
```

Editors type check unsaved buffers. The field is the input; its default is
empty, so nothing changes for the `inko` command. It's an `IndexMap` because
config.rs already imports one, which saves a `use` line.

### compiler/src/modules_parser.rs

```diff
     fn parse(&mut self, file: &PathBuf) -> Option<Module> {
-        let input = match read(file) {
+        let input = match self.state.config.overlay.get(file) {
+            Some(source) => Ok(source.clone()),
+            None => read(file),
+        };
+        let input = match input {
```

`ModulesParser::parse` is the one place source files are read, for the
checked file and every module it imports. With an overlay entry for the path,
it parses that text instead. The error handling after it is unchanged.
Module resolution still checks that the file exists on disk, so the overlay
replaces files' contents but can't add files.

The alternative is to mirror the project into a temporary directory, as an
editor plugin might for `inko check`. For a std-sized project (222 files) creating
and deleting a mirror of symlinks takes about 33 ms, roughly 30% of a check,
and every path in every answer would need mapping back. The overlay costs a
hash lookup per module.

### compiler/src/format.rs

```diff
-struct Document {
+pub struct Document {
...
-    fn new() -> Document {
+    pub fn new() -> Document {
...
-    fn format(mut self, ast: nodes::Module) -> String {
+    pub fn format(mut self, ast: nodes::Module) -> String {
```

`textDocument/formatting` must give exactly what `inko fmt` writes. The
formatter's public interface, `Formatter`, reads its input from files or
stdin and writes the result back to the files or stdout, neither of which
a server may do (a query writes nothing). `Document` is the part that
turns a parsed module (`Parser::with_comments`) into text; `Formatter`
calls nothing else to format. Making it public reuses the formatter
itself, rather than a copy of its 2,000 lines that would have to be kept
the same as upstream's. Nothing about how it formats changes.

## Rebasing onto a new release

1. Put the new tag in `UPSTREAM_VERSION`, remove `.upstream/`, and run
   `scripts/upstream.sh`. If the patch applies, go to step 3.
2. If it doesn't, re-create the changes by hand on a clean clone (each is
   shown above):
   - `sed -i '' 's/pub(crate)/pub/g' compiler/src/hir.rs`, then put
     `pub(crate)` back on `struct LowerToHir`.
   - Make `hir` public in `compiler/src/lib.rs`.
   - Re-apply the `check_with` hook to `Compiler::check`, the overlay field
     to `Config`, and the overlay lookup to `ModulesParser::parse`, as shown
     above.
   - Add `Compiler::diagnostics` at the end of `impl Compiler`, and make
     `DiagnosticId`, `Diagnostic` and its `is_error`, `id`, `message`,
     `file` and `location` public.
   - Add `check_after_errors` to `Config`, and `|| all` to each pass in
     `check_types`. A pass the release added needs it too.
   - Re-apply the four skips. If one moved or changed, reread the code
     around it: it must still only skip what an earlier pass rejected.
3. Run `scripts/refresh-patch.sh`, then `cargo test`.
   - `tests/citations.rs` fails for each upstream line Cuttlebone's code
     cites that moved or changed. For each, reread the cited upstream code,
     compare it with the Cuttlebone code that mirrors it (the comment next
     to the citation says what it mirrors), fix any difference, and update
     the citation and the test's table.
   - `tests/crosscheck.rs` fails if a reconstructed type disagrees with what
     the compiler stores.
   - The golden files (tests/golden) change when upstream's std changes.
     Regenerate them with `UPDATE_GOLDEN=1 cargo test`, and look at the diff
     for `<missing>` and for changes in which nodes are `synthetic`.
   - `tests/corpus.rs` fails if any std expression lost its type.
   - A pass after an error may `unwrap` something new. Cuttlebone survives
     that (it checks again, stopping at the error), but answers less.
     `tests/broken.rs` fails if a pass panics after an error in any of
     tests/fixtures/diagnostics (which between them have every diagnostic),
     and the edit-replay corpus (`tests/replay.rs`) for lost coverage.
   - `tests/diagnostics.rs` fails if Cuttlebone's diagnostics differ from
     the new release's `inko check` (which must then be installed). Its
     fixtures may need new cases for new diagnostic IDs.
