# Milestone 11: a license, a release index, and Parakeet's requests

**Done.** Cuttlebone 0.11.0 is published,
<https://github.com/setdef/Cuttlebone/releases/tag/v0.11.0>, built by the
release workflow and checked from outside:

- **License: MPL-2.0**, the same as Inko's and Parakeet's. LICENSE is at
  the root, Cargo.toml says `license = "MPL-2.0"`, README.md has a License
  section, and every archive holds Cuttlebone's LICENSE beside the
  compiler's `LICENSE-MPL-2.0` (section 1).
- **A release index**, `index.json`, published with each release and at
  `releases/latest/download/index.json`: every release's version, Inko
  versions and manifest URL, newest first. The manifest's fields are
  unchanged (section 2).
- **Parakeet's five requests**, each with its test written first and seen
  to fail (section 3):
  - a declaration error's effect decided by the compiler's ids where
    names have them
  - an import of a module that doesn't exist blank in the repaired copy
  - definition at `c.value += 1` answering the setter and the getter
  - kept checks surviving a save
  - references asked at a declaration in std or `dep/`
- **The check of every commit scales latency limits by the load.** The
  milestone tier and the mutations hold them as written (section 4).

"Never a wrong answer" holds: the whole edit-replay corpus, with and
without kept checks, has no wrong answer, and no coverage was lost
(section 5).

**Tests:** 305 (27 unit, 264 integration, 14 latency), up from 297.
`scripts/mutate.py` has 303 mutations, 27 of them new and 2 with new
snippets. **The patch didn't change.**

This milestone was requested: built by a coding agent with little or no
review.

## 1. License

