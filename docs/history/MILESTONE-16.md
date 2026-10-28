# Milestone 16: what a client that relies on Cuttlebone lost

**Done, but not released yet.** Everything for 0.16.0 is committed and
the release workflow's dry run passed on f7c64a4 (section 9); pushing the
tag `v0.16.0` was refused by the session's permissions, and is left to
you.

Parakeet, slimmed to rely on Cuttlebone (its `slimming-2` branch,
docs/history/SESSION-slimming-2.md), accepted regressions wherever
Cuttlebone had no answer. This milestone answers them, by the compiler's
ids and types, with standard LSP only (0.16.0):

1. **Diagnostics past a module that doesn't parse** (section 1): the
   other modules are checked without it and what imports it, exactly as
   `inko check` checks a copy of the project without them.
2. **Implementations, hierarchies and file renames past a declaration
   error elsewhere** (section 2): refused only where a unit the error
   makes unsafe names what they're about, as references since milestone
   8; file renames too, fresh or refused.
3. **Call Hierarchy's callers in code being edited** (section 2): the
   same rule, so a signature being typed refuses only the callers of what
   it calls.
4. **Several answers in one request** (section 3): `textDocument/codeLens`,
   every declaration's implementations and supertypes, and each `impl`'s
   trait and its methods' trait's methods, from one check of the whole
   project.
5. **A status for every file rename, naming its files** (section 4).
6. **A selection step with a declaration's doc comment** (section 5).
7. **A trait's required and default methods** in document symbols'
   `detail` (section 6).

## 1. Diagnostics past a module that doesn't parse

A module that doesn't parse, or imports one that can't be found, stops
the compiler before it checks any other module (`compile_hir` bails on
any error), so one file mid-edit blanked every other file's errors, in
the editor and the project view. `inko check FILE`, which Parakeet ran
per file, didn't have that problem, but it isn't `inko check` of the
project either.

When a project's full check stops that way, Cuttlebone (`query::unparsed`)
takes every file with an error, adds every module that imports one of
them, directly or not (from their imports' text, by module name), and
checks the project again without those files. The files left out get the
first check's diagnostics (`inko check`'s: the syntax error, the module
not found, and nothing for an importer); every other file gets the second
check's. That's exactly what `inko check` reports for a copy of the
project without those files, which is the oracle: tests/diagnostics.rs
derives the files to leave out independently (from `inko check`'s
errors and the `import` lines), copies the project without them, runs the
installed `inko check` on the copy, and holds Cuttlebone to it for every
fixture project. The `syntax` fixture gained a module that imports none
of the broken ones (`fine.inko`, with an error, now reported) and one
that imports one (`user.inko`, left out), and a new `unfound` fixture has
only a module not found. Where `inko check` has an answer of its own (the
project parses), nothing changed.

Over LSP, tests/publish.rs types a module into not parsing and back, in
both encodings: the other modules' errors stay, at the same ranges after
non-ASCII text; the module importing it is cleared, as `inko check`
leaves it; and everything comes back when it parses. Pull diagnostics
share the core, and tests/pull.rs holds every fixture project's pull to
it.

The second check needs the compiler to leave files out of a project's
check: four patch lines (below). Copying the project instead would
write to disk, which no check may.

