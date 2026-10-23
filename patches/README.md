# The upstream patch

`upstream.patch` applies to the upstream compiler release named in
`../UPSTREAM_VERSION`. It's a modification of MPL-2.0 files, so it's
MPL-2.0 too. Publishing it here, next to the release tag it applies to,
makes the source of every modified file available.

The patch makes two changes:

1. **Hook** (compiler/src/compiler.rs, compiler/src/lib.rs, 13 lines):
   `mod hir` becomes `pub mod hir`. `Compiler::check` becomes a wrapper around
   a new `Compiler::check_with(file, inspect)`, which calls
   `inspect(&Database, &[hir::Module])` after type checking and before
   lowering to MIR, even when type checking fails.
2. **Visibility** (compiler/src/hir.rs, 469 lines, mechanical): every
   `pub(crate)` becomes `pub`, except `struct LowerToHir`, which stays
   `pub(crate)` so the crate doesn't warn about private types in its
   signature.

## Rebasing onto a new release

1. Put the new tag in `UPSTREAM_VERSION`, remove `.upstream/`, and run
   `scripts/upstream.sh`. If the patch applies, you're done.
2. If it doesn't, re-create the changes by hand on a clean clone:
   - `sed -i '' 's/pub(crate)/pub/g' compiler/src/hir.rs`, then put
     `pub(crate)` back on `struct LowerToHir`.
   - Make `hir` public in `compiler/src/lib.rs`.
   - Re-apply the `check_with` hook to `Compiler::check` by hand.
3. Run `scripts/refresh-patch.sh`, then `cargo test`. The golden files
   (tests/golden) will change when upstream's std changes. Regenerate them
   with `UPDATE_GOLDEN=1 cargo test`, and look at the diff for `<missing>`.