- **LICENSE** is the MPL-2.0's text, the same file as Inko's own (Inko's
  and Parakeet's copies differ only in the indentation of one paragraph).
- **Cargo.toml** has `license = "MPL-2.0"`, and **README.md** a License
  section: Cuttlebone is MPL-2.0, the patch is MPL-2.0 because it modifies
  MPL-2.0 files, and what each archive holds.
- **Each archive** (scripts/dist-build.sh) holds `LICENSE` (Cuttlebone's)
  next to `LICENSE-MPL-2.0` (the compiler's, unchanged). The archive's
  README says "Cuttlebone is licensed under the Mozilla Public License 2.0
  (LICENSE)". Its paragraph on the compiler crates, with their source
  link, is as it was, and so are the patched files' own notices.
  THIRD-PARTY.md now begins "Besides Cuttlebone's own code (MPL-2.0, in
  LICENSE)". The compiler crates' entries still point to
  `LICENSE-MPL-2.0`. I read all three in the published Intel macOS
  archive, and they read correctly.
- **Checked:** tests/guardrails.rs (`cuttlebone_is_mpl_2_0`) checks the
  LICENSE text, Cargo.toml and README.md. scripts/dist-verify.py now
  requires `LICENSE` in the archive, byte for byte the repository's.
  scripts/check-dist.sh checks that it refuses an archive with another
  LICENSE.

## 2. The release index

docs/releases.md, "The index", is the reference.

```json
{
  "schema": 1,
  "releases": [
    {"cuttlebone": "0.11.0", "inko": ["0.21.1"], "manifest": "https://github.com/setdef/Cuttlebone/releases/download/v0.11.0/manifest.json"},
    {"cuttlebone": "0.10.0", "inko": ["0.21.1"], "manifest": "https://github.com/setdef/Cuttlebone/releases/download/v0.10.0/manifest.json"}
  ]
}
```

- **Where:** `index.json` is an asset of each release, so
  `releases/latest/download/index.json` always resolves to the latest
  release's. A tag's own index lists the releases up to that one.
- **Built from the manifests** (scripts/dist-index.py). When a release is
  published, the workflow lists every published release that has a
  `manifest.json` (through `gh api`), fetches each manifest, and writes
  each one's version and the Inko versions of its builds. The new
  release's manifest is read from disk. So the index can't disagree with
  the manifests, and a release deleted from GitHub leaves the next index.
  0.10.0, the first release with a manifest, is listed.
- **`inko` is a list**, newest first, because a manifest's `inko` is per
  build, so one release could carry builds for several Inko versions.
- **How a client uses it:** fetch the latest index, take the first release
  whose `inko` has the user's version, and install from its manifest as
  before. If that manifest has no build for the user's platform, the next
  release in the index with that Inko may.
- **Checked:** scripts/dist-check-index.py checks an index from its URL
  alone. It checks the shape, that releases are newest first and listed
  once each, and that each manifest fetches and says what the index does.
  The workflow's check jobs run it at the tag's URL and at the latest URL,
  with `--newest` set to the tag. scripts/check-dist.sh builds an index
  from this machine's manifest and an older one, all at `file://` URLs,
  and checks it. It also checks that the tools refuse a version listed
  twice, an index in the wrong order, and an index whose Inko versions
  disagree with its manifest.
- **The manifest is unchanged.** It has the same fields and schema 1, as
  Parakeet's download needs. The release's `manifest.json` has exactly
  the 0.10.0 fields, which I checked on the published one.

## 3. Parakeet's requests

From the "Requests for Cuttlebone" sections of Parakeet's reports
(SESSION-declaration-errors-and-0.12.md, SESSION-require-cuttlebone.md,
SESSION-compound-assignments.md, in Parakeet's docs/history/). Each test
below was written first and failed before its fix.

### Declaration errors, by ids

**Before:** an error in `A.size`'s signature refused every unit (method or
other item) that mentioned `size` or `A`, in its text or in its
expressions' types. Milestone 8 listed this as a decision to revisit.

**Now** (src/units.rs, src/query.rs): `units::error_effect` also records,
for each item the error is in, that item's header (a method's signature,
a type's or `impl`'s line, a field, an import) and the names it declares
there. The check resolves those to the compiler's ids: each reference or
declaration the walk found in the header with that name (`Checked::ids`).
In the guard, a name the error is about counts in a unit unless both of
these hold:

- the check has ids for the name
- every time the unit's text says the name, the check resolved it to
  another symbol

So `x.size(2.0)` on a `B` (`B.size`'s id) and `B.size`'s own declaration
no longer count, but `a.size(1)` (`A.size`'s id) does. Three cases still
count by name, because no id says otherwise:

- a mention the check didn't resolve
- a name the check found no id for in the header
- names in the types of a unit's expressions, which are strings (a value
  of type `A`)

Changes since the last good check have no ids from this check, so they
count by name as before.

**Test:** tests/broken.rs, `a_declaration_error_affects_its_own_symbols_not_others_with_the_same_name`:
Parakeet's case, with `A.size(unit: Missing)` and `B.size`. `run`, which
calls `B.size`, and `B.size` itself are answered (`Guarded`). Units that
call `A.size`, call a module's `measure(unit: Missing)` (by id only), or
hold a value of type `A` are refused.

**In the corpus** the change is small (section 5): its declaration-error
scenarios break names that few other symbols share.

### An import of a module that doesn't exist

**Before:** `import app.nope` stops the compiler before any type checking,
in every module of the check (`noHir`), like a syntax error. But the file
parses, so the repaired copy skipped it, and nothing answered until a good
check existed. `import app.util (Nope)`, a missing symbol, is a
declaration error, and was already answered.

**Now:**

- **The compiler says where.** It reports "the module '...' couldn't be
  found" (`invalid-file`) at the import (compiler/src/modules_parser.rs:119-124,
  in tests/citations.rs). When nothing was type checked, `check::run`
  returns those imports' files and lines (`Run::unfound`). The `noHir`
  reason carries them (`Unavailable::unfound`, never sent to clients).
- **The repaired copy** blanks those import lines, in whichever module
  they are, open or not, as well as any lines it blanks already. The copy
  is checked as any repaired copy is. Its answers are `fresh` and
  `repaired`, and are given only where the blank import can't matter.
- **What a blank import is about.** The guard counted every name in a
  changed import, so `import app.nope` made `app` count, and every unit
  using `app.util` (whose module type is `app.util`) was refused. A whole
  import now counts only the names it binds: the last name of its path,
  and the names in its symbol list (`units::import_names`), not the
  path's leading names or its build tags. Imports' declaration errors use
  the same rule.

**Test:** tests/repaired.rs, `an_import_of_a_module_that_doesnt_exist_is_blank_in_the_copy`.
A project is opened (with no good check) whose main.inko imports
`app.util`, `app.nope` and `app.nope.deeper (Thing)`, and whose util.inko
(not open) imports `app.missing`. A hover in `main` is answered,
`repaired`. One in a method using `nope.value` is `noHir`. A unit test
covers `import_names`.

### Definition at a compound assignment to a setter

**Before:** at `c.value += 1`, the compiler's two calls (the getter
`value`, then the setter `value=`) have the same name, and definition
picked one: the setter.