A lone file (not in a project's `src/` or `test/`) is checked as before,
with what it imports, as `inko check FILE` does.

## 2. Whole-project answers past a declaration error elsewhere

Implementations, incoming calls, supertypes, subtypes and file renames
gave no answer while any declaration in the project had an error:
`analyze` refused every whole-project question but references past one.
Now each uses references' rule (`hidden`): refused only if a unit that the
error makes unsafe (`Guard`) mentions a name the answer is about
(`about`):

| Question | Names |
|---|---|
| references, incoming calls | the symbol's at the position |
| implementations of a trait | the trait's (an `impl` of it names it) |
| implementations of a trait's method | the method's (its implementations in an `impl` with an error are untyped, and name themselves) |
| supertypes, subtypes | the type's or trait's |
| a module's uses (file renames) | the module's last name (`util`): an import's path and a use by name end with it |

A first version also counted a trait's method's trait; a test with an
`impl` of an undefined type showed the method already names itself, so it
was dropped rather than kept untested. Answers are `fresh` (from the
documents as they are), never from the last good check: one would miss
an `impl` or a call added since, wherever it is, so it couldn't be
complete. File renames stay fresh-only: they change code.

Item 3, callers in code being edited, is the same rule: a signature being
typed that doesn't check yet (`fn added -> Strin` calling `helper`) makes
its own method unsafe, so `helper`'s callers are refused and `total`'s
answered, moved by the lines added (tests/hierarchy.rs).

tests/hierarchy.rs asks all seven (implementations of a trait and of its
method, callers of two methods, a type's supertypes, a trait's subtypes,
a file rename) clean, then past an unrelated error (each the same as
clean, `fresh`), past an `impl` of the trait with an error (those about it
refused, the callers answered), past an `impl` for a type that doesn't
exist, and in an edited document. The edit-replay corpus now asks
supertypes too, and every whole-project answer past an error is compared
with the final text's: none wrong, and answered went from 270 to 347-358
of 644 in the sample (implementations 350, incoming calls, supertypes
and subtypes 347; without older checks 358 and 354).

**Cost.** `hidden` built a guard (an outline of the module and its walk)
for every module of the project, per question: harmless for one
reference, but a document's code lenses ask dozens, and across all of std
past an error they timed out (over 5 s). Now the guards are built once per
request, only for modules whose text has a name a question is about, and
each guard collects its unsafe units' names once.

## 3. Several answers in one request: code lenses

LSP's way to show an answer with each declaration is `textDocument/codeLens`.
A batch of requests isn't part of LSP (JSON-RPC batches aren't in the
spec, and lsp-server doesn't take them), and the hierarchies take one
position each. So: for a client with `textDocument.codeLens`,
`codeLensProvider` (not resolved: every lens has its command) and one
command, `cuttlebone.showLocations`.

`query::lenses` outlines the document from its text and asks, in one
`ask_with`: implementations and supertypes at each trait, implementations
at each trait's method, supertypes at each type, definition at each
`impl`'s trait name, and declaration at each `impl`'s method. One check of
the whole project answers the first three, one of the file the rest. Each
lens's `command.arguments[0]` is `{"kind": ..., "locations": [...]}`, and
its title says it for people (`2 implementations`, `implements ToString,
Named`, `requires Named`, `Named`, `Named.name`). A lens with nothing to
show isn't given; the status's `covered` names every declaration whose
lenses are all there are, so past an error a client can keep what it had
for the rest. `workspace/executeCommand` with the command shows the
first location with `window/showDocument`, to a client that supports it.

tests/hierarchy.rs checks every lens of a module, in both encodings,
against the answer of its own request at the same position, and the
titles; past an error, that the trait it could hide is left out of
`covered` and the `impl`'s own lenses stay; the command's
`window/showDocument`; and that a client without code lenses isn't
offered them. tests/common/spec.rs now checks the requests a server
sends (only one the spec has servers send, with its params, and
`window/showDocument` only for a client with `window.showDocument.support`),
and that `codeLensProvider` and `executeCommandProvider` go only to a
client with code lenses; tests/conformance.rs checks those checks.

## 4. A status for every file rename

A file rename where no module's name changes (out of `src/`, a name no
module can have) answered `null` with no status, so a client couldn't
tell "no edits" from "no answer". It now has a `fresh` status, with no
check. And a rename's status named the first module that moves as
`textDocument`, not anything the request named: it now has the request's
own `files` instead (`[{"oldUri", "newUri"}]`), as sent; every other
status already named the request's document (a hierarchy's, its item's),
and all have the request's `id` (tests/rename_files.rs, for a file, a
directory and a rename that edits nothing).

## 5. A selection step with a declaration's doc comment

Selection ranges gain a step from a declaration's doc comment's first
`#` to its end, after the declaration's own. Two things came up over
std (tests/selection.rs checks every declaration around every name):

- **A statement ran past a declaration's edge.** A trait's required
  method has no body to end its statement, which went on into the next
  method, so the step with the comment overlapped it and was left out.
  Steps that cross a declaration's (or its comment's) edge are now left
  out instead: declarations always nest.
