#!/bin/sh
# The latency limits as written, for the reference machine, scaled by how
# this machine compares with it (AGENTS.md, "Checks"): measures a fixed
# baseline workload first (the upstream compiler alone, type checking a
# module that imports all of std: `measure_the_baseline` in
# tests/latency.rs), divides it by the reference machine's, and runs the
# latency tests one at a time with every strict limit scaled by that
# ratio. It prints the baseline and the scale, and adds them to a GitHub
# Actions job's summary.
#
# Usage: scripts/latency.sh [FILTER...]   (filters as cargo test takes them)
set -eu

cd "$(dirname "$0")/.."

out=target/latency-baseline.txt

mkdir -p target
env CUTTLEBONE_LATENCY_BASELINE="$out" cargo test --quiet --test latency -- \
    --ignored --exact measure_the_baseline --nocapture >/dev/null
cat "$out"

if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then
    {
        echo '```'
        cat "$out"
        echo '```'
    } >>"$GITHUB_STEP_SUMMARY"
fi

scale=$(sed -n 's/^scale \([0-9.]*\).*/\1/p' "$out")

exec env CUTTLEBONE_LATENCY_STRICT=1 CUTTLEBONE_LATENCY_SCALE="$scale" \
    cargo test --quiet --test latency -- --test-threads=1 "$@"
