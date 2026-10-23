#!/bin/sh
# Writes the changes made in .upstream/inko back to patches/upstream.patch.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
git -C "$root/.upstream/inko" diff > "$root/patches/upstream.patch"
git -C "$root/.upstream/inko" diff --stat | tail -1
