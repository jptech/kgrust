# Kanagawa diagnostics: output-format specification

Target: `kgrust` compat binary must produce stdout that satisfies
`kanagawa/test/syntax/syntax-test.py` and matches human expectations set by the
reference compiler.

Every claim below carries a `path:line` citation into the reference tree
(`third_party/kanagawa`, read-only). Claims that could not be
grounded in a file in that tree are explicitly marked **UNVERIFIED**.

Reference-tree revision at time of extraction: `8e0a3fb` (`git log --oneline -1`).

---

## 0. Executive summary (the five things that must not break)

1. **Everything goes to STDOUT.** There is no `std::cerr` in the C++ backend and
   no `stderr` use in the Haskell driver. The harness reads only
   `result.stdout` (`test/syntax/syntax-test.py:111`).
2. **Frontend errors** are one blob: literal `Error 1: ` immediately followed by
   the concatenated megaparsec `errorBundlePretty` text
   (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114`). *All* frontend errors
   are code 1; there are no other frontend codes.
3. **Frontend warnings** end their message line with ` [--Wno-<name>]`, and the
   harness anchors that with `$` (`test/syntax/syntax-test.py:122`). Nothing may
   follow on that physical line.
4. **Backend errors** are `Error <N> at: <file> (<line>): <msg>` on one line
   (`compiler/cpp/errors.h:110`), where `<N>` is a `CompileError` enum value.
5. **Exit code is 0 or 1 and nothing else.** The harness asserts exactly 1 on
   failure and exactly 0 on success (`test/syntax/syntax-test.py:158`).

---

## 1. Channels, colour, buffering, ordering

### 1.1 Channel

| Producer | Sink | Citation |
|---|---|---|
| Frontend errors / warnings / `Error 1: Warnings treated as errors` | stdout via `putStrLn` | `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:118`, `compiler/hs/app/Main.hs:93` |
| Backend errors and warnings (`ErrStream`) | `std::cout` | `compiler/cpp/errors.h:108`, `:114`, `:120` |
| Backend catch-all (`Error: <what()>`) | `std::cout` | `compiler/cpp/kanagawa.cpp:257` |
| Backend recursion error | `std::cout` | `compiler/cpp/enumerate_function_instances.cpp:1008` |
| `InitCompiler` failure | `std::cout` | `compiler/cpp/parse_tree.cpp:6294` |

`grep -rn "std::cerr" compiler/cpp/*.cpp compiler/cpp/*.h` returns nothing.
`grep -rn "stderr" compiler/hs/app compiler/hs/lib` returns nothing.

**The only stderr path** is the GHC runtime's uncaught-exception handler. It is
reachable: `parseFile` rethrows a top-level file failure as `FileException`
(`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:83`,
`compiler/hs/lib/Language/Kanagawa/Error.hs:73-78`). That produces
`<progname>: <message>` on **stderr** and exit code 1, with **no** `Error N` on
stdout. This only fires for the *top-level* source file / base library, never
for `import`ed files (those are turned into in-band `FileError` components; see
§3.5). See §11 (Open questions) for the probe.

### 1.2 Colour

`ErrStream` holds a `ChangeConsoleColor` RAII member (`compiler/cpp/errors.h:95-101`,
`:145`). On non-Windows `SetConsoleColor` is an empty function
(`compiler/cpp/platform.cpp:197`); on Windows it calls `SetConsoleTextAttribute`
(`compiler/cpp/platform.cpp:37`). **No ANSI escape sequences are ever written to
the stream on any platform.** The byte stream is plain ASCII/UTF-8.

> Rust rule: the legacy renderer must emit **no** SGR/ANSI codes when producing
> compat output, because the harness anchors a warning regex with `$`
> (`test/syntax/syntax-test.py:122`) and a trailing `\x1b[0m` would break it.

### 1.3 Interleaving and flush

The Haskell driver prints all frontend warnings, then `hFlush stdout`, then
calls into C++ (`compiler/hs/app/Main.hs:93-97`). The C++ side flushes in
`~ErrStream` (`compiler/cpp/errors.h:132`) and at both exits of `Codegen`
(`compiler/cpp/kanagawa.cpp:258`, `:264`). Therefore the observed stdout order is
always:

```
[frontend parse errors]           -> process exits
[frontend desugar errors]         -> process exits
[frontend warnings]               (Main.hs:93)
["Error 1: Warnings treated as errors" if --Werror]  -> process exits (Main.hs:95)
[backend warnings + errors]       (interleaved with backend progress output)
["Running internal consistency checks."]  (kanagawa.cpp:246)
```

### 1.4 Exit codes

| Situation | Code | Citation |
|---|---|---|
| Success | 0 (`exitSuccess`) | `compiler/hs/app/Main.hs:101` |
| Any frontend error | 1 (`exitFailure` inside `exitError`) | `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:119` |
| `--Werror` with >=1 frontend warning | 1 | `compiler/hs/app/Main.hs:94-95` |
| Backend returned false | 1 | `compiler/hs/app/Main.hs:102` |
| No source filenames | 1, message `Missing source filename(s)` (no `Error N` prefix!) | `compiler/hs/app/Main.hs:39` |

`exitFailure` in GHC is `ExitFailure 1`. The harness requires exactly 1
(`test/syntax/syntax-test.py:158`).

### 1.5 Non-diagnostic stdout noise the harness tolerates

These lines are emitted on successful compiles and must not accidentally match
`Error (\d+)`:

- `Running internal consistency checks.` — `compiler/cpp/kanagawa.cpp:246`
  (suppressed by `KANAGAWA_SKIP_CONSISTENCY_CHECKS`).
- `No codegen backend specified` — only when `Verbosity() > Normal`,
  `compiler/cpp/kanagawa.cpp:185`.
- `Compilation times:` + CSV — only when `Verbosity() > Normal`,
  `compiler/cpp/kanagawa.cpp:251`, `compiler/cpp/compiler.cpp:1546`.
- `Device name: <name>` — `compiler/cpp/config.cpp:159` (verbosity-gated).

---

## 2. The frontend error envelope

```haskell
exitErrors e = exitError $ "Error 1: " ++ concatMap errorBundlePretty e
exitError x = do
  putStrLn x
  exitFailure
```
`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:113-119`

Consequences, exactly:

- The literal prefix is `Error 1: ` — capital E, one space after the colon, and
  it is **glued directly onto the first byte of the first bundle**, i.e. onto
  the `file:line:col:` line. There is no newline between them.
- Multiple bundles are concatenated with **no separator**. Each bundle's text
  already ends in `\n`, so bundle *k+1* starts on a fresh line with no blank
  line between bundles.
- `putStrLn` adds one final `\n`, so the whole blob ends with `\n\n` (bundle
  text's trailing newline + `putStrLn`'s).
- Only the *first* bundle gets the `Error 1: ` prefix. Bundles 2..n start
  flush-left with their `file:line:col:` line.

Call sites of `exitErrors`:

| Stage | Citation |
|---|---|
| Pretty-print mode parse errors | `compiler/hs/app/Main.hs:49` |
| Compile mode parse errors | `compiler/hs/app/Main.hs:74` |
| Compile mode desugar/typecheck errors (deduped by `nub`) | `compiler/hs/app/Main.hs:82-83` |

Errors are grouped into one bundle per source file, sorted by offset within the
file (`compiler/hs/lib/Language/Kanagawa/Error.hs:169`:
`mapM (makeErrorBundle' . NE.sortBy (comparing offset)) . NE.groupBy (equating (srcFile . fst))`).
Note `groupBy` on a *list* (not a sort) — errors from the same file that are not
adjacent in the traversal order produce **multiple** bundles for that file.

---

## 3. `errorBundlePretty`: the exact byte shape

The frontend does not implement its own renderer. It uses megaparsec's
`errorBundlePretty` (imported at `compiler/hs/app/Main.hs:31`, used at
`compiler/hs/app/Main.hs:93` and `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114`).

Resolved dependency in this environment:
`megaparsec-9.7.0` (`~/.local/state/cabal/store/ghc-9.6.7/mgprsc-9.7.0-06501131/`),
constraint `megaparsec >= 9.0` (`compiler/kanagawa.cabal:48`, `:108`).

The megaparsec `.hs` sources are not present in the tree (only the compiled
`.a`), so the algorithm below is **reconstructed**. It is *validated
byte-for-byte* against the two captured compiler outputs checked into the tree
(`doc/programming-guide.md:240-244` and `doc/programming-guide.md:307-311`), and
its literal strings are confirmed present in
`.../mgprsc-9.7.0-06501131/lib/libHSmgprsc-9.7.0-06501131.a`
(`strings` hits: `unexpected `, `expecting `, `unknown parse error`,
`unknown fancy parse error`, `incorrect indentation (got `, `, should be `,
`less than `, `equal to `, `greater than `, `end of input`, `, or `, ` or `,
`<empty line>`, ` | `).

### 3.1 Bundle rendering

```
render(bundle):
    out = ""
    pst = bundle.posState                 # see §3.4 for its initial value
    for e in bundle.errors:               # already sorted by offset
        (maybe_line, pst) = reach_offset(e.offset, pst)
        epos = pst.sourcePos
        out += "\n"
        out += source_pos_pretty(epos) + ":\n"
        out += offending_line_block(maybe_line, epos, elen(e))
        out += parse_error_text_pretty(e)
    return out[1:]                        # drop the leading "\n"
```

So: **a blank line separates consecutive errors inside one bundle** (previous
chunk ends `\n`, next chunk starts `\n`), and the very first `\n` is dropped.

`source_pos_pretty(SourcePos name line col)`:
- if `name` is empty: `"<line>:<col>"`
- else: `"<name>:<line>:<col>"`

`name` is the file path passed to the parser / bundle. For the syntax harness
this is the **absolute canonicalised path** (`canonicalizePath` at
`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:105`; the parser is handed
`canonicalPath` at `:95`). For desugar-stage bundles the file comes from
`sourceName begin` of the `Src` note
(`compiler/hs/lib/Language/Kanagawa/Error.hs:198`).

### 3.2 The offending-line block

```
offending_line_block(None, _, _) = ""
offending_line_block(Some(sline), epos, elen):
    line_number = str(epos.line)          # 1-based, decimal
    padding     = len(line_number) + 1
    rpshift     = epos.col - 1            # 1-based col -> 0-based shift
    sline_len   = len(sline)
    pointer_len = (sline_len - rpshift + 1) if (rpshift + elen > sline_len) else elen
    pointer     = " "*rpshift + "^"*pointer_len
    return (" "*padding + "|\n"
          + line_number + " | " + sline + "\n"
          + " "*padding + "| " + pointer + "\n")
```

Verification against `doc/programming-guide.md:241-243` (markdown-indented by 4;
strip 4 spaces):

```
  |
3 |             uint32 z = x + y;
  |                        ^
```
`line_number="3"` -> `padding=2` -> `"  |"`. `epos.col=24` -> `rpshift=23` ->
`"  | " + 23 spaces + "^"` = `"  "` + `"|"` + 24 spaces + `"^"`. That is exactly
the captured bytes.

Verification against `doc/programming-guide.md:308-310`: `line_number="9"`,
`padding=2`, `epos.col=39`, `rpshift=38`, `"  |" + 39 spaces + "^"`. Exact match.

Note the asymmetry that trips people up: the gutter line before the source is
`" "*padding + "|"` (**no trailing space**), the caret line is
`" "*padding + "| "` (**with** trailing space before the pointer).

### 3.3 `elen` (caret width)

```
elen(TrivialError _ None    _)  = 1
elen(TrivialError _ Some(x) _)  = error_item_length(x)
elen(FancyError   _ xs)         = max(1, max(error_fancy_length(f) for f in xs))

error_item_length(Tokens ts) = len(ts)      # stream is String, Token = Char
error_item_length(Label _)   = 1
error_item_length(EndOfInput)= 1

error_fancy_length(ErrorFail _)          = 1
error_fancy_length(ErrorIndentation ...) = 1
error_fancy_length(ErrorCustom a)        = error_component_len(a)
```

The Kanagawa `ShowErrorComponent (ExtendedError s)` instance
(`compiler/hs/lib/Language/Kanagawa/Error.hs:68-71`) defines **only**
`showErrorComponent`, so `errorComponentLen` keeps its class default of `1`.

**Therefore every Kanagawa `FancyError` renders a single `^`.** All desugar
errors, all warnings, all import failures, and all file errors are `FancyError`
(`fancyError = errFancy offset . fancy . ErrorFail`,
`compiler/hs/lib/Language/Kanagawa/Error.hs:118-119`). Only raw parser
`TrivialError`s can produce a multi-character caret run, and only when the
unexpected item is `Tokens`.

### 3.4 `reach_offset`, tab expansion, and `linePrefix`

`PosState` is constructed in two different places with two different tab widths.

**(a) Parser bundles** (syntax errors) come from `runParserT`
(`compiler/hs/lib/Language/Kanagawa/Parser.hs:106`), whose initial `PosState`
uses megaparsec's `defaultTabWidth`, which is **8**. (Reconstructed; not
readable from this tree — see §11.)

**(b) Desugar / warning bundles** are built by `makeErrorBundle`
(`compiler/hs/lib/Language/Kanagawa/Error.hs:156-166`) with an explicit
`pstateTabWidth = mkPos 4` (`:163`), `pstateOffset = 0`,
`pstateSourcePos = initialPos file`, `pstateLinePrefix = ""`.

> **Trap:** the same source file with a literal TAB renders with different
> columns and different displayed indentation depending on whether the
> diagnostic came from the parser (width 8) or from the desugarer (width 4).
> A single-tab-width Rust implementation cannot be byte-identical on tabbed
> input. See §11.

Position advance while scanning the prefix `input[pstateOffset .. offset)`:

```
'\n'  -> line += 1; col = 1
'\t'  -> col = col + w - ((col - 1) mod w)     # next multiple of w, +1
else  -> col += 1
```

Displayed line = characters from the current position's line start up to (not
including) the next `'\n'`, with tabs expanded to width `w` **using the same
rule**, then:

```
if expanded == "": displayed = "<empty line>"
```

`linePrefix` handling: after each `reach_offset`, `pstateLinePrefix` is set to
the consumed part of the current line; when the *next* error lands on the same
line, that prefix is prepended so the full line is still shown. Practically:
two errors on the same line both display the whole line, with correct columns.

Source text is read with `readFileUtf8`, which sets `utf8_bom`
(`compiler/hs/lib/Language/Kanagawa/Internal.hs:24-32`) — a leading BOM is
stripped before offsets are computed. Columns are **character** counts, not byte
counts.

### 3.5 `parse_error_text_pretty` (the message part)

```
parse_error_text_pretty(TrivialError _ us ps):
    if us is None and ps is empty: return "unknown parse error\n"
    return message_items("unexpected ", {show_error_item(u) for u in us})
         + message_items("expecting ",  {show_error_item(p) for p in ps})

message_items(prefix, set):
    if set is empty: return ""
    return prefix + or_list(sorted(set)) + "\n"

or_list([x])        = x
or_list([x, y])     = x + " or " + y
or_list(xs)         = ", ".join(xs[:-1]) + ", or " + xs[-1]

parse_error_text_pretty(FancyError _ xs):
    if xs is empty: return "unknown fancy parse error\n"
    return "".join(show_error_fancy(f) + "\n" for f in sorted(xs))
```

`show_error_fancy`:

| Constructor | Rendering |
|---|---|
| `ErrorFail msg` | `msg` verbatim (may itself contain `\n`) |
| `ErrorIndentation ord ref act` | `"incorrect indentation (got " ++ show act ++ ", should be " ++ p ++ show ref ++ ")"` where `p` is `less than `/`equal to `/`greater than ` — never produced by Kanagawa |
| `ErrorCustom a` | `showErrorComponent a` (see below) |

**Set ordering is by `ErrorFancy`'s derived `Ord`:**
`ErrorFail < ErrorIndentation < ErrorCustom`. This is why the human-readable
message always prints *before* any attached `note:` block.

### 3.6 The three custom components

`compiler/hs/lib/Language/Kanagawa/Error.hs:52-58`, `:64-67`:

```haskell
data ExtendedError s = ImportError (ErrorBundle s)
                     | ErrorContext Int (ErrorBundle s)
                     | FileError String

showErrorComponent (ImportError x)    = "In imported file: " ++ errorBundlePretty x
showErrorComponent (ErrorContext _ x) = "note: " ++ errorBundlePretty x
showErrorComponent (FileError x)      = x
```

- **`ImportError`** — produced by `importFailure`
  (`compiler/hs/lib/Language/Kanagawa/Error.hs:121-122`), raised at
  `compiler/hs/lib/Language/Kanagawa/Parser.hs:202` when an imported file itself
  fails to parse. Renders as `In imported file: ` glued onto the nested bundle's
  own `file:line:col:` line, i.e. the nested caret block is indented by nothing
  but its first line carries the prefix.
- **`ErrorContext n bundle`** — produced only by `addErrorContext`
  (`compiler/hs/lib/Language/Kanagawa/Error.hs:192-195`) while flattening a
  `SrcStack` note. The nested bundle always contains exactly one error whose
  message is the literal string `while compiling`
  (`compiler/hs/lib/Language/Kanagawa/Error.hs:193`). Renders as
  `note: <nested bundle>`.
- **`FileError s`** — produced by `parseFileFailure` /
  `parseFileException` (`compiler/hs/lib/Language/Kanagawa/Error.hs:124-128`).
  Renders the string verbatim. Two producers:
  - `Can't find file: <name> in any of the import directories\n<dir>\n<dir>\n...`
    (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:108`; the directory list is
    `unlines importDirs`, so it already ends in `\n`, giving a trailing blank
    line after `unlines` in §3.5 adds another).
  - `Imported module name does not match, expected <a.b.c>`
    (`compiler/hs/lib/Language/Kanagawa/Parser.hs:206`).
  - `Circular dependency` (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:90`).

  `importDirs` order is
  `map (</> "device" </> targetDevice) importDir ++ importDir`
  (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:110`), i.e. every
  `--import-dir` with `/device/<target-device>` appended, then every
  `--import-dir` bare. Default `targetDevice` is `"mock"`
  (`compiler/hs/lib/Language/Kanagawa/Parser/Options.hs:47`).

`ExtendedError`'s `Ord` instance is deliberately partial
(`compiler/hs/lib/Language/Kanagawa/Error.hs:62-64`):
`compare (ErrorContext x _) (ErrorContext y _) = compare x y; compare _ _ = LT`.
Consequence: multiple `ErrorContext`s in one error print in increasing depth
order (outermost instantiation frame last, since `expandErrorStack` numbers from
0 at the innermost, `compiler/hs/lib/Language/Kanagawa/Error.hs:186-190`).

### 3.7 Note/context stack construction (secondary spans)

`Src` is either a leaf `Src {offset, begin, end, docs}`, a `SrcStack Src Src`,
or `SrcUnknown` (`compiler/hs/lib/Language/Kanagawa/Parser/Syntax.hs:610-621`).
`(<>)` builds right-nested stacks (`:623-626`).
`noteContext ctx e = NotedExp (noteOf e <> noteOf ctx) (expOf e)`
(`compiler/hs/lib/Language/Kanagawa/Desugar.hs:100-101`) — so the *primary*
location stays first and the context is appended.

`makeErrorBundles` (`compiler/hs/lib/Language/Kanagawa/Error.hs:168-206`) then:
1. groups by `srcFile` (head of the stack),
2. sorts by `offset` (head of the stack),
3. for a `SrcStack a b`, walks `b` producing one `ErrorContext k` per frame
   (`expandErrorStack`),
4. rewrites the sentinel offset: `FancyError (-1)` becomes `FancyError <head offset>`
   (`:183`).

`SrcUnknown` is `undefined` in `symbolLoc`, `srcFile`, and `offset`
(`compiler/hs/lib/Language/Kanagawa/Error.hs:106,113,190,199`) — reaching it crashes
the RTS with a `Prelude.undefined` message on **stderr**.

---

## 4. Frontend warning output

### 4.1 Construction

```haskell
warning offset kind msg e = copyNote (WarningF kind (fancyError offset message) e) e
  where message = "warning: " ++ msg ++ " [--" ++ disableSwitch kind ++ "]"
warningMsg = warning (-1)
```
`compiler/hs/lib/Language/Kanagawa/Error.hs:142-145`, `:153-154`

`disableSwitch k = "Wno-" ++ show k`
(`compiler/hs/lib/Language/Kanagawa/Warning.hs:43-44`), so the suffix is
` [--Wno-<name>]`.

### 4.2 Emission

```haskell
ws <- makeErrorBundles readFileUtf8 $ warnings (enabledWarnings warnings_state) untypedProgram
mapM_ (putStrLn . errorBundlePretty) ws
```
`compiler/hs/app/Main.hs:92-93`

- One `putStrLn` **per bundle** (per source file group), so each bundle's text
  (which already ends in `\n`) is followed by a blank line.
- Warnings are filtered by the enabled set *before* rendering
  (`compiler/hs/lib/Language/Kanagawa/Error.hs:94-99`), so a disabled warning
  produces no bytes at all.
- Warnings are emitted only in compile mode, only after the frontend produced no
  errors (`compiler/hs/app/Main.hs:82-85`).
- No `nub` is applied to warnings (contrast `nub $ errors` at
  `compiler/hs/app/Main.hs:82`) — duplicate warnings are printed as many times as
  they occur.

### 4.3 The exact line

```
warning: <message> [--Wno-<name>]
```

This is one element of the `FancyError` set, rendered by
`show_error_fancy (ErrorFail msg) = msg` and terminated by the `"\n"` that §3.5
appends. Because `ErrorFail` sorts before `ErrorCustom`, this line is emitted
**before** any `note:` block, so the `[--Wno-...]` really is at end-of-line and
`re.search(r'warning:.*\[--Wno-([^\]]*)\]$', line)` matches.

> **Rust rule:** never append a trailing space, a period, a colour reset, or a
> secondary-label suffix after `]` on that line.

---

## 5. The six warning kinds

`compiler/hs/lib/Language/Kanagawa/Warning.hs:20-35`:

```haskell
data WarningKind = Conversion | Deprecated | InitializerOverrides
                 | RedundantImport | Shadow | TransactionSizeMissing
show Conversion             = "conversion"
show Deprecated             = "deprecated"
show InitializerOverrides   = "initializer-overrides"
show TransactionSizeMissing = "transaction-size"
show RedundantImport        = "redundant-import"
show Shadow                 = "shadow"
```

| Kind | Switch names | Default | Emitter | Exact message | Firing condition |
|---|---|---|---|---|---|
| `Conversion` | `--Wconversion` / `--Wno-conversion`, also `--Wall` | **off** | Haskell | `` Narrowing conversion from `S` to `T` `` | `checkConversion'` reaches `s tIsConvertibleTo t` but not `s <: t`, in a context using `warningNarrowingConversions`: function-call arguments (`TypeCheck.hs:97`) and variable initialisers (`TypeCheck.hs:131`). `TypeCheck.hs:101`. |
| `Deprecated` | `--Wdeprecated` / `--Wno-deprecated` | **on** | **C++** | `Non-const global variables are deprecated, use class member variables instead` | Non-const declaration at global scope, when `_enableDeprecationWarnings`. `compiler/cpp/parse_tree.cpp:2383-2387` |
| `InitializerOverrides` | `--Winitializer-overrides` / `--Wno-initializer-overrides` | **on** | Haskell | `` Initializer overrides prior initialization of `NAME` `` | A designated initializer repeats an earlier designator in the same initializer list. `TypeCheck.hs:57-67` |
| `RedundantImport` | `--Wredundant-import` / `--Wno-redundant-import` | **on** | Haskell | `The import is redundant` | Parser marked an `Import` node `Redundant`. `compiler/hs/lib/Language/Kanagawa/Parser.hs:239` |
| `Shadow` | `--Wshadow` / `--Wno-shadow` | **on** | Haskell | `` This declaration of `NAME` shadows the existing declaration `` | A local `Variable`/`FuncParam` declaration with a non-empty scope name, not a member/class declaration, whose name resolves in the enclosing scope to a shadowable `Variable` (with non-empty scope) or a `FuncParam`. `TypeCheck.hs:110-121` |
| `TransactionSizeMissing` | `--Wtransaction-size` / `--Wno-transaction-size`, also `--Wall` | **off** | **C++** | `Call to function with [[last]] parameter without [[transaction_size]] at the call site` | `Enqueue` op of type `FunctionCall` with `transactionSize == 0` to a function that `HasEndTransactionParameter()`, when `_enableTransactionSizeWarning`. `compiler/cpp/lower.cpp:5415-5426` |

`enableSwitch k = "W" ++ show k`, `disableSwitch k = "Wno-" ++ show k`
(`compiler/hs/lib/Language/Kanagawa/Warning.hs:40-44`). Switch registration:
`compiler/hs/app/Options/CmdArgs.hs:123-135`.

### 5.1 Critical asymmetry

`Deprecated` and `TransactionSizeMissing` are **enabled by the Haskell flag
machinery but emitted by the C++ backend**, therefore they:

- use the `Warning <N> at: <file> (<line>): <msg>` shape (§7.2), not
  `warning: ... [--Wno-...]`;
- **never match** `test/syntax/syntax-test.py:122`;
- are unaffected by `Main.hs`'s `--Werror` early exit (§6).

The flag is marshalled at `compiler/hs/app/Options/FFI.hsc:135-136`:
```haskell
_enableTransactionSizeWarning = Set.member TransactionSizeMissing (enabledWarnings warnings_state)
_enableDeprecationWarnings    = Set.member Deprecated             (enabledWarnings warnings_state)
```

Corroboration: across all 72 registered syntax tests (`grep -c '^add_syntax_test' test/syntax/CMakeLists.txt`) the only asserted warning names are
`shadow`, `redundant-import`, `conversion`, `initializer-overrides`
(`grep -rho 'warning:[^ ]*' test/syntax`). Never `deprecated` or
`transaction-size`.

### 5.2 Enabled-set computation

```haskell
enabledWarnings = foldl (flip update) mempty
  where update (Enable a)  = Set.insert a
        update (Disable a) = Set.delete a
```
`compiler/hs/lib/Language/Kanagawa/Warning.hs:46-50`

It is an **ordered left fold**, so later flags win. The defaults list is the
`&= ignore` first alternative
(`compiler/hs/app/Options/CmdArgs.hs:112-117`):
`[Enable RedundantImport, Enable Deprecated, Enable InitializerOverrides, Enable Shadow]`.
`--Wall` appends `[Enable Conversion, Enable TransactionSizeMissing]`
(`:119-121`).

That the list *accumulates* (rather than replaces) is confirmed by
`test/syntax/CMakeLists.txt:40-45` (which always passes
`--Wno-redundant-import`) combined with `test/syntax/CMakeLists.txt:93` and
`:123` (which pass `--Wredundant-import` on top and *do* expect
`redundant-import` warnings in `test/syntax/imports.k` /
`test/syntax/options.k`).

---

## 6. `--Werror`

```haskell
when (not (null ws) && warnings_as_errors) $
    exitError "Error 1: Warnings treated as errors"
```
`compiler/hs/app/Main.hs:94-95`; flag declared at
`compiler/hs/app/Options/CmdArgs.hs:110` with `&= name "Werror" &= explicit`.

Exact behaviour:

1. Warnings are printed first (`Main.hs:93`), in full, with their normal
   `[--Wno-...]` suffixes.
2. Then the literal line `Error 1: Warnings treated as errors` is printed
   (via `putStrLn`, `Parser/IO.hs:118`) and the process exits **1**.
3. `ws` is the list of *bundles*; the check is on `not (null ws)`, i.e. at least
   one file produced at least one enabled warning.
4. **The C++ backend is never reached** in that case — `compile` is called only
   after this check.
5. `--Werror` also sets `_warningsAsErrors` for the backend
   (`compiler/hs/app/Options/FFI.hsc:137`). There, `WarningStream` sets
   `_compileError = true` (`compiler/cpp/compiler.cpp:183-185`), which later
   turns into `throw std::runtime_error("Function instance enumeration failed")`
   or `"IR generation failed"` (`compiler/cpp/kanagawa.cpp:118-133`), rendered as
   `Error: <msg>` (`kanagawa.cpp:257`) with exit 1. Note that path prints **no**
   `Error <digits>` line of its own — a `--Werror`-only C++ warning failure would
   leave `first_compile_error_code == 0` in the harness while exiting 1.

Tests exercising it: `test/syntax/CMakeLists.txt:97` (`all-warnings-as-errors`,
`--Wall --Werror`) and `:101` (`callbacks`, `--Wconversion --Werror`);
`test/syntax/all-warnings-as-errors.k` expects `expected:1` for each block.

---

## 7. The C++ backend diagnostic shapes

### 7.1 Errors — `ErrStream`

```cpp
ErrStream(fileName, lineNumber, error, callTrace)
    : _stream(std::cout), _changeConsoleColor(ConsoleColor::Red), _callTrace(callTrace)
{ _stream << "Error " << (uint32_t)error << " at: " << fileName << " (" << lineNumber << "): "; }

~ErrStream() { _stream << "\n"; _stream << _callTrace; _stream.flush(); }
```
`compiler/cpp/errors.h:106-133`

Byte shape:

```
Error <N> at: <fileName> (<lineNumber>): <streamed message><LF><callTrace>
```

- `<N>` is `static_cast<uint32_t>(CompileError)`, decimal.
- `<fileName>` is `GetSourceFileName(location._fileIndex)`
  (`compiler/cpp/compiler.cpp:178`), which is `_fileNames[fileIndex]` **without**
  path stripping (`compiler/cpp/compiler.cpp:1494-1501`) — i.e. the absolute
  canonical path handed over from Haskell.
- `<lineNumber>` is `location._beginLine`, a `size_t`
  (`compiler/cpp/parse_tree.h:36-44`). It is **1-based when valid and `0` for
  synthesised `Location{0}`** (e.g. `compiler/cpp/compiler.cpp:1426`, `:1434`).
  There is **no column** in backend diagnostics.
- The message is whatever was `operator<<`'d; it may contain embedded `\n`
  (e.g. `compiler/cpp/compiler.cpp:1417-1424`).
- `<callTrace>` is `Compiler::GetCallTrace()` captured at construction
  (`compiler/cpp/compiler.cpp:173-178`); it is `""` unless IR generation or
  function-instance enumeration is in progress.
- `_compileError = true` is set for every error (`compiler/cpp/compiler.cpp:177`).

### 7.2 Warnings — `ErrStream`

Two constructors (`compiler/cpp/errors.h:113-123`):

```
Warning <N> at: <fileName> (<lineNumber>): <message><LF>
Warning <N>: <message><LF>
```

The second (no location) is used only for `TooManyRams`
(`compiler/cpp/compiler.cpp:1035`). `_callTrace` is always `""` for warnings.
`_compileError` is set only when `--Werror`
(`compiler/cpp/compiler.cpp:183-185`).

### 7.3 The call trace block

```cpp
str << "  Call trace:\n";
...
str << "    " << name << " in file ";
str << LocationToString(callLoc, true /*stripLeadingPath*/, " at line ") << "\n";
```
`compiler/cpp/enumerate_function_instances.cpp:1810-1845`

Byte shape:

```
  Call trace:
    <Class>::<function> in file <basename> at line <N>
    <caller> in file <basename> at line <N>
    ...
