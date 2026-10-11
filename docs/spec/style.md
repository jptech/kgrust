# Kanagawa House Style — empirically measured, for `kg fmt`

Status: implementation spec for the Rust rewrite (`kgrust`), crate `kg-fmt`.
Reference tree: `third_party/kanagawa` (read-only, never modified).

Every rule below is **measured**, not asserted. Each rule carries the shell command that
produced its evidence and that command's verbatim output. Claims about the reference
compiler carry `path:line` citations. Claims that could not be grounded are marked
**UNVERIFIED**.

Primary sources:

- The corpus: 480 `.k` files under `library/`, `test/`, `doc/`, `tools/`.
- `third_party/kanagawa/compiler/hs/lib/Language/Kanagawa/PrettyPrint.hs` (232 lines) — the existing pretty-printer (§13).
- `third_party/kanagawa/compiler/hs/app/Main.hs` (Main.hs:45-55) — its driver.
- `third_party/kanagawa/compiler/hs/app/Options/CmdArgs.hs:145-153` — its defaults.
- `third_party/kanagawa/compiler/hs/lib/Language/Kanagawa/Parser/Lexer.hs:164-219` — the reserved-word list, used to name constructs.
- `third_party/kanagawa/compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:85-200` — the authoritative operator/attribute spellings.

**There is no pre-existing style guide.** The repository has no `.editorconfig`, no
`.clang-format` for `.k`, and no `CONTRIBUTING` file; `.gitattributes` contains one line
(`*.k linguist-language=C++`). Verified:

```
$ cd third_party/kanagawa && cat .gitattributes && \
  find . -maxdepth 3 \( -name '.editorconfig' -o -name '.clang-format' -o -name 'CONTRIBUTING*' \) | grep -v build
*.k linguist-language=C++
```

(no output from `find` — the only `.clang-format` in the tree is `./thirdparty/circt/.clang-format`,
which governs vendored C++, not Kanagawa.)

This document is therefore the first written statement of the house style, and it is derived
from the code rather than from anyone's opinion.

---

## 0. Corpus, method, and the two traps in it

### 0.1 Corpus definition

```
$ cd third_party/kanagawa
$ for d in library test doc; do printf "%s: " "$d"; find "$d" -name '*.k' | wc -l; done
library:      124
test:      345
doc:        0
$ find library test doc tools -name '*.k' | sort > /tmp/corpus480.txt && wc -l < /tmp/corpus480.txt
     480
```

`doc/` contains **zero** `.k` files (its content is Markdown). The 480 figure is
`library/` (124) + `test/` (345) + `tools/` (11). The 11 files under `tools/` are
`tools/chkdoc/contexts/*.k` (2) and `tools/sandcastle/test/sample/**.k` (9); they are
documentation-tool fixtures and are the only files in the corpus that do **not** carry the
MIT header (§8.2). All commands in this document use the file list built above, referred to as
`/tmp/corpus480.txt`. Commands whose prompt shows a bare relative path (`--include='*.k' .`)
are run from inside the *stripped* copy of the corpus (§0.2); Appendix A gives the full setup.

```
$ cat $(cat /tmp/corpus480.txt) | wc -l
  119696
$ LC_ALL=C awk 'END{print NR}' $(cat /tmp/corpus480.txt)
119782
```

The 86-line discrepancy is exactly the number of files that lack a final newline (§1.6).

### 0.2 Method

Rules that need to distinguish code from comments and string literals were measured over a
*stripped* copy of the corpus: every `//…`, `/*…*/` and `"…"` body replaced by spaces,
byte offsets and line numbers preserved. The stripper is 25 lines of Python; it is the same
tool used for every "(stripped corpus)" measurement below. Measurements labelled
"(raw corpus)" run against the original bytes.

### 0.3 Trap 1 — 69 files in `test/syntax/` are *not* translation units

```
$ grep -lE '^expected:[0-9-]+$' $(cat /tmp/corpus480.txt) | tee /tmp/harness.txt | wc -l
      69
$ sed 's|/[^/]*$||' /tmp/harness.txt | sort | uniq -c
  69 test/syntax
$ grep -rhcE '^expected:' $(cat /tmp/corpus480.txt) | awk '{s+=$1}END{print s}'
2840
$ grep -rhoE '^[a-z_]+:[^ ]*' $(cat /tmp/harness.txt) | sed -E 's/:[0-9-]+$/:N/' | sort | uniq -c | sort -rn | head -8
2840 expected:N
  34 public:
  10 warning:redundant-import
  10 warning:N
   7 warning:
   3 warning:initializer-overrides
   1 warning:conversion
```

These files are **multi-case test scripts**: each case is a Kanagawa program indented by
4 (or 2) spaces, terminated by a column-0 `expected:N` or `warning:…` directive. Example
(`test/syntax/imports.k:1-20`):

```
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

    inline void main()
    {
        static a1<10> x;
    }

expected:1

    import missing

expected:1
```

Consequences for `kg fmt`:

1. **They are not parseable as a whole.** Running the formatter on them is a category error.
   `kg fmt` must either detect the harness marker and refuse, or the build must exclude
   `test/syntax/*.k` from any format-check gate.
2. They are the sole source of the "base indent 2 or 4" outliers (§1.3) and of the
   275 indented `import` statements (§10.4).
3. All headline numbers in this document are given for the full 480 and, where the harness
   files distort the answer, for the **411-file plain corpus** (`comm -23 corpus480 harness`).

### 0.4 Trap 2 — `test/syntax/imports/binary.k` is not text

```
$ od -c test/syntax/imports/binary.k
0000000  300 336    ޭ  **
0000004
```

Four bytes (`c0 de ba ad`), no newline, not UTF-8. It is a fixture for the "imported file is
not source" error path. `kg fmt` must not choke on it; every command in this document that
sorts or `tr`s file content needs `LC_ALL=C` because of this one file.

---

## 1. Indentation

### 1.1 Unit: 4 spaces

```
$ cat $(cat /tmp/corpus480.txt) | grep -P '^ +\S' | awk '{match($0,/^ */); n=RLENGTH; tot++; m[n%4]++} \
    END {print "total indented lines:", tot; for(i=0;i<4;i++) printf "indent %% 4 == %d : %d (%.2f%%)\n", i, m[i], 100*m[i]/tot}'
total indented lines: 75075
indent % 4 == 0 : 72401 (96.44%)
indent % 4 == 1 : 128 (0.17%)
indent % 4 == 2 : 2152 (2.87%)
indent % 4 == 3 : 394 (0.52%)
```

On the 411-file plain corpus (harness fixtures excluded) the signal is stronger:

```
$ LC_ALL=C awk '/^ +[^ ]/{match($0,/^ */); n=RLENGTH; t++; m[n%4]++} \
    END{print "total:",t; for(i=0;i<4;i++) printf "  %%4==%d : %d (%.2f%%)\n", i, m[i], 100*m[i]/t}' $(cat /tmp/plain411.txt)
total: 51780
  %4==0 : 50592 (97.71%)
  %4==1 : 120 (0.23%)
  %4==2 : 756 (1.46%)
  %4==3 : 312 (0.60%)
```

