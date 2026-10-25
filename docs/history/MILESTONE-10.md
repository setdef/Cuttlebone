# Milestone 10: prebuilt binaries

**Done.** Cuttlebone 0.10.0 is published as a GitHub release,
<https://github.com/setdef/Cuttlebone/releases/tag/v0.10.0>, with
self-contained binaries for four targets and a manifest that lists them:

```
https://github.com/setdef/Cuttlebone/releases/download/v0.10.0/manifest.json
https://github.com/setdef/Cuttlebone/releases/latest/download/manifest.json
```

| Target | Archive | Built on | Verified on |
|---|---|---|---|
| `aarch64-apple-darwin` | 5,420,206 bytes | `macos-15` | macOS 15 (CI); macOS 27 on an M1 Max, with the real Inko 0.21.1 |
| `x86_64-apple-darwin` | 5,686,236 bytes | `macos-15-intel` | macOS 15 on Intel (CI) |
| `x86_64-unknown-linux-musl` | 15,710,012 bytes | `ubuntu-24.04`, in Alpine 3.22 | Alpine 3.22 and Ubuntu 24.04 (CI, and Docker Desktop's emulation here) |
| `aarch64-unknown-linux-musl` | 13,295,693 bytes | `ubuntu-24.04-arm`, in Alpine 3.22 | Alpine 3.22 and Ubuntu 24.04 (CI, and Docker Desktop here) |

Each binary needs only the operating system and Inko 0.21.1: no Rust, no
LLVM, no Homebrew. A client can depend on it without its users building
anything. Nothing was published to a registry or package manager, and
the server's behaviour didn't change.

docs/releases.md is the reference: builds, archives, the manifest, and how
a release is made. README.md says how to install one.

## 1. Platforms

The platforms are the ones Inko's installation guide supports: Linux 4.11
or newer, macOS 15 or newer, and FreeBSD 13.2 or newer, each on 64-bit
AMD64 and ARM64. Windows isn't supported, so there's no build for it.

- **macOS on ARM64 and AMD64.** Both are built, on GitHub's `macos-15`
  and `macos-15-intel` runners (the ones Inko's own CI uses), for macOS 15
  (`MACOSX_DEPLOYMENT_TARGET=15.0`, `minos 15.0` in the binary). AMD64 was
  one of the two "only if cheap" platforms. It was: the same script as on
  ARM64, plus one matrix entry, and it built and verified the first time.
- **Linux on AMD64 and ARM64**, as static musl binaries: one per CPU,
  verified in Alpine 3.22 (musl) and Ubuntu 24.04 (glibc) containers.
- **FreeBSD isn't built.** It wasn't cheap. What it would take:
  1. A FreeBSD virtual machine in CI. GitHub has no FreeBSD runners.
     Inko's CI uses `cross-platform-actions/action` with its own FreeBSD
     14.0 image and 8 GB of memory, and notes that its builds there are
     slow.
  2. LLVM 18 that links statically there. FreeBSD's `llvm18` package
     installs under `/usr/local/llvm18`, and I haven't checked whether it
     ships the static libraries llvm-sys needs. If not, it means building
     LLVM from ports with them, which costs an hour or more per build
     unless cached.
  3. Rules for which of FreeBSD's own libraries a binary may link, in
     scripts/dist-deps.sh (base system in `/lib` and `/usr/lib`, nothing
     in `/usr/local`).
  4. Verifying in the same VM, since nothing else runs FreeBSD binaries.

  scripts/dist-manifest.py already knows the `x86_64-unknown-freebsd`
  target, and the manifest's `os` has `freebsd`.

## 2. Builds, and how static linking was achieved

**`dist` (cargo-dist) wasn't used.** Before writing anything, I compared
it with what this needs, but I didn't try a build with it, so these are
reasons from its design, not from its failing:

- Its musl builds run on Ubuntu runners with musl-tools or zig. LLVM 18
  built for musl, with static libraries, is what Alpine packages and what
  Inko's own Alpine CI image links against. Ubuntu has no such package, so
  linking it statically would mean building LLVM for musl.
- The names must carry the Inko version, the manifest is its own format
  (`dist-manifest.json` describes dist's own plan, not builds by Inko
  version), and every archive must be verified on its platform before
  anything is published.

With dist, the Linux builds would have had to be custom jobs anyway,
which leaves it little to do. The workflow is 120 lines
(`.github/workflows/release.yml`), and every step is a script that runs
the same on a laptop.

**Linux** (`scripts/dist-linux.sh`, `scripts/dist-build.sh`): built in an
`alpine:3.22` container with Alpine's `llvm18-static`, `zlib-static`,
`zstd-static`, `libxml2-static` and `libstdc++-dev`, as Inko's
`ci/docker/alpine/Dockerfile` does. Alpine's Rust targets musl with a
static C runtime, and llvm-sys then links LLVM's system libraries
statically too. Two things didn't link on their own:

- **libffi.** inkwell's build script always asks for `dylib=ffi`. A
  static link turned that into `libffi.so.8`, and with it an interpreter
  (`/lib/ld-linux-aarch64.so.1`): a binary that could run nowhere.
  scripts/dist-deps.sh caught it. inkwell has a `no-libffi-linking`
  feature. Cuttlebone's Cargo.toml turns it on for musl targets only
  (`[target.'cfg(target_env = "musl")'.dependencies]`, which Cargo
  unifies with the compiler's inkwell), so the patch doesn't change. The
  build links `libffi.a` itself, by file name (`-l:libffi.a`), because
  rustc ends its static libraries with `-Bdynamic`, and a plain `-lffi`
  then finds the `.so` again.
- **`__clear_cache`**, which LLVM calls on ARM64. It's in libgcc, and Rust
  links musl targets with `-nodefaultlibs`, so the build adds `-lgcc`.

Result: `statically linked` (`file`), no `INTERP` and no `NEEDED`
(`readelf`), and `ldd` says "not a dynamic executable" on Ubuntu (and
"Not a valid dynamic program" on Alpine).

**macOS** (`scripts/dist-build.sh`): llvm-sys already links Homebrew's
LLVM statically. But it puts llvm@18's `lib/` and Homebrew's `lib/` on
the linker's search path, so `-lzstd` found `libzstd.dylib` there. And
`libSystem`'s re-export of `libunwind` was matched to llvm@18's own
`libunwind.dylib` (ld: "reexported library ... couldn't be matched with
any parent library and will be linked directly"). The build now points
`LLVM_SYS_181_PREFIX` at a prefix of its own: a copy of `llvm-config`
(which finds its libraries relative to itself), the headers, links to the
static libraries, and zstd's `libzstd.a`, but no dylibs and no
`libunwind.a`. zstd is then linked statically, the unwinder is the OS's,
and what's left is the OS's own: `libSystem`, `libc++`, `libz`,
`libxml2`, `libffi`, `libncurses` and `libiconv`, all in `/usr/lib`.

**What links is checked** for every build before it's archived, in CI and
locally (`scripts/dist-deps.sh`):

- **macOS:** every `otool -L` entry must be under `/usr/lib/` or
  `/System/Library/`.
- **Linux:** `readelf` must find no `NEEDED` entry and no `INTERP`
  header.

The build fails otherwise, and with it the release.

**Each archive** (docs/releases.md, "Archives") holds `cuttlebone`, a
README, Inko's `LICENSE` as `LICENSE-MPL-2.0` (for the compiler crates),
and a `THIRD-PARTY.md` listing every crate in the binary (from
`cargo metadata`, normal dependencies only) with its license files, and
the C and C++ libraries linked statically
(`scripts/dist-licenses.py`). The build uses `--locked` and its own target
directory (`target/dist`), so it never touches the development build.

## 3. The manifest

`manifest.json` on each release (docs/releases.md, "The manifest"):

```json
{
  "schema": 1,
  "cuttlebone": "0.10.0",
  "builds": [
    {"inko": "0.21.1", "target": "aarch64-apple-darwin", "os": "macos", "arch": "arm64",
     "minimumOs": "15.0", "url": "https://github.com/setdef/Cuttlebone/releases/download/v0.10.0/cuttlebone-0.10.0-inko-0.21.1-aarch64-apple-darwin.tar.gz",
     "size": 5420206, "sha256": "62099dc59ce465f4de178c5c4b809162c1a2249eca7ab453fdc5365ef46cd6",
     "binary": "cuttlebone-0.10.0-inko-0.21.1-aarch64-apple-darwin/cuttlebone"}
  ]
}
```

(abridged above: the SHA-256 is cut to fit, and only one build of four is
shown). `os` and `arch` use Inko's own words (`macos`, `linux`,
`freebsd`; `amd64`, `arm64`), so a client matches them without Rust's
triples. `inko` is per build, not per release, so a later release could
carry builds for several Inko versions without a new schema. Today
every build in a release has the Inko version its source pins. Archive
names carry the Inko version
(`cuttlebone-<version>-inko-<inko>-<target>.tar.gz`). That names which
Inko a build is for, not the build after Inko, and the binary and
everything else stay `cuttlebone` (AGENTS.md, "Names").

## 4. Verifying, and the 0.10.0 release

**Before publishing**, each build job verifies its own archive
(`scripts/dist-verify.py`). It checks the archive's name and exactly
which files it holds, then runs the binary as an LSP server:
`--version` must print `cuttlebone 0.10.0 (Inko 0.21.1)`, `initialize`
must answer `serverInfo.version` 0.10.0 and
`experimental.cuttlebone.inko` 0.21.1, a hover on `numbers` in
`tests/fixtures/release` must answer `Array[Int]`, and `shutdown` and
`exit` must end it with status 0. The hover goes through a worker
process, so it covers `cuttlebone --worker` too.

The server needs an `inko` of the right version. CI runners have none,
so the verifier writes a stand-in that prints `inko 0.21.1` and the
pinned release's `std/src` as its standard library. On this Mac it also
ran with the real Inko 0.21.1 (ivm), with the same answers. Linux
archives run in `alpine:3.22` and `ubuntu:24.04` containers, through
`docker run -i`, with the files they read at the same paths.

**The release.** A hand-run of the workflow (`workflow_dispatch`, run
40163492139) built and verified all four targets without publishing.
The tag `v0.10.0` on 4f7d247 then ran run 40163744189:

| Job | Time |
|---|---|
| build `aarch64-apple-darwin` | 1 min 34 s |
| build `x86_64-apple-darwin` | 3 min 24 s |
| build `x86_64-unknown-linux-musl` | 2 min 18 s |
| build `aarch64-unknown-linux-musl` | 2 min 2 s |
| publish | 9 s |
| check the published release, on each of the four runners | 10 to 16 s |

`publish` checks that the tag is `v` plus Cargo.toml's version, writes
the manifest with the release's download URLs, and creates the release,
public at once, with the CHANGELOG.md entry as its notes.

**Checked from outside** (`scripts/dist-check-release.py`): given only
the manifest's URL, it checks the manifest's shape, downloads each
archive, checks its size and SHA-256, and runs the verifier on each build
it can run there. In CI, every runner checked all four downloads and ran
its own build: each macOS build natively, and each Linux build on its own
CPU in Alpine and Ubuntu. On this Mac (an M1 Max, macOS 27), I ran it on
`releases/latest/download/manifest.json`. All four checksums matched. The
ARM64 macOS build ran with the real Inko 0.21.1, and both Linux builds ran
in Alpine and Ubuntu, AMD64 under Docker Desktop's emulation. The Intel
macOS build couldn't run here, since Rosetta isn't installed. I verified
its checksum by hand, and CI's `macos-15-intel` runner ran it.

## 5. Tests and mutations

`scripts/check-dist.sh` is new in the milestone tier. It builds this
machine's archive, verifies it, and publishes it with a manifest to a
`file://` URL, then checks it from there. It also checks that each tool
refuses what it should: an archive of another version or another Inko, a
manifest whose checksum is wrong, and the development build, which links
Homebrew's libraries. Five mutations, all caught by it:

- release binaries may link anything (scripts/dist-deps.sh)
- the macOS release links Homebrew's LLVM directory (scripts/dist-build.sh)
- an archive of any version verifies (scripts/dist-verify.py)
- an archive for any Inko verifies (scripts/dist-verify.py)
- published checksums not checked (scripts/dist-check-release.py)

What isn't covered by a mutation: the Linux-only parts of
scripts/dist-build.sh (libffi and libgcc), which only the workflow
exercises. Removing either fails the Linux build itself, at the link or
in scripts/dist-deps.sh, as it did while I wrote them. The workflow's own
steps aren't covered either.

**The tiers.** Cargo.toml and scripts/mutate.py changed, so the
milestone tier says the exhaustive tier must pass too (AGENTS.md). The
exhaustive tier runs everything the milestone tier does, so I ran it in
its place, on 2dc8474 (the report's first commit; this paragraph is all
that changed since).

- **The first run** reported all 276 mutations caught by 00:55. Then the
  Mac ran out of battery and went into low-power sleep at 00:57 (`pmset
  -g log`), before the runner printed its summary, and it was stopped at
  the two-hour limit while asleep.
- **The second run**, on AC power, with other processes keeping the load
  average between 140 and 530, passed every step before the mutations:
  formatting, clippy, the tests, the patch, the release build, the whole
  corpus, every test without kept checks, the selection check, and
  `scripts/check-dist.sh`. The runner's control passed in 230 s, and
  **274 of the 276 mutations were caught, in 2,863 s. The other two
  timed out at the runner's 900 s**: "the batch's answers out of order"
  (milestone 6's, which timed out under load in milestones 8 and 9 too)
  and "highlights missing uses in unsafe units". Neither guards code this
  milestone changed.
- **Run again alone** (`scripts/mutate.py "the batch's answers out of
  order" "highlights missing uses in unsafe units"`), both were caught by
  the edit-replay corpus, in 770 s.

The check of every commit passed on each commit, through the pre-commit
hook, some only on a second try after the load had dropped (Remaining
unknowns, 5).

## Every patch line added, and why

None: the patch is unchanged, and patches/README.md with it. inkwell's
libffi is handled in Cuttlebone's own Cargo.toml (section 2).

## What clients should know

1. **Use the manifest**, not the release page. Fetch
   `releases/latest/download/manifest.json` (or a tag's), and pick the
   build whose `inko`, `os` and `arch` match the `inko --version` you'll
   pass as the `inko` option. Check `size` and `sha256`, unpack, and run
   `binary`. The steps are in docs/releases.md.
2. **One Inko per build.** With any other Inko, `initialize` fails with
   `inkoVersionMismatch` (docs/lsp.md). When the user's Inko is older than
   the latest release's, there's no build for it in that manifest. An
   older release's manifest, at its own tag's URL, may have one. Nothing
   lists every release's Inko version yet, except the GitHub releases API
   and CHANGELOG.md.
3. **macOS: not signed or notarized.** The binaries are only ad hoc
   signed (by the linker). A download by a browser is quarantined, and
   Gatekeeper refuses it (`xattr -d com.apple.quarantine` lifts that). A
   download by a client's own HTTP code, or `curl`, isn't quarantined,
   and runs; the external check above downloaded with Python's urllib.
4. **One file.** `cuttlebone` runs its checks in child processes of
   itself, so the binary is the whole server. It can live anywhere, such
   as a client's own data directory.
5. **Sizes.** The archives are 5.4 MB (macOS) to 15.7 MB (Linux). The
   unpacked binaries are 18.5 MB on macOS and 51 MB on Linux on ARM64.
   They keep line tables for panics' backtraces.

## Decisions you may want to revisit

- **Cuttlebone has no license.** The repository has no LICENSE file, and
  Cargo.toml no `license` field. The archives carry the licenses of what
  they contain, Inko's MPL-2.0 included (with the source of the patched
  files linked from each README), but nothing grants anyone rights to
  Cuttlebone's own code. That's yours to choose. A file at the
  repository's root, and a line in scripts/dist-build.sh copying it into
  the archives, would do it.
- **THIRD-PARTY.md links the C libraries' license texts** rather than
  including them, since the build machines don't reliably have them. Each
  crate's own license files are included in full.
- **Releases are public at once**, as you allowed. A broken build can't
  be published, since every build is verified first. Only the check after
  publishing could find a problem the release already has, such as a
  download that differs.
- **Alpine is pinned at 3.22**, and actions by commit. Alpine's
  `llvm18` packages will go away in some later release; the version is
  one line in scripts/dist-linux.sh.
- **Linux binaries keep their line tables** (51 MB unpacked on ARM64,
  against 18.5 MB on macOS). Stripping would save space at the cost of
  panic backtraces' lines.

## Remaining unknowns

1. **Old and unusual Linux systems.** The binaries are tested on Alpine
   3.22 and Ubuntu 24.04 only. Being static, they should run on any
   kernel from 4.11 on, but none older was tried. Nor was a system whose
   `/proc` is missing, which `std::env::current_exe` (the worker) needs.
2. **macOS older than 15, and 15 on real hardware.** The binaries target
   macOS 15. They ran on GitHub's macOS 15 runners and on macOS 27 here,
   not on anything older.
3. **Gatekeeper over time.** Ad hoc signed binaries downloaded without
   quarantine run today. If Apple tightens that, notarization would need
   a Developer ID, which is your call.
4. **FreeBSD** (section 1).
5. **The latency tests on a loaded machine.** Twice during this
   milestone, other processes pushed the load average past 150 (once
   400), and the pre-commit hook failed in tests/latency.rs. Commits waited
   until it settled. Nothing in this milestone changes the server's
   speed, but the hook will keep failing on such a machine.
