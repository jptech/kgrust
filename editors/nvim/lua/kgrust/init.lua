--- kgrust for Neovim: `.k` files as the `kanagawa` filetype, and `kg lsp` as
--- their language server. `setup` is the whole of it, and may be called again
--- to change it. `plugin/kgrust.lua` calls it, with `vim.g.kgrust` as its
--- options, when this directory is on the runtime path at startup.
local M = {}

M.defaults = {
  -- The server's command line: a `kg` that is not on PATH goes here.
  cmd = { 'kg', 'lsp' },
  -- false: the filetype and highlighting alone, no server.
  lsp = true,
}

function M.setup(opts)
  opts = vim.tbl_deep_extend('force', M.defaults, opts or {})
  -- Neovim reads `.k` as `kwt` on its own.
  vim.filetype.add({ extension = { k = 'kanagawa' } })
  if vim.fn.has('nvim-0.11') == 0 then
    vim.notify('kgrust: the language server needs Neovim 0.11 or later', vim.log.levels.WARN)
    return
  end
  vim.lsp.config('kanagawa', { cmd = opts.cmd })
  local found = vim.fn.executable(opts.cmd[1]) == 1
  if opts.lsp and not found then
    -- Said once, here, rather than an error each time a buffer opens.
    vim.notify(('kgrust: `%s` is not on PATH; highlighting only'):format(opts.cmd[1]), vim.log.levels.WARN)
  end
  vim.lsp.enable('kanagawa', opts.lsp and found)
end

--- 'indentexpr' for Kanagawa buffers, as `kg fmt` lays them out: braces on
--- their own lines, a lone statement under its `if` or `for` one level in,
--- and lists written with leading separators:
---
---     module data.fifo          template
---         { FIFO                    < typename T  //< Type of an entry.
---         , fifo_nb                 , auto Size
---         }                         >
---
--- A list's separators and its closer line up; a comment inside it, or the
--- first item after an opener holding a comment, sits two columns in; a
--- comment continuing a trailing `//<` lines up with it; and the line after
--- the closer goes back to the line that opened the list. A line inside a
--- block comment keeps its own indent. A typing aid, not a formatter:
--- `kg fmt`, or `vim.lsp.buf.format()`, lays out the rest.
local function text(lnum)
  return vim.fn.getline(lnum)
end
local function is_comment(t)
  return t:match('^%s*//') ~= nil
end
local function code_of(t)
  return (t:gsub('%s*//.*$', ''))
end
-- `{ FIFO`, `( T lhs`, `< auto N`: the opening line of a leading-separator
-- list, its bracket left open. `{ -16, 2048 },` and `[[pipelined]]` are not.
local pairs_of = { ['{'] = '%b{}', ['('] = '%b()', ['['] = '%b[]', ['<'] = '%b<>' }
local function opens_list(t)
  t = code_of(t)
  local open = t:match('^%s*([{(%[<])%s*[^%s{}<=%[]')
  return open ~= nil and t:match('^%s*' .. pairs_of[open]) == nil
end
local function is_separator(t)
  return t:match('^%s*,') ~= nil
end
local function is_closer(t)
  return t:match('^%s*[})%]>]') ~= nil
end
-- `if (…)`, `static for (…)`, `else`: a head whose lone statement goes below it.
local function is_control_head(t)
  t = code_of(t)
  return t:match('^%s*else%s*$') ~= nil
    or t:match('^%s*static%s+if%s*%b()%s*$') ~= nil
    or t:match('^%s*static%s+for%s*%b()%s*$') ~= nil
    or t:match('^%s*else%s+if%s*%b()%s*$') ~= nil
    or t:match('^%s*if%s*%b()%s*$') ~= nil
    or t:match('^%s*for%s*%b()%s*$') ~= nil
    or t:match('^%s*while%s*%b()%s*$') ~= nil
end
-- The nearest line above `lnum` holding code, not a line comment.
local function code_above(lnum)
  local l = vim.fn.prevnonblank(lnum)
  while l > 0 and is_comment(text(l)) do
    l = vim.fn.prevnonblank(l - 1)
  end
  return l
end
-- The list `lnum` is inside: the nearest line above at or left of every line
-- between that opens the list or separates it, or nil outside any list.
local function list_line_above(lnum)
  local floor = math.huge
  for l = vim.fn.prevnonblank(lnum), 1, -1 do
    local t = text(l)
    if t:match('%S') and not is_comment(t) then
      local indent = vim.fn.indent(l)
      if indent <= floor and (opens_list(t) or is_separator(t)) then
        return l
      end
      if is_closer(t) or code_of(t):match('[;{]%s*$') then
        return nil
      end
      floor = math.min(floor, indent)
    end
  end
end
-- The outermost of a chain of brace-less heads ending at `head`: a
-- `static for` over an `if`, an `else` with its `if`.
local function outer_head(head)
  local width = vim.fn.shiftwidth()
  while true do
    if code_of(text(head)):match('^%s*else') then
      local l = code_above(head - 1)
      while l > 0 and vim.fn.indent(l) > vim.fn.indent(head) do
        l = code_above(l - 1)
      end
      if l > 0 and vim.fn.indent(l) == vim.fn.indent(head) and is_control_head(text(l)) then
        head = l
      end
    end
    local up = code_above(head - 1)
    if up > 0 and is_control_head(text(up)) and vim.fn.indent(head) == vim.fn.indent(up) + width then
      head = up
    else
      return head
    end
  end