```

- `name` is `GetUnmangledName()` for a global function, else
  `<Class>::<function>` (`:1826-1829`).
- `LocationToString(loc, true, " at line ")` = `StripLeadingPath(fileName) + " at line " + beginLine`
  (`compiler/cpp/ir.cpp:9085-9109`, `compiler/cpp/strip_leading_path.h:9-32`);
  it returns `""` if `!loc._valid`.
- The frame walk stops when `GetCaller` finds zero or >1 callers
  (`:1791`, `:1834-1844`).

> **Near miss with the harness:** `"  Call trace:".strip() == "Call trace:"`,
> whereas the harness tests for `"Call Stack:"`
> (`test/syntax/syntax-test.py:126`). Different word, different case. Safe — but
> only just. Do not "normalise" this to `Call Stack:` in Rust.

### 7.4 Special backend error shapes (not `ErrStream`)

These four shapes also reach stdout and are matched by
`re.search(r'Error (\d+)', line)`:

1. **Recursion** (`compiler/cpp/enumerate_function_instances.cpp:1008-1011`) —
   note: **no `at:`, no colon after the number**:
   ```
   Error 37 recursion detected in function: <flattened name>
   ```
   immediately followed by `throw std::runtime_error("Recursion is not allowed")`
   which surfaces as `Error: Recursion is not allowed` (§7.4.4).
2. **Hardcoded code in an exception message** (`compiler/cpp/ir.cpp:4914`, `:4964`):
   ```
   throw std::runtime_error("Error 27: Invalid object reference");
   throw std::runtime_error("Error 61: Invalid assignment to a const variable");
   ```
   which surface as `Error: Error 27: Invalid object reference`.
3. **`InitCompiler` failure** (`compiler/cpp/parse_tree.cpp:6283-6298`) — prints
   `e.what()` with **no prefix at all**, then returns false.
4. **Catch-all** (`compiler/cpp/kanagawa.cpp:255-262`):
   ```
   Error: <runtime_error::what()>
   ```
   `what()` strings reachable here include `Function instance enumeration failed`
   (`kanagawa.cpp:120`), `IR generation failed` (`kanagawa.cpp:133`),
   `Type checking failed` (`compiler/cpp/compiler.cpp:1372`),
   `Device capability check failed` (`compiler/cpp/compiler.cpp:1490`),
   `Debug symbols generation failed` (`compiler/cpp/symbols.cpp:534`),
   `Undefined function` (`compiler/cpp/compiler.cpp:374`),
   `Recursion is not allowed`, and any `RuntimeErrorWithTrace` (§8).
   Because `Error: ` has no digits, **it does not set
   `first_compile_error_code`**.

### 7.5 A real double-emission bug to reproduce or deliberately not reproduce

`compiler/cpp/parse_tree.cpp:3966-3973`:

```cpp
if (arguments.size() != functionDesc._parameterTypes.size())
{
    ErrStream errStream = g_compiler->ErrorStream(_location, CompileError::InvalidCall);
    g_compiler->ErrorStream(_location, CompileError::InvalidCall)
        << "Argument(" << arguments.size() << ")/Parameter(" << ... << ") count mismatch for: " ...;
}
```

The named `errStream` writes its header eagerly in the constructor and its `\n`
+ call trace in its destructor at end of block. Observed stdout is therefore:

```
Error 27 at: F (L): Error 27 at: F (L): Argument(2)/Parameter(3) count mismatch for: foo(uint32 a,uint32 b,uint32 c)
<callTrace>
<callTrace>
```

i.e. a doubled header on one line and a doubled call trace. The harness still
extracts `27`. The Rust port should emit the single, correct form; the
divergence is invisible to the harness (see §10.2).

### 7.6 Complete `CompileError` code table

`compiler/cpp/errors.h:6-84`. 76 values, max 84.
**Unassigned (gaps): 15, 23, 39, 42, 46, 48, 55, 80.**

| N | Name | N | Name | N | Name |
|---|---|---|---|---|---|
| 1 | SyntaxError | 30 | InvalidMember | 60 | DivideByZero |
| 2 | DuplicateFunction | 31 | InvalidExtern | 61 | AssignmentToConst |
| 3 | DuplicateType | 32 | UnknownIdentifier | 62 | InvalidInitializerList |
| 4 | UnknownType | 33 | DuplicateIdentifier | 63 | InvalidStaticLocal |
| 5 | UnknownFunction | 34 | InvalidAtomic | 64 | BasicBlockNotSchedulable |
| 6 | ExportTypeRestriction | 35 | CallRateNotPredicated | 65 | InvalidAttribute |
| 7 | InvalidGlobalAssignment | 36 | ExpressionNotStatic | 66 | InvalidExportClass |
| 8 | MultipleMemoryWrites | 37 | RecursionNotSupported | 67 | InvalidQuadPortMemory |
| 9 | InvalidArrayIndexing | 38 | InvalidWaitForCondition | 68 | InvalidRAMInitialization |
| 10 | PartialElementWrite | 40 | UnmatchedComment | 69 | InvalidEccFunction |
| 11 | InvalidLiteral | 41 | InvalidReset | 70 | EccUnsupported |
| 12 | UnorderedOperation | 43 | StaticAssert | 71 | AmbiguousDebugSymbols |
| 13 | InvalidSwitch | 44 | TypeToWide | 72 | InvalidMemoryReadDelay |
| 14 | InvalidConversion | 45 | TooManyReadPorts | 73 | MethodReturningReference |
| 16 | InvalidType | 47 | InvalidPerThreadUsage | 74 | InvalidExternFunction |
| 17 | InvalidArraySize | 49 | ReservedName | 75 | InvalidCallback |
| 18 | InvalidFunctionModifier | 50 | InvalidInspectionTarget | 76 | DeprecatedFeature |
| 19 | InvalidNesting | 51 | TooManyInspectableVariables | 77 | InvalidAssignment |
| 20 | InvalidFunctionReturnType | 52 | InspectableTooDeep | 78 | InvalidIntrinsicCall |
| 21 | InvalidReturn | 53 | InvalidString | 79 | InvalidTemplateArgument |
| 22 | InvalidIterationCount | 54 | InvalidConst | 81 | MissingDeviceConfigProperty |
| 24 | NonValueType | 56 | InvalidEndTransaction | 82 | InvalidDeviceConfigProperty |
| 25 | InvalidLoop | 57 | InvalidUpdateRate | 83 | QuadPortUnsupported |
| 26 | InvalidMux | 58 | InvalidClockSpecifier | 84 | MissingDeviceConfigSchema |
| 27 | InvalidCall | 59 | InvalidDebugView | | |
| 28 | InvalidContainer | | | | |
| 29 | ProtectionModifier | | | | |

Codes actually asserted by the syntax suite
(`grep -rho 'expected:[0-9]*' test/syntax`, 38 distinct values, counts in
parentheses):
0 (1768), 1 (834), 14 (27), 34 (24), 18 (23), 21 (16), 16 (14), 27 (13),
9 (10), 24 (10), 65 (9), 64 (9), 62 (7), 53 (7), 13 (7), 37 (6), 25 (6),
29 (5), 75 (4), 69 (4), 54 (4), 26 (4), 12 (4), 79 (3), 22 (3), 20 (3),
77 (2), 7 (2), 67 (2), 63 (2), 56 (2), 33 (2), 32 (2), 57 (1), 5 (1),
36 (1), 35 (1), 17 (1). (Plus one bare `expected:` which `int(... or 0)`
turns into 0, `test/syntax/syntax-test.py:90`.)

### 7.7 Complete `CompileWarning` code table

`compiler/cpp/errors.h:86-92`:

| N | Name | Emitted at | Gate |
|---|---|---|---|
| 8 | `TooManyRams` | `compiler/cpp/compiler.cpp:1035` | always (no `--W` flag) |
| 10 | `LastWithoutTransactionSize` | `compiler/cpp/lower.cpp:5424` | `--Wtransaction-size` / `--Wall` |
| 12 | `NonConstGlobal` | `compiler/cpp/parse_tree.cpp:2386` | `--Wdeprecated` (default on) |
| 13 | `FreeNonInlineFunction` | **never** — `grep -rn "FreeNonInlineFunction" compiler/` finds only the enum declaration | dead |

---

## 8. Internal compiler error and `Call Stack:`

### 8.1 What produces the trace

```cpp
class RuntimeErrorWithTrace : public std::runtime_error {
  RuntimeErrorWithTrace(const std::string& errorMessage)
    : std::runtime_error(errorMessage + "\nCompiler Internal Call Stack:\n" + GetStackTrace()) {}
};
```
`compiler/cpp/pch.h:47-54`. `GetStackTrace()` is
`boost::stacktrace::stacktrace()` stringified (`compiler/cpp/stack_trace.cpp:7-13`).

Producers of `RuntimeErrorWithTrace`:
- `OnAssertionFailure` (`compiler/cpp/compiler.cpp:1561-1577`) — the release-build
  `assert()` redirect (`compiler/cpp/ship_assert.h:5-8`). Message:
  `Internal compiler error: <file>:<line>` plus, when `g_compiler` exists,
  `"\n\n" + GetCallTrace()`. **It only throws when
  `GetCodeGenConfig()._releaseAssert` is true; otherwise the failed assertion is
  silently ignored** (`compiler.cpp:1564`). `_releaseAssert = not no_asserts`
  (`compiler/hs/app/Options/FFI.hsc:112`) and `no_asserts` defaults to false
  (`compiler/hs/app/Options/CmdArgs.hs:109`), so it is on by default.
- `MpToSizeT` overflow, `SafeLookup`, `SafeInsert`, `SafeInsertIdempotent`
  (`compiler/cpp/pch.h:100, 152, 190, 200, 225`).

All of them are caught by `catch (std::runtime_error& e)` in `Codegen`
(`compiler/cpp/kanagawa.cpp:255-262`) and printed as `Error: ` + `what()`.

### 8.2 Resulting byte shape

```
Error: Internal compiler error: <abs path>/ir.cpp:<line>

  Call trace:
    <Class>::<fn> in file <basename> at line <N>

