#!/bin/sh
# Builds the release server for this machine's target and packages it as a
# self-contained archive in dist/ (docs/releases.md):
#
#   dist/cuttlebone-<version>-inko-<inko>-<target>.tar.gz
#
# Run it on macOS, or in Alpine for Linux (scripts/dist-linux.sh
# runs it there). scripts/upstream.sh must have prepared .upstream/ first.
# It fails if the binary links anything outside the operating system's own
# libraries (scripts/dist-deps.sh).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
inko=$(sed 's/^v//' UPSTREAM_VERSION)
target=$(rustc -vV | sed -n 's/^host: //p')
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/dist}"
target_dir=$CARGO_TARGET_DIR

case "$target" in
    *-apple-darwin)
        # Homebrew's llvm@18 is the LLVM to link. llvm-sys links its static
        # libraries, but puts its lib/ directory (and Homebrew's) on the
        # linker's search path, so the linker also takes libzstd and
        # libunwind from there as dynamic libraries: a binary that needs
        # Homebrew to run. A prefix of our own with only llvm-config, the
        # headers and the static libraries (zstd's included) links them
        # statically instead, and the rest from the OS.
        llvm=$(brew --prefix llvm@18)
        zstd=$(brew --prefix zstd)
        prefix="$target_dir/dist-llvm"
        rm -rf "$prefix"
        mkdir -p "$prefix/bin" "$prefix/lib"
        # A copy, not a link: llvm-config finds the libraries relative to
        # where it is.
        cp "$llvm/bin/llvm-config" "$prefix/bin/"
        ln -s "$llvm/include" "$prefix/include"
        for lib in "$llvm"/lib/*.a; do
            ln -s "$lib" "$prefix/lib/"
        done
        # The OS has its own unwinder, in libSystem.
        rm -f "$prefix/lib/libunwind.a"
        ln -s "$zstd/lib/libzstd.a" "$prefix/lib/"
        export LLVM_SYS_181_PREFIX="$prefix"
        # The oldest macOS Inko supports (and tests).
        export MACOSX_DEPLOYMENT_TARGET=15.0
        ;;
    *-unknown-linux-musl)
        # Alpine's Rust targets musl with a static C runtime, and llvm-sys
        # then links LLVM's system libraries (libstdc++, zlib, zstd,
        # libxml2) statically too, as Inko's own Alpine builds do. Two more,
        # after LLVM's libraries:
        #
        # - libffi, which inkwell would link dynamically (Cargo.toml turns
        #   that off for musl). By file name, since rustc ends its static
        #   libraries with -Bdynamic, and -lffi would then find libffi.so.
        # - libgcc: on ARM64, LLVM calls __clear_cache, which is there, and
        #   Rust links musl targets without the C compiler's default
        #   libraries.
        export RUSTFLAGS="${RUSTFLAGS-} -C link-arg=-l:libffi.a -C link-arg=-lgcc"
        ;;
    *)
        echo "no release build for $target (docs/releases.md)" >&2
        exit 1
        ;;
esac

cargo build --quiet --release --locked --bin cuttlebone

name="cuttlebone-$version-inko-$inko-$target"
out="$root/dist/$name"
rm -rf "$out" "$out.tar.gz"
mkdir -p "$out"
cp "$target_dir/release/cuttlebone" "$out/"
cp LICENSE "$out/LICENSE"
cp .upstream/inko/LICENSE "$out/LICENSE-MPL-2.0"
python3 scripts/dist-licenses.py "$target" > "$out/THIRD-PARTY.md"
cat > "$out/README.md" <<EOF
# Cuttlebone $version for Inko $inko ($target)

\`cuttlebone\` is a language server for Inko, over LSP on stdin and stdout.
It works with Inko $inko only: \`inko\` on PATH (or the \`inko\`
initialization option) must be Inko $inko. \`cuttlebone --version\`
prints both versions.

Documentation and source: https://github.com/setdef/Cuttlebone/tree/v$version

Cuttlebone is licensed under the Mozilla Public License 2.0 (LICENSE).

It contains the compiler crates of Inko $inko
(https://github.com/inko-lang/inko/tree/v$inko), which are MPL-2.0
(LICENSE-MPL-2.0), with the changes in patches/upstream.patch of the
source above. THIRD-PARTY.md lists everything else it contains, and
their licenses.
EOF

scripts/dist-deps.sh "$out/cuttlebone"

# COPYFILE_DISABLE: without it, macOS's tar adds ._ files of extended
# attributes.
COPYFILE_DISABLE=1 tar -czf "$out.tar.gz" -C "$root/dist" "$name"
echo "dist/$name.tar.gz"