Per-file minimum positive indent (a robust estimate of each file's indent unit):

```
$ for f in $(cat /tmp/corpus480.txt); do \
    m=$(grep -P '^ +\S' "$f" | awk '{match($0,/^ */); print RLENGTH}' | sort -n | head -1); echo "${m:-none}"; done \
  | sort -n | uniq -c | sort -k2 -n
  26 none
   2 1
  30 2
   9 3
 413 4
```

413 of the 454 files that indent at all use 4. Of the 30 files whose minimum is 2, 22 are
`test/syntax/helper/*.k` fixtures (§1.3).

Direct measurement of the *step*, i.e. the indent of a body line minus the indent of the line
that opened its brace (stripped corpus):

```
  body-line indent - opener indent = +4            44974 (87.17%)
  body-line indent - opener indent = +0             3595 ( 6.97%)
  body-line indent - opener indent = +8             1814 ( 3.52%)
  body-line indent - opener indent = +2              272 ( 0.53%)
```

The `+0` bucket is dominated by `public:`/`private:` labels (§2.5), by the `{ First` /
`, Next` continuation lines of leading-comma lists (§3), and by hanging comments; it is not
a competing indent style.

**Rule.** `indent_width = 4`, `indent_style = "space"`.

### 1.2 Tabs: 4 files, and only one of them indents with a tab

```
$ grep -lP '\t' $(cat /tmp/corpus480.txt)
library/numeric/complex.k
library/numeric/float64.k
test/library/processor/risc-v.k
test/logic/test_cases_30.k
$ grep -cP '^\t' $(cat /tmp/corpus480.txt) | awk -F: '$2>0'
library/numeric/complex.k:2
```

The complete inventory of tab uses in the entire 480-file corpus:

| file | lines | what the tab is |
|---|---|---|
| `library/numeric/complex.k:12-13` | 2 | **leading indentation** of two struct members — the only tab indentation in the corpus |
| `library/numeric/float64.k:188-192` | 5 | inside `//` comment text, used to lay out a numbered list |
| `test/library/processor/risc-v.k:173-224` | 52 | inside string literals (`"lui\ta4, 0xfffff"` — RISC-V disassembly expectations) |
| `test/logic/test_cases_30.k:221` | 1 | separator before a trailing `//` comment |

**Rule.** Tabs never appear in Kanagawa indentation except for one two-line accident.
`kg fmt` converts leading tabs to `indent_width` spaces. Tabs **inside string literals must
be preserved byte-for-byte** — `test/library/processor/risc-v.k` compares disassembler output
against tab-containing strings, so rewriting them changes test semantics.

### 1.3 The 2-space files

```
$ grep -lP '^  \S' $(cat /tmp/corpus480.txt) | wc -l
      31
$ grep -hP '^  \S' $(cat /tmp/corpus480.txt) | sort | uniq -c | sort -rn | head -8
 259   }
 242   {
  99   inline void main()
  68   class Foo
  62   private:
  16   public:
  14   class Main
  11   inline void foo()
```

31 files. The content shows what they are: harness fixtures (§0.3) whose *embedded* program
is indented by 2 instead of 4, plus `test/syntax/helper/*.k` and `library/test/unit.k`.
Example, `test/syntax/helper/callbacks.k:4-20`, whose module export list is at indent 2:

```
module helper.callbacks
  { void_F_void
  , void_F_bool
  ...
  }
```

**Rule.** 2-space indent is not a competing house style; it is confined to test scaffolding.
Normalize to 4.

### 1.4 Continuation / hanging indent

Genuine wrapped expressions are vanishingly rare. Measuring line breaks at a binary operator
over the stripped corpus:

```
$ grep -rhcP '(\&\&|\|\||[+\-*/%^]|==|!=|<=|>=|<<|>>|\&|\|)\s*$' --include='*.k' . | awk '{s+=$1}END{print s}'
28
$ grep -rhcP '^\s*(\&\&|\|\||[+*/%^]|==|!=|<=|>=|<<|>>|\&|\|)\s' --include='*.h' . | awk '{s+=$1}END{print s}'
14
```

28 lines end with an operator; 14 begin with one. Out of 119 782 lines. **There is no
established convention for breaking an expression at an operator.** Kanagawa authors break
*lists* (§3), not expressions.

**Rule.** `kg fmt` must not invent an operator-wrapping style. If a statement exceeds
`line_width` and contains no breakable list, leave it alone.

### 1.5 Trailing whitespace

```
$ grep -lP '[ \t]+$' $(cat /tmp/corpus480.txt) | wc -l
      60
$ grep -hcP '[ \t]+$' $(cat /tmp/corpus480.txt) | awk '{s+=$1}END{print s}'
228
$ grep -cP '[ \t]+$' $(cat /tmp/corpus480.txt) | grep -v ':0$' | sort -t: -k2 -rn | head -5
test/syntax/atomic.k:28
test/syntax/member-function-template.k:23
test/syntax/function-constants.k:16
test/syntax/extern-class.k:15
library/numeric/int/multi_word.k:12
```

228 lines in 60 files (0.19% of lines). Accidental. **Rule:** strip.

### 1.6 Final newline and line endings

```
$ n=0; for f in $(cat /tmp/corpus480.txt); do \
    last=$(tail -c1 "$f" | od -An -tx1 | tr -d ' '); [ "$last" != "0a" ] && n=$((n+1)); done; echo $n
86
$ grep -lU $'\r' $(cat /tmp/corpus480.txt) | wc -l
       0
```

86 of 480 files (17.9%) end without a newline, including `library/base.k` (whose last line,
`static assert (version <= device_schema::schema_version);`, is also followed by two trailing
spaces). Zero files use CRLF.

**Rule.** `final_newline = true`, `newline = "lf"`. Adding the missing newline is a real
diff on 86 files; it is still correct — a missing final newline is never intentional and every
one of the 86 is a paste artifact.

---

## 2. Brace style

Kanagawa has **no `namespace` keyword** — the reserved-word list is
`Lexer.hs:164-219` and contains `module` but not `namespace`. The module-scope construct is
`module a.b { exports }` (§3.1), which is a *list*, not a block. So the constructs that take
a brace-delimited block are: `class`, `struct`, `union`, `enum`, function bodies, lambda
bodies, `if`/`else`/`static if`, `for`/`static for`/`unrolled_for`, `while`, `do`, `switch`,
bare nested scopes, `atomic` blocks, and initializer lists.

### 2.1 Headline: 98.7% Allman

Raw counts first (raw corpus, so comment lines are included and inflate both sides slightly):

```
$ grep -hcE '^[[:space:]]*\{[[:space:]]*$' $(cat /tmp/corpus480.txt) | awk '{s+=$1}END{print s}'
13421
$ grep -hcE '[^[:space:]{].*\{[[:space:]]*$' $(cat /tmp/corpus480.txt) | awk '{s+=$1}END{print s}'
209
$ grep -hcE '^[[:space:]]*\}[[:space:]]*;?[[:space:]]*$' $(cat /tmp/corpus480.txt) | awk '{s+=$1}END{print s}'
12865
```

### 2.2 Per-construct classification

Measured on the stripped corpus. A `{` is *Allman* if the line containing it holds nothing
else; *K&R* if the line ends with `{` after other code. The construct is identified from the
header (the same line for K&R, the preceding non-blank line for Allman).

```
construct                      Allman    K&R  %Allman   first example
atomic                            401      8    98.0%   library/codec/crc.k:72
class                            1752      1    99.9%   library/codec/crc.k:32
do                                 52      0   100.0%   library/control/loop.k:47
else                              307      0   100.0%   library/control/loop.k:194
enum                              106      0   100.0%   library/compiler/device/schema.k:14
for                               254      0   100.0%   library/data/random/xoshiro.k:30
function                         6817      2   100.0%   library/base/system.k:10
if                               1095      1    99.9%   library/codec/crc.k:54
initializer-list (= {)             50    137    26.7%   library/data/datetime.k:42
lambda                            972     23    97.7%   library/control/async.k:156
module                             12      0   100.0%   library/data/vector.k:13
other (nested/attributed)         371      1    99.7%   library/control/loop.k:437
static for                        468      0   100.0%   library/codec/crc.k:56
static if                         180      0   100.0%   library/control/loop.k:187
struct                            421      0   100.0%   library/compiler/device/schema.k:27
switch                             78      0   100.0%   library/processor/risc_v/internal/decoder.k:268
union                              39      0   100.0%   library/data/buffer/gearbox.k:25
TOTAL                           13375    173    98.7%
```

Reading of this table:

- **Everything that is a *block* is Allman.** `class`, `struct`, `union`, `enum`, function
  bodies, lambda bodies, `if`, `else`, `for`, `static for`, `static if`, `while`, `do`,
  `switch`, `atomic` and bare nested scopes are ≥97.7% Allman, and most are 100%.
- `else` is **never** written `} else` — the closing brace of the `if` arm sits alone and
  `else` starts the next line:
  ```
  $ grep -rhcP '^\s*\}\s*else' --include='*.k' . | awk '{s+=$1}END{print s}'
  0
  $ grep -rhcP '^\s*else\s*$' --include='*.k' . | awk '{s+=$1}END{print s}'
  329
  $ grep -rhoP '\belse if\b' --include='*.k' . | wc -l
  129
  ```
  `else if` chains stay on one line (129 occurrences), which is not a brace decision.
- **Initializer lists are the exception and go K&R** (`T x = {` at end of line, 137 vs 50).
  This is a different construct — a value, not a block — and must have its own knob.
- The **1 K&R function definition** in the whole corpus is
  `test/library/control/async/rendezvous/lenient.k:37`:
  `    void run(unit::tag_t tag)    {` — note the four stray spaces before the brace. It is
  an obvious slip; the same file uses Allman everywhere else.
- The **23 K&R lambdas** are all of the shape
  `pipelined_for(16, [tag](uint8 itr){` and are concentrated in
  `test/library/numeric/int/multi_word.k` (`:59`, `:69`, `:79`, …). Everywhere else lambdas
  are Allman — e.g. `library/control/async.k:155-159`:
  ```
          async_then<T>(task,
              [](T value)
              {
                  _returnFifo.enqueue(value);
  ```

### 2.3 Collapsed (one-line) bodies

```
$ grep -rhoP '\)\s*\{\s*\}' --include='*.k' . | wc -l          # `foo() {}`
79
$ grep -rhoP '\)\s*\{[^{}]+\}' --include='*.k' . | wc -l       # `foo() { stmt; }`
92
$ grep -rhcP '^\s*\{\s*\}\s*$' --include='*.k' . | awk '{s+=$1}END{print s}'   # `{}` alone on its own line
10
```

171 collapsed bodies against 13 375 expanded ones (1.3%). They cluster in test files where a
stub is the point (`test/logic/test_cases_41.k:414`) and in
`library/control/async/rendezvous/lenient.k:120`.

**Rule.** Do not collapse and do not expand. `collapse_empty_body = "preserve"`,
`collapse_short_body = "preserve"`. Both directions of normalization would produce large,
semantically empty diffs; neither has majority support strong enough to justify that.

### 2.4 Semicolon after a type body

```
  class:  closing brace with NO ";"   1754     closing brace followed by ";"    11
  struct: closing brace with NO ";"    426     closing brace followed by ";"    28
  union:  closing brace with NO ";"     35     closing brace followed by ";"     6
  enum:   closing brace with NO ";"    108     closing brace followed by ";"     3
```

First `};` examples: `library/control/async.k:201`, `library/data/buffer/accumulating.k:99`,
`library/data/buffer/gearbox.k` (union), `test/syntax/extern-class.k:394` (enum).

The grammar does not require the semicolon — `class'` is
`Class scope ident <$> braces (many memberDecl)` (`Parser.hs:331-339`) with no trailing
`semi`, and the reference pretty-printer emits none (`PrettyPrint.hs:212`, `225`, `228`,
`214` map `Class`/`Struct`/`Union`/`Enum` to `mempty` in `term`).

**Rule.** `type_body_semicolon = "remove"` (97.6% of the corpus already omits it).

### 2.5 `public:` / `private:` are **not** indented

The labels sit at the same column as the `class` keyword and the opening brace, i.e. one
level *out* from the members:

```
  label indent - enclosing `{` indent = +0       2187
  label indent - enclosing `{` indent = +4         67   test/library/control/loop.k:200
  label indent - enclosing `{` indent = +2          3   test/interface/export_class_no_instances.k:9
  label indent - enclosing `{` indent = +1          2   test/syntax/atomic.k:65
```

97.0% at +0. Canonical shape, `library/data/fifo.k:29-36`:

```
class FIFO
{
public:
    //| `Size` must be a power of 2.
    static assert(0 == (Size & (Size - 1)));

private:
    using pointer_t = index_t<Size>;
```

**Rule.** `access_label_indent = 0` (relative to the enclosing `class` keyword).

### 2.6 `switch` / `case`

```
  case/default indent - switch `{` indent = +4    251   library/processor/risc_v/internal/decoder.k:270
  case/default indent - switch `{` indent = +0    123   test/logic/conditional_update.k:184
  case label alone on its line                    177
  case label + statement on same line             197
```

67% indent `case` by one level inside the `switch` block; the case *body* then sits at +8.
Canonical, `library/processor/risc_v/internal/decoder.k:268-279`:

```
        switch (funct3)
        {
            case Branch::BLT:
                decoded.instr.control.condition = Condition::LT;
                break;
            default:
                decoded.instr.control.condition = static_cast(funct3);
                break;
        }
```

`break;` is not optional: the grammar mandates it on every arm
(`Parser.hs:602-603` — `SwitchCase … <* keyword "break" <* semi`).

**Rule.** `case_indent = 4` (relative to the `switch` brace); case bodies at +8. The 197
one-line `case X: stmt;` forms are all in `test/logic/*` sweeps and are covered by
`collapse_short_body = "preserve"`.

---

## 3. Multi-line lists: leading comma is the house style

This is the single most distinctive thing about Kanagawa source, and the single biggest
divergence from the reference pretty-printer (§13.4).

### 3.1 Module export lists

The header of every library file looks like `library/data/fifo.k:1-16`:

```
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

module data.fifo
    { FIFO
    , FIFO_store_and_forward
    , fifo_nb
    }

import data.counter
import data.memory
```

Measured over all 208 multi-line export lists:

```
--- module export list  (total multi-line groups: 208)
      leading comma                             129  library/base.k:5
      no-comma (single item)                     74
      mixed                                       4  library/data/array.k:8
      trailing comma                              1  library/numeric/int/multi_word.k:6
```

Excluding single-item lists, **129 leading vs 1 trailing = 99.2%**.

Geometry:

```
  export-list `{` indent - `module` indent = +4               186
  export-list `{` indent - `module` indent = +2                18
  export-list `{` indent - `module` indent = +0                 2
  export-list `}` alone on its line                           205  (of 205)
  export-list `}` indent - `{` indent = +0                    205
  export-list leading `,` indent - `{` indent = +0            843
  export-list leading `,` indent - `{` indent = +1              1
```

```
$ for f in $(cat /tmp/corpus480.txt); do LC_ALL=C awk '/^module /{getline nx; if (nx ~ /\{/) \
    { if (nx ~ /^[ ]*\{[ ]*$/) print "brace-alone"; else print "brace+first-item"}}' "$f"; done | sort | uniq -c
   5 brace-alone
 201 brace+first-item
```

**Rule.** `{` on the line *after* `module`, indented +4, **hugging the first export**
(`{ FIFO`, 97.6%); one export per line thereafter, each prefixed `, `; `}` alone on its own
line, aligned with `{`.

The single trailing-comma outlier is `library/numeric/int/multi_word.k:6-11`:

```
module numeric.int.multi_word
    { add_with_carry,
      sub_with_borrow,
      min_with_metadata,
      max_with_metadata,
      get_initial_comp_metadata
    }
```

The four "mixed" cases are `library/data/array.k:8-15` and friends, which embed `//|` doc
comments **inside** the export list:

```
module data.array
    { array
    , //| # Constructing arrays
      indices
    , copy_array
```

This shape must survive formatting — see §12, example 9.

Note that `doc/programming-guide.md:4372-4382` shows a *different* export-list style
(`{` on its own line, 4-space body, trailing commas). That is prose written for the guide;
the 237 real module declarations in the corpus do not follow it. The corpus wins.

### 3.2 Template parameter lists

```
--- template parameter list (multi-line)  (total 121)
      leading comma                          98  library/codec/crc.k:23
      single-item / no comma                 19  library/control/async/rendezvous/lenient.k:65
      trailing comma                          3  test/library/processor/risc-v/risc_v_wrapper.k:20
      mixed                                   1  library/processor/risc_v/internal/core.k:73
--- template parameter list (single-line)  (total 1330)
      one line                             1330
--- template `<` opener placement  (total 121)
      `<` starts its own line               116
      same line as `template`                 5
--- template `>` closer placement  (total 121)
      `>` starts its own line               115
      trails last param                       6
```

Leading vs trailing among multi-item lists: **98 vs 3 = 97.0%**.

Whether `template` gets its own line:

```
  `template <...` on one line                      1295
  `template` alone on its line                      122   library/codec/crc.k:22
    `<` indent - `template` indent = +4             117
    `<` indent - `template` indent = +0               3
    `<` indent - `template` indent = +8               2
    `>` indent - `template` indent = +4             111
    leading `,` indent - `<` indent = +0            439   (of 439)
```

**Rule.** If the template parameter list fits on one line, write `template <T, U>` (1295
cases). If it must break, put `template` alone on its line, `<` on the next line at +4, each
subsequent parameter on its own line prefixed `, ` aligned with `<`, and `>` alone at the same
column as `<`.

Canonical, `library/codec/crc.k:22-31`:

```
template
    < auto Width            //< Width (in bits) of the computed CRC.
    , uint<Width> P         //< The reversed polynomial.
    , uint<Width> Init      //< The initial value of CRC.
    , uint<Width> XorOut    //< The value to XOR with the final result.
    , bool ReflectInput     //< Whether to reverse the bits of each input byte.
    , bool ReflectOutput    //< Whether to reverse the bits of the CRC output result.
    , auto MaxBytes         //< The maximum number of bytes that can be processed per call.
    , auto ThreadCount      //< Number of CRC values that can be computed concurrently.
    >
class crc_calculator
```

### 3.3 Function parameter lists

```
--- function definition parameter list (multi-line)  (total 125)
      leading comma                          89  library/codec/crc.k:40
      single-item / no comma                 29  library/control/async/rendezvous/lenient.k:54
      trailing comma                          4  test/library/data/datetime.k:292
      mixed                                   3  library/data/buffer/accumulating.k:203
--- function definition parameter list (single-line)  (total 8803)
      one line                             8803
--- lambda parameter list (single-line)  (total 914)
      one line                              914
```

Leading vs trailing among multi-item lists: **89 vs 4 = 95.7%**. 98.6% of all parameter lists
fit on one line, and **every** lambda parameter list in the corpus is single-line.

Geometry of the 125 that break:

```
  `(` starts its own line                             113
  `(` stays on the header line                         12
  `(`-line indent - decl-line indent = +4             112   library/codec/crc.k:40
  `(`-line indent - decl-line indent = +0               1
  `)` on its own line                                 111
  `)` shares a line                                     2
```

Canonical, `library/codec/crc.k:39-44`:

```
    inline uint<Width> compute
        ( count_t<ThreadCount> identifier   //< Which of the `ThreadCount` CRCs to update.
        , uint8[MaxBytes] input             //< Input data.
        , count_t<MaxBytes> input_bytes     //< Number of valid bytes in the input data.
        , bool reset                        //< Indicates if CRC value should be reset before processing `input`.  Should be set to `true` on the first call associated with a particular input stream.
        )
    {
```

**Rule.** Same shape as template lists: the opener moves to its own line at +4 and the
commas lead.

### 3.4 Initializer lists — trailing comma

```
--- initializer list  (total 248 multi-line)
      trailing comma                            188  library/data/datetime.k:43
      leading comma                              46  library/data/closure.k:70
      no-comma                                   12
      mixed                                       2  test/syntax/callbacks.k:245
--- nested initializer list  (total 81)
      trailing comma                             44  library/device/agilex-7-f-023/hardware/config.k:33
      no-comma                                   19
      leading comma                              17  test/syntax/callbacks.k:1118
      mixed                                       1  test/syntax/callbacks.k:245
```

Trailing vs leading: **188 vs 46 = 80.3%** (top-level), **44 vs 17 = 72.1%** (nested).
Combined with §2.2 (initializer lists open K&R, `= {` at end of line), initializers are the
one construct that reads like C rather than like Haskell.

Interior padding of *single-line* initializers is genuinely split:

```
$ grep -rhoP '=\s*\{ [^{}]*\}' --include='*.k' . | wc -l          # `= { a, b }`
138
$ grep -rhoP '=\s*\{[^{}\s][^{}]*\}' --include='*.k' . | wc -l    # `= {a, b}`
266
$ grep -rhoP '=\s*\{[^{}]*\}' --include='*.k' . | sed -E 's/[A-Za-z_][A-Za-z0-9_]*/ID/g; s/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -6
 174 = {}
  69 = {.ID = ID}
  18 = { N, N, N, N, N, N, N, N }
  17 = {ID, ID}
  16 = {ID, N}
  15 = { N, N, N, N }
```

34% padded / 66% unpadded, and in `library/` alone it is 2 padded vs 16 unpadded.
Nested rows are written `{{` not `{ {` (1387 vs 27).

**Rule.** `list_style_initializers = "trailing_comma"`; `space_inside_braces_initializer =
"preserve"` (the split is too even, and the padded cases are hand-aligned numeric tables
such as `library/numeric/float32/internal.k:877-888` where padding is load-bearing for
column alignment).

### 3.5 Enum bodies — trailing comma

```
--- enum body  (total 107 multi-line)
      trailing comma                             58  library/compiler/device/schema.k:15
      no-comma                                   46
      mixed                                       2  test/syntax/doc-comments.k:76
      leading comma                               1  library/processor/risc_v/isa.k:186
```

Trailing vs leading: **58 vs 1**.

```
  one constant per line                      58
  enum body on ONE line                      49
  multiple constants share a line             3
```

Canonical, `library/compiler/device/schema.k:13-24`:

```
enum MemoryType : uint3
{
    //| LUTram / MLAB RAM
    LUT = 0,

    //| Block RAMs, for example M20K on Intel FPGA
    Block = 1,

    //| Deep RAMs, for example URAM on a Xilinx FPGA
    Deep = 2
}
```

The single leading-comma enum, `library/processor/risc_v/isa.k:185-192`, is also the corpus's
only hand-aligned `=` table:

```
enum ABI : Reg
{ zero = 0   //< Hard-wired zero
, ra   = 1   //< Return address
, sp   = 2   //< Stack pointer
```

See §10.3 — this must be preserved, not reflowed.

### 3.6 Base lists: **there are none**

Kanagawa `class` has no inheritance clause. The parser production is
`Parser.hs:331-339`:

```haskell
class' symbols params = (do
    scope <- getScopeQualifier
    ident <- keyword "class" *> identifier
    ...
                Class scope ident <$> if skipBody
                    then Seq [] <$ L.skipBraces
                    else braces $ many memberDecl) <?> "class"
```

— identifier, then immediately the brace body. No `:`, no base list. Confirmed in the corpus:

```
$ grep -hcE '^[[:space:]]*class[[:space:]]+[A-Za-z_][A-Za-z0-9_<>, ]*:' $(cat /tmp/corpus480.txt) | awk '{s+=$1}END{print s}'
0
$ grep -hoE '^[[:space:]]*(export[[:space:]]+)?class[[:space:]]+[A-Za-z_][A-Za-z0-9_]*.*$' $(cat /tmp/corpus480.txt) \
    | sed 's/^[[:space:]]*//' | sed -E 's/[A-Za-z_][A-Za-z0-9_]*/ID/g' | sort | uniq -c | sort -rn
1756 ID ID
   8 ID ID{}
   1 ID ID{} /*<
   1 ID ID {
```

Every one of the 1766 class headers is exactly `class <name>`.

`enum E : uint8` *does* carry an underlying type after a colon; it is always on the same line
as the `enum` keyword (§6.7). The file `test/syntax/helper/custom_base.k` is about the
`mini-base` *module*, not class inheritance.

**Rule.** `kg fmt` needs no base-list handling. If the Rust parser grows one, that is a
language change, not a formatting question.

### 3.7 Summary table

| list | style | evidence | ratio |
|---|---|---|---|
| module export list | **leading** `,` | §3.1 | 129 : 1 |
| template parameter list | **leading** `,` | §3.2 | 98 : 3 |
| function parameter list | **leading** `,` | §3.3 | 89 : 4 |
| lambda parameter list | never breaks | §3.3 | 914 : 0 |
| initializer list | **trailing** `,` | §3.4 | 188 : 46 |
| nested initializer list | **trailing** `,` | §3.4 | 44 : 17 |
| enum body | **trailing** `,` | §3.5 | 58 : 1 |
| class / struct / union body | no commas (`;`-terminated members) | §3.4 measurement showed 421/421 struct and 39/39 union bodies comma-free | — |
| base list | **does not exist** | §3.6 | — |

---

## 4. `[[attribute]]` placement

The complete attribute vocabulary is `Syntax.hs:124-145` (`Attribute`, 9 argument-taking
forms) and `Syntax.hs:150-179` (`Flag`, 13 bare forms):

`call_rate`, `ecc`, `fifo_depth`, `latency`, `max_threads`, `name`, `schedule`,
`thread_rate`, `transaction_size` — take a parenthesized argument;
`async`, `atomic`, `last`, `memory`, `initialize`, `no_backpressure`, `pure`,
`non_replicated`, `pipelined`, `quad_port`, `reorder_by_looping`, `reset`, `unordered` —
bare flags.

`[[a]] [[b]]` is exactly equivalent to `[[a, b]]`: the separator production is
`commaOrBrackets = comma <|> try (L.symbol "]]" >> L.symbol "[[")` (`Parser.hs:165`).

### 4.1 Line-level counts

```
lines with an attribute specifier: 932
  same line, attribute first, decl follows       656  ( 70.4%)  library/control/async.k:40
  own line, decl on NEXT line                    174  ( 18.7%)  library/compiler/hal.k:12
  mid-line (parameter / nested position)         102  ( 10.9%)  library/control/async.k:189
```

### 4.2 Per-attribute counts (occurrence-level, so `[[a, b]]` counts twice)

```
attribute            own-line same-line mid-line   first own-line example
call_rate                   0        11        0
ecc                         0         4        7
fifo_depth                  3        12        3   test/syntax/loops.k:130
latency                    15        27        0   library/compiler/hal.k:12
max_threads                38        46       12   library/test/runner.k:24
name                       33         0        1   library/compiler/hal.k:16
schedule                   53         0        0   library/data/hash_table.k:412
thread_rate                 4        11        7   test/syntax/callbacks.k:717
transaction_size            0        25        8
async                      15       117        0   test/interface/another_exported_class.k:21
last                        0         2       41
memory                      0       131       40
initialize                  0         0        4
no_backpressure             6        17        0   test/interface/another_exported_class.k:21
pure                        0         3        0
non_replicated              0         3        2
pipelined                   0       228        0
quad_port                   0         7        1
reorder_by_looping          6         0        0   library/control/wait.k:103
reset                       8         8        0   test/syntax/function-attributes.k:138
unordered                   5       101        0   library/processor/risc_v.k:206
TOTAL                     186       753      126   (17.5% own-line, 70.7% same-line, 11.8% mid-line)
```

This is **not** a uniform convention. It splits by attribute:

- **Always own-line:** `schedule` (53/53), `name` (33/34), `reorder_by_looping` (6/6).
  These are the attributes whose argument is long — a module-mangled string or a schedule
  expression. `library/compiler/hal.k:12-17`:
  ```
  public:
      [[latency(LATENCY)]]
      float32 op(float32 x, float32 y);
  }

  [[name("_hardware_dsp__fmul32")]]
  extern fmul32;
  ```
- **Always same-line:** `pipelined` (228/228), `call_rate` (11/11), `transaction_size`
  (25 same-line, 0 own-line), `memory` (131 same-line), `unordered` (101/106),
  `async` (117/132). Example `library/control/async.k:40`:
  `        [[async]] void exec(task_t task, then_t then)`
- **Mid-line by construction:** `last` (41/43) and `initialize` (4/4) attach to a *call* or a
  *parameter*, not to a declaration, so they can only be mid-line. Example
  `library/control/async.k:189`:
  `        atomic [[fifo_depth(WaitForDepth)]] do; while(!check_and_decrement());`

**Rule.** `attribute_placement = "preserve"`. There is no majority strong enough to normalize
against, and normalizing `[[schedule(…)]]` inline or `[[pipelined]]` onto its own line would
both be wrong. Provide `attribute_own_line_when = ["name", "schedule", "reorder_by_looping"]`
as an *opt-in* normalization list; default it empty.

---

## 5. Line width

### 5.1 Distribution over all 480 files

```
total lines               : 119782
max                       : 747
p50                       : 14
p75                       : 34
p90                       : 56
p95                       : 73
p98                       : 89
p99                       : 104
p99.5                     : 126
p99.9                     : 285
lines >   60 :  10058  ( 8.397%)   fits: 91.603%
lines >   72 :   6170  ( 5.151%)   fits: 94.849%
lines >   80 :   3561  ( 2.973%)   fits: 97.027%
lines >   88 :   2527  ( 2.110%)   fits: 97.890%
lines >   90 :   2253  ( 1.881%)   fits: 98.119%
lines >  100 :   1418  ( 1.184%)   fits: 98.816%
lines >  110 :    960  ( 0.801%)   fits: 99.199%
lines >  120 :    666  ( 0.556%)   fits: 99.444%
lines >  132 :    453  ( 0.378%)   fits: 99.622%
lines >  160 :    277  ( 0.231%)   fits: 99.769%
lines >  200 :    169  ( 0.141%)   fits: 99.859%
```

10-column histogram:

```
   0-  9 :  49450
  10- 19 :  18475
  20- 29 :  16986
  30- 39 :  11123
  40- 49 :   8193
  50- 59 :   5081
  60- 69 :   3294
  70- 79 :   3453
  80- 89 :   1341
  90- 99 :    910
 100-109 :    482
 110-119 :    305
 120-129 :    211
 130-139 :     98
 140-149 :     50
 150-159 :     36
 160-169 :     38
 170-179 :      9
 180-189 :     10
 190-199 :     68
 200-209 :      1
```

### 5.2 Per-file maximum width

```
$ for f in $(cat /tmp/corpus480.txt); do LC_ALL=C awk '{if(length($0)>m)m=length($0)}END{print m+0}' "$f"; done \
  | sort -n | awk '{a[c++]=$1} END{print "files:",c; print "median file-max:",a[int(c/2)]; \
    print "p90 file-max:",a[int(0.9*c)]; print "max:",a[c-1]; \
    for(i=0;i<c;i++){if(a[i]<=80)f80++; if(a[i]<=100)f100++; if(a[i]<=120)f120++} \
    print "files entirely within 80:",f80; print "files entirely within 100:",f100; print "files entirely within 120:",f120}'
files: 480
median file-max: 80
p90 file-max: 138
max: 747
files entirely within 80: 244
files entirely within 100: 345
files entirely within 120: 403
```

The median per-file maximum is **exactly 80**. That is not a coincidence: authors were
aiming at 80 and overrunning it in half the files. On the 411-file plain corpus the median
per-file max is 83, p90 142, and 194/281/336 files fit in 80/100/120.

### 5.3 What the long lines actually are

```
$ LC_ALL=C awk 'length($0)>80{s=$0; sub(/^[ \t]+/,"",s); \
    if (s ~ /^\/\//) print "comment-only line"; \
    else if (s ~ /^\/\*/ || s ~ /^\*/) print "block-comment line"; \
    else if (s ~ /\/\/</) print "code + trailing //< doc"; \
    else if (s ~ /\/\//) print "code + trailing // comment"; \
    else print "code only"}' $(cat /tmp/corpus480.txt) | sort | uniq -c | sort -rn
2432 code only
 863 comment-only line
 225 code + trailing //< doc
  35 code + trailing // comment
   6 block-comment line
```

```
$ LC_ALL=C awk 'length($0)>100{s=$0; sub(/^[ \t]+/,"",s); \
    if (s ~ /^\/\//) print "comment-only line"; \
    else if (s ~ /^\/\*/ || s ~ /^\*/) print "block-comment line"; \
    else if (s ~ /\/\/</) print "code + trailing //< doc"; \
    else if (s ~ /\/\//) print "code + trailing // comment"; \
    else print "code only"}' $(cat /tmp/corpus480.txt) | sort | uniq -c | sort -rn
1092 code only
 204 comment-only line
 107 code + trailing //< doc
  11 code + trailing // comment
   4 block-comment line
```

The 10 longest lines in the corpus are all one file:

```
747	library/numeric/float32/internal.k:879	    {{ 552, 1131956, 2710314564 }, { 35, 70472, 1885700250 }
746	library/numeric/float32/internal.k:883	    {{ 460, 943066, 4258546107 }, { 29, 58713, 2078852149 },
746	library/numeric/float32/internal.k:882	    {{ 483, 990280, 3871523637 }, { 30, 61652, 2030568584 },
...
```

— a generated constant table. `library/numeric/fixed.k:817` (694 cols) is the same.
Excluding those two files, code-only lines over 80 drop from 2432 to 2246.

### 5.4 Decision

The reference tool's default is 80 (`Options/CmdArgs.hs:147`:
`line_width = 80 &= help "Width of the page in characters; default 80" &= name "w"`).

Recommendation: **`line_width = 100`**, not 80.

Rationale, all from the numbers above:

1. At 80, 2.97% of lines (3561) are over budget; at 100, 1.18% (1418). The marginal 2143
   lines are overwhelmingly *comment* lines (659 of them) and `code + //< doc` lines (118),
   which a formatter should not reflow anyway (§8.4) — reflowing them destroys the column
   alignment that makes them readable.
2. 244/480 files already fit in 80 and 345/480 in 100. Setting 80 would mean `kg fmt` has an
   opinion about 101 more files than setting 100 does, for no readability gain.
3. `line_width` in `kg fmt` is a *break-if-you-must* threshold, not a hard limit: the only
   things `kg fmt` breaks are lists (§3), and long `//<` doc comments and generated tables
   are not lists.

`line_width = 80` remains available for byte-parity experiments against `kanagawa pretty`.

---

## 6. Spacing

All measurements in this section run over the **stripped** corpus, so string and comment
contents cannot pollute them.

### 6.1 Binary operators — spaced

```
op           a OP b       aOPb  %spaced
+              1949         89    95.6
-               716         88    89.1
*               494         29    94.5
/               214         20    91.5
%               240         14    94.5
==             3472          3    99.9
!=              388          1    99.7
<=              131          0   100.0
>=              181          0   100.0
&&              322          0   100.0
||              301          0   100.0
&               228          0   100.0
|               125          0   100.0
^               113          0   100.0
<<              386          1    99.7
>>              336        280    54.5
=             11821         46    99.6
```

Two of these numbers are measurement artifacts and must be read carefully:

- **`>>` is not 54.5%.** The "unspaced" hits are nested template closers, not shifts:
  ```
  $ grep -rhoP '.{6}(?<=[\w\)\]])>>(?=[\w\(]).{4}' --include='*.k' . | sed -E 's/[A-Za-z_][A-Za-z0-9_]*/ID/g' | sort | uniq -c | sort -rn | head -6
    23 ID>>(ID
    20 ID<ID>>(ID
    17 <ID>>({{ID
     9 ID>>(ID);
     5 ID<ID, ID>>(ID
     4 ID<ID>>(ID);
  ```
  `Foo<Bar<T>>(x)` — two closing angles followed by a call. Real shift operators are spaced.
- **`-` is not 89.1%.** Module names may contain `-` (`dotSepName` allows it,
  `Lexer.hs:230-233`), so `mini-base`, `agilex-7`, `stratix-10`, `arria-10` and the warning
  name `redundant-import` all count as "unspaced minus":
  ```
  $ grep -rhoP '.{6}(?<=[\w\)\]])-(?=[\w\(]).{4}' --include='*.k' . | sort | uniq -c | sort -rn | head -8
    10 undant-impo
     7 tratix-10.c
     7 agilex-7.co
     5 .arria-10.c
     4 e with-dash
     3 alizer-over
     2 rial<N-1>()
     2 N & (N-1))
  ```

The genuine exception is **inside template argument lists and array subscripts**, where
authors tighten arithmetic: `Foo<N+1>`, `a[i+1]`, `bits<width-1>`.

```
$ grep -rhoP '.{6}(?<=[\w\)\]])\+(?=[\w\(]).{4}' --include='*.k' . | sed -E 's/[A-Za-z_][A-Za-z0-9_]*/ID/g' | sort | uniq -c | sort -rn | head -6
  13    ID[ID+1] ID
   8 ID<ID+1>[ID
   6 ID<ID+1> ID
   4 { 4, 5+6, 1
   4  ID<ID+1> ID
   3 ID)+1> ID
```

**Rule.** `space_around_binary_ops = true` everywhere, including inside `<…>` and `[…]`.
The 89 tight `+` and 29 tight `*` are a minority even in that niche; normalizing them is a
small, safe diff. (Complete operator vocabulary from `Syntax.hs:85-104`:
`+ - * / % & && | || ^ ^^ << >> == != > >= < <=`; `Syntax.hs:117-122` gives the unary set
`~ - ! bitsizeof bytesizeof clog2`; `lutmul` has no infix spelling — `Syntax.hs:88` maps it
to `undefined` and it is written `lutmul(a, b)`.)

### 6.2 Assignment operators

```
op         spaced   unspaced  %spaced
+=            120         30     80.0
-=             13          0    100.0
*=              2          0    100.0
/=              1          0    100.0
%=              1          0    100.0
&=              2          0    100.0
|=             12          0    100.0
^=              9          0    100.0
&&=           130          0    100.0
||=            19          0    100.0
^^=             3          0    100.0
=           11821         46     99.6
```

The complete assignment vocabulary is `Parser.hs:616-628`:
`= += -= *= /= %= &= |= ^= &&= ||= ^^=`. **There is no `<<=` or `>>=`.**

The 30 unspaced `+=` are all inside one-line `case` arms in test sweeps:

```
$ grep -rhoP '.{8}(?<=[\w\)\]])\+=(?=[\w\(]).{4}' --include='*.k' . | sort | uniq -c | sort -rn | head -4
  12 ase 0: x+=1; b
   8 fault: x+=1; b
   4 ase 2: x+=1; b
   2 fault: x+=y; b
```

**Rule.** `space_around_assign = true`.

### 6.3 Increment / decrement, unary, ternary

```
$ grep -rhoP '\w(\+\+|--)' --include='*.k' . | wc -l          # postfix
247
$ grep -rhoP '(\+\+|--)\w' --include='*.k' . | wc -l          # prefix
14
$ grep -rhoP '(?<![!<>=])!(?=[\w\(])'  --include='*.k' . | wc -l   ->  638
$ grep -rhoP '(?<![!<>=])! (?=[\w\(])' --include='*.k' . | wc -l   ->    0
$ grep -rhoP '~(?=[\w\(])'  --include='*.k' . | wc -l              ->   84
$ grep -rhoP '~ (?=[\w\(])' --include='*.k' . | wc -l              ->    0
$ grep -rhoP ' \? '               --include='*.k' . | wc -l        ->  595
$ grep -rhoP '(?<=\w)\?(?=\w)' --include='*.k' . | wc -l        ->    0
```

Unary prefix operators bind tight (100%, `!x`, `~x`, `-x`). The ternary is always
` ? … : ` (595 : 0). `++`/`--` are desugared by the parser
(`Parser.hs:607-614`: `x++` becomes `Assign x (Binary Add x 1)`) and there is no spacing
question — they are always written adjacent to the operand.

**Rule.** No space after a unary prefix operator; spaces around `?` and `:` in a ternary.

### 6.4 Commas

```
$ grep -rhoP ', ' --include='*.k' . | wc -l
42784
$ grep -rhoP ',[^ ]' --include='*.k' . | grep -v ',$' | wc -l
133
$ grep -rhoP ',[^ ]' --include='*.k' . | sort | uniq -c | sort -rn | head -6
  71 ,0
  34 ,1
  20 ,5
   1 ,t
   1 ,N
   1 ,j
```

125 of the 133 "tight commas" are `,0`, `,1`, `,5` inside the generated numeric tables in
`library/numeric/float32/internal.k`. Excluding those, tight commas are 8 in 42 792.

Space *before* a comma exists only as leading-comma continuation (`\n    , x`):

```
$ grep -rhnE '\S +,' --include='*.k' . | wc -l
     207
```

— 207 of these are the aligned enum/parameter tables where the comma is padded to a column.

**Rule.** `space_after_comma = true`, `space_before_comma = false`, with the leading-comma
list form (§3) as a separate, deliberate construct rather than a spacing exception.

### 6.5 `::`, `.`, `->`

```
$ grep -rhoP '\w::\w'    --include='*.k' . | wc -l   ->  8014
$ grep -rhoP '\w :: \w'  --include='*.k' . | wc -l   ->     0
$ grep -rhoP '\w ::\w'   --include='*.k' . | wc -l   ->     0
$ grep -rhoP '\w:: \w'   --include='*.k' . | wc -l   ->     0

$ grep -rhoP '\w\.\w'    --include='*.k' . | wc -l   ->  9680
$ grep -rhoP '\w \. \w'  --include='*.k' . | wc -l   ->     1

$ grep -rhoP ' -> '                        | wc -l   ->   949
$ grep -rhoP '(?<=[\w\)])->(?=[\w(])'      | wc -l   ->   141
$ grep -rhoP '\.\w+ = '                    | wc -l   ->  1376   # designated initializer
$ grep -rhoP '\.\w+='                      | wc -l   ->     0
```

`::` is **100%** tight (8014 : 0). `.` is 99.99% tight. `->` (function-type and lambda return
arrow) is 87.0% spaced; the tight cases are compact type expressions:

```
$ grep -rhoP '.{6}(?<=[\w\)])->(?=[\w(]).{6}' --include='*.k' . | sed -E 's/[A-Za-z_][A-Za-z0-9_]*/ID/g' | sort | uniq -c | sort -rn | head -5
  53 ID)->ID
  31 ID)->ID ID
   5  ID(()->ID ID
   5     ()->ID ID
   4  ID)->ID
```

**Rule.** `space_around_scope_op = false`, member `.` tight, `space_around_arrow = true`,
designated initializer always `.field = value`.

### 6.6 Parentheses, brackets, angles

```
'( x'  space after '('          : 214
'(x'   no space after '('       : 26451      99.20% tight
'x )'  space before ')'         : 17
'x)'   no space before ')'      : 25777      99.93% tight
'()'   empty parens             : 4955
'( )'  empty parens spaced      : 0

'[x'   no space after '['       : 6375       99.30% tight
'[ x'  space after '['          : 45
'x]'   no space before ']'      : 7366      100.00% tight
'x ]'  space before ']'         : 0

simple `Ident<...>` instantiations on one line : 8539
   '< ' padded                                 :   24      0.28%
   ' >' padded                                 :   23      0.27%
```

**Rule.** `space_inside_parens = false`, `space_inside_brackets = false`,
`space_inside_angles = false`.

### 6.7 Space between a callee/keyword and `(`

This is the messiest area of the whole style and needs a per-keyword table.

```
$ ex() { a=$(grep -rhoP "$2" --include='*.k' . | wc -l); b=$(grep -rhoP "$3" --include='*.k' . | wc -l); \
         printf "%-22s %8s %8s %8s\n" "$1" "$a" "$b" "$(awk -v a=$a -v t=$((a+b)) 'BEGIN{printf "%.1f",100*a/t}')"; }
$ ex "if"            '(?<![\w])(?<!static )if \('  '(?<![\w])(?<!static )if\('
$ ex "static if"     'static if \('               'static if\('
$ ex "for"           '(?<![\w])(?<!static )for \(' '(?<![\w])(?<!static )for\('
$ ex "static for"    'static for \('              'static for\('
$ ex "while"         '(?<![\w])while \('          '(?<![\w])while\('
$ ex "static assert" 'static assert \('           'static assert\('
$ ex "switch"        '(?<![\w])switch \('         '(?<![\w])switch\('
$ ex "return"        '(?<![\w])return \('         '(?<![\w])return\('
pattern                  spaced unspaced  %spaced
if                         1179         23     98.1
static if                   207          5     97.6
for                         257          2     99.2
static for                  285        215     57.0
while                        92         79     53.8
static assert                67        494     11.9
switch                       59         19     75.6
return                       33         13     71.7
```

and, over `library/` alone (the curated half of the corpus):

```
pattern                  spaced unspaced  %spaced
if                          370         13     96.6
static if                    58          1     98.3
for                           2          0    100.0
static for                   72         63     53.3
while                         9         14     39.1
static assert                 8        156      4.9
switch                       15          0    100.0
return                       21         13     61.8
```

The pseudo-function keyword operators are **100% tight** in both halves:

```
concat(   212 : 0        mux(       437 : 0        clog2(     112 : 0
lutmul(    21 : 0        decltype(  239 : 0        cast<...>( 1501 (never `cast <`)
fan_out(    7 : 0        bitoffsetof( 32 : 0       byteoffsetof( 10 : 0
bitsizeof( 287 : 5       bytesizeof(  36 : 3       static_assert(  6 : 0
```

Ordinary function calls are tight:

```
$ grep -rhoP '(?<![\w])(?!if|for|while|switch|return|do|else|case|default|static|assert|template|class|struct|union|enum|module|import|using|export|extern|inline|noinline|atomic|public|private|break)\w+\(' --include='*.k' . | wc -l
15243
$ grep -rhoP '(?<![\w])(?!if|for|while|switch|return|do|else|case|default|static|assert|template|class|struct|union|enum|module|import|using|export|extern|inline|noinline|atomic|public|private|break)\w+ \(' --include='*.k' . | wc -l
114
$ grep -rhoP '(?<![\w])\w+ \(' --include='*.k' . | sort | uniq -c | sort -rn | head -14
1386 if (
 542 for (
  92 while (
  68 assert (
  59 switch (
  57 pipelined_for (
  33 return (
  22 range_for (
  18 const (
   6 Foo (
   5 bitsizeof (
   3 bytesizeof (
   2 static (
   1 while_do (
```

99.26% of ordinary calls are `name(`; the residue is 57 `pipelined_for (` + 22 `range_for (`
(library helper functions that *look* like control flow) plus a handful of typos.

The `template` keyword is a separate case, because its delimiter is `<` rather than `(`:

```
$ printf "'template <' : %s\n'template<'  : %s\n" \
    "$(grep -rhoP 'template <' --include='*.k' . | wc -l)" "$(grep -rhoP 'template<' --include='*.k' . | wc -l)"
'template <' : 1100
'template<'  : 236
```

82.3% spaced corpus-wide, but only 64.6% in `library/` (243 : 133) against 89.0% in `test/`
(837 : 103). The tight form is concentrated in four numeric-library files
(`library/numeric/fixed.k` 33, `library/numeric/float32.k` 32, `library/data/bits.k` 18,
`library/numeric/float64.k` 10 — 93 of the 133 library occurrences). This is a majority
convention, not a near-universal one; §11 defaults it to `true` but it is the weakest
normalization in the table.

**Rules.**

| construct | default | evidence |
|---|---|---|
| `if (`, `static if (`, `for (`, `switch (` | space | 98.1 / 97.6 / 99.2 / 75.6% |
| `while (` | space | 53.8% corpus-wide but `while` almost always appears as `do; while(…)` in library idiom; §10.5 marks this **preserve** |
| `return (` | no opinion — `return` takes an expression, not a parenthesized operand | 71.7% |
| `static for (` | **preserve** | 57.0% corpus / 53.3% library — genuinely split |
| `static assert(` | **no space** | 88.1% corpus / 95.1% library |
| `concat( mux( clog2( lutmul( decltype( cast< fan_out( bitoffsetof( byteoffsetof( bitsizeof( bytesizeof( static_assert(` | no space | 100% (bitsizeof 98.3%, bytesizeof 92.3%) |
| ordinary call | no space | 99.26% |

`static assert` being tight is worth flagging: it is a two-word keyword (`keyword2`,
`Lexer.hs:255-256`), so `static assert(x)` reads oddly, but 494 of 561 occurrences are written
that way and the reference pretty-printer agrees (`PrettyPrint.hs:151`:
`src (StaticAssert a) = "static assert" <> parens (pretty a)`).

### 6.8 Colons

```
$ grep -rhoP 'for\s*\([^)]* : ' --include='*.k' . | wc -l   ->  748     # range-for
$ grep -rhoP 'for\s*\([^)]*\w:\w' --include='*.k' . | wc -l ->    0
$ grep -rhoP 'enum\s+\w+\s+:\s+\w'                  | wc -l ->  106
$ grep -rhoP 'enum\s+\w+\s*:\s*\w'                  | wc -l ->  108     # so 2 are tight
$ grep -rhoP 'case [^:]+:(?!:)'                     | wc -l ->  210
$ grep -rhoP 'case [^:]+ :(?!:)'                    | wc -l ->    0
```

Range-for colon: **always** ` : ` (748 : 0). `enum E : T`: 106 spaced, 2 tight.
Label colons (`case X:`, `default:`, `public:`, `private:`) are always tight (210 : 0).

**Rule.** `space_around_range_colon = true`, `space_around_enum_colon = true`, label colons
tight.

### 6.9 Semicolon

```
$ grep -rhP '\S +;' $(cat /tmp/corpus480.txt) | grep -v '"' | grep -v '//' | wc -l
       5
$ grep -rhoF ';' $(cat /tmp/corpus480.txt) | wc -l
   30432
```

Five real occurrences in 30 432 semicolons, and two of them are the `do ; while (…)` idiom
(`do` with an empty body), where the space is arguably meaningful:

```
            do ; while (body());
            do ; while (body(tid));
    using set_index_t = index_t<_setCount> ;
    using entry_index_t = uint<_entryIndexBits> ;
            do ; while (body());
```

**Rule.** `space_before_semicolon = false`. Special-case the empty `do` body: emit `do;`.
The complete `do` census (171 occurrences of the keyword, stripped corpus):

```
$ grep -rhoP '(?<![\w])do\s*[;{]?' --include='*.k' . | sed 's/[[:space:]]*$//' | sort | uniq -c | sort -rn
 111 do        (of which 55 are `do` alone at end of line — the Allman block form)
  89 do;
  22 do {
   3 do ;
   2 do{
```

`do;` 89 : `do ;` 3 for the empty-body form; `do` at EOL 55 : `do {` 22 : `do{` 2 for the
block form (so the block form is 71.4% Allman, consistent with §2.2's 52 : 0 for the
Allman/K&R classifier, which only counts the `{`-on-its-own-line cases it can attribute).

### 6.10 `*` and `&` declarators: not applicable

Kanagawa has no pointers and no user-written reference declarators.

```
$ grep -rhoP '\w&[^&=]' --include='*.k' . | wc -l
0
```

Zero occurrences of an identifier immediately followed by `&` other than `&&`/`&=`. `&` is
only bitwise-AND; `&&` logical AND; `&=` / `&&=` compound assignment. The `Reference` AST node
exists (`Syntax.hs`, printed as `x&` at `PrettyPrint.hs:143`) but is **synthesized**, not
parsed — `grep -n Reference compiler/hs/lib/Language/Kanagawa/Parser.hs` returns nothing.
It models the implicit `this` reference (`Frontend.hs:453-457`) and class-instance types
(`Type.hs:1249`).

**Consequence for §13:** `PrettyPrint.hs:143` emits syntax the parser cannot read (§13.5).

---

## 7. Blank lines

### 7.1 Run lengths

```
total blank-line runs: 22538   total blank lines: 22883
  run of 1 blank line(s):  22200  (98.50%)
  run of 2 blank line(s):    331  ( 1.47%)
  run of 3 blank line(s):      7  ( 0.03%)
```

Nothing longer than 3. Where the multi-blank runs occur:

```
  run of 2 blank lines, previous line at indent 0        292  library/compiler/config.k:57
  run of 2 blank lines, previous line at indent 4         30  library/data/random/xoshiro.k:16
  run of 3 blank lines, previous line at indent 0          7  library/intel/agilex-7/config.k:137
  run of 2 blank lines, previous line at indent 8          7  test/library/data/fifo/wide_to_narrow.k:97
  run of 2 blank lines, previous line at indent 10         1  test/library/processor/risc-v.k:3495
  run of 2 blank lines, previous line at indent 16         1  test/logic/test_cases_30.k:599
```

88% of the 2-blank runs are at a top-level (indent-0) boundary, i.e. used as a coarse section
separator between groups of declarations.

**Rule.** `max_blank_lines = 2`. Collapse 3+ to 2; **preserve** 2 (§10.1).

### 7.2 Inside a brace

```
lines that are exactly `{`: 13375
  next line blank :     19 (0.14%)
  next line code  :  13356 (99.86%)
lines that are exactly `}` or `};`: 12817
  prev line blank :     33 (0.26%)
  prev line code  :  12784 (99.74%)
```

**Rule.** `blank_line_after_open_brace = 0`, `blank_line_before_close_brace = 0`. Strip both.

### 7.3 Between declarations, by nesting depth

Measured as: after a line that is exactly `}` / `};`, is the next non-blank line preceded by a
blank line?

```
  indent  0: next-decl cases 2772 -> blank 2764 ( 99.7%)  no-blank    8 (  0.3%)   [close/else    1]
  indent  2: next-decl cases  255 -> blank  251 ( 98.4%)  no-blank    4 (  1.6%)   [close/else    3]
  indent  4: next-decl cases 4798 -> blank 4677 ( 97.5%)  no-blank  121 (  2.5%)   [close/else  976]
  indent  6: next-decl cases   11 -> blank   11 (100.0%)  no-blank    0 (  0.0%)   [close/else   19]
  indent  8: next-decl cases 1228 -> blank 1079 ( 87.9%)  no-blank  149 ( 12.1%)   [close/else 1125]
  indent 12: next-decl cases  376 -> blank  266 ( 70.7%)  no-blank  110 ( 29.3%)   [close/else  424]
  indent 16: next-decl cases  141 -> blank   81 ( 57.4%)  no-blank   60 ( 42.6%)   [close/else  130]
  indent 20: next-decl cases   58 -> blank   35 ( 60.3%)  no-blank   23 ( 39.7%)   [close/else   44]
  indent 24: next-decl cases    9 -> blank    7 ( 77.8%)  no-blank    2 ( 22.2%)   [close/else   23]
  indent 28: next-decl cases   17 -> blank    3 ( 17.6%)  no-blank   14 ( 82.4%)   [close/else   17]
```

A clean gradient: at module scope a blank line between declarations is essentially mandatory
(99.7%), at class-member scope near-mandatory (97.5%), inside a function body it decays to a
coin flip by depth 16.

**Rule.** Insert exactly one blank line between two top-level declarations
(`blank_line_between_top_level_decls = 1`) and between two class members (indent 4). Below
that, **preserve** the author's choice — an inner block that ends and is immediately followed
by more statements is normal control flow, not a declaration boundary.

### 7.4 Around access labels

```
  label is first line after `{`                  1280
  label preceded by a blank line                  918
  label directly after a member (no blank)         61
```

918 : 61 = 93.8% of non-first labels get a blank line before them; the first label in a class
body never does (it is the line right after `{`).

**Rule.** `blank_line_before_access_label = 1`, suppressed when the label is the first line of
the body.

### 7.5 File preamble

```
$ n=0; for f in $(cat /tmp/corpus480.txt); do [ -z "$(LC_ALL=C sed -n 3p "$f")" ] && n=$((n+1)); done; echo "$n / 480"
470 / 480
```

Line 3 is blank in 470 of 480 files — the blank line after the two-line MIT header (§9.1).

Around the module declaration:

```
  blank line immediately before `module`   171
  a module doc comment immediately before  66
```

(the 66 are `//|` or `/*| … */` blocks documenting the module, e.g.
`library/compiler/config.k:4-16` — a `/*|` block that runs from line 4 to line 16 and is
immediately followed by `module compiler.config` on line 17.)

Around the import block:

```
  blank line AFTER import block          307   (of 307 — 100%)
  blank line BEFORE import block         303
  non-blank before import block            4
```

**Rule.**

```
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
                              <- exactly one blank line
[optional //| or /*| module doc comment]
module a.b
    { X
    }
                              <- exactly one blank line
import ...
import ...
                              <- exactly one blank line
<first declaration>
```

---

## 8. Comments

### 8.1 Census

```
$ for pat in '//|' '// |' '//<' '// <' '/*|' '/*<' '/*!' '/*' '//'; do \
    printf "%-8s : %s\n" "$pat" "$(grep -rhoF "$pat" $(cat /tmp/corpus480.txt) | wc -l)"; done
//|      : 1115
// |     :   36
//<      :  545
// <     :    6
/*|      :   61
/*<      :   44
/*!      :    6
/*       :  423
//       : 10108
```

The lexer accepts horizontal whitespace between the introducer and the doc marker:
`docComment'` does `(string "//" <|> string "/*") *> hspace *> char prefix`
(`Lexer.hs:82`), which is why `// |` (36) and `// <` (6) exist and are still doc comments.
The two markers are defined at `Lexer.hs:63-65`:
`docCommentPrefixPre = '|'`, `docCommentPrefixPost = '<'`.
`/*!` (6 occurrences) is **not** a doc marker — it is an ordinary block comment.

### 8.2 The license header

```
$ n=0; for f in $(cat /tmp/corpus480.txt); do \
    [ "$(LC_ALL=C head -1 "$f")" = "// Copyright (c) Microsoft Corporation." ] && n=$((n+1)); done; echo "$n / 480"
468 / 480
$ n=0; for f in $(cat /tmp/corpus480.txt); do \
    [ "$(LC_ALL=C sed -n 2p "$f")" = "// Licensed under the MIT License." ] && n=$((n+1)); done; echo "$n / 480"
468 / 480
```

Exactly two lines, no leading/trailing blank inside, followed by one blank line:

```
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.
```

The 12 exceptions are enumerated in full — all of them are outside `library/` and `test/`
proper:

```
test/syntax/imports/binary.k                     (the 4-byte binary fixture, §0.4)
tools/chkdoc/contexts/emptyfile.k                "// an empty file"
tools/chkdoc/contexts/emptyfunction.k            "// an empty function"
tools/sandcastle/test/sample/a/b/c.k             starts with `module sample.a.b.c`
tools/sandcastle/test/sample/alias.k
tools/sandcastle/test/sample/callbacks.k
tools/sandcastle/test/sample/decls.k             starts with `/*| Module description.`
tools/sandcastle/test/sample/decls/initializer.k
tools/sandcastle/test/sample/examples.k
tools/sandcastle/test/sample/links.k             starts with `//| [To custom-anchor](#custom-anchor)`
tools/sandcastle/test/sample/self_exposed.k
tools/sandcastle/test/sample/template_instance.k
```

So within `library/` + `test/`, 468 of 469 text files carry the header — 100% of everything
that is really Kanagawa source.

**Rule.** `license_header` is a configurable literal; `require_license_header = false` by
default (inserting a copyright notice is a legal act, not a formatting act), but `kg fmt`
must never *reorder* or *reindent* lines 1-2 when they match.

### 8.3 `//|` (pre-doc) and `//<` (post-doc) placement

```
$ printf "own line: %s   trailing: %s\n" \
   "$(grep -rhcE '^[[:space:]]*//\|' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}')" \
   "$(grep -rhcE '^[[:space:]]*[^[:space:]/].*//\|' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}')"
own line: 1087   trailing: 22
$ printf "own line: %s   trailing: %s\n" \
   "$(grep -rhcE '^[[:space:]]*//<' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}')" \
   "$(grep -rhcE '^[[:space:]]*[^[:space:]/].*//<' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}')"
own line: 20   trailing: 525
$ grep -rhcE '^[[:space:]]*[,<>{}].*//<' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}'
423
```

- `//|` is **98.0% on its own line, above the thing it documents.**
- `//<` is **96.3% trailing, on the same line as the thing it documents**, and 423 of those
  525 (80.6%) are on a leading-comma list item — i.e. `//<` exists to annotate template
  parameters and function parameters in the leading-comma tables of §3.

This matches the semantics: `docCommentPre` / `docCommentPost` (`Lexer.hs:90-92`) attach the
`|` form before a construct and the `<` form after it.

### 8.4 Doc-block continuation lines

A `//|` block is continued with **bare `//`** lines, not repeated `//|`. The lexer requires
this: once the `|` prefix is seen, `docComment'` consumes `many (some lineComment)` where
`lineComment = string "//" *> manyTill anySingle (char '\n')` (`Lexer.hs:84-87`) — every
following `//` line is absorbed into the same doc comment.

```
   continuation indent == `//|` indent            2696   (of 2696 — 100%)
   continuation uses bare `//`                    2695
   continuation uses `//<`                           1
   doc block with 0 continuation line(s)           632
   doc block with 1 continuation line(s)           152
   doc block with 2 continuation line(s)            70
   doc block with 3+ continuation lines            233
```

Canonical, `library/data/fifo.k:18-25`:

```
//| Basic FIFO.
template
    < typename T                  //< Type for each entry of the FIFO.
    , auto Size                   //< Maximum number of entries that can be stored.
    , bool EnqueueBlocking = true //< Block on enqueue if the FIFO is full until an entry
                                  // frees up. By default this is true. Otherwise,
                                  // the caller must ensure that the FIFO is not full.
```

Note the trailing-`//<` continuation on lines 23-24: it is a bare `//` **aligned to the `//<`
column**, not to the code indent.

**Rule.** Never rewrite `//` to `//|` or vice versa. Continuation lines keep the exact indent
of the marker line they continue.

### 8.5 Comment indentation and alignment

```
own-line comments followed by code: 9321
   same indent as next code line      8972 ( 96.3%)
   deeper than next code               214 (  2.3%)
   shallower than next code            135 (  1.4%)
(comment indent - next-code indent) distribution:
     +0 :   8972 ( 96.3%)
     -4 :    119 (  1.3%)
     +4 :     27 (  0.3%)
```

An own-line comment sits at the indent of the code it introduces (96.3%).

Trailing comments:

```
runs of >=2 consecutive trailing-comment lines : 164
   ... where every comment starts in the same column : 144 (87.8%)
run length distribution: {2: 83, 3: 41, 4: 34, 5: 1, 6: 2, 7: 1, 8: 1, 32: 1}

spaces between end-of-code and start of trailing comment:
     1 space(s):    309 ( 42.9%)
     2 space(s):     58 (  8.0%)
     3 space(s):     38 (  5.3%)
     4 space(s):     36 (  5.0%)
     6 space(s):     30 (  4.2%)
     5 space(s):     28 (  3.9%)
     7 space(s):     20 (  2.8%)
    40 space(s):     14 (  1.9%)
    15 space(s):     14 (  1.9%)
     8 space(s):     12 (  1.7%)
```

The long tail (40, 15, 8 spaces) is column alignment. **87.8% of multi-line trailing-comment
runs are column-aligned.** A formatter that collapses trailing comments to one space would
destroy 144 hand-built tables.

**Rule.** `align_trailing_comments = "preserve"`, `trailing_comment_min_spaces = 1`.
A *lone* trailing comment gets one space; a comment that is part of an aligned run keeps its
column.

### 8.6 Space after the comment introducer

```
'// text'  : 7549     '//text'  : 75    (after excluding URLs and the 814 bare `//` lines)
'//| text' : 1111     '//|text' : 4
'//< text' : 544      '//<text' : 1
$ grep -rhcE '^[[:space:]]*//[[:space:]]*$' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}'
814
```

814 bare `//` lines are used as vertical spacers *inside* doc blocks (they cannot be blank
lines, because a blank line would terminate the doc comment).

There are **no banner/ruler comments** anywhere:

```
$ grep -rhoE '^[[:space:]]*//[-=*/]{3,}' $(cat /tmp/corpus480.txt) | wc -l
0
```

**Rule.** `space_after_comment_marker = true`, except a bare `//` on an otherwise empty line
stays bare.

### 8.7 Block comments

```
$ printf "own line: %s  mid-line: %s\n" \
    "$(grep -rhcE '^[[:space:]]*/\*' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}')" \
    "$(grep -rhcE '^[[:space:]]*[^[:space:]/].*/\*' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}')"
own line: 352  mid-line: 56
```

Block comments are nesting (`L.skipBlockCommentNested "/*" "*/"`, `Lexer.hs:103`,
`Lexer.hs:116-119`). 86% start their own line. Interior lines have no `*` gutter — the
convention is flush-left prose, e.g. `library/compiler/config.k:4-16`.

**Rule.** Preserve block-comment interiors verbatim. Never reflow, never add a `*` gutter.

---

## 9. Imports and the module header

### 9.1 Position

```
files containing at least one `import` (col 0): 307
  import block is contiguous (no blank/other lines inside)   300
  imports AFTER module decl                                  147
  no module decl in file                                     159
  import block has internal gaps                               7   library/data/vector.k:10
  imports STRADDLE module decl                                 1
```

Imports always follow the `module` declaration when there is one (147 : 1). 159 files have
imports but no `module` — these are the top-level test programs, which are compiled as
anonymous units.

```
$ grep -rhcE '^import ' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}'    ->  790
$ grep -rhcE '^[[:space:]]+import ' $(cat /tmp/corpus480.txt)|awk '{s+=$1}END{print s}'  ->  275
```

The 275 indented `import`s live in exactly 31 files. The complete list:

```
$ grep -rlE '^[[:space:]]+import ' $(cat /tmp/corpus480.txt) | sort
library/processor/risc_v.k
test/syntax/all-warnings-as-errors.k
test/syntax/atomic.k
test/syntax/auto-placeholder.k
test/syntax/basics.k
test/syntax/closures.k
test/syntax/constexpr.k
test/syntax/decltype.k
test/syntax/doc-comments.k
test/syntax/extern-class.k
test/syntax/function-attributes.k
test/syntax/function-template.k
test/syntax/function-type.k
test/syntax/higher-order-functions.k
test/syntax/imports.k
test/syntax/initializer-list.k
test/syntax/local-classes.k
test/syntax/local-functions.k
test/syntax/memory.k
test/syntax/no-warning.k
test/syntax/options.k
test/syntax/shadow-warning.k
test/syntax/static-for.k
test/syntax/template-argument-deduction-from-instance.k
test/syntax/template-argument-deduction.k
test/syntax/template-instance-identifiers.k
test/syntax/template-template-parameter.k
test/syntax/template.k
test/syntax/this-capture.k
test/syntax/type-constexpr.k
test/syntax/using.k
```

30 of the 31 are `test/syntax/` harness fixtures (§0.3); the one non-fixture,
`library/processor/risc_v.k`, indents an import inside a doc-comment code example. Do not
treat any of these as an indentation convention.

### 9.2 Ordering

The measurement: for each file, take the column-0 `import` lines in source order, extract the
module name (field 2), and test `names == sorted(names)`.

```
$ python3 -c '
import sys,os
for sub,mn in [("library/",2),("test/",2),("library/",3),("test/",3),("library/",4),("test/",4)]:
    n=s=0
    for f in open("/tmp/corpus480.txt").read().split():
        if not f.startswith(sub): continue
        imp=[l for l in open(f,errors="replace").read().split("\n") if l.startswith("import ")]
        if len(imp)<mn: continue
        n+=1; names=[l.split()[1] for l in imp]
        if names==sorted(names): s+=1
    print(f"{sub:9} (>= {mn} imports): {s}/{n} sorted ({100*s/n:.1f}%)")'
library/ (>= 2 imports): 38/56 sorted (67.9%)
test/    (>= 2 imports): 19/164 sorted (11.6%)
library/ (>= 3 imports): 19/24 sorted (79.2%)
test/    (>= 3 imports): 12/84 sorted (14.3%)
library/ (>= 4 imports): 13/18 sorted (72.2%)
test/    (>= 4 imports): 7/43 sorted (16.3%)
ALL      (any count)   : 144/307 sorted (46.9%)
```

`library/` sorts (72-79%); `test/` does not (14-16%, because test files conventionally put
`import test.unit as unit` first regardless of alphabet). The canonical library block is
`library/data/fifo.k:10-16`:

```
import data.counter
import data.memory
import numeric.int.operator
import sync.atomic
import sync.lock
import type.coerce
import type.stdtype
```

The **complete** list of the 18 `library/` files with ≥2 imports whose block is not sorted,
with their full import order:

```
library/base.k                                 .cmdargs, base.system, control.async, control.loop, control.wait,
                                               data.array, data.closure, data.memory, data.optional, data.tuple,
                                               debug.print, hardware.dsp, compiler.config, compiler.device.schema,
                                               numeric.float32.operator, type.coerce, type.cstddef, type.stdtype
library/compiler/config.k                      compiler.device.schema, compiler.device.config
library/data/counter/saturating.k              numeric.int.limits, data.order
library/data/fifo/multi.k                      sync.lock.multi, numeric.int.operator.modular
library/data/fifo/transpose.k                  sync.lock, sync.atomic, data.array, numeric.int.operator
library/data/vector.k                          data.vector, .options, data.array, data.function, data.optional,
                                               data.order, data.tuple, sync.atomic, numeric.int.operator,
                                               numeric.int.operator.unsigned, type.stdtype
library/device/agilex-7-f-023/hardware/dsp.k       compiler.hal, compiler.config
library/device/arria-10-gx-1150/hardware/dsp.k     compiler.hal, compiler.config
library/device/mock/hardware/dsp.k                 compiler.hal, compiler.config
library/device/stratix-10-gx-2800/hardware/dsp.k   compiler.hal, compiler.config
library/device/stratix-10-nx-2100/hardware/dsp.k   compiler.hal, compiler.config
library/device/ultrascaleplus-xcu250/hardware/dsp.k compiler.hal, compiler.config
library/mini-base.k                            .cmdargs, data.closure, debug.print, compiler.config,
                                               compiler.device.schema
library/numeric/float32.k                      data.bits, data.optional, data.tuple, numeric.fixed.internal,
                                               numeric.float32.internal, numeric.float32.internal.operator,
                                               numeric.float.internal
library/numeric/float32/internal/operator.k    numeric.float.internal, data.bits
library/processor/risc_v/trace/risc_v_dv.k     processor.risc_v.trace, processor.risc_v.internal.decoder
library/sync/lock.k                            type.stdtype, data.counter
library/test/unit.k                            test.internal.params, test.internal.assert
```

Two patterns recur in that list and both look deliberate rather than sloppy:
`compiler.hal` before `compiler.config` (all 6 device `dsp.k` files) and
`compiler.device.schema` before `compiler.device.config`. That matters because `import` is
executed *during parsing*: `parseImports` (`Parser.hs:194-206`) parses the imported file and
folds its symbols into the current symbol table (`updateSymbolTable` → `updateImports`,
`Parser.hs:203-206`) before the next line of the importing file is read. Whether these
specific orderings are *required* is **UNVERIFIED** (§14, Q9), but a formatter has no way to
prove reordering is safe, which is reason enough for `sort_imports = false`.

Note `library/base.k` and `library/data/vector.k` import `.cmdargs` / `.options` — the
compiler's synthetic modules (`cmdargsSpecialModule` = `".cmdargs"` and `optionsSpecialModule` = `".options"`,
`Parser.hs:41-45`, accepted by `moduleName` at `Parser.hs:179-183`) — which must stay where they are.

**Rule.** `sort_imports = false` by default. 53% of files would move if it were true, and
sorting is not a whitespace change: `import` is a *side-effecting* directive (`parseImports`,
`Parser.hs:194-206`, parses the imported file and merges its symbols into the symbol table at
that point) and the compiler has a `redundant-import` warning
(`test/syntax/*.k` uses `warning:redundant-import` 10 times, §0.3) whose firing can depend on
order. Ship `sort_imports = true` as an opt-in for `library/`-style code.

### 9.3 Grouping

```
gap sizes inside import blocks: {1: 6, 61: 1}
  blank line AFTER import block          307   (100%)
  blank line BEFORE import block         303   (98.7%)
```

300 of 307 import blocks are one contiguous run. Six have a single one-line gap; one file
(`library/data/vector.k:10` onward) has a 61-line gap because it re-imports after a large
declaration. **There is no two-group (stdlib/local) convention.**

```
`import X`         : 542
`import X as Y`    : 248
```

Aliased imports are common (31%) but are not segregated into their own group: only 134/307
files match "plain group first, then aliased group, each sorted".

**Rule.** `group_imports = false`. One contiguous block, one blank line after.

### 9.4 Module re-export forms

The export list may contain, besides plain identifiers, two module forms
(`Parser.hs:185-192`):

- `module a.b` — re-export the whole module (`ModuleIdentifier`), 16 occurrences of the
  shape `{ ID ID` in the corpus.
- `module a.b \ c.d` — module difference (`ModuleDiff`), 17 occurrences of the shape
  `{ ID ID \ ID`.

Examples: `library/compiler/hal.k:4-6` (`{ module compiler.hal }` — a self re-export) and
`test/syntax/helper/custom_base.k:4-7`:

```
module helper.custom_base
  { Foo
  , module mini-base
  }
```

**Rule.** These are ordinary export-list items and follow §3.1 exactly.

---

## 10. Preserve, do not normalize

Ten places where the corpus is genuinely divided and `kg fmt` must therefore keep the author's
choice. Each is listed with the measurement that shows the division.

### 10.1 Blank-line runs of 2

331 runs of exactly 2 blank lines (§7.1), 292 of them at a top-level boundary. Collapsing to
1 would touch 331 sites and delete a deliberate section-break signal. **Preserve up to
`max_blank_lines = 2`; collapse 3+ (7 sites) to 2.**

### 10.2 Whether a short body collapses

171 collapsed bodies (79 `{}`, 92 `{ stmt; }`) vs 13 375 expanded (§2.3). 1.3% is too small to
be the rule and too deliberate to be noise (`inline void foo(){}` is a legitimate stub idiom in
`test/syntax/helper/*.k`). **`collapse_empty_body = "preserve"`,
`collapse_short_body = "preserve"`.**

### 10.3 Column alignment (trailing comments, aligned `=`, aligned commas)

- 144/164 multi-line trailing-comment runs are column-aligned (§8.5).
- Only 31/1690 runs of `=`-bearing lines are `=`-aligned (§10.3 measurement below) — so `=`
  alignment is *not* a convention, but the 31 that exist are hand-built tables:
  ```
  runs of >=2 consecutive lines containing a top-level `=` : 1690
     ... where the `=` is in the SAME column but the preceding code differs in length
         (i.e. deliberate column alignment)                : 31 (1.8%)
  ```
  The clearest is `library/processor/risc_v/isa.k:186-192`, which aligns the identifier, the
  `=`, the value *and* the `//<` comment simultaneously.

**`align_assignments = false`** (never *introduce* alignment), but the formatter must not
*destroy* existing alignment either: treat any run of ≥2 lines whose trailing comments or
`=` share a column as an atom and re-emit it verbatim.

### 10.4 Import order

46.9% sorted corpus-wide, 79.2% in `library/` with ≥3 imports, 14.3% in `test/` (§9.2).
**`sort_imports = false`.**

### 10.5 `while (` vs `while(`

53.8% spaced corpus-wide, 39.1% in `library/` (§6.7) — the only control keyword where
`library/` prefers the *tight* form, because the library idiom is `do; while(cond);` rather
than a standalone `while` loop. **Preserve.**

### 10.6 `static for (` vs `static for(`

285 : 215 corpus, 72 : 63 library (§6.7). A dead heat. **Preserve.**

### 10.7 Initializer brace padding

`= { a, b }` 138 vs `= {a, b}` 266 corpus-wide; 2 vs 16 in `library/` (§3.4). The padded form
is concentrated in aligned numeric tables. **`space_inside_braces_initializer = "preserve"`.**

### 10.8 `[[attribute]]` placement

70.7% same-line / 17.5% own-line / 11.8% mid-line, and the split is *per attribute*, not
per author (§4.2). **`attribute_placement = "preserve"`.**

### 10.9 `case` bodies on the label line

197 `case X: stmt;` vs 177 `case X:` + indented body (§2.6). **Preserve** (covered by
`collapse_short_body`).

### 10.10 Tabs inside string literals

52 tabs inside string literals in `test/library/processor/risc-v.k:173-224` encode expected
RISC-V disassembly output. **Never touch bytes inside a string literal**, including tabs and
the `{{` / `}}` escapes of interpolated strings (`doc/programming-guide.md:572-573`).

### 10.11 Bonus: blank line between statements deeper than indent 8

Blank-before-next-declaration falls from 99.7% at indent 0 to 57.4% at indent 16 (§7.3). Below
class-member scope, **preserve**.

---

## 11. Proposed `[fmt]` defaults for `kanagawa.toml`

Every knob below has a §-reference to the measurement that fixes its default.

```toml
[fmt]
# ── File-level (§1) ────────────────────────────────────────────────────────────
indent_style                = "space"    # §1.2  1 tab-indented file in 480
indent_width                = 4          # §1.1  96.44% of indents are a multiple of 4
tab_width                   = 4          # width accounting for tabs surviving in comments
newline                     = "lf"       # §1.6  0 CRLF files
final_newline               = true       # §1.6  86/480 currently lack it
trim_trailing_whitespace    = true       # §1.5  228 lines in 60 files
line_width                  = 100        # §5.4  98.8% of lines already fit; ref tool default is 80
max_blank_lines             = 2          # §7.1  331 runs of 2, 7 runs of 3, 0 runs of 4+

# ── Braces (§2) ────────────────────────────────────────────────────────────────
brace_style                 = "allman"   # §2.2  98.7% overall; 100% for class/struct/union/enum/for/else
brace_style_lambda          = "allman"   # §2.2  972 : 23
brace_style_initializer     = "kr"       # §2.2  137 : 50 — `T x = {` stays on the decl line
collapse_empty_body         = "preserve" # §10.2
collapse_short_body         = "preserve" # §10.2
type_body_semicolon         = "remove"   # §2.4  2323 : 48
access_label_indent         = 0          # §2.5  2187 : 67 (relative to the `class` keyword)
blank_line_before_access_label = 1       # §7.4  918 : 61, suppressed for the first label
case_indent                 = 4          # §2.6  251 : 123 (relative to the `switch` brace)

# ── Lists (§3) ─────────────────────────────────────────────────────────────────
list_indent                 = 4          # §3.1/§3.2/§3.3  186/117/112 at +4
list_style_module_exports   = "leading_comma"    # §3.1  129 : 1
list_style_template_params  = "leading_comma"    # §3.2   98 : 3
list_style_function_params  = "leading_comma"    # §3.3   89 : 4
list_style_call_args        = "leading_comma"    # §3 (call/param paren) 180 leading : 229 trailing → see note
list_style_initializers     = "trailing_comma"   # §3.4  188 : 46
list_style_enum_body        = "trailing_comma"   # §3.5   58 : 1
module_export_brace_hugs_first_item = true       # §3.1  201 : 5
template_keyword_own_line   = true       # §3.2  when the list breaks: 116 : 5
param_open_paren_own_line   = true       # §3.3  when the list breaks: 113 : 12
closer_aligns_with_opener   = true       # §3.1/§3.2/§3.3  205/111/111 of 205/121/113
enum_one_constant_per_line  = "preserve" # §3.5  58 multi-line : 49 single-line

# ── Spacing (§6) ───────────────────────────────────────────────────────────────
space_around_binary_ops     = true       # §6.1  95.6–100% per operator
space_around_assign         = true       # §6.2  99.6%
space_after_unary_prefix    = false      # §6.3  638:0 for `!`, 84:0 for `~`
space_around_ternary        = true       # §6.3  595 : 0
space_after_comma           = true       # §6.4  42784 : 8 (excluding generated tables)
space_before_comma          = false      # §6.4
space_around_scope_op       = false      # §6.5  8014 : 0
space_around_member_dot     = false      # §6.5  9680 : 1
space_around_arrow          = true       # §6.5  949 : 141
space_inside_parens         = false      # §6.6  26451 : 214
space_inside_brackets       = false      # §6.6   6375 :  45
space_inside_angles         = false      # §6.6   8539 :  24
space_inside_braces_initializer = "preserve"     # §10.7  266 : 138
space_before_semicolon      = false      # §6.9  5 real occurrences in 30432
space_after_control_keyword = true       # §6.7  `if (` 98.1%, `for (` 99.2%, `switch (` 75.6%
space_after_while           = "preserve" # §10.5  53.8% corpus / 39.1% library
space_after_static_for      = "preserve" # §10.6  285 : 215
space_after_static_assert   = false      # §6.7  494 : 67 (and PrettyPrint.hs:151 agrees)
space_before_call_paren     = false      # §6.7  15243 : 114
space_after_template_keyword = true      # §6.7  1100 : 236 corpus, 243 : 133 library — weakest default
space_around_range_colon    = true       # §6.8  748 : 0
space_around_enum_colon     = true       # §6.8  106 : 2
space_before_label_colon    = false      # §6.8  210 : 0

# ── Attributes (§4) ────────────────────────────────────────────────────────────
attribute_placement         = "preserve" # §10.8  753 same-line : 186 own-line : 126 mid-line
attribute_own_line_when     = []         # opt-in; corpus would suggest ["name","schedule","reorder_by_looping"]

# ── Comments (§8) ──────────────────────────────────────────────────────────────
doc_comment_pre             = "//|"      # Lexer.hs:64
doc_comment_post            = "//<"      # Lexer.hs:65
space_after_comment_marker  = true       # §8.6  7549 : 75
comment_indent              = "follow_code"      # §8.5  96.3%
align_trailing_comments     = "preserve" # §8.5  144/164 aligned runs
trailing_comment_min_spaces = 1          # §8.5  42.9% use exactly 1
reflow_comments             = false      # §8.7  never rewrap prose
license_header              = "// Copyright (c) Microsoft Corporation.\n// Licensed under the MIT License.\n"
require_license_header      = false      # §8.2  468/480 have it, but inserting one is a legal act

# ── Imports and preamble (§9) ──────────────────────────────────────────────────
sort_imports                = false      # §10.4  46.9% corpus / 79.2% library-with-3+
group_imports               = false      # §9.3  300/307 blocks are one contiguous run
blank_lines_after_imports   = 1          # §9.3  307/307
blank_lines_before_imports  = 1          # §9.3  303/307
blank_lines_after_license   = 1          # §7.5  470/480
blank_line_between_top_level_decls = 1   # §7.3  99.7% at indent 0
blank_line_after_open_brace = 0          # §7.2  13356 : 19
blank_line_before_close_brace = 0        # §7.2  12784 : 33

# ── Things kg fmt must never do ────────────────────────────────────────────────
align_assignments           = false      # §10.3  31/1690 — never introduce alignment
reorder_declarations        = false      # imports are side-effecting (Parser.hs:194)
wrap_expressions            = false      # §1.4  28 operator-terminated lines in 119782
normalize_string_literals   = false      # §10.10 tabs and {{ }} escapes are load-bearing
format_harness_files        = false      # §0.3  69 files in test/syntax are not translation units
```

Note on `list_style_call_args`: multi-line *call argument* lists (as opposed to *parameter*
lists) are 180 leading : 229 trailing over 1217 multi-line paren groups, of which 803 have no
comma at all. That population mixes real call sites with the parameter lists of function
*declarations* (`extern`, prototypes). The default above follows the parameter-list rule for
consistency; a project that disagrees can set `"trailing_comma"`. This is the one knob in the
table whose default is a judgement call rather than a measurement.

---

## 12. Ten worked before/after examples

Each drawn from a real corpus line, cited. "Before" is the byte-exact current content;
"After" is what `kg fmt` with the §11 defaults must produce.

### Example 1 — the only tab-indented file, plus a trailing space (`library/numeric/complex.k:8-14`)

Before (`⟶` = TAB, `␣` = trailing space):

```
//| Complex number
template<typename T>␣
struct complex
{
⟶T real_part;
⟶T imag_part;
}
```

After:

```
//| Complex number
template <typename T>
struct complex
{
    T real_part;
    T imag_part;
}
```

Three changes: tab → 4 spaces (§1.2), trailing space stripped (§1.5), `template<` →
`template <` (§6.7 — 1100 : 236 corpus-wide, 243 : 133 in `library/`; this is the weakest
normalization `kg fmt` performs, and the one most likely to be turned off by a project that
disagrees). The file also lacks a final newline (§1.6), added silently.

### Example 2 — trailing-comma export list becomes leading-comma (`library/numeric/int/multi_word.k:5-11`)

Before:

```
module numeric.int.multi_word
    { add_with_carry,
      sub_with_borrow,
      min_with_metadata,
      max_with_metadata,
      get_initial_comp_metadata
    }
```

After:

```
module numeric.int.multi_word
    { add_with_carry
    , sub_with_borrow
    , min_with_metadata
    , max_with_metadata
    , get_initial_comp_metadata
    }
```

This is the *only* trailing-comma export list in 208 (§3.1). The continuation indent moves
from 6 (aligned under the first item) to 4 (aligned under `{`), which is what 843 of 844
leading-comma continuations do.

### Example 3 — doc comments inside an export list must survive (`library/data/array.k:7-16`)

Before:

```
module data.array
    { array
    , //| # Constructing arrays
      indices
    , copy_array
    , repeat
    , singleton
    , iterate
    , //| # Element access
      at
```

After: **byte-identical.** This is one of the four "mixed comma" export lists (§3.1). A
naive leading-comma re-emitter would produce `, //| # Constructing arrays indices` or drop
the comment. The formatter must model an export-list item as `[doc-comment] identifier` and
keep a doc comment that precedes an item on its own line, indented to the item column
(`{`-indent + 2).

### Example 4 — the corpus's one K&R function definition (`test/library/control/async/rendezvous/lenient.k:36-38`)

Before:

```
public:
    void run(unit::tag_t tag)    {
        _rendezvous.start([tag](optional<pair<uint32, uint32>>[4] requests)
```

After:

```
public:
    void run(unit::tag_t tag)
    {
        _rendezvous.start([tag](optional<pair<uint32, uint32>>[4] requests)
```

6817 Allman function definitions to 2 K&R (§2.2). Note that the lambda argument on the next
line is *already* Allman (its `{` is on line 38 of the original) and must not be touched.

### Example 5 — K&R lambda in a `pipelined_for` (`test/library/numeric/int/multi_word.k:57-62`)

Before:

```
inline void add_with_carry_test(unit::tag_t tag)
{
    pipelined_for(16, [tag](uint8 itr){
        uint32 a = rng1(itr);
        uint32 b = rng2(itr);
        uint1 carry = itr % 2;
```

After:

```
inline void add_with_carry_test(unit::tag_t tag)
{
    pipelined_for(16, [tag](uint8 itr)
    {
        uint32 a = rng1(itr);
        uint32 b = rng2(itr);
        uint1 carry = itr % 2;
```

972 Allman lambdas to 23 K&R (§2.2). The brace moves to its own line at the indent of the
line that *started the statement* (4), not at the lambda's column — matching
`library/control/async.k:155-159`, where the `{` of a lambda argument sits at the lambda's
own indent.

### Example 6 — stray `;` after a class body (`library/control/async.k:197-202`)

Before:

```
    inline T dequeue()
    {
        return _returnFifo.dequeue();
    }
};


//| A wrapper around an array of `async_await` objects. ...
```

After:

```
    inline T dequeue()
    {
        return _returnFifo.dequeue();
    }
}


//| A wrapper around an array of `async_await` objects. ...
```

The `;` is dropped (§2.4: 1754 : 11). The **two** blank lines are kept (§10.1: 331 such
runs, 292 at indent 0). A formatter that collapsed them to one would produce a 331-site diff
for no gain.

### Example 7 — the aligned enum table (`library/processor/risc_v/isa.k:185-192`)

Before:

```
enum ABI : Reg
{ zero = 0   //< Hard-wired zero
, ra   = 1   //< Return address
, sp   = 2   //< Stack pointer
, gp   = 3   //< Global pointer
, tp   = 4   //< Thread pointer
, t0   = 5   //< Temporary/alternate link register
, t1   = 6   //< Temporary
```

After: **byte-identical.**

This violates three defaults at once — it is the only leading-comma enum body (58 : 1, §3.5),
its `{` hugs the first constant like an export list, and it aligns `=` (31/1690, §10.3).
It is nevertheless correct output, because §10.3 says a run of ≥2 lines sharing an `=` column
*and* a trailing-comment column is an alignment atom, re-emitted verbatim. The alternative —
reformatting it to `{\n    zero = 0,\n    ra = 1,\n …` — would destroy a 33-entry register
table that is read as a table.

### Example 8 — attribute placement is per-attribute (`library/compiler/hal.k:8-17`)

Before:

```
class fmul32
{
public:
    [[latency(LATENCY)]]
    float32 op(float32 x, float32 y);
}

[[name("_hardware_dsp__fmul32")]]
extern fmul32;
```

and `library/control/async.k:38-43`:

```
    {
    public:
        [[async]] void exec(task_t task, then_t then)
        {
            then(task());
        }
    }
```

After: **both byte-identical.** `attribute_placement = "preserve"` (§10.8). Under
`attribute_placement = "same_line"` the first would collapse to
`    [[latency(LATENCY)]] float32 op(float32 x, float32 y);` — legal, but it disagrees with
33/34 uses of `[[name(...)]]` and 53/53 uses of `[[schedule(...)]]`, so it is not the default.

### Example 9 — a long parameter list breaks with a leading `(` (`library/codec/crc.k:38-45`)

Before (already canonical):

```
    //| Process up to `MaxBytes` of new input data.
    inline uint<Width> compute
        ( count_t<ThreadCount> identifier   //< Which of the `ThreadCount` CRCs to update.
        , uint8[MaxBytes] input             //< Input data.
        , count_t<MaxBytes> input_bytes     //< Number of valid bytes in the input data.
        , bool reset                        //< Indicates if CRC value should be reset before processing `input`.  Should be set to `true` on the first call associated with a particular input stream.
        )
    {
```

After: **byte-identical**, even though line 42 is 179 columns wide. This is the interaction
that makes the formatter hard: `line_width = 100` is exceeded, but the overflow is entirely
inside a trailing `//<` doc comment, and §8.5 + §5.4 forbid reflowing it. The four `//<`
markers are column-aligned at 44; that alignment is preserved because §10.3 treats the run as
an atom.

Contrast the "after" a naive width-driven formatter would produce, which is **wrong**:

```
        , bool reset //< Indicates if CRC value should be reset before processing
                     // `input`.  Should be set to `true` on the first call
```

### Example 10 — `static assert` keeps its tight paren; `if` keeps its space (`library/base.k:48-57`)

Before (note: no final newline, and two trailing spaces on the last line):

```
// The device specific module `numeric.float32.operator` must expose the
// following functions which are used by the compiler to implement floating
// point operators.
static assert (decltype(F::mul) == (float32, float32) -> float32);
static assert (decltype(F::add) == (float32, float32) -> float32);
static assert (decltype(F::sub) == (float32, float32) -> float32);
static assert (decltype(F::mad) == (float32, float32, float32) -> float32);

// Check that the version of the device configuration schema is supported by the compiler.
static assert (version <= device_schema::schema_version);␣␣
```

After:

```
// The device specific module `numeric.float32.operator` must expose the
// following functions which are used by the compiler to implement floating
// point operators.
static assert(decltype(F::mul) == (float32, float32) -> float32);
static assert(decltype(F::add) == (float32, float32) -> float32);
static assert(decltype(F::sub) == (float32, float32) -> float32);
static assert(decltype(F::mad) == (float32, float32, float32) -> float32);

// Check that the version of the device configuration schema is supported by the compiler.
static assert(version <= device_schema::schema_version);
```

`static assert (` → `static assert(` (§6.7: 494 : 67 corpus, 156 : 8 library), trailing spaces
stripped (§1.5), final newline added (§1.6). The `-> ` spacing (949 : 141) and the tight
`decltype(` (239 : 0) and `::` (8014 : 0) are already correct.

### Bonus example 11 — a harness fixture that `kg fmt` must refuse (`test/syntax/imports.k:1-13`)

```
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

    inline void main()
    {
        static a1<10> x;
    }

expected:1

    import missing

expected:1
```

After: **byte-identical, with a diagnostic.** The file is 1072 lines holding 95 `expected:N`
markers and 142 indented `import` statements; it is not a translation unit (§0.3).
`kg fmt` detects `^expected:[0-9-]+$` or
`^warning:[a-z-]*$` at column 0 and skips the file with
`kg fmt: skipping test/syntax/imports.k: multi-case test harness format`.

---

## 13. The existing pretty-printer: what it does, and where it is broken

`compiler/hs/lib/Language/Kanagawa/PrettyPrint.hs`, 232 lines, exports two functions
(`PrettyPrint.hs:11-14`): `prettyExp` and `prettySource`.

### 13.1 Entry points and defaults

`prettyExp` (`PrettyPrint.hs:22-32`) is a **debug dump**, not a source printer: it is
`cata $ nest 4 . (line <>) . withDocs` over `viaShow`, i.e. it prints Haskell `Show` output
indented by 4. It is the only place in the module that reads the `Src.docs` field
(`PrettyPrint.hs:25-31`), and it wraps them in `dquotes`. Used by
`Main.hs:78` (`--dump-parse`) and `Main.hs:89` (`--dump-program`).

`prettySource :: Int -> NotedExp n e -> Doc ann` (`PrettyPrint.hs:37-38`) is the source
printer:

```haskell
prettySource n = histo src . desugar removeNestedSeq
```

Its driver is `Main.hs:45-55`:

```haskell
handle opt@PrettyPrint{..} _ = do
    results <- evalStateT (parseProgram $ getParseOptions opt) []
    ...
        else forM_ (rights results) (sourcePretty . fst) >> exitSuccess
  where
    sourcePretty = putStrLn . renderString . engine layout . prettySource indent . removeRedundantNameScope
    layoutOptions = PP.LayoutOptions (PP.AvailablePerLine line_width ribbon)
    engine Pretty = PP.layoutPretty layoutOptions
    engine Smart  = PP.layoutSmart  layoutOptions
```

CLI defaults (`Options/CmdArgs.hs:145-153`):

| flag | default | source |
|---|---|---|
| `--w`, `--line-width` | 80 | `CmdArgs.hs:147` |
| `--indent` | 4 | `CmdArgs.hs:148` |
| `--ribbon` | 1.0 | `CmdArgs.hs:149` |
| `--layout` | `Pretty` (vs `Smart`) | `CmdArgs.hs:150` |

There is **no in-place mode and no `--check` mode**: `Main.hs:52` is `putStrLn`, so output
always goes to stdout. There is no way to format a tree.

### 13.2 Layout it produces (accurate description)

Helper combinators (`PrettyPrint.hs:40-49`):

```haskell
braces           = PP.braces . (<> line) . nest n        -- `{` inline, body nested, `}` outdented
braces'          = PP.braces . nest n                    -- no trailing line: used only for initializer lists
commaSepList     = (nest n . group . (line' <>) . vsep . punctuate comma) . map pretty
commaLineSepList = (vcat . punctuate comma) . map pretty
tupledList       = parens . commaSepList
arrayDimList     = encloseSep lbracket rbracket (rbracket <> lbracket) . map pretty
statementList    = braces . hcat . map ((line <>) . statement)
fieldsList       = braces . hcat . map ((line <>) . semiTerm)
attributeList    = brackets . brackets . commaSepList
moduleName       = hcat . punctuate dot . map pretty
```

Consequences:

- **Brace style is inconsistent.** Allman for `class` (`:88`), `enum` (`:95`),
  `struct` (`:155`), `union` (`:174`), `switch` (`:156`), function bodies (`:107`),
  lambdas (`:116`) and `ModuleDecl` (`:129`) — all of which insert `<> line <>` before the
  braced group. **K&R for every control-flow statement**: `if` (`:118-119`),
  `static if` (`:120-121`), `for`/`RangeFor` (`:140`), `do…while` (`:94`),
  `static for` (`:175`), `unrolled_for` (`:176`) — all of which use `<+>` (a single space)
  before `statement b`. The corpus is 98.7% Allman *including* control flow (§2.2), so the
  pretty-printer disagrees with the house style on 1095 `if`, 307 `else`, 254 `for`, 468
  `static for`, 180 `static if` and 52 `do` sites.
- **Every list is trailing-comma.** `commaSepList` (`:42`) and `commaLineSepList` (`:43`)
  both use `punctuate comma`, which appends the separator. The corpus is leading-comma for
  module exports (129 : 1), template parameters (98 : 3) and function parameters (89 : 4)
  (§3). `ModuleDecl` (`:129`) uses `commaLineSepList`, so an export list round-trips as
  ```
  module a.b
  {
      X,
      Y
  }
  ```
  which matches neither the corpus (§3.1) nor itself elsewhere.
- **Attributes are always inline.** `attributes` (`:63-67`) is
  `attributeList x <> space` — an unconditional trailing space, never a newline. The 186
  own-line attribute occurrences (§4.2), including all 53 `[[schedule(…)]]` and 33 of 34
  `[[name(…)]]`, are collapsed onto the declaration line.
- **`template` never gets its own line.** `:159` is
  `"template" <+> angles (commaSepList a) <> line <> pretty b`.
- **Indent unit is the `--indent` value** (default 4) applied via `nest n`, matching §1.1.

### 13.3 Bug: all comments are dropped

`prettySource` (`:37-181`) never inspects the `Note`. Its algebra is `src`, which pattern
matches only on the syntax functor; the `Src` record carrying `docs` is consumed by `histo`
and discarded. The only `docs` reader in the file is `prettyExp` at `:25-31`.

It is worse than that: **non-doc comments never reach the AST at all.** The lexer's
whitespace consumer (`Lexer.hs:94-119`) *skips* comments:

```haskell
whitespaceAll = L.space space1 lineCmnt blockCmnt
  where
    lineCmnt  = L.skipLineComment "//"
    blockCmnt = L.skipBlockCommentNested "/*" "*/"
```

and the doc-preserving variant `whitespaceNonDoc` (`Lexer.hs:105-119`) still skips everything
that is *not* a `//|` / `//<` doc comment. So of the 10 108 `//` comments in the corpus, only
the 1115 `//|` and 545 `//<` doc comments are even representable — and only when
`--parse-docs` is on, which defaults to `def` = `False` (`CmdArgs.hs:67`).

Net effect: `kanagawa pretty foo.k` deletes 100% of the comments in `foo.k`.

**This alone makes the existing tool unusable as a formatter and is the primary reason
`kg fmt` must be written against a lossless CST, not the compiler AST.**

### 13.4 Bug: over-parenthesization

`PrettyPrint.hs:80`:

```haskell
src (Binary op e1 e2)  = parens (hang 0 (pretty e1 <> softline <> pretty op <+> pretty e2))
```

Every binary node is wrapped in parentheses unconditionally — no precedence comparison
against the parent. `a + b * c - d` round-trips as `((a + (b * c)) - d)`. With 3472 `==`,
1949 `+`, 322 `&&` and 301 `||` occurrences in the corpus (§6.1), the output is unreadable
and the diff against the input is total. (`LutMul` at `:79` is special-cased to
`lutmul(e1, e2)` and is correct.)

### 13.5 Bug list, complete, with citations

| # | line | symptom |
|---|---|---|
| 1 | `PrettyPrint.hs:78` | `src Barrier = "barier"` — **typo**. The keyword is `barrier` (`Lexer.hs:169`). Output does not reparse. 27 `barrier` occurrences in the corpus. |
| 2 | `PrettyPrint.hs:81` | `src (BitOffset a b) = "bitoffset" …` — the keyword is **`bitoffsetof`** (`Lexer.hs:171`, parser at `Parser.hs:864`: `BitOffset <$ open "bitoffsetof"`). Output does not reparse. 32 occurrences. |
| 3 | `PrettyPrint.hs:85` | `src (ByteOffset a b) = "byteoffset" …` — the keyword is **`byteoffsetof`** (`Lexer.hs:175`, `Parser.hs:865`). Output does not reparse. 10 occurrences. |
| 4 | `PrettyPrint.hs:37-181` | **All comments dropped** (§13.3). |
| 5 | `PrettyPrint.hs:80` | **Over-parenthesizes every binary expression** (§13.4). |
| 6 | `PrettyPrint.hs:176` | `src (UnrolledFor a b c) = "static for" …` — `unrolled_for` is printed as `static for`, silently renaming a distinct construct (`Lexer.hs:215` reserves `unrolled_for`; it has its own AST node distinct from `StaticFor` at `:175`). The corpus happens to contain 0 `unrolled_for`, so this is latent. |
| 7 | `PrettyPrint.hs:143` | `src (Reference a) = pretty a <> "&"` emits `T&`, which **the parser cannot read** — there is no `Reference` production in `Parser.hs` (§6.10). Reachable whenever a class-instance type is printed. |
| 8 | `PrettyPrint.hs:99` | `src Error{} = undefined` — **crashes** (⊥) on any `Error` node. Pretty-printing a file with a recoverable parse error aborts the process. |
| 9 | `PrettyPrint.hs:54`, `:67` | `from _ = undefined` in `fromSeq` and `attributes` — **crashes** if the child is not a `Seq`. Applies to `tupled`, `commaSep`, `commaLineSep`, `arrayDim`, `statements`, `fields`. |
| 10 | `PrettyPrint.hs:127`, `:154` | `InterpolatedString` and `StringLiteral` are re-emitted with `dquotes (pretty s)` — **no re-escaping**. A string containing `"` or `\` produces invalid output, and a string containing a literal `{` loses its `{{` doubling (`doc/programming-guide.md:572-573`). The corpus has 52 tab-bearing strings in `test/library/processor/risc-v.k` alone. |
| 11 | `PrettyPrint.hs:118-121`, `:140`, `:94`, `:175-176` | **K&R control flow** in an otherwise-Allman printer (§13.2), disagreeing with 98.7% of the corpus. |
| 12 | `PrettyPrint.hs:42-43`, `:129`, `:159` | **Trailing-comma lists** for module exports, template parameters and function parameters, where the corpus is 95–99% leading-comma (§3). |
| 13 | `PrettyPrint.hs:63-67` | **Attributes always inline**, never own-line (§4). |
| 14 | `PrettyPrint.hs:38`, `Main.hs:52` | The tree is **desugared before printing**: `desugar removeNestedSeq` (`Desugar.hs:132-137`, flattens nested `Seq`) and `removeRedundantNameScope` (`Frontend.hs:157-164`, strips qualifiers that resolve to the same symbol). Output is not the user's source. |
| 15 | `Parser.hs:607-614`, `:631-635` | Compounded with #14: `x++` is desugared to `x = x + 1` and `x += y` to `x = x + y` **at parse time**, so those forms cannot round-trip. 247 postfix `++`/`--` and 150 `+=` in the corpus. |
| 16 | `Parser.hs:967-968`, `PrettyPrint.hs:132` | `a ? b : c` is desugared to `Mux a (Seq [c, b])` at parse time and printed as `mux(a, c, b)`. 595 ternaries in the corpus become `mux` calls. |
| 17 |  `Parser.hs:194-206` | There is **no `Import` node** in the syntax (`grep -n Import compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs` → no matches). `parseImports` resolves and *inlines* imported files during parsing. Pretty-printing therefore emits the entire transitive import closure as one giant module, with no `import` statements. |
| 18 | `PrettyPrint.hs:147` | `isInternalQualifier` (`:191-195`) silently **drops** any scope qualifier whose name begins with `@` or equals `unresolvedName`. Correct for compiler-internal names; a formatter must not do it. |
| 19 | `PrettyPrint.hs:230-231` | `semiTerm`'s `term` returns `mempty` for `Variable … DeclareUninitConst`, so an uninitialized `const` declaration is printed **without its terminating `;`**. |
| 20 | `Options/CmdArgs.hs:145-153` | The `pretty` mode's record literal sets only `line_width`, `indent`, `ribbon`, `layout`. The other nine `PrettyPrint` fields (`base_library`, `define`, `dump_parse`, `files`, `import_dir`, `no_implicit_base`, `parse_docs`, `target_device`, `using`) are not initialized in the literal, yet `Main.hs:142-150` reads all of them via `getParseOptions`. Whether `cmdargs` fills them or this is a latent bottom is **UNVERIFIED** — see §14, probe P4. |

Non-bugs worth recording, so the Rust rewrite does not "fix" them:

- `PrettyPrint.hs:157-158` appends `break;` to every `case`/`default`. That is **correct**:
  the grammar requires it (`Parser.hs:602-603`).
- `PrettyPrint.hs:151` emits `static assert(` with no space. That **matches** the corpus
  majority (494 : 67, §6.7).
- `PrettyPrint.hs:204-206` prefixes `static` to a `DefaultInitialization` in *statement*
  position only; class-member position goes through `semiTerm` (`:149`) instead. Both forms
  exist in the grammar (`Parser.hs:346` and `Parser.hs:475`), so this is correct.
- `PrettyPrint.hs:170-172` prints `bitsizeof x`, `bytesizeof x` (space, no parens) and
  `clog2(x)` (parens). All three are legal; `Parser.hs:927-929` registers all three as
  `prefix'` keyword operators and `clog2` too, but the corpus writes all three with parens
  (287, 36, 112). A style difference, not a bug.

### 13.6 Verdict for `kg fmt`

`PrettyPrint.hs` is an AST *dumper* that happens to emit Kanagawa-shaped text. It cannot be
the basis of a formatter, for four independent reasons: it loses comments (#4), it loses
sugar (#14-16), it loses module structure (#17), and it emits three unparseable spellings
(#1-3, #7). `kg fmt` must be a **CST formatter**: lossless syntax tree, trivia attached to
tokens, and every rule in §1-§10 implemented over that tree. Nothing in `PrettyPrint.hs` is
reusable except its list of construct spellings, which is better taken from
`Lexer.hs:164-219` and `Syntax.hs:85-200` directly.

---

## 14. Open questions

Each item names the probe that settles it.

**Q1 — `line_width`: 80 or 100?**
The corpus says authors *aimed* at 80 (median per-file max = 80, §5.2) but hit it only half
the time (244/480 files, §5.2). The reference tool's default is 80 (`CmdArgs.hs:147`).
§11 proposes 100.
*Probe:* implement `kg fmt` with both, run over `library/` (124 files), and count changed
lines that are **not** comment-only:
```
kg fmt --line-width 80  --diff library/**/*.k | grep -c '^[+-][^+-]'
kg fmt --line-width 100 --diff library/**/*.k | grep -c '^[+-][^+-]'
```
Pick the smaller. This is the only §11 default whose evidence is ambiguous.

**Q2 — Is `list_style_call_args` leading or trailing?**
The 1217 multi-line paren groups mix call sites with declaration parameter lists; the raw
split is 180 leading : 229 trailing : 803 no-comma (§3, §11 note).
*Probe:* re-run the §3.3 classifier but require the matching `)` to be followed by `;`
(a call statement) rather than `{` (a definition), and re-split. If calls come out
trailing-dominant while definitions are leading-dominant, `kg fmt` needs two knobs, not one.

**Q3 — Does an aligned-comment run survive a width change?**
§10.3 says a run of ≥2 lines with column-aligned trailing comments is an atom. But
`library/codec/crc.k:42` is 179 columns (§12, example 9). If the atom's *code* part later
grows past `line_width`, does the atom re-align to a new column, or stay?
*Probe:* write a golden test: take `library/codec/crc.k:39-45`, lengthen one parameter name
by 10 characters, and assert the expected output. There is no corpus evidence either way —
this is a design decision the golden test must pin down.

**Q4 — Is `kanagawa pretty --parse-docs` reachable?**
`Options/CmdArgs.hs:145-153` does not initialize `parse_docs` in the `pretty` record, yet
`Main.hs:148` reads it. **UNVERIFIED** whether `cmdargs` supplies a default or this is a
`Missing field in record construction` bottom.
*Probe:* build the reference compiler and run
```
kanagawa pretty --help=all
kanagawa pretty --parse-docs library/data/fifo.k
```
If the second command crashes, bug #20 is real and doc comments are unreachable in `pretty`
mode; if it succeeds, check whether the emitted output contains `//|` (it should not — §13.3
says `prettySource` never reads `docs` regardless).

**Q5 — What does `kanagawa pretty` actually emit for a known input?**
Every claim in §13.2 is read off the combinator definitions, not observed. No compiler binary
exists in the tree (`find build -name 'kanagawa*' -type f` → empty; `which kanagawa` → not
found).
*Probe:* build per `BUILDING.md`, then
```
kanagawa pretty --no-implicit-base library/codec/crc.k > /tmp/crc.pretty.k
diff <(sed -n '20,50p' library/codec/crc.k) <(sed -n '1,40p' /tmp/crc.pretty.k)
```
This is the single highest-value probe in this document: it converts §13.2 from "derived from
source" to "observed", and gives `kg fmt` a byte-level parity target for the `--legacy-pretty`
compatibility mode if one is wanted.

**Q6 — Should `kg fmt` refuse harness files, or format the embedded programs?**
§0.3 says refuse. The alternative — parse each 4-space-indented block between `expected:`
markers as its own translation unit and format it in place — would bring 69 files
(2840 cases) under the formatter.
*Probe:* count how many of the 2840 embedded cases are syntactically complete programs:
```
awk '/^expected:/{if(n>0)c++; n=0; next} NF{n++} END{print c}' test/syntax/*.k
```
If most are complete, the ambitious option is viable; if many are deliberate syntax-error
fixtures (which the `expected:1` count suggests — `expected:1` means "expect failure"), it is
not.

**Q7 — Does `space_after_while = "preserve"` interact badly with `do; while(…)`?**
§6.9 shows `do ; while (…)` written three ways. `library/control/*` uses `do; while(...)`;
`test/logic/*` uses `do ; while (...)`.
*Probe:* enumerate every `do`/`while` pair and classify:
```
grep -rhoP 'do\s*;?\s*(\{)?.{0,4}while\s*\(' $(cat corpus480.txt) | sort | uniq -c | sort -rn
```
If the `do;`/`do ;`/`do {`/`do\n{` forms partition cleanly by subtree, `kg fmt` should
normalize within a file rather than globally.

**Q9 — Does import order ever change compilation?**
§9.2 shows 18 `library/` files whose import blocks are unsorted, with two recurring
"dependency-looking" orderings (`compiler.hal` before `compiler.config`). `parseImports`
merges symbols into the table as it goes (`Parser.hs:194-206`), and template-instantiation
parsing consults that table (`Parser.hs:655-684`), so order *can* matter in principle.
*Probe:* build the reference compiler, then for each of the 18 files sort its import block
and recompile:
```
for f in $(cat unsorted18.txt); do
  cp "$f" /tmp/orig.k
  awk '/^import /{a[n++]=$0; next} {if(n){asort(a); for(i=1;i<=n;i++) print a[i]; n=0} print}' /tmp/orig.k > "$f"
  kanagawa --backend=sv "$f" >/dev/null 2>&1 || echo "ORDER MATTERS: $f"
  cp /tmp/orig.k "$f"
done
```
If none fail, `sort_imports = true` becomes a defensible default for `library/`.
(Run this on a scratch clone — the reference tree is read-only.)

**Q8 — Is `library/` or the whole corpus the style authority?**
Several rules differ between `library/` and `test/`: import sorting (79.2% vs 14.3%, §9.2),
`while (` (39.1% vs 53.8%, §6.7), `return (` (61.8% vs 71.7%), initializer padding
(2:16 vs 138:266, §3.4). §11 uses corpus-wide numbers except where noted.
*Probe:* none needed from the compiler — this is a project decision. Recommendation: treat
`library/` as authoritative for anything where the two halves disagree by more than 20
percentage points, because `test/` is machine-generated in large stretches
(`test/logic/test_cases_*.k` is 104 files of sweeps).

---

## Appendix A — reproducing every measurement

```bash
KG=third_party/kanagawa
SP=/tmp/kgstyle && mkdir -p $SP && cd $KG

# corpora
find library test doc tools -name '*.k' | sort > $SP/corpus480.txt        # 480
grep -lE '^expected:[0-9-]+$' $(cat $SP/corpus480.txt) | sort > $SP/harness.txt   # 69
comm -23 $SP/corpus480.txt $SP/harness.txt > $SP/plain411.txt            # 411
grep '^library/' $SP/corpus480.txt > $SP/lib.txt                          # 124

# stripped copy (comments and string bodies blanked, offsets preserved)
python3 $SP/strip.py $SP/stripped < $SP/corpus480.txt
```

`strip.py` (the only tool needed beyond coreutils; reproduced in full so the measurements are
auditable):

```python
import sys, os
def strip(text):
    out=[]; i=0; n=len(text)
    while i<n:
        c=text[i]
        if c=='/' and i+1<n and text[i+1]=='/':
            j=text.find('\n',i);  j=n if j<0 else j
            out.append(' '*(j-i)); i=j
        elif c=='/' and i+1<n and text[i+1]=='*':
            j=text.find('*/',i+2); j=n if j<0 else j+2
            out.append(''.join(ch if ch=='\n' else ' ' for ch in text[i:j])); i=j
        elif c=='"':
            j=i+1
            while j<n and text[j]!='"':
                if text[j]=='\\': j+=1
                j+=1
            j=min(j+1,n)
            out.append('""'+' '*max(0,(j-i)-2)); i=j
        else:
            out.append(c); i+=1
    return ''.join(out)
for f in sys.stdin.read().split():
    t=open(f,encoding='utf-8',errors='replace').read()
    d=os.path.join(sys.argv[1], f); os.makedirs(os.path.dirname(d),exist_ok=True)
    open(d,'w',encoding='utf-8').write(strip(t))
```

Caveat for anyone re-running these: `strip.py` replaces a string literal's body with spaces,
so a naive "space before `;`" grep over the stripped corpus reports 185 false positives
(`= ""      ;`). The §6.9 measurement therefore runs over the **raw** corpus with
`grep -v '"' | grep -v '//'` and reports 5. Every measurement in this document states which
corpus it used.

Also: `LC_ALL=C` is required on any `sort`, `tr` or `cut` that touches the whole corpus,
because `test/syntax/imports/binary.k` is not UTF-8 (§0.4).

## Appendix B — headline numbers at a glance

| quantity | value |
|---|---|
| files | 480 (411 excluding harness fixtures) |
| lines | 119 782 (84 059 excluding harness fixtures) |
| indent unit | 4 spaces; 96.44% of indents are ≡0 (mod 4) |
| tab-indented files | 1 (`library/numeric/complex.k:12-13`) |
| CRLF files | 0 |
| files without a final newline | 86 |
| lines with trailing whitespace | 228 in 60 files |
| Allman `{` : K&R `{` | 13 375 : 173 (98.7%) |
| module export lists, leading : trailing comma | 129 : 1 |
| template parameter lists, leading : trailing | 98 : 3 |
| function parameter lists, leading : trailing | 89 : 4 |
| initializer lists, trailing : leading | 188 : 46 |
| enum bodies, trailing : leading | 58 : 1 |
| class base lists | 0 (the construct does not exist) |
| attributes: same-line : own-line : mid-line | 753 : 186 : 126 |
| p50 / p90 / p99 line width | 14 / 56 / 104 |
| median per-file max width | 80 |
| lines > 80 / > 100 / > 120 | 2.97% / 1.18% / 0.56% |
| `::` tight | 8014 : 0 |
| `, ` after comma | 42 784 : 8 |
| blank-line runs of 1 / 2 / 3 | 22 200 / 331 / 7 |
| blank line after `{` | 19 of 13 375 (0.14%) |
| blank line between top-level decls | 2764 of 2772 (99.7%) |
| files with the exact 2-line MIT header | 468 of 480 |
| `//|` on its own line | 1087 : 22 (98.0%) |
| `//<` trailing | 525 : 20 (96.3%) |
| doc-block continuations using bare `//` at the same indent | 2695 of 2696 |
| aligned trailing-comment runs | 144 of 164 (87.8%) |
| import blocks that are contiguous | 300 of 307 |
| import blocks sorted (`library/`, ≥3 imports) | 19 of 24 (79.2%) |
| PrettyPrint.hs lines | 232 |
| PrettyPrint.hs bugs enumerated | 20 |
