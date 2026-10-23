# The Cuttlebone protocol

Parakeet (or any other client) starts `cuttlebone` as a long-lived child
process and talks to it over stdin and stdout. This is Cuttlebone's own small
protocol, not LSP. An LSP front end will come later, on the same core.

This document describes protocol version **1**.

## Transport

- Each message is one JSON object on one line, in UTF-8, ended by `\n`. JSON
  escapes the newlines inside strings, so a buffer's text fits on one line.
- The client writes requests to Cuttlebone's stdin, and Cuttlebone writes
  exactly one reply per request to stdout, in the order the requests arrived.
  It handles one request at a time.
- stderr carries free-form logs, such as a compiler panic's message and
  backtrace note. Clients may show or discard it, but shouldn't parse it.
- Cuttlebone exits with status 0 when stdin ends or after replying to
  `shutdown`.

A request:

```json
{"id": 1, "method": "typeAt", "params": {...}}
```

`id` is any JSON value. The reply echoes it. A line that isn't a JSON object
gets a reply with `"id": null`.

A reply:

```json
{"id": 1, "status": "fresh", "result": {...}, "elapsedMs": 84.2}
{"id": 1, "status": "unavailable", "reason": {"code": "noHir", "message": "..."}, "elapsedMs": 31.0}
```

## Status

Every reply carries `status`:

| Status | Meaning |
|---|---|
| `fresh` | Computed from the files as they are now (on disk, overridden by the request's `contents`). `result` holds the answer, which may be `null` when there's nothing at the position. |
| `unavailable` | Cuttlebone has no answer. `reason` says why. The client should fall back to its own heuristics. |
| `stale` | Reserved. A later version may answer from a stored earlier result when the current code can't be typed, and will mark such answers `stale`. Version 1 never sends it. |

Clients must treat any status other than `fresh` as having no usable
`result`, and must accept and ignore fields they don't know, so that later
versions can add statuses and fields without a new protocol version.

`reason` always has `code`, a stable identifier, and `message`, a description
for people. Some codes add fields, listed below.

| Code | When |
|---|---|
| `invalidRequest` | The line isn't a JSON object, or `params` is missing something or has the wrong type. |
| `unknownMethod` | There's no method by that name. |
| `notInitialized` | A query came before a successful `initialize`. |
| `protocolMismatch` | `initialize` asked for another protocol version. Adds `protocol`, the version this build speaks. |
| `inkoNotFound` | `initialize` couldn't run `inko --version` or `inko print std`, or the latter isn't a directory. Adds `cuttlebone` and `supported`. |
| `inkoVersionMismatch` | The project's `inko` isn't the version this build of Cuttlebone links. Adds `cuttlebone`, `supported` and `installed`. |
| `fileNotFound` | The file asked about, or a path in `contents`, doesn't exist on disk. |
| `invalidPosition` | The position's line is past the end of the file. |
| `noHir` | The file, or a module it imports, couldn't be parsed or found (for example a syntax error, or an import of a module that doesn't exist), so nothing was type checked. |
| `declarationErrors` | An error in a declaration (for example an undefined type in a signature) stopped the type checker before it typed any method body. |
| `untyped` | The compiler couldn't type the expression at the position (or the receiver, for `membersAt`), usually because of an error in it. Other expressions in the file may still have types. |
| `compilerPanic` | The compiler panicked. `message` has the panic's message and location. The process keeps running. |
| `compilerError` | The compiler reported an internal error, such as a project that needs a newer Inko or dependencies that failed to sync. |
| `internal` | A bug in Cuttlebone. |

## Positions

A position is `{"line": 0, "character": 4}`:

- `line` counts from 0.
- `character` counts UTF-16 code units from the start of the line, from 0.
  This is what IntelliJ's document offsets and LSP use.

A range is `{"start": position, "end": position}`, with `end` exclusive.

A position means the character that starts there, as in LSP. A client that
wants the expression just before a caret (such as a caret right after `foo`)
should send the position of the character before the caret.

Lines end at `\n`. A `\r` before it counts as part of the line, so it never
affects a position before it. A character past the end of a line means the
end of the line.

The compiler counts columns in extended grapheme clusters, so Cuttlebone
converts: a cluster such as 👩‍👩‍👦 is one compiler column, 8 UTF-16 code
units, 5 code points and 18 bytes. A position inside a cluster (for example
the second half of a surrogate pair) means the whole cluster. The compiler
segments each token separately, and Cuttlebone each line, which only differs
when a combining character or joiner directly follows a token boundary (for
example a string literal that starts with U+0301).

## Unsaved contents

`typeAt` and `membersAt` take an optional `contents` object that maps file
paths to text to use instead of what's on disk:

```json
"contents": {"/home/me/project/src/main.inko": "import std.stdio\n..."}
```

- It applies to the file asked about and to any module it imports, so a
  client can send every unsaved buffer of the project.
- Each path must exist on disk; Cuttlebone resolves symlinks and `..` before
  matching.
- Nothing is kept between requests. A request without `contents` sees the
  files on disk.

## Methods

### `initialize`

The handshake. Every other query is refused until one succeeds. It may be
sent again, for example after the user picks another toolchain.

Params:

| Field | Type | |
|---|---|---|
| `protocol` | number | Optional. The protocol version the client speaks. If given and different from Cuttlebone's, the reply is `protocolMismatch`. |
| `inko` | string | Optional. The `inko` executable the project uses (in Parakeet, the toolchain from *Settings → Languages & Frameworks → Inko*). Defaults to `inko` on `PATH`. |
| `project` | string | Optional. The project's root, the directory containing `src/` (and `inko.pkg`, if any). Files under its `src/` or `test/` are checked as part of it. For any other file, the root is the nearest directory whose `src/` or `test/` contains it, or the file's own directory. |

