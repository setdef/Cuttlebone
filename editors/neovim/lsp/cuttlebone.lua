-- Cuttlebone for Neovim 0.11 or newer, in nvim-lspconfig's format.
--
-- Copy this file to ~/.config/nvim/lsp/cuttlebone.lua (or put the directory
-- above it on 'runtimepath'), then add this to your init.lua:
--
--   vim.lsp.enable('cuttlebone')
--
-- To change a setting, call vim.lsp.config before enabling it, e.g.:
--
--   vim.lsp.config('cuttlebone', {
--     cmd = { '/path/to/cuttlebone' },
--     init_options = { inko = '/path/to/inko' },
--   })
--
-- Neovim detects the `inko` filetype for *.inko files by itself.
return {
  cmd = { 'cuttlebone' },
  filetypes = { 'inko' },
  -- The project root: the nearest directory with an inko.pkg, or else with
  -- a src/ directory.
  root_markers = { 'inko.pkg', 'src' },
  -- See docs/lsp.md for every option. Neovim negotiates UTF-8 positions.
  init_options = vim.empty_dict(),
}
