#!/usr/bin/env python3
"""Checks a published release from outside, starting from nothing but its
manifest's URL (docs/releases.md): the manifest's shape, then for each
build, downloads its archive, checks its size and SHA-256, and runs it
(scripts/dist-verify.py) wherever this machine can. A Linux build runs in
Alpine and in Ubuntu containers, with Docker on Linux on the build's CPU
or Docker Desktop on macOS (which emulates the other CPU), and a macOS
build on macOS on its own CPU. It fails if any build can't be run
here, unless `--partial` is given.

Usage: scripts/dist-check-release.py MANIFEST_URL [--partial] [--inko PATH]

`--inko PATH`, a real Inko of the manifest's version, is used for builds
run on this machine itself, instead of a stand-in.
"""

import argparse
import hashlib
import json
import pathlib
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request

root = pathlib.Path(__file__).resolve().parent.parent
# Printed in order with the output of the commands it runs.
sys.stdout.reconfigure(line_buffering=True)
IMAGES = ["alpine:3.22", "ubuntu:24.04"]
FIELDS = {"inko", "target", "os", "arch", "minimumOs", "url", "size", "sha256", "binary"}
VERSION = re.compile(r"^\d+\.\d+\.\d+$")

parser = argparse.ArgumentParser()
parser.add_argument("manifest")
parser.add_argument("--partial", action="store_true")
parser.add_argument("--inko")
args = parser.parse_args()


def fetch(url):
    with urllib.request.urlopen(url, timeout=300) as response:
        return response.read()


manifest = json.loads(fetch(args.manifest))
problems = []


def expect(condition, message):
    if not condition:
        problems.append(message)

    return condition


expect(manifest.get("schema") == 1, f"schema is {manifest.get('schema')}, not 1")
expect(VERSION.match(str(manifest.get("cuttlebone"))), f"cuttlebone is {manifest.get('cuttlebone')!r}")
expect(manifest.get("builds"), "no builds")

if problems:
    sys.exit("\n".join(problems))

version = manifest["cuttlebone"]
host_os = {"Darwin": "macos", "Linux": "linux", "FreeBSD": "freebsd"}.get(platform.system())
host_arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "amd64", "amd64": "amd64"}.get(platform.machine())
unrun = []
# Docker on Linux runs containers of its own CPU; Docker Desktop on macOS
# emulates the other.
docker = subprocess.run(["docker", "info"], capture_output=True).returncode == 0 if shutil.which("docker") else False

with tempfile.TemporaryDirectory(prefix="cuttlebone-release-") as work:
    for build in manifest["builds"]:
        target = build.get("target")
        print(f"{target}:")

        if not expect(set(build) == FIELDS, f"{target}: fields {sorted(build)}, not {sorted(FIELDS)}"):
            continue

        name = build["url"].rsplit("/", 1)[1]
        expect(
            name == f"cuttlebone-{version}-inko-{build['inko']}-{target}.tar.gz",
            f"{target}: archive named {name}",
        )
        expect(build["binary"] == f"{name.removesuffix('.tar.gz')}/cuttlebone", f"{target}: binary {build['binary']}")

        data = fetch(build["url"])
        sha256 = hashlib.sha256(data).hexdigest()
        print(f"  downloaded {build['url']}: {len(data)} bytes, SHA-256 {sha256}")

        if not (
            expect(len(data) == build["size"], f"{target}: {len(data)} bytes, not {build['size']}")
            and expect(sha256 == build["sha256"], f"{target}: SHA-256 {sha256}, not {build['sha256']}")
        ):
            continue

        archive = pathlib.Path(work) / name
        archive.write_bytes(data)
        verify = [sys.executable, str(root / "scripts/dist-verify.py"), str(archive), "--version", version, "--inko-version", build["inko"]]

        if build["os"] == "linux" and docker and (build["arch"] == host_arch or host_os == "macos"):
            runs = [verify + ["--docker", image] for image in IMAGES]
        elif (build["os"], build["arch"]) == (host_os, host_arch):
            runs = [verify + (["--inko", args.inko] if args.inko else [])]
        else:
            unrun.append(target)
            print("  can't be run here")
            continue

        for run in runs:
            expect(subprocess.run(run).returncode == 0, f"{target}: {' '.join(run[2:])} failed")

if unrun:
    print(f"not run here: {', '.join(unrun)}")
    expect(args.partial, f"builds not run: {', '.join(unrun)} (pass --partial to allow it)")

if problems:
    sys.exit("FAIL:\n" + "\n".join(problems))

print(f"every build of cuttlebone {version} in {args.manifest} checked")
