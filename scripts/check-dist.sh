#!/bin/sh
# Checks the release tooling (docs/releases.md) on this machine: builds
# this machine's archive (scripts/dist-build.sh, which checks what it
# links), verifies it, publishes it with a manifest to a file:// URL and
# checks it from there, and checks that each refuses what it should: an
# archive of other versions or with another LICENSE, a manifest whose
# checksum is wrong, an index that disagrees with its manifests, and a
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

# The same archive with another LICENSE than Cuttlebone's.
other="$release/other-license"
name=$(basename "$archive" .tar.gz)
mkdir -p "$other"
tar -xzf "$archive" -C "$other"
echo "Some other license" > "$other/$name/LICENSE"
COPYFILE_DISABLE=1 tar -czf "$other/$name.tar.gz" -C "$other" "$name"
fails python3 scripts/dist-verify.py "$other/$name.tar.gz"

cp "$archive" "$release/"
python3 scripts/dist-manifest.py "file://$release" "$release"/*.tar.gz > "$release/manifest.json"
python3 scripts/dist-check-release.py "file://$release/manifest.json"

sed 's/"sha256": "./"sha256": "x/' "$release/manifest.json" > "$release/tampered.json"
fails python3 scripts/dist-check-release.py "file://$release/tampered.json"

# The index: this release, and an older one for another Inko, whose
# manifest is published, from the manifests themselves.
mkdir -p "$release/older"
sed -e 's/"cuttlebone": "[^"]*"/"cuttlebone": "0.0.1"/' -e 's/"inko": "[^"]*"/"inko": "0.0.1"/' \
    "$release/manifest.json" > "$release/older/manifest.json"
python3 scripts/dist-index.py "file://$release/older/manifest.json" \
    --use "file://$release/new/manifest.json=$release/manifest.json" \
    "file://$release/new/manifest.json" > "$release/index.json"
mkdir -p "$release/new"
cp "$release/manifest.json" "$release/new/"
python3 scripts/dist-check-index.py "file://$release/index.json" --newest "$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)"
fails python3 scripts/dist-index.py "file://$release/new/manifest.json" "file://$release/manifest.json"
python3 -c 'import json, sys; i = json.load(open(sys.argv[1])); i["releases"].reverse(); print(json.dumps(i))' \
    "$release/index.json" > "$release/reversed.json"
fails python3 scripts/dist-check-index.py "file://$release/reversed.json"
sed 's/"0.0.1"$/"0.0.2"/' "$release/index.json" > "$release/wrong-inko.json"
fails python3 scripts/dist-check-index.py "file://$release/wrong-inko.json"

# The development build links Homebrew's LLVM directory (.cargo/config.toml).
if [ "$(uname -s)" = Darwin ]; then
    cargo build --quiet --release --bin cuttlebone
    fails scripts/dist-deps.sh "${CARGO_TARGET_DIR:-$root/target}/release/cuttlebone"
fi

echo "the release tooling works on $(uname -s) $(uname -m)"
