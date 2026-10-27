#!/bin/sh
# The checks of each tier (AGENTS.md, "Checks"). The pre-commit hook runs
# the first.
#
# Usage: scripts/check.sh [--milestone [REV] | --release | --latency | --exhaustive]
#
#   (none)            every commit: formatting, clippy, the tests (with a
#                     sample of the edit-replay corpus, and latency limits
#                     scaled by the machine's load), the patch, and the
#                     release build. About a minute.
#   --milestone [REV] before a milestone report, locally: the above, the
#                     whole corpus, the latency limits as written
#                     (advisory: a failure is reported, and doesn't fail
#                     the tier, since a loaded machine can't meet them),
#                     every test again without kept checks (the answers
#                     must be the same), the mutation selection's own
#                     check, the release tooling, and the mutations a
#                     change since REV could affect. REV is the
#                     milestone's first commit; by default, the last
#                     commit that changed a milestone report.
#   --release         what the release workflow runs on macOS before it
#                     publishes: the milestone tier's checks but the
#                     latency limits and the mutations, which other jobs
#                     run (--latency, and scripts/mutate.py --shard).
#   --latency         the latency limits as written, failing, one test at
#                     a time: the release workflow's job of its own, on a
#                     machine that has run nothing else.
#   --exhaustive      all of the release tier on one machine: the release
#                     checks, the latency limits, and every mutation.
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
    --release) tier=release ;;
    --latency) tier=latency ;;
    --exhaustive) tier=exhaustive ;;
    *)
        echo "usage: scripts/check.sh [--milestone [REV] | --release | --latency | --exhaustive]" >&2
        exit 2
        ;;
esac

start=$(date +%s)
advisories=

step() {
    printf '==> %s\n' "$1"
    shift
    began=$(date +%s)
    "$@"
    printf '    (%ss)\n' "$(($(date +%s) - began))"
}

# A step whose failure is reported but doesn't fail the tier.
advisory() {
    printf '==> %s (advisory)\n' "$1"
    name=$1
    shift
    began=$(date +%s)
    if "$@"; then
        printf '    (%ss)\n' "$(($(date +%s) - began))"
    else
        printf '    FAILED, advisory here (%ss)\n' "$(($(date +%s) - began))"
        advisories="$advisories
    $name"
    fi
}

# One test at a time: each measures a machine nothing else of ours loads,
# as the limits assume.
strict="the latency limits as written, however loaded the machine is, one test at a time"

if [ "$tier" = latency ]; then
    step "$strict" env CUTTLEBONE_LATENCY_STRICT=1 cargo test --quiet --test latency -- --test-threads=1
    printf '==> passed in %ss\n' "$(($(date +%s) - start))"
    exit 0
fi

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
    step "every test without kept checks, and the whole corpus" \
        env CUTTLEBONE_NO_CACHE=1 CUTTLEBONE_CORPUS_FULL=1 cargo test --quiet
    step "the milestone tier selects every mutation that guards changed code" \
        python3 scripts/check-selection.py
    step "the release tooling, with this machine's archive" scripts/check-dist.sh
    # Last, so that the others run whatever it finds.
    case "$tier" in
        milestone) advisory "$strict" env CUTTLEBONE_LATENCY_STRICT=1 cargo test --quiet --test latency -- --test-threads=1 ;;
        exhaustive) step "$strict" env CUTTLEBONE_LATENCY_STRICT=1 cargo test --quiet --test latency -- --test-threads=1 ;;
    esac
fi

case "$tier" in
    milestone) step "mutations a change since $since could affect" python3 scripts/mutate.py --since "$since" ;;
    exhaustive) step "every mutation" python3 scripts/mutate.py ;;
esac

if [ -n "$advisories" ]; then
    printf '==> advisory steps that failed:%s\n' "$advisories"
fi

printf '==> passed in %ss\n' "$(($(date +%s) - start))"
