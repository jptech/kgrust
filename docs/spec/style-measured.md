# House style, measured

Independent measurement over the 468 `.k` files of `library/`, `test/` and `doc/`
(118,876 lines; the non-UTF-8 fixture `test/syntax/imports/binary.k` excluded).
These are the numbers behind `kg-config`'s `[fmt]` defaults. Every figure below
is reproducible with the command shown.

## Indentation — 4 spaces, no tabs

```
$ grep -l $'\t' <corpus>            # files containing a tab
0
$ sed -n 's/^\( \{1,\}\)[^ ].*$/\1/p' <corpus> | awk '{print length($0)}' | sort -n | uniq -c | sort -rn
37079  4
21173  8
 8847 12
 3035 16
 1111 20
 1066  2
  438 24
  386  6
```

Clean multiples of four dominate; the counts at 2 and 6 are continuation lines
inside a wrapped expression, not a competing indent unit.

**Default: `indent = 4`, `use-tabs = false`.**

## Braces — Allman, by 65:1

```
$ grep -c '^ *{ *$'        <corpus>   # a line that is only an opening brace
13373
$ grep -c '[^ {].*{ *$'    <corpus>   # code followed by a trailing brace
207
```

**Default: opening brace on its own line, for every construct.**

## Multi-line lists — leading commas

The raw line counts favour trailing commas (5,729 to 2,023), but that is an
artefact of single-line lists and initializer lists. Counted by *file*, leading
commas are the more common convention (158 files to 108), and the context
histogram shows exactly where they are used:

```
$ grep -B1 '^ *,' <corpus> | grep -v '^ *,' | awk '{print $1}' | sort | uniq -c | sort -rn
156  {      <- module export lists, initializer lists
156  (      <- function parameter lists
 99  <      <- template parameter lists
```

The canonical shape, from `library/data/fifo.k:1-6`:

```kanagawa
module data.fifo
    { FIFO
    , FIFO_store_and_forward
    , fifo_nb
    }
```

**Default: when a delimited list breaks, the opening delimiter goes on the
previous line, each element begins a line, separators lead, and the closing
delimiter sits on its own line aligned with the separators.**

## Line width

```
$ awk '{print length($0)}' <corpus> | sort -n   # n = 118876
p50=14  p90=56  p95=73  p99=105  max=747
over 80: 3549   over 100: 1418   over 120: 666
```

Kanagawa lines are short: half are under 15 characters. Only 1.2% exceed 100.

**Default: `line-width = 100`** — wide enough that the formatter almost never
has to break code the authors wrote on one line, narrow enough to be a real
constraint on the tail.

## Attributes

```
$ grep -c '^ *\[\[[^]]*\]\] *$'   <corpus>   # attribute alone on a line
173
$ grep -c '^ *\[\[[^]]*\]\] *[^ ]' <corpus>  # attribute followed by code
656
```

Attributes are usually written inline, ahead of the construct they modify, 4:1.
This contradicts the assumption that `[[attr]]` belongs on its own line above a
declaration — the corpus says otherwise.

**Default: keep an attribute group on the same line as what follows, and break
only when the result would exceed `line-width`.**

## What the formatter must preserve rather than normalize

* **Blank-line runs** between declarations — authors group related declarations
  deliberately and a formatter that collapses runs destroys that grouping.
  Collapse two or more blank lines to at most one *inside* a body, preserve one
  blank line between top-level declarations.
* **Import order.** Import order is semantically load-bearing (a later import can
  shadow an earlier one), so `sort-imports` defaults to `false`.
* **Whether a short body is written on one line.** Both `do ; while (x);` and a
  braced multi-line body occur; the author's choice carries intent.
