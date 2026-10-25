#!/bin/sh
# Fails if a release binary links anything outside the operating system's
# own libraries, printing what it links:
#
#   macOS: only /usr/lib/ and /System/Library/ (otool -L)
#   Linux: nothing at all, no interpreter and no shared libraries
#          (readelf), so one binary runs on musl and glibc distributions
#
# Usage: scripts/dist-deps.sh BINARY
set -eu

bin=$1

case "$(uname -s)" in
    Darwin)
        otool -L "$bin"
        # The first line is the binary's own name.
        outside=$(otool -L "$bin" | tail -n +2 | awk '{print $1}' |
            grep -v -e '^/usr/lib/' -e '^/System/Library/' || true)
        ;;
    Linux)
        readelf --dynamic --program-headers "$bin" |
            grep -e NEEDED -e INTERP -e 'Requesting program interpreter' || true
        outside=$(readelf --dynamic --program-headers "$bin" |
            grep -e NEEDED -e INTERP || true)
        ;;
    *)
        echo "unknown OS $(uname -s)" >&2
        exit 1
        ;;
esac

if [ -n "$outside" ]; then
    echo "$bin links libraries that aren't the OS's own:" >&2
    echo "$outside" >&2
    exit 1
fi

echo "$bin links only the OS's own libraries"
