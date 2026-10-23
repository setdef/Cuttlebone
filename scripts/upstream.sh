#!/bin/sh
# Prepares .upstream/inko: the release named in UPSTREAM_VERSION with
# patches/upstream.patch applied. Set UPSTREAM_REPO to clone from a local
# checkout instead of GitHub.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
version=$(cat "$root/UPSTREAM_VERSION")
repo=${UPSTREAM_REPO:-https://github.com/inko-lang/inko.git}
dir="$root/.upstream/inko"

if [ ! -d "$dir/.git" ]; then
    mkdir -p "$root/.upstream"
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$version" "$repo" "$dir"
fi

if [ "$(git -C "$dir" describe --tags --exact-match HEAD 2>/dev/null)" != "$version" ]; then
    echo "$dir isn't at $version; remove it and run this again" >&2
    exit 1
fi

if git -C "$dir" apply --reverse --check "$root/patches/upstream.patch" 2>/dev/null; then
    echo "patch already applied to $dir"
else
    git -C "$dir" apply "$root/patches/upstream.patch"
    echo "applied patches/upstream.patch to $dir"
fi