**Now:** `query::symbols_at` returns every symbol with a name at exactly
the selected location. Definition answers each declaration, as a
`Location[]` when there are several (LSP's `Definition` allows either),
the setter first, so a client that takes the first gets what it did. Every
other position answers one `Location`, as before, which the test also
checks. `query::definition` now returns `Vec<Place>`. Hover, highlights
and references are unchanged: references of either method already found
the site.

**Test:** tests/navigation.rs, `definition_at_a_compound_assignment_to_a_setter_is_the_getter_and_the_setter`,
over LSP: `c.value += 1`, `c.value = 2` and `c.value`, with the conformance
checks on every message.

### Kept checks right after a save

**Why a save lost them:** not the clock window. On APFS (and ext4, and any
file system that records nanoseconds) the margin is zero, since only a
time in whole seconds gets the 2 s (`check::before`). The cause was the
fingerprint itself:

- It recorded the stamp (size, times, inode) of every module file the
  check read. That included the open document's own file, though the
  check read the document's text, not the file. A save writes the file,
  so every kept check of that text no longer matched, and the next
  question was a whole check.
- A safe save (write another file, rename it over the first, IntelliJ's
  default) also changes the directory's times and the file's inode, and
  every fingerprint covers `src/`.

**Now** (src/check.rs): a fingerprint knows which files had an open
document (`Target::overlay`) and doesn't stamp them: their existence is
in their directory. A directory whose stamp changed still counts as the
same if the names in it that module resolution could find are the same:
directories, `.inko` files and `inko.pkg`. A file created or deleted
still counts (it changes the names), and so does any file that isn't open
(its own stamp). A check whose documents' files changed while it ran is
still kept, since it never read them.

**Test:** tests/cache.rs, `a_save_keeps_the_checks_of_the_text_it_saved`,
over LSP. An edited document is asked about twice, then saved in place,
safely (temporary file and rename), and in place again, each with
`didSave`. After each save, the question is `cached`. A module that isn't
open, changed on disk, isn't. Nor is a file created in `src/`. It failed
at the first in-place save before the fix.

### References at a declaration outside the project

**Before:** references asked at std's `Array.size` in std's own file
checked std as a project of its own. Over LSP Parakeet got `noHir`. Here,
over the core API with the pinned std, it answered std's own uses.
Parakeet's workaround found a use in the project first.

**Now** (`query::searched`): for a file in the session's std, the
references search every session project with a `src/` or `test/`, each
checked whole, and merge the answers. For a file in a project's `dep/`,
they search that project. Only the project's modules count, not the
requested file itself (std's or the dependency's own uses), and not the
declaration. A module no project uses answers `[]`: nothing in the project
can refer to it. Files in a project, and files outside any (a lone
script), are searched as before.

**Test:** tests/definition.rs, `references_at_a_declaration_outside_the_project_are_the_projects_uses`.
At std's `Array.size`, the answer is the uses in both of the session's
projects. At a dependency's `area` (in `dep/`, with `inko.pkg`
requiring it), it's the project's two uses. At std.json's `parse`, which
neither project uses, it's `[]`.

