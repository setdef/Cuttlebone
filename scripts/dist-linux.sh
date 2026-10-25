#!/bin/sh
# Builds the Linux release archive for the Docker host's CPU (AMD64 or
# ARM64) in an Alpine container, with Alpine's static LLVM 18 and musl, as
# Inko's own Alpine CI image does (ci/docker/alpine/Dockerfile in Inko's
# repository). The binary is fully static, so it runs on musl and glibc
# distributions alike. scripts/upstream.sh must have prepared .upstream/
# first.
#
# Usage: scripts/dist-linux.sh
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
alpine=${ALPINE_IMAGE:-alpine:3.22}

# The container's own target and toolchain directories, apart from the
# host's (which may be a different OS), inside target/ so they're kept
# between runs and ignored by git.
arch=$(docker run --rm "$alpine" uname -m)
cache="target/alpine-$arch"

docker run --rm -i \
    -v "$root:/cuttlebone" \
    -w /cuttlebone \
    -e CARGO_TARGET_DIR="/cuttlebone/$cache/target" \
    -e CARGO_HOME="/cuttlebone/$cache/cargo" \
    -e RUSTUP_HOME="/cuttlebone/$cache/rustup" \
    -e LLVM_SYS_181_PREFIX=/usr/lib/llvm18 \
    "$alpine" sh -eu <<'EOF'
apk add --quiet --no-progress build-base rustup python3 \
    llvm18-dev llvm18-static libstdc++-dev libffi-dev zlib-static zstd-static libxml2-static
export PATH="$CARGO_HOME/bin:$PATH"
if [ ! -x "$CARGO_HOME/bin/cargo" ]; then
    # The toolchain itself comes from rust-toolchain.toml.
    rustup-init --quiet -y --no-modify-path --profile minimal --default-toolchain none
fi
scripts/dist-build.sh
EOF
