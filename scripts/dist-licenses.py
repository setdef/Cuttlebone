#!/usr/bin/env python3
"""Prints THIRD-PARTY.md for a release archive: every crate the
`cuttlebone` binary contains on TARGET, with its license and license
files, and the C and C++ libraries it links statically.

Usage: scripts/dist-licenses.py TARGET
"""

import json
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parent.parent
target = sys.argv[1]

metadata = json.loads(
    subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
)

packages = {p["id"]: p for p in metadata["packages"]}
nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
cuttlebone = next(p["id"] for p in metadata["packages"] if p["name"] == "cuttlebone")

# What the binary contains: normal dependencies, transitively. Build
# dependencies and proc macros run only while building, and
# dev-dependencies only in tests.
seen = set()
todo = [cuttlebone]

while todo:
    node = nodes[todo.pop()]

    for dep in node["deps"]:
        if dep["pkg"] in seen or packages[dep["pkg"]]["targets"][0]["kind"] == ["proc-macro"]:
            continue

        if any(k["kind"] is None for k in dep["dep_kinds"]):
            seen.add(dep["pkg"])
            todo.append(dep["pkg"])

# Static libraries, by target: what llvm-sys and Rust's standard library
# link (scripts/dist-build.sh).
native = [
    ("LLVM 18", "Apache-2.0 WITH LLVM-exception", "https://github.com/llvm/llvm-project/blob/release/18.x/llvm/LICENSE.TXT"),
    ("zstd", "BSD-3-Clause", "https://github.com/facebook/zstd/blob/dev/LICENSE"),
]

if target.endswith("-linux-musl"):
    native += [
        ("musl", "MIT", "https://git.musl-libc.org/cgit/musl/tree/COPYRIGHT"),
        ("libstdc++ and libgcc (GCC)", "GPL-3.0-or-later WITH GCC-exception-3.1", "https://gcc.gnu.org/onlinedocs/libstdc++/manual/license.html"),
        ("zlib", "Zlib", "https://zlib.net/zlib_license.html"),
        ("libffi", "MIT", "https://github.com/libffi/libffi/blob/master/LICENSE"),
        ("libxml2", "MIT", "https://gitlab.gnome.org/GNOME/libxml2/-/blob/master/Copyright"),
    ]

out = []
out.append(f"# Third-party software in this build ({target})\n")
out.append(
    "Besides Cuttlebone's own code, the `cuttlebone` binary contains the "
    "following, statically linked.\n"
)
out.append("## C and C++ libraries\n")
out.append("| Library | License | License text |\n|---|---|---|")

for name, license, url in native:
    out.append(f"| {name} | {license} | {url} |")

out.append("")
out.append("## Rust crates\n")
out.append("| Crate | Version | License |\n|---|---|---|")

crates = sorted((packages[i] for i in seen), key=lambda p: (p["name"], p["version"]))

for p in crates:
    out.append(f"| {p['name']} | {p['version']} | {p['license'] or 'see below'} |")

out.append("")
out.append("## License files of the Rust crates\n")

for p in crates:
    directory = pathlib.Path(p["manifest_path"]).parent
    files = sorted(
        f
        for f in directory.iterdir()
        if f.is_file() and f.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT"))
    )

    # The upstream compiler's crates have no license file of their own: the
    # repository's (MPL-2.0) is LICENSE-MPL-2.0 in this archive.
    if not files and p["source"] is None:
        out.append(f"### {p['name']} {p['version']}\n\nMPL-2.0: see LICENSE-MPL-2.0.\n")
        continue

    for f in files:
        out.append(f"### {p['name']} {p['version']}: {f.name}\n")
        out.append("```")
        out.append(f.read_text(errors="replace").rstrip())
        out.append("```\n")

    if not files:
        out.append(f"### {p['name']} {p['version']}\n\n{p['license']}, with no license file in the crate.\n")

print("\n".join(out))
