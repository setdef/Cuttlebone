#!/bin/sh
# Builds the Inko release Cuttlebone pins (UPSTREAM_VERSION), without the
# patch, for the tests' oracles (`inko check`, `inko doc`, `inko fmt`) on
# a machine without Inko, such as the release workflow's runners. It
# clones the release into DIR (never .upstream/, which is patched), builds
# it there with LLVM 18 (LLVM_SYS_181_PREFIX), and prints the directory to
# put on PATH. Nothing is installed.
#
# Usage: scripts/ci-inko.sh DIR
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
version=$(cat "$root/UPSTREAM_VERSION")
dir=${1:?usage: scripts/ci-inko.sh DIR}

if [ ! -d "$dir/.git" ]; then
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$version" \
        https://github.com/inko-lang/inko.git "$dir"
fi

# Its standard library is the clone's, as an installed Inko's is its own.
(cd "$dir" && INKO_STD="$dir/std/src" cargo build --quiet --release --bin inko >&2)

test "$("$dir/target/release/inko" --version)" = "inko ${version#v}" || {
    echo "$dir/target/release/inko isn't Inko ${version#v}" >&2
    exit 1
}
echo "$dir/target/release"
