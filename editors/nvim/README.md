# kgrust for Neovim

`.k` files as Kanagawa, with `kg lsp`, kgrust's language server, attached:
diagnostics as you type, inferred widths on hover and as inlay hints,
go to definition, references, rename, completion, formatting and semantic
highlighting. That's everything the server answers, which the
[VS Code extension's README](../vscode/README.md) lists in full.

Needs **Neovim 0.11** or later, and `kg` on your `PATH` (a
[release or nightly archive](../../README.md#install), or `cargo xtask
install` from a checkout). Without `kg`, the plugin highlights and indents,
and says once that it found no server.

## Install

The plugin is the `editors/nvim` directory of this repository.

**lazy.nvim:**

```lua
{
  'jptech/kgrust',
  lazy = false,
  config = function(plugin)
    vim.opt.rtp:append(plugin.dir .. '/editors/nvim')
    require('kgrust').setup()
  end,
}
```

**vim-plug:** `Plug 'jptech/kgrust', { 'rtp': 'editors/nvim' }`

**Without a plugin manager**, link the directory into a package:

```sh
mkdir -p ~/.local/share/nvim/site/pack/kgrust/start
ln -s /path/to/kgrust/editors/nvim ~/.local/share/nvim/site/pack/kgrust/start/kgrust
```

Installed through a runtime path like the last two, the plugin sets itself
up at startup, reading its options from `vim.g.kgrust` if you set that first.

## Options

```lua
require('kgrust').setup({
  cmd = { '/path/to/kg', 'lsp' },  -- default { 'kg', 'lsp' }
  lsp = true,                      -- false: highlighting and indentation, no server
})
```

The server is an ordinary `vim.lsp.config` entry named `kanagawa`, so
anything else goes there: `vim.lsp.config('kanagawa', { … })`. Its root is
the nearest directory holding a `kanagawa.toml` (or `.git`). The server
reads the project's manifests itself, editions included.

Formatting is the server's, which is `kg fmt`'s: `vim.lsp.buf.format()`,
or on every save:

```lua
vim.api.nvim_create_autocmd('BufWritePre', {
  pattern = '*.k',
  callback = function() vim.lsp.buf.format({ timeout_ms = 2000 }) end,
})
```

## What's in it

| File | Does |
|---|---|
| `lua/kgrust/init.lua` | `setup`: `.k` as the `kanagawa` filetype (Neovim's own default for `.k` is `kwt`), and the server enabled; `indent`, the `indentexpr` |
| `lsp/kanagawa.lua` | the server's configuration: `kg lsp`, the filetype, the root markers |
| `plugin/kgrust.lua` | `setup` at startup, for runtime-path installs |
| `ftplugin/kanagawa.lua` | comments (`// %s`; a `//|` run continues with `//`), four-space indents, the `indentexpr` |
| `syntax/kanagawa.vim` | highlighting until the server's semantic tokens arrive; **generated** by `cargo xtask codegen` from the parser's keyword list, like the VS Code grammar, and checked by `codegen --check` |
| `test/smoke.lua`, `test/indent.k` | the test below |

**Indentation** follows `kg fmt`'s layout:
- braces on their own lines;
- lists with leading separators (`{ a`, `, b`, `}`), as `module` exports and template parameters are written;
- a lone statement one level in under its `if` or `for`;
- comments continuing a `//<`.

It is a typing aid, not a formatter. Measured on 25 files of the reference's library and tests that it was not tuned on, each formatted by `kg fmt` (2026-11-01), the indent it gives each line matches `kg fmt`'s on 98.4% of lines, against 94.8% for `cindent`. A whole-file `gg=G` leaves 91% of lines alone, against 75% for `cindent`. Format with the server to get the rest right.

## Testing

`test/smoke.lua` drives a headless Neovim through the plugin as a user meets
it:
- the filetype, highlighting and comment settings;
- `gg=G` leaving `test/indent.k`, `kg fmt`'s layout of every case above, unchanged;
- the server attaching;
- a diagnostic on the right line;
- hover on a parameter;
- formatting through the server.

```sh
KG_BIN=target/release KANAGAWA_LIBRARY=third_party/kanagawa/library \
  nvim --headless --clean -l editors/nvim/test/smoke.lua
```

`KG_BIN` is the directory holding the `kg` to test (default: the one on
`PATH`). `KANAGAWA_LIBRARY` is needed by a source build, which bundles no
standard library. Tested on Neovim 0.12.5 on macOS. CI does not run it.
