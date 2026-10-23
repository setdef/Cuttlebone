# Cuttlebone

Cuttlebone, a language server for Inko.

It links the upstream compiler crates from a pinned release with a small
patch (see [patches/](patches/README.md)), type checks with them, and answers
questions about types. For now it's a type service for
[Parakeet](https://github.com/setdef/Parakeet), the IntelliJ plugin, over a
small JSON protocol of its own ([docs/protocol.md](docs/protocol.md)). An LSP
front end for other editors will come later, on the same core.

- [SPIKE.md](SPIKE.md): the feasibility spike.
- [MILESTONE-1.md](MILESTONE-1.md): the type service.

## Building

You need Rust (rust-toolchain.toml pins the version) and LLVM 18, because the
upstream compiler crate links LLVM even though Cuttlebone never generates
code. `.cargo/config.toml` assumes Homebrew's `llvm@18`. Set
`LLVM_SYS_181_PREFIX` if yours is somewhere else.

    scripts/upstream.sh      # clone the pinned release into .upstream/ and patch it
    cargo build --release

## Binaries

- `cuttlebone`: the type service. It reads requests from stdin and writes
  replies to stdout, one JSON object per line ([docs/protocol.md](docs/protocol.md)).
  `cuttlebone --version` prints its version and the Inko version it links.
- `dump-types`: prints every expression in a project or file with its type,
  for debugging and the golden tests.

      target/release/dump-types path/to/project          # or path/to/file.inko
      target/release/dump-types --summary path/to/project

## Layout

- `src/walker.rs`: walks the typed HIR, recording each expression's type,
  which nodes desugaring introduced, and cross-checks of the types it
  reconstructs.
- `src/query.rs`: `typeAt` and `membersAt`, and running a check so a compiler
  panic can't take the process down.
- `src/position.rs`: converting between the compiler's positions (1-based,
  grapheme clusters) and the protocol's (0-based, UTF-16).
- `src/server.rs`: the JSON protocol.

## Tests

    cargo test

Dependencies (the compiler) are optimized even in debug builds, so this is
also fast enough for the latency limits.

| Test | Checks |
|---|---|
| `tests/type_at.rs` | which expression a position means (desugared array literals, string interpolation, `for` and `while` loops, ...), UTF-16 positions after non-ASCII text |
| `tests/members.rs` | `membersAt` with a completion placeholder: visibility, static and instance methods, type arguments, modules, type parameters |
| `tests/broken.rs` | answers for code with errors: an unknown method, an undefined type in a signature, a syntax error, the placeholder |
| `tests/overlay.rs` | unsaved contents replacing files on disk, and the project's `inko.pkg` |
| `tests/panic.rs` | a compiler panic is reported, and the next request works |
| `tests/crosscheck.rs` | reconstructed types agree with what the compiler stores |
| `tests/citations.rs` | the upstream lines the code cites still say what it mirrors |
| `tests/e2e.rs` | the real binary over stdio: handshake, queries, errors, and latency limits |
| `tests/golden.rs` | `dump-types` output for a few std modules and fixtures, compared with `tests/golden/*.txt`. Run `UPDATE_GOLDEN=1 cargo test` to rewrite them. |
| `tests/corpus.rs` | every expression in std (src/ and test/) has a type, except for the allow-listed kinds |