Compiler Internal Call Stack:
<boost::stacktrace frames>
```

### 8.3 The harness check is currently dead — and must stay that way

`test/syntax/syntax-test.py:126` sets `internal_compiler_error = 1` iff a stdout
line satisfies `line.strip() == "Call Stack:"`. Repo-wide
(`grep -rn "Call Stack" --exclude-dir=thirdparty --exclude-dir=build`) the only
producer is the literal `"\nCompiler Internal Call Stack:\n"`
(`compiler/cpp/pch.h:51`), whose stripped form is
`Compiler Internal Call Stack:` — **not equal** to `Call Stack:`. So the check
never fires against the reference binary today.

> **Rust rules (hard requirements):**
> 1. **Never emit a stdout line whose stripped content is exactly `Call Stack:`.**
>    Any panic/backtrace header must be worded differently.
> 2. If reproducing the reference wording, use the exact string
>    `Compiler Internal Call Stack:` and keep the preceding blank line.
> 3. Prefer routing Rust panics/backtraces to **stderr** so they cannot
>    accidentally trip the check, but keep the process exit code at 1 — the
>    harness will then report `Expected error N Actual 0` rather than
>    `Unexpected internal compiler error`, which is a *less* informative failure.
>    Recommended compromise: print a one-line stdout marker that is not
>    `Call Stack:` (e.g. `Error: Internal compiler error: <site>`) and put the
>    backtrace on stderr.

---

## 9. Eleven real / derived example outputs

Examples 1 and 2 are **verbatim** from the tree. Examples 3-11 are **derived**
from the format strings plus a specific checked-in test input; each states its
derivation so it can be re-checked once a reference binary is buildable.

Throughout, `<T>` denotes the absolute canonical path of the harness temp file,
e.g. `/build/test/syntax/shadow-warning/syntax_test_temp_file.k`.

### Example 1 — narrowing-conversion warning (VERBATIM, `doc/programming-guide.md:240-244`)

```
test.k:3:24:
  |
