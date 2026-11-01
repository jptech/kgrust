-- The plugin as a user meets it, in a Neovim driven headless: a `.k` file
-- opened in a project, then its filetype, its highlighting and `kg lsp`'s
-- answers. Run from anywhere:
--
--   KG_BIN=<directory holding kg> KANAGAWA_LIBRARY=<the standard library> \
--     nvim --headless --clean -l editors/nvim/test/smoke.lua
--
-- KG_BIN defaults to the `kg` on PATH. KANAGAWA_LIBRARY is needed by a `kg`
-- built from source, which bundles no library (a release's finds its own).
-- Exits 0 when every answer arrives, else 1 with the first that did not.

local plugin = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h:h')
local function fail(msg)
  io.stderr:write('FAIL: ' .. msg .. '\n')
  os.exit(1)
end
local function check(ok, msg)
  if not ok then
    fail(msg)
  end
end

-- As a plugin manager would: the directory on the runtime path, its
-- plugin/ sourced, and Neovim's own filetype and syntax machinery on.
vim.opt.rtp:prepend(plugin)
if os.getenv('KG_BIN') then
  vim.env.PATH = os.getenv('KG_BIN') .. ':' .. vim.env.PATH
end
vim.cmd('filetype plugin indent on')
vim.cmd('syntax on')
vim.cmd('runtime! plugin/kgrust.lua')

-- Indentation: `gg=G` over a file in `kg fmt`'s layout changes nothing.
vim.cmd.edit(plugin .. '/test/indent.k')
local formatted = vim.api.nvim_buf_get_lines(0, 0, -1, false)
vim.cmd('silent normal! gg=G')
for i, want in ipairs(formatted) do
  local got = vim.api.nvim_buf_get_lines(0, i - 1, i, false)[1]
  check(got == want, ('indent.k:%d re-indented as [%s], kg fmt writes [%s]'):format(i, got, want))
end
vim.cmd('bwipeout!')

local root = vim.fn.tempname()
vim.fn.mkdir(root .. '/src', 'p')
local manifest = { '[package]', 'name = "smoke"', '', '[project]', 'sources = ["src"]' }
if os.getenv('KANAGAWA_LIBRARY') then
  table.insert(manifest, ('import-dirs = ["%s"]'):format(os.getenv('KANAGAWA_LIBRARY')))
end
vim.fn.writefile(manifest, root .. '/kanagawa.toml')
vim.fn.writefile({
  '//| Adds one.',
  'inline uint32 add_one(uint32 x)',
  '{',
  '    return x + 1;',
  '}',
  '',
  'inline uint32 broken(uint32 x)',
  '{',
  '    return y;',
  '}',
}, root .. '/src/main.k')

vim.cmd.edit(root .. '/src/main.k')
check(vim.bo.filetype == 'kanagawa', 'filetype: ' .. vim.bo.filetype)
check(vim.b.current_syntax == 'kanagawa', 'syntax/kanagawa.vim not loaded')
check(vim.bo.commentstring == '// %s', 'ftplugin/kanagawa.lua not loaded')
local function group(line, col)
  return vim.fn.synIDattr(vim.fn.synID(line, col, 1), 'name')
end
check(group(1, 1) == 'kanagawaDocComment', 'a `//|` line is ' .. group(1, 1))
check(group(2, 1) == 'kanagawaModifier', '`inline` is ' .. group(2, 1))
check(group(2, 8) == 'kanagawaType', '`uint32` is ' .. group(2, 8))

local function client()
  return vim.lsp.get_clients({ bufnr = 0, name = 'kanagawa' })[1]
end
check(vim.wait(15000, function()
  return client() ~= nil and client().initialized
end, 50), 'kg lsp did not attach')

-- `y` names nothing: one error, on line 9.
check(vim.wait(30000, function()
  return #vim.diagnostic.get(0) > 0
end, 50), 'no diagnostics')
local diagnostic = vim.diagnostic.get(0)[1]
check(diagnostic.lnum == 8, ('the diagnostic is on line %d: %s'):format(diagnostic.lnum + 1, diagnostic.message))

-- Hover on the `x` of `return x + 1;`.
local function hover()
  local replies = vim.lsp.buf_request_sync(0, 'textDocument/hover', {
    textDocument = { uri = vim.uri_from_bufnr(0) },
    position = { line = 3, character = 11 },
  }, 10000) or {}
  for _, reply in pairs(replies) do
    local contents = reply.result and reply.result.contents
    if contents then
      return type(contents) == 'table' and (contents.value or vim.inspect(contents)) or contents
    end
  end
  return ''
end
local shown = ''
check(vim.wait(20000, function()
  shown = hover()
  return shown:find('uint32', 1, true) ~= nil
end, 200), 'hover on `x` says: ' .. shown)

-- Formatting through the server: a line written badly comes back as kg fmt writes it.
vim.api.nvim_buf_set_lines(0, 3, 4, false, { 'return x+1;' })
vim.lsp.buf.format({ timeout_ms = 10000 })
local line = vim.api.nvim_buf_get_lines(0, 3, 4, false)[1]
check(line == '    return x + 1;', 'formatted as: ' .. line)

io.stdout:write('ok: filetype, syntax, ftplugin, indentation, attach, diagnostics, hover, formatting\n')
client():stop()
vim.wait(5000, function()
  return client() == nil
end, 50)
os.exit(0)
