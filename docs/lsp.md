# Cuttlebone's language server

`cuttlebone` is a language server. It speaks standard
[LSP 3.18](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.18/specification/)
over stdin and stdout, for any editor with an LSP client. It assumes nothing
about the editor. This document covers what it supports, the few things
that LSP leaves to the server, and the few [extensions](#extensions) where
LSP has no way to do something.

It sends only what the client's capabilities say the client supports
(Markdown hovers, completion's label details and kinds, signature help's
parameter offsets and active parameters, diagnostic tags), and every
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
- `cuttlebone --version` prints `cuttlebone 0.9.0 (Inko 0.21.1)`.

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
  "message": "Cuttlebone 0.9.0 works with Inko 0.21.1 only, but /usr/local/bin/inko is Inko 0.20.0. Install Inko 0.21.1, or use a build of Cuttlebone for Inko 0.20.0.",
  "data": {"retry": false, "code": "inkoVersionMismatch", "cuttlebone": "0.9.0", "supported": "0.21.1", "installed": "0.20.0"}
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

Only what works:

```json
{
  "capabilities": {
    "positionEncoding": "utf-16",
    "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": true}},
    "hoverProvider": true,
    "completionProvider": {"triggerCharacters": ["."], "resolveProvider": false},
    "definitionProvider": true,
    "typeDefinitionProvider": true,
    "referencesProvider": true,
    "documentHighlightProvider": true,
    "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
    "inlayHintProvider": true,
    "experimental": {"cuttlebone": {"inko": "0.21.1", "std": "/path/to/std", "statusNotifications": false, "diagnostics": true}}
  },
  "serverInfo": {"name": "cuttlebone", "version": "0.9.0"}
}
```

`experimental.cuttlebone` isn't a capability. It tells a client which Inko
and standard library the session checks with, and whether status
notifications and [diagnostics](#diagnostics) are on. Diagnostics need no
capability: the server publishes them unless `diagnostics` is `false`.

### Clients that turn on every capability

Some clients (JetBrains' LSP support, for one) use every capability a
server advertises, with no setting per feature, and diagnostics need no
capability at all. If the client, or a plugin for Inko in it, already
provides a feature itself, the user sees it twice: two hovers, two sets of
inlay hints, every error reported twice.

Cuttlebone can't know what else is running, so turning a feature off is the
client's job, on its side:

- **Requests** (hover, completion, definition, type definition,
  references, document highlight, signature help, inlay hints): don't send
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

The type of the expression at the position, as a Markdown code block:

```json
{"contents": {"kind": "markdown", "value": "```inko\nArray[Int]\n```"},
 "range": {"start": {"line": 4, "character": 18}, "end": {"line": 4, "character": 26}}}
```

- The format is the first of `markdown` and `plaintext` in the client's
  `textDocument.hover.contentFormat`, its order of preference. A client
  that lists neither, or sends no `contentFormat`, gets
  `{"kind": "plaintext", "value": "Array[Int]"}`.
- `range` is the expression's range.
- The type is formatted the way the compiler formats types in its
  diagnostics, e.g. `ref Array[Int]`, `Result[Int, String]`,
  `uni Future[Int]`. A type the compiler couldn't infer fully may contain
  `?`.

The result is `null` when nothing is there. For example, in a comment,
between declarations, or on a method's name in its definition. It's also
`null` when there's no answer, and [status](#status) says why.

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

The members available after a `.`. `.` is the trigger character, and
completion invoked by hand works too, after a `.` and any typed prefix.

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

The result is `null` when the cursor isn't after a `.`, or when there's no
answer ([status](#status) says why). It's never a guess.

## `textDocument/definition`

Where the symbol at the position is declared, as one `Location`:

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
the call doesn't resolve to a method (an undefined method, a closure call,
or a type's constructor such as `Point(x: 1)`), and `null` with a
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

## Diagnostics

Cuttlebone publishes the errors and warnings `inko check` reports, with
`textDocument/publishDiagnostics`, without being asked:

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
  queued or running, so a burst of hovers doesn't compete with a check
  starting. A query that arrives while a diagnostics check runs starts at
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

- **When it's sent:** for every query (hover, completion, definition, type
  definition, references, document highlight, signature help and inlay
  hints) that gets a result, just before the
  response, with the request's `id`. Requests
  answered with an error (cancelled, invalid) get none.
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
the one a new check would give.

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
  `add` or `Counter`. For an import, every name in the import counts.
  Names changed outside method bodies since the file's last good check
  count too, since the error may come from them: a type renamed, so that
  its uses don't resolve.
- **No untyped expressions there.** A unit with an expression the check
  couldn't type gets no answers.
- **Complete lists.** Document highlights are given only if no unit that
  gets no answers mentions the name. References are given only if no unit
  of the project that gets no answers mentions it, in any module. Inlay
  hints cover the units that get answers, and the status's `covered` says
  which ([What an answer covers](#what-an-answer-covers)).

This holds for errors in the file asked about, in a module it imports, in
one that imports it and in one that's unrelated (which only references
check). Errors in method bodies don't count: nothing outside a method
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
   method bodies are in. Blank lines keep every
   other line where it is. Answers are `fresh`, with `"repaired": true` in
   the [status](#status): they come from the documents as they are, except
   for the blank lines.
2. **The last good check** of the file: the server keeps the open
   documents as they were at each file's last check that typed method
   bodies with no error in a declaration (a query's, or a diagnostics
   check's), and every edit since. The
   position is mapped back through the edits, and every range in the
   answer forward. Answers are `stale`.

| Request | Answers from |
|---|---|
| hover, definition, type definition, document highlight, signature help, inlay hints | the documents as they are, or else a repaired copy, or else the last good check (`stale`) |
| completion | the documents as they are, or else a repaired copy: its items are inserted into the code, so never stale |
| references | the documents as they are only: a rename relies on them, and a blank line may have a use |
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
  may be unfinished, so any name it starts counts). A change inside a
  method's body only affects that method.
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
documents as they are, and kept like them ([Kept checks](#kept-checks)), so
an answer in broken code can take up to three checks the first time, and
none for the next questions about the same text: on an Apple M1 Max, under `cargo test` (dependencies
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
  (-32800), whether it's queued or running. A running check's worker is
  killed. Clients that cancel requests they no longer need (VS Code's
  does, as its `staleRequestSupport` says) save the checks.
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
  is cancelled, or the server shuts down, and starts a new one for the next
  query.
  The checks it kept go with it.
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
  the editor (a `git checkout`, a formatter, a save) is never answered
  from an older check, without `workspace/didChangeWatchedFiles`. A check
  that read a file changed while it ran, or so shortly before it that the
  file system's clock can't tell (2 s, on one that records whole seconds),
  isn't kept.

This covers all three kinds of answer: the documents as they are, a
repaired copy, and the last good check's version, each of which is a
check of its own text. Typing in broken code checks a new repaired copy
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
imports all of std).

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
good check. Several faults can be given, separated by commas. Only builds
with the `fault-injection` cargo feature have this, and
only `cargo test` enables it: release builds ignore the variable.

## Example

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":"file:///home/me/hello","capabilities":{},"initializationOptions":{"inko":"/usr/local/bin/inko"}}}
← {"jsonrpc":"2.0","id":1,"result":{"capabilities":{...},"serverInfo":{"name":"cuttlebone","version":"0.9.0"}}}
→ {"jsonrpc":"2.0","method":"initialized","params":{}}
→ {"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///home/me/hello/src/main.inko","languageId":"inko","version":1,"text":"..."}}}
→ {"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///home/me/hello/src/main.inko"},"position":{"line":5,"character":14}}}
← {"jsonrpc":"2.0","id":2,"result":{"contents":{"kind":"markdown","value":"```inko\nInt\n```"},"range":{"start":{"line":5,"character":14},"end":{"line":5,"character":16}}}}
→ {"jsonrpc":"2.0","id":3,"method":"shutdown"}
← {"jsonrpc":"2.0","id":3,"result":null}
→ {"jsonrpc":"2.0","method":"exit"}
```
