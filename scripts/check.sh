#!/bin/sh
# Every check a commit must pass (AGENTS.md). The pre-commit hook runs it.
#
# Usage: scripts/check.sh [--full]
#
# --full adds the mutation runner (scripts/mutate.py), which takes several
# minutes. Run it before any milestone report.
set -eu

cd "$(dirname "$0")/.."

case "${1-}" in
    "") full=false ;;
    --full) full=true ;;
    *)
        echo "usage: scripts/check.sh [--full]" >&2
        exit 2
        ;;
esac

start=$(date +%s)

step() {
    printf '==> %s\n' "$1"
    shift
    "$@"
}

step "formatting" cargo fmt --check
step "clippy, all targets" cargo clippy --quiet --all-targets -- -D warnings
# Without the tests' dev-dependency, so without fault injection: the
# features a release build has.
step "clippy, release features" cargo clippy --quiet --lib --bins -- -D warnings
step "tests" cargo test --quiet
step "the patch applies to a fresh $(cat UPSTREAM_VERSION)" scripts/check-patch.sh
step "the release build has no test-only code" scripts/check-release.sh

if [ "$full" = true ]; then
    step "mutations" python3 scripts/mutate.py
fi

printf '==> passed in %ss\n' "$(($(date +%s) - start))"
