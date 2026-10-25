#!/usr/bin/env python3
"""Verifies a release archive (docs/releases.md): its name and contents,
and that its binary starts, prints its versions, answers `initialize` with
them, and answers a hover in tests/fixtures/release.

Usage:
  scripts/dist-verify.py ARCHIVE [--docker IMAGE] [--inko PATH | --std DIR]
                         [--version V] [--inko-version I]

The versions default to those in the archive's name, which must match
Cargo.toml and UPSTREAM_VERSION when the archive was built from this tree
(scripts/dist-build.sh); scripts/dist-check-release.py passes the
manifest's. `--docker IMAGE` runs the binary in a container of IMAGE (for
Linux binaries on other distributions), with the files it reads at the same
paths. The server needs an `inko` of the right version: `--inko PATH` is a
real one, else a stand-in that prints the version and `--std DIR` (default
.upstream/inko/std/src) as its standard library.
"""

import argparse
import json
import pathlib
import queue
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading

root = pathlib.Path(__file__).resolve().parent.parent
# Printed in order with the output of the commands it runs.
sys.stdout.reconfigure(line_buffering=True)

NAME = re.compile(r"^cuttlebone-(\d+\.\d+\.\d+)-inko-(\d+\.\d+\.\d+)-([a-z0-9_]+-[a-z0-9_]+-[a-z0-9_-]+)$")
FILES = {"cuttlebone", "README.md", "LICENSE", "LICENSE-MPL-2.0", "THIRD-PARTY.md"}
TIMEOUT = 120


def fail(message):
    print(f"FAIL: {message}", file=sys.stderr)
    sys.exit(1)


def check(condition, message):
    if not condition:
        fail(message)