3 |             uint32 z = x + y;
  |                        ^
warning: Narrowing conversion from 'uint33' to 'uint32' [--Wconversion]
```

> **Discrepancy:** the doc renders the type names in `'single quotes'`, but the
> source builds the message with backticks:
> ``"Narrowing conversion from `" ++ showUnmangled s ++ "` to `" ++ showUnmangled t ++ "`"``
> (`compiler/hs/lib/Language/Kanagawa/TypeCheck.hs:101`). The doc is stale/
> normalised for Markdown. **Trust the source: backticks.** Example 2 (same
> document, same section) *does* use backticks, confirming the doc is
> inconsistent rather than the source.

### Example 2 — invalid conversion, error (VERBATIM, `doc/programming-guide.md:307-311`)

```
Error 1: test.k:9:39:
  |
9 |         , {.is_valid = true, .value = -20}
  |                                       ^
Invalid conversion from `int6` to `uint32`
Insert an explicit cast to silence this issue `cast<uint32>()`
```

Derivation of the two-line message:
`conversionError s t hint = errorMsg (("Invalid conversion from `" ++ ... ++ "`") ++ hint)`
with `hint = "\nInsert an explicit cast to silence this issue `cast<" ++ ... ++ ">()`"`
(`compiler/hs/lib/Language/Kanagawa/TypeCheck.hs:99`, `:103`). It is a *single*
`ErrorFail` whose string contains an embedded newline; §3.5's `unlines`
terminates it.

### Example 3 — shadow warning with `note:` context (DERIVED)

Input: `test/syntax/shadow-warning.k` first block, lines 4-10, run under
`test/syntax/CMakeLists.txt:77` (no helper, so no import header). The harness
`.strip()`s every line before writing (`test/syntax/syntax-test.py:95`), so the
temp file is:

```
1  // Copyright (c) Microsoft Corporation.
2  // Licensed under the MIT License.
3
4  inline void main()
5  {
6  bool x;
7  {
8  uint32 x;
9  }
10 }
11
```

`bool x;` puts `x` at 6:6; `uint32 x;` puts `x` at 8:8. Primary span is the
inner declaration's name, context is the outer declaration
(`compiler/hs/lib/Language/Kanagawa/TypeCheck.hs:110-115` + `noteContext`).

```
<T>:8:8:
  |
