# Cuttlebone's language server

`cuttlebone` is a language server. It speaks standard
[LSP 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
over stdin and stdout, for any editor with an LSP client. It assumes nothing
about the editor. This document covers what it supports, and the few things
that LSP leaves to the server.

## Starting it

```
cuttlebone            # or: cuttlebone --stdio
```

- stdio is the only transport. `--stdio` is accepted because some clients
  (such as VS Code's) pass it.
- stderr carries free-form logs: compiler panics, killed checks, worker
  output. Clients may show it or discard it, but shouldn't parse it.
- `cuttlebone --version` prints `cuttlebone 0.1.0 (Inko 0.21.1)`.

One process serves one editor session, with any number of workspace folders.

## `initialize`

### Initialization options

All of these are optional.

| Option | Type | Default | |
|---|---|---|---|
| `inko` | string | `inko` on `PATH` | The `inko` executable to check the version of and take the standard library from. |
| `checkTimeoutMs` | number | 5000 | How long one check may run before it's killed (see [Robustness](#robustness)). |
| `debounceMs` | number | 50 | How long a document must go without edits before a query on it starts (see [Debouncing and cancellation](#debouncing-and-cancellation)). |
| `statusNotifications` | boolean | false | Send a [`cuttlebone/status`](#status) notification before each hover and completion response. |

### The Inko version

Cuttlebone links one Inko release's compiler (0.21.1 for this build). During
`initialize` it runs `inko --version` (with a 10 s timeout). If the version
is a different one, it refuses the session. `initialize` then gets an error
response instead of a result:

```json
{"jsonrpc": "2.0", "id": 1, "error": {
  "code": -32803,
  "message": "Cuttlebone 0.1.0 works with Inko 0.21.1 only, but /usr/local/bin/inko is Inko 0.20.0. Install Inko 0.21.1, or use a build of Cuttlebone for Inko 0.20.0.",
  "data": {"retry": false, "code": "inkoVersionMismatch", "cuttlebone": "0.1.0", "supported": "0.21.1", "installed": "0.20.0"}
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
    "experimental": {"cuttlebone": {"inko": "0.21.1", "std": "/path/to/std", "statusNotifications": false}}
  },
  "serverInfo": {"name": "cuttlebone", "version": "0.1.0"}
}
```

`experimental.cuttlebone` isn't a capability. It tells a client which Inko
and standard library the session checks with, and whether status
notifications are on.

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
- **Lines end at `\n` or `\r\n`.** A lone `\r` isn't a line break for
  Cuttlebone or the compiler, though LSP counts it as one. Positions after a
  lone `\r` are off, until a later version handles it.

## `textDocument/hover`

The type of the expression at the position, as a Markdown code block:

```json
{"contents": {"kind": "markdown", "value": "```inko\nArray[Int]\n```"},
 "range": {"start": {"line": 4, "character": 18}, "end": {"line": 4, "character": 26}}}
```

- A client whose `textDocument.hover.contentFormat` lacks `markdown` gets
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
| `kind` | Method (2), Field (5), Constant (21) or Class (7, for a type in a module). |
| `detail` | The declaration, with the receiver's type arguments filled in, including in default methods from generic traits (e.g. `fn pub inline get(index: Int) -> Result[ref Int, OutOfBounds]` on an `Array[Int]`). |
| `labelDetails.description` | The module that defines it. |

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

- **When it's sent:** for every hover and completion request that gets a
  result, just before the response, with the request's `id`. Requests
  answered with an error (cancelled, superseded, invalid) get none.
- **Why opt-in:** clients that don't know the notification would otherwise
  receive one they don't handle. A client that doesn't opt in gets plain LSP
  results.

| `status` | Meaning |
|---|---|
| `fresh` | Computed from the documents as they are now. The response holds the answer, which may be `null` when there's nothing at the position. |
| `unavailable` | Cuttlebone has no answer, and the response is `null`. `reason` says why. A client with heuristics of its own can fall back to them. |
| `stale` | Reserved. A later version may answer from a stored earlier result when the current code can't be typed, and will mark such answers `stale`. This version never sends it. |

Clients must treat any status other than `fresh` as having no usable answer.
They must also ignore fields and codes they don't know.

`reason` has `code`, a stable identifier, and `message`, a description for
people. `elapsedMs` is how long the check took inside the worker, or 0 if
there was no check.

| Code | When |
|---|---|
| `fileNotFound` | The document, or an open document's path, doesn't exist on disk. |
| `notAFile` | The document's URI isn't a `file:` URI. |
| `invalidPosition` | The position's line is past the end of the document. |
| `noHir` | The file, or a module it imports, couldn't be parsed or found (for example a syntax error, or an import of a module that doesn't exist), so nothing was type checked. |
| `declarationErrors` | An error in a declaration (for example an undefined type in a signature) stopped the type checker before it typed any method body. |
| `untyped` | The compiler couldn't type the expression at the position (or the receiver, for completion), usually because of an error in it. Other expressions in the file may still have types. |
| `compilerPanic` | The compiler panicked. `message` has the panic's message and location. |
| `compilerError` | The compiler reported an internal error, such as a project whose `inko.pkg` (or a dependency's) requires a newer Inko, or a manifest that doesn't parse. |
| `timeout` | The check took longer than `checkTimeoutMs`, and was killed. |
| `checkCrashed` | The worker process died without an answer, for example from an abort or a stack overflow. `message` has its exit status or signal. |
| `checkFailed` | The worker process couldn't be started. |
| `internal` | A bug in Cuttlebone. |

## Debouncing and cancellation

Requests are queued and answered one check at a time, in order.

- **Debouncing.** A query on a document edited less than `debounceMs` ago
  waits until that much time has passed since the edit, so checks don't
  start in the middle of typing.
- **Newer edits.** A `didChange` cancels the queued and running queries on
  that document: their positions refer to text that's gone. Each gets a
  `ContentModified` (-32801) error, and a running check's worker is killed.
  Queries on other documents carry on.
- **`$/cancelRequest`** answers the request with `RequestCancelled`
  (-32800), whether it's queued or running. A running check's worker is
  killed.
- **`shutdown`** cancels everything queued or running in the same way.

Other errors are `MethodNotFound` (-32601) for anything not listed here,
`InvalidParams` (-32602), and `InvalidRequest` (-32600) for a request after
`shutdown` or a second `initialize`. Unknown notifications are ignored.

## Robustness

Each query runs in a fresh worker process (`cuttlebone --worker`):

- The server kills the worker when the query times out (`checkTimeoutMs`),
  is cancelled, or is superseded.
- If the worker panics, aborts, or overflows its stack, the server reports
  it (`compilerPanic` or `checkCrashed`) and goes on.
- Every check builds its own compiler state, so nothing carries over from
  one to the next.

On an Apple M1 Max, a worker adds 1 to 10 ms to a 110 to 130 ms check
(MILESTONE-2.md has the numbers).

The server's own subprocesses (`inko --version`, `inko print std`) have a
10 s timeout.

## Freshness, cost and side effects

- **Freshness.** Every query type checks the document and every module it
  imports (including the standard library's prelude) from scratch, with the
  open documents' text. Nothing is cached, so an answer is never older than
  the request. Queries don't lower to MIR, which types never depend on.
- **Cost.** A query on a file that imports all of the standard library (116
  modules) takes about 120 ms on an Apple M1 Max. One on a typical std
  module takes 30 to 100 ms.
- **No side effects.** Unlike `inko check`, a query never syncs `dep/`, never
  touches the network, and never creates or locks anything in Inko's data
  directory. It writes nothing anywhere. It does enforce the Inko version
  that the project's and its dependencies' `inko.pkg` require. Dependencies
  are whatever `inko pkg sync` last put in `dep/`.

## `shutdown` and `exit`

- `shutdown` answers `null`.
- `exit` ends the process with status 0 after `shutdown`, and 1 without it.
- If stdin closes without `exit`, the process ends with status 1.
- Malformed messages (bad framing or JSON) end the session, as lsp-server
  handles them; clients don't send those.

## Testing faults

For tests only: with `CUTTLEBONE_FAULT=hang:<suffix>` or
`CUTTLEBONE_FAULT=abort:<suffix>` in the server's environment, a worker
checking a file whose path ends with `<suffix>` hangs or aborts instead of
checking. No real input is known to do either, and the server must survive
both.

## Example

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"rootUri":"file:///home/me/hello","capabilities":{},"initializationOptions":{"inko":"/usr/local/bin/inko"}}}
← {"jsonrpc":"2.0","id":1,"result":{"capabilities":{...},"serverInfo":{"name":"cuttlebone","version":"0.1.0"}}}
→ {"jsonrpc":"2.0","method":"initialized","params":{}}
→ {"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///home/me/hello/src/main.inko","languageId":"inko","version":1,"text":"..."}}}
→ {"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{"textDocument":{"uri":"file:///home/me/hello/src/main.inko"},"position":{"line":5,"character":14}}}
← {"jsonrpc":"2.0","id":2,"result":{"contents":{"kind":"markdown","value":"```inko\nInt\n```"},"range":{"start":{"line":5,"character":14},"end":{"line":5,"character":16}}}}
→ {"jsonrpc":"2.0","id":3,"method":"shutdown"}
← {"jsonrpc":"2.0","id":3,"result":null}
→ {"jsonrpc":"2.0","method":"exit"}
```
