-- A headless smoke check: starts Cuttlebone through Neovim's own LSP client,
-- with the configuration in lsp/cuttlebone.lua, opens a file, hovers, and
-- goes to a definition. Run by editors/check.sh, which sets these
-- environment variables:
--
--   CUTTLEBONE  the cuttlebone binary
--   FILE        the .inko file to open
--   NEEDLE      text to hover, at its first occurrence
--   OFFSET      characters into NEEDLE to hover at
--   EXPECTED    the type the hover must show
--   DEFINITION  text to go to the definition of, at its first occurrence
--   DEFINITION_OFFSET  characters into DEFINITION
--   DEFINED_LINE  the 1-based line in FILE the definition must be on
--   DEFINED     the text the definition's range must cover
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

-- Puts the cursor `offset` characters into the first occurrence of `needle`.
local function go_to(needle, offset)
  vim.api.nvim_win_set_cursor(0, { 1, 0 })

  local line = vim.fn.search(vim.fn.escape(needle, '\\/.*$^~[]'), 'cw')

  if line == 0 then
    fail(needle .. ' isn\'t in the file')
  end

  local text = vim.api.nvim_buf_get_lines(0, line - 1, line, true)[1]
  local start = text:find(needle, 1, true) - 1
  local column = vim.str_byteindex(text:sub(start + 1), 'utf-32', tonumber(offset))

  vim.api.nvim_win_set_cursor(0, { line, start + column })
end

local needle = os.getenv('NEEDLE')

go_to(needle, os.getenv('OFFSET'))

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

-- Go to definition. With the wrong encoding, the position is past the end
-- of the line, and there's no definition.
local definition = os.getenv('DEFINITION')

go_to(definition, os.getenv('DEFINITION_OFFSET'))
params = vim.lsp.util.make_position_params(0, client.offset_encoding)
response = client:request_sync('textDocument/definition', params, 20000, 0)

if not response or response.err or not response.result then
  fail('no definition: ' .. vim.inspect(response))
end

local location = response.result.uri and response.result or response.result[1]
local range = location.range
local file = vim.uv.fs_realpath(vim.uri_to_fname(location.uri))
local line = vim.api.nvim_buf_get_lines(0, range.start.line, range.start.line + 1, true)[1]
-- Positions are in UTF-8, so characters are bytes.
local covered = line:sub(range.start.character + 1, range['end'].character)

if file ~= vim.uv.fs_realpath(os.getenv('FILE'))
  or range.start.line + 1 ~= tonumber(os.getenv('DEFINED_LINE'))
  or covered ~= os.getenv('DEFINED')
then
  fail('the definition is ' .. vim.inspect(location) .. ', covering ' .. vim.inspect(covered))
end

io.stdout:write('neovim: ok: hover at ' .. needle .. ' shows ' .. os.getenv('EXPECTED')
  .. ' (positions in ' .. client.offset_encoding .. ')\n')
io.stdout:write('neovim: ok: definition at ' .. definition .. ' is ' .. covered
  .. ' on line ' .. (range.start.line + 1) .. '\n')
vim.cmd('qall!')
