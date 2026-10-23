#!/bin/sh
# Checks that patches/upstream.patch applies cleanly to a fresh copy of the
# pinned release (exported from its tag in .upstream/inko), and that it's
# exactly what .upstream/inko has applied, so the build and the published
# patch can't drift apart.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
version=$(cat "$root/UPSTREAM_VERSION")
dir="$root/.upstream/inko"
patch="$root/patches/upstream.patch"

if [ ! -d "$dir/.git" ]; then
    echo "$dir is missing; run scripts/upstream.sh" >&2
    exit 1
fi

fresh=$(mktemp -d)
trap 'rm -rf "$fresh"' EXIT

git -C "$dir" archive "$version" | tar -x -C "$fresh"

if ! (cd "$fresh" && git apply --check "$patch"); then
    echo "patches/upstream.patch doesn't apply to a fresh $version" >&2
    exit 1
fi

if ! git -C "$dir" diff | cmp -s - "$patch"; then
    echo "patches/upstream.patch isn't what .upstream/inko has applied;" \
        "run scripts/refresh-patch.sh, or scripts/upstream.sh on a fresh clone" >&2
    exit 1
fi
