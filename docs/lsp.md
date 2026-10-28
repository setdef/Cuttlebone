# Cuttlebone's language server

`cuttlebone` is a language server. It speaks standard
[LSP 3.18](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/)
over stdin and stdout, for any editor with an LSP client. It assumes nothing
about the editor. This document covers what it supports, the few things
that LSP leaves to the server, and the few [extensions](#extensions) where
LSP has no way to do something.

It sends only what the client's capabilities say the client supports
(Markdown hovers, completion's label details and kinds, document symbols'
hierarchy and symbol kinds, signature help's
parameter offsets and active parameters, diagnostic tags, folding range
kinds and limits, code actions as literals and which is preferred), and every
message the tests receive is checked against the 3.18 specification's
meta model (tests/common/spec.rs).

## Starting it

```
cuttlebone            # or: cuttlebone --stdio [--clientProcessId=PID]
```

- stdio is the only transport. `--stdio` is accepted because some clients
  (such as VS Code's) pass it, as LSP recommends.
- **The client's process.** If the client's process started the server
  and goes away, the server exits (with status 1), even if stdin stays
  open, as LSP says of `initialize`'s `processId`. It looks once a
  second. `--clientProcessId=PID` (or `--clientProcessId PID`), which LSP
  recommends too, gives the id for a client that sends `processId: null`.
  A process that isn't the server's parent isn't watched: stdin closing
  ends the session then, as it always does.
- stderr carries free-form logs: compiler panics, killed checks, worker
  output. Clients may show it or discard it, but shouldn't parse it.
- `cuttlebone --version` prints `cuttlebone 0.16.0 (Inko 0.21.1)`.

One process serves one editor session, with any number of workspace folders.

## `initialize`

### Initialization options

All of these are optional.

| Option | Type | Default | |
|---|---|---|---|
| `inko` | string | `inko` on `PATH` | The `inko` executable to check the version of and take the standard library from. |
| `checkTimeoutMs` | number | 5000 | How long one check may run before it's killed (see [Robustness](#robustness)). |
| `debounceMs` | number | 50 | How long a document must go without edits before a query on it starts (see [Debouncing and cancellation](#debouncing-and-cancellation)). |
| `statusNotifications` | boolean | false | Send a [`cuttlebone/status`](#status) notification before each query's response, and after each [diagnostics](#diagnostics) check. |
| `diagnostics` | boolean | true | Check projects and publish [diagnostics](#diagnostics). |
| `diagnosticsDebounceMs` | number | 300 | How long a project must go without edits before its diagnostics are checked again. |

### The Inko version

Cuttlebone links one Inko release's compiler (0.21.1 for this build). During
`initialize` it runs `inko --version` (with a 10 s timeout). If the version
is a different one, it refuses the session. `initialize` then gets an error
response instead of a result:

```json
{"jsonrpc": "2.0", "id": 1, "error": {
  "code": -32803,
  "message": "Cuttlebone 0.16.0 works with Inko 0.21.1 only, but /usr/local/bin/inko is Inko 0.20.0. Install Inko 0.21.1, or use a build of Cuttlebone for Inko 0.20.0.",
  "data": {"retry": false, "code": "inkoVersionMismatch", "cuttlebone": "0.16.0", "supported": "0.21.1", "installed": "0.20.0"}
}}
```

- `message` is written to be shown to people as it is; editors usually show
  it in a notification.
- `data.code` is `inkoVersionMismatch`, or `inkoNotFound` if `inko` couldn't
  be run, failed, or printed a standard library directory that doesn't
  exist. `installed` is `null` when it isn't known.
- `retry: false` follows LSP's `InitializeError`. A client may still send
  `initialize` again (for example after the user picks another toolchain),
  and the server answers it as if it were the first.
- Until an `initialize` succeeds, other requests get `ServerNotInitialized`
  (-32002).

On a match, Cuttlebone runs `inko print std` to find the standard library to
check against.

### Project roots

The workspace folders (`workspaceFolders`, else `rootUri`, else `rootPath`)
are the project roots. A file in a root's `src/` or `test/` is checked as
part of that root's project: its `inko.pkg` and `dep/` apply. The innermost
root wins. Any other file is checked as part of the nearest directory whose
`src/` or `test/` contains it, or else its own directory. The compiler
resolves each file's imports from its own nearest `src/` either way.

### Position encoding

Cuttlebone negotiates `positionEncoding` from the client's
`general.positionEncodings`. It picks the first of `utf-8` and `utf-16` in
the client's order, and UTF-16 if the client offers neither or nothing.
Neovim offers UTF-8 first, so it gets UTF-8; VS Code and most others get
UTF-16. `utf-32` isn't supported.

### What it advertises

Only what works (`workspace.fileOperations` only for a client whose
`workspace.fileOperations.willRename` is `true`):

```json
{
  "capabilities": {
    "positionEncoding": "utf-16",
    "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": true}},
    "hoverProvider": true,
    "completionProvider": {"triggerCharacters": [".", "@"], "resolveProvider": false},
    "definitionProvider": true,
    "declarationProvider": true,
    "implementationProvider": true,
    "typeDefinitionProvider": true,
    "referencesProvider": true,
    "documentHighlightProvider": true,
    "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
    "inlayHintProvider": true,
    "documentSymbolProvider": true,
    "foldingRangeProvider": true,
    "selectionRangeProvider": true,
    "documentFormattingProvider": true,
    "codeActionProvider": {"codeActionKinds": ["quickfix"]},
    "workspaceSymbolProvider": true,
    "callHierarchyProvider": true,
    "typeHierarchyProvider": true,
    "workspace": {"fileOperations": {"willRename": {"filters": [
      {"scheme": "file", "pattern": {"glob": "**/*.inko", "matches": "file"}},
      {"scheme": "file", "pattern": {"glob": "**", "matches": "folder"}}]}}},
    "experimental": {"cuttlebone": {"inko": "0.21.1", "std": "/path/to/std", "statusNotifications": false, "diagnostics": true}}
  },
  "serverInfo": {"name": "cuttlebone", "version": "0.16.0"}
}
```

`experimental.cuttlebone` isn't a capability. It tells a client which Inko
and standard library the session checks with, and whether status
notifications and [diagnostics](#diagnostics) are on. Published
diagnostics need no capability: the server publishes them unless
`diagnostics` is `false`, or the client pulls them
(`diagnosticProvider`, only for a client with `textDocument.diagnostic`).

### Clients that turn on every capability

Some clients (JetBrains' LSP support, for one) use every capability a
server advertises, with no setting per feature, and diagnostics need no
capability at all. If the client, or a plugin for Inko in it, already
provides a feature itself, the user sees it twice: two hovers, two sets of
inlay hints, every error reported twice.

Cuttlebone can't know what else is running, so turning a feature off is the
client's job, on its side:

- **Requests** (hover, completion, definition, declaration, type
  definition, implementation, references, call and type hierarchies,
  file renames, document highlight, signature help, inlay hints, document
  and workspace symbols, folding ranges, selection ranges, formatting, code
  actions): don't send
  them, or don't show their answers. A client that registers capabilities
  through its own configuration can leave them out; one that can't can drop
  the server's capability from the `InitializeResult` before its LSP layer
  sees it.
- **Diagnostics**: send `"diagnostics": false` in `initializationOptions`.
  Cuttlebone then never checks for or publishes them, which also saves the
  full checks.

## Documents

`didOpen`, `didChange` (full or incremental), `didSave` and `didClose` keep
the text of open documents. Every query type checks with the text of every
open document instead of what's on disk, so unsaved changes to a module are
seen by the files that import it. `didSave` with `text` replaces the
document's text; without it, nothing changes.

- **Files must exist on disk.** The compiler finds modules on disk and then
  reads their text from the open documents. An open document that isn't
  saved yet (an `untitled:` URI, or a path that doesn't exist) isn't type
  checked. A query on one answers `null` (reason `notAFile` or
  `fileNotFound`).
- **Lines end at `\n`, `\r\n` or a lone `\r`**, as LSP says, in every
  position and range. The compiler counts only `\n` (a lone `\r` is
  whitespace to it), and Cuttlebone converts between the two. No position
  is between a `\r` and its `\n`: the end of a line is before both.

## `textDocument/hover`

The type of the expression at the position, and the declaration of what
the name there refers to or declares, with its doc comment, as Markdown:

```json
{"contents": {"kind": "markdown", "value": "```inko\nResult[ref Int, OutOfBounds]\n```\n\n---\n\n```inko\nfn pub get(index: Int) -> Result[ref T, OutOfBounds]\n```\n\nDefined in `std.array.Array`\n\n---\n\nReturns an immutable reference to the value at the given index.\n..."},
 "range": {"start": {"line": 4, "character": 18}, "end": {"line": 4, "character": 26}}}
```

The value has up to three sections, each that there is, in this order,
with `\n\n---\n\n` between them:

1. **The type** of the expression at the position, in a code block, as
   before this milestone. A client that wants the type alone takes the
   first code block if it's the first section: it's there whenever the
   position is in an expression.
2. **The declaration**, if the name at the position (as
   [definition](#textdocumentdefinition) finds it) is a module, type,
   trait, method, field, constructor or constant: its signature as the
   source writes it, up to its body, in a code block (`fn pub static new(`
   and its parameters over several lines stay so; type arguments are as
   written, `T`, not the call's), then `Defined in` and where it is: its
   module, and what it's in (`std.array.Array`; a method of an `impl`
   block is in its type). For a module, `Module` and its name instead.
   Variables and type parameters have no declaration section: their type
   is the first.
3. **The doc comment**, as Markdown, the same text `inko doc` reads: the
   comments right above the declaration, a line each, without the `#`
   and the space after it (for a module, the comments at the start of its
   file). A method implementing a trait's without a doc comment of its own
   has the trait's method's, as `inko doc` gives it. Left out if there's
   none.

- **Where:** declarations in the project, in dependencies (`dep/`) and in
  the standard library alike, from the file the compiler read.
- **The format** is the first of `markdown` and `plaintext` in the
  client's `textDocument.hover.contentFormat`, its order of preference. A
  client that lists neither, or sends no `contentFormat`, gets plain text:
  the same sections without code blocks and with `\n\n` between them
  (`Defined in std.array.Array`).
- **`range`** is the expression's range, or with no expression there (a
  declaration's own name, such as `get` in `fn pub get`), the name's.
- The type is formatted the way the compiler formats types in its
  diagnostics, e.g. `ref Array[Int]`, `Result[Int, String]`,
  `uni Future[Int]`. A type the compiler couldn't infer fully may contain
  `?`.
- **From another check** (a repaired copy, the last good check, past a
  declaration's error), a hover is given only if, as well as the rules
  every answer follows, the text of the declaration's doc comment and
  signature (and the trait's method's, for one it has) is unchanged since,
  with the line before it: a line typed right above may be a comment.

The result is `null` when nothing is there. For example, in a comment,
between declarations, or on a keyword. It's also `null` when there's no
answer, and [status](#status) says why.

Besides expressions, variable bindings count: the name in `let name = ...`,
in a `match` or `for` pattern, and a closure's argument. Their type is the
variable's.

A position means the character that starts there. A position inside a
character (for example the second half of a surrogate pair), or inside a
grapheme cluster such as 👩‍👩‍👦, means the whole cluster.

### Which expression a position means

The compiler desugars some constructs into several nodes that share source
ranges. For example `[10, 20]` becomes a scope that defines a variable
`$array` with `Array.with_capacity(2)`, calls `$array.push(10)` and
`$array.push(20)`, and returns `$array`. All of these have the literal's
range, except that each push has its element's range.

So Cuttlebone marks the nodes that desugaring introduces as synthetic, and
picks:

1. among the expressions containing the position that aren't synthetic,
2. the innermost one (the latest start, then the earliest end),
3. and of several with the same range, the outermost.

What that gives for each desugared construct:

| Source | At | Gives |
|---|---|---|
| `[10, 20]` | `[`, `]`, `,` | the array (`Array[Int]`) |
| | `10` | the element (`Int`), not the push (`Nil`) |
| `"hi ${name}"` | a quote | the string (`String`) |
| | `hi ` | the text (`String`) |
| | `name` | the value (`name`'s type), not the generated `to_string` or push |
| `"${name}"` | `name` | the value |
| `for x in xs.iter { ... }` | `for` | the loop (`Nil`) |
| | `x` | the binding (the element's type) |
| | `xs.iter` | the iterator expression, not `$iter`, `into_iter` or `$iter.next` |
| `while c { ... }` | `while` | the loop |
| | `c` | the condition, not the generated `break` |
| `if c { ... }` without `else` | `if` | the `if` (`Nil`), not the generated `nil` |
| `!x` | `!` | the negation (`Bool`), not the generated `false` |
| `await m` / `async m` | `await` / `async` | the awaited value / the future |
| | `m` | the method call, not the generated promise argument |
| a body's last value, in a method returning `uni T` | the value | the value as written, not the `recover` the type checker wraps it in |
| `a += 1` | `+=` | the assignment (`Nil`), which has the same range as the `+` call inside it |

## `textDocument/completion`

Three kinds: the members after a `.`, a name being typed (with symbols to
import), and the modules and symbols of an `import`. `.` and `@` are the
trigger characters; completion invoked by hand, or as a client asks while
a name is typed, works too.

### Members, after a `.`

The members available after a `.` and any typed prefix.

A buffer with nothing after a `.` doesn't parse. Cuttlebone therefore inserts
an identifier at the cursor in its own copy of the document, so the
receiver keeps its type:

- `value.|` is checked as `value.cuttlebone_completion_placeholder`
- `value.si|` is checked as `value.sicuttlebone_completion_placeholder`

Clients send the buffer as it is.

```json
{"isIncomplete": false, "items": [
  {"label": "size", "kind": 2, "detail": "fn pub inline size -> Int", "labelDetails": {"description": "std.string"}},
  {"label": "label", "kind": 5, "detail": "let pub @label: String", "labelDetails": {"description": "main"}}
]}
```

| Field | |
|---|---|
| `label` | The member's name, which is also what's inserted. |
| `kind` | Method (2), Field (5), Constant (21) or Class (7, for a type in a module). Constant only for a client whose `completionItemKind.valueSet` has it (the kinds every client has end at Reference, 18); others get Value (12). |
| `detail` | The declaration, with the receiver's type arguments filled in, including in default methods from generic traits (e.g. `fn pub inline get(index: Int) -> Result[ref Int, OutOfBounds]` on an `Array[Int]`). |
| `labelDetails.description` | The module that defines it, for a client with `completionItem.labelDetailsSupport`. Others get no `labelDetails`. |

Items are sorted by name, and each name appears once. They aren't filtered
by the typed prefix, and `isIncomplete` is `false`, so the client filters as
the user types. Which members are listed follows the compiler's own rules
for `receiver.name`:

- **On a value:** its type's instance methods (including those from
  implemented traits), and its fields if the type isn't `async` and the
  field is visible from this module. Private members are visible within
  the same root namespace (e.g. `foo.bar` from `foo.baz`). An `async`
  type's private methods are only listed on `self`.
- **On a type** (`Counter.`): its static methods.
- **On a module** (`shapes.`): its methods, and the constants and types it
  defines.
- **On a type parameter or trait:** the methods of its requirements.
- Methods that need a `mut` or owned receiver aren't listed on a `ref`
  receiver.

### Names

Anywhere else in code a name is typed (or about to be), the names it could
be, as the compiler resolves a name without a receiver: the same
placeholder is inserted after what's typed (`am|` is checked as
`amcuttlebone_completion_placeholder`), and the check says what's in scope
there.

```json
{"isIncomplete": true, "items": [
  {"label": "amount", "kind": 6, "detail": "let amount: Int", "sortText": "0amount", "labelDetails": {"description": "main"}},
  {"label": "add", "kind": 2, "detail": "fn pub mut add(amount: Int)", "sortText": "1add", "labelDetails": {"description": "main"}},
  {"label": "Counter", "kind": 7, "detail": "type Counter", "sortText": "2Counter", "labelDetails": {"description": "main"}},
  {"label": "StringBuffer", "kind": 7, "detail": "type pub StringBuffer", "sortText": "3StringBuffer",
   "labelDetails": {"description": "std.string"},
   "additionalTextEdits": [{"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 0}},
                            "newText": "import std.string (StringBuffer)\n"}]}
]}
```

| Group (`sortText`'s first digit) | What | `kind` |
|---|---|---|
| 0 | The variables in scope at the position: a method's or closure's arguments in it, a `let`'s from after it to the end of its block, a `match` case's bindings in that case. The innermost of a name. | Variable (6) |
| 1 | What the type of the method the position is in has, as `self` calls it without a receiver: an instance method's instance methods (and those of the traits it implements, private ones too), a static method's static methods, a module method's module's methods. | Method (2) |
| 2 | What the module binds: its own types, traits, constants and methods, what it imports (modules, and symbols), and the prelude's (`Int`, `String`, `Array`, `Option`, `panic`, ...). | Class (7), Interface (8), Constant (21, or Value), Function (3), Module (9) |
| 3 | Public types, traits, methods and constants of the modules the file can import but doesn't, whose names match what's typed (as [workspace symbols](#workspacesymbol) match), at most 100, each with the edit that imports it. Not while nothing's typed. | as above |

- **Keywords**, as Keyword (14), after the names (`sortText` starting
  with 4), where the grammar takes them: where an expression starts,
  those that start one (`async`, `await`, `break`, `false`, `fn`, `for`,
  `if`, `let`, `loop`, `match`, `mut`, `next`, `nil`, `recover`, `ref`,
  `return`, `self`, `throw`, `true`, `try`, `while`); in a type's name,
  those that start a type (`fn`, `move`, `mut`, `ref`, `uni`), or only
  `fn` right after `ref`, `mut`, `uni` or `move`. The type checker may
  still refuse one there (`break` outside a loop).
- **After `@`**, the fields of the type of the method it's in, private
  ones too, and nothing else (`isIncomplete` `false`).
- **In a type's name** (a signature, a field's type, an annotation), only
  types, traits, modules and the type parameters of the method it's in
  and of its type (TypeParameter, 25, or Class).
- **Never what's out of scope:** a variable declared after the position,
  in another method, or in another closure or case; nothing private from
  another module; no symbol whose name the module binds already (it'd
  clash, or is there); nothing a module can't import: a project's
  `test/` modules only from its tests, a dependency's only from the
  project that requires it, by its name.
- **The import edit** (`additionalTextEdits`) is as `inko fmt` orders
  imports: into the module's import's symbol list, in order (`self`
  first), if it has one; `(self, Name)` after an import of the module
  alone; or else a new `import module (Name)` line in the first group of
  imports, by path, or before the first declaration and its doc comment,
  with a blank line after it. An import with build tags isn't added to.
- `isIncomplete` is `true`: what to import depends on what's typed, so the
  client asks again as more is.
- `null` in a comment, a string, or anywhere else that isn't code.

### In an `import`

On a line starting with `import`, or in its symbol list over several
lines, the answer comes at once from the [index](#workspacesymbol) of
modules, with no check (so in code that doesn't type check or parse):

- after `import ` and in the path, the next name of each module the file
  can import (`import std.` lists `fmt`, `net`, ...), as Module (9), with
  the module's name as `detail`
- in the symbol list, `self` and the module's public types, traits,
  methods and constants that aren't listed yet

### In code being edited

| Answer from | Members | Names |
|---|---|---|
| the documents as they are | yes | yes |
| past an error in a declaration ([Errors in declarations](#errors-in-declarations)) | where the error can't matter | where the error can't matter, and no error can bind or hide a name the answer has (below) |
| a repaired copy ([Answers in code being edited](#answers-in-code-being-edited)) | where the blank lines can't matter | the same, for the blank lines too |
| the last good check (`stale`) | never: items are inserted into the code | never |

**Where a declaration differs** (a line outside a method's body blank in
a repaired copy, or an error in a declaration), names are refused only
where it can bind or hide a name, or change what the answer says of
one. Each such change, or error, refuses them if it's
- in the file asked about, outside every type, trait and `impl` block
  (an import, a module's method or constant, a declaration being typed):
  it may bind a name anywhere in the module;
- a declaration of a name the answer lists, binds (`bound`, which
  decides the symbols to import) or mentions in a `detail` (a signature,
  a type), or the header of a type or trait of that name; or
- inside the type or trait of the method the position is in, or inside
  a trait whose methods that type has (it implements it, or a trait it
  implements requires it), or an `impl` block of one: a member added
  there is in scope through `self`.

A change or error anywhere else, such as a method of another type being
typed, or an error in another module's declaration the file doesn't
import, leaves the names as they are. As for every answer, the position's
own method must not mention what the error is about, and the blank
lines of a repaired copy mustn't be before it in the same method.

So names are answered while a method's body is typed (the unfinished line
is blank in the copy, before the position), and while another type's
declarations are, and not while a declaration they depend on is
unfinished, or has an error. The symbols to import and the `import`
lines come from the text alone, as
[document symbols](#textdocumentdocumentsymbol) do.

The result is `null` when the position isn't code, or when there's no
answer ([status](#status) says why). It's never a guess.

## `textDocument/definition`

Where the symbol at the position is declared, as one `Location` (or
several, for a [compound assignment to a setter](#a-compound-assignment-to-a-setter)):

```json
→ {"jsonrpc": "2.0", "id": 4, "method": "textDocument/definition", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "position": {"line": 44, "character": 34}}}
← {"jsonrpc": "2.0", "id": 4, "result": {
    "uri": "file:///home/me/hello/src/main.inko",
    "range": {"start": {"line": 14, "character": 9}, "end": {"line": 14, "character": 14}}}}
```

The range is the declared name (`moved` in `fn pub moved(by: Int)`), not
the whole declaration. A module's range is empty, at the start of its file.

| At | Goes to |
|---|---|
| a method call, `x.moved(2)`, `moved(2)`, or an operator such as `+` | the called method. For a static call, `Point.new`, the static method. |
| a call through a type parameter's bound (`shape.area` with `T: Area`) | the trait's method |
| a call to a default method a type inherits from a trait without redefining it (`iter.map`) | the trait's method |
| a call to a method a type implements for a trait | the type's implementation |
| a variable | its binding: `let`, a pattern (`case Some(x)`), a method's argument, a closure's argument, a `for` loop's variable |
| a named argument, `new(x: 1)` | the method's argument |
| a field, `@x`, or `point.x`, or `x:` in `Point(x: 1)`, or in a pattern | the field's declaration (`@x` in `let pub @x: Int`) |
| a type, trait or type parameter, in a signature (`-> Point`, `T: Area`, `Array[T]`) or an expression (`Point.new`, `Point(x: 1)`) | its declaration. `Self` is the type it stands for. |
| a constant | its declaration |
| a constructor, `Option.Some(x)` or `case Some(x)` | the constructor (`case Some(T)`) |
| a module in an import (`shapes` in `import geometry.shapes`), or used as a receiver (`beta.Error`) | the module's file. Each segment of an import's path goes to the module it names, if that module was checked: `geometry` alone has none. |
| an imported symbol, `Circle` in `import geometry.shapes (Circle)`, or its alias | the symbol's declaration. `self` is the module. |
| a declaration's own name | that declaration |

Symbols are matched by the compiler's ids, never by name, so two modules'
`Error` types are never confused.

### A refused assignment

At an assignment the compiler refused (to an immutable variable or field,
or a variable a non-moving closure captured), the compiler recorded no
reference, but it did look the name up: definition gives the innermost
variable of that name in scope there, or for `@x`, the field of the
method's type, as the compiler found it.

### A compound assignment to a setter

`c.value += 1` calls the getter `value`, and then the setter `value=` with
the sum: the compiler gives both calls the same name. Definition there is
both declarations, as a `Location[]` (which LSP allows), the setter's
first. Everywhere else it's one `Location`, as above: `c.value = 2` is the
setter, and `c.value` the getter. References find the name from either
method, and a document highlight there is the setter's (both assignments,
as writes).

The result is `null` when there's nothing at the position (a comment, a
literal, a keyword), or the name doesn't resolve (an undefined method, say).
It's never a guess. It's also `null` when there's no answer, with the same
[status](#status) reasons as hover.

**Where locations point.** A location's file is the one the compiler read:

- the project's own files
- a dependency's files in the project's `dep/`
- the standard library's files in the directory `inko print std` printed,
  which `experimental.cuttlebone.std` reports

So every location is a file on the user's machine that matches what they
compile against. Positions use the negotiated encoding, in whichever file
the location is in.

## `textDocument/typeDefinition`

Where the type of the expression at the position is declared, as one
`Location`. The type's ownership (`ref`, `mut`, `uni`) and type arguments are
stripped: for a value of type `ref Option[String]`, the result is `Option`'s
declaration in std.option.

| The type | Goes to |
|---|---|
| a type, with any ownership and type arguments (`Point`, `ref Option[String]`, `uni Array[Int]`) | the type's declaration (`Option`) |
| a trait (`ref ToString`) | the trait's declaration |
| a type parameter (`T`, or `ref T`), even one with an `impl ... if T: Trait` bound | the type parameter's declaration (`T` in `fn total[T: Area]`) |
| a tuple (`(Int, String)`) | the `TupleN` type in std.tuple that it's an instance of (`Tuple2`) |
| a module used as a receiver | the module's file |
| a closure (`fn (Int) -> Int`), `Never`, a C type (`Int32`, `Pointer[T]`) | `null`: they have no declaration |

Which expression a position means is the same as for
[hover](#which-expression-a-position-means), so a method call is its return
type. On a type name with no expression there, such as one in a signature,
the result is that type's (or trait's or type parameter's) declaration.

The result is `null` when nothing is at the position or the type has no
declaration, and `null` with status `unavailable` (reason `untyped`) when the
expression has no type.

## `textDocument/references`

Every reference to the symbol at the position, as an array of `Location`s,
sorted by file and position:

```json
→ {"jsonrpc": "2.0", "id": 5, "method": "textDocument/references", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "position": {"line": 45, "character": 16},
    "context": {"includeDeclaration": true}}}
← {"jsonrpc": "2.0", "id": 5, "result": [
    {"uri": "file:///home/me/hello/src/alpha.inko", "range": {"start": {"line": 1, "character": 9}, "end": {"line": 1, "character": 14}}},
    {"uri": "file:///home/me/hello/src/alpha.inko", "range": {"start": {"line": 4, "character": 37}, "end": {"line": 4, "character": 42}}},
    {"uri": "file:///home/me/hello/src/alpha.inko", "range": {"start": {"line": 5, "character": 4}, "end": {"line": 5, "character": 9}}},
    {"uri": "file:///home/me/hello/src/main.inko", "range": {"start": {"line": 1, "character": 14}, "end": {"line": 1, "character": 19}}},
    {"uri": "file:///home/me/hello/src/main.inko", "range": {"start": {"line": 45, "character": 16}, "end": {"line": 45, "character": 21}}}
]}
```

- **The symbol** is found as for [definition](#textdocumentdefinition), on a
  reference or on a declaration's name. References are the same kinds of
  names definition goes from, and are matched by the compiler's ids.
- **Where it looks:** the project's own modules, in its `src/` and `test/`.
  Not dependencies or the standard library, even when the symbol is
  declared there. A file outside a project's `src/` and `test/` is its own
  project.
- **Asked at a declaration outside the project**, in the standard library
  or in a dependency in a project's `dep/` (say std's `Array.size`, in
  its own file), it's the project's uses that are found: those of every
  workspace folder's project for the standard library (each checked
  whole), and of the project whose `dep/` it is for a dependency. The
  standard library's and the dependency's own uses aren't listed, nor is
  the declaration, which isn't the project's. A module no project uses
  has no references: `[]`.
- **The whole project is type checked**, not just the file and what it
  imports, so modules that don't import the file are found. This costs
  more than the other queries ([cost](#freshness-cost-and-side-effects)).
- **`context.includeDeclaration`** adds the declaration's name, if it's in
  the project. A request without `context` is treated as
  `includeDeclaration: false`.
- **Distinct ids are distinct symbols.** A call through a type parameter's
  bound (`T: ToString`, `x.to_string`) refers to the trait's method, and a
  call on a `String` to `String`'s own `to_string`, so the two don't find
  each other. Calls to an inherited default method do count as references to
  the trait's method, and an enum constructor's calls and patterns are the
  same symbol.
- **Desugaring** doesn't add references: a `for` loop's `Some(value)`,
  `$`-variables, the `==` that `!x` becomes, and string interpolation's
  `to_string` aren't listed.

The result is `null` when nothing is at the position, `[]` when the symbol
has no references in the project, and `null` with a [status](#status)
reason when there's no answer. Because the whole project is checked, a
syntax error anywhere in it makes references unavailable, even if the file
itself is fine. An error in a declaration anywhere does only if it could
hide a reference ([Errors in declarations](#errors-in-declarations)).

## `textDocument/implementation`

What implements the trait, or the trait's method, at the position, as
`Location[]` (the names, as definition's ranges are), sorted by file and
position:

| At | Gives |
|---|---|
| a trait (its declaration's name, or a name that refers to it: `T: Named`, `impl Named for Box`) | every type the type database records as implementing it |
| a trait's method, required or default (its name, or a call through a bound: `value.name` with `T: Named`) | every type's method that implements it: a required method's implementations, and a default method where a type redefines it. Types that inherit a default method as it is have no method of their own to go to. |
| anything else: a type, a type's own method, a variable | `null` |

- **By the compiler's ids:** the type database's own record of each
  `impl` (`TraitId::implemented_by`, `MethodSource::Implemented`), never
  names. tests/corpus_hierarchy.rs checks, over the whole of std, that
  every implementation the database records is found, and nothing else.
- **Where:** every module of the check, which checks the whole project as
  [references](#textdocumentreferences) do: the project's own modules,
  and the dependencies' and std's that it imports, directly or not. Asked
  at a declaration in std or a dependency, it checks every project that
  could use it, as references do. A type in a std module that no project
  imports isn't found: it isn't part of what the project compiles.
- **Complete or none:** only from the documents as they are, never from a
  repaired copy or the last good check. Past an error in a declaration
  anywhere in the project, as references are
  ([Errors in declarations](#errors-in-declarations)): refused (reason
  `declarationErrors`) only if a unit the error makes unsafe mentions
  the trait's name (or the method's, for a trait's method): an `impl`
  with an error may be missing from the database, and it names the trait
  (and its methods, which then go untyped, name themselves). An error elsewhere can't hide one, and the answer is
  `fresh`.
- **Why never stale:** an answer from the last good check would miss an
  `impl` added since, wherever it is, so it couldn't be complete for the
  documents as they are.

## `textDocument/declaration`

In Inko, a method a type writes for a trait (`fn pub name` in `impl Named
for Box`) is declared by the trait's method (`fn pub name -> String` in
`trait Named`), and defined by the type's. So declaration is:

| At | Goes to |
|---|---|
| a type's method that implements a trait's method (its name, or a call to it: `box.name`) | the trait's method (`MethodSource::Implemented`) |
| anything else | the same as [definition](#textdocumentdefinition) |

Definition at the same places goes to the type's method. The result has
the same shapes, statuses and answers in code being edited as definition.

## Call hierarchy

`textDocument/prepareCallHierarchy` gives the method at the position (a
call, or its declaration's name) as a `CallHierarchyItem[]` of one, or
`null` if there's no method there:

```json
{"name": "total", "kind": 12, "detail": "main", "uri": "file:///home/me/hello/src/main.inko",
 "range": {"start": {"line": 10, "character": 0}, "end": {"line": 14, "character": 1}},
 "selectionRange": {"start": {"line": 10, "character": 3}, "end": {"line": 10, "character": 8}}}
```

- `kind` is Function (12) for a module's method, Method (6) for a type's
  or trait's.
- `detail` is its module and what it's in (`std.array.Array`), as hover's
  `Defined in`.
- `range` is the whole declaration and `selectionRange` its name, as
  [document symbols](#textdocumentdocumentsymbol) have them.
- No `data`: `callHierarchy/incomingCalls` and `outgoingCalls` find the
  method again at the item's `selectionRange.start`, in its document as
  it is then. If that's no longer a method's declared name, the result is
  `null`.

`callHierarchy/incomingCalls`: every call to the method, by the compiler's
ids, in the project's own modules (as [references](#textdocumentreferences)
finds them), grouped by the method each call is in, as
`CallHierarchyIncomingCall[]`: `from` is that method's item, `fromRanges`
the calls' names in its file. Sorted by where the callers are. Like
references, it checks the whole project; distinct ids are distinct
methods, so a call through a bound calls the trait's method, not an
implementation. Complete or none, as for references: past an error in a
declaration elsewhere, refused only if a unit it makes unsafe mentions
the method's name, so also in a document being edited, where a method
whose signature doesn't check yet refuses only the callers of what it
calls.

`callHierarchy/outgoingCalls`: every method the method's body calls, as
`CallHierarchyOutgoingCall[]`: `to` is the called method's item,
`fromRanges` the calls in the method's file. Operators (`+`) are calls
too. A method with no body (a trait's required method) calls nothing:
`[]`. It checks the method's file, as definition does, and answers from
the last good check when the method itself is unchanged since, but never
from a repaired copy: a blank line in the method may have a call, even
after its name (the edit-replay corpus found this: tests/replay.rs).

## Type hierarchy

`textDocument/prepareTypeHierarchy` gives the type or trait at the
position as a `TypeHierarchyItem[]` of one (`kind` Class (5), Enum (10)
or Interface (11) for a trait; the other fields as for the call
hierarchy), or `null`.

| Request | For a type | For a trait |
|---|---|---|
| `typeHierarchy/supertypes` | the traits it implements | the traits it requires (`trait Labelled: Named`) |
| `typeHierarchy/subtypes` | `[]`: Inko's types have none | the traits that require it, then the types that implement it |

Each is sorted by where it's declared, from the type database
(`TypeId::implemented_traits`, `TraitId::required_traits`,
`TraitId::implemented_by`), checking the whole project as implementations
do (tests/corpus_hierarchy.rs checks it against the database over std),
and past an error in a declaration, refused only where a unit it makes
unsafe mentions the type's or trait's name (an `impl` naming it, or a
trait requiring it).
The item is found again at its `selectionRange.start`, as for the call
hierarchy.

## `workspace/willRenameFiles`

Before a client renames or moves files or directories, the edits that
keep every import of each module that moves working, as a
`WorkspaceEdit` (`changes`, in the negotiated encoding), or `null` if
there are none. Inko resolves a module from its path in its project's
`src/` or `test/` (`src/app/util.inko` is `app.util`), so:

- **Each import's path** that resolves to the module, by the compiler's
  ids, in every module of the project, becomes the new module's:
  `import app.util (helper)` becomes `import core.tools (helper)`.
- **Uses by its own name** (`util.helper` after `import app.util`) become
  the new name's last part, if that changes (`tools.helper`). An alias
  (`import app.util (self as u)`) stays as it is.
- **A directory** moves every module in it.
- **No edits** for a file moved out of its project's `src/` and `test/`,
  to a name a module can't have (`My-Util.inko`), or that nothing imports.

Only a client whose `workspace.fileOperations.willRename` is `true` is
asked to send it: the server registers for files matching `**/*.inko` and
folders. It checks the whole project, as references do, and answers only
from the documents as they are (reason `noHir` otherwise, and `null`).
Past an error in a declaration, it answers, `fresh`, unless a unit the
error makes unsafe mentions the module's last name (`util`, which an
import's path and a use by name end with): reason `declarationErrors`.
Never from a repaired copy or the last good check: its edits change code. Nothing is written,
and the files aren't moved: the client does both. tests/rename_files.rs
applies the edits, moves the files, and checks the project.

## `textDocument/documentHighlight`

Every name in the document that refers to or declares the symbol at the
position, as an array of `DocumentHighlight`s sorted by position:

```json
→ {"jsonrpc": "2.0", "id": 6, "method": "textDocument/documentHighlight", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "position": {"line": 41, "character": 4}}}
← {"jsonrpc": "2.0", "id": 6, "result": [
    {"range": {"start": {"line": 32, "character": 12}, "end": {"line": 32, "character": 17}}, "kind": 3},
    {"range": {"start": {"line": 41, "character": 4}, "end": {"line": 41, "character": 9}}, "kind": 3},
    {"range": {"start": {"line": 41, "character": 12}, "end": {"line": 41, "character": 17}}, "kind": 2},
    {"range": {"start": {"line": 42, "character": 4}, "end": {"line": 42, "character": 9}}, "kind": 3},
    {"range": {"start": {"line": 43, "character": 16}, "end": {"line": 43, "character": 21}}, "kind": 2},
    {"range": {"start": {"line": 48, "character": 37}, "end": {"line": 48, "character": 42}}, "kind": 2},
    {"range": {"start": {"line": 51, "character": 26}, "end": {"line": 51, "character": 31}}, "kind": 3}
]}
```

for `total` in tests/fixtures/features/src/main.inko: `let mut total = 0`,
`total = total + 1`, `total += ...`, `counter.add(total, 2)`,
`describe("日本", total, ...)` (in UTF-16, `日本` is two units) and
`case Some(first) -> total = first`.

The symbol, and which names refer to it, are found as for
[references](#textdocumentreferences), by the compiler's ids, but only in
this document, which is type checked with what it imports (not the whole
project). Two symbols with the same name, such as a field `count`, an
argument `count` and a variable `count`, are never highlighted together.

| `kind` | For |
|---|---|
| Write (3) | A variable's binding: `let x`, a pattern's `x` (`case Some(x)`, a `for` loop's), a method's or closure's argument. An assignment's target: `x = 1`, `x := 1`, `x += 1`, `@x = 1`, `a.x = 1`. |
| Read (2) | Every other reference: a variable or field read, a call, a type or module name, a named argument, a field set by name in `Point(x: 1)`. |
| Text (1) | The declaration of anything but a variable: a method's, type's, field's or constant's name. |

The result is `null` when there's nothing at the position, and `null` with
a [status](#status) reason (the same as hover's) when there's no answer.

## `textDocument/signatureHelp`

The signature of the method a call's parentheses belong to, and which
parameter the argument at the position is for. `(` and `,` trigger it.

```json
→ {"jsonrpc": "2.0", "id": 7, "method": "textDocument/signatureHelp", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "position": {"line": 43, "character": 23}}}
← {"jsonrpc": "2.0", "id": 7, "result": {
    "signatures": [{
      "label": "fn mut add(amount: Int, times: Int)",
      "parameters": [{"label": [11, 22]}, {"label": [24, 34]}],
      "activeParameter": 1
    }],
    "activeSignature": 0,
    "activeParameter": 1
}}
```

- **Which call:** the innermost call written with parentheses whose
  parentheses contain the position: after the `(` and up to the `)`. On
  the method's name, before the `(` or after the `)`, it's the enclosing
  call's, if any. A call without parentheses (`counter.reset`) has none.
- **A type's constructor** (`Point(x: 1, y: 2)`) has the type's fields as
  its parameters, in the order the type declares them, labelled as the
  type with its type arguments (as the call infers them) and the fields
  with their types: `Box[String](value: String, size: Int)`. Arguments
  are matched to them as to a method's, by position or by name.
- **The signature** is the called method after the compiler's resolution,
  formatted as completion's `detail` is, with the call's type arguments
  filled in: `map` on an `Iter[ref Int]` given `fn (n) { n * 2 }` is
  `fn pub move map[R](func: fn (ref Int) -> Int) -> ...`. There's one
  signature, since Inko has no overloading.
- **Parameters** are `[start, end)` offsets into the label, in the
  negotiated encoding, for a client with
  `signatureInformation.parameterInformation.labelOffsetSupport`. Inko's
  identifiers are ASCII, so in practice the offsets are the same in every
  encoding. Other clients get each parameter's text instead (`"label":
  "amount: Int"`), which is part of the signature's label.
- **The active parameter:**
  - a positional argument is the parameter at its index
  - a named argument (`times: 1`) is the parameter of that name, in any
    order
  - after a `,` with nothing typed yet, the next positional parameter, or
    after named arguments, the first parameter not named yet
  - none past the last parameter: `null` for a client with
    `signatureInformation.noActiveParameterSupport` (LSP 3.18; the spec's
    text also calls it `signatureHelp.noActiveParameterSupport`, which
    counts too). For others it's left out, which the spec says means the
    first, since they have no way to be told there's none.
- **`activeParameter` on the signature** as well as on the result, for a
  client with `signatureInformation.activeParameterSupport`.
- **While typing.** `counter.add(` and `counter.add(1, ` don't parse. If the
  document doesn't parse but does with a `)` inserted at the position,
  Cuttlebone checks that copy instead (as completion inserts its
  placeholder), so signature help works before the `)` is typed. Calls
  with the wrong number of arguments still resolve.

The result is `null` when the position isn't in a call's parentheses, or
the call doesn't resolve to a method or a type (an undefined method, or a
closure call), and `null` with a
[status](#status) reason when there's no answer.

## `textDocument/inlayHint`

Types the code doesn't write, and parameter names at call sites, in the
requested range:

```json
→ {"jsonrpc": "2.0", "id": 8, "method": "textDocument/inlayHint", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "range": {"start": {"line": 35, "character": 0}, "end": {"line": 44, "character": 0}}}}
← {"jsonrpc": "2.0", "id": 8, "result": [
    {"position": {"line": 35, "character": 15}, "label": ": Array[Int]", "kind": 1, "paddingRight": false},
    {"position": {"line": 38, "character": 13}, "label": ": (Int, String)", "kind": 1, "paddingRight": false},
    {"position": {"line": 43, "character": 16}, "label": "amount:", "kind": 2, "paddingRight": true},
    {"position": {"line": 43, "character": 23}, "label": "times:", "kind": 2, "paddingRight": true}
]}
```

for this code:

```inko
    let numbers = [1, 2, 3]       # numbers: Array[Int]
    let tuple = pair              # tuple: (Int, String)
    counter.add(total, 2)         # counter.add(amount: total, times: 2)
```

Hints are sorted by position, and those whose position is inside the range
(its start and end included) are returned. They're plain strings, without
tooltips, edits or `inlayHint/resolve`.

**Type hints** (`kind` 1) go right after a name, as `: Type`, formatted as
hover formats types:

- a variable a `let` binds to a name with no type annotation: `let x = ...`
  and `let mut x = ...`, but not `let x: Int = ...`, and not the variables in
  a pattern (`let (a, b) = ...`) or in `match` and `for`
- a closure's argument with no type annotation: `fn (n) { ... }`, but not
  `fn (n: Int) { ... }`

A type hint is left out when the code already says the type. That is, when
the `let`'s value is:

- a literal: an integer, float, string, `true`, `false` or `nil` (an array
  literal isn't: `[1, 2]` gets `: Array[Int]`)
- a cast: `x as Int`
- a type's constructor: `Point(x: 1, y: 2)`
- a static method of a type that returns that type, with any type arguments
  or ownership: `Counter.new`, `Map.new`, `Option.Some(1)`. A static method
  returning another type, such as `Int.parse(...)` (an `Option[Int]`), gets
  a hint.

It's also left out when the compiler couldn't infer the type fully (an
error, or a type with a `?` placeholder).

**Parameter hints** (`kind` 2) go right before an argument, as `name:`,
with `paddingRight`:

- only for calls written with parentheses, to a method that resolved
- only for positional arguments: `counter.add(1, times: 2)` gets `amount:`
  only
- not when the argument is a variable or field with the parameter's name:
  `describe(name, count)` gets nothing for `count` if the parameter is
  `count`
- not for operators (`a + b`), closure calls, or enum constructors
  (`Option.Some(1)`), whose parameters have generated names

The result is `[]` when there are no hints in the range, and `null` with a
[status](#status) reason when there's no answer, as for the other queries.
Where a declaration has an error, the hints are those of the parts it
can't affect ([Errors in declarations](#errors-in-declarations)). Where
code doesn't parse, the hints come from a repaired copy or the last good
check, in the parts that still hold
([Answers in code being edited](#answers-in-code-being-edited)). Then the
answer covers only part of the range, and the status's
[`covered`](#what-an-answer-covers) says which: elsewhere, such as in the
method being edited, a client can't take "no hints" to mean there are
none.

## `textDocument/documentSymbol`

The document's declarations, from its text alone: no type check, so it
answers in code that doesn't type check, in documents that aren't files
(an unsaved new one), and, as below, in code that doesn't parse.

```json
→ {"jsonrpc": "2.0", "id": 9, "method": "textDocument/documentSymbol", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"}}}
← {"jsonrpc": "2.0", "id": 9, "result": [
    {"name": "Counter", "detail": "type pub Counter", "kind": 5,
     "range": {"start": {"line": 11, "character": 0}, "end": {"line": 26, "character": 1}},
     "selectionRange": {"start": {"line": 11, "character": 9}, "end": {"line": 11, "character": 16}},
     "children": [
       {"name": "count", "detail": "let mut @count: Int", "kind": 8,
        "range": {"start": {"line": 12, "character": 2}, "end": {"line": 12, "character": 21}},
        "selectionRange": {"start": {"line": 12, "character": 10}, "end": {"line": 12, "character": 16}}},
       {"name": "new", "detail": "fn pub static new(label: String, count: Int) -> Counter", "kind": 6, ...}
     ]}
]}
```

| Declaration | `kind` | `name` | Children |
|---|---|---|---|
| a type (`type`, `type async`, `type copy`, `type extern`, ...) | Class (5) | `Counter` | its fields, constructors and methods |
| an enum (`type enum`) | Enum (10) | `Shape` | the same |
| a trait | Interface (11) | `Area` | its methods |
| an `impl` block, of a trait or reopening a type | Object (19) | `impl Area for Shape`, `impl Counter` | its methods |
| a method of a type, trait or `impl` block, static or not | Method (6) | `add`, `+` | |
| a module's method, or an external function (`fn extern`) | Function (12) | `describe` | |
| a field | Field (8) | `count`, for `@count` | |
| an enum's constructor | EnumMember (22) | `Some` | |
| a constant | Constant (14) | `MAX` | |

- **`range`** is the declaration as the compiler locates it, from its first
  keyword to its end (a method's closing brace), without its doc comment.
  **`selectionRange`** is its name, as definition's ranges are: a field's
  includes its `@`. An `impl` block's is the type's name.
- **`detail`** is the declaration as the source writes it, up to its body,
  on one line: `fn pub static new(label: String, count: Int) -> Counter`
  for a signature written over several lines, `let pub MAX = 10` for a
  constant. Type arguments are as written, not filled in.
- **A trait's required and default methods.** A trait's method with a
  body, a default one, has ` { ... }` after its signature in `detail`
  (`fn pub double -> Float { ... }`); a required one, which every `impl`
  of the trait must write, has none (`fn pub area -> Float`). So a
  client can list what implementing the trait must write and what it may
  override, from the trait's file's symbols (definition at the `impl`'s
  trait name finds it). tests/corpus_symbols.rs checks both against the
  type database for every trait in std. `SymbolTag` has only
  `Deprecated`, so `detail` is the place the spec has.
- **Kinds every client has.** A client whose
  `documentSymbol.symbolKind.valueSet` doesn't have Object and EnumMember
  (past Array, 18, the last every client has) gets Class (5) for an `impl`
  block, and Constructor (9) for an enum's constructor.
- **A client without `hierarchicalDocumentSymbolSupport`** gets
  `SymbolInformation[]`: the same declarations, depth first, each with its
  `location` (its `range`) and, inside another, that one's name as
  `containerName`.
- Imports aren't symbols.
- **Unsaved text.** An open document's text is read, or else the file on
  disk.

**In code that doesn't parse**, the symbols are those of a copy of the text
with the lines in the way blank: the line of a bracket nothing closes, then
the line the parser stops at or the line before it (whichever lets it get
further), in turn, until the copy parses (at most 16 lines). Which `{` is
left open isn't always known: if a `}` that starts a line closes a `{` on
a line indented otherwise, that `{` is tried first too, and its copy is
the answer if it keeps every symbol the other does, and more. Blank lines
keep every other line where it is, so the ranges are the text's own, in
the negotiated encoding. A symbol is given only where the copy can't have
changed it:

- not a declaration whose signature has a blank line, nor what it contains
  (the copy's signature isn't the source's)
- not a top-level declaration, nor what it contains, that the blank lines
  could have moved in or out of another: one of its lines has another
  depth of braces in the copy than in the text, relative to its first
  line, or the text is deeper where it starts (a blank line opened a
  brace before it)
- not a top-level declaration after a blank line at the top of the copy
  that starts a type, trait or `impl` block in the text (`type pub Foo`
  without its `{` yet): what follows may be in it

**A brace opened in a method's body**, on a blank line, and not closed
yet, moves nothing: no body can hold a declaration, so it closes in that
method. Where every blank line that opens a brace is in a method's body
(and none closes one), every symbol the copy has is given, but the
methods with those lines, which may end at another `}` once it's closed.

So typing a statement in a method leaves every symbol there, and typing
`if x {` without its `}` leaves out only that method, until the brace is
closed: the type it's in, and everything after it, stay. The [status](#status) says `"repaired": true` for such an
answer. If no copy parses, the result is `null`, with reason `noHir`.

The tests check the names and ranges against the compiler's own
declarations in every module of std (tests/syntax.rs), and the edit-replay
corpus checks that each symbol given while code is being edited is one of
the final text's, at the same place (tests/replay.rs).

**Cost.** No check: answered at once, by the server itself, without waiting
for a running query. On an Apple M1 Max, under `cargo test`, about
30 ms for std's largest module (110 KB), and 80 ms when a line doesn't
parse; tests/latency.rs holds both to 150 ms.

## `textDocument/foldingRange`

Where the document can be folded, from its tokens alone: no check and no
parse, so it answers in code that doesn't type check or parse, and in
documents that aren't files.

```json
→ {"jsonrpc": "2.0", "id": 10, "method": "textDocument/foldingRange", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"}}}
← {"jsonrpc": "2.0", "id": 10, "result": [
    {"startLine": 0, "endLine": 1, "kind": "comment"},
    {"startLine": 2, "endLine": 5, "kind": "imports"},
    {"startLine": 9, "endLine": 19}
]}
```

| What | From | To | `kind` |
|---|---|---|---|
| a block: a type's, trait's, `impl` block's or method's body, a closure, `if`, `match`, a loop, a multi-line argument list, array or tuple | the line of its `{`, `(` or `[` | the line before the one that closes it, which stays in sight | none |
| comment lines one after another (a doc comment, or any other), each the first thing on its line | the first | the last | `comment` |
| the imports at the top level, one after another (comments between them included) | the first `import` | the end of the last | `imports` |

- **Whole lines only:** no `startCharacter` or `endCharacter`, which
  clients with `lineFoldingOnly` ignore, and others don't need.
- **Only what folds:** a range is at least two lines (a block's
  brackets three apart). Brackets in strings and comments don't count.
- **Kinds** are sent only if the client's
  `foldingRange.foldingRangeKind.valueSet` lists them; without, the range
  has no `kind`. `region` isn't used: Inko has no region markers.
- **`collapsedText`**, for a client with
  `foldingRange.foldingRange.collapsedText`, is what it shows for a
  collapsed range: a comment's first line (`# A counter.`), `import ...`
  for the imports, and a block's brackets around `...` (`{...}`, `(...)`
  or `[...]`).
- **`rangeLimit`**, if the client gives one, is the most ranges sent: the
  first, in order of where they start.
- **In code that doesn't parse**, brackets are matched as far as they
  match: a closing bracket closes the nearest open one of its kind (and
  any opened inside it), and one of its kind that nothing opened closes
  nothing. A bracket nothing closes folds nothing, so typing `if x {`
  without its `}` folds what's around it differently until it's closed.
- **Lines are LSP's:** a lone `\r` ends one, as everywhere.

The tests check that every declaration's body over three lines or more in
std folds from its `{` to the line before its `}`, and every doc comment
of two lines or more folds (tests/folding.rs).

## `textDocument/selectionRange`

For each position, the ranges around it, innermost first, each the
`parent` of the one before, from the document's tokens and declarations
alone: no check, so it answers in code that doesn't type check or parse,
and in documents that aren't files.

```json
→ {"jsonrpc": "2.0", "id": 11, "method": "textDocument/selectionRange", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "positions": [{"line": 15, "character": 19}]}}
← {"jsonrpc": "2.0", "id": 11, "result": [
    {"range": {"start": {"line": 15, "character": 19}, "end": {"line": 15, "character": 24}},
     "parent": {"range": {"start": {"line": 15, "character": 19}, "end": {"line": 15, "character": 27}},
      "parent": {"range": ..., "parent": ...}}}
]}
```

At `times` in `counter.add(1, times: 2)` (tests/selection.rs), the ranges are `times`,
`times: 2`, `1, times: 2`, `(1, times: 2)`, `counter.add(1, times: 2)`,
the method's statements, its body's braces, the method, and so on out to
the whole document. From the inside out:

| Range | What |
|---|---|
| the token | the name, literal, operator or comment at the position (the one starting there, or else the one ending there); none in whitespace |
| the chain up to it | names, `.`s and argument lists joined, up to the token and its own arguments: `a.b` at `b` in `a.b.c(x)` |
| the chain | all of it: `a.b.c(x)` |
| the element | in an argument list, array or tuple: between the commas around it |
| the statement | in a block, or at the top level: from a new line to the next, where a line doesn't end with something that goes on (an operator, `.`, `,`, `=`, `->`) and the next doesn't start with `.` |
| inside the bracket | from the first token after the `(`, `[`, `{` or quote to the last before the one that closes it |
| the bracket | with its pair; then the same again for the bracket around it |
| each declaration | around the position, as [document symbols](#textdocumentdocumentsymbol) have it (from a copy with lines blank where the text doesn't parse) |
| the declaration with its doc comment | from the doc comment's first `#` to the declaration's end, for one with a doc comment on lines of its own |
| the document | all of it |

- Each range contains the one before it and is larger: a range that
  would overlap without containing it is left out, and so is one equal
  to it. A declaration and its doc comment's range win: a statement that
  runs on past one's edge (a trait's required method has no body to end
  it) is left out.
- **In code that doesn't parse**, a bracket nothing closes is a token
  like any other, as for [folding](#textdocumentfoldingrange): typing
  `foo(` doesn't change what's around it elsewhere.
- **A position past the end** of the document: the result is `null`, with
  reason `invalidPosition`.

The tests check every declaration's name in std and the fixtures, and
every token in a sixteenth of the modules: each chain nested, and every
declaration around the position in it, and again with its doc comment
(tests/selection.rs).

**Cost.** No check: answered at once, by the server itself.

## `textDocument/formatting`

The document as `inko fmt` formats it, exactly: Cuttlebone runs the
upstream formatter itself (compiler/src/format.rs; patches/README.md),
on the document's text, as `inko fmt` parses it. Nothing is written: the
result is the edit, which the client applies.

```json
→ {"jsonrpc": "2.0", "id": 11, "method": "textDocument/formatting", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "options": {"tabSize": 2, "insertSpaces": true}}}
← {"jsonrpc": "2.0", "id": 11, "result": [
    {"range": {"start": {"line": 4, "character": 0}, "end": {"line": 6, "character": 2}},
     "newText": "  fn async main {\n    let s = 'héllo' + 'x'\n"}
]}
```

- **One edit**, of the part that differs (from the first character that
  changes to the last), in the negotiated encoding; `[]` if the text is
  formatted already.
- **`options` are ignored:** `inko fmt` has none (two spaces, its own
  line width).
- **Unsaved text:** an open document's text, or else the file on disk.
- **Text that doesn't parse** isn't formatted, as `inko fmt` refuses it:
  `null`, with reason `noHir`.

The tests compare it with the installed `inko fmt` (given the text on
stdin, so it writes nothing) for every module of std and the fixtures, as
they are and with their indentation taken away (tests/formatting.rs).

**Cost.** No check: answered at once, by the server itself.

## `textDocument/codeAction`

Quick fixes for the [diagnostics](#diagnostics) in the requested range,
chosen by each diagnostic's ID and what its message names. **Every fix is
checked before it's offered:** its edits are applied to a copy of the
documents, which is checked in full, as `inko check` checks (to MIR, as
diagnostics are), and the fix is given only if that reports fewer of the
diagnostic it fixes, and no error the documents didn't have: each error
after it, counted by file, ID and message (positions move with the
edits), is at most as many as before, less the one fixed. So a file with
other errors, as one being typed usually has, still gets the fixes that
remove theirs and break nothing else; a fix that would add an error, or
leave its diagnostic, isn't offered. Nothing is written: the client
applies the edit.

| ID | Message | Fix (`title`) |
|---|---|---|
| `invalid-symbol` | the symbol 'X' is undefined | `Import X from M`: for each module the file can import with a public X (type, trait, method or constant), the import added as completion adds it, as `inko fmt` orders imports |
| `unused-symbol` | the symbol 'X' is unused, in an import | `Remove X from the import` if its list has others, or else `Remove the unused import` (its line) |
| `unused-symbol` | the symbol 'x' is unused, a variable | `Rename x to _x`, which the compiler doesn't count as unused |
| `unused-result` | the result of this expression is unused | `Assign the result to _`: `let _ = ` before it |
| `invalid-assign` | the variable (or field) 'x' is immutable and can't be assigned a new value | `Make x mutable`: `mut` in the `let` that defines it, where the compiler looked the name up (the innermost variable of that name in scope, or the field of the method's type), in whichever file it is |
| `missing-field` | the field 'x' must be assigned a value | `Add x: v`: the field added to the call, with a value of its type as the type's declaration writes it (`0` for `Int`, `0.0`, `''`, `false`, `[]` and `Option.None` for `Float`, `String`, `Bool`, `Array[...]` and `Option[...]`), and `Add x: x` if the file has the name `x` elsewhere; each only if it checks |
| `invalid-implementation` | the method 'SIG' must be implemented for 'T' | `Implement m`, or `Implement the missing methods` if the `impl` block lacks several: each method it lacks, with the compiler's signature for it (a trait's type parameters replaced by the block's type arguments, from the trait's declaration in the file or the module it's imported from) and a body of `panic('not implemented')`, at the end of the block. One action, for all of the block's diagnostics |
| `missing-trait` | the trait 'R' isn't implemented for type 'T' | `Implement R for T`: `impl R for T` after the `impl` block that requires it, with the methods a check of the documents with it says it lacks, as above |
| `unreachable` | this code is unreachable | `Remove the unreachable code`: the statement, the rest of its block and the blank lines before it, or a `case` arm's lines |

```json
→ {"jsonrpc": "2.0", "id": 12, "method": "textDocument/codeAction", "params": {
    "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
    "range": {"start": {"line": 4, "character": 32}, "end": {"line": 4, "character": 38}},
    "context": {"diagnostics": [...], "only": ["quickfix"]}}}
← {"jsonrpc": "2.0", "id": 12, "result": [
    {"title": "Import helper from lib", "kind": "quickfix", "isPreferred": true,
     "diagnostics": [{"range": ..., "severity": 1, "code": "invalid-symbol", "source": "cuttlebone", "message": "the symbol 'helper' is undefined"}],
     "edit": {"changes": {"file:///home/me/hello/src/main.inko": [
       {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "import lib (helper)\n"}]}}}
]}
```

- **Which diagnostics:** those the check of the documents as they are
  reports in the document, overlapping the range, whatever the client's
  `context.diagnostics` says. The names to import (`invalid-symbol`),
  which the server's index of modules offers, are those the client's
  say are undefined and every name in the requested range (from the
  start of the word it starts in to the end of the one it ends in), so a
  client that shows diagnostics of its own, and sends none, gets them
  too. Each action's `diagnostics` is the one it fixes, as published.
- **Only for clients that take literals:** the server advertises
  `codeActionProvider` only to a client with `codeActionLiteralSupport`,
  since it has no commands, and answers others `[]`. A request whose
  `context.only` has no kind that includes `quickfix` gets `[]` at once.
- **`isPreferred`**, for a client with `isPreferredSupport`, marks the
  only fix for a diagnostic.
- **One action for several diagnostics:** a fix that's the same for
  several (the same title and edits, as stubbing an `impl` block's
  methods is for each method it lacks) is one action, whose
  `diagnostics` has them all, checked once.
- **What a check can't see:** the compiler stops after a pass that
  reported an error (`missing-trait` is reported where types are
  defined, before method bodies are checked), so a fix for such an error
  lets later passes report what they find, and counts as adding those
  errors if the documents have any: it isn't offered until they're
  fixed.
- **Edits** are `WorkspaceEdit.changes`, by URI, in the negotiated
  encoding.
- **Cost:** one full check of the documents, and one for each fix (at
  most 8 per request), each like a diagnostics check: about 100 to
  300 ms each on a small project. The check timeout applies to the whole
  request.

The tests apply every fix offered to a copy of the project and run the
installed `inko check` on it: fewer of the diagnostic, and no error that
`inko check` didn't report before (tests/actions.rs).

## `workspace/symbol`

The declarations, in every module the workspace's projects can import,
whose names match the query:

```json
→ {"jsonrpc": "2.0", "id": 10, "method": "workspace/symbol", "params": {"query": "stdou"}}
← {"jsonrpc": "2.0", "id": 10, "result": [
    {"name": "Stdout", "kind": 5, "containerName": "std.stdio",
     "location": {"uri": "file:///usr/local/lib/inko/std/std/stdio.inko",
                  "range": {"start": {"line": 118, "character": 15}, "end": {"line": 118, "character": 21}}}},
    ...
]}
```

- **Which modules:** the standard library's (in the directory `inko print
  std` printed), each workspace folder's own, in its `src/` and `test/`,
  and its dependencies', in its `dep/`, where its `inko.pkg` says the
  compiler finds them (`dep/<url>/v<major>/src`). The same declarations
  as [document symbols](#textdocumentdocumentsymbol) (all of them, private
  ones too), `impl` blocks among them: named by their header (`impl
  ToString for Counter`, which a query matches as any name), of the same
  kind as in document symbols (Object, or Class), located at the type's
  name, so a client that indexes finds every implementation.
- **From the syntax alone**, as document symbols are: the projects needn't
  type check, or parse. An open document's text counts instead of its
  file's, so a method typed and not saved is found.
- **Matching**, as clients filter: the query's characters appear in the
  name in the same order, in any case (`stou` finds `Stdout`, `cntr`
  `Counter`). An empty query matches everything.
- **Order:** the whole name first, then names that start with the query,
  then those that have it in a run, then the rest; the same case as the
  query first; then the workspace's own, then dependencies', then std's;
  then by name. At most 500 are given, the best, for a query that isn't
  empty.
- **Everything, for a client that indexes:** an empty query gets every
  declaration, with no limit. A client that sends a `partialResultToken`
  gets every match of any query as partial results: `$/progress`
  notifications with that token, 500 symbols each, in the same order,
  and then an empty response, as the spec says partial results end.
- **Whether a declaration is public** isn't in a workspace symbol: LSP
  has no way to say it there. `SymbolTag` has only `Deprecated`,
  `WorkspaceSymbol` has no `detail`, and `containerName` is the qualifier
  clients show (and Cuttlebone's is the path, `std.string.String`).
  Rather than an extension, the standard alternative is the module's
  [document symbols](#textdocumentdocumentsymbol), whose `detail` is the
  declaration as written, with its `pub` and its signature: a client
  that indexes can ask for those of the modules it needs.
- **`location`** is the name, as definition's ranges are, in the
  negotiated encoding. **`containerName`** is the module and what the
  declaration is in: `std.string.String` for `String.size`. A method of an
  `impl` block is in its type.
- **`kind`** is as for document symbols. A client whose
  `workspace.symbol.symbolKind.valueSet` doesn't have EnumMember gets
  Constructor (9) for an enum's constructor.
- Every result has a range, so `workspaceSymbol/resolve` isn't needed.

**Freshness.** The server keeps every module's declarations between
requests. Before each answer it looks at the files again (their size and
modification time, and the directories' contents) and reads only what
changed, was created or is open with other text; deleted files are gone.
Nothing is written anywhere.

**Cost.** On an Apple M1 Max, under `cargo test`, for a project that
imports all of std: about 220 ms for the first request, which reads and
parses every module, and 6 to 35 ms for each after it as a query is typed.
Every symbol (an empty query, over 5,000 of them) takes 180 to 290 ms in
one response, and 80 to 110 ms as partial results, measured at load
averages around 350. tests/latency.rs holds them to 500 ms, 50 ms and
400 ms.

## Diagnostics

Cuttlebone publishes the errors and warnings `inko check` reports, with
`textDocument/publishDiagnostics`, without being asked, or, for a client
that pulls them, answers `textDocument/diagnostic` and
`workspace/diagnostic` ([Pull diagnostics](#pull-diagnostics)):

```json
{"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {
  "uri": "file:///home/me/hello/src/main.inko",
  "diagnostics": [
    {"range": {"start": {"line": 10, "character": 16}, "end": {"line": 10, "character": 22}},
     "severity": 1, "code": "moved", "source": "cuttlebone",
     "message": "'values' can't be used as it has been moved"},
    {"range": {"start": {"line": 12, "character": 8}, "end": {"line": 12, "character": 14}},
     "severity": 2, "code": "unused-symbol", "source": "cuttlebone",
     "message": "the symbol 'unused' is unused", "tags": [1]}
  ]
}}
```

- **The same as `inko check`.** Each diagnostic is one that
  `inko check --format json` reports for the project, in the same order, with
  the same range, severity (`1` for an error, `2` for a warning), `code` (the
  compiler's ID, e.g. `invalid-symbol`) and `message`. tests/diagnostics.rs
  checks this against the installed `inko check`. That includes warnings,
  and the errors only lowering to MIR finds (moves, patterns that don't
  cover every case, and invalid constant expressions), so diagnostics come
  from a **full check**, not the queries' type check. The exceptions are
  under [Where it differs from `inko check`](#where-it-differs-from-inko-check).
- **`tags`:** `unused-symbol` and `unreachable` warnings are tagged
  `Unnecessary` (1), which clients usually show by fading the code, if the
  client's `publishDiagnostics.tagSupport.valueSet` has it.
- **Ranges** are in the negotiated encoding. The compiler counts columns in
  grapheme clusters (👩‍👩‍👦 is one column), and Cuttlebone converts them as
  it does for every other range.
- **Unsaved text.** The check uses the text of every open document, as
  queries do.
- **No side effects**, as for queries: no `dep/` sync, no network, no writes.

### What's checked, and where diagnostics are published

Diagnostics are checked per **target**: a project, or a file outside any
project's `src/` and `test/`.

- A file in a project's `src/` or `test/` (see [Project roots](#project-roots))
  makes the **whole project** a target, like `inko check` in the project's
  directory: every module in `src/` and `test/`, not only open ones.
- Any other file is a target of its own, like `inko check file.inko`.
- The standard library's files and dependencies' (in a `dep/` directory
  next to a `src/`) are never checked, even when open: they aren't the
  user's to fix.

A target is checked:

- when the server starts, for each workspace folder that has a `src/` or
  `test/` directory, so diagnostics appear before anything is opened
- the first time a file of it is opened
- `diagnosticsDebounceMs` after an edit to any of its open documents
  (`didChange`, a `didSave` with different text, or a `didClose` of a
  document with unsaved changes, which makes the file on disk count again)

After each check, Cuttlebone publishes the diagnostics of every file that
has any, open or not, and an empty list for each file it published in last
time that has none now. So a fixed error goes away. Diagnostics may be in
files outside the target, such as a module a file outside any project
imports.

### Scheduling

A full check costs more than a query's: it lowers to MIR, and checks the
whole project. On an Apple M1 Max, in a release build, about 0.2 s for a
project that imports all of std, and 0.42 s for std itself (`inko check`
takes about as long). So diagnostics never hold up queries:

- **A worker of their own.** One diagnostics check runs at a time, in a
  separate worker process from the queries', with the same `checkTimeoutMs`
  and the same handling of panics and crashes ([Robustness](#robustness)).
  A query never waits for a diagnostics check, and a diagnostics check
  never waits for a query once it's started.
- **Queries first.** A diagnostics check only *starts* when no query is
  queued or running (a cancelled one whose check runs on counts), so a
  burst of hovers doesn't compete with a check starting. A query that arrives while a diagnostics check runs starts at
  once, in parallel. A hover on a std-sized project takes about 104 ms
  during a check of the project, and 102 ms without one (tests/latency.rs
  holds every query to 500 ms during a check).
- **A longer debounce.** `diagnosticsDebounceMs` (300 by default) is longer
  than queries' `debounceMs` (50): diagnostics only need to be ready when
  typing pauses, and each check costs more.
- **Superseded checks are cancelled.** An edit to a target whose check is
  running kills that check's worker, and the target is checked again after
  the debounce. Several edits to a target waiting to be checked leave one
  check, after the last.
- **Several targets** wait in one queue, and are checked one at a time,
  the one waiting longest first.

A check that doesn't finish (it times out, crashes, the project requires a
newer Inko, or it's superseded) publishes nothing, so the last diagnostics
published stay. With `statusNotifications`, a [status](#status)
notification says why.

### Pull diagnostics

A client whose capabilities have `textDocument.diagnostic` is offered
pull diagnostics instead, and then gets **no** `publishDiagnostics`:

```json
"diagnosticProvider": {"identifier": "cuttlebone", "interFileDependencies": true, "workspaceDiagnostics": true}
```

- **`textDocument/diagnostic`**: the document's diagnostics, the same as
  would be published (from a full check of its target, with the
  documents as they are), as a `full` report with a `resultId`, or
  `unchanged` if the client's `previousResultId` is that id. A document
  that isn't a file has none; a file of std or a dependency has what a
  target's check found in it.
- **`workspace/diagnostic`**: every target's (each workspace folder that's
  a project, and each open document's), so the whole project's errors,
  open or not: a report for every file with diagnostics, and for every
  file the client has a `previousResultIds` entry for (empty, if it has
  none now), each `unchanged` if its id is the client's. `version` is
  `null`.
- **When:** a pull is answered from a check of its targets with the
  documents exactly as they are when it's answered, never an older one:
  if one is pending or running for the target, or the documents changed
  since the last, it waits for the next (and schedules one, at once, if
  none is coming), in the diagnostics' own queue and worker
  ([Scheduling](#scheduling)), so queries come first. Edits schedule
  checks after `diagnosticsDebounceMs`, as for publishing.
- **A long poll:** a workspace pull with `previousResultIds` whose every
  report would be `unchanged` is held until a check changes one, as the
  spec allows, so a client that pulls again at once doesn't check the
  project in a loop. `$/cancelRequest` ends it (`RequestCancelled`).
- **A check that doesn't finish** (it times out, crashes, or the compiler
  reports its own error): the pulls waiting for it get `RequestFailed`
  (-32803), with `data: {"retriggerRequest": false}`, the spec's
  `DiagnosticServerCancellationData`. One superseded by an edit waits for
  the next check. A compiler panic answers with what was found before it,
  as publishing does. At `shutdown`, waiting pulls get `ServerCancelled`
  (-32802).
- **`diagnostics: false`** doesn't turn pulls off: a client that pulls is
  answered when it asks.
- **Result ids** are a hash of the file's diagnostics, so the same
  diagnostics have the same id.

The same as `inko check`, as publishing is: tests/pull.rs holds a workspace
pull of every fixture project to the core's diagnostics, which
tests/diagnostics.rs holds to the installed `inko check`.

### Where it differs from `inko check`

- **A compiler panic.** Lowering some code to MIR panics (for example a
  `break` inside a closure inside a loop; compiler/src/mir/passes.rs:1836).
  `inko check` then crashes without printing anything. Cuttlebone publishes
  what the compiler reported before the panic (such as the type checker's
  warnings), and its status is `unavailable` with reason `compilerPanic`.
- **The standard library and dependencies** aren't checked as targets of
  their own (see above). A diagnostic `inko check` reports in one of their
  files while checking the user's project is still published.
- **A file in two targets.** If two targets report diagnostics in the same
  file (two lone files that import the same broken module), the file shows
  the most recent check's.

## Status

LSP results have nowhere to say *why* there's no answer, so Cuttlebone sends
a custom notification if the client opts in with
`"statusNotifications": true`:

```json
{"jsonrpc": "2.0", "method": "cuttlebone/status", "params": {
  "id": 7, "method": "textDocument/hover",
  "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
  "status": "unavailable",
  "reason": {"code": "declarationErrors", "message": "an error in a declaration ..."},
  "elapsedMs": 31.0
}}
```

- **When it's sent:** for every query (hover, completion, definition,
  declaration, type definition, implementation, references, the call and
  type hierarchies' requests, file renames, document highlight, signature help, inlay
  hints, document and workspace symbols, folding ranges, selection ranges,
  formatting, code actions) that gets a result, just before the
  response, with the request's `id`. Requests
  answered with an error (cancelled, invalid) get none. Document symbols,
  which need no check, are `fresh` (with `"repaired": true` if the text
  didn't parse), or `unavailable`: `notAFile` or `fileNotFound` if
  there's no text to read, `noHir` if no copy of it parses. Folding
  ranges, selection ranges and formatting need no check either: `fresh`,
  or `unavailable` for the same reasons (formatting is `noHir` if the
  text doesn't parse; selection ranges `invalidPosition` for a position
  past the end). Workspace
  symbols are always `fresh`, and their status has no `textDocument`.
- **What it's about:** every status has the request's `id` and
  `method`, and its document as `textDocument`, the one the request
  names (a hierarchy's item's, for the hierarchies' requests). A file
  rename names no document: its status has the request's own `files`
  instead, as it sent them (`[{"oldUri": ..., "newUri": ...}]`). One
  where no module's name changes, answered `null` without a check, is
  `fresh` too, so `null` with `fresh` means no edits, not no answer.
- **For diagnostics:** after each diagnostics check, with
  `"method": "textDocument/publishDiagnostics"`, no `id`, and the checked
  target's URI (a project directory, or a file) as `target` instead of
  `textDocument`. `fresh` means it published; `unavailable` means it
  didn't (`timeout`, `checkCrashed`, `compilerError`, or `superseded` when an
  edit cancelled it), except that `compilerPanic` publishes what was found
  before the panic.
- **Why opt-in:** clients that don't know the notification would otherwise
  receive one they don't handle. A client that doesn't opt in gets plain LSP
  results.

| `status` | Meaning |
|---|---|
| `fresh` | Computed from the documents as they are now. The response holds the answer, which may be `null` when there's nothing at the position. |
| `unavailable` | Cuttlebone has no answer, and the response is `null`. `reason` says why. A client with heuristics of its own can fall back to them. |
| `stale` | Computed from an earlier version of the documents: the last that type checked, because the code as it is now can't be (it doesn't parse), or a declaration's error may change the answer. The answer holds for the documents as they are: every position and range in it is in text that hasn't changed since, mapped to where that text is now, in code no change since could make mean something else. Only read-only requests give these ([Answers in code being edited](#answers-in-code-being-edited)). |

A `fresh` answer from a [repaired copy](#answers-in-code-being-edited)
has `"repaired": true` too. An answer from a check kept from an earlier
query about the same text (rather than one run for it) has
`"cached": true` ([Kept checks](#kept-checks)); its status is otherwise
the one a new check would give. So does an `unavailable` answer that took
no check: every check it took (of the documents as they are, and of a
repaired copy or the last good version tried after it) was kept.

A client that doesn't know `stale` must treat it, like any status other
than `fresh`, as having no usable answer. One that does may show a stale
answer as it shows a fresh one, or mark it. Clients must also ignore fields
and codes they don't know.

### What an answer covers

An inlay hint request asks about a range, and an answer from a repaired
copy or the last good check may hold for only part of it. Its status says
which parts, as `covered`, in the negotiated position encoding. For a
request from line 0 to line 99, with the method from line 29 to line 56
being edited:

```json
{"jsonrpc": "2.0", "method": "cuttlebone/status", "params": {
  "id": 9, "method": "textDocument/inlayHint",
  "textDocument": {"uri": "file:///home/me/hello/src/main.inko"},
  "status": "stale",
  "covered": [
    {"start": {"line": 0, "character": 0}, "end": {"line": 29, "character": 2}},
    {"start": {"line": 56, "character": 1}, "end": {"line": 99, "character": 0}}
  ],
  "elapsedMs": 0.4
}}
```

- **In a covered range**, the answer's hints are all there are: a position
  there with no hint has none. Each range includes its end, as the
  request's range does, and lies within the request's.
- **Outside them**, the answer has no hints, and says nothing about
  whether there are any. A client with guesses of its own can show those
  there.
- **Every inlay hint status has `covered`.** A `fresh` answer from the
  documents as they are covers the whole request: `[request's range]`. An
  answer with no result (`unavailable`) covers nothing: `[]`. A `repaired`
  or `stale` answer covers what holds: positions in text unchanged since
  the check, with the characters on either side, in a unit no change
  since made mean something else (the same rule that decides which hints
  are given).
- **A client that ignores `covered`** sees what it did before: the hints
  given, and nothing else.

Other requests' answers are whole or none: hover, definition, type
definition and signature help are about one position, and document
highlights and references are given only when complete, so they don't
have `covered`.

`reason` has `code`, a stable identifier, and `message`, a description for
people. `elapsedMs` is how long the answer took inside the worker: the
check, or the lookup in a kept one, or 0 if it never reached the worker.

| Code | When |
|---|---|
| `fileNotFound` | The document, or an open document's path, doesn't exist on disk. |
| `notAFile` | The document's URI isn't a `file:` URI. |
| `invalidPosition` | The position's line is past the end of the document. |
| `noHir` | The file, or a module it imports, couldn't be parsed or found (for example a syntax error, or an import of a module that doesn't exist), so nothing was type checked. |
| `declarationErrors` | An error in a declaration (for example an undefined type in a signature) may change what's at the position once it's fixed, or for references, could hide one ([Errors in declarations](#errors-in-declarations)). |
| `untyped` | The compiler couldn't type the expression at the position (or the receiver, for completion; or the expression, for type definition), usually because of an error in it. Other expressions in the file may still have types. |
| `compilerPanic` | The compiler panicked. `message` has the panic's message and location. |
| `compilerError` | The compiler reported an internal error, such as a project whose `inko.pkg` (or a dependency's) requires a newer Inko, or a manifest that doesn't parse. |
| `timeout` | The check took longer than `checkTimeoutMs`, and was killed. |
| `checkCrashed` | The worker process died without an answer, for example from an abort or a stack overflow. `message` has its exit status or signal. |
| `checkFailed` | The worker process couldn't be started. |
| `superseded` | Diagnostics only: an edit to the target cancelled its check, and a new one follows. |
| `shutdown` | The server got `shutdown` before the query was answered. |
| `internal` | A bug in Cuttlebone. |

## Errors in declarations

An error in a declaration, such as an undefined type in a signature or a
field, an import of something that doesn't exist, or an `impl` of an
undefined trait, stops `inko check` before it types any method body, in
any module. Cuttlebone's queries run every pass of the type checker
anyway, so the bodies get types (diagnostics are still `inko check`'s
own, from a check that stops: [Diagnostics](#diagnostics)). Some of those
types depend on the broken declaration, and will be different once it's
fixed. So while the documents have such an error, an answer from them is
given only where the error can't matter. The rules are those for
answers from a check of other text
([Answers in code being edited](#answers-in-code-being-edited)), with
each error as a change of the item it's in:

- **Not in the error's unit.** The method, or other item (a type's
  header and fields, a trait, an `impl` block, a constant, an import) the
  error is in gets no answers.
- **Not mentioning what it's about.** A unit that mentions (in its text,
  or in the types of its expressions) the name of the item with the
  error, or of the items it's in, gets no answers: for an undefined type
  in `fn add(amount: Strin)` in `type Counter`, every unit that mentions
  `add` or `Counter`. For an import, every name it binds counts: the last
  name of its path and those in its symbol list.
- **By the compiler's ids, where it has them.** A name the error is about
  stands for the symbols the check found at it in those items' headers
  (`add` for `Counter.add`, `Counter` for the type). A unit doesn't count
  as mentioning it where each time its text says the name, the check
  resolved it to another symbol: with an error in `A.size`'s signature, a
  method that only calls `B.size` (or `B.size` itself) is answered, and
  one that calls `a.size` isn't. A name the check didn't resolve there,
  and names in the types of a unit's expressions (which have no ids),
  count as before.
  Names changed outside method bodies since the file's last good check
  count too, since the error may come from them: a type renamed, so that
  its uses don't resolve.
- **No untyped expressions there.** A unit with an expression the check
  couldn't type gets no answers.
- **Complete lists.** Document highlights are given only if no unit that
  gets no answers mentions the name. References are given only if no unit
  of the project that gets no answers mentions it, in any module, and so
  are the other answers about the whole project, each for the names it's
  about: implementations of a trait (its name) or a trait's method (the
  method's), incoming calls (the method's), supertypes
  and subtypes (the type's or trait's), and file renames (the module's
  last name). Inlay
  hints cover the units that get answers, and the status's `covered` says
  which ([What an answer covers](#what-an-answer-covers)).

This holds for errors in the file asked about, in a module it imports, in
one that imports it and in one that's unrelated (which only the answers
about the whole project check). Errors in method bodies don't count: nothing outside a method
depends on its body. Answers are `fresh`: they come from the documents as
they are. Elsewhere the answer is `null`, with reason
`declarationErrors`, and Cuttlebone tries a repaired copy and the last
good check, as for code being edited (below).

A check with an error in a declaration isn't a good check for later
answers from older checks. In the rare case that a pass after an error
fails (one is known: renaming `type Option` in std.option itself, which
the compiler requires), Cuttlebone checks again, stopping at the error as
`inko check` does, and has no fresh answer.

**Cost.** None: the passes after the error run on a check that would have
stopped, and the rest is a lookup in each answer's unit (or, for
references, the units of every module of the project).

## Answers in code being edited

Code being typed usually doesn't parse. Then a check of the documents as
they are has nothing to answer from, in that file or any file that
imports it (reason `noHir`), and where a declaration has an error, it
answers only where the error can't matter (reason `declarationErrors`;
[Errors in declarations](#errors-in-declarations)). Cuttlebone then tries
two other checks, in this order:

1. **A repaired copy.** The lines in the way are made blank, in a copy of
   the documents, and the copy is checked. For a document that doesn't
   parse, the first of these that parses: the lines changed since the
   file's last good check (below), then also the line of a bracket
   nothing closes (the parser often reports that lines later), then also
   each line the parser stops at, in turn. For a declaration error, the
   methods or items that the changes since the last good check outside
   method bodies are in. For an import of a module that doesn't exist
   (`import app.nope`, which stops the compiler before any type checking,
   like a syntax error), that import, in whichever module it is, open or
   not. Blank lines keep every
   other line where it is. Answers are `fresh`, with `"repaired": true` in
   the [status](#status): they come from the documents as they are, except
   for the blank lines.
2. **The last good check** of the file: the server keeps the open
   documents as they were at each file's last check that typed method
   bodies with no error in a declaration (a query's, or a diagnostics
   check's), and every edit since. The
   position is mapped back through the edits, and every range in the
   answer forward. Answers are `stale`. A document closed since has its file's text
   from then on: the edits end at the text on disk when it was closed,
   and a change on disk after that (a `git checkout`, a refactoring with
   no editor open), or opening it again with other text, is one more
   change. Edits are never paired with a text they don't end at
   (tests/undo.rs).

| Request | Answers from |
|---|---|
| hover, definition, declaration, type definition, document highlight, signature help, inlay hints, preparing a call or type hierarchy | the documents as they are, or else a repaired copy, or else the last good check (`stale`) |
| outgoing calls | the documents as they are, or else the last good check (`stale`): every call in the method must be there |
| completion | the documents as they are, or else a repaired copy: its items are inserted into the code, so never stale. Names only from a copy whose blank lines are in method bodies ([Completion](#in-code-being-edited)) |
| document symbols | the text as it is, or else a copy with the lines in the way blank, from the syntax alone ([`textDocument/documentSymbol`](#textdocumentdocumentsymbol)) |
| references, implementations, incoming calls, supertypes and subtypes, file renames | the documents as they are only, past an error in a declaration only where it can't hide a part of the answer ([Errors in declarations](#errors-in-declarations)): a rename relies on them, and a blank line may hide a use or an `impl` |
| diagnostics | the documents as they are, always |

Either way, an answer is only given where it holds for the documents as
they are:

- **Unchanged text.** The position must be in text that's the same in the
  checked copy or version, and so must every range in the answer, with
  the characters on either side of it (an identifier typed onto is a
  different identifier). The edits are exact: they're the `didChange`
  ranges. Each range is mapped to where its text is now, in the negotiated
  encoding.
- **Unchanged meaning.** A file is split into units: each method, and each
  other item (a type apart from its methods, a trait, an `impl` block, a
  constant, an import). A unit's types depend on its own text and the
  declarations of what it uses, never on another method's body. So the
  answer is given only if its unit (and, for highlights and inlay hints,
  the unit of each item in it) wasn't changed itself, and doesn't mention
  a name that a change outside a method body is about: the method, field,
  type or import it's in, and the names it removed or added (an added name
  may be unfinished, so any name it starts counts). A whole import, added
  or removed, is about the names it binds: the last name of its path
  (`util` in `import app.util`) and those in its symbol list, not the
  path's leading names (`app`). A change inside a method's body only
  affects that method.
- **Before the edit, in a repaired copy.** In a method whose body has
  blank lines, what ends before the first one still means the same once
  the blank text is back, if all of it has a complete type: text added
  later can only fill in a type the checker couldn't infer yet (`?`), or
  fail to type. So there, answers are given before the first blank line.
  Not from the last good check, where a complete type may have come from
  text that changed since.
- **No errors there then.** A unit that had an expression the check
  couldn't type gets no answers: it may name something that didn't exist
  then, and is being typed now. Completion's own placeholder doesn't count.
- **Complete lists.** Document highlights are given only if the name isn't
  in changed text, or in a unit (or part of one) with no answers, where
  uses could be missing. Inlay hints are those in units that hold, and the
  request is unavailable if none of its range is in one. The status says
  which parts of the range the hints cover
  ([What an answer covers](#what-an-answer-covers)).

Everywhere else the answer is `null`, with the fresh check's reason.

**Cost.** A repaired copy or an older version is checked like the
documents as they are, and kept like them ([Kept checks](#kept-checks)),
and so is a check of the documents that doesn't parse. So an answer in
broken code can take up to three checks the first time, and none for the
next questions about the same text, whether they have an answer or not: on an Apple M1 Max, under `cargo test` (dependencies
optimized), a hover in a std-sized project takes about 135 ms from a
repaired copy, and 240 ms when neither may answer (tests/latency.rs holds both to 500 ms). A document
that doesn't parse is found in about a millisecond, before any type
checking.

**Memory.** The server keeps at most 4 versions of the open documents
(each shared by every file whose last good check used it; a file whose
version is dropped has no stale answers until its next good check), plus
the documents sent with up to 4 running checks. Each changed file's edits
since a version take at most 1024 runs of unchanged text, 24 bytes each;
past that, the shortest runs count as changed. Nothing is written to
disk.

## Debouncing and cancellation

Requests are queued and answered one check at a time, in order.
Diagnostics have their own queue and worker ([Scheduling](#scheduling)).

- **Debouncing.** A query on a document edited less than `debounceMs` ago
  waits until that much time has passed since the edit, so checks don't
  start in the middle of typing.
- **Newer edits.** A request is answered for the documents as they were
  when it arrived, even if an edit (or opening, saving or closing a
  document) comes before its answer. LSP says a server shouldn't fail a
  request because of a change it sees after it: the result may still be
  of use to the client, which cancels what it doesn't need (the spec's
  "Implementation Considerations", and `ContentModified`). So a running
  check carries on, and a queued query keeps the documents it was asked
  about, shared by every query queued at the same edit. Its answer's
  positions and ranges are in that text. No query gets `ContentModified`.
  Such a check is never the [last good check](#answers-in-code-being-edited)
  for later answers.
- **`$/cancelRequest`** answers the request with `RequestCancelled`
  (-32800) at once, whether it's queued or running. A queued request,
  such as one still waiting out the debounce, never reaches the worker.
  A running check carries on in its worker, which keeps it and everything
  it kept before, as if it had been answered: the answer goes nowhere.
  The next query waits for it, for at most 250 ms after it's ready to
  start. Then, or at `checkTimeoutMs` from the check's start if nothing
  waits, the worker is killed, as a running check's is at the timeout. So
  a hung check is still killed, and a cancel never costs the checks the
  worker kept. Clients that cancel requests they no longer need (VS
  Code's does, as its `staleRequestSupport` says) save the checks that
  haven't started.
- **`shutdown`** stops everything queued or running. Those queries get
  `null`, with status `unavailable` and reason `shutdown`: the client
  didn't cancel them, so `RequestCancelled` would be wrong.

Other errors are `MethodNotFound` (-32601) for anything not listed here,
`InvalidParams` (-32602), and `InvalidRequest` (-32600) for a request after
`shutdown` or a second `initialize`. Unknown notifications are ignored.

## Robustness

Queries run in a worker process (`cuttlebone --worker`), one at a time.
The worker stays between queries, keeping the checks it did
([Kept checks](#kept-checks)):

- The server kills the worker when a query times out (`checkTimeoutMs`),
  or the server shuts down, and starts a new one for the next query.
  The checks it kept go with it. A cancelled query's check runs on
  instead, unless the next query has waited 250 ms for it, or it reaches
  the timeout ([Debouncing and cancellation](#debouncing-and-cancellation)).
- If the worker panics, aborts, or overflows its stack, the server reports
  it (`compilerPanic` or `checkCrashed`) and goes on. A check the
  compiler panicked in isn't kept. A panic while answering from a kept
  check, which is only ever read, leaves it as it was: the same question
  is `compilerPanic` again, as it would be from a new check.
- Every check builds its own compiler state: a kept check is only ever
  read, never checked again.
- A worker exits by itself (with status 124) once a query has run for
  `checkTimeoutMs` plus 1 s, and when its stdin closes. If the server is
  killed outright, its workers don't outlive it for longer than that.
- Diagnostics checks each run in a worker of their own, which exits after
  the check.

On an Apple M1 Max, a fresh worker added 13 to 20 ms to a 100 ms check of
a file that imports all of std, in a release build
([history/MILESTONE-2.md](history/MILESTONE-2.md) has the numbers); a
worker kept between queries saves the 5.5 ms of starting one.

The server's own subprocesses (`inko --version`, `inko print std`) have a
10 s timeout.

## Freshness, cost and side effects

- **Freshness.** Every query is answered from a type check of the
  document and every module it imports (including the standard library's
  prelude), with the open documents' text exactly as they are, and the
  files on disk as they are, so a `fresh` answer is never older than the
  request. The check is a new one, or one kept from an earlier query about
  the same text ([Kept checks](#kept-checks)). Only when that check can't
  answer is a repaired copy or an earlier version of the documents
  checked instead, for a `repaired` or `stale` answer
  ([Answers in code being edited](#answers-in-code-being-edited)). Queries
  don't lower to MIR, which types never depend on.
- **Cost.** A query on a file that imports all of the standard library (116
  modules) takes about 100 to 120 ms on an Apple M1 Max. One on a typical std
  module takes 30 to 100 ms. References check the whole project instead of
  one file and its imports: about 270 ms for all of std's own `src/` and
  `test/` as the project.
- **No side effects.** Unlike `inko check`, a query (or a diagnostics check) never syncs `dep/`, never
  touches the network, and never creates or locks anything in Inko's data
  directory. It writes nothing anywhere. It does enforce the Inko version
  that the project's and its dependencies' `inko.pkg` require. Dependencies
  are whatever `inko pkg sync` last put in `dep/`.

### Kept checks

A check answers every question about the text it checked. The worker
keeps the last 4 checks it did, and answers a query from one of them
without checking again when everything the query would check is the same:

- **The same target**: the same file and project (references check the
  whole project, so they have a check of their own), with the same standard
  library.
- **The same text, exactly**, of every open document. Any edit, opening or
  closing a document makes the text, and so the check, a different one.
- **The same files on disk.** A check records what it read: each module's
  file, the manifests (`inko.pkg`), and every directory under std, `src/`,
  `test/` and `dep/`, and the project's directory (a file created or
  deleted there changes its directory). Before each answer, the worker
  looks at all of them again (their size, modification and change times,
  and inode), and checks again if any differs. So a file changed outside
  the editor (a `git checkout`, a formatter) is never answered from an
  older check, without `workspace/didChangeWatchedFiles`. A check that
  read a file changed while it ran, or so shortly before it that the file
  system's clock can't tell (2 s, on one that records whole seconds; none
  on APFS, ext4 and others that record nanoseconds), isn't kept.
- **Saving doesn't count.** An open document's file isn't what a check
  read: it read the document's text. So a save, which writes that text
  to the file, keeps every check of it, whether the editor writes the
  file in place or writes another and renames it over the first (which
  changes the directory's times, but not the names in it, which are what
  module resolution reads). What a save changes is the text, if it sends
  other text in `didSave`, and that's a different check anyway.

**An undo** returns to text that was checked before, so a kept check of
it answers again, if nothing it read on disk changed since; a module
changed on disk and put back has new times, so it's checked again. Answers
after an undo are a new server's for the same text
(tests/undo.rs).

This covers all three kinds of answer: the documents as they are, a
repaired copy, and the last good check's version, each of which is a
check of its own text. It covers checks with nothing to answer from too:
code that doesn't parse (`noHir`), an error in a declaration that stopped
the type checker (`declarationErrors`), or the compiler's own error
(`compilerError`). The same question then gets the same `unavailable`
answer without checking again. Since a check that doesn't parse doesn't
say which modules it got to, what it read counts as every source file
under std, `src/`, `test/` and `dep/` as well as the directories. These
hold no compiler state, so up to 8 are kept besides the 4 checks, and
never push one out. A check the compiler panicked in isn't kept. Typing in broken code checks a new repaired copy
on each edit, but reuses the older version's check. Completion's copy
(with its placeholder) is used once, and not kept.

A kept answer is exactly the answer a new check would give: it comes from
the same check of the same text, with the same rules for which request may
answer from which kind of check. Its [status](#status) is the same, with
`"cached": true` added.

**Cost.** A burst of queries about unchanged text costs one check and a
lookup each: on an Apple M1 Max, in a release build, 1,000 definition
requests sent at once on a std-sized project are answered in 0.7 s
(tests/latency.rs holds them to 3 s under `cargo test`). Looking again at
what a check read costs about 2 µs a path (150 paths for a file that
imports all of std). In code that doesn't parse, a second question with
no answer on that project takes about 10 ms in the worker instead of
30 ms: the lookups, and trying the repaired copies' blank lines again (a
parse each, about 2 ms in all).

**Memory.** Each kept check holds its compiler's state: about 33 MB for a
file that imports all of std. On an Apple M1 Max the worker holds 48 MB
with one such check, 146 MB with 4, and 182 MB at most, while a fifth is
checked before the oldest goes. A check of a whole project is bigger. The
worker frees a check when it keeps a fifth, and all of them when it's
killed or exits. Diagnostics checks are never kept: they lower to
MIR, which consumes what the queries would need.

## `shutdown` and `exit`

- `shutdown` answers `null`, after answering the queries still queued or
  running with `null` (reason `shutdown`).
- `exit` ends the process with status 0 after `shutdown`, and 1 without it.
- If stdin closes without `exit`, the process ends with status 1.
- Malformed messages (bad framing or JSON) end the session, as lsp-server
  handles them; clients don't send those.

## Extensions

LSP 3.18 has a standard way to do most of what Cuttlebone does, and
Cuttlebone uses it. These are the exceptions, and why the spec's own
channels don't solve the problem:

| Extension | What it's for | Why not the spec |
|---|---|---|
| [`cuttlebone/status`](#status), opted into with `statusNotifications` | Why a request has no answer, whether an answer is `stale` or `repaired`, `cached`, how long it took, [what it covers](#what-an-answer-covers); why a diagnostics check didn't publish | **Reasons:** the spec's channel is an error response, `RequestFailed` (-32803) with `data`, which would tie the reason to its request. But clients show those errors to people, and "no answer here" is routine: the code doesn't parse while it's typed. VS Code's client (vscode-languageclient 11.0.0, `handleFailedRequest` in lib/common/client.js) treats every error other than `RequestCancelled`, `ServerCancelled` and `ContentModified` as a failure, and shows it in an error notification ("Request textDocument/hover failed.") with its default `revealOutputChannelOn` (`Error`). Neovim 0.12.6 shows an `ERROR` message for an error to `documentHighlight` (its default handler, runtime/lua/vim/lsp/handlers.lua) and `signatureHelp` (buf.lua), and a `WARN` for completion; only hover, inlay hints and definitions are logged quietly. A reason as an error would mean popups while typing, so the result is `null` and the reason goes here. **Staleness, cost:** results have no field for where an answer came from. **Coverage:** an `InlayHint[]` result has nowhere to say which parts of the range it covers. The spec's nearest mechanisms don't say it either: partial results (`partialResultToken` and `$/progress`) stream more of a result, which must still end up whole; `workspace/inlayHint/refresh` asks the client to request every hint again, without saying where an answer was missing; an error, or a per-hint `data`, can't mark a range with no hints in it. **Diagnostics:** `publishDiagnostics` has no way to say a check didn't finish. |
| `experimental.cuttlebone` in the server's capabilities | Which Inko and standard library the session checks with, and whether status notifications and diagnostics are on | `experimental` is the spec's place for what isn't in it. `serverInfo` has only the server's name and version, and no capability says what a server checks against. |
| `initializationOptions` (`inko`, `checkTimeoutMs`, `debounceMs`, `statusNotifications`, `diagnostics`, `diagnosticsDebounceMs`) | Settings | `initializationOptions` is the spec's field for them, and their names are the server's to define. `workspace/configuration` would only arrive after `initialize`, which needs `inko` to accept or refuse the session. |
| `data` in `initialize`'s refusal: `code`, `cuttlebone`, `supported`, `installed`, beside `retry` | A stable code for why the session was refused, and the versions involved | The spec's `InitializeError` has only `retry`; `data` is where a response error carries more. `RequestFailed` is the spec's code for a request that failed, since `InitializeErrorCodes` has only the deprecated `unknownProtocolVersion`. |

## Testing faults

For tests only: with `CUTTLEBONE_FAULT=hang:<suffix>` or
`CUTTLEBONE_FAULT=abort:<suffix>` in the server's environment, a worker
checking a file whose path ends with `<suffix>` hangs or aborts instead of
checking. No real input is known to do either, and the server must
survive both. With `CUTTLEBONE_FAULT=norepair:<suffix>`, it checks no
[repaired copies](#answers-in-code-being-edited), so tests can reach
answers from the last good check. With `CUTTLEBONE_FAULT=stop:<suffix>`,
checks of that file (and of whole projects) stop at the first pass with
an error, as `inko check`'s do, and as a check does after a pass after an
error panics ([Errors in declarations](#errors-in-declarations)), so tests
can reach the answers that then come from repaired copies and the last
good check. With `CUTTLEBONE_FAULT=slow:<suffix>`, each check of such a
file takes 300 ms longer, a deliberately slower server for the latency
limits to fail (.github/workflows/latency.yml, by hand). Several faults
can be given, separated by commas. Only builds
with the `fault-injection` cargo feature have this, and
only `cargo test` enables it: release builds ignore the variable.

## Example

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":"file:///home/me/hello","capabilities":{},"initializationOptions":{"inko":"/usr/local/bin/inko"}}}
← {"jsonrpc":"2.0","id":1,"result":{"capabilities":{...},"serverInfo":{"name":"cuttlebone","version":"0.16.0"}}}
→ {"jsonrpc":"2.0","method":"initialized","params":{}}
→ {"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///home/me/hello/src/main.inko","languageId":"inko","version":1,"text":"..."}}}
→ {"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///home/me/hello/src/main.inko"},"position":{"line":5,"character":14}}}
← {"jsonrpc":"2.0","id":2,"result":{"contents":{"kind":"markdown","value":"```inko\nInt\n```"},"range":{"start":{"line":5,"character":14},"end":{"line":5,"character":16}}}}
→ {"jsonrpc":"2.0","id":3,"method":"shutdown"}
← {"jsonrpc":"2.0","id":3,"result":null}
→ {"jsonrpc":"2.0","method":"exit"}
```