8 | uint32 x;
  |        ^
warning: This declaration of `x` shadows the existing declaration [--Wno-shadow]
note: <T>:6:6:
  |
6 | bool x;
  |      ^
while compiling


```

(Two trailing blank lines: one from the `unlines` that terminates the
`ErrorCustom` element in §3.5, one from `putStrLn` at `Main.hs:93`.)

Harness effect: line 5 matches `warning:.*\[--Wno-([^\]]*)\]$` -> `shadow`;
`test/syntax/shadow-warning.k:12` says `warning:shadow`. Exit 0.

### Example 4 — missing import (DERIVED)

Input: `test/syntax/imports.k:11` block, run under `test/syntax/CMakeLists.txt:123`.
Temp file after the previous block's reset is `"\nimport missing\n\n"`, so the
`import` keyword is at offset 1 = 2:1. `getOffset` is taken *before* `keyword "import"`
(`compiler/hs/lib/Language/Kanagawa/Parser.hs:195-201`), and `elen` is 1 (§3.3).

```
Error 1: <T>:2:1:
  |
2 | import missing
  | ^
Can't find file: missing.k in any of the import directories
<SRC>/library/device/mock
<SRC>/test/syntax/imports/device/mock
<SRC>/test/syntax/device/mock
<SRC>/library
<SRC>/test/syntax/imports
<SRC>/test/syntax