## 4. Latency limits on a busy machine

**Chosen:** strict limits move to the milestone tier, and the check of
every commit scales them by the load. AGENTS.md ("Checks", "Latency")
says so.

- **Every commit:** tests/latency.rs's limits are multiplied by the
  last minute's load average per core, when it's over 1
  (`/proc/loadavg`, or `sysctl vm.loadavg` on macOS). At a load of 170
  on this Mac's 10 cores, hover's 500 ms becomes 8.5 s. Other processes
  can then slow a commit's hook but not fail it. A regression that makes
  queries several times slower still fails on a quiet machine.
- **Milestone tier and mutations:** `CUTTLEBONE_LATENCY_STRICT=1`, which
  scripts/check.sh sets in a new step ("the latency limits as written,
  however loaded the machine is") and scripts/mutate.py sets for every
  mutation's tests. The limits are then exactly as written. So the
  milestone tier proves what it did, and it should be run when the
  machine is quiet.
- **Ratios** between two measurements of the same run (a question after a
  cancel at most 2.5 times one without; a second unanswered question at
  most 0.6 of the first) aren't scaled in either tier: load affects both
  sides.

I chose this over a baseline measured in the same run because a baseline
needs a reference for an idle machine, which differs per machine, and the
load factor needs nothing. The first commit hook of this step failed in
tests/latency.rs at a load of 175 (references across std 782 ms against
500 ms, the burst of definitions 8.0 s against 3 s). With the change, the
same tests passed at load averages of 110 to 147. The strict run passed
at load averages of 24 to 60.

**Test:** `limits_are_as_written_when_strict` checks that the limits are
unscaled with the variable set, which the mutation runner does. The
scaling itself isn't guarded by a mutation. Removing it makes the
per-commit tier stricter, not wrong, and only a loaded machine shows it.

## 5. The corpus

The whole edit-replay corpus (20 scenarios), Apple M1 Max, after this
milestone. "Before" is the floors in tests/replay.rs, which milestones 8
to 10 measured and didn't change. Answered questions (any source):

| Request | Asked | As the server runs: before | after | Without older checks: before | after | Without repaired copies: before | after | Wrong |
|---|---|---|---|---|---|---|---|---|
| hover | 21,047 | 11,904 | 11,904 | 10,350 | 10,358 | 9,896 | 9,896 | 0 |
| definition | 21,047 | 12,311 | 12,311 | 10,721 | 10,729 | 10,217 | 10,217 | 0 |
| typeDefinition | 21,047 | 11,990 | 11,990 | 10,400 | 10,408 | 9,896 | 9,896 | 0 |
| completion | 420 | 115 | 115 | 109 | 109 | 93 | 93 | 0 |
| signatureHelp | 471 | 382 | 382 | 311 | 311 | 298 | 298 | 0 |
| inlayHint | 11,618 | 6,652 | 6,652 | 5,784 | 5,792 | 5,831 | 5,831 | 0 |
| inlayHint, whole file | 483 | 483 | 483 | 386 | 386 | 483 | 483 | 0 |
| documentHighlight | 21,047 | 10,789 | 10,789 | 9,616 | 9,622 | 9,351 | 9,351 | 0 |
| references | 3,334 | 1,249 | 1,249 | 1,261 | 1,261 | 1,249 | 1,249 | 0 |

- **No wrong answer, and no coverage lost**, in any mode. The
  milestone tier also replays it all with no kept checks.
- **Without older checks** (a project opened broken), hover, definition
  and type definition each gain 8, inlay hints 8 and highlights 6. I
  didn't measure which of the two changes that affect it (ids, or what a
  whole import is about) gave them. The floors for that mode are raised
  to the new counts.
