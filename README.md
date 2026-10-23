# Cuttlebone

Cuttlebone, a language server for Inko.

This repository is currently a feasibility spike (see [SPIKE.md](SPIKE.md)).
It links the upstream compiler crates from a pinned release with a small
patch (see [patches/](patches/README.md)), and has one binary, `dump-types`,
which prints every expression in a project or file with its inferred type.

## Building

You need Rust (rust-toolchain.toml pins the version) and LLVM 18, because the
upstream compiler crate links LLVM even though Cuttlebone never generates
code. `.cargo/config.toml` assumes Homebrew's `llvm@18`. Set
`LLVM_SYS_181_PREFIX` if yours is somewhere else.

    scripts/upstream.sh      # clone the pinned release into .upstream/ and patch it
    cargo build --release
    target/release/dump-types path/to/project          # or path/to/file.inko
    target/release/dump-types --summary path/to/project

## Tests

    cargo test

- `tests/golden.rs` compares `dump-types` output for a few std modules and
  `tests/fixtures/edges` with `tests/golden/*.txt`. Run
  `UPDATE_GOLDEN=1 cargo test` to rewrite them.
- `tests/corpus.rs` fails if any expression in std (src/ and test/) has no
  type, unless its kind is in the test's allow-list.