Cuttlebone runs `inko --version` and compares it with the Inko version it
links. On a match it runs `inko print std` to find the standard library to
check against.

Result:

```json
{"cuttlebone": "0.1.0", "inko": "0.21.1", "protocol": 1, "std": "/path/to/lib/inko/std"}
```

A mismatch:

```json
{"id": 1, "status": "unavailable", "reason": {"code": "inkoVersionMismatch",
  "message": "this build of Cuttlebone supports Inko 0.21.1, but /usr/bin/inko is Inko 0.20.0",
  "cuttlebone": "0.1.0", "supported": "0.21.1", "installed": "0.20.0"}}
```

### `typeAt`

The type of the expression at a position.

Params:

| Field | Type | |
|---|---|---|
| `file` | string | The file's absolute path. |
| `position` | position | |
| `contents` | object | Optional. See [Unsaved contents](#unsaved-contents). |

Result, or `null` when no expression contains the position (for example in a
comment, between declarations, or on a method's name in its definition):

```json
{"type": "Array[Int]", "range": {"start": {"line": 4, "character": 18}, "end": {"line": 4, "character": 26}}, "kind": "Scope"}
```

| Field | |
|---|---|
| `type` | The type, formatted the way the compiler formats types in its diagnostics, e.g. `ref Array[Int]`, `Result[Int, String]`, `uni Future[Int]`. A type the compiler couldn't infer fully may contain `?`. |
| `range` | The expression's range. |
| `kind` | The compiler's node kind (e.g. `Call`, `IdentifierRef`, `Binding`). For debugging only; the set of kinds isn't part of the protocol. |

Besides expressions, variable bindings count: the name in `let name = ...`,
in a `match` or `for` pattern, and a closure's argument. Their type is the
variable's.

#### Which expression a position means

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

### `membersAt`

The members available after a `.`: the methods, fields, constants and types
that `receiver.name` could refer to, for the call whose name is at the
position.

Completion relies on a placeholder. The client sends a copy of the buffer (in
`contents`) with a dummy name at the caret, for example
`name.IntellijIdeaRulezzz`, which parses. The receiver keeps its type, and the
position is any character of the dummy name, or the position right after it.

Params: as for `typeAt`.

Result, or `null` when the position isn't on a call's name:

```json
{
  "receiver": {"type": "String", "range": {"start": {...}, "end": {...}}},
  "members": [
    {"name": "size", "kind": "method", "signature": "fn pub inline size -> Int", "type": "Int", "module": "std.string"},
    {"name": "label", "kind": "field", "signature": "let pub @label: String", "type": "String", "module": "main"}
  ]
}
```

| Field | |
|---|---|
| `name` | What to insert after the `.`. |
| `kind` | `method`, `field`, `constant` or `type`. |
| `signature` | The declaration, with the receiver's type arguments filled in, including in default methods from generic traits (e.g. `fn pub inline get(index: Int) -> Result[ref Int, OutOfBounds]` on an `Array[Int]`). |
| `type` | What using the member gives: a method's return type, a field's or constant's type, or the type itself. |
| `module` | The module that defines it. |

Members are sorted by name, and each name appears once. Which members are
listed follows the compiler's own rules for `receiver.name`:

- On a value: its type's instance methods (including those from implemented
  traits), and its fields if the type isn't `async` and the field is visible
  from this module. Private members are visible within the same root
  namespace (e.g. `foo.bar` from `foo.baz`). An `async` type's private
  methods are only listed on `self`.
- On a type (`Counter.`): its static methods.
- On a module (`shapes.`): its methods, and the constants and types it
  defines.
- On a type parameter or trait: the methods of its requirements.
- Methods that need a `mut` or owned receiver aren't listed on a `ref`
  receiver.

Members aren't filtered by what's already typed after the dot; the client
does that.

### `shutdown`

Replies `fresh` with a `null` result, then exits.

## Freshness and cost

Every query type checks the file and every module it imports (including the
standard library's prelude) from scratch, with `contents` applied. Nothing is
cached between requests, so an answer is never older than the request. It
doesn't lower to MIR, which types never depend on.

A query on a file that imports all of the standard library (116 modules)
takes about 110 ms on an Apple M1 Max; one on a typical std module takes 30
to 90 ms.

## Robustness

A compiler panic is caught and reported as `compilerPanic`, and the process
keeps answering. A failure that can't be caught (an abort, a stack overflow
deeper than the check thread's 64 MiB, or a hang) would take the process
down or stall it, so clients should apply a timeout to each request (a few
seconds is plenty) and restart Cuttlebone if it times out or exits.

## Example

```
→ {"id":1,"method":"initialize","params":{"protocol":1,"inko":"/usr/local/bin/inko","project":"/home/me/hello"}}
← {"id":1,"status":"fresh","result":{"cuttlebone":"0.1.0","inko":"0.21.1","protocol":1,"std":"/usr/local/lib/inko/std"}}
→ {"id":2,"method":"typeAt","params":{"file":"/home/me/hello/src/main.inko","position":{"line":5,"character":14}}}
← {"id":2,"status":"fresh","result":{"type":"Int","range":{"start":{"line":5,"character":14},"end":{"line":5,"character":16}},"kind":"Int"},"elapsedMs":38.3}
→ {"id":3,"method":"shutdown"}
← {"id":3,"status":"fresh","result":null}
```