- **Why so little:** the corpus's declaration-error scenarios break a
  return type, an argument's type, an `impl`'s trait or a type's name, in
  fixtures with few same-named symbols. It has no import of a module that
  doesn't exist, no compound assignment to a setter, and no references
  asked at std. The tests above cover those.

## 6. The 0.11.0 release

The tag `v0.11.0` on 2bb61be ran the workflow (run 40234235149):

| Job | Time |
|---|---|
| build `aarch64-apple-darwin` | 1 min 48 s |
| build `x86_64-apple-darwin` | 3 min 9 s |
| build `x86_64-unknown-linux-musl` | 2 min 11 s |
| build `aarch64-unknown-linux-musl` | 2 min 6 s |
| publish (the manifest, the index, the release) | 15 s |
| check the published release and the index, on each of the four runners | 11 to 23 s |

| Target | Archive (0.10.0) | Archive (0.11.0) |
|---|---|---|
| `aarch64-apple-darwin` | 5,420,206 bytes | 5,470,184 bytes |
| `x86_64-apple-darwin` | 5,686,236 bytes | 5,730,947 bytes |
| `x86_64-unknown-linux-musl` | 15,710,012 bytes | 15,863,260 bytes |
| `aarch64-unknown-linux-musl` | 13,295,693 bytes | 13,445,044 bytes |

**Checked from outside**, on this Mac (M1 Max, macOS 27), from the
latest URLs alone:

- `scripts/dist-check-index.py .../releases/latest/download/index.json
  --newest 0.11.0`: two releases, 0.11.0 and 0.10.0, each with Inko
  0.21.1, each manifest fetched and agreeing.
