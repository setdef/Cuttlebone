#!/usr/bin/env python3
"""Prints the release index, index.json (docs/releases.md): every
release's version, the Inko versions it has builds for, and its
manifest's URL, newest first. It's built from the manifests themselves,
so each release's index lists every release published before it.

Usage: scripts/dist-index.py [--use URL=PATH]... MANIFEST_URL...

Each MANIFEST_URL is fetched (https:// or file://) and listed at that URL.
`--use URL=PATH` reads the manifest for URL from the file PATH instead,
for a release whose manifest isn't published yet. A version may appear
only once.
"""

import argparse
import json
import pathlib
import re
import sys
import urllib.request

VERSION = re.compile(r"^\d+\.\d+\.\d+$")

parser = argparse.ArgumentParser()
parser.add_argument("--use", action="append", default=[])
parser.add_argument("manifests", nargs="+")
args = parser.parse_intermixed_args()
local = dict(u.split("=", 1) for u in args.use)


def fetch(url):
    if url in local:
        return pathlib.Path(local[url]).read_bytes()

    with urllib.request.urlopen(url, timeout=300) as response:
        return response.read()


releases = []

for url in dict.fromkeys(args.manifests):
    manifest = json.loads(fetch(url))
    version = manifest.get("cuttlebone")

    if manifest.get("schema") != 1:
        sys.exit(f"{url}: schema {manifest.get('schema')}, not 1")

    if not VERSION.match(str(version)) or not manifest.get("builds"):
        sys.exit(f"{url}: no version, or no builds")

    if any(r["cuttlebone"] == version for r in releases):
        sys.exit(f"{url}: cuttlebone {version} appears twice")

    releases.append(
        {
            "cuttlebone": version,
            "inko": sorted({b["inko"] for b in manifest["builds"]}, key=lambda v: tuple(map(int, v.split("."))), reverse=True),
            "manifest": url,
        }
    )

releases.sort(key=lambda r: tuple(map(int, r["cuttlebone"].split("."))), reverse=True)
print(json.dumps({"schema": 1, "releases": releases}, indent=2))
