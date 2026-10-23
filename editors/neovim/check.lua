-- A headless smoke check: starts Cuttlebone through Neovim's own LSP client,
-- with the configuration in lsp/cuttlebone.lua, opens a file, and hovers.
-- Run by editors/check.sh, which sets these environment variables:
--
--   CUTTLEBONE  the cuttlebone binary
--   FILE        the .inko file to open
--   NEEDLE      text to hover, at its first occurrence
--   OFFSET      characters into NEEDLE to hover at
--   EXPECTED    the type the hover must show
--   INKO        optional: the inko executable
local function fail(message)
  io.stderr:write('neovim: FAIL: ' .. message .. '\n')
  vim.cmd('cquit 1')
end

local here = vim.fs.dirname(debug.getinfo(1, 'S').source:sub(2))

vim.opt.runtimepath:prepend(here)
vim.lsp.config('cuttlebone', {
  cmd = { os.getenv('CUTTLEBONE') },
  init_options = { inko = os.getenv('INKO') },
})
vim.lsp.enable('cuttlebone')
vim.cmd.edit(os.getenv('FILE'))

if vim.bo.filetype ~= 'inko' then
  fail('the filetype is ' .. vim.bo.filetype .. ', not inko')
end

local client
local started = vim.wait(20000, function()
  client = vim.lsp.get_clients({ bufnr = 0, name = 'cuttlebone' })[1]
  return client ~= nil and client.initialized
end, 20)

if not started then
  fail('the server didn\'t start; see :LspLog (' .. vim.lsp.log.get_filename() .. ')')
end

-- Neovim offers UTF-8 first, so that's what Cuttlebone must pick.
if client.offset_encoding ~= 'utf-8' then
  fail('the position encoding is ' .. client.offset_encoding .. ', not utf-8')
end

local needle = os.getenv('NEEDLE')
local line = vim.fn.search(vim.fn.escape(needle, '\\/.*$^~[]'), 'cw')

if line == 0 then
  fail(needle .. ' isn\'t in the file')
end

local text = vim.api.nvim_buf_get_lines(0, line - 1, line, true)[1]
local start = text:find(needle, 1, true) - 1
local column = vim.str_byteindex(text:sub(start + 1), 'utf-32', tonumber(os.getenv('OFFSET')))

vim.api.nvim_win_set_cursor(0, { line, start + column })

local params = vim.lsp.util.make_position_params(0, client.offset_encoding)
local response = client:request_sync('textDocument/hover', params, 20000, 0)

if not response or response.err or not response.result then
  fail('no hover: ' .. vim.inspect(response))
end

local value = response.result.contents.value
local expected = '```inko\n' .. os.getenv('EXPECTED') .. '\n```'

if value ~= expected then
  fail('the hover shows ' .. vim.inspect(value) .. ', not ' .. vim.inspect(expected))
end

io.stdout:write('neovim: ok: hover at ' .. needle .. ' shows ' .. os.getenv('EXPECTED')
  .. ' (positions in ' .. client.offset_encoding .. ')\n')
vim.cmd('qall!')