class Server:
    """The binary as an LSP server, over its stdin and stdout."""

    def __init__(self, command):
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.messages = queue.Queue()
        self.next_id = 0
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        out = self.process.stdout

        while True:
            length = None

            while True:
                line = out.readline()

                if not line:
                    self.messages.put(None)
                    return

                line = line.strip()

                if not line:
                    break

                if line.lower().startswith(b"content-length:"):
                    length = int(line.split(b":")[1])

            self.messages.put(json.loads(out.read(length)))

    def send(self, message):
        body = json.dumps({"jsonrpc": "2.0", **message}).encode()
        self.process.stdin.write(b"Content-Length: %d\r\n\r\n%s" % (len(body), body))
        self.process.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"id": self.next_id, "method": method, "params": params})

        while True:
            try:
                message = self.messages.get(timeout=TIMEOUT)
            except queue.Empty:
                fail(f"no response to {method} in {TIMEOUT} s")

            check(message is not None, f"the server exited before answering {method}")

            if message.get("id") == self.next_id and "method" not in message:
                check("error" not in message, f"{method} failed: {message.get('error')}")
                return message["result"]

    def notify(self, method, params):
        self.send({"method": method, "params": params})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=pathlib.Path)
    parser.add_argument("--docker")
    parser.add_argument("--inko", dest="inko_path")
    parser.add_argument("--std", type=pathlib.Path, default=root / ".upstream/inko/std/src")
    parser.add_argument("--version")
    parser.add_argument("--inko-version")
    args = parser.parse_args()

    archive = args.archive.resolve()
    stem = archive.name.removesuffix(".tar.gz")
    match = NAME.match(stem)

    check(archive.name.endswith(".tar.gz") and match, f"{archive.name} isn't named as docs/releases.md says")

    version, inko, target = match.groups()
    check(args.version in (None, version), f"{archive.name} is version {version}, not {args.version}")
    check(args.inko_version in (None, inko), f"{archive.name} is for Inko {inko}, not {args.inko_version}")

    # Docker Desktop shares the user's temporary directory with its VM.
    work = pathlib.Path(tempfile.mkdtemp(prefix="cuttlebone-verify-")).resolve()

    try:
        with tarfile.open(archive) as tar:
            names = tar.getnames()
            check(
                set(names) == {stem} | {f"{stem}/{f}" for f in FILES},
                f"{archive.name} should hold exactly {stem}/ with {sorted(FILES)}, not {names}",
            )
            tar.extractall(work, filter="data")

        check(
            (work / stem / "LICENSE").read_bytes() == (root / "LICENSE").read_bytes(),
            f"{archive.name}'s LICENSE isn't Cuttlebone's own (the repository's LICENSE)",
        )

        binary = work / stem / "cuttlebone"
        project = work / "project"
        shutil.copytree(root / "tests/fixtures/release", project)

        if args.inko_path:
            inko_path = str(pathlib.Path(args.inko_path).resolve())
            mounts = [work, pathlib.Path(inko_path).parent]
        else:
            std = args.std.resolve()
            check(std.is_dir(), f"{std} isn't a directory (run scripts/upstream.sh, or pass --std)")
            stand_in = work / "inko"
            stand_in.write_text(
                "#!/bin/sh\n"
                "# A stand-in for inko: its version, and a standard library.\n"
                f'case "$1" in\n--version) echo "inko {inko}" ;;\n'
                f'print) echo "{std}" ;;\n*) exit 1 ;;\nesac\n'
            )
            stand_in.chmod(0o755)
            inko_path = str(stand_in)
            mounts = [work, std]

        prefix = []

        if args.docker:
            arch = {"x86_64": "amd64", "aarch64": "arm64"}[target.split("-")[0]]
            prefix = ["docker", "run", "--rm", "-i", "--platform", f"linux/{arch}"]

            for m in mounts:
                prefix += ["-v", f"{m}:{m}:ro"]

            prefix.append(args.docker)

        where = f" in {args.docker}" if args.docker else ""
        print(f"{archive.name}{where}:")

        printed = subprocess.run(prefix + [str(binary), "--version"], capture_output=True, text=True, timeout=TIMEOUT)
        expected = f"cuttlebone {version} (Inko {inko})"
        check(printed.returncode == 0, f"--version failed: {printed.stderr}")
        check(printed.stdout.strip() == expected, f"--version printed {printed.stdout!r}, not {expected!r}")
        print(f"  --version: {expected}")

        server = Server(prefix + [str(binary)])
        result = server.request(
            "initialize",
            {
                # In a container, this process isn't the server's parent.
                "processId": None,
                "rootUri": project.as_uri(),
                "capabilities": {},
                "initializationOptions": {"inko": inko_path},
            },
        )
        info = result["serverInfo"]
        session = result["capabilities"]["experimental"]["cuttlebone"]
        check(info == {"name": "cuttlebone", "version": version}, f"serverInfo is {info}")
        check(session["inko"] == inko, f"experimental.cuttlebone.inko is {session['inko']}, not {inko}")
        print(f"  initialize: cuttlebone {info['version']}, Inko {session['inko']}")

        server.notify("initialized", {})
        main_inko = project / "src/main.inko"
        uri = main_inko.as_uri()
        server.notify(
            "textDocument/didOpen",
            {"textDocument": {"uri": uri, "languageId": "inko", "version": 1, "text": main_inko.read_text()}},
        )
        # `numbers` in `let numbers = [10, 20]`.
        hover = server.request("textDocument/hover", {"textDocument": {"uri": uri}, "position": {"line": 5, "character": 8}})
        contents = hover and hover["contents"]
        check(contents == {"kind": "plaintext", "value": "Array[Int]"}, f"hover answered {hover}")
        print("  hover: Array[Int]")

        server.request("shutdown", None)
        server.notify("exit", None)
        code = server.process.wait(timeout=TIMEOUT)
        check(code == 0, f"the server exited with {code} after shutdown and exit")
        print("  exit: 0")
    finally:
        shutil.rmtree(work, ignore_errors=True)


main()
