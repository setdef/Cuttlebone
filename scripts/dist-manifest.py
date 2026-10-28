#!/usr/bin/env python3
"""Prints a release's manifest.json (docs/releases.md) for its archives.

Usage: scripts/dist-manifest.py BASE_URL ARCHIVE...

BASE_URL is where the archives are downloaded from, such as
https://github.com/setdef/Cuttlebone/releases/download/v0.16.0; each
archive's URL is BASE_URL/NAME. Every archive must be for this tree's
version (Cargo.toml) and Inko version (UPSTREAM_VERSION), and each target
must appear once.
"""

import hashlib
import json
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent.parent

# Every target a release may have, in the words of Inko's documentation
# (docs/releases.md).
TARGETS = {
    "aarch64-apple-darwin": {"os": "macos", "arch": "arm64", "minimumOs": "15.0"},
    "x86_64-apple-darwin": {"os": "macos", "arch": "amd64", "minimumOs": "15.0"},
    "x86_64-unknown-linux-musl": {"os": "linux", "arch": "amd64", "minimumOs": "4.11"},
    "aarch64-unknown-linux-musl": {"os": "linux", "arch": "arm64", "minimumOs": "4.11"},
    "x86_64-unknown-freebsd": {"os": "freebsd", "arch": "amd64", "minimumOs": "13.2"},
}

version = re.search(r'^version = "(.*)"$', (root / "Cargo.toml").read_text(), re.M).group(1)
inko = (root / "UPSTREAM_VERSION").read_text().strip().removeprefix("v")
base = sys.argv[1].rstrip("/")
builds = []

for path in map(pathlib.Path, sys.argv[2:]):
    prefix = f"cuttlebone-{version}-inko-{inko}-"
    name = path.name

    if not (name.startswith(prefix) and name.endswith(".tar.gz")):
        sys.exit(f"{name} isn't an archive of cuttlebone {version} for Inko {inko}")

    target = name.removeprefix(prefix).removesuffix(".tar.gz")

    if target not in TARGETS:
        sys.exit(f"{name}: unknown target {target}")

    if any(b["target"] == target for b in builds):
        sys.exit(f"{name}: {target} appears twice")

    data = path.read_bytes()
    builds.append(
        {
            "inko": inko,
            "target": target,
            **TARGETS[target],
            "url": f"{base}/{name}",
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
            "binary": f"{name.removesuffix('.tar.gz')}/cuttlebone",
        }
    )

if not builds:
    sys.exit("no archives")

manifest = {
    "schema": 1,
    "cuttlebone": version,
    "builds": sorted(builds, key=lambda b: (b["inko"], b["target"])),
}

print(json.dumps(manifest, indent=2))
