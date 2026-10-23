#!/bin/sh
# Smoke checks in real editors: each starts cuttlebone through the editor's
# own LSP client, opens tests/fixtures/unicode/src/main.inko, and checks
# that a hover after non-ASCII text shows the right type, that go to
# definition from a name after non-ASCII text finds its binding, and that an
# error typed after non-ASCII text gets a diagnostic, which goes away when
# the line is removed again. Neovim must negotiate UTF-8 positions, and VS
# Code gets UTF-16. Nothing is saved.
#
# Usage: editors/check.sh [neovim] [vscode]     (default: both)
#
# Neovim runs headless. VS Code has no headless mode on macOS, so its check
# opens a window for a few seconds, with a throwaway profile and no other
# extensions; on Linux, run it under xvfb-run. Set CUTTLEBONE to test
# another binary (default: target/release/cuttlebone, built if missing),
# INKO for an inko other than the one on PATH, and VSCODE for another VS
# Code executable.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
CUTTLEBONE=${CUTTLEBONE:-$root/target/release/cuttlebone}

if [ ! -x "$CUTTLEBONE" ]; then
    (cd "$root" && cargo build --release)
fi

WORKSPACE=$root/tests/fixtures/unicode
FILE=$WORKSPACE/src/main.inko
# '[3].size' follows '中文' on its line: 6 bytes, but 2 UTF-16 units. With
# the wrong encoding, the hover lands on `size` (Int) instead.
NEEDLE='[3].size'
OFFSET=0
EXPECTED='Array[Int]'
# The `e` after '日本' (6 bytes, 2 UTF-16 units) is the variable defined
# on line 12. With the wrong encoding, the position is past the line's end.
DEFINITION="'日本' + e"
DEFINITION_OFFSET=7
DEFINED_LINE=12
DEFINED='e'
# A line inserted after line 12, with an undefined name after '日本' (6
# bytes, 2 UTF-16 units): the diagnostic must cover exactly the name, so
# its range must be in the editor's encoding.
BROKEN="    let g = '日本' + nope"
BROKEN_AFTER=12
ERROR_CODE=invalid-symbol
ERROR_TEXT=nope
INKO=${INKO:-}
export CUTTLEBONE WORKSPACE FILE NEEDLE OFFSET EXPECTED INKO
export DEFINITION DEFINITION_OFFSET DEFINED_LINE DEFINED
export BROKEN BROKEN_AFTER ERROR_CODE ERROR_TEXT

editors=${*:-neovim vscode}
status=0

for editor in $editors; do
    case $editor in
        neovim)
            state=$(mktemp -d)
            XDG_STATE_HOME=$state nvim --headless --clean \
                -c "luafile $root/editors/neovim/check.lua" || status=1
            rm -rf "$state"
            ;;
        vscode)
            if [ ! -d "$root/editors/vscode/node_modules" ]; then
                (cd "$root/editors/vscode" && npm ci --no-audit --no-fund)
            fi

            if out=$(node "$root/editors/vscode/test/run.js" 2>&1); then
                printf '%s\n' "$out" | grep '^vscode:'
            else
                printf '%s\n' "$out" | grep -E '^vscode:|Error' >&2
                status=1
            fi
            ;;
        *)
            echo "unknown editor: $editor" >&2
            exit 2
            ;;
    esac
done

exit $status
