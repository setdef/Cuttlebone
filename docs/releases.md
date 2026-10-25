# Prebuilt binaries

Each version of Cuttlebone is published as a
[GitHub release](https://github.com/setdef/Cuttlebone/releases) with
prebuilt binaries, so it can be installed (by people, or by a client on
their behalf) without Rust and LLVM. `.github/workflows/release.yml` builds
them when a version tag is pushed, verifies each on its own platform, and
publishes them with a manifest that lists them, and an index of every
release. Nothing is published to any
registry or package manager.

## Builds

A build works with **one Inko version**: the compiler it links
([docs/lsp.md](lsp.md#the-inko-version)). Its name says which, and so does
`cuttlebone --version`. Using a build with any other Inko, `initialize`
fails with `inkoVersionMismatch`.

| Target | OS | CPU | Runs on |
|---|---|---|---|
| `aarch64-apple-darwin` | macOS | ARM64 | macOS 15 or newer |
| `x86_64-apple-darwin` | macOS | AMD64 | macOS 15 or newer |
| `x86_64-unknown-linux-musl` | Linux | AMD64 | Linux 4.11 or newer, any distribution: glibc (Ubuntu, Debian, Fedora, ...) and musl (Alpine) alike |
| `aarch64-unknown-linux-musl` | Linux | ARM64 | the same |

These are the platforms Inko supports, less one: **FreeBSD** isn't
built. GitHub has no FreeBSD runners, so it would need a virtual machine
(as Inko's own CI runs, with cross-platform-actions), and a way to link
LLVM 18 statically there, which nothing here has tried (MILESTONE-10.md,
in docs/history/, says what it would take).

Windows isn't supported by Inko, so there's no build for it.

### Self-contained

Every binary needs nothing but the operating system: no LLVM, no
Homebrew, no particular C library. The workflow fails before publishing if
one links anything else (`scripts/dist-deps.sh`):

- **Linux:** fully static, built in Alpine with musl and Alpine's static
  LLVM 18 (`scripts/dist-linux.sh`), as Inko's own Alpine CI is. No
  interpreter, no shared libraries: `ldd` says "not a dynamic executable".
- **macOS:** LLVM 18 and zstd are linked statically; the rest is the OS's
  own, in `/usr/lib` (`libSystem`, `libc++`, `libz`, `libxml2`, `libffi`,
  `libncurses`, `libiconv`).

`cuttlebone` runs its checks in child processes of itself, so the one file
is the whole server.

## Archives

Each build is one `.tar.gz`:

```
cuttlebone-<version>-inko-<inko>-<target>.tar.gz
cuttlebone-0.11.0-inko-0.21.1-aarch64-apple-darwin.tar.gz
```

It holds one directory of the same name (without `.tar.gz`), with:

| File | |
|---|---|
| `cuttlebone` | The server. |
| `README.md` | Its versions, its license, and where its source is. |
| `LICENSE` | Cuttlebone's own license, the MPL-2.0 (the repository's LICENSE). |
| `LICENSE-MPL-2.0` | The license of the Inko compiler crates it contains, also the MPL-2.0. |
| `THIRD-PARTY.md` | Every crate and library it contains, with their licenses. |

On macOS, a binary downloaded with a browser is quarantined, and
Gatekeeper refuses to run it, since it isn't signed or notarized.
`xattr -d com.apple.quarantine cuttlebone` lifts that. Downloads by
`curl`, or by a client's own HTTP code, aren't quarantined.

## The manifest

Each release has a `manifest.json` that lists its builds, so a client can
pick the one for the user's platform and Inko without scraping:

```
https://github.com/setdef/Cuttlebone/releases/download/v<version>/manifest.json
https://github.com/setdef/Cuttlebone/releases/latest/download/manifest.json
```

```json
{
  "schema": 1,
  "cuttlebone": "0.11.0",
  "builds": [
    {
      "inko": "0.21.1",
      "target": "aarch64-apple-darwin",
      "os": "macos",
      "arch": "arm64",
      "minimumOs": "15.0",
      "url": "https://github.com/setdef/Cuttlebone/releases/download/v0.11.0/cuttlebone-0.11.0-inko-0.21.1-aarch64-apple-darwin.tar.gz",
      "size": 5014108,
      "sha256": "7283c51f39992d946216e6b49c0648dec4698d5235856e574bf274793f273d6e",
      "binary": "cuttlebone-0.11.0-inko-0.21.1-aarch64-apple-darwin/cuttlebone"
    }
  ]
}
```

| Field | |
|---|---|
| `schema` | The manifest's format: 1. A client should refuse a manifest whose `schema` it doesn't know; a change that would break a client gets a new number. Fields may be added within a schema. |
| `cuttlebone` | The version of every build in it. |
| `builds` | One per Inko version and target, sorted by both. |
| `builds[].inko` | The Inko version the build works with, as `inko --version` prints it (without `inko `). |
| `builds[].target` | Rust's target triple. |
| `builds[].os` | `macos`, `linux` or `freebsd`. |
| `builds[].arch` | `amd64` or `arm64`. |
| `builds[].minimumOs` | The oldest OS it runs on: macOS's version, Linux's kernel, or FreeBSD's release. |
| `builds[].url` | The archive. |
| `builds[].size` | The archive's size in bytes. |
| `builds[].sha256` | The archive's SHA-256, in lowercase hex. |
| `builds[].binary` | The server's path in the archive. |

To install a build, a client:

1. runs `inko --version` (the one it will give the server as `inko`),
2. fetches the manifest, and picks the build whose `inko`, `os` and
   `arch` match (none matching means no build of this version for that
   Inko on that platform),
3. downloads `url`, and checks `size` and `sha256` before unpacking,
4. unpacks it, and runs `binary` (on macOS no quarantine applies to a
   download by its own code).

Today each release has builds for one Inko version, the one its source
pins. A client whose user has an older Inko needs an older release of
Cuttlebone, which [the index](#the-index) finds.

## The index

Each release also has an `index.json` that lists every release, newest
first, with the Inko versions it has builds for and its manifest's URL.
The latest release's is at a URL that doesn't change:

```
https://github.com/setdef/Cuttlebone/releases/latest/download/index.json
https://github.com/setdef/Cuttlebone/releases/download/v<version>/index.json
```

```json
{
  "schema": 1,
  "releases": [
    {
      "cuttlebone": "0.11.0",
      "inko": ["0.21.1"],
      "manifest": "https://github.com/setdef/Cuttlebone/releases/download/v0.11.0/manifest.json"
    },
    {
      "cuttlebone": "0.10.0",
      "inko": ["0.21.1"],
      "manifest": "https://github.com/setdef/Cuttlebone/releases/download/v0.10.0/manifest.json"
    }
  ]
}
```

| Field | |
|---|---|
| `schema` | The index's format: 1, with the same rules as the manifest's `schema`. |
| `releases` | Every release published with a manifest, newest first, each once. 0.10.0 is the first. |
| `releases[].cuttlebone` | The release's version. |
| `releases[].inko` | The Inko versions its manifest has builds for, newest first. |
| `releases[].manifest` | Its manifest's URL, at its own tag. |

So for a user whose Inko isn't the latest release's, a client:

1. fetches `releases/latest/download/index.json`,
2. takes the first release whose `inko` has the user's version (none means
   no release of Cuttlebone works with that Inko),
3. fetches its `manifest`, and installs from it as above. If the manifest
   has no build for the user's `os` and `arch`, the next release in the
   index with that Inko may.

The index is written when a release is published, from the manifests of
every release published before it and its own
(`scripts/dist-index.py`), so it says what they say. A tag's own
`index.json` lists the releases up to that one; only the latest's is
complete. Each version's entry in [CHANGELOG.md](../CHANGELOG.md) also
says which Inko `cuttlebone --version` names.

## How a release is made

1. The milestone's last commit is on `main`, with its version in
   Cargo.toml and CHANGELOG.md.
2. Push the tag `v<version>`. The workflow refuses a tag that doesn't
   match Cargo.toml.
3. The workflow builds each target on its own runner, checks what it
   links, and verifies it (`scripts/dist-verify.py`): it starts, prints
   both versions, answers `initialize` with them, and answers a hover in
   `tests/fixtures/release`, with a stand-in `inko` of the right version
   whose standard library is the pinned release's. A Linux build is
   verified in Alpine and in Ubuntu containers.
4. It publishes the release with the archives, `manifest.json` and
   `index.json` (every published release's manifest, and this one's), the
   version's CHANGELOG.md entry as its notes.
5. It downloads the published release again from the manifest's URL on
   each runner, checks each archive's size and SHA-256, and verifies those
   it can run there (`scripts/dist-check-release.py`). It checks the index
   at the tag's URL and the latest release's: newest first, this release
   the newest, and each release's manifest saying what the index does
   (`scripts/dist-check-index.py`).

Run by hand (`workflow_dispatch`), the workflow does steps 3 alone. To build
and check locally: `scripts/dist-build.sh` (macOS) or
`scripts/dist-linux.sh` (Docker), then `scripts/check-dist.sh` checks the
tooling, and `scripts/dist-check-release.py MANIFEST_URL` checks a
published release from any machine, and `scripts/dist-check-index.py
INDEX_URL` the index.
