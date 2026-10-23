#!/bin/sh
# Makes git use the versioned hooks in .githooks/ for this clone.
set -eu

cd "$(dirname "$0")/.."
git config core.hooksPath .githooks
echo "git now runs .githooks/pre-commit (scripts/check.sh) before each commit"