- `scripts/dist-check-release.py .../releases/latest/download/manifest.json
  --inko` (the real Inko 0.21.1, from ivm): all four archives downloaded,
  with their sizes and SHA-256 matching. The ARM64 macOS build ran with
  the real Inko. Both Linux builds ran in Alpine 3.22 and Ubuntu 24.04
  (AMD64 under Docker Desktop's emulation). Each answered `--version`,
  `initialize` and a hover, and the verifier found LICENSE equal to the
  repository's. The Intel macOS build can't run here (no Rosetta), as in
  milestone 10. I checked its checksum, listed its five files, and
  compared its LICENSE by hand. CI's `macos-15-intel` runner ran it.
- The published manifest has exactly 0.10.0's fields.

docs/releases.md's example manifest now shows the published ARM64 macOS
build's size and SHA-256.

## 7. Tests and mutations

| File | What |
|---|---|
| tests/guardrails.rs (+1) | LICENSE is the MPL-2.0, Cargo.toml and README.md say so. |
| tests/navigation.rs (+1) | Definition at a compound assignment to a setter, over LSP. |
| tests/cache.rs (+1) | A save, in place and safely, keeps the checks of its text, over LSP. |
| tests/definition.rs (+1, 5 changed) | References at a declaration in std (two projects) and in `dep/`, and in a module no project uses. Changed: definitions are lists of places now. |
| tests/repaired.rs (+1) | An import of a module that doesn't exist, in the file and in a module that isn't open. |
| tests/broken.rs (+1) | A declaration error's effect by ids. |
| tests/latency.rs (+1) | Strict limits are as written. |
| tests/citations.rs | The compiler's message for a module it can't find. |
| src/units.rs (+1, 2 changed) | The names an import binds. Changed: an import's leading names no longer count. |
| tests/replay.rs | The floors without older checks, raised. |
| scripts/check-dist.sh | An archive with another LICENSE refused, and the index built, checked and refused when wrong. |

27 new mutations, all caught when run on their own during the milestone:

- **License (3):** archives without Cuttlebone's LICENSE, an archive with
  another LICENSE verifies, Cargo.toml without the license.
- **Index (4):** the index in the order given, a version twice in the
  index, an index in any order checks, an index's Inko versions not
  checked.
- **Latency (1):** strict limits scaled by the load.
- **Compound assignment (2):** the setter only, one definition as an
  array.
- **Saves (3):** a saved document counts as a change on disk, a
  directory's stamp alone counts, a directory's names never change.
- **References outside the project (5):** std's declarations search std,
  a dependency's search the dependency, references in the asked file
  outside the project, a module the project doesn't use unavailable,
  another project's references dropped.
- **Missing modules (5):** imports of modules not found aren't blank,
  blank only in open documents, none reported, every name in an import
  counts, an import's module name doesn't count.
- **Ids (4):** ids ignored, a mention by the error's own id doesn't count,
  names in types count by id, an error's ids from anywhere in its items.
  The second survived at first, because the types rule refused the same
  unit. The test now also calls a module's method with an error, which
  only its id says.

Two snippets moved: "fingerprints leave out directories" and
"fingerprints compare nothing".

What isn't guarded by a mutation: the load scaling (section 4), and the
case where a mention the check didn't resolve still counts
(`is_none_or`). Such a unit has an expression the check couldn't type,
which refuses it anyway, so no test can tell the two apart.

**The tiers.** Filled in below.

## Every patch line added, and why

None: the patch is unchanged, and patches/README.md with it.

## What clients should know

1. **The index** for an older Inko: `releases/latest/download/index.json`,
   then the first release whose `inko` has the user's version, then its
   manifest. The manifest is unchanged.
2. **Definition can be a `Location[]`**, at `c.value += 1` (setter, then
   getter). A client that only handles one `Location` should take the
   first.
3. **Saving is free.** A save no longer makes the next question a whole
   check, whether the client saves in place or safely.
4. **References at std's or a dependency's declaration** answer the
   workspace's uses. Parakeet's workaround (find a use, then ask there)
   is no longer needed. With several workspace folders, std's references
   check each project whole, so they cost one check per project.
5. **Fewer `declarationErrors`:** a method calling a same-named method of
   another type is answered. A missing module's import no longer darkens
   the file: the rest of it is answered `fresh`, `repaired`.
6. **Archives** have a sixth file, `LICENSE`. The binary's path in the
   manifest (`binary`) is unchanged.

## Decisions you may want to revisit

- **Names in types still count by name.** A value of type `A` is refused
  for an error in `A.size`. Type strings have no ids, and I kept it
  conservative. Measured in the corpus, it costs little.
- **Ids only from the error's own check.** Changes since the last good
  check still count by name, since their names were resolved, if at all,
  in another check.
- **A whole import counts only what it binds.** This also narrows
  milestone 8's rule for an import's declaration error (`std` in
  `import std.fmt (Format)` no longer counts). I found no case where a
  leading name matters, since Inko has no qualified paths in
  expressions. The corpus agrees.
- **std's references check every workspace project**, each whole. A
  client with many folders pays for each. Picking one would be cheaper,
  but would leave out uses.
- **Load scaling uses the 1-minute load average**, which lags. A spike
  that starts during the tests is only partly seen. That errs towards
  failing, as before.
- **The index lists only releases with a manifest**, so it starts at
  0.10.0.

## Remaining unknowns

1. **Safe saves elsewhere.** Tested with a rename in the same directory.
   An editor that writes a temporary `.inko` file next to the module, or
   saves through another directory, would change the names and the
   kept checks would go. That's safe, but a lost lookup.
2. **The ids rule on real code.** The corpus barely exercises it. The
   argument: a unit whose every mention of the name resolves to another
   symbol doesn't use the erroneous item by that name. Its other ways in
   (types, unresolved names) still count. But it's an argument, not a
   measurement.
3. **Missing modules imported under build tags**, or reached only
   through a dependency's own imports, aren't in the tests. The compiler's
   diagnostic places them the same way.
4. **From Parakeet's earlier reports, not in this milestone:** completion
   after a line that doesn't parse elsewhere in the file, `signatureHelp`
   for constructors, the type of a range, the position in
   `cuttlebone/status`, and `textDocument/implementation`.
5. **From earlier milestones, still open:** FreeBSD, notarization, and
   Windows.
