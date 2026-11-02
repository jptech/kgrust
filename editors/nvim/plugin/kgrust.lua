-- For installs that put `editors/nvim` on the runtime path before startup: a
-- plugin manager's subdirectory option, or a link under `pack/*/start`.
-- lazy.nvim users call `require('kgrust').setup()` from `config` (README.md).
if vim.g.loaded_kgrust then
  return
end
vim.g.loaded_kgrust = true
require('kgrust').setup(vim.g.kgrust)
