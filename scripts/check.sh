#!/bin/sh
# The checks of each tier (AGENTS.md, "Checks"). The pre-commit hook runs
# the first.
#
# Usage: scripts/check.sh [--milestone [REV] | --exhaustive]
#
#   (none)            every commit: formatting, clippy, the tests (with a
#                     sample of the edit-replay corpus, and latency limits
#                     scaled by the machine's load), the patch, and the
#                     release build. About a minute.
#   --milestone [REV] before a milestone report: the above, the whole
#                     corpus, the latency limits as written (unscaled),
#                     every test again without kept checks (the
#                     answers must be the same), the mutation selection's
#                     own check, and the mutations a change since REV could
#                     affect. REV is the milestone's first commit; by
#                     default, the last commit that changed a milestone
#                     report.
#   --exhaustive      when the mutation runner or shared test
#                     infrastructure changes: the milestone tier's checks
#                     with every mutation.
set -eu

cd "$(dirname "$0")/.."

tier=commit
since=

case "${1-}" in
    "") ;;
    --milestone)
        tier=milestone
        since=${2:-$(git log -1 --format=%h -- 'docs/history/MILESTONE-*.md')}
        ;;
    --exhaustive) tier=exhaustive ;;
    *)
        echo "usage: scripts/check.sh [--milestone [REV] | --exhaustive]" >&2
        exit 2
        ;;
esac

start=$(date +%s)

step() {
    printf '==> %s\n' "$1"
    shift
    began=$(date +%s)
    "$@"
    printf '    (%ss)\n' "$(($(date +%s) - began))"
}

step "formatting" cargo fmt --check
step "clippy, all targets" cargo clippy --quiet --all-targets -- -D warnings
# Without the tests' dev-dependency, so without fault injection: the
# features a release build has.
step "clippy, release features" cargo clippy --quiet --lib --bins -- -D warnings
step "tests" cargo test --quiet
step "the patch applies to a fresh $(cat UPSTREAM_VERSION)" scripts/check-patch.sh
step "the release build has no test-only code" scripts/check-release.sh

if [ "$tier" != commit ]; then
    step "the whole edit-replay corpus" \
        env CUTTLEBONE_CORPUS_FULL=1 cargo test --quiet --test integration replay::
    step "the latency limits as written, however loaded the machine is" \
        env CUTTLEBONE_LATENCY_STRICT=1 cargo test --quiet --test latency
    step "every test without kept checks, and the whole corpus" \
        env CUTTLEBONE_NO_CACHE=1 CUTTLEBONE_CORPUS_FULL=1 cargo test --quiet
    step "the milestone tier selects every mutation that guards changed code" \
        python3 scripts/check-selection.py
    step "the release tooling, with this machine's archive" scripts/check-dist.sh
fi

case "$tier" in
    milestone) step "mutations a change since $since could affect" python3 scripts/mutate.py --since "$since" ;;
    exhaustive) step "every mutation" python3 scripts/mutate.py ;;
esac

printf '==> passed in %ss\n' "$(($(date +%s) - start))"
