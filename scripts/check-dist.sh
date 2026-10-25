#!/bin/sh
# Checks the release tooling (docs/releases.md) on this machine: builds
# this machine's archive (scripts/dist-build.sh, which checks what it
# links), verifies it, publishes it with a manifest to a file:// URL and
# checks it from there, and checks that each refuses what it should: an
# archive of other versions, a manifest whose checksum is wrong, and a
# binary that links Homebrew's libraries (the development build).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

fails() {
    if "$@" > /dev/null 2>&1; then
        echo "should have failed: $*" >&2
        exit 1
    fi
}

archive=$(scripts/dist-build.sh | tail -n 1)
python3 scripts/dist-verify.py "$archive"
fails python3 scripts/dist-verify.py "$archive" --version 0.0.0
fails python3 scripts/dist-verify.py "$archive" --inko-version 0.0.0

release="${CARGO_TARGET_DIR:-$root/target}/check-dist"
rm -rf "$release"
mkdir -p "$release"
cp "$archive" "$release/"
python3 scripts/dist-manifest.py "file://$release" "$release"/*.tar.gz > "$release/manifest.json"
python3 scripts/dist-check-release.py "file://$release/manifest.json"

sed 's/"sha256": "./"sha256": "x/' "$release/manifest.json" > "$release/tampered.json"
fails python3 scripts/dist-check-release.py "file://$release/tampered.json"

# The development build links Homebrew's LLVM directory (.cargo/config.toml).
if [ "$(uname -s)" = Darwin ]; then
    cargo build --quiet --release --bin cuttlebone
    fails scripts/dist-deps.sh "${CARGO_TARGET_DIR:-$root/target}/release/cuttlebone"
fi

echo "the release tooling works on $(uname -s) $(uname -m)"