end
-- `{ 0, 1,` / `2, 3 };`: lines of one statement continued after a trailing
-- comma. The first of them, for a line in such a run; nil where the run is
-- a block's items rather than a statement: an `enum`'s, or an initializer's.
local function continued_from(lnum)
  local start = lnum
  while true do
    local up = code_above(start - 1)
    local t = up > 0 and code_of(text(up)) or ''
    if up > 0 and t:match(',%s*$') and not is_separator(t) and not opens_list(t) then
      start = up
    elseif t:match('^%s*{%s*$') then
      -- Braces on their own line open a body, unless what they follow
      -- leaves a value or an item to come.
      local head = code_above(up - 1)
      local h = head > 0 and code_of(text(head)) or ''
      if h:match('^%s*enum%s') or h:match('[=,{]%s*$') then
        return nil
      end
      return start
    elseif t:match('{%s*$') then
      return nil
    else
      return start
    end
  end
end

-- The line opening what the closer on `lnum` closes: the nearest above at its
-- indent that starts with an opener.
local function opener_of(lnum)
  local at = vim.fn.indent(lnum)
  for l = lnum - 1, 1, -1 do
    local t = text(l)
    if t:match('%S') and not is_comment(t) then
      local indent = vim.fn.indent(l)
      if indent == at and t:match('^%s*[{(%[<]') then
        return l
      elseif indent < at then
        return nil
      end
    end
  end
end

function M.indent(lnum)
  lnum = lnum or vim.v.lnum
  local line = text(lnum)
  local syntax = vim.fn.synIDattr(vim.fn.synID(lnum, math.max(vim.fn.indent(lnum), 0) + 1, 1), 'name')
  if syntax:find('BlockComment', 1, true) then
    return -1
  end
  local above = vim.fn.prevnonblank(lnum - 1)
  if above == 0 then
    return 0
  end
  local width = vim.fn.shiftwidth()

  -- A comment continuing a trailing `//<`, or a run of comments.
  if is_comment(line) then
    if is_comment(text(above)) then
      return vim.fn.indent(above)
    end
    local col = text(above):find('//<', 1, true)
    if col then
      return col - 1
    end
  end

  local code = code_above(lnum - 1)
  local prior = code > 0 and code_of(text(code)) or ''

  -- Inside a leading-separator list.
  local list = list_line_above(lnum - 1)
  if list and not opens_list(line) then
    local at = vim.fn.indent(list)
    if is_separator(line) or is_closer(line) then
      return at
    end
    return at + 2
  end

  -- The line after a list's closer goes back to the line that opened it.
  local indent = vim.fn.indent(above)
  if code > 0 and is_closer(prior) then
    local opener = opener_of(code)
    if opener and opens_list(text(opener)) then
      local owner = code_above(opener - 1)
      indent = owner > 0 and vim.fn.indent(owner) or 0
      if is_closer(line) then
        indent = indent - width
      end
      return math.max(indent, 0)
    end
  end

  if opens_list(line) then
    -- An `enum`'s list stands where its `{` would; every other list, one in.
    if prior:match('^%s*enum%s') or prior:match('^%s*struct%s') or prior:match('^%s*union%s') then
      return vim.fn.indent(code)
    end
    return vim.fn.indent(above) + width
  end

  if code > 0 and is_control_head(prior) and not line:match('^%s*{') then
    return vim.fn.indent(code) + width
  end
  -- After a lone statement under a head, or a block under one: back out of
  -- every head in the chain, except that an `else` stands with its `if`.
  local head = code > 0 and code_above(code - 1) or 0
  if is_closer(prior) and prior:match('^%s*}') then
    local opener = opener_of(code)
    head = opener and code_above(opener - 1) or 0
    if head > 0 and not (is_control_head(text(head)) and vim.fn.indent(head) == vim.fn.indent(code)) then
      head = 0
    end
  elseif not (head > 0 and prior:match(';%s*$') and vim.fn.indent(code) == vim.fn.indent(head) + width) then
    head = 0
  end
  if head > 0 and is_control_head(text(head)) then
    indent = vim.fn.indent(line:match('^%s*else') and head or outer_head(head))
  end

  -- A statement continued after a trailing comma, and the line after it.
  local start = code > 0 and not is_separator(prior) and not opens_list(prior) and continued_from(code)
  if start then
    if prior:match(',%s*$') then
      return vim.fn.indent(start) + width
    elseif start ~= code then
      indent = vim.fn.indent(start)
    end
  end

  if prior:match('{%s*$') or prior:match('^%s*public:%s*$') or prior:match('^%s*private:%s*$') then
    indent = vim.fn.indent(code) + width
  end
  if is_closer(line) or line:match('^%s*public:') or line:match('^%s*private:') then
    indent = indent - width
  end
  return math.max(indent, 0)
end

return M
