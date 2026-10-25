#!/usr/bin/env python3
"""Checks a published release index (docs/releases.md) from outside,
starting from nothing but its URL: its shape, that its releases are
newest first and each listed once, and, for each, that its manifest can
be fetched and says what the index does (the version, and the Inko
versions of its builds). It doesn't download the archives:
scripts/dist-check-release.py checks a release's own.

Usage: scripts/dist-check-index.py INDEX_URL [--newest VERSION]

`--newest VERSION` also requires that the index's newest release is
VERSION.
"""

import argparse
import json
import re
import sys
import urllib.request

VERSION = re.compile(r"^\d+\.\d+\.\d+$")
FIELDS = {"cuttlebone", "inko", "manifest"}

parser = argparse.ArgumentParser()
parser.add_argument("index")
parser.add_argument("--newest")
args = parser.parse_args()


def fetch(url):
    with urllib.request.urlopen(url, timeout=300) as response:
        return response.read()


def key(version):
    return tuple(map(int, version.split(".")))


index = json.loads(fetch(args.index))
problems = []


def expect(condition, message):
    if not condition:
        problems.append(message)

    return condition


expect(index.get("schema") == 1, f"schema is {index.get('schema')}, not 1")
releases = index.get("releases") or []
expect(releases, "no releases")
versions = []

for release in releases:
    if not expect(set(release) == FIELDS, f"a release with fields {sorted(release)}, not {sorted(FIELDS)}"):
        continue

    version = release["cuttlebone"]

    if not expect(VERSION.match(str(version)), f"version {version!r}"):
        continue

    versions.append(version)
    manifest = json.loads(fetch(release["manifest"]))
    inko = {b.get("inko") for b in manifest.get("builds", [])}
    print(f"{version}: Inko {', '.join(release['inko'])}, {release['manifest']}")
    expect(manifest.get("cuttlebone") == version, f"{version}: its manifest is of {manifest.get('cuttlebone')}")
    expect(set(release["inko"]) == inko, f"{version}: Inko {release['inko']}, but its manifest has {sorted(inko)}")
    expect(len(release["inko"]) == len(inko), f"{version}: an Inko version listed twice")

expect(versions == sorted(set(versions), key=key, reverse=True), f"releases not newest first, once each: {versions}")

if args.newest:
    expect(versions[:1] == [args.newest], f"the newest release is {versions[:1]}, not {args.newest}")

if problems:
    sys.exit("FAIL:\n" + "\n".join(problems))

print(f"{len(versions)} releases in {args.index} checked")
