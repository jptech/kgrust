-- Kanagawa buffers: comments, indentation and layout as `kg fmt` writes them.
if vim.b.did_ftplugin then
  return
end
vim.b.did_ftplugin = true

vim.bo.commentstring = '// %s'
-- `//` alone also continues a `//|` or `//<` run: the lexer reads every line
-- comment after a doc comment's first as part of it (`Parser/Lexer.hs:82-84`),
-- so repeating the `|` would put it in the documentation.
vim.bo.comments = 's1:/*,mb:*,ex:*/,://'
vim.opt_local.formatoptions:remove('t')
vim.opt_local.formatoptions:append('croql')
-- `kg fmt`'s defaults: four spaces, braces on their own lines.
vim.bo.expandtab = true
vim.bo.shiftwidth = 4
vim.bo.softtabstop = 4
-- Not 'cindent': on the reference's library as `kg fmt` writes it, it
-- re-indents a fifth of the lines (block comments' text, import lists).
vim.bo.indentexpr = "v:lua.require'kgrust'.indent()"
vim.bo.indentkeys = '0{,0},0),0],!^F,o,O,e,<:>'
vim.bo.autoindent = true

vim.b.undo_ftplugin = 'setlocal commentstring< comments< formatoptions< expandtab< shiftwidth< softtabstop< indentexpr< indentkeys< autoindent<'
