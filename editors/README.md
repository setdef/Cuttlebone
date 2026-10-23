# Editor setups

Cuttlebone is a plain LSP server (see [docs/lsp.md](../docs/lsp.md)), so any
editor with an LSP client can start it. These are minimal setups for two
editors, each with a scripted smoke check.

Build the server first (`cargo build --release`). Then either put
`target/release/cuttlebone` on your `PATH`, or give its absolute path in the
settings below. Cuttlebone also needs `inko` 0.21.1, on `PATH` or configured.

## Checking both

    editors/check.sh            # or: editors/check.sh neovim / vscode

Each check:

1. starts `cuttlebone` through the editor's own LSP client
2. opens `tests/fixtures/unicode/src/main.inko`
3. hovers `[3]` on the line `let e = '中文' + [3].size.to_string`, and
   expects `Array[Int]`
4. goes to the definition of `e` on the line `let f = '日本' + e`, and
   expects the `e` that `let e` binds, on line 12

`中文` and `日本` are 6 bytes but 2 UTF-16 units. A position in the wrong
encoding lands on `size` and shows `Int`, or lands past the end of the line
and has no definition, so the check also proves the encoding negotiation: Neovim gets UTF-8, VS Code UTF-16. Each check prints
`<editor>: ok: ...` or `<editor>: FAIL: ...`, and the script exits non-zero
on any failure.

- **Neovim** runs headless (`nvim --headless`). `cargo test` runs it too
  (`tests/editors.rs`), unless `nvim` isn't on `PATH`.
- **VS Code** has no headless mode on macOS. The check opens a VS Code window
  for a few seconds, with a throwaway profile and other extensions disabled,
  and closes it again. Nothing needs clicking. On Linux, run it under
  `xvfb-run`. It uses the VS Code in `/Applications` (or `$VSCODE`). Without
  one, `@vscode/test-electron` downloads VS Code.

## Neovim (0.11 or newer)

[`neovim/lsp/cuttlebone.lua`](neovim/lsp/cuttlebone.lua) is a server
configuration in nvim-lspconfig's format, for Neovim's built-in
`vim.lsp.config`. Neovim already detects the `inko` filetype.

1. Copy it to `~/.config/nvim/lsp/cuttlebone.lua`.
2. Add this to `init.lua`:

   ```lua
   vim.lsp.enable('cuttlebone')

   -- Optional, before enable:
   vim.lsp.config('cuttlebone', {
     cmd = { '/path/to/cuttlebone' },
     init_options = { inko = '/path/to/inko', statusNotifications = false },
   })
   ```

To check by hand: open an `.inko` file, then run `:checkhealth vim.lsp`. It
should list `cuttlebone` as attached. Press `K` on an expression to see its
type, and `<C-]>` (or `:lua vim.lsp.buf.definition()`) on a name to go to
its definition. `grr` lists its references. Type `.` after a value and
press `<C-x><C-o>` to see its members (or let your completion plugin show
them).

## VS Code

[`vscode/`](vscode) is a minimal extension. It registers `.inko` files as
the `inko` language and starts `cuttlebone` over stdio with
`vscode-languageclient`. It has two settings:

- `cuttlebone.path` (default `cuttlebone`)
- `cuttlebone.inko` (default: `inko` on `PATH`)

VS Code started from the Dock doesn't see your shell's `PATH`, so set
`cuttlebone.path` (and `cuttlebone.inko`, if needed) to absolute paths.

To try it by hand:

1. In a terminal: `cd editors/vscode && npm ci`.
2. In VS Code: **File → Open Folder…**, and choose `editors/vscode`.
3. Press **F5** (or **Run → Start Debugging**). An *Extension Development
   Host* window opens on `tests/fixtures/unicode`.
4. In that window: **Code → Settings… → Settings** (⌘,). Search for
   `cuttlebone.path`, and enter the absolute path of
   `target/release/cuttlebone`. If `inko` isn't on the `PATH` VS Code sees,
   set `cuttlebone.inko` too. Then run **Developer: Reload Window** from the
   Command Palette (⇧⌘P).
5. Open `src/main.inko` from the Explorer.
6. Hover over `[3]` in `let e = '中文' + [3].size.to_string`. The hover shows
   `Array[Int]`; over `size` it shows `Int`.
7. Press F12 on the `e` in `let f = '日本' + e`. The cursor goes to the `e`
   in `let e`. ⇧F12 on it lists both. ⌘-click `Stdout` to open std.stdio.
8. On a new line in `main`, type `out.`. The suggestions list `print` and
   the other `Stdout` methods, each with its signature.
9. If nothing happens, open **View → Output** and pick *Cuttlebone* in the
   drop-down. It shows the server's log, including a refused Inko version.

To install it for everyday use instead, run **Developer: Install Extension
from Location…** from the Command Palette, and choose `editors/vscode` (after
`npm ci`). An Inko extension you already have may also register `.inko`
files; that's fine. If it also shows hovers, you'll see both.