```

Directory list = `unlines importDirs` with
`importDirs = map (</> "device" </> "mock") importDir ++ importDir`
(`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:108,110`); `importDir` order is
the command-line order `--import-dir=<SRC>/library`,
`--import-dir=<SRC>/test/syntax/imports` (`test/syntax/CMakeLists.txt:44,123`),
then `--import-dir <helperdir>` (`test/syntax/syntax-test.py:106`).
Exit 1; harness reads `Error 1` -> `expected:1` at `test/syntax/imports.k:13`. OK.

### Example 5 — error inside an imported file (`ImportError`) (DERIVED)

Input: `test/syntax/imports.k:413` (`import undefined`, `expected:1` at `:415`).
The imported fixture is:

```
test/syntax/imports/undefined.k
1  // Copyright (c) Microsoft Corporation.
2  // Licensed under the MIT License.
3
4  module undefined
5      { UndefinedSymbol
6      }
```

`checkSymbols` cannot resolve the re-exposed symbol and calls
`registerSymbolError x "undefined symbol"`
(`compiler/hs/lib/Language/Kanagawa/Parser.hs:223`), which prefixes the name:
`registerSymbolError e = registerFancyError (symbolOffset e) . (("`" ++ getName e ++ "`: ") ++)`
(`compiler/hs/lib/Language/Kanagawa/Error.hs:134`). `undefined.k` therefore
returns `Left bundle`, and `importFailure` wraps it
(`compiler/hs/lib/Language/Kanagawa/Parser.hs:202`,
`compiler/hs/lib/Language/Kanagawa/Error.hs:70`). `UndefinedSymbol` starts at
5:7.

```
Error 1: <T>:2:1:
  |
2 | import undefined
  | ^
In imported file: <SRC>/test/syntax/imports/undefined.k:5:7:
  |