- **`inko doc` reads a comment after code on the line above** as a
  declaration's doc comment (`let FILE_MODE = 0x1B6 # 666` above `let
  DIR_MODE`). That's no step to select: only a doc comment on lines of
  its own gives one. The test checks both, the second because the first
  run's mutation found it unchecked.

## 6. A trait's required and default methods

`SymbolTag` has only `Deprecated`, so `detail` is the spec's place: a
trait's method with a body (a default one) has ` { ... }` after its
signature in document symbols' `detail`, and a required one has none.
With definition at an `impl`'s trait name, a client can list what
implementing it must write and what it may override.
tests/corpus_symbols.rs holds both lists to the type database
(`TraitId::required_methods` and `default_methods`) for every trait in
std (over 40).

## 7. Measurements

Apple M1 Max (the reference machine), under `cargo test`, at load
averages of 250 to 650 from other work on the machine the whole session,
so upper bounds:

| What | Measured | Limit (tests/latency.rs) |
|---|---|---|
| code lenses of std's `string.inko`, across all of std | 270 to 750 ms | 750 ms |
| the same, past an error in another std module | 0.9 to 1.2 s (over 5 s before guards were built once per request) | 1.5 s |
| a document's diagnostics pulled, std-sized project | 230 ms | 1 s |
| the same, past a module that doesn't parse (two checks, the first stopping at once) | 306 ms | 1 s |

In the milestone tier's strict latency step, this Mac's baseline
measured 204 ms (scale 0.73, so every limit as written), and every
limit held.

## 8. Tests and mutations

| File | What |
|---|---|
| tests/diagnostics.rs | the oracle past a module that doesn't parse: `inko check` of the project without it and its importers; the `syntax` fixture's new modules, the `unfound` fixture |
| tests/publish.rs | a module typed into not parsing and back, both encodings |
| tests/hierarchy.rs | the seven whole-project answers clean, past unrelated and related errors, in an edited document; code lenses against their requests, past an error, their command, without the capability |
| tests/replay.rs | supertypes asked too; the sample's coverage floors raised |
| tests/rename_files.rs | a status for every rename, naming its files |
| tests/selection.rs | the doc comment step for every documented declaration in std, and none for a comment after code |
| tests/symbols.rs, tests/corpus_symbols.rs | default methods marked; every trait's methods against the type database |
| tests/latency.rs | code lenses across std, clean and past an error; a pull past a module that doesn't parse |
| tests/common/spec.rs, tests/conformance.rs | requests from the server, and code lens capabilities |
| tests/lsp.rs | the hung check's follow-up hover asked again if a loaded machine times it out too (it did, at a load average of 460) |

**The milestone tier, locally**: `scripts/check.sh --milestone 69e5644`
on d3c731a: formatting 1 s, clippy 15 s, tests 110 s, the patch 0 s, the
release build 21 s, the whole corpus 66 s, every test without kept checks
149 s, the selection 4 s, the release tooling 26 s, and the strict latency
step (advisory) passed, 53 s. Its mutation step was stopped by the
session's 2-hour limit for a background command after 338 of its 460
selected mutations, all caught (the machine's load average was 650); the
other 119 then ran on their own (`scripts/mutate.py` with their names,
50 minutes): 118 caught, and one not, a code lens for a trait with no
implementations, since every trait in the fixture had some. With a trait
nothing implements in the test, it's caught (this report's commit).

Earlier in the milestone, a first run of the tier stopped at a load flake
in tests/lsp.rs (above), and a run of the new mutations found the comment
after code above a declaration selected (section 5). Two old mutations
whose code this milestone rewrote point at the new code, and a third,
the blanket refusal of whole-project answers, gave way to two that refuse
too much and too little.

## Every patch line added, and why

Seven lines (compiler/src/config.rs +4, compiler/src/compiler.rs +3),
justified in patches/README.md ("leaving files out"):

```diff
+
+    /// Source files a check of the whole project leaves out.
+    pub exclude: Vec<PathBuf>,
```
```diff
+            exclude: Vec::new(),
```
```diff
             all_source_modules(&self.state.config, true)
                 .map_err(CompileError::Internal)?
