# The Kanagawa syntax-test conformance oracle

**Normative specification for `cargo xtask conformance` in the Rust rewrite.**

This document specifies, to the byte, what the reference harness
`kanagawa/test/syntax/syntax-test.py` does, what CMake registers it to do, and what
corpus of assertions it evaluates. It is written so that a Rust reimplementation can be
diffed against it line by line.

Reference tree: `third_party/kanagawa` (read-only).
All `path:line` citations below are relative to that root unless the path is already absolute.

Every number in this document was produced by re-running the reference regexes over the
corpus; the exact commands are reproduced in [§4.4](#44-how-the-numbers-were-produced).

---

## 0. Provenance and topology

| Thing | Where |
|---|---|
| Oracle script | `test/syntax/syntax-test.py` (173 lines) |
| Test registration | `test/syntax/CMakeLists.txt` (147 lines) |
| Test corpus | `test/syntax/*.k` — 73 files |
| Helper modules | `test/syntax/helper/*.k` — 44 files |
| Extra import roots | `test/syntax/imports/**` — 50 files |
| Error-code enum | `compiler/cpp/errors.h:6-84` (`enum class CompileError`) |
| Backend warning enum | `compiler/cpp/errors.h:86-92` (`enum class CompileWarning`) |
| Frontend warning kinds | `compiler/hs/lib/Language/Kanagawa/Warning.hs:20-27` |
| Compiler binary | `${CMAKE_BINARY_DIR}/dist/bin/kanagawa` — `compiler/hs/CMakeLists.txt:29`, imported as `kanagawa::exe` at `compiler/hs/CMakeLists.txt:135-138` |
| Subdirectory hookup | `test/CMakeLists.txt:4` (`add_subdirectory(syntax)`) — note this is **outside** the `if (VERILATOR_EXE)` guard at `test/CMakeLists.txt:7`, so syntax tests always register |

Two CMake convenience targets exist and are not part of the oracle semantics:
`syntax_tests` (a phony target that only forces `kanagawa_runtime` to build,
`test/syntax/CMakeLists.txt:12-14`) and `run_syntax_tests` (runs
`ctest -R "^syntax\."  --output-on-failure --verbose`, `test/syntax/CMakeLists.txt:17-22`).

---

## 1. The oracle algorithm (normative)

### 1.1 Command-line interface

`test/syntax/syntax-test.py:11-18` declares exactly five arguments:

| Flag | Required | Default | Meaning |
|---|---|---|---|
| `-kanagawa` | yes | — | path to the compiler executable (`syntax-test.py:12`) |
| `-testfile` | yes | — | path to the `.k` corpus file (`syntax-test.py:13`) |
| `-helper` | no | `None` | helper *module* name, not a path (`syntax-test.py:14`) |
| `-helperdir` | yes | — | directory passed to the compiler as an import root (`syntax-test.py:15`) |
| `-options` | no | `''` | one string, later split on ASCII whitespace (`syntax-test.py:16`) |

Note the single-dash spelling. `argparse` accepts `-kanagawa X`, `-kanagawa=X`, and
unambiguous prefixes; the Rust harness does not need to reproduce argparse's prefix
matching because CMake always passes the full names (`test/syntax/CMakeLists.txt:54-65`).

### 1.2 Constants and derived paths

```
TEMP_FILE_NAME = "syntax_test_temp_file.k"          # syntax-test.py:20
test_file_base = Path(testfile).stem                # syntax-test.py:28   e.g. "atomic_do"
done_path      = CWD / f"{test_file_base}.done"     # syntax-test.py:29
error_path     = CWD / f"{test_file_base}.error"    # syntax-test.py:30
subfolder      = CWD / test_file_base               # syntax-test.py:41-42  (mkdir exist_ok=True)
temp_file_path = subfolder / TEMP_FILE_NAME         # syntax-test.py:44
```

`CWD` is the ctest `WORKING_DIRECTORY`, i.e. `${CMAKE_CURRENT_BINARY_DIR}` for
`test/syntax` (`test/syntax/CMakeLists.txt:71`). Because every corpus file has a unique
stem, distinct tests never collide on `subfolder`, `.done`, or `.error`, so ctest may run
them in parallel.

### 1.3 Startup sequence

1. If `-testfile` is not an existing file: print `File does not exist: <path>` and
   `sys.exit(1)` (`syntax-test.py:23-25`). No `.error` file is written.
2. Unlink `<stem>.done` and `<stem>.error` if present, swallowing any exception
   (`syntax-test.py:32-36`).
3. Read the whole test file as **UTF-8** and `str.splitlines()` it (`syntax-test.py:38`).
   Python's `str.splitlines()` splits on `\n`, `\r\n`, `\r`, and additionally on
   `\v`, `\f`, `\x1c`, `\x1d`, `\x1e`, `\x85`, `U+2028` and `U+2029`; a trailing line
   terminator does **not** produce a final empty element. (The corpus is pure ASCII with
   LF endings — verified — so a Rust implementation may split on `\n` and drop one
   trailing empty element.)
4. `mkdir` the subfolder (`syntax-test.py:41-42`).
5. Compute the header line (`syntax-test.py:46-48`):
   ```
   imports = f"import helper.{helper}"   if -helper was given, else ""
   ```
6. `write_temp_header()` (`syntax-test.py:50-55`): **truncate** `temp_file_path`, open it
   with `encoding='ascii'`, and write `imports + "\n"` **only if `imports` is non-empty**.
   When `-helper` is absent the temp file is created empty (zero bytes).

Because the temp file is opened with `encoding='ascii'`, any non-ASCII byte in a snippet
would raise `UnicodeEncodeError` and crash the script with a traceback (exit 1) — an
uncaught path, since the `try` at `syntax-test.py:109` only wraps `subprocess.run`. The
current corpus is 100% ASCII, so this never fires.

### 1.4 The main loop

```python
for raw_line in lines:                                   # syntax-test.py:81
    line = raw_line.strip()                              # syntax-test.py:82
    expected_error = 0                                   # syntax-test.py:83
    expected_warning = ""                                # syntax-test.py:84

    error_match   = re.search(r'expected:(\d*)', line)   # syntax-test.py:86
    warning_match = re.search(r'warning:([^\s]*)', line) # syntax-test.py:87

    if error_match:                                      # syntax-test.py:89
        expected_error = int(error_match.group(1) or 0)  # syntax-test.py:90
    elif warning_match:                                  # syntax-test.py:91
        expected_warning = warning_match.group(1)        # syntax-test.py:92
    else:                                                # syntax-test.py:93
        append(line + "\n") to temp file                 # syntax-test.py:94-95
        continue                                         # syntax-test.py:96
    ... run the compiler and evaluate one assertion ...
    write_temp_header()                                  # syntax-test.py:166
```

Five properties of this loop are load-bearing and each has bitten a naive reimplementation:

1. **`line` is `raw_line.strip()`.** The *stripped* text is what gets appended to the
   snippet (`syntax-test.py:95`). All leading and trailing whitespace — including the
   4-space indentation that every corpus file uses — is destroyed before the compiler
   ever sees it. Column numbers in compiler diagnostics therefore do not correspond to
   the corpus file. `str.strip()` with no argument strips
   `\t \n \v \f \r space` plus Unicode whitespace.
2. **A marker line is never appended to the snippet.** The `continue` at
   `syntax-test.py:96` lives in the `else` branch; both marker branches fall through to
   the compile step and then to `write_temp_header()`, which *truncates*. A marker line
   is consumed, not stored.
3. **Reset-to-header, not reset-to-empty.** `write_temp_header()` at
   `syntax-test.py:166` rewrites the file to exactly the `import helper.<name>` line (or
   to nothing when there is no helper). Snippet *N+1* therefore starts from the header
   again and shares no state with snippet *N* — including the copyright comment block at
   the top of every corpus file, which is part of snippet 1 only.
4. **`expected_error` / `expected_warning` are re-initialised every iteration**
   (`syntax-test.py:83-84`), so a marker's expectation never leaks forward.
5. **The `if`/`elif` gives `expected:` strict precedence over `warning:`.** If a single
   line matched both patterns, only the error expectation would be evaluated and
   `expected_warning` would stay `""`, which disables the warning check entirely
   (see §1.8). *(No line in the current corpus contains both — verified.)*

### 1.5 Marker recognition — the exact regexes

Two Python `re.search` calls, i.e. **unanchored substring searches**, not full-line matches:

```
error marker:    expected:(\d*)
warning marker:  warning:([^\s]*)
```

Consequences that a Rust implementation must reproduce exactly:

* `\d*` matches **zero** digits. A bare `expected:` yields `group(1) == ""`, and
  `int("" or 0)` evaluates the `or` first: `"" or 0` → `0`, so `int(0)` → **`0`**
  (`syntax-test.py:90`). **`expected:` with no digits means "expect success", identical
  to `expected:0`.** *(The current corpus contains zero bare `expected:` markers — every
  occurrence carries digits — so this is latent behaviour that must nonetheless be
  implemented.)*
* `\d` in Python 3 `str` patterns is **Unicode-aware**: it matches any character with
  Unicode category `Nd`, e.g. `expected:٣` (Arabic-Indic three) would parse as 3 via
  `int()`. Rust's `\d` in the `regex` crate is Unicode-aware by default too, but
  `str::parse::<u32>()` rejects non-ASCII digits — use ASCII-only `[0-9]*` and note the
  divergence, or reproduce the Unicode behaviour. The corpus uses ASCII digits only.
* `\d*` is greedy but the search is unanchored, so `expected:12` yields `12`; a line
  `// see expected:14 above` would be a *marker*, not a comment. Because it is
  `re.search`, `expected:` anywhere on the line — including inside a string literal or a
  trailing comment — turns that line into a marker and the line is discarded.
* `[^\s]*` is greedy over non-whitespace, so `warning:conversion` yields `conversion`
  and `warning:` alone yields the empty string `""` — a *matched* marker with an empty
  value. Seven such lines exist (`imports.k:437,442,455,492,501,523,695`), and they mean
  "compile this snippet, require exit 0 and no `Error N`, but assert nothing about
  warnings" (§1.8).
* Integer overflow is impossible in Python; `expected:99999999999999999999` would parse
  fine and simply never match. Rust must use a wide type or saturate.

Empirically, all 2922 marker lines in the corpus are *canonical*: after `strip()` they
fully match `expected:\d*|warning:\S*` — verified, zero exceptions. A Rust harness may
therefore parse markers with an anchored full-line match **for the current corpus**, but
the substring semantics above are the normative rule.

### 1.6 The compiler invocation

Built fresh inside the loop for every marker (`syntax-test.py:103-106`):

```python
cmd = [args.kanagawa]
if args.options:
    cmd.extend(args.options.split())
cmd.extend(['--place-iterations=1', '--import-dir', args.helperdir, TEMP_FILE_NAME])

result = subprocess.run(cmd, capture_output=True, text=True, cwd=subfolder)
```

* `args.options.split()` is `str.split()` with no argument: splits on runs of arbitrary
  whitespace and discards empty fields. **An option containing a space is destroyed.**
  (This is why `--base-library helper/custom_base.k` at `test/syntax/CMakeLists.txt:91`
  survives — CMake joins the list with a space and Python splits it back into two argv
  entries — and why a `CMAKE_SOURCE_DIR` containing a space would break the harness.)
* `if args.options:` guards against the empty string, which would otherwise contribute
  nothing anyway; `''.split()` returns `[]`.
* `--place-iterations=1` is always appended, unconditionally, **after** the caller's
  options, so it cannot be overridden by `-options` (last-wins in `cmdargs` would
  actually let the later one win, and the later one is always `1`). Compiler default is
  3000 (`compiler/hs/app/Options/CmdArgs.hs:91`); forcing 1 makes placement effectively
  a no-op and keeps the tests fast.
* `--import-dir` is passed as **two argv entries** (`'--import-dir'`, `args.helperdir`),
  unlike the `--import-dir=` form CMake uses for the library root. `import_dir` is a
  list-valued option (`compiler/hs/app/Options.hs:73`, declared at
  `compiler/hs/app/Options/CmdArgs.hs:62`), so both accumulate.
* The final positional argument is the **bare filename** `syntax_test_temp_file.k`, not a
  path — it resolves against `cwd`.
* `cwd=subfolder` — the compiler runs inside `<build>/test/syntax/<stem>/`. Any output
  artefacts (e.g. the SystemVerilog emitted by `syntax.options`, which passes
  `--backend=sv`) land there. The output base name defaults to the stem of the last input
  file, i.e. `syntax_test_temp_file` (`compiler/hs/app/ParseTree.hs:48`).
* `capture_output=True, text=True` → stdout and stderr are captured separately and decoded
  with the locale's preferred encoding with universal-newline translation.
  **stderr is captured but never scanned** — only `result.stdout` feeds the matchers
  (`syntax-test.py:111`). stderr is only echoed into the `.error` file on failure
  (`syntax-test.py:69`).
* No timeout, no shell, no environment modification.

### 1.7 Output scanning

```python
output_lines = result.stdout.split('\n')                          # syntax-test.py:111
for output_line in output_lines:
    print(output_line)                                            # syntax-test.py:114
    if first_compile_error_code == 0:                             # syntax-test.py:116
        m = re.search(r'Error (\d+)', output_line)                # syntax-test.py:117
        if m: first_compile_error_code = int(m.group(1))          # syntax-test.py:118-119
    if first_compile_warning_code == "":                          # syntax-test.py:121
        wm = re.search(r'warning:.*\[--Wno-([^\]]*)\]$', output_line)  # syntax-test.py:122
        if wm: first_compile_warning_code = wm.group(1)           # syntax-test.py:123-124
    if output_line.strip() == "Call Stack:":                      # syntax-test.py:126
        internal_compiler_error = 1                               # syntax-test.py:127
exit_code = result.returncode                                     # syntax-test.py:129
```

**`Error (\d+)` — first match wins, and only the first.** The guard
`if first_compile_error_code == 0` freezes the value after the first hit. Note the
degenerate case: a compiler line literally containing `Error 0` would set the variable to
`0`, leaving the "not yet found" sentinel unchanged, so a later `Error 5` on a subsequent
line would still be picked up. No such output exists today.

The scan is *per line*, so `Error` and its number must be on the same output line. Every
producer satisfies this:

| Producer | Format | Citation |
|---|---|---|
| Haskell frontend, parse/desugar/typecheck errors | `Error 1: ` + megaparsec `errorBundlePretty` | `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114` |
| Haskell frontend, `--Werror` trip | `Error 1: Warnings treated as errors` | `compiler/hs/app/Main.hs:95` |
| C++ backend, normal diagnostics | `Error <n> at: <file> (<line>): <msg>` | `compiler/cpp/errors.h:110` |
| C++ backend, direct recursion report | `Error 37 …` written straight to `std::cout` | `compiler/cpp/enumerate_function_instances.cpp:1008` |
| C++ backend, exception escape | `Error: ` + `what()`; two `what()` strings themselves start `Error 27:` / `Error 61:` | `compiler/cpp/kanagawa.cpp:257`, `compiler/cpp/ir.cpp:4914`, `compiler/cpp/ir.cpp:4964` |

`Error: Function instance enumeration failed` (`compiler/cpp/kanagawa.cpp:257` +
`kanagawa.cpp:120`) does **not** match `Error (\d+)` — no digits follow `Error `. Such a
run exits 1 with `first_compile_error_code == 0`, so an `expected:0` marker fails on the
error comparison at `syntax-test.py:137` before the exit-code rule is ever reached.

**`warning:.*\[--Wno-([^\]]*)\]$` — anchored at end of line.** The `$` is essential:

* In Python, `$` matches at the end of the string *or* immediately before a trailing
  `\n`. Since the lines come from `split('\n')`, there is no trailing `\n`, so `$` is a
  true end-of-string anchor here. **A trailing `\r` (CRLF output on Windows, or a
  compiler that emits `\r\n`) defeats the match.** `text=True` performs universal-newline
  translation on the captured stream, which converts `\r\n` → `\n`, so in practice this
  is safe — but a lone `\r` inside a line would still break it.
* `.` does not match `\n`, but there are no `\n`s inside a split element, so this is moot.
* `.*` is greedy, so with multiple `[--Wno-…]` groups on one line the **last** one is
  captured. No current output does this.
* Porting note: in Rust's `regex` crate `$` (without `(?m)`) means end-of-haystack and
  does **not** get Python's "or just before a final `\n`" concession. To be exactly
  equivalent, feed the crate a line that carries no trailing newline — which is what
  splitting on `'\n'` already gives you. Do not enable `(?m)` and do not use `\z`
  reasoning to "improve" it; both change behaviour on a hypothetical multi-line
  haystack.

The matched text is produced by the **frontend only**
(`compiler/hs/lib/Language/Kanagawa/Error.hs:145`):

```haskell
message = "warning: " ++ msg ++ " [--" ++ disableSwitch kind ++ "]"
```

with `disableSwitch = ("Wno-" ++) . show`
(`compiler/hs/lib/Language/Kanagawa/Warning.hs:44`). The C++ backend's warnings are
formatted `Warning <n> at: <file> (<line>): <msg>` and `Warning <n>: <msg>`
(`compiler/cpp/errors.h:116`, `compiler/cpp/errors.h:122`) — capital `W`, no `[--Wno-…]`
suffix — so **backend warnings are invisible to this oracle**.

**The `Call Stack:` internal-error probe is dead code in the current tree.** The check
demands a line that `strip()`s to exactly `Call Stack:` (`syntax-test.py:126`). A
repo-wide search finds only two candidate strings, and neither matches:

* `"\nCompiler Internal Call Stack:\n"` — `compiler/cpp/pch.h:51`
  (`RuntimeErrorWithTrace`, thrown by `OnAssertionFailure` at
  `compiler/cpp/compiler.cpp:1561-1576`). `strip()` gives `Compiler Internal Call Stack:`.
* `"  Call trace:\n"` — `compiler/cpp/enumerate_function_instances.cpp:1814`.
  `strip()` gives `Call trace:`.

`internal_compiler_error` therefore never becomes 1 with today's compiler. A Rust
reimplementation **must still implement the check verbatim** (it is the oracle's
contract, and the string may be restored), but must not expect it to fire, and must not
"fix" it to match `Compiler Internal Call Stack:` — doing so would change which snippets
pass. See [Open questions](#10-open-questions).

### 1.8 Assertion evaluation — order and semantics

The four checks run in this fixed order; the **first** one that trips calls `fail()` and
`sys.exit(1)`, so later checks never run.

1. **Internal compiler error** (`syntax-test.py:134-135`)
   `if internal_compiler_error != 0:` → `"Unexpected internal compiler error"`.
2. **Error code** (`syntax-test.py:137-142`)
   `if expected_error != first_compile_error_code:` →
   `f"Unexpected result. Expected error: {expected_error} Actual: {first_compile_error_code}"`.
   This runs for **warning markers too**, where `expected_error` is `0` — so every
   `warning:` marker additionally asserts "no `Error N` on stdout".
3. **Warning** (`syntax-test.py:144-156`), a three-way dispatch:
   ```python
   if expected_warning == "0":
       if first_compile_warning_code != "":
           fail(f"Unexpected result. Expected no warning. Actual: '{first_compile_warning_code}'")
   elif expected_warning != "" and expected_warning != first_compile_warning_code:
       fail(f"Unexpected result. Expected warning: '{expected_warning}' Actual: '{first_compile_warning_code}'")
   ```
   | `expected_warning` | meaning |
   |---|---|
   | `"0"` | assert **no** frontend warning was emitted |
   | `""` | assert **nothing** — this is the value for every `expected:` marker *and* for the seven bare `warning:` markers |
   | anything else, e.g. `"shadow"` | assert the **first** frontend warning's `--Wno-` name equals it exactly (byte comparison, case-sensitive) |

   Note the asymmetry: a `warning:shadow` marker is satisfied if the *first* warning is
   `shadow`, regardless of how many further warnings follow; and `warning:0` is
   *stronger* than "no shadow warning" — it forbids **any** frontend warning.
4. **Exit code** (`syntax-test.py:158-163`)
   ```python
   if (expected_error != 0 and exit_code != 1) or (expected_error == 0 and exit_code != 0):
       fail(f"Failure! Expected compiler exit code {expected_error}, got {exit_code}")
   ```
   Restated: **expected code 0 ⇒ process exit status must be exactly 0; expected code
   ≠ 0 ⇒ process exit status must be exactly 1** — *not* the expected error code. The
   failure message printing `{expected_error}` as if it were an exit code is a cosmetic
   bug in the reference; reproduce the message text verbatim if you want byte-identical
   output, but the *rule* is the one above.

   The Haskell driver only ever calls `exitSuccess`/`exitFailure`
   (`compiler/hs/app/Main.hs:100-102`, `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:117-119`),
   so 0 and 1 are the only statuses a well-behaved run produces; a crash (signal) would
   surface as a negative `returncode` in Python and trip this rule.

### 1.9 Failure reporting

`fail()` (`syntax-test.py:58-78`) is the single exit path for a failed assertion:

1. `print(msg)` (`syntax-test.py:63`).
2. Write `<stem>.error` in the CWD, UTF-8, `newline=''` (no translation)
   (`syntax-test.py:65-72`): `msg + "\n"`, then — when a `CompletedProcess` is available —
   `"Stdout:\n{result.stdout}\n"` and `"Stderr:\n{result.stderr}\n"`, then a line of 50
   `-` characters, then the current temp-file contents read back as ASCII.
3. Echo `----- syntax_test_temp_file.k contents -----\n<contents>` to stdout
   (`syntax-test.py:76`).
4. `sys.exit(1)` (`syntax-test.py:78`).

An exception from `subprocess.run` itself is routed to the same path with
`result=None`: `f"Error running command: {e}"` (`syntax-test.py:131-132`). Note the
subsequent references to `result` at `syntax-test.py:135` etc. would raise
`UnboundLocalError` — unreachable, because `fail()` exits.

**A test aborts at its first failing assertion.** Assertion *k+1..n* of a file are never
evaluated once assertion *k* fails. This is the single most important behavioural
difference from the per-assertion Rust harness proposed in §9.

### 1.10 Success path

After the loop: `print("Success!")` and `Path(f"{stem}.done").touch()`
(`syntax-test.py:168-169`), exit status 0 (falling off `main`).

Note a corollary: **trailing non-marker lines are silently discarded.** If a file ends
with source lines after its last marker, that residue is accumulated into the temp file
and never compiled. Every corpus file ends with a marker, so nothing is currently lost.

### 1.11 Complete worked example

`test/syntax/declaration-order.k` (6 lines, no trailing newline) registered at
`test/syntax/CMakeLists.txt:95` with `HELPER declaration_order`:

```
1  // Copyright (c) Microsoft Corporation.
2  // Licensed under the MIT License.
3  (blank)
4      const auto Bar = Foo;
5  (blank)
6  expected:0
```

`write_temp_header()` writes `import helper.declaration_order\n`. Lines 1–5 are stripped
and appended. Line 6 is a marker: it is **not** appended. The compiler is invoked once,
in `<build>/test/syntax/declaration-order/`, on this exact file:

```
import helper.declaration_order
// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

const auto Bar = Foo;

```

(note the lost indentation on `const auto Bar`), and must exit 0 with no `Error N` line.
Then the temp file is truncated back to the single `import` line and the loop ends.

---

## 2. The exact argv, fully expanded

CMake builds the option string at `test/syntax/CMakeLists.txt:40-52`: four fixed
defaults, then the per-test `OPTIONS`, joined with single spaces.

```
OPTIONS = --jobs=1
          --Wno-redundant-import
          --base-library=${CMAKE_SOURCE_DIR}/library/mini-base.k
          --import-dir=${CMAKE_SOURCE_DIR}/library
          <extra OPTIONS from the add_syntax_test call, in declaration order>
```

The harness is then launched as (`test/syntax/CMakeLists.txt:54-65`):

```
python3 <SRC>/test/syntax/syntax-test.py
        -kanagawa  <BUILD>/dist/bin/kanagawa
        -testfile  <SRC>/test/syntax/<TEST_FILE>
        -helperdir <SRC>/test/syntax
        -options   "<OPTIONS joined by spaces>"
        [-helper   <HELPER>]                       # appended only if HELPER was given
```

with `WORKING_DIRECTORY = <BUILD>/test/syntax` (`test/syntax/CMakeLists.txt:71`) and
ctest name `syntax.<TEST_NAME>` (`test/syntax/CMakeLists.txt:31`).

Therefore the **compiler argv, in order**, is exactly:

```
argv[0]  <BUILD>/dist/bin/kanagawa
argv[1]  --jobs=1
argv[2]  --Wno-redundant-import
argv[3]  --base-library=<SRC>/library/mini-base.k
argv[4]  --import-dir=<SRC>/library
argv[5..k]  <extra OPTIONS, whitespace-split>
argv[k+1]  --place-iterations=1
argv[k+2]  --import-dir
argv[k+3]  <SRC>/test/syntax
argv[k+4]  syntax_test_temp_file.k
cwd        <BUILD>/test/syntax/<test-file-stem>/
```

Nothing is passed on stdin; no environment variables are set or cleared.

### 2.1 What each fixed default does

| Flag | Effect | Citation |
|---|---|---|
| `--jobs=1` | 1 GHC capability; makes diagnostic ordering deterministic | `compiler/hs/app/Options/CmdArgs.hs:61`, consumed at `compiler/hs/app/Main.hs:62-68` |
| `--Wno-redundant-import` | disables the `RedundantImport` warning, which is **on by default** | `compiler/hs/app/Options/CmdArgs.hs:133`; defaults at `CmdArgs.hs:112-117` |
| `--base-library=<SRC>/library/mini-base.k` | replaces the default `base.k` | `compiler/hs/app/Options/CmdArgs.hs:66`; selected at `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:73-75` |
| `--import-dir=<SRC>/library` | first import root | `compiler/hs/app/Options/CmdArgs.hs:62` |
| `--place-iterations=1` | placement iterations (default 3000) | `compiler/hs/app/Options/CmdArgs.hs:91` |
| `--import-dir <SRC>/test/syntax` | second import root, makes `helper.*` and `imports/*` resolvable | `syntax-test.py:106` |

Default-enabled warnings are `RedundantImport`, `Deprecated`, `InitializerOverrides`,
`Shadow` (`compiler/hs/app/Options/CmdArgs.hs:112-117`); `--Wall` additionally enables
`Conversion` and `TransactionSizeMissing` (`CmdArgs.hs:119-121`). Because
`--Wno-redundant-import` is a *default* for every syntax test, the three tests that care
about redundant imports re-enable it explicitly with `--Wredundant-import`
(`test/syntax/CMakeLists.txt:93`, `:123`) — and `cmdargs` applies the states in argv
order via a left fold (`compiler/hs/lib/Language/Kanagawa/Warning.hs:46-50`), so the
later `--Wredundant-import` wins.

### 2.2 Import resolution used by these argv

`import a.b.c` becomes the relative path `a/b/c.k`
(`compiler/hs/lib/Language/Kanagawa/Parser.hs:198`, `addExtension (joinPath ns) "k"`),
searched with `findFile (dir : importDirs)` where `dir` is the directory of the
*importing* file and

```haskell
importDirs = map (</> "device" </> targetDevice opt) (importDir opt) ++ importDir opt
```

(`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:110`). With the default
`--target-device=mock` (`compiler/hs/app/Options/CmdArgs.hs:87`) the effective search
order for a syntax test is:

```
<cwd of the temp file>                 # <BUILD>/test/syntax/<stem>/
<SRC>/library/device/mock
<SRC>/test/syntax/device/mock          # does not exist
<SRC>/library
<SRC>/test/syntax
```

plus `<SRC>/test/syntax/imports` (and its `device/mock` variant) for `syntax.imports`.
Two module names are synthesised in-memory rather than read from disk:
`.cmdargs` (carrying `--define` and `--using` values) and `.options` (carrying `backend`,
`stall`, `optimize`, `max_threads_default`, `max_threads_limit`) —
`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:100-102`, built at `IO.hs:49-65`.
`test/syntax/options.k` exercises the latter.

---

## 3. Complete inventory of registered tests

72 `add_syntax_test(...)` calls, `test/syntax/CMakeLists.txt:76-147`, one per line, in
declaration order. No call uses the `TEST_FILE` override (`CMakeLists.txt:33-37`), so
every test file is `<TEST_NAME>.k`. "assertions" = number of marker lines in that file =
number of compiler invocations that test performs when it passes.


| # | CMakeLists line | ctest name | test file | `-helper` | extra OPTIONS (appended after the 4 defaults) | assertions |
|--:|--:|---|---|---|---|--:|
| 1 | 76 | `syntax.base-test` | `base-test.k` | — | `--no-implicit-base=false` `--template-iterations=1` | 1 |
| 2 | 77 | `syntax.shadow-warning` | `shadow-warning.k` | — | — | 14 |
| 3 | 78 | `syntax.interpolated-string` | `interpolated-string.k` | `interpolated_string` | — | 54 |
| 4 | 79 | `syntax.string` | `string.k` | `string` | — | 29 |
| 5 | 80 | `syntax.return-type-polymorphism` | `return-type-polymorphism.k` | `return_type_polymorphism` | `--Wconversion` | 27 |
| 6 | 81 | `syntax.assignment` | `assignment.k` | — | — | 2 |
| 7 | 82 | `syntax.literal-operand` | `literal-operand.k` | `literal_operand` | `--Wconversion` `--define=ELEVEN#11` | 12 |
| 8 | 83 | `syntax.type-constexpr` | `type-constexpr.k` | `type_constexpr` | — | 51 |
| 9 | 84 | `syntax.type-identifier` | `type-identifier.k` | `type_identifier` | — | 24 |
| 10 | 85 | `syntax.empty-struct` | `empty-struct.k` | — | — | 2 |
| 11 | 86 | `syntax.extern-class` | `extern-class.k` | `extern_class` | — | 55 |
| 12 | 87 | `syntax.function-type` | `function-type.k` | — | — | 20 |
| 13 | 88 | `syntax.dependent-template` | `dependent-template.k` | `dependent_template` | — | 17 |
| 14 | 89 | `syntax.designated-initializer` | `designated-initializer.k` | `designated_initializer` | — | 42 |
| 15 | 90 | `syntax.name-attr` | `name-attr.k` | — | — | 10 |
| 16 | 91 | `syntax.custom-base` | `custom-base.k` | — | `--no-implicit-base=false` `--base-library` `helper/custom_base.k` | 1 |
| 17 | 92 | `syntax.statement` | `statement.k` | — | — | 14 |
| 18 | 93 | `syntax.options` | `options.k` | — | `--Wredundant-import` `--backend=sv` `--stall=3` `--target-device=ultrascaleplus-xcu250` | 7 |
| 19 | 94 | `syntax.function-call` | `function-call.k` | — | — | 5 |
| 20 | 95 | `syntax.declaration-order` | `declaration-order.k` | `declaration_order` | — | 1 |
| 21 | 96 | `syntax.doc-comments` | `doc-comments.k` | — | `--parse-docs` | 28 |
| 22 | 97 | `syntax.all-warnings-as-errors` | `all-warnings-as-errors.k` | — | `--Wall` `--Werror` | 3 |
| 23 | 98 | `syntax.memory` | `memory.k` | `memory` | `--target-device=mock` | 31 |
| 24 | 99 | `syntax.conversion-warning` | `conversion-warning.k` | — | `--Wconversion` | 20 |
| 25 | 100 | `syntax.no-warning` | `no-warning.k` | — | `--Wno-shadow` | 2 |
| 26 | 101 | `syntax.callbacks` | `callbacks.k` | `callbacks` | `--Wconversion` `--Werror` | 94 |
| 27 | 102 | `syntax.template-template-parameter` | `template-template-parameter.k` | `template_template_parameter` | — | 39 |
| 28 | 103 | `syntax.template-param-default` | `template-param-default.k` | `template_param_default` | — | 40 |
| 29 | 104 | `syntax.quad-port-memory` | `quad-port-memory.k` | — | — | 2 |
| 30 | 105 | `syntax.export-class` | `export-class.k` | `export_class` | — | 28 |
| 31 | 106 | `syntax.scheduling` | `scheduling.k` | — | — | 7 |
| 32 | 107 | `syntax.static-for` | `static-for.k` | — | — | 21 |
| 33 | 108 | `syntax.static-if` | `static-if.k` | `static_if` | — | 74 |
| 34 | 109 | `syntax.loops` | `loops.k` | — | — | 15 |
| 35 | 110 | `syntax.this-capture` | `this-capture.k` | — | — | 19 |
| 36 | 111 | `syntax.literals` | `literals.k` | — | — | 40 |
| 37 | 112 | `syntax.decltype` | `decltype.k` | — | — | 21 |
| 38 | 113 | `syntax.auto-placeholder` | `auto-placeholder.k` | `auto_placeholder` | — | 36 |
| 39 | 114 | `syntax.template-argument-deduction` | `template-argument-deduction.k` | `template_argument_deduction` | — | 221 |
| 40 | 115 | `syntax.template-argument-deduction-from-instance` | `template-argument-deduction-from-instance.k` | `template_argument_deduction_from_instance` | — | 12 |
| 41 | 116 | `syntax.function-auto-return` | `function-auto-return.k` | `function_auto_return` | — | 20 |
| 42 | 117 | `syntax.struct-constexpr` | `struct-constexpr.k` | `struct_constexpr` | — | 13 |
| 43 | 118 | `syntax.higher-order-functions` | `higher-order-functions.k` | `higher_order_functions` | — | 211 |
| 44 | 119 | `syntax.lambdas` | `lambdas.k` | — | — | 52 |
| 45 | 120 | `syntax.closures` | `closures.k` | — | — | 33 |
| 46 | 121 | `syntax.local-functions` | `local-functions.k` | `local_functions` | — | 35 |
| 47 | 122 | `syntax.function-constants` | `function-constants.k` | `function_constants` | — | 39 |
| 48 | 123 | `syntax.imports` | `imports.k` | — | `--Wredundant-import` `--import-dir=${CMAKE_CURRENT_SOURCE_DIR}/imports` | 120 |
| 49 | 124 | `syntax.initializer-list` | `initializer-list.k` | `initializer_list` | — | 88 |
| 50 | 125 | `syntax.pipelined` | `pipelined.k` | — | — | 15 |
| 51 | 126 | `syntax.template` | `template.k` | — | — | 47 |
| 52 | 127 | `syntax.type-check` | `type-check.k` | `type_check` | — | 63 |
| 53 | 128 | `syntax.struct-template` | `struct-template.k` | `struct_template` | — | 6 |
| 54 | 129 | `syntax.constexpr` | `constexpr.k` | `constexpr` | `--define=FOO_CONST#10` | 180 |
| 55 | 130 | `syntax.using` | `using.k` | `aliases` | — | 40 |
| 56 | 131 | `syntax.member-function-template` | `member-function-template.k` | `member_function_template` | — | 35 |
| 57 | 132 | `syntax.function-template` | `function-template.k` | `function_template` | — | 21 |
| 58 | 133 | `syntax.function-template-param` | `function-template-param.k` | `function_template_param` | — | 37 |
| 59 | 134 | `syntax.enum` | `enum.k` | `enum` | — | 59 |
| 60 | 135 | `syntax.const-correctness` | `const-correctness.k` | `const_correctness` | — | 23 |
| 61 | 136 | `syntax.nested-templates` | `nested-templates.k` | `nested_templates` | — | 19 |
| 62 | 137 | `syntax.template-instance-identifiers` | `template-instance-identifiers.k` | `template_instance_identifiers` | — | 41 |
| 63 | 138 | `syntax.struct` | `struct.k` | `structs` | — | 11 |
| 64 | 139 | `syntax.union` | `union.k` | — | — | 4 |
| 65 | 140 | `syntax.union-template` | `union-template.k` | `union_template` | — | 6 |
| 66 | 141 | `syntax.local-classes` | `local-classes.k` | — | — | 17 |
| 67 | 142 | `syntax.local-structs` | `local-structs.k` | `local_structs` | — | 14 |
| 68 | 143 | `syntax.static-locals` | `static-locals.k` | `static_locals` | — | 6 |
| 69 | 144 | `syntax.function-attributes` | `function-attributes.k` | — | — | 23 |
| 70 | 145 | `syntax.atomic` | `atomic.k` | — | — | 51 |
| 71 | 146 | `syntax.last` | `last.k` | — | — | 14 |
| 72 | 147 | `syntax.atomic_do` | `atomic_do.k` | — | — | 32 |
| | | **72 tests** | | | | **2516** |

### 3.1 Machine-readable registry

The block below is byte-for-byte derived from `test/syntax/CMakeLists.txt:76-147` and is
intended to be embedded in the Rust harness (`include_str!` + `serde_json`, or converted
to a `const` table by a build script). `<SRC>` is the reference tree root; substitute at
runtime.

```json
{
 "source": "kanagawa/test/syntax/CMakeLists.txt:76-147",
 "count": 72,
 "default_options": ["--jobs=1", "--Wno-redundant-import", "--base-library=<SRC>/library/mini-base.k", "--import-dir=<SRC>/library"],
 "trailing_argv": ["--place-iterations=1", "--import-dir", "<SRC>/test/syntax", "syntax_test_temp_file.k"],
 "tests": [
  {"ctest_name": "syntax.base-test", "stem": "base-test", "test_file": "base-test.k", "helper": null, "extra_options": ["--no-implicit-base=false", "--template-iterations=1"], "cmake_line": 76, "assertions": 1, "expected_markers": 1, "warning_markers": 0},
  {"ctest_name": "syntax.shadow-warning", "stem": "shadow-warning", "test_file": "shadow-warning.k", "helper": null, "extra_options": [], "cmake_line": 77, "assertions": 14, "expected_markers": 0, "warning_markers": 14},
  {"ctest_name": "syntax.interpolated-string", "stem": "interpolated-string", "test_file": "interpolated-string.k", "helper": "interpolated_string", "extra_options": [], "cmake_line": 78, "assertions": 54, "expected_markers": 54, "warning_markers": 0},
  {"ctest_name": "syntax.string", "stem": "string", "test_file": "string.k", "helper": "string", "extra_options": [], "cmake_line": 79, "assertions": 29, "expected_markers": 29, "warning_markers": 0},
  {"ctest_name": "syntax.return-type-polymorphism", "stem": "return-type-polymorphism", "test_file": "return-type-polymorphism.k", "helper": "return_type_polymorphism", "extra_options": ["--Wconversion"], "cmake_line": 80, "assertions": 27, "expected_markers": 24, "warning_markers": 3},
  {"ctest_name": "syntax.assignment", "stem": "assignment", "test_file": "assignment.k", "helper": null, "extra_options": [], "cmake_line": 81, "assertions": 2, "expected_markers": 2, "warning_markers": 0},
  {"ctest_name": "syntax.literal-operand", "stem": "literal-operand", "test_file": "literal-operand.k", "helper": "literal_operand", "extra_options": ["--Wconversion", "--define=ELEVEN#11"], "cmake_line": 82, "assertions": 12, "expected_markers": 0, "warning_markers": 12},
  {"ctest_name": "syntax.type-constexpr", "stem": "type-constexpr", "test_file": "type-constexpr.k", "helper": "type_constexpr", "extra_options": [], "cmake_line": 83, "assertions": 51, "expected_markers": 51, "warning_markers": 0},
  {"ctest_name": "syntax.type-identifier", "stem": "type-identifier", "test_file": "type-identifier.k", "helper": "type_identifier", "extra_options": [], "cmake_line": 84, "assertions": 24, "expected_markers": 24, "warning_markers": 0},
  {"ctest_name": "syntax.empty-struct", "stem": "empty-struct", "test_file": "empty-struct.k", "helper": null, "extra_options": [], "cmake_line": 85, "assertions": 2, "expected_markers": 2, "warning_markers": 0},
  {"ctest_name": "syntax.extern-class", "stem": "extern-class", "test_file": "extern-class.k", "helper": "extern_class", "extra_options": [], "cmake_line": 86, "assertions": 55, "expected_markers": 55, "warning_markers": 0},
  {"ctest_name": "syntax.function-type", "stem": "function-type", "test_file": "function-type.k", "helper": null, "extra_options": [], "cmake_line": 87, "assertions": 20, "expected_markers": 20, "warning_markers": 0},
  {"ctest_name": "syntax.dependent-template", "stem": "dependent-template", "test_file": "dependent-template.k", "helper": "dependent_template", "extra_options": [], "cmake_line": 88, "assertions": 17, "expected_markers": 17, "warning_markers": 0},
  {"ctest_name": "syntax.designated-initializer", "stem": "designated-initializer", "test_file": "designated-initializer.k", "helper": "designated_initializer", "extra_options": [], "cmake_line": 89, "assertions": 42, "expected_markers": 37, "warning_markers": 5},
  {"ctest_name": "syntax.name-attr", "stem": "name-attr", "test_file": "name-attr.k", "helper": null, "extra_options": [], "cmake_line": 90, "assertions": 10, "expected_markers": 10, "warning_markers": 0},
  {"ctest_name": "syntax.custom-base", "stem": "custom-base", "test_file": "custom-base.k", "helper": null, "extra_options": ["--no-implicit-base=false", "--base-library", "helper/custom_base.k"], "cmake_line": 91, "assertions": 1, "expected_markers": 1, "warning_markers": 0},
  {"ctest_name": "syntax.statement", "stem": "statement", "test_file": "statement.k", "helper": null, "extra_options": [], "cmake_line": 92, "assertions": 14, "expected_markers": 14, "warning_markers": 0},
  {"ctest_name": "syntax.options", "stem": "options", "test_file": "options.k", "helper": null, "extra_options": ["--Wredundant-import", "--backend=sv", "--stall=3", "--target-device=ultrascaleplus-xcu250"], "cmake_line": 93, "assertions": 7, "expected_markers": 6, "warning_markers": 1},
  {"ctest_name": "syntax.function-call", "stem": "function-call", "test_file": "function-call.k", "helper": null, "extra_options": [], "cmake_line": 94, "assertions": 5, "expected_markers": 5, "warning_markers": 0},
  {"ctest_name": "syntax.declaration-order", "stem": "declaration-order", "test_file": "declaration-order.k", "helper": "declaration_order", "extra_options": [], "cmake_line": 95, "assertions": 1, "expected_markers": 1, "warning_markers": 0},
  {"ctest_name": "syntax.doc-comments", "stem": "doc-comments", "test_file": "doc-comments.k", "helper": null, "extra_options": ["--parse-docs"], "cmake_line": 96, "assertions": 28, "expected_markers": 28, "warning_markers": 0},
  {"ctest_name": "syntax.all-warnings-as-errors", "stem": "all-warnings-as-errors", "test_file": "all-warnings-as-errors.k", "helper": null, "extra_options": ["--Wall", "--Werror"], "cmake_line": 97, "assertions": 3, "expected_markers": 3, "warning_markers": 0},
  {"ctest_name": "syntax.memory", "stem": "memory", "test_file": "memory.k", "helper": "memory", "extra_options": ["--target-device=mock"], "cmake_line": 98, "assertions": 31, "expected_markers": 31, "warning_markers": 0},
  {"ctest_name": "syntax.conversion-warning", "stem": "conversion-warning", "test_file": "conversion-warning.k", "helper": null, "extra_options": ["--Wconversion"], "cmake_line": 99, "assertions": 20, "expected_markers": 0, "warning_markers": 20},
  {"ctest_name": "syntax.no-warning", "stem": "no-warning", "test_file": "no-warning.k", "helper": null, "extra_options": ["--Wno-shadow"], "cmake_line": 100, "assertions": 2, "expected_markers": 0, "warning_markers": 2},
  {"ctest_name": "syntax.callbacks", "stem": "callbacks", "test_file": "callbacks.k", "helper": "callbacks", "extra_options": ["--Wconversion", "--Werror"], "cmake_line": 101, "assertions": 94, "expected_markers": 94, "warning_markers": 0},
  {"ctest_name": "syntax.template-template-parameter", "stem": "template-template-parameter", "test_file": "template-template-parameter.k", "helper": "template_template_parameter", "extra_options": [], "cmake_line": 102, "assertions": 39, "expected_markers": 39, "warning_markers": 0},
  {"ctest_name": "syntax.template-param-default", "stem": "template-param-default", "test_file": "template-param-default.k", "helper": "template_param_default", "extra_options": [], "cmake_line": 103, "assertions": 40, "expected_markers": 40, "warning_markers": 0},
  {"ctest_name": "syntax.quad-port-memory", "stem": "quad-port-memory", "test_file": "quad-port-memory.k", "helper": null, "extra_options": [], "cmake_line": 104, "assertions": 2, "expected_markers": 2, "warning_markers": 0},
  {"ctest_name": "syntax.export-class", "stem": "export-class", "test_file": "export-class.k", "helper": "export_class", "extra_options": [], "cmake_line": 105, "assertions": 28, "expected_markers": 28, "warning_markers": 0},
  {"ctest_name": "syntax.scheduling", "stem": "scheduling", "test_file": "scheduling.k", "helper": null, "extra_options": [], "cmake_line": 106, "assertions": 7, "expected_markers": 7, "warning_markers": 0},
  {"ctest_name": "syntax.static-for", "stem": "static-for", "test_file": "static-for.k", "helper": null, "extra_options": [], "cmake_line": 107, "assertions": 21, "expected_markers": 21, "warning_markers": 0},
  {"ctest_name": "syntax.static-if", "stem": "static-if", "test_file": "static-if.k", "helper": "static_if", "extra_options": [], "cmake_line": 108, "assertions": 74, "expected_markers": 74, "warning_markers": 0},
  {"ctest_name": "syntax.loops", "stem": "loops", "test_file": "loops.k", "helper": null, "extra_options": [], "cmake_line": 109, "assertions": 15, "expected_markers": 15, "warning_markers": 0},
  {"ctest_name": "syntax.this-capture", "stem": "this-capture", "test_file": "this-capture.k", "helper": null, "extra_options": [], "cmake_line": 110, "assertions": 19, "expected_markers": 19, "warning_markers": 0},
  {"ctest_name": "syntax.literals", "stem": "literals", "test_file": "literals.k", "helper": null, "extra_options": [], "cmake_line": 111, "assertions": 40, "expected_markers": 40, "warning_markers": 0},
  {"ctest_name": "syntax.decltype", "stem": "decltype", "test_file": "decltype.k", "helper": null, "extra_options": [], "cmake_line": 112, "assertions": 21, "expected_markers": 21, "warning_markers": 0},
  {"ctest_name": "syntax.auto-placeholder", "stem": "auto-placeholder", "test_file": "auto-placeholder.k", "helper": "auto_placeholder", "extra_options": [], "cmake_line": 113, "assertions": 36, "expected_markers": 36, "warning_markers": 0},
  {"ctest_name": "syntax.template-argument-deduction", "stem": "template-argument-deduction", "test_file": "template-argument-deduction.k", "helper": "template_argument_deduction", "extra_options": [], "cmake_line": 114, "assertions": 221, "expected_markers": 221, "warning_markers": 0},
  {"ctest_name": "syntax.template-argument-deduction-from-instance", "stem": "template-argument-deduction-from-instance", "test_file": "template-argument-deduction-from-instance.k", "helper": "template_argument_deduction_from_instance", "extra_options": [], "cmake_line": 115, "assertions": 12, "expected_markers": 12, "warning_markers": 0},
  {"ctest_name": "syntax.function-auto-return", "stem": "function-auto-return", "test_file": "function-auto-return.k", "helper": "function_auto_return", "extra_options": [], "cmake_line": 116, "assertions": 20, "expected_markers": 20, "warning_markers": 0},
  {"ctest_name": "syntax.struct-constexpr", "stem": "struct-constexpr", "test_file": "struct-constexpr.k", "helper": "struct_constexpr", "extra_options": [], "cmake_line": 117, "assertions": 13, "expected_markers": 13, "warning_markers": 0},
  {"ctest_name": "syntax.higher-order-functions", "stem": "higher-order-functions", "test_file": "higher-order-functions.k", "helper": "higher_order_functions", "extra_options": [], "cmake_line": 118, "assertions": 211, "expected_markers": 211, "warning_markers": 0},
  {"ctest_name": "syntax.lambdas", "stem": "lambdas", "test_file": "lambdas.k", "helper": null, "extra_options": [], "cmake_line": 119, "assertions": 52, "expected_markers": 52, "warning_markers": 0},
  {"ctest_name": "syntax.closures", "stem": "closures", "test_file": "closures.k", "helper": null, "extra_options": [], "cmake_line": 120, "assertions": 33, "expected_markers": 33, "warning_markers": 0},
  {"ctest_name": "syntax.local-functions", "stem": "local-functions", "test_file": "local-functions.k", "helper": "local_functions", "extra_options": [], "cmake_line": 121, "assertions": 35, "expected_markers": 35, "warning_markers": 0},
  {"ctest_name": "syntax.function-constants", "stem": "function-constants", "test_file": "function-constants.k", "helper": "function_constants", "extra_options": [], "cmake_line": 122, "assertions": 39, "expected_markers": 39, "warning_markers": 0},
  {"ctest_name": "syntax.imports", "stem": "imports", "test_file": "imports.k", "helper": null, "extra_options": ["--Wredundant-import", "--import-dir=${CMAKE_CURRENT_SOURCE_DIR}/imports"], "cmake_line": 123, "assertions": 120, "expected_markers": 98, "warning_markers": 22},
  {"ctest_name": "syntax.initializer-list", "stem": "initializer-list", "test_file": "initializer-list.k", "helper": "initializer_list", "extra_options": [], "cmake_line": 124, "assertions": 88, "expected_markers": 88, "warning_markers": 0},
  {"ctest_name": "syntax.pipelined", "stem": "pipelined", "test_file": "pipelined.k", "helper": null, "extra_options": [], "cmake_line": 125, "assertions": 15, "expected_markers": 15, "warning_markers": 0},
  {"ctest_name": "syntax.template", "stem": "template", "test_file": "template.k", "helper": null, "extra_options": [], "cmake_line": 126, "assertions": 47, "expected_markers": 47, "warning_markers": 0},
  {"ctest_name": "syntax.type-check", "stem": "type-check", "test_file": "type-check.k", "helper": "type_check", "extra_options": [], "cmake_line": 127, "assertions": 63, "expected_markers": 63, "warning_markers": 0},
  {"ctest_name": "syntax.struct-template", "stem": "struct-template", "test_file": "struct-template.k", "helper": "struct_template", "extra_options": [], "cmake_line": 128, "assertions": 6, "expected_markers": 6, "warning_markers": 0},
  {"ctest_name": "syntax.constexpr", "stem": "constexpr", "test_file": "constexpr.k", "helper": "constexpr", "extra_options": ["--define=FOO_CONST#10"], "cmake_line": 129, "assertions": 180, "expected_markers": 180, "warning_markers": 0},
  {"ctest_name": "syntax.using", "stem": "using", "test_file": "using.k", "helper": "aliases", "extra_options": [], "cmake_line": 130, "assertions": 40, "expected_markers": 40, "warning_markers": 0},
  {"ctest_name": "syntax.member-function-template", "stem": "member-function-template", "test_file": "member-function-template.k", "helper": "member_function_template", "extra_options": [], "cmake_line": 131, "assertions": 35, "expected_markers": 35, "warning_markers": 0},
  {"ctest_name": "syntax.function-template", "stem": "function-template", "test_file": "function-template.k", "helper": "function_template", "extra_options": [], "cmake_line": 132, "assertions": 21, "expected_markers": 21, "warning_markers": 0},
  {"ctest_name": "syntax.function-template-param", "stem": "function-template-param", "test_file": "function-template-param.k", "helper": "function_template_param", "extra_options": [], "cmake_line": 133, "assertions": 37, "expected_markers": 37, "warning_markers": 0},
  {"ctest_name": "syntax.enum", "stem": "enum", "test_file": "enum.k", "helper": "enum", "extra_options": [], "cmake_line": 134, "assertions": 59, "expected_markers": 59, "warning_markers": 0},
  {"ctest_name": "syntax.const-correctness", "stem": "const-correctness", "test_file": "const-correctness.k", "helper": "const_correctness", "extra_options": [], "cmake_line": 135, "assertions": 23, "expected_markers": 23, "warning_markers": 0},
  {"ctest_name": "syntax.nested-templates", "stem": "nested-templates", "test_file": "nested-templates.k", "helper": "nested_templates", "extra_options": [], "cmake_line": 136, "assertions": 19, "expected_markers": 19, "warning_markers": 0},
  {"ctest_name": "syntax.template-instance-identifiers", "stem": "template-instance-identifiers", "test_file": "template-instance-identifiers.k", "helper": "template_instance_identifiers", "extra_options": [], "cmake_line": 137, "assertions": 41, "expected_markers": 41, "warning_markers": 0},
  {"ctest_name": "syntax.struct", "stem": "struct", "test_file": "struct.k", "helper": "structs", "extra_options": [], "cmake_line": 138, "assertions": 11, "expected_markers": 11, "warning_markers": 0},
  {"ctest_name": "syntax.union", "stem": "union", "test_file": "union.k", "helper": null, "extra_options": [], "cmake_line": 139, "assertions": 4, "expected_markers": 4, "warning_markers": 0},
  {"ctest_name": "syntax.union-template", "stem": "union-template", "test_file": "union-template.k", "helper": "union_template", "extra_options": [], "cmake_line": 140, "assertions": 6, "expected_markers": 6, "warning_markers": 0},
  {"ctest_name": "syntax.local-classes", "stem": "local-classes", "test_file": "local-classes.k", "helper": null, "extra_options": [], "cmake_line": 141, "assertions": 17, "expected_markers": 17, "warning_markers": 0},
  {"ctest_name": "syntax.local-structs", "stem": "local-structs", "test_file": "local-structs.k", "helper": "local_structs", "extra_options": [], "cmake_line": 142, "assertions": 14, "expected_markers": 14, "warning_markers": 0},
  {"ctest_name": "syntax.static-locals", "stem": "static-locals", "test_file": "static-locals.k", "helper": "static_locals", "extra_options": [], "cmake_line": 143, "assertions": 6, "expected_markers": 6, "warning_markers": 0},
  {"ctest_name": "syntax.function-attributes", "stem": "function-attributes", "test_file": "function-attributes.k", "helper": null, "extra_options": [], "cmake_line": 144, "assertions": 23, "expected_markers": 23, "warning_markers": 0},
  {"ctest_name": "syntax.atomic", "stem": "atomic", "test_file": "atomic.k", "helper": null, "extra_options": [], "cmake_line": 145, "assertions": 51, "expected_markers": 51, "warning_markers": 0},
  {"ctest_name": "syntax.last", "stem": "last", "test_file": "last.k", "helper": null, "extra_options": [], "cmake_line": 146, "assertions": 14, "expected_markers": 14, "warning_markers": 0},
  {"ctest_name": "syntax.atomic_do", "stem": "atomic_do", "test_file": "atomic_do.k", "helper": null, "extra_options": [], "cmake_line": 147, "assertions": 32, "expected_markers": 32, "warning_markers": 0}
 ]
}
```

---

## 4. Per-file assertion counts

### 4.1 Verification of the plan's claims

| Claim in the plan | Actual | Verdict |
|---|---|---|
| 2843 `expected:` markers | **2843** | ✅ exact |
| 79 `warning:` markers | **79** | ✅ exact |
| 1768 with code 0 | **1768** | ✅ exact |
| 834 with code 1 | **834** | ✅ exact |
| 241 with other codes | **241** | ✅ exact |
| (total assertions) | **2922** | — |

**But the plan's numbers count all 73 `test/syntax/*.k` files, including
`basics.k`, which CMake never registers (§8).** The numbers that a conformance harness
mirroring ctest will actually execute are:

| Scope | files | `expected:` | `warning:` | total | code 0 | code 1 | other codes |
|---|--:|--:|--:|--:|--:|--:|--:|
| All `test/syntax/*.k` | 73 | 2843 | 79 | **2922** | 1768 | 834 | 241 |
| Registered by CMake | 72 | 2437 | 79 | **2516** | 1589 | 706 | 142 |
| Dead (`basics.k` only) | 1 | 406 | 0 | **406** | 179 | 128 | 99 |

Both totals are worth tracking. A Rust harness should default to the 2516 registered
assertions (that is what `ctest -R '^syntax\.'` measures) and offer a
`--include-unregistered` flag that adds `basics.k`'s 406.

### 4.2 Per-file table

Breakdown notation `NxM` = expected code `N` appears `M` times. Warning breakdown
`vxM` = marker value `v` appears `M` times; `(empty)` is a bare `warning:` line.

| # | file | src lines | `expected:` | `warning:` | total | registered as | `expected:` breakdown (code x count) | `warning:` breakdown |
|--:|---|--:|--:|--:|--:|---|---|---|
| 1 | `all-warnings-as-errors.k` | 28 | 3 | 0 | 3 | syntax.all-warnings-as-errors | 1x3 | - |
| 2 | `assignment.k` | 28 | 2 | 0 | 2 | syntax.assignment | 0x1 77x1 | - |
| 3 | `atomic.k` | 1168 | 51 | 0 | 51 | syntax.atomic | 0x31 1x1 9x1 21x1 34x11 64x6 | - |
| 4 | `atomic_do.k` | 597 | 32 | 0 | 32 | syntax.atomic_do | 0x14 25x2 34x13 65x3 | - |
| 5 | `auto-placeholder.k` | 575 | 36 | 0 | 36 | syntax.auto-placeholder | 0x29 1x7 | - |
| 6 | `base-test.k` | 5 | 1 | 0 | 1 | syntax.base-test | 0x1 | - |
| 7 | `basics.k` | 5003 | 406 | 0 | 406 | **NOT REGISTERED** | 0x179 1x128 7x1 9x8 12x4 13x5 14x15 16x10 17x1 18x11 20x2 21x4 24x10 25x3 26x3 27x4 29x4 33x1 35x1 36x1 37x4 54x2 57x1 65x3 77x1 | - |
| 8 | `callbacks.k` | 1200 | 94 | 0 | 94 | syntax.callbacks | 0x59 1x28 5x1 27x1 62x3 75x2 | - |
| 9 | `closures.k` | 645 | 33 | 0 | 33 | syntax.closures | 0x26 1x6 14x1 | - |
| 10 | `const-correctness.k` | 239 | 23 | 0 | 23 | syntax.const-correctness | 0x2 1x21 | - |
| 11 | `constexpr.k` | 1475 | 180 | 0 | 180 | syntax.constexpr | 0x133 1x47 | - |
| 12 | `conversion-warning.k` | 260 | 0 | 20 | 20 | syntax.conversion-warning | - | 0x11 conversionx9 |
| 13 | `custom-base.k` | 9 | 1 | 0 | 1 | syntax.custom-base | 0x1 | - |
| 14 | `declaration-order.k` | 6 | 1 | 0 | 1 | syntax.declaration-order | 0x1 | - |
| 15 | `decltype.k` | 252 | 21 | 0 | 21 | syntax.decltype | 0x18 1x2 54x1 | - |
| 16 | `dependent-template.k` | 231 | 17 | 0 | 17 | syntax.dependent-template | 0x7 1x10 | - |
| 17 | `designated-initializer.k` | 542 | 37 | 5 | 42 | syntax.designated-initializer | 0x18 1x14 14x1 27x1 54x1 62x2 | 0x2 initializer-overridesx3 |
| 18 | `doc-comments.k` | 234 | 28 | 0 | 28 | syntax.doc-comments | 0x17 1x11 | - |
| 19 | `empty-struct.k` | 56 | 2 | 0 | 2 | syntax.empty-struct | 0x2 | - |
| 20 | `enum.k` | 651 | 59 | 0 | 59 | syntax.enum | 0x34 1x25 | - |
| 21 | `export-class.k` | 303 | 28 | 0 | 28 | syntax.export-class | 0x16 1x12 | - |
| 22 | `extern-class.k` | 829 | 55 | 0 | 55 | syntax.extern-class | 0x25 1x16 21x8 65x1 75x2 79x3 | - |
| 23 | `function-attributes.k` | 345 | 23 | 0 | 23 | syntax.function-attributes | 0x7 1x5 18x10 27x1 | - |
| 24 | `function-auto-return.k` | 154 | 20 | 0 | 20 | syntax.function-auto-return | 0x15 1x2 14x1 21x2 | - |
| 25 | `function-call.k` | 75 | 5 | 0 | 5 | syntax.function-call | 0x1 1x1 21x1 37x2 | - |
| 26 | `function-constants.k` | 307 | 39 | 0 | 39 | syntax.function-constants | 0x22 1x17 | - |
| 27 | `function-template-param.k` | 383 | 37 | 0 | 37 | syntax.function-template-param | 0x27 1x10 | - |
| 28 | `function-template.k` | 255 | 21 | 0 | 21 | syntax.function-template | 0x10 1x10 9x1 | - |
| 29 | `function-type.k` | 115 | 20 | 0 | 20 | syntax.function-type | 0x8 1x12 | - |
| 30 | `higher-order-functions.k` | 3673 | 211 | 0 | 211 | syntax.higher-order-functions | 0x184 1x27 | - |
| 31 | `imports.k` | 1072 | 98 | 22 | 120 | syntax.imports | 0x50 1x48 | (empty)x7 0x6 redundant-importx9 |
| 32 | `initializer-list.k` | 809 | 88 | 0 | 88 | syntax.initializer-list | 0x45 1x40 14x1 62x2 | - |
| 33 | `interpolated-string.k` | 273 | 54 | 0 | 54 | syntax.interpolated-string | 0x40 1x13 7x1 | - |
| 34 | `lambdas.k` | 844 | 52 | 0 | 52 | syntax.lambdas | 0x42 1x7 26x1 32x2 | - |
| 35 | `last.k` | 219 | 14 | 0 | 14 | syntax.last | 0x6 27x6 56x2 | - |
| 36 | `literal-operand.k` | 141 | 0 | 12 | 12 | syntax.literal-operand | - | 0x12 |
| 37 | `literals.k` | 174 | 40 | 0 | 40 | syntax.literals | 0x33 1x7 | - |
| 38 | `local-classes.k` | 397 | 17 | 0 | 17 | syntax.local-classes | 0x15 1x1 14x1 | - |
| 39 | `local-functions.k` | 631 | 35 | 0 | 35 | syntax.local-functions | 0x31 1x3 18x1 | - |
| 40 | `local-structs.k` | 236 | 14 | 0 | 14 | syntax.local-structs | 0x10 1x4 | - |
| 41 | `loops.k` | 181 | 15 | 0 | 15 | syntax.loops | 0x5 1x3 16x4 25x1 65x2 | - |
| 42 | `member-function-template.k` | 441 | 35 | 0 | 35 | syntax.member-function-template | 0x20 1x15 | - |
| 43 | `memory.k` | 487 | 31 | 0 | 31 | syntax.memory | 0x15 1x12 69x4 | - |
| 44 | `name-attr.k` | 85 | 10 | 0 | 10 | syntax.name-attr | 0x5 1x5 | - |
| 45 | `nested-templates.k` | 297 | 19 | 0 | 19 | syntax.nested-templates | 0x14 1x5 | - |
| 46 | `no-warning.k` | 16 | 0 | 2 | 2 | syntax.no-warning | - | 0x2 |
| 47 | `options.k` | 52 | 6 | 1 | 7 | syntax.options | 0x4 1x2 | redundant-importx1 |
| 48 | `pipelined.k` | 281 | 15 | 0 | 15 | syntax.pipelined | 0x4 1x6 18x1 20x1 22x3 | - |
| 49 | `quad-port-memory.k` | 42 | 2 | 0 | 2 | syntax.quad-port-memory | 67x2 | - |
| 50 | `return-type-polymorphism.k` | 255 | 24 | 3 | 27 | syntax.return-type-polymorphism | 0x21 1x3 | 0x2 conversionx1 |
| 51 | `scheduling.k` | 224 | 7 | 0 | 7 | syntax.scheduling | 0x4 64x3 | - |
| 52 | `shadow-warning.k` | 159 | 0 | 14 | 14 | syntax.shadow-warning | - | 0x4 shadowx10 |
| 53 | `statement.k` | 135 | 14 | 0 | 14 | syntax.statement | 0x2 1x12 | - |
| 54 | `static-for.k` | 408 | 21 | 0 | 21 | syntax.static-for | 0x20 1x1 | - |
| 55 | `static-if.k` | 1339 | 74 | 0 | 74 | syntax.static-if | 0x59 1x10 13x2 14x1 29x1 33x1 | - |
| 56 | `static-locals.k` | 96 | 6 | 0 | 6 | syntax.static-locals | 0x4 63x2 | - |
| 57 | `string.k` | 263 | 29 | 0 | 29 | syntax.string | 0x21 1x1 53x7 | - |
| 58 | `struct-constexpr.k` | 117 | 13 | 0 | 13 | syntax.struct-constexpr | 0x7 1x6 | - |
| 59 | `struct-template.k` | 70 | 6 | 0 | 6 | syntax.struct-template | 0x4 1x2 | - |
| 60 | `struct.k` | 126 | 11 | 0 | 11 | syntax.struct | 0x4 1x7 | - |
| 61 | `template-argument-deduction-from-instance.k` | 125 | 12 | 0 | 12 | syntax.template-argument-deduction-from-instance | 0x10 1x2 | - |
| 62 | `template-argument-deduction.k` | 2358 | 221 | 0 | 221 | syntax.template-argument-deduction | 0x172 1x48 14x1 | - |
| 63 | `template-instance-identifiers.k` | 316 | 41 | 0 | 41 | syntax.template-instance-identifiers | 0x21 1x20 | - |
| 64 | `template-param-default.k` | 445 | 40 | 0 | 40 | syntax.template-param-default | 0x33 1x7 | - |
| 65 | `template-template-parameter.k` | 464 | 39 | 0 | 39 | syntax.template-template-parameter | 0x17 1x17 14x5 | - |
| 66 | `template.k` | 743 | 47 | 0 | 47 | syntax.template | 0x27 1x20 | - |
| 67 | `this-capture.k` | 563 | 19 | 0 | 19 | syntax.this-capture | 0x19 | - |
| 68 | `type-check.k` | 555 | 63 | 0 | 63 | syntax.type-check | 0x19 1x44 | - |
| 69 | `type-constexpr.k` | 341 | 51 | 0 | 51 | syntax.type-constexpr | 0x36 1x15 | - |
| 70 | `type-identifier.k` | 317 | 24 | 0 | 24 | syntax.type-identifier | 0x10 1x14 | - |
| 71 | `union-template.k` | 70 | 6 | 0 | 6 | syntax.union-template | 0x4 1x2 | - |
| 72 | `union.k` | 63 | 4 | 0 | 4 | syntax.union | 0x2 1x2 | - |
| 73 | `using.k` | 426 | 40 | 0 | 40 | syntax.using | 0x25 1x15 | - |
| | **TOTAL (73 files)** | | **2843** | **79** | **2922** | | | |

### 4.3 `warning:` markers by file (all 79)

Only 8 of the 73 files contain any `warning:` marker.

| file | `(empty)` | `0` | `conversion` | `initializer-overrides` | `redundant-import` | `shadow` | total |
|---|--:|--:|--:|--:|--:|--:|--:|
| `conversion-warning.k` | 0 | 11 | 9 | 0 | 0 | 0 | 20 |
| `designated-initializer.k` | 0 | 2 | 0 | 3 | 0 | 0 | 5 |
| `imports.k` | 7 | 6 | 0 | 0 | 9 | 0 | 22 |
| `literal-operand.k` | 0 | 12 | 0 | 0 | 0 | 0 | 12 |
| `no-warning.k` | 0 | 2 | 0 | 0 | 0 | 0 | 2 |
| `options.k` | 0 | 0 | 0 | 0 | 1 | 0 | 1 |
| `return-type-polymorphism.k` | 0 | 2 | 1 | 0 | 0 | 0 | 3 |
| `shadow-warning.k` | 0 | 4 | 0 | 0 | 0 | 10 | 14 |
| **TOTAL** | **7** | **39** | **10** | **3** | **10** | **10** | **79** |

The seven `(empty)` markers are `test/syntax/imports.k` lines
**437, 442, 455, 492, 501, 523, 695**. Each asserts only "exit 0 and no `Error N`"; the
warning comparison is skipped entirely because `expected_warning == ""` fails both
branches at `syntax-test.py:144` and `syntax-test.py:151`. They are almost certainly
*intended* to mean `warning:0`, and if a Rust harness "helpfully" treats them as
`warning:0` it will start failing snippets the reference accepts (each of those seven
snippets sits between two `warning:redundant-import` cases, i.e. they are the
"import is genuinely needed, no warning" companions — but the assertion as written
does not test that). See [Open questions](#10-open-questions).

### 4.4 How the numbers were produced

Every count above comes from re-running the reference's own two regexes with the
reference's own `if`/`elif` precedence over every `test/syntax/*.k`:

```python
# scratch/count.py  — run with: python3 count.py
import re, glob, os
from collections import Counter
tot_err = Counter(); tot_warn = Counter(); n_err = n_warn = 0
for f in sorted(glob.glob('third_party/kanagawa/test/syntax/*.k')):
    for raw in open(f, encoding='utf-8').read().splitlines():
        line = raw.strip()
        em = re.search(r'expected:(\d*)', line)
        wm = re.search(r'warning:([^\s]*)', line)
        if em:
            tot_err[int(em.group(1) or 0)] += 1; n_err += 1
        elif wm:
            tot_warn[wm.group(1)] += 1; n_warn += 1
print(n_err, n_warn, tot_err[0], tot_err[1], n_err - tot_err[0] - tot_err[1])
# -> 2843 79 1768 834 241
```

Cross-checks with plain shell (these agree because every marker line is canonical):

```console
$ cd third_party/kanagawa/test/syntax
$ grep -rhoE 'expected:[0-9]*' *.k | wc -l          # 2843
$ grep -rhoE 'warning:[^[:space:]]*' *.k | wc -l    # 79  (grep -c would undercount, use -o)
$ grep -rhoE 'expected:[0-9]*' *.k | sort | uniq -c | sort -rn
   1768 expected:0
    834 expected:1
     27 expected:14
     24 expected:34          # (full 38-row histogram in section 5)
$ grep -c '^add_syntax_test' CMakeLists.txt          # 72
$ ls *.k | sed 's/\.k$//' | sort > /tmp/all_k.txt
$ grep '^add_syntax_test' CMakeLists.txt | sed -E 's/^add_syntax_test\("([^"]+)".*/\1/' | sort > /tmp/reg.txt
$ comm -23 /tmp/all_k.txt /tmp/reg.txt               # basics
```

Canonicality check (0 violations):

```python
assert re.fullmatch(r'expected:\d*|warning:\S*', line)   # for every marker line
```

---

## 5. Expected-error-code histogram

38 distinct values appear across the corpus: the sentinel `0` plus 37 real
`CompileError` enumerators. Every non-zero value that appears is a valid enumerator —
there are no stale codes in the corpus.

| code | count (all 73 files) | count (72 registered) | `CompileError` enum name | errors.h line | producer |
|--:|--:|--:|---|---|---|
| 0 | 1768 | 1589 | *(not an enum value)* | n/a | sentinel: **no** `Error N` line on stdout **and** exit code 0 |
| 1 | 834 | 706 | `SyntaxError` | `compiler/cpp/errors.h:8` | **frontend (Haskell)** |
| 5 | 1 | 1 | `UnknownFunction` | `compiler/cpp/errors.h:12` | C++ backend |
| 7 | 2 | 1 | `InvalidGlobalAssignment` | `compiler/cpp/errors.h:14` | C++ backend |
| 9 | 10 | 2 | `InvalidArrayIndexing` | `compiler/cpp/errors.h:16` | C++ backend |
| 12 | 4 | 0 | `UnorderedOperation` | `compiler/cpp/errors.h:19` | C++ backend |
| 13 | 7 | 2 | `InvalidSwitch` | `compiler/cpp/errors.h:20` | C++ backend |
| 14 | 27 | 12 | `InvalidConversion` | `compiler/cpp/errors.h:21` | C++ backend |
| 16 | 14 | 4 | `InvalidType` | `compiler/cpp/errors.h:22` | C++ backend |
| 17 | 1 | 0 | `InvalidArraySize` | `compiler/cpp/errors.h:23` | C++ backend |
| 18 | 23 | 12 | `InvalidFunctionModifier` | `compiler/cpp/errors.h:24` | C++ backend |
| 20 | 3 | 1 | `InvalidFunctionReturnType` | `compiler/cpp/errors.h:26` | C++ backend |
| 21 | 16 | 12 | `InvalidReturn` | `compiler/cpp/errors.h:27` | C++ backend |
| 22 | 3 | 3 | `InvalidIterationCount` | `compiler/cpp/errors.h:28` | C++ backend |
| 24 | 10 | 0 | `NonValueType` | `compiler/cpp/errors.h:29` | C++ backend |
| 25 | 6 | 3 | `InvalidLoop` | `compiler/cpp/errors.h:30` | C++ backend |
| 26 | 4 | 1 | `InvalidMux` | `compiler/cpp/errors.h:31` | C++ backend |
| 27 | 13 | 9 | `InvalidCall` | `compiler/cpp/errors.h:32` | C++ backend |
| 29 | 5 | 1 | `ProtectionModifier` | `compiler/cpp/errors.h:34` | C++ backend |
| 32 | 2 | 2 | `UnknownIdentifier` | `compiler/cpp/errors.h:37` | C++ backend |
| 33 | 2 | 1 | `DuplicateIdentifier` | `compiler/cpp/errors.h:38` | C++ backend |
| 34 | 24 | 24 | `InvalidAtomic` | `compiler/cpp/errors.h:39` | C++ backend |
| 35 | 1 | 0 | `CallRateNotPredicated` | `compiler/cpp/errors.h:40` | C++ backend |
| 36 | 1 | 0 | `ExpressionNotStatic` | `compiler/cpp/errors.h:41` | C++ backend |
| 37 | 6 | 2 | `RecursionNotSupported` | `compiler/cpp/errors.h:42` | C++ backend |
| 53 | 7 | 7 | `InvalidString` | `compiler/cpp/errors.h:54` | C++ backend |
| 54 | 4 | 2 | `InvalidConst` | `compiler/cpp/errors.h:55` | C++ backend |
| 56 | 2 | 2 | `InvalidEndTransaction` | `compiler/cpp/errors.h:56` | C++ backend |
| 57 | 1 | 0 | `InvalidUpdateRate` | `compiler/cpp/errors.h:57` | C++ backend |
| 62 | 7 | 7 | `InvalidInitializerList` | `compiler/cpp/errors.h:62` | C++ backend |
| 63 | 2 | 2 | `InvalidStaticLocal` | `compiler/cpp/errors.h:63` | C++ backend |
| 64 | 9 | 9 | `BasicBlockNotSchedulable` | `compiler/cpp/errors.h:64` | C++ backend |
| 65 | 9 | 6 | `InvalidAttribute` | `compiler/cpp/errors.h:65` | C++ backend |
| 67 | 2 | 2 | `InvalidQuadPortMemory` | `compiler/cpp/errors.h:67` | C++ backend |
| 69 | 4 | 4 | `InvalidEccFunction` | `compiler/cpp/errors.h:69` | C++ backend |
| 75 | 4 | 4 | `InvalidCallback` | `compiler/cpp/errors.h:75` | C++ backend |
| 77 | 2 | 1 | `InvalidAssignment` | `compiler/cpp/errors.h:77` | C++ backend |
| 79 | 3 | 3 | `InvalidTemplateArgument` | `compiler/cpp/errors.h:79` | C++ backend |
| | **2843** | **2437** | 38 distinct codes (37 enum + sentinel 0) | | |

### 5.1 Frontend vs backend

**Exactly one code — `1` — is producible by the Haskell frontend.** Verified two ways:

* The only two `"Error "` string literals in `compiler/hs/` are
  `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:114` (`"Error 1: "`) and
  `compiler/hs/app/Main.hs:95` (`"Error 1: Warnings treated as errors"`).
* `grep -rn 'CompileError::SyntaxError' compiler/cpp/` returns **nothing** — the C++
  side never emits code 1 either, so 1 is unambiguously "the frontend rejected it".

`Error 1` therefore covers, jointly: lexing, parsing, module/import resolution, desugaring,
template instantiation, constant evaluation (`static assert`), symbol resolution, type
checking, and `--Werror` warning promotion. All 706 registered `expected:1` assertions
(834 including `basics.k`) are decidable by a frontend-only Rust implementation.

Codes ≥ 2 are all emitted by the C++ middle-end/backend through
`ErrStream` (`compiler/cpp/errors.h:104-150`), constructed via
`Compiler::ErrorStream` (`compiler/cpp/compiler.cpp:173-179`), which also sets
`_compileError = true`. Three codes additionally have hand-rolled emission paths that
still satisfy the `Error (\d+)` regex:

* code 37 — `std::cout << "Error " << …RecursionNotSupported…` at
  `compiler/cpp/enumerate_function_instances.cpp:1008`;
* code 27 — `throw std::runtime_error("Error 27: Invalid object reference")` at
  `compiler/cpp/ir.cpp:4914`, surfacing through `compiler/cpp/kanagawa.cpp:257` as
  `Error: Error 27: …`;
* code 61 — `throw std::runtime_error("Error 61: Invalid assignment to a const variable")`
  at `compiler/cpp/ir.cpp:4964`. *(Code 61 is not used by any corpus marker.)*

`Compiler::WarningStream` (`compiler/cpp/compiler.cpp:181-189`) sets `_compileError` when
`--Werror` is in effect, so a backend warning under `--Werror` fails the build with exit
1 but **no** `Error N` line — an `expected:0` marker would then fail check 2, not check 4.

### 5.2 Codes exercised only by the dead `basics.k`

Six codes vanish from the suite entirely if `basics.k` stays unregistered:

| code | enum | errors.h | occurrences (all in `basics.k`) |
|--:|---|---|--:|
| 12 | `UnorderedOperation` | `compiler/cpp/errors.h:19` | 4 |
| 17 | `InvalidArraySize` | `compiler/cpp/errors.h:23` | 1 |
| 24 | `NonValueType` | `compiler/cpp/errors.h:29` | 10 |
| 35 | `CallRateNotPredicated` | `compiler/cpp/errors.h:40` | 1 |
| 36 | `ExpressionNotStatic` | `compiler/cpp/errors.h:41` | 1 |
| 57 | `InvalidUpdateRate` | `compiler/cpp/errors.h:57` | 1 |

### 5.3 Which files use which non-trivial code

| code | files that use it (occurrences in parentheses) |
|--:|---|
| 5 | `callbacks.k` (1) |
| 7 | `basics.k` (1) *(dead)*, `interpolated-string.k` (1) |
| 9 | `atomic.k` (1), `basics.k` (8) *(dead)*, `function-template.k` (1) |
| 12 | `basics.k` (4) *(dead)* |
| 13 | `basics.k` (5) *(dead)*, `static-if.k` (2) |
| 14 | `basics.k` (15) *(dead)*, `closures.k` (1), `designated-initializer.k` (1), `function-auto-return.k` (1), `initializer-list.k` (1), `local-classes.k` (1), `static-if.k` (1), `template-argument-deduction.k` (1), `template-template-parameter.k` (5) |
| 16 | `basics.k` (10) *(dead)*, `loops.k` (4) |
| 17 | `basics.k` (1) *(dead)* |
| 18 | `basics.k` (11) *(dead)*, `function-attributes.k` (10), `local-functions.k` (1), `pipelined.k` (1) |
| 20 | `basics.k` (2) *(dead)*, `pipelined.k` (1) |
| 21 | `atomic.k` (1), `basics.k` (4) *(dead)*, `extern-class.k` (8), `function-auto-return.k` (2), `function-call.k` (1) |
| 22 | `pipelined.k` (3) |
| 24 | `basics.k` (10) *(dead)* |
| 25 | `atomic_do.k` (2), `basics.k` (3) *(dead)*, `loops.k` (1) |
| 26 | `basics.k` (3) *(dead)*, `lambdas.k` (1) |
| 27 | `basics.k` (4) *(dead)*, `callbacks.k` (1), `designated-initializer.k` (1), `function-attributes.k` (1), `last.k` (6) |
| 29 | `basics.k` (4) *(dead)*, `static-if.k` (1) |
| 32 | `lambdas.k` (2) |
| 33 | `basics.k` (1) *(dead)*, `static-if.k` (1) |
| 34 | `atomic.k` (11), `atomic_do.k` (13) |
| 35 | `basics.k` (1) *(dead)* |
| 36 | `basics.k` (1) *(dead)* |
| 37 | `basics.k` (4) *(dead)*, `function-call.k` (2) |
| 53 | `string.k` (7) |
| 54 | `basics.k` (2) *(dead)*, `decltype.k` (1), `designated-initializer.k` (1) |
| 56 | `last.k` (2) |
| 57 | `basics.k` (1) *(dead)* |
| 62 | `callbacks.k` (3), `designated-initializer.k` (2), `initializer-list.k` (2) |
| 63 | `static-locals.k` (2) |
| 64 | `atomic.k` (6), `scheduling.k` (3) |
| 65 | `atomic_do.k` (3), `basics.k` (3) *(dead)*, `extern-class.k` (1), `loops.k` (2) |
| 67 | `quad-port-memory.k` (2) |
| 69 | `memory.k` (4) |
| 75 | `callbacks.k` (2), `extern-class.k` (2) |
| 77 | `assignment.k` (1), `basics.k` (1) *(dead)* |
| 79 | `extern-class.k` (3) |

### 5.4 Complete `CompileError` enum (for reference)

All 74 enumerators, `compiler/cpp/errors.h:6-84`. "used" = appears as an `expected:` value
somewhere in `test/syntax/*.k`.

| value | name | line | used |
|--:|---|--:|:-:|
| 1 | `SyntaxError` | 8 | ✅ |
| 2 | `DuplicateFunction` | 9 | |
| 3 | `DuplicateType` | 10 | |
| 4 | `UnknownType` | 11 | |
| 5 | `UnknownFunction` | 12 | ✅ |
| 6 | `ExportTypeRestriction` | 13 | |
| 7 | `InvalidGlobalAssignment` | 14 | ✅ |
| 8 | `MultipleMemoryWrites` | 15 | |
| 9 | `InvalidArrayIndexing` | 16 | ✅ |
| 10 | `PartialElementWrite` | 17 | |
| 11 | `InvalidLiteral` | 18 | |
| 12 | `UnorderedOperation` | 19 | ✅ |
| 13 | `InvalidSwitch` | 20 | ✅ |
| 14 | `InvalidConversion` | 21 | ✅ |
| 16 | `InvalidType` | 22 | ✅ |
| 17 | `InvalidArraySize` | 23 | ✅ |
| 18 | `InvalidFunctionModifier` | 24 | ✅ |
| 19 | `InvalidNesting` | 25 | |
| 20 | `InvalidFunctionReturnType` | 26 | ✅ |
| 21 | `InvalidReturn` | 27 | ✅ |
| 22 | `InvalidIterationCount` | 28 | ✅ |
| 24 | `NonValueType` | 29 | ✅ |
| 25 | `InvalidLoop` | 30 | ✅ |
| 26 | `InvalidMux` | 31 | ✅ |
| 27 | `InvalidCall` | 32 | ✅ |
| 28 | `InvalidContainer` | 33 | |
| 29 | `ProtectionModifier` | 34 | ✅ |
| 30 | `InvalidMember` | 35 | |
| 31 | `InvalidExtern` | 36 | |
| 32 | `UnknownIdentifier` | 37 | ✅ |
| 33 | `DuplicateIdentifier` | 38 | ✅ |
| 34 | `InvalidAtomic` | 39 | ✅ |
| 35 | `CallRateNotPredicated` | 40 | ✅ |
| 36 | `ExpressionNotStatic` | 41 | ✅ |
| 37 | `RecursionNotSupported` | 42 | ✅ |
| 38 | `InvalidWaitForCondition` | 43 | |
| 40 | `UnmatchedComment` | 44 | |
| 41 | `InvalidReset` | 45 | |
| 43 | `StaticAssert` | 46 | |
| 44 | `TypeToWide` | 47 | |
| 45 | `TooManyReadPorts` | 48 | |
| 47 | `InvalidPerThreadUsage` | 49 | |
| 49 | `ReservedName` | 50 | |
| 50 | `InvalidInspectionTarget` | 51 | |
| 51 | `TooManyInspectableVariables` | 52 | |
| 52 | `InspectableTooDeep` | 53 | |
| 53 | `InvalidString` | 54 | ✅ |
| 54 | `InvalidConst` | 55 | ✅ |
| 56 | `InvalidEndTransaction` | 56 | ✅ |
| 57 | `InvalidUpdateRate` | 57 | ✅ |
| 58 | `InvalidClockSpecifier` | 58 | |
| 59 | `InvalidDebugView` | 59 | |
| 60 | `DivideByZero` | 60 | |
| 61 | `AssignmentToConst` | 61 | |
| 62 | `InvalidInitializerList` | 62 | ✅ |
| 63 | `InvalidStaticLocal` | 63 | ✅ |
| 64 | `BasicBlockNotSchedulable` | 64 | ✅ |
| 65 | `InvalidAttribute` | 65 | ✅ |
| 66 | `InvalidExportClass` | 66 | |
| 67 | `InvalidQuadPortMemory` | 67 | ✅ |
| 68 | `InvalidRAMInitialization` | 68 | |
| 69 | `InvalidEccFunction` | 69 | ✅ |
| 70 | `EccUnsupported` | 70 | |
| 71 | `AmbiguousDebugSymbols` | 71 | |
| 72 | `InvalidMemoryReadDelay` | 72 | |
| 73 | `MethodReturningReference` | 73 | |
| 74 | `InvalidExternFunction` | 74 | |
| 75 | `InvalidCallback` | 75 | ✅ |
| 76 | `DeprecatedFeature` | 76 | |
| 77 | `InvalidAssignment` | 77 | ✅ |
| 78 | `InvalidIntrinsicCall` | 78 | |
| 79 | `InvalidTemplateArgument` | 79 | ✅ |
| 81 | `MissingDeviceConfigProperty` | 80 | |
| 82 | `InvalidDeviceConfigProperty` | 81 | |
| 83 | `QuadPortUnsupported` | 82 | |
| 84 | `MissingDeviceConfigSchema` | 83 | |

Values 15, 23, 39, 42, 46, 48, 55, 80 are **absent from the enum** (retired codes);
`errors.h` has no `= 15`, `= 23`, `= 39`, `= 42`, `= 46`, `= 48`, `= 55`, `= 80` entries.

The separate `CompileWarning` enum (`compiler/cpp/errors.h:86-92`) has four members —
`TooManyRams = 8`, `LastWithoutTransactionSize = 10`, `NonConstGlobal = 12`,
`FreeNonInlineFunction = 13` — none of which the oracle can observe, because their
rendering (`compiler/cpp/errors.h:116`, `:122`) does not match
`warning:.*\[--Wno-…\]$`.

---

## 6. Warning values

### 6.1 The six frontend `WarningKind`s

`compiler/hs/lib/Language/Kanagawa/Warning.hs:20-27` declares exactly six constructors;
`Warning.hs:29-35` gives their wire names; `Warning.hs:40-44` derives the CLI switches
(`enableSwitch = "W" ++ show`, `disableSwitch = "Wno-" ++ show`).

| `WarningKind` | `show` (the string the oracle compares against) | enable flag | disable flag | on by default? | used by a `warning:` marker? |
|---|---|---|---|:-:|:-:|
| `Conversion` | `conversion` | `--Wconversion` | `--Wno-conversion` | no (needs `--Wconversion` or `--Wall`) | ✅ 10× |
| `Deprecated` | `deprecated` | `--Wdeprecated` | `--Wno-deprecated` | **yes** | ❌ 0× |
| `InitializerOverrides` | `initializer-overrides` | `--Winitializer-overrides` | `--Wno-initializer-overrides` | **yes** | ✅ 3× |
| `RedundantImport` | `redundant-import` | `--Wredundant-import` | `--Wno-redundant-import` | **yes** (but disabled by the syntax-test default options) | ✅ 10× |
| `Shadow` | `shadow` | `--Wshadow` | `--Wno-shadow` | **yes** | ✅ 10× |
| `TransactionSizeMissing` | `transaction-size` | `--Wtransaction-size` | `--Wno-transaction-size` | no (needs `--Wall`) | ❌ 0× |

Declaration order in `data WarningKind` is `Conversion, Deprecated,
InitializerOverrides, RedundantImport, Shadow, TransactionSizeMissing`
(`Warning.hs:21-26`); note that `show` for `TransactionSizeMissing` is defined *before*
`RedundantImport` in the instance block (`Warning.hs:33-34`), which is irrelevant to
semantics but will confuse a line-by-line port.

Default-on set: `{RedundantImport, Deprecated, InitializerOverrides, Shadow}`
(`compiler/hs/app/Options/CmdArgs.hs:112-117`, marked `&= ignore` so it is not a CLI flag).
`--Wall` adds `{Conversion, TransactionSizeMissing}` (`CmdArgs.hs:119-121`).
Resolution is a left fold over the argv-ordered `[WarningState WarningKind]`
(`Warning.hs:46-50`), so the **last** flag mentioning a kind wins.

### 6.2 The distinct `warning:` marker values (complete list, 6 of them)

| marker text | count | semantics |
|---|--:|---|
| `warning:0` | 39 | assert **no** frontend warning at all |
| `warning:conversion` | 10 | assert first frontend warning is `Conversion` |
| `warning:redundant-import` | 10 | assert first frontend warning is `RedundantImport` |
| `warning:shadow` | 10 | assert first frontend warning is `Shadow` |
| `warning:initializer-overrides` | 3 | assert first frontend warning is `InitializerOverrides` |
| `warning:` (empty) | 7 | assert nothing about warnings (see §4.3) |

`warning:deprecated` and `warning:transaction-size` are **never used**. There is no
`warning:` marker with a value that is not a valid `WarningKind` name (other than the
special `0` and the empty string).

---

## 7. Helper modules

### 7.1 Mechanism

If `-helper NAME` is supplied, and only then, the temp file's first line is literally
`import helper.NAME` (`syntax-test.py:46-48`, written at `syntax-test.py:52-53`). Because
`import a.b` maps to the path `a/b.k`
(`compiler/hs/lib/Language/Kanagawa/Parser.hs:198`) and `<SRC>/test/syntax` is an import
root (`syntax-test.py:106`), `import helper.NAME` resolves to
`<SRC>/test/syntax/helper/NAME.k`. Helper names are therefore **snake_case module
identifiers**, while the test names are **kebab-case**; the two are related only by
convention and several pairs deliberately differ (`using` → `aliases`, `struct` →
`structs`).

The header is rewritten identically after **every** assertion (`syntax-test.py:166`), so
every snippet in a helper-using file begins with that import and nothing else.

### 7.2 Helper file inventory — all 44 files in `test/syntax/helper/`

40 of the 72 registered tests pass a `HELPER`, and those 40 values are all distinct, so
**40 of the 44 helper files are injected via `-helper`**. The remaining four are reached
by other means (last column). **No helper file is dead.**

| # | helper file | injected as `-helper` by | other references |
|--:|---|---|---|
| 1 | `helper/aliases.k` | `syntax.using` (`CMakeLists.txt:130`) | — |
| 2 | `helper/auto_placeholder.k` | `syntax.auto-placeholder` (`CMakeLists.txt:113`) | — |
| 3 | `helper/callbacks.k` | `syntax.callbacks` (`CMakeLists.txt:101`) | — |
| 4 | `helper/const_correctness.k` | `syntax.const-correctness` (`CMakeLists.txt:135`) | — |
| 5 | `helper/constexpr.k` | `syntax.constexpr` (`CMakeLists.txt:129`) | — |
| 6 | `helper/custom_base.k` | — **never used as `-helper`** | not imported; supplied as `--base-library helper/custom_base.k` by `syntax.custom-base` (`CMakeLists.txt:91`) |
| 7 | `helper/declaration_order.k` | `syntax.declaration-order` (`CMakeLists.txt:95`) | — |
| 8 | `helper/dependent_template.k` | `syntax.dependent-template` (`CMakeLists.txt:88`) | — |
| 9 | `helper/designated_initializer.k` | `syntax.designated-initializer` (`CMakeLists.txt:89`) | — |
| 10 | `helper/doc_comment_module.k` | — **never used as `-helper`** | `import helper.doc_comment_module` inside a snippet (`doc-comments.k:232`) |
| 11 | `helper/enum.k` | `syntax.enum` (`CMakeLists.txt:134`) | — |
| 12 | `helper/export_class.k` | `syntax.export-class` (`CMakeLists.txt:105`) | also `import helper.export_class` inside 5 snippets (`extern-class.k:110,116,122,128,134`) |
| 13 | `helper/extern_class.k` | `syntax.extern-class` (`CMakeLists.txt:86`) | — |
| 14 | `helper/function_auto_return.k` | `syntax.function-auto-return` (`CMakeLists.txt:116`) | also `import helper.function_auto_return` inside a snippet (`function-type.k:102`) |
| 15 | `helper/function_constants.k` | `syntax.function-constants` (`CMakeLists.txt:122`) | — |
| 16 | `helper/function_template.k` | `syntax.function-template` (`CMakeLists.txt:132`) | — |
| 17 | `helper/function_template_param.k` | `syntax.function-template-param` (`CMakeLists.txt:133`) | — |
| 18 | `helper/higher_order_functions.k` | `syntax.higher-order-functions` (`CMakeLists.txt:118`) | also imported with an alias inside 2 snippets (`higher-order-functions.k:505,1556`) |
| 19 | `helper/initializer_list.k` | `syntax.initializer-list` (`CMakeLists.txt:124`) | — |
| 20 | `helper/interpolated_string.k` | `syntax.interpolated-string` (`CMakeLists.txt:78`) | — |
| 21 | `helper/invalid_extern_class.k` | — **never used as `-helper`** | `import helper.invalid_extern_class` inside 7 snippets (`extern-class.k:623,627,653,662,668,694,704`) |
| 22 | `helper/literal_operand.k` | `syntax.literal-operand` (`CMakeLists.txt:82`) | — |
| 23 | `helper/local_functions.k` | `syntax.local-functions` (`CMakeLists.txt:121`) | — |
| 24 | `helper/local_structs.k` | `syntax.local-structs` (`CMakeLists.txt:142`) | — |
| 25 | `helper/member_function_template.k` | `syntax.member-function-template` (`CMakeLists.txt:131`) | — |
| 26 | `helper/memory.k` | `syntax.memory` (`CMakeLists.txt:98`) | — |
| 27 | `helper/nested_templates.k` | `syntax.nested-templates` (`CMakeLists.txt:136`) | — |
| 28 | `helper/return_type_polymorphism.k` | `syntax.return-type-polymorphism` (`CMakeLists.txt:80`) | — |
| 29 | `helper/shadow_warning.k` | — **never used as `-helper`** | `import helper.shadow_warning` inside a snippet (`shadow-warning.k:64`) |
| 30 | `helper/static_if.k` | `syntax.static-if` (`CMakeLists.txt:108`) | — |
| 31 | `helper/static_locals.k` | `syntax.static-locals` (`CMakeLists.txt:143`) | — |
| 32 | `helper/string.k` | `syntax.string` (`CMakeLists.txt:79`) | — |
| 33 | `helper/struct_constexpr.k` | `syntax.struct-constexpr` (`CMakeLists.txt:117`) | — |
| 34 | `helper/struct_template.k` | `syntax.struct-template` (`CMakeLists.txt:128`) | — |
| 35 | `helper/structs.k` | `syntax.struct` (`CMakeLists.txt:138`) | — |
| 36 | `helper/template_argument_deduction.k` | `syntax.template-argument-deduction` (`CMakeLists.txt:114`) | — |
| 37 | `helper/template_argument_deduction_from_instance.k` | `syntax.template-argument-deduction-from-instance` (`CMakeLists.txt:115`) | — |
| 38 | `helper/template_instance_identifiers.k` | `syntax.template-instance-identifiers` (`CMakeLists.txt:137`) | also imported with an alias inside 13 snippets (`template-instance-identifiers.k:92,98,104,110,121,132,151,170,189,208,228,248,268`) |
| 39 | `helper/template_param_default.k` | `syntax.template-param-default` (`CMakeLists.txt:103`) | — |
| 40 | `helper/template_template_parameter.k` | `syntax.template-template-parameter` (`CMakeLists.txt:102`) | — |
| 41 | `helper/type_check.k` | `syntax.type-check` (`CMakeLists.txt:127`) | — |
| 42 | `helper/type_constexpr.k` | `syntax.type-constexpr` (`CMakeLists.txt:83`) | — |
| 43 | `helper/type_identifier.k` | `syntax.type-identifier` (`CMakeLists.txt:84`) | — |
| 44 | `helper/union_template.k` | `syntax.union-template` (`CMakeLists.txt:140`) | — |

The complete set of `import helper.*` statements that appear *inside* corpus sources — as
opposed to being injected by `-helper` — is exactly seven module names:
`helper.doc_comment_module`, `helper.export_class`, `helper.function_auto_return`,
`helper.higher_order_functions`, `helper.invalid_extern_class`, `helper.shadow_warning`,
`helper.template_instance_identifiers`.

### 7.3 The extra import root for `syntax.imports`

`syntax.imports` adds `--import-dir=<SRC>/test/syntax/imports`
(`test/syntax/CMakeLists.txt:123`). That tree holds 50 files:

```
imports/a1/ :  a1.k a2.k a3.k a4.k a5.k a6.k a7.k a8.k a9.k a10.k a11.k a12.k a13.k a14.k a15.k a16.k a17.k a18.k a19.k a20.k a21.k a22.k a23.k a24.k a25.k a26.k a27.k a28.k a29.k a30.k a31.k a32.k   (32)
imports/a1/b1/b1.k   imports/a1/b1/c1/c1.k                (2)
imports/a1/b2/b2.k   imports/a1/b2/c1/c1.k                (2)
imports/diff/bar.k   imports/diff/foo.k   imports/diff/foo_bar.k
imports/diff/foo_minus_bar.k              imports/diff/foo_self.k
imports/diff/self_foo.k                   imports/diff/self_missing.k   (7)
imports/binary.k     imports/export.k     imports/function.k
imports/reset.k      imports/undefined.k  imports/with-dash.k           (6)
imports/with-dash/with-dash.k                                           (1)
```
32 + 2 + 2 + 7 + 6 + 1 = **50**.

`imports/binary.k` is deliberately **not** valid UTF-8: its entire content is the four
bytes `c0 de de ad` (`od -An -tx1 imports/binary.k`), used to exercise the
compiler's file-decoding error path (`readFileUtf8` wrapped in `Ex.try` at
`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:106-107`). A Rust harness that reads
import roots eagerly must not choke on it.

---

## 8. Dead tests

Exactly one corpus file is never registered:

| file | size | markers | codes it uniquely covers |
|---|--:|--:|---|
| `test/syntax/basics.k` | 66 599 bytes, 5003 lines | 406 (`expected:` only) | 12, 17, 24, 35, 36, 57 |

Evidence:

```console
$ ls *.k | sed 's/\.k$//' | sort | comm -23 - <(grep '^add_syntax_test' CMakeLists.txt \
      | sed -E 's/^add_syntax_test\("([^"]+)".*/\1/' | sort)
basics
```

and a repo-wide search for `basics` outside `thirdparty/` finds no CMake, Python, YAML or
Markdown reference. It is the single largest file in the suite (406 of the 2922
assertions, 13.9%) and its `expected:` breakdown is
`0x179 1x128 7x1 9x8 12x4 13x5 14x15 16x10 17x1 18x11 20x2 21x4 24x10 25x3 26x3 27x4 29x4 33x1 35x1 36x1 37x4 54x2 57x1 65x3 77x1`.

No helper file and no `imports/` file is dead (§7.2, and every `imports/` file is reached
from `imports.k` snippets).

Recommendation for the Rust harness: register `basics.k` under a distinct suite
(`syntax_unregistered::basics`) that is compiled and reported but excluded from the
"parity with ctest" gate, so its 406 assertions become a bonus signal rather than a
false regression.

---

## 9. Rust harness design

### 9.1 Goals

1. **Bit-exact oracle semantics** — same markers, same regexes, same precedence, same
   pass/fail verdicts as `syntax-test.py`.
2. **One libtest-mimic test per assertion**, not per file: 2516 independently
   named, independently reportable, independently `--exact`-runnable tests (2922 with
   `basics.k`). This is strictly *more* information than the reference, which aborts a
   file at its first failure.
3. **Runs without a C++ backend**, which the Rust rewrite does not have.
4. **Optional `--reference` mode** that shells out to a real `kanagawa` binary and
   reproduces the reference verdicts exactly, for differential testing.

### 9.2 Naming

Each assertion is one libtest-mimic `Trial`. Proposed name:

```
syntax::<file-stem>::<index>            e.g.  syntax::atomic_do::17
```

where `<file-stem>` is the corpus file's stem with `-` → `_` (so the name is a legal Rust
path segment and is greppable), and `<index>` is the **0-based ordinal of the marker
within the file**, zero-padded to the width of the file's marker count so that
lexicographic order equals execution order (`syntax::basics::007` … `syntax::basics::405`).

The marker's **source line number** must accompany the name — put it in the trial's
`kind` field (libtest-mimic prints `kind` in brackets before the name) and repeat it in
every failure message:

```
Trial::test(format!("syntax::{stem}::{idx:0width$}"), run)
    .with_kind(format!("{file}:{line}"))
```

so a failure reads

```
[test/syntax/atomic_do.k:412] syntax::atomic_do::17 ... FAILED
    Unexpected result. Expected error: 34 Actual: 0
    ---- snippet (test/syntax/atomic_do.k:398-411) ----
    …
```

Rationale for index-plus-line rather than line alone: the index is stable under
whitespace edits to a snippet and gives a natural ordering; the line is what a human
needs to open the file. Emitting both costs nothing.

### 9.3 Snippet materialisation

Precompute, once per corpus file, a `Vec<Assertion>`:

```rust
struct Assertion {
    file: PathBuf,        // test/syntax/<stem>.k
    index: usize,         // 0-based marker ordinal
    marker_line: usize,   // 1-based line number of the marker line
    span: (usize, usize), // 1-based inclusive line range of the snippet body
    kind: Expect,         // Error(u32) | Warning(WarningExpect)
    source: String,       // header + stripped body lines, each + '\n'
}

enum Expect { Error(u32), Warning(WarningExpect) }
enum WarningExpect { None_, Any /* the bare `warning:` case */, Named(String) }
```

Build it with the reference algorithm verbatim: accumulate `line.trim()` + `'\n'`;
on a marker, emit an `Assertion` whose `source` is `header + accumulated`, then reset
`accumulated` to empty (the header is re-prepended, matching `write_temp_header`).
Assertions are fully independent, which is exactly what makes per-assertion tests sound.

Note `Expect::Warning` also carries an implicit `Error(0)` obligation (check 2 of §1.8);
model it that way rather than as a separate variant, or you will drop 79 error checks.

### 9.4 Frontend-only mode (default)

The Rust rewrite has a frontend and no C++ backend, so partition the assertions by what
is decidable:

| `Expect` | frontend-only verdict | count (registered) |
|---|---|--:|
| `Error(1)` | **decidable** — the frontend must produce ≥ 1 diagnostic | 706 |
| `Error(0)` | **decidable as a necessary condition** — the frontend must produce **zero** diagnostics. Not sufficient: the reference also requires the backend to succeed. | 1589 |
| `Error(n)`, n ≥ 2 | **not decidable** — the code is produced by C++ passes that do not exist in Rust | 142 |
| `Warning(_)` | **decidable** — warnings are a pure frontend product (`Error.hs:145`), and the accompanying `Error(0)` obligation is the `Error(0)` row's necessary condition | 79 |

Concretely:

* `Error(1)` → `assert!(frontend.diagnostics().iter().any(Diagnostic::is_error))`. Pass
  when the frontend rejects. **Do not** compare messages or spans — the oracle never does.
* `Error(0)` → `assert!(frontend.diagnostics().iter().all(|d| !d.is_error()))`. Report as
  `PASS (frontend-clean)`. The residual risk — the frontend accepts but the real backend
  would have emitted `Error n` — is bounded and measurable via `--reference` (§9.6).
* `Error(n≥2)` → the trial returns
  `Err(Failed::from("requires C++ backend: expected Error <n>"))` under a **skip/xfail**
  policy: libtest-mimic has no first-class "skipped" outcome that survives `--format
  terse`, so use `Trial::test(...).with_kind("xfail-backend")` and have the runner turn a
  *pass* into a hard failure ("unexpectedly decidable") and a *frontend rejection* into a
  reported soft failure. The pragmatic default: **the frontend must not reject these
  snippets either** — every `expected:n≥2` snippet is by construction frontend-clean,
  because the reference's `first_compile_error_code` would otherwise have been 1. That
  turns all 142 into a *usable* assertion: `assert!(no frontend errors)`, tagged
  `xfail-backend` so that nobody mistakes it for full coverage.
  This is a genuine, citable strengthening: for `expected:34` to have passed against the
  reference, the frontend must have emitted nothing (else the first `Error (\d+)` match
  would have been `1`, tripping `syntax-test.py:137`).
* `Warning(None_)` → no warning diagnostics; `Warning(Named(k))` → the **first** warning
  diagnostic's kind renders to `k`; `Warning(Any)` → no assertion (but still no errors).

Effective frontend-only coverage: **2516 / 2516 assertions runnable**, of which 2374
(1589 + 706 + 79) are full-strength and 142 are the weakened "frontend-clean" form.

### 9.5 Warning ordering

`Warning(Named(k))` compares against the *first* warning, where "first" is defined by the
reference as "first stdout line matching the regex". In the Haskell compiler that order is
produced by `makeErrorBundles`, which groups by source file with `NE.groupBy` and sorts
within a group by byte offset (`compiler/hs/lib/Language/Kanagawa/Error.hs:168-169`),
then prints bundle by bundle (`compiler/hs/app/Main.hs:92-93`). The Rust frontend must
therefore sort warnings by `(file-group-in-emission-order, byte offset)` before taking the
first. All 23 non-`0`, non-empty warning markers in the corpus have exactly one warning
kind in play, so this only matters for future tests — but implement it, and add a
debug assertion that flags any snippet producing warnings of two different kinds.

### 9.6 `--reference` mode

```
cargo xtask conformance [--reference <path-to-kanagawa>] [--kanagawa-src <path>]
                        [--include-unregistered] [--jobs N] [--filter <substr>]
```

When `--reference` is given, each trial stops consulting the Rust frontend and instead
performs the reference's own steps:

1. Create `<target>/conformance/<stem>/` (per **file**, reused across that file's
   assertions — but see the concurrency note below).
2. Write `syntax_test_temp_file.k` with the assertion's `source` (ASCII, LF).
3. Spawn the binary with the argv of §2 verbatim, `cwd` = that directory, capturing
   stdout and stderr separately. Options come from the embedded registry (§3.1) with
   `<SRC>` substituted from `--kanagawa-src` (defaulting to the sibling checkout).
4. Scan **stdout only**, line by line after `split('\n')`, applying
   `Error (\d+)` (first wins), `warning:.*\[--Wno-([^\]]*)\]$` (first wins, end-anchored,
   no trailing `\r` tolerated) and `trimmed == "Call Stack:"`.
5. Apply checks 1–4 of §1.8 in order, producing the reference's exact message strings.

Concurrency: the reference relies on one process per test file and a per-file scratch
directory. Per-assertion parallelism needs a **per-assertion** scratch directory —
`<target>/conformance/<stem>/<index>/syntax_test_temp_file.k` — otherwise two trials of
the same file race on the temp file. This changes `cwd`, which is observable: the
compiler's first import-search entry is the temp file's own directory
(`compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:103`, `dir:importDirs`). No corpus
snippet imports a sibling file by bare name, so the deeper path is behaviourally inert;
assert that by running `--reference --jobs 1` with per-file directories once and
diffing verdicts.

`--reference` mode is also the tool that measures the §9.4 residual risk: run it, and
every `Error(0)` assertion that the reference passes but the Rust frontend-only mode
would also pass tells you nothing, whereas any assertion where the two disagree is a
concrete bug report.

### 9.7 Two harness invariants worth asserting at startup

* The parsed registry has **72** entries and their `assertions` sum to **2516**;
  the corpus scan finds **2843 + 79 = 2922** markers across **73** files. Hard-fail with
  a diff if the reference tree drifts — this is the tripwire that tells you the spec is
  stale.
* Every `HELPER` value resolves to an existing `test/syntax/helper/<name>.k`, and every
  registered `TEST_NAME` resolves to an existing `test/syntax/<name>.k`.

### 9.8 What deliberately is *not* reproduced

* `.done` / `.error` sidecar files (`syntax-test.py:65-72`, `:169`) — replaced by
  libtest-mimic's own reporting plus, optionally, a `--json` output.
* Echoing every compiler stdout line to the harness's stdout (`syntax-test.py:114`) —
  captured and only printed on failure.
* Abort-file-on-first-failure (§1.9) — deliberately replaced by per-assertion isolation.
  This means the Rust harness can report *more* failures than ctest for the same tree;
  the parity gate must compare "set of failing assertions" ⊇ "first failing assertion per
  failing file", not raw counts.

---

## 10. Open questions

Each entry names the probe that settles it.

1. **Is the `Call Stack:` check meant to be `Compiler Internal Call Stack:`?**
   The oracle tests for a string no component emits (§1.7). Probe: build the reference,
   force an assertion failure (a snippet that trips `KanagawaAssert`, or run with
   `_releaseAssert` on and an ICE-triggering input), and observe whether
   `syntax-test.py` reports `Unexpected internal compiler error` or instead falls through
   to the error-code comparison. Expected result with today's tree: it falls through.
   *Until settled, implement the literal check.*

2. **Are the seven bare `warning:` markers in `imports.k` intended to be `warning:0`?**
   (§4.3.) Probe: patch a private copy of `imports.k` changing lines
   437, 442, 455, 492, 501, 523, 695 to `warning:0` and re-run
   `ctest -R '^syntax\.imports$'`. If it still passes, the two spellings are equivalent
   on this corpus and the Rust harness may treat them identically; if it fails, the bare
   form is load-bearing and must stay a no-op.

3. **Is `expected:` (no digits) intended to exist?** No corpus line uses it today, so the
   `int(group(1) or 0)` path is untested. Probe: append `expected:` to a scratch copy of
   `base-test.k` after a valid snippet and confirm the harness compiles and requires
   success. *Implement as `0` regardless — that is what the code does.*

4. **Does any `expected:0` snippet pass the frontend but fail the C++ backend?**
   This bounds the strength of frontend-only mode (§9.4). Probe: `--reference` mode over
   all 1589 registered `Error(0)` assertions with a build of the reference compiler; any
   that the reference fails would indicate corpus rot, and any that the Rust frontend
   accepts while the reference backend rejects is out of scope for a frontend-only gate.

5. **Does moving `cwd` one level deeper (per-assertion scratch dirs) change any verdict?**
   (§9.6.) Probe: run `--reference` twice, once with per-file dirs at `--jobs 1` and once
   with per-assertion dirs, and diff the verdict vectors. Expected: identical.

6. **Should `basics.k` be registered?** It carries 406 assertions and is the sole user of
   codes 12, 17, 24, 35, 36, 57. Probe: add
   `add_syntax_test("basics")` to a private copy of `test/syntax/CMakeLists.txt` and run
   `ctest -R '^syntax\.basics$'`. If it passes, the file is merely un-wired; if it fails,
   it is stale and its 406 assertions must not be adopted as a gate without triage.
   *(This is a read-only tree; do the experiment in a copy.)*

7. **Is stderr ever load-bearing?** The oracle ignores it entirely (§1.6). Probe: run any
   failing snippet and check whether the reference compiler writes diagnostics to stderr
   at all. If it does, an entire class of diagnostics is invisible to the suite — worth
   knowing before the Rust CLI decides which stream to use.
