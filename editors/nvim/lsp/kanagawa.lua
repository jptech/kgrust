-- `kg lsp`, kgrust's language server, as `vim.lsp.enable('kanagawa')` reads
-- it (Neovim 0.11+). `require('kgrust').setup()` enables it for you.
return {
  cmd = { 'kg', 'lsp' },
  filetypes = { 'kanagawa' },
  -- A project's root holds its `kanagawa.toml`; the server reads the
  -- manifests itself, so this only decides which buffers share a server.
  root_markers = { 'kanagawa.toml', '.git' },
}