+                .into_iter()
+                .filter(|(_, p)| !self.state.config.exclude.contains(p))
+                .collect()
```

The default is empty, so the `inko` command is unchanged; only a check
of the whole project reads it. Two citations of compiler.rs moved down by
three lines with it (tests/citations.rs).

## What Parakeet gets back

Each item, and the regression or fallback in SESSION-slimming-2.md
("Fallbacks kept, and what was lost", "Requests for Cuttlebone") it fixes:

1. **Diagnostics past a module that doesn't parse** → "While a module of
   the project doesn't parse ... the other modules show no errors or
   warnings, in the editor or the Project view" (request 8). The editor's
   pull and the workspace pull keep every other module's errors while
   one is typed (or a quick fix's `y: ` is left to fill in), so
   `InkoIntegrationTest`'s split halves would pass as one again. The
   module being typed and what imports it show what `inko check` does.
2. **Implementations, hierarchies and file renames past a declaration
   error elsewhere** → "Where a declaration has an error anywhere in the
   project ... no trait gutter icons, Go to Implementation finds nothing,
   Type Hierarchy and Call Hierarchy's callers are empty, Rename of a
   trait's method lists a conflict, and a move lists a conflict"
   (request 2). Each is refused now only where the error could change it,
   so an error in an unrelated module no longer darkens them; a move past
   an error gets its edits unless a broken unit names the module.
3. **Callers in code being edited** → "In code that doesn't type check,
   Call Hierarchy showed Find Usages' possible usages; it shows nothing
   now" (request 3): it shows the callers wherever references would be
   given, complete, which is more than possible usages.
4. **Code lenses** → "A type's gutter icon for the traits it implements
   is gone" and the 380-requests-for-230-keystrokes problem (request 7):
   one `textDocument/codeLens` a highlighting pass gives every trait's and
   trait method's implementations, every type's traits (the icon can come
   back), and the icons pointing up (an `impl`'s trait, its methods'
   trait's methods), which `InkoTraitImpl` reads from the syntax now. The
   locations are in the lens's arguments, so no further request is needed.
5. **A status for every file rename, naming its files** → "it replies
   `null` with no status where no module's name changes ... so Parakeet
   only asks where one does", and "its status for a file rename names the
   first module that moves ... so Parakeet takes that request's status by
   method" (requests 4 and 5). Parakeet can ask for every move, and match
   the status by `id` and `files`.
6. **A selection step with the doc comment** → "Extend Selection no longer
   has a step for a declaration with the comment above it" (request 6).
7. **A trait's required and default methods** → "Implement Methods still
   ... reads its methods from `InkoDefinitionIndex`: Cuttlebone has no
   list of a trait's required and default methods yet" (request 1):
   definition at the `impl`'s trait name, then the trait's children in
   its file's document symbols, `detail` saying which have a default.
   With milestone 15's workspace symbols, that's what Implement Methods,
   Override Methods and Generate → Implement Trait need to stop reading
   the trait themselves, and `InkoDefinitionIndex` and
   `InkoSourceDefinitions` can go.

Still Parakeet's own: rename and refactorings as code actions (the next
milestone).

## Decisions you may want to revisit

- **Diagnostics leave out every file with an error**, not only the one
  that doesn't parse, once a check stops before type checking: the
  compiler bailed before it got to any of them, so their other errors
  aren't known, and `inko check` of the rest needs them gone. An error in
  a file outside the project (a dependency that doesn't parse) checks
  nothing again.
- **Importers are found from their imports' text**, not the compiler's
  dependency graph, which a stopped check doesn't return. An import with
  build tags (`import x if mac`) counts on every platform: at worst a
  module is left out that needn't be.
- **The importers of a module that doesn't parse get nothing**, as `inko
  check` gives them, rather than their last diagnostics: their input
  changed. A client that wants to keep showing those can.
- **No stale whole-project answers.** The request allowed answering from
  the last good check where the symbol is in unchanged text; an `impl` or
  call added since anywhere would be missing, so that wouldn't be
  complete. Refusing only where an error could change the answer is the
  one used.
- **The names a whole-project answer is about are textual**, as
  references' have been: `Box` is refused past an error in another type's
  `impl Named for Thing` when Box's own `impl Named for Box` mentions
  `Named` (a broken name's unit), which is conservative.
- **Code lenses aren't refreshed by the server** (`workspace/codeLens/refresh`):
  a client asks again on its own schedule. An edit elsewhere can change a
  document's lenses (an `impl` added in another module).
- **The lens command shows only the first location**: LSP has no request
  to show several, and the locations are in the arguments for a client
  that wants the list.
- **Default methods are marked in `detail`** with ` { ... }`, which is
  display text; a client that parsed `detail` as a signature has to strip
  it, for a trait's methods only.
- **Past an error, code lenses across all of std have a limit of their
  own** (1.5 s): nearly every std module names `String` or `ToString`, so
  each needs a guard. In a project, only its own modules count.

## Remaining unknowns

1. **IntelliJ's code lens support** (and whether Parakeet uses lenses or
   reads them as data) wasn't tried; the tests are against the spec, its
   meta model and the client's capabilities.
2. **The machine was heavily loaded** (load averages of 300 to 600) the
   whole session, so the measurements are upper bounds.
3. **Diagnostics aren't in the edit-replay corpus**, which replays
   queries; the oracle and tests/publish.rs cover them.

## 9. The 0.16.0 release

0.15.0's release run first failed only because its latency job never got
a runner (it was cancelled after 28 minutes, with no steps); it was run
again, outside this session, and 0.15.0 was published before 0.16.0, so
the index lists both in order.

**The dry run** on this report's commit, f7c64a4 (run 40767709553,
workflow_dispatch, 33 minutes), passed every job: the release checks,
the latency job (the limits as written, scaled by its runner's
baseline), all 16 mutation shards and the dist shard (each control
passing, every mutation caught), and the four builds.

**Not released.** Pushing the tag `v0.16.0` (on f7c64a4) was refused by
this session's permission system as creating a public surface, so
nothing is published and nothing was checked from outside. To release:

    git tag -a v0.16.0 -m "Cuttlebone 0.16.0" f7c64a4
    git push origin v0.16.0

The workflow then runs the release tier again, publishes, and checks the
published release and the index on four runners.
