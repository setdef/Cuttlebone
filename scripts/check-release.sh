#!/bin/sh
# Builds the release server, and checks that it contains no test-only code:
# no fault injection (the `fault-injection` feature) and none of the helpers
# in src/dump.rs (which name the pinned standard library's path).
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
bin="${CARGO_TARGET_DIR:-$root/target}/release/cuttlebone"

cargo build --quiet --release --bin cuttlebone --manifest-path "$root/Cargo.toml"

if grep -qaF CUTTLEBONE_FAULT "$bin"; then
    echo "the release build contains fault injection (CUTTLEBONE_FAULT)" >&2
    exit 1
fi

if grep -qaF .upstream/inko/std/src "$bin"; then
    echo "the release build contains test helpers from src/dump.rs" >&2
    exit 1
fi