5 |     { UndefinedSymbol
  |       ^
`UndefinedSymbol`: undefined symbol


```

There is no indentation of the nested block; only its *first* line carries
`In imported file: `. Two trailing blank lines, for the same reason as
Example 3.

> **Contrast — the `FileError` path.** `test/syntax/imports.k:15`
> (`import binary`) points at `test/syntax/imports/binary.k`, which is a 4-byte
> file containing `C0 DE DE AD` (`xxd`). `C0` is not a legal UTF-8 lead byte, so
> `readFileUtf8` (`compiler/hs/lib/Language/Kanagawa/Internal.hs:24-32`, encoding
> `utf8_bom`) throws while `evaluate (length s)` forces the contents; `Ex.try`
> converts it to `Left (displayException e)`
> (`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:106-107`) and it becomes a
> `FileError`, **not** an `ImportError` — so there is no nested caret block, just
> the raw exception text:
> ```
> Error 1: <T>:2:1:
>   |
> 2 | import binary
>   | ^
> <SRC>/test/syntax/imports/binary.k: hGetContents: invalid argument (cannot decode byte sequence starting from 192)
>
> ```
> The wording is GHC 9.6.6's, read off `kg-oracle --errors` on 2026-10-10
> (§12 item 10); 192 is `C0`, the first byte the decoder could not decode.

### Example 6 — `--Werror` (DERIVED, `test/syntax/all-warnings-as-errors.k:8-16`)

Options include `--Wall --Werror` (`test/syntax/CMakeLists.txt:97`), on top of
the suite default `--Wno-redundant-import`, so the enabled set is
`{Deprecated, InitializerOverrides, Shadow, Conversion, TransactionSizeMissing}`.

After the previous block's reset the temp file is:

```
1
2  inline void Foo(uint32 x)
3  {
4  }
5
6  inline void main()
7  {
8  int32 x;
9  Foo(x);
10 }
11
```

The warning is attached to the *argument* expression
(`checkArg` -> `checkConversion warningNarrowingConversions`,
`compiler/hs/lib/Language/Kanagawa/TypeCheck.hs:97`), i.e. `x` at 9:5. Its note
is a plain `Src`, not a `SrcStack`, so there is no `note:` block.

```
<T>:9:5:
  |
9 | Foo(x);
  |     ^
warning: Narrowing conversion from `int32` to `uint32` [--Wconversion]

Error 1: Warnings treated as errors
```

The blank line comes from `putStrLn` after the warning bundle
(`compiler/hs/app/Main.hs:93`); the `Error 1:` line comes from
`compiler/hs/app/Main.hs:95` via `exitError`. Exit 1.
The harness reads `Error 1` (the warning line has no `Error <digits>`), so
`expected:1` at `test/syntax/all-warnings-as-errors.k:18` passes.

### Example 7 — backend error with call trace (DERIVED, `compiler/cpp/errors.h:110` + `compiler/cpp/parse_tree.cpp:3958-3961`)

```
Error 27 at: /work/test/syntax/function-call/syntax_test_temp_file.k (12): Argument(2)/Parameter(3) count mismatch for: Foo(uint32 a,uint32 b,uint32 c)
  Call trace:
    Main::Bar in file syntax_test_temp_file.k at line 12
    main in file syntax_test_temp_file.k at line 4
```

Parameter list format from `FunctionDesc::GetParameterList`
(`compiler/cpp/parse_tree.cpp:6665-6676`): `(` + comma-separated
`<typeName> <paramName>` (no space after the comma), + `...`/`, ...` for
varargs, + `)`.

### Example 8 — backend warning (DERIVED, `compiler/cpp/errors.h:116` + `compiler/cpp/parse_tree.cpp:2386`)

```
Warning 12 at: /work/design.k (7): Non-const global variables are deprecated, use class member variables instead
```

and the locationless form (`compiler/cpp/errors.h:122` + `compiler/cpp/compiler.cpp:1035-1036`):

```
Warning 8: Unable to find mapping using fewer than 2160 Block RAMs
```

Neither matches the harness warning regex — no `warning:` and no `[--Wno-...]`.

### Example 9 — recursion (DERIVED, `compiler/cpp/enumerate_function_instances.cpp:1008-1011` + `compiler/cpp/kanagawa.cpp:257`)

```
Error 37 recursion detected in function: Main::Fib
Error: Recursion is not allowed
```

Note the missing `at:` and missing colon after `37`. `re.search(r'Error (\d+)')`
still yields 37, matching `expected:37` in the suite.

### Example 10 — internal compiler error (DERIVED, `compiler/cpp/compiler.cpp:1567-1576` + `compiler/cpp/pch.h:51` + `compiler/cpp/kanagawa.cpp:257`)

```
Error: Internal compiler error: /src/compiler/cpp/ir.cpp:5421

  Call trace:
    Main::Process in file design.k at line 88

Compiler Internal Call Stack:
 0# GetStackTrace() at /src/compiler/cpp/stack_trace.cpp:9
 1# OnAssertionFailure(char const*, unsigned long) at /src/compiler/cpp/compiler.cpp:1576
 2# ...
```

The stack-frame lines are boost's format and are **not** part of the contract.
The only contractual property is: **no line strips to `Call Stack:`**.

### Example 11 — device configuration failure (DERIVED, `compiler/cpp/compiler.cpp:1419-1436` + `compiler/cpp/kanagawa.cpp:257`)

Illustrates `Location{0}` -> line number `0`, and a multi-line `ErrStream`
message:

```
Error 81 at: /work/design.k (0): Missing device configuration properties:
  block_ram_depth
  lut_ram_depth
Error: Missing device configuration properties:
  block_ram_depth
  lut_ram_depth
```

---

## 10. Design: the `kg-diag` crate

### 10.1 Data model

```rust
// kg-diag/src/lib.rs

/// Stable, renderer-independent identity of a diagnostic class.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct DiagCode {
    /// Stable kgrust identifier, e.g. "KG0113". Used by `kg explain`,
    /// LSP `Diagnostic.code`, and #[allow]-style suppression in future.
    pub stable: &'static str,
    /// The number the legacy renderer must print. `Legacy::Frontend` => 1,
    /// `Legacy::Backend(n)` => n (the CompileError value).
    pub legacy: Legacy,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Legacy {
    /// Rendered by the frontend envelope: `Error 1: <bundle>`.
    Frontend,
    /// Rendered by the backend envelope: `Error <n> at: <file> (<line>): <msg>`.
    Backend(u32),
    /// Frontend warning; carries the WarningKind for the `[--Wno-...]` suffix.
    Warning(WarningKind),
    /// Backend warning: `Warning <n> at: ...` / `Warning <n>: ...`.
    BackendWarning(u32),
    /// No legacy number (e.g. `Error: <what>` catch-all, `Missing source filename(s)`).
    None,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Severity { Error, Warning, Note, Help }

/// Byte range in a specific file. Columns are derived, never stored.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Span { pub file: FileId, pub lo: u32, pub hi: u32 }

pub struct Label {
    pub span: Span,
    /// Rendered by the LSP renderer; the legacy renderer ignores label text
    /// for primary labels (the reference has no inline label text).
    pub text: Option<String>,
}

/// Reproduces megaparsec `ErrorContext` frames (template/instantiation stack).
pub struct Related {
    pub span: Span,
    /// Legacy renderer prints `note: <bundle>` where the nested bundle's
    /// message is this string. For instantiation frames it is exactly
    /// "while compiling".
    pub message: String,
    /// Frame depth; ties the ordering to megaparsec's `ErrorContext Int` Ord.
    pub depth: u32,
}

pub struct FixIt {
    pub span: Span,
    pub replacement: String,
    pub label: String,
}

pub struct Diagnostic {
    pub code: DiagCode,
    pub severity: Severity,
    /// May be `None` for diagnostics with no location (e.g. `--Werror` summary,
    /// `TooManyRams`, `Missing source filename(s)`).
    pub primary: Option<Span>,
    pub secondary: Vec<Label>,
    pub related: Vec<Related>,
    pub fixits: Vec<FixIt>,
    /// Free-form trailing lines. In the legacy renderer these are joined into
    /// the single message string with '\n', exactly like the reference's
    /// `hint` concatenation in TypeCheck.hs:103.
    pub notes: Vec<String>,
    pub message: String,
}
```

Invariants that keep legacy fidelity possible:

- `message` must be the *complete* human string minus the `warning: ` prefix and
  minus the `[--Wno-...]` suffix; the renderer adds both. This guarantees §4.3.
- `notes` entries are appended to `message` with `\n` by the legacy renderer, in
  order, before the terminating newline — matching Example 2.
- `related` is rendered only by the legacy renderer as `note: <nested block>`,
  sorted by `depth` ascending.
- `fixits` have **no** legacy rendering (the reference has none). They exist for
  LSP only. Emitting them in legacy mode would risk adding text after
  `[--Wno-...]`.

### 10.2 Renderer 1: `LegacyTerminal`

```rust
pub struct LegacyTerminal<'a> {
    pub sources: &'a SourceMap,
    /// 8 for parser-produced diagnostics, 4 for post-parse diagnostics.
    /// See §3.4 — the reference genuinely uses two different widths.
    pub tab_width: u8,
    /// Absolute canonical path, matching `canonicalizePath` in the reference.
    pub path_style: PathStyle,
}
```

Responsibilities, in the order they must be applied:

1. **Grouping.** Collect diagnostics into *bundles*: consecutive runs with the
   same `file`, in emission order (mirrors `NE.groupBy (equating srcFile)`,
   `compiler/hs/lib/Language/Kanagawa/Error.hs:169` — grouping, not sorting).
   Within a bundle, sort by `lo` ascending.
2. **Error envelope.** For a run of `Severity::Error` with `Legacy::Frontend`:
   emit `"Error 1: "`, then all bundles concatenated with no separator, then a
   single `\n`. Then exit 1.
3. **Warning envelope.** For each warning bundle: emit the bundle text then a
   single `\n` (the `putStrLn` per bundle).
4. **Bundle rendering.** Implement §3.1-§3.5 exactly. This is ~120 lines of
   Rust and must be its own module (`kg-diag/src/legacy/megaparsec.rs`) with
   golden tests.
5. **Backend envelope.** For `Legacy::Backend(n)`:
   `format!("Error {} at: {} ({}): {}\n", n, abs_path, line, msg)`, then the
   call-trace block if present. For `Legacy::BackendWarning(n)`: `Warning ...`
   with no call trace.
6. **Never** emit ANSI codes, never emit a line stripping to `Call Stack:`.

Explicitly **not** reproduced (allowed divergences, see §11.2):
- the doubled `Error 27 at:` header of §7.5;
- boost stack-frame formatting;
- exact `unexpected`/`expecting` wording for `TrivialError` (see §11.2).

### 10.3 Renderer 2: `Lsp`

Maps to `lsp_types::Diagnostic`:

| `kg-diag` | LSP |
|---|---|
| `primary` | `range` |
| `severity` | `severity` (`Error`/`Warning`/`Information`/`Hint`) |
| `code.stable` | `code: NumberOrString::String` |
| — | `codeDescription.href` = `kg://explain/<stable>` |
| `message` + `notes` joined by `\n` | `message` |
| `related` + `secondary` | `relatedInformation[]` (`location` + `message`) |
| `fixits` | published separately as `CodeAction` with a `WorkspaceEdit` |
| `Legacy::Warning(k)` | `data: {"disableSwitch": "--Wno-<k>"}` so the client can offer "disable this warning" |

The LSP renderer must **never** consult `Legacy` for text: the `[--Wno-...]`
suffix belongs to the legacy renderer only, and duplicating it into `message`
makes IDE output ugly.

### 10.4 `kg explain`

- Codes live in one generated table, `kg-diag/src/codes.rs`, produced by a build
  script from `kg-diag/codes.toml`.
- Namespacing:
  - `KG0001`..`KG0999` — frontend (all of which render as legacy `Error 1`).
    This is where the reference's total loss of information is recovered: the
    reference has *one* frontend code, kgrust assigns a distinct stable code per
    diagnostic-producing site.
  - `KGB0001`..`KGB0084` — backend, in **exact 1:1 correspondence with
    `CompileError`** (§7.6). `KGB0027` <-> `CompileError::InvalidCall`. The eight
    gaps (15, 23, 39, 42, 46, 48, 55, 80) are reserved and never allocated.
  - `KGW0001`..`KGW0006` — the six `WarningKind`s (§5), plus `KGWB0008`,
    `KGWB0010`, `KGWB0012` for the backend `CompileWarning`s. `KGWB0013`
    (`FreeNonInlineFunction`) is reserved-dead.
- `codes.toml` entry shape:
  ```toml
  [KGB0027]
  name        = "invalid-call"
  legacy      = { backend = 27 }
  cpp_enum    = "CompileError::InvalidCall"
  title       = "Invalid call"
  explain     = "docs/explain/KGB0027.md"
  ```
- `kg explain KGB0027` prints `docs/explain/KGB0027.md`.
  `kg explain 27` resolves via the `legacy.backend` index.
  `kg explain 1` is ambiguous by construction and must print a *list* of the
  `KG0xxx` frontend codes with their titles, plus a note that the reference
  compiler collapses them all to `Error 1`.
- A CI test asserts: every `Legacy::Backend(n)` used anywhere in the codebase
  has an entry; every entry has an `explain` file; no two entries share a
  `stable`; the backend table matches `errors.h` (kept as a checked-in fixture
  copy of `compiler/cpp/errors.h:6-93`).

---

## 11. Rendering-compatibility test plan

### 11.1 What MUST match (harness-observable)

These are the only four things `test/syntax/syntax-test.py` inspects. Each gets a
dedicated Rust integration test.

| # | Assertion | Harness citation | Rust test |
|---|---|---|---|
| T1 | For the *first* stdout line matching `Error (\d+)`, `\1` equals the expected code. All other lines must not match earlier. | `syntax-test.py:116-119`, `:137` | `compat::first_error_code` — replays all 2 843 `expected:N` blocks across the 73 top-level `test/syntax/*.k` files |
| T2 | For the *first* stdout line matching `warning:.*\[--Wno-([^\]]*)\]$`, `\1` equals the expected kind; and when `warning:0`, **no** line matches. | `syntax-test.py:121-124`, `:144-156` | `compat::first_warning_kind` — replays all 79 `warning:`-annotated blocks (32 named kinds, 39 `warning:0`, 7 bare `warning:`) |
| T3 | No stdout line satisfies `line.strip() == "Call Stack:"`. | `syntax-test.py:126-127`, `:134` | `compat::no_call_stack_marker` — asserted on *every* block, including deliberately-crashing fuzz inputs |
| T4 | Exit code is exactly 1 when an error is expected, exactly 0 otherwise. | `syntax-test.py:158` | `compat::exit_code` |

Derived hard constraints these imply, which deserve their own unit tests:

- **T1a** — the frontend envelope literal is `Error 1: ` and appears on the same
  physical line as `file:line:col:`. A renderer that put `Error 1:` on its own
  line still passes T1, but a renderer that emitted `error[KG0113]:` would fail.
  Test: `legacy::frontend_envelope_prefix`.
- **T1b** — no diagnostic line may contain the substring `Error <digits>` before
  the real one. Particular hazard: the echoed source line in the caret block. If
  a test source contains the literal text `Error 5`, the reference would also
  mis-fire, so kgrust must echo *exactly the same* source text, no more.
  Test: `legacy::source_echo_is_verbatim`.
- **T1c** — the backend envelope is `Error <n> at: ` with `<n>` immediately after
  a single space. Test: `legacy::backend_envelope`.
- **T2a** — nothing may follow `]` on the warning line: no trailing space, no
  ANSI reset, no CR. Test: `legacy::warning_suffix_is_eol` (asserts the rendered
  line matches `^warning: .*\[--Wno-[a-z-]+\]$` with `$` = true end of line).
- **T2b** — the warning message line must precede any `note:` block (megaparsec's
  `ErrorFail < ErrorCustom`, §3.5). Test: `legacy::warning_before_notes`.
- **T4a** — `Severity::Error` anywhere in the frontend must lead to exit 1 and no
  backend invocation. Test: `driver::error_short_circuits_backend`.

### 11.2 What MAY differ

Justification for each is that the harness cannot observe it.

| Divergence | Why it is safe |
|---|---|
| **Message wording of any frontend (`Error 1`) diagnostic** | T1 only reads the number `1`. `expected:1` blocks account for 834 of 2 843 assertions and none of them constrain text. |
| `unexpected`/`expecting` phrasing and token-set ordering for parse errors | Same as above; also depends on megaparsec `showTokens`/`Ord` internals not present in this tree (§11.4). |
| Caret run length for `TrivialError` (`elen` from `Tokens`) | Cosmetic only. Kanagawa `FancyError`s are always 1 (§3.3), so all *semantic* diagnostics already agree. |
| The doubled `Error 27 at:` header of §7.5 | The first match still yields 27. |
| Boost stack-frame formatting | Not inspected. |
| Extra informational stdout lines (`Running internal consistency checks.` etc.) | Not inspected, provided they never match `Error (\d+)` or the warning regex. |
| Absolute vs. relative paths in the `file:line:col:` prefix | Not inspected. **But** keep absolute+canonical to match human expectations and `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:105`. |
| `In imported file: ` nested-bundle layout | Not inspected. |

### 11.3 Golden-file tests (byte-exact, human-expectation tier)

Beyond the harness, keep a `tests/golden/` corpus rendered byte-exactly:

1. `narrowing_conversion_warning.txt` — must equal Example 1 modulo the
   backtick fix (§9 Example 1 discrepancy note).
2. `invalid_conversion_error.txt` — must equal Example 2 **byte for byte**; this
   is the only fully-trusted multi-line frontend error capture in the tree
   (`doc/programming-guide.md:307-311`).
3. `shadow_with_note.txt` — Example 3; locks in the `note:`/`while compiling`
   nesting and the two trailing blank lines.
4. `two_errors_same_bundle.txt` — locks in the single blank line between errors
   inside one bundle (§3.1) and the *absence* of a blank line between bundles
   (§2).
5. `empty_line_caret.txt` — an error whose offending line is empty; must render
   `<empty line>` (§3.4).
6. `tab_expansion_4.txt` / `tab_expansion_8.txt` — the same tabbed source
   rendered by the post-parse (width 4) and parser (width 8) paths (§3.4).
7. `caret_clamped_at_eol.txt` — an error at the last column, exercising
   `pointer_len` clamping (§3.2).
8. `backend_error_with_call_trace.txt` — Example 7.

A `--bless` mode regenerates them. Any diff in 1-8 is a review-required change.

### 11.4 Differential harness (once a reference binary can be built)

`tools/difftest`: run reference and kgrust on the same input, then compare with
a normaliser that:
- strips absolute path prefixes,
- collapses boost stack frames,
- **does not** normalise whitespace inside caret blocks.

Report three tiers: *identical*, *harness-equivalent* (T1-T4 agree, bytes
differ), *divergent*. Gate CI on zero *divergent* across `test/syntax/*.k`,
`test/library/**`, `test/interface/*.k`.

---

## 12. Open questions

Each names the probe program that would settle it. None is currently answerable
from the tree alone.

1. **Parser tab width.** Is the parser bundle's `pstateTabWidth` really
   megaparsec's `defaultTabWidth` (8) while post-parse bundles use 4
   (`compiler/hs/lib/Language/Kanagawa/Error.hs:163`)?
   *Probe:* a file containing `\tint x = ;` (parse error) and a second file
   containing `\tuint8 z = <narrowing expr>;` (post-parse warning); compare the
   reported columns and the rendered indentation of the echoed line.

2. **`<empty line>` reachability.** Does any real Kanagawa diagnostic ever point
   at an empty line? *Probe:* a file whose last line is empty and which triggers
   `Error 1` at EOF (e.g. unterminated `{`), and check whether the caret block
   shows `<empty line>`.

3. **Multi-bundle separator.** §2 claims no blank line between bundles from
   different files. *Probe:* one source with a desugar error and an imported
   file with a desugar error at the same pass, forcing two bundles from
   `makeErrorBundles`; inspect for a blank line between them.

4. **`errorComponentLen` default.** Confirmed only by absence of an override in
   `compiler/hs/lib/Language/Kanagawa/Error.hs:68-71`. *Probe:* an import failure
   (`FileError`) and confirm exactly one `^`.

5. **cmdargs list-accumulation for `warnings_state`.** §5.2 infers accumulation
   from `test/syntax/CMakeLists.txt` usage. *Probe:*
   `kanagawa --Wno-shadow --Wshadow f.k` on a shadowing source — a warning
   proves accumulation, silence proves replacement.

6. **stderr for top-level file failures.** §1.1 claims
   `kanagawa nonexistent.k` throws `FileException` to the RTS handler
   (stderr, exit 1, no stdout `Error N`).
   *Probe:* `kanagawa --base-library=... nonexistent.k 2>err 1>out`; inspect both
   streams and `$?`. If confirmed, kgrust should decide deliberately whether to
   improve on this (print `Error 1: Can't find file: ...` to stdout) — an
   improvement that is harness-visible and therefore must be coordinated with
   any test that relies on the current behaviour.

7. **Exit code on a C++ exception that is not a `std::runtime_error`.**
   `Codegen` catches only `std::runtime_error`
   (`compiler/cpp/kanagawa.cpp:255`); `std::bad_alloc` or `std::logic_error`
   would cross the FFI boundary and terminate the process with a signal, not
   exit 1. *Probe:* build with an injected `throw std::logic_error` and observe
   `$?`.

8. **Ordering of `showErrorItem` outputs.** `expecting` items are a `Set String`
   printed in ascending `String` order; the exact ordering for labels containing
   punctuation is `Ord String` (code-point) but this is unverified against real
   output. *Probe:* a parse error with >= 3 alternatives and record the order.

9. **Whether `--Werror` should also gate backend-only warnings before RTL is
   written.** Reference sets `_compileError` at `compiler/cpp/compiler.cpp:183`
   but only *checks* it after function-instance enumeration and IR generation
   (`compiler/cpp/kanagawa.cpp:118`, `:131`), so a `TooManyRams` warning raised
   later during placement would not fail the build. *Probe:* a design that
   over-subscribes block RAM, compiled with `--Werror`; check `$?`.

10. **`displayException` wording for a UTF-8 decoding failure.** *Answered
    2026-10-10* (`kg-oracle --errors`, GHC 9.6.6):
    `<path>: hGetContents: invalid argument (cannot decode byte sequence
    starting from N)`, N the first undecodable byte in decimal; an unreadable
    file is `<path>: openFile: permission denied (Permission denied)`. Example
    5's contrast block had guessed `(invalid byte sequence)`.
    *Probe:* `kanagawa --import-dir=test/syntax/imports f.k` where `f.k` is
    `import binary`; capture the exact line. Relevant because it is the only
    diagnostic whose text is produced by the GHC runtime rather than by
    Kanagawa, and a Rust port has to choose its own wording.

11. **Does `warning:` with an empty kind in a test file mean "any warning is
    acceptable"?** `test/syntax/syntax-test.py:87,92,151` shows `expected_warning
    = ""` short-circuits the warning check entirely, so those 7 blocks assert
    only exit code 0. Confirm no test intends stricter behaviour.
    *Probe:* none needed against the binary; this is a harness-semantics question
    for the kgrust test port.
