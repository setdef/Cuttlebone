# The upstream patch

`upstream.patch` applies to the upstream compiler release named in
`../UPSTREAM_VERSION`. It's a modification of MPL-2.0 files, so it's
MPL-2.0 too. Publishing it here, next to the release tag it applies to,
makes the source of every modified file available.

Nothing in the type checker, the types crate, or code generation changes. It
adds one way into the compiler (a hook), one input (an overlay of source
text), and makes the typed HIR readable.

| File | Lines | Why |
|---|---|---|
| compiler/src/lib.rs | 1 changed | `mod hir` becomes `pub mod hir`, so Cuttlebone can read the typed HIR. |
| compiler/src/hir.rs | 469 changed | Every `pub(crate)` becomes `pub`, except `struct LowerToHir`. Mechanical. |
| compiler/src/compiler.rs | +17, 1 changed | The `check_with` hook. |
| compiler/src/config.rs | +4 | The overlay field. |
| compiler/src/modules_parser.rs | +5, 1 changed | Reading the overlay. |

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
+        self.check_with(file, |_, _| true)
+    }
+
+    /// Type checks like `check`, passing the typed HIR to `inspect` before
+    /// it's lowered to MIR. `inspect` also runs if type checking fails, and
+    /// returning `false` from it skips lowering to MIR.
+    pub fn check_with<F: FnOnce(&types::Database, &[hir::Module]) -> bool>(
+        &mut self,
+        file: Option<PathBuf>,
+        inspect: F,
+    ) -> Result<(), CompileError> {
         self.prepare()?;
```

`check` keeps its behaviour (`inko check` is unchanged) by calling
`check_with` with an `inspect` that does nothing and continues to MIR. The
signature and doc comment are the hook's interface. `check_with` is
`check`'s old body, so it does the same parsing, lowering and passes.

```diff
         let mut hir = self.compile_hir(ast)?;
+        let typed = self.check_types(&mut hir);
-        self.check_types(&mut hir)?;
+        if !inspect(&self.state.db, &hir) {
+            return typed;
+        }
+
+        typed?;
```

- `let typed = ...` instead of `?`: `inspect` must also run when type
  checking fails, because a file with an error in one method body still has
  types everywhere else, and that's the normal state of a file being edited.
- `inspect(&self.state.db, &hir)`: the type database and the typed HIR are
  everything a type answer needs, and both are dropped once lowering to MIR
  consumes the HIR. Passing them by reference keeps the compiler's ownership
  as it was.
- `if !inspect(...) { return typed; }`: types never depend on MIR, and MIR
  lowering is more than half of a check's time (spike: 120 ms of 270 ms for
  std). It also panics on some input the type checker accepts (a `break`
  inside a closure inside a loop; compiler/src/mir/passes.rs:1836), so
  skipping it takes that crash off Cuttlebone's path. `dump-types` and the
  golden tests still lower to MIR by returning `true`.
- `typed?;`: the original early return, for when `inspect` continues.

### compiler/src/config.rs

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

The alternative is to mirror the project into a temporary directory, as
Parakeet does for `inko check`. For a std-sized project (222 files) creating
and deleting a mirror of symlinks takes about 33 ms, roughly 30% of a check,
and every path in every answer would need mapping back. The overlay costs a
hash lookup per module.

## Rebasing onto a new release

1. Put the new tag in `UPSTREAM_VERSION`, remove `.upstream/`, and run
   `scripts/upstream.sh`. If the patch applies, go to step 3.
2. If it doesn't, re-create the changes by hand on a clean clone:
   - `sed -i '' 's/pub(crate)/pub/g' compiler/src/hir.rs`, then put
     `pub(crate)` back on `struct LowerToHir`.
   - Make `hir` public in `compiler/src/lib.rs`.
   - Re-apply the `check_with` hook to `Compiler::check`, the overlay field
     to `Config`, and the overlay lookup to `ModulesParser::parse`, as shown
     above.
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
