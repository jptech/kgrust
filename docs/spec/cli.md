# Kanagawa compiler — command-line interface specification

> **Status, 2026-10-11.** Extracted at kanagawa `8e0a3fb` (2026-05-18); the
> reference is pinned at `98a2d8c` (`reference.lock`). Since the extraction
> the CLI files changed: upstream added a third mode, `list-deps` (#136,
> `CmdArgs.hs:155-165`, `Main.hs:111-122`), and moved the `--file-list`
> write to right after parsing (§4.6). `Main.hs`, `CmdArgs.hs`,
> `Options.hs`, `verilog.cpp` and `test/compiler/cli` citations are
> re-derived at the pin; the rest of §1–§8 still describes `8e0a3fb`, which
> for every other cited file is the pin. §9 and §10 are the pre-implementation
> design of `kg` and `kanagawa.toml`; each now opens with what was built, and
> where they differ `crates/kg-cli/src/lib.rs` and `crates/kg-config/src/lib.rs`
> are authoritative.

Reference implementation under test: `third_party/kanagawa` (READ-ONLY).
Every claim below carries a `path:line` citation relative to that tree. Claims that could
not be grounded in the tree are explicitly marked **UNVERIFIED**.

Primary sources actually read for this document:

| File | Role |
|---|---|
| `compiler/hs/app/Options/CmdArgs.hs` | the entire flag surface (declarative `cmdargs` record) |
| `compiler/hs/app/Options.hs` | the `Options` sum type (`PrettyPrint` / `Compile` / `ListDeps` / `LangServer`) |
| `compiler/hs/app/Main.hs` | dispatch, `_cmdArgs` construction, exit codes, `--file-list`, `list-deps` |
| `compiler/hs/app/Options/FFI.hsc` | the Haskell→C marshalling map (`Storable Options`) |
| `compiler/hs/app/ParseTree.hs` | `Codegen` FFI call, `--output` default derivation |
| `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs` | import search path, `.cmdargs`/`.options` modules, `exitError` |
| `compiler/hs/lib/Language/Kanagawa/Parser/Options.hs` | `ParseOptions` defaults |
| `compiler/hs/lib/Language/Kanagawa/Warning.hs` | the 6 warning kinds and `--W`/`--Wno-` name derivation |
| `compiler/hs/lib/Language/Kanagawa/Frontend.hs` | `--frontend-passes` semantics |
| `compiler/hs/lib/Language/Kanagawa/Template.hs` | `--template-passes` / `--template-iterations` semantics |
| `compiler/hs/lib/Language/Kanagawa/Error.hs` | warning message format `[--Wno-<name>]` |
| `compiler/cpp/options.h` | the three C structs and their field order |
| `compiler/cpp/config.cpp` | `SetupCodeGenConfig`, force-zeroing, post-parse validation |
| `compiler/cpp/kanagawa.cpp` | `Codegen` entry point, backend error handling |
| `compiler/cpp/parse_tree.cpp` | `InitCompiler` entry point |
| `compiler/cpp/errors.h` | `Error <N> at: <file> (<line>): <msg>` format and the error-code enum |
| `compiler/cpp/verilog.cpp` | the single consumer of `_cmdArgs` |
| `build/cmake/test_helper.cmake` | the standard test option set |
| `test/**/CMakeLists.txt`, `test/syntax/syntax-test.py`, `tools/chkdoc/chkdoc.cpp` | every actual invocation |

## 0. Invocation model

### 0.1 Argument parser

The binary uses the Haskell `cmdargs` library (`compiler/kanagawa.cabal:44`,
`cmdargs >= 0.10.10`) in *implicit* (`Data`-reflection) mode. The whole CLI is one
declarative record literal (`compiler/hs/app/Options/CmdArgs.hs:18-174`).

`cmdargs` is **not vendored** in this tree, so parser micro-semantics (prefix matching,
auto-assigned short flags, enum-value matching, error text) cannot be cited to a file here.
Where that matters it is called out as **UNVERIFIED** with a probe in §11.

### 0.2 Modes (subcommands)

```haskell
mode :: Mode (CmdArgs Options)
mode = cmdArgsMode $ modes [compile &= auto, pretty, listDeps] &=
    program "kanagawa" &=
    verbosity &=
    help "Kanagawa compiler" &=
    summary ("Kanagawa Compiler " ++ makeReleaseName releaseVersion releaseBuildChannel ++ ", (C) Microsoft")
```
— `compiler/hs/app/Options/CmdArgs.hs:166-171` (at `8e0a3fb`, `:155-160`, without `listDeps`)

* Three modes: `compile` (`CmdArgs.hs:138`, marked `&= auto` at `:167` ⇒ **default mode**, so
  `kanagawa foo.k` == `kanagawa compile foo.k`), `pretty` (`CmdArgs.hs:152`), and
  `list-deps` (`CmdArgs.hs:155-165`, upstream #136, 2026-08-27; not in `8e0a3fb`).
* `list-deps` parses the program and resolves its imports, runs no frontend pass
  and no codegen, and prints the program's files, sorted, one per line — or writes
  them to `--file-list` (`Main.hs:111-122`; §4.6). Its record `ListDeps{}` assigns
  no field, so all nine of its fields are inherited flags, as `pretty`'s are
  (`Options.hs:132-142`, §1.3): `--base-library`, `--define`, `--file-list`,
  `--import-dir`, `--no-implicit-base`, `--parse-docs`, `--target-device`,
  `--using` and the positional files.
* Program name for help output is hard-coded `"kanagawa"` (`CmdArgs.hs:168`); this is
  *not* the string used for `_cmdArgs` (see §6).
* `verbosity` (`CmdArgs.hs:169`) adds the `--verbose`/`--quiet` pair.
* `compile` mode help details (`CmdArgs.hs:140-143`):
  ```
  Usage example:
      Compile source.k and generate SystemVerilog RTL:
        kanagawa --backend=sv source.k
  ```
* `pretty` mode help string: `"Parse and pretty print source file(s)"` (`CmdArgs.hs:153`).
* A fourth constructor `LangServer` exists in the type (`compiler/hs/app/Options.hs:143-146`)
  but is **not** in the `modes` list and its handler is commented out
  (`compiler/hs/app/Main.hs:124-125`). There is no language-server subcommand.

### 0.3 Main dispatch

```haskell
main = do
    args <- getArgs
    prog <- getProgName
    options <- (if null args then withArgs ["--help=all"] else id) getOptions
    if null $ O.files options
        then exitError "Missing source filename(s)"
        else handle options $ unwords $ prog : args
```
— `compiler/hs/app/Main.hs:33-40`

* **Zero arguments ⇒ the process behaves exactly as `kanagawa --help=all`**
  (`Main.hs:37`), which `cmdargs` prints to stdout and exits 0.
* After parsing, an empty positional file list is a hard error
  (`Main.hs:38-39`) → `Missing source filename(s)` on stdout, exit 1.

### 0.4 Version / build channel

`summary` embeds `makeReleaseName releaseVersion releaseBuildChannel`
(`CmdArgs.hs:171`, `compiler/hs/app/ReleaseVersion.hs:56-62`):
`"<maj>.<min>.<patch>"` for `Release`, `+ " (local)"` for `Local`, `+ " (nightly)"` for `Nightly`.
Defaults are `0.0.0` and `Local` when the CPP macros are absent
(`ReleaseVersion.hs:24-51`).

**Latent bug worth reproducing or fixing deliberately:** the CMake build passes
`-DBUILD_CHANNEL=<n>` (`compiler/hs/CMakeLists.txt:62-63`) but `ReleaseVersion.hs:44`
tests `RELEASE_BUILD_CHANNEL`. The channel is therefore *always* `Local` in every build
produced by this tree, so the summary line is always `... (local) ...`.

---

## 1. Every flag

### 1.0 Exact flag count (verifies / corrects the "~90 flags" claim)

The `compile` record literal (`compiler/hs/app/Options/CmdArgs.hs:20-136`) has **92 fields**
(enumerated mechanically from `:20` `files` through `:111` `warnings_state`):

| Bucket | Count | Note |
|---|---|---|
| `files` | 1 | positional (`&= args`, `CmdArgs.hs:20`) — not a flag |
| `warnings_state` | 1 | an `enum` over 13 alternatives ⇒ **13** flags (`CmdArgs.hs:111-136`) |
| all other fields | 90 | exactly one primary long flag each (`CmdArgs.hs:21-110`) |

**Answer: the "~90 flags" claim counts only the 90 one-flag fields. The true
`compile`-mode flag count is 103** (90 + 13 warning switches).

Accepted *spellings* in `compile` mode = 103 long names + 5 declared aliases
(`-f`, `-d`, `-u`, `-O`, `--target`) = **108**, plus the two short flags `cmdargs`
auto-assigned, `-j` and `-o` (§11 Q1, answered).

`pretty` mode: **4** flags declared (`CmdArgs.hs:147-150`) + **8** flags that come from
`PrettyPrint`'s unassigned record fields (`Options.hs:24-37`; see §1.3) = **12**.

Built-in `cmdargs` flags (both modes): `--help`/`-?`, `--version`/`-V`,
`--numeric-version`, `--verbose`/`-v`, `--quiet`/`-q` — **5** (UNVERIFIED spelling, §11 Q2).

Grand total distinct user-visible flags: **103 + 12 + 5 = 120**.

At the pin (`98a2d8c`), `list-deps` adds **8** more, counted the same way as
`pretty`'s inherited ones (§0.2): **128**. `crates/kg-cli/src/legacy/flags.rs`
implements and counts all 128 (`spelling_count`).

### 1.1 `compile` mode — exhaustive table

Reading key:
* **Long** — the long flag. Names shown in `code` are declared with `&= name`; a `!`
  suffix means the field is also `&= explicit` (⇒ `cmdargs` derives *no* additional
  names, including no auto short flag).
* Flags without a `!` get their long name derived from the Haskell field name by
  replacing `_` with `-`, and `cmdargs` *may* auto-assign a short flag (§11 Q1).
* **Val** — argument metavar. `INT`/`NUM` are derived from the field type;
  `FILE`/`DIR` come from `&= typFile`/`&= typDir`; quoted metavars come from `&= typ`.
  `BOOL` fields are switches that also accept `=true`/`=false`
  (proved in-tree: `--no-implicit-base=false`, `test/syntax/CMakeLists.txt:76`).
* **Group** — `&= groupname`, controls only `--help` sectioning.
* Source line = line in `compiler/hs/app/Options/CmdArgs.hs`.

| # | Long | Short | Val | Default | Group | Haskell field | Help text | Src |
|---|---|---|---|---|---|---|---|---|
| — | *(positional)* | — | FILE… | `[]` | — | `files` | *(no help; `&= args`)* | 20 |
| 1 | `--additional-latency` ! | — | INT | `1` | Logic | `additional_latency` | Extra latency added to ease routing | 21 |
| 2 | `--auto-pipeline-ratio` ! | — | INT | `0` | Logic | `auto_pipeline_scale` | The maximum number of extra pipeline registers to insert on function entry points. On platforms that support it, the actual pipeline depth is determined by the EDA tool. | 22 |
| 3 | `--auto-pipeline-cross-region` ! | — | INT | `0` | Logic | `auto_pipeline_cross_region` | The maximum number of extra pipeline registers to insert on cross-region function entry points. On platforms that support it, the actual pipeline depth is determined by the EDA tool. | 23 |
| 4 | `--carry-chain-width` ! | — | INT | `40` | Logic | `carry_chain_width` | The number of output bits in a carry-chain operation (add/sub) that is considered to be 1 level of logic | 24 |
| 5 | `--carry-select` | auto? | BOOL | `False` | Logic | `carry_select` | Reduce pipeline length by implementing add/sub/compare operations with replicated, speculative computations | 25 |
| 6 | `--rmw-memory-write-delay` | auto? | INT | `1` | Logic | `rmw_memory_write_delay` | Additional delay to add to memory writes for read-modify-write memory operations | 26 |
| 7 | `--rmw-memory-read-delay` | auto? | INT | `0` | Logic | `rmw_memory_read_delay` | Additional delay to add to memory reads for read-modify-write memory operations | 27 |
| 8 | `--rmw-hardened-write-delay` | auto? | INT | `1` | Logic | `rmw_hardened_write_delay` | Additional delay to add to memory writes for read-modify-write memory operations (when hardened forwarding logic is used) | 28 |
| 9 | `--rmw-hardened-max-width` | auto? | INT | `9223372036854775807` (`maxBound::Int`) | Logic | `rmw_hardened_max_width` | Prevent hardened forwarding logic from being used when BRAM width is > rmw_hardened_max_width AND BRAM height is <= rmw_hardened_max_height | 29 |
| 10 | `--rmw-hardened-max-height` | auto? | INT | `0` | Logic | `rmw_hardened_max_height` | (same text as #9) | 30 |
| 11 | `--rmw-memory-favor-area` | auto? | BOOL | `False` | Logic | `rmw_memory_favor_area` | Optimize read-modify-write memory operations for smaller area. By default optimize for Fmax. | 31 |
| 12 | `--strict-memory-read-write` | auto? | BOOL | `False` | Logic | `strict_memory_read_write` | Require concurrent memory read-write operations to the same address to result in the read seeing the old data (at a potential area cost) | 32 |
| 13 | `--io-register-delay` | auto? | INT | `0` | Logic | `io_register_delay` | Delay to account for routing data to/from DSP/BRAM input/output registers (measured in levels of logic) | 33 |
| 14 | `--mem-to-reg` | auto? | NUM (Float) | `0.0` | Logic | `mem_to_reg` | Upper threshold to move data from registers to memory (measured in memory bits per register bit). Higher values lead to more memory consumption. | 34 |
| 15 | `--fifo-to-dsp` | auto? | INT | `0` | Logic | `fifo_to_dsp` | Local data propagation FIFOs for a number of pipeline stages less than or equal to this threshold will be converted to DSPs | 35 |
| 16 | `--fifo-write-delay` | auto? | INT | `1` | Logic | `fifo_write_delay` | Number of cycles to add to fifo writes | 36 |
| 17 | `--fifo-merge-distance` | auto? | INT | `4` | Logic | `fifo_merge_distance` | Maximum distance (pipeline stages) between merged fifo enqueue operations | 37 |
| 18 | `--frequency` | `-f` | `MHz` | `0` | Logic | `frequency` | Clock 0 frequency | 38 |
| 19 | `--max-block-ram` | auto? | INT | `-1` (→ `SIZE_MAX`) | Logic | `max_block_ram` | Maximum number of Block RAMs generated | 39 |
| 20 | `--block-ram-util-threshold` | auto? | INT | `20` | Logic | `block_ram_util_threshold` | Percent minimum Block RAM utilization | 40 |
| 21 | `--max-deep-ram` | auto? | INT | `-1` (→ `SIZE_MAX`) | Logic | `max_deep_ram` | Maximum number of Deep RAMs generated | 41 |
| 22 | `--deep-ram-util-threshold` | auto? | INT | `20` | Logic | `deep_ram_util_threshold` | Percent minimum Deep RAM utilization | 42 |
| 23 | `--max-mux-sources` | auto? | INT | `16` | Logic | `max_mux_sources` | Decompose multiplexers with more than this many dynamic inputs | 43 |
| 24 | `--max-mul-src-width` | auto? | INT | `32` | Logic | `max_mul_src_width` | Decompose multipliers with inputs wider than this | 44 |
| 25 | `--max-threads-default` | auto? | INT | `512` | Logic | `max_threads_default` | Default value for the maximum number of threads allowed in a function at one time | 45 |
| 26 | `--max-threads-limit` | auto? | INT | `512` | Logic | `max_threads_limit` | Upper bound on the maximum number of threads allowed in a function at one time | 46 |
| 27 | `--schedule-limit` | auto? | INT | `4096` | Logic | `schedule_limit` | Maximum number of iterations during instruction scheduling | 47 |
| 28 | `--register-ratio` | auto? | INT | `2` | Logic | `register_ratio` | Insert pipeline registers after every N levels of combinatorial logic | 48 |
| 29 | `--max-register-ratio` | auto? | INT | `0` | Logic | `max_register_ratio` | Atomic blocks can relax scheduling to insert pipeline register after every N levels of combinatorial logic | 49 |
| 30 | `--no-reset-fanout` | auto? | BOOL | `False` | Logic | `no_reset_fanout` | Disable generation of reset fanout and rely on CAD tools to perform fanout | 50 |
| 31 | `--reset-cycles` ! | — | INT | `3` | Logic | `reset_cycles` | Number of cycles to use to propagate the reset signal across the chip. | 51 |
| 32 | `--reset-fan-out-cycles` | auto? | INT | `1` | Logic | `reset_fan_out_cycles` | Number of cycles used to fan-out the reset signal | 52 |
| 33 | `--reset-duration-cycles` | auto? | INT | `0` | Logic | `reset_duration_cycles` | Number of extra cycles to hold the reset signal high | 53 |
| 34 | `--return-stages` | auto? | INT | `3` | Logic | `return_stages` | Number of extra pipeline stages to add for returns from functions with multiple call sites | 54 |
| 35 | `--stallable-pipelines` | auto? | BOOL | `False` | Logic | `stallable_pipelines` | Generate pipelines where individual stages can be stalled (reduces FIFO depth) | 55 |
| 36 | `--clock-gating` | auto? | INT | `0` | Logic | `clock_gating` | Level of clock gating to apply (0 to disable) | 56 |
| 37 | `--clock-gating-threshold` | auto? | INT | `5` | Logic | `clock_gating_threshold` | Instantiate a clock gate if it will apply to more than N register bits | 57 |
| 38 | `--conditional-memory-reads` | auto? | BOOL | `False` | Logic | `conditional_memory_reads` | For memory reads that are conditional, try and make the memory read enable signal conditional as well. This can lower power consumption, but in some cases may negatively impact the maximum clock frequency | 58 |
| 39 | `--semaphore-delay` | auto? | INT | `3` | Logic | `semaphore_delay` | Number of pipeline registers to add to the input signals that drive an internal semaphore module that is used to limit threads within a function | 59 |
| 40 | `--sparse-reg-opt` | auto? | BOOL | `False` | Logic | `sparse_reg_opt` | Eliminate unused or constant bits in registers. | 60 |
| 41 | `--jobs` | auto? | INT (`Maybe Int`) | *unset* (`Nothing`) | Compiler | `jobs` | Number of parallel jobs to use during compilation. By default equal to number of processors. Negative value can be used to specify how many fewer jobs than number of processors to run. | 61 |
| 42 | `--import-dir` ! | — | DIR (repeatable) | `[]` | Compiler | `import_dir` | Directories to search for imported files | 62 |
| 43 | `--define` ! | `-d` | `name#val` (repeatable) | `[]` | Compiler | `define` | Define a global constant | 63 |
| 44 | `--using` ! | `-u` | `name#type` (repeatable) | `[]` | Compiler | `using` | Define a global type alias | 64 |
| 45 | `--no-implicit-base` | auto? | BOOL | `False` | Compiler | `no_implicit_base` | Disable implicit import of base library | 65 |
| 46 | `--base-library` | auto? | FILE | `"base.k"` | Compiler | `base_library` | Base library file path | 66 |
| 47 | `--parse-docs` | auto? | BOOL | `False` | Compiler | `parse_docs` | Parse documentation comments | 67 |
| 48 | `--output` | auto? | `STRING` | `""` | Output | `output` | Base name of output file(s) | 68 |
| 49 | `--dgml` | auto? | FILE | `""` | Output | `dgml` | Output design in Directed Graph Markup format | 69 |
| 50 | `--dgml-detailed` | auto? | DIR | `""` | Output | `dgml_detailed` | Output each basic block in Directed Graph Markup format | 70 |
| 51 | `--resource-usage` | auto? | FILE | `""` | Output | `resource_usage` | Output resource utilization statistics | 71 |
| 52 | `--file-list` | auto? | FILE | `""` | Output | `file_list` | Update specified file whenever list compiled files (including imported files) changes | 72 |
| 53 | `--identifier-length` | auto? | INT | `100` | Output | `identifier_length` | Maximum length of identifiers in generated code | 73 |
| 54 | `--skip-circt-lowering` | auto? | BOOL | `False` | Output | `skip_circt_lowering` | Skip CIRCT lowering passes; do not emit CIRCT-lowered SystemVerilog module bodies (other outputs such as RtlMap.json and *_types.sv may still be written). Implies --write-circt-ir | 74 |
| 55 | `--backend` / `--target` | auto? | `Sv`\|`None` | `None` | Code generation | `backend` | Code generation backend: sv (SystemVerilog) or not specified (produce metadata only) | 75 |
| 56 | `--debug` | auto? | BOOL | `False` | Code generation | `debug` | Debug mode | 76 |
| 57 | `--dump-memory-metadata` | auto? | BOOL | `False` | Code generation | `dump_memory_metadata` | Output metadata files for memories and fifos | 77 |
| 58 | `--strip-debug-symbols` | auto? | BOOL | `False` | Code generation | `strip_debug_symbols` | Generate debug symbols without full file path names | 78 |
| 59 | `--force-mem-to-array` | auto? | INT | `0` | Code generation | `force_mem_to_array` | Regardless of cost, forces conversion of memories less than the specified size to arrays. For testing purposes only | 79 |
| 60 | `--max-stack-depth` | auto? | INT | `64` | Code generation | `max_stack_depth` | Maximum number of stack frames to inserted into the generated code | 80 |
| 61 | `--no-fifo-stutter` | auto? | BOOL | `False` | Code generation | `no_fifo_stutter` | Disable fifos stutter | 81 |
| 62 | `--no-debug-view` | auto? | BOOL | `False` | Code generation | `no_debug_view` | Disables debug views | 82 |
| 63 | `--no-verilog-header` | auto? | BOOL | `False` | Code generation | `no_verilog_header` | Don't output the informational header on top of generated Verilog files | 83 |
| 64 | `--code-coverage` | auto? | BOOL | `False` | Code generation | `code_coverage` | Enable generation of code coverage using SystemVerilog coverpoints | 84 |
| 65 | `--code-coverage-mux-threshold` | auto? | INT | `0` | Code generation | `code_coverage_mux_threshold` | Only generate code coverage code for muxes with this number of cases; the default value of 0 means to generate for all muxes | 85 |
| 66 | `--optimize` | `-O` | `LEVEL` | `2` | Code generation | `optimize` | Optimize generated code | 86 |
| 67 | `--target-device` | auto? | `STRING` | `"mock"` | Code generation | `target_device` | Target FPGA device name | 87 |
| 68 | `--stall` | auto? | INT | `0` | Code generation | `stall` | Randomly force each basic block to stall (acceptable values [0,3]; default is 0 meaning no stalls) | 88 |
| 69 | `--work-list-size` | auto? | INT | `0` | Code generation | `work_list_size` | Optimization work list upper bound (log2) | 89 |
| 70 | `--place-display` | auto? | BOOL | `False` | Placement | `place_display` | Display animation of the placement process | 90 |
| 71 | `--place-iterations` | auto? | INT | `3000` | Placement | `place_iterations` | The number of iterations to run placement for | 91 |
| 72 | `--place-pull` | auto? | NUM (Float) | `100.0` | Placement | `place_pull` | Coefficient of linear force that pulls connected nodes together | 92 |
| 73 | `--place-push` | auto? | NUM (Float) | `0.001` | Placement | `place_push` | Coefficient of inverse square force that pushes nodes away from others | 93 |
| 74 | `--place-seed` | auto? | INT | `1` | Placement | `place_seed` | Random number seed | 94 |
| 75 | `--place-update-rate` | auto? | NUM (Float) | `0.005` | Placement | `place_update_rate` | Maximum length of gradient vectors | 95 |
| 76 | `--place-wall` | auto? | NUM (Float) | `0.001` | Placement | `place_wall` | Coefficient of inverse cube force that pushes nodes away from walls | 96 |
| 77 | `--check-narrowing-assignment` | auto? | BOOL | `False` | Diagnostics | `check_narrowing_assignment` | Assert that the input to a narrowing conversion can be represented by the result type | 97 |
| 78 | `--dump-IR` (case: §11 Q3) | auto? | BOOL | `False` | Diagnostics | `dump_IR` | Dump IR | 98 |
| 79 | `--dump-opt` | auto? | BOOL | `False` | Diagnostics | `dump_opt` | Dump optimizations | 99 |
| 80 | `--dump-parse` | auto? | BOOL | `False` | Diagnostics | `dump_parse` | Dump abstract syntax tree(s) | 100 |
| 81 | `--dump-program` | auto? | BOOL | `False` | Diagnostics | `dump_program` | Dump desugared abstract syntax tree | 101 |
| 82 | `--dump-types` | auto? | BOOL | `False` | Diagnostics | `dump_types` | Dump abstract syntax tree with type annotations | 102 |
| 83 | `--dump-source` | auto? | BOOL | `False` | Diagnostics | `dump_source` | Dump desugared source code of the program | 103 |
| 84 | `--frontend-passes` ! | — | INT (optional value, `opt maxBound`) | `maxBound::Int` | Diagnostics | `passes` | Subset of frontend passes to run, default all | 104 |
| 85 | `--template-iterations` ! | — | INT (optional value, `opt maxBound`) | `maxBound::Int` | Diagnostics | `template_iterations` | Number of iterations of template instantiation to run, default all | 105 |
| 86 | `--template-passes` ! | — | INT (optional value, `opt maxBound`) | `maxBound::Int` | Diagnostics | `template_passes` | Subset of desugaring passes to run after the final template instantiation iteration specified via --template-iterations, default all | 106 |
| 87 | `--write-ir-post-opt` | auto? | BOOL | `False` | Diagnostics | `write_ir_post_opt` | Serialize IR to json after optimizations | 107 |
| 88 | `--write-circt-ir` | auto? | BOOL | `False` | Diagnostics | `write_circt_ir` | Serialize to CIRCT IR before CIRCT passes | 108 |
| 89 | `--no-asserts` | auto? | BOOL | `False` | Diagnostics | `no_asserts` | Disable asserts | 109 |
| 90 | `--Werror` ! | — | BOOL | `False` | Warnings | `warnings_as_errors` | Treat warnings as errors | 110 |

The 13 `warnings_state` switches (all `&= explicit`, all in group `Warnings`,
`CmdArgs.hs:111-136`) — see §3 for semantics:

| # | Long | Value appended | Help text | Src |
|---|---|---|---|---|
| 91 | `--Wall` | `[Enable Conversion, Enable TransactionSizeMissing]` | Enable all warnings | 119-121 |
| 92 | `--Wconversion` | `[Enable Conversion]` | Warn for implicit conversions that may alter a value | 123 |
| 93 | `--Wdeprecated` | `[Enable Deprecated]` | Warn for deprecated features | 124 |
| 94 | `--Winitializer-overrides` | `[Enable InitializerOverrides]` | Warn for overriding designated initializers | 125 |
| 95 | `--Wredundant-import` | `[Enable RedundantImport]` | Warn for redundant imports | 126 |
| 96 | `--Wshadow` | `[Enable Shadow]` | Warn for shadowing declarations | 127 |
| 97 | `--Wtransaction-size` | `[Enable TransactionSizeMissing]` | Warn for missing [[transaction_size]] attribute at calls to functions with the [[last]] attribute | 128 |
| 98 | `--Wno-conversion` | `[Disable Conversion]` | Don't warn for implicit conversions that may alter a value | 130 |
| 99 | `--Wno-deprecated` | `[Disable Deprecated]` | Don't warn for deprecated features | 131 |
| 100 | `--Wno-initializer-overrides` | `[Disable InitializerOverrides]` | Don't warn for overriding designated initializers | 132 |
| 101 | `--Wno-redundant-import` | `[Disable RedundantImport]` | Don't warn for redundant imports | 133 |
| 102 | `--Wno-shadow` | `[Disable Shadow]` | Don't warn for shadowing declarations | 134 |
| 103 | `--Wno-transaction-size` | `[Disable TransactionSizeMissing]` | Don't warn for missing [[transaction_size]] attribute at calls to functions with the [[last]] attribute | 135 |

### 1.2 `pretty` mode — declared flags

`compiler/hs/app/Options/CmdArgs.hs:145-153`:

| Long | Short | Val | Default | Haskell field | Help text | Src |
|---|---|---|---|---|---|---|
| `--line-width` | `-w` | INT | `80` | `line_width` | Width of the page in characters; default 80 | 147 |
| `--indent` | auto? | INT | `4` | `indent` | Number of spaces per indentation level; default 4 | 148 |
| `--ribbon` | auto? | NUM (Double) | `1.0` | `ribbon` | Fraction of line that should be filled before trying to break it up; default 1.0 | 149 |
| `--layout` | auto? | `Smart`\|`Pretty` | `Pretty` | `layout` | Layout variant to be used: pretty or smart | 150 |

### 1.3 `pretty` mode — the 8 *implicit* flags (record fields never assigned)

`PrettyPrint` declares 13 fields (`compiler/hs/app/Options.hs:23-37`) but the `pretty`
record literal assigns only 4 (`CmdArgs.hs:146-151`). The module is compiled with
`{-# OPTIONS_GHC -fno-warn-missing-fields #-}` (`CmdArgs.hs:1`), i.e. the omission is
deliberate. The omitted fields are:

`base_library`, `define`, `dump_parse`, `files`, `import_dir`, `no_implicit_base`,
`parse_docs`, `target_device`, `using` (`Options.hs:24-36`).

They **must** be populated by `cmdargs` at runtime, because `Main.hs` reads them
unconditionally in `pretty` mode: `O.files options` at `Main.hs:38` and
`base_library`/`define`/`files`/`import_dir`/`no_implicit_base`/`parse_docs`/
`target_device`/`using` at `Main.hs:163-171`. If `cmdargs` left them as GHC's
`recConError` bottoms, `kanagawa pretty x.k` would crash before doing anything.

⇒ For a compat binary: **`pretty` mode accepts `--base-library`, `--define`/`-d`,
`--dump-parse`, `--import-dir`, `--no-implicit-base`, `--parse-docs`, `--target-device`,
`--using`/`-u` plus positional `FILE…`, with the same defaults as `compile`
(`Options.hs` types; defaults per §11 Q4).** Their help text in `pretty` mode is empty and
their group is unset (no annotations were attached in `CmdArgs.hs`). **UNVERIFIED**: exact
default values `cmdargs` synthesises for unassigned fields; probe in §11 Q4.

`pretty`-mode behaviour (`Main.hs:45-55`): parse all files, on any parse error
`exitErrors`; otherwise print each file with
`renderString . engine layout . prettySource indent . removeRedundantNameScope`
where `engine Pretty = layoutPretty`, `engine Smart = layoutSmart`, and layout options are
`PP.LayoutOptions (PP.AvailablePerLine line_width ribbon)` (`Main.hs:52-55`).
Note `dump_parse` is read into `ParseOptions` for `pretty` **only via the constructor**,
never acted on in the `pretty` branch.

---

## 2. Marshalling map: Haskell field → C struct field

### 2.1 The three C structs and their exact field order

`compiler/cpp/options.h`. `typedef unsigned BOOL;` (`options.h:18`) — **4 bytes**, not 1.
`size_t` is 8 bytes on all supported targets. Haskell `Storable Bool` also writes 4 bytes
(`HTYPE_INT`), `Int` writes 8, `Float` writes 4 — which is why the untyped
`#{poke}` macros line up.

**`PlacementOptions`** (`options.h:20-42`), 7 fields:

| Ord | C field | C type | h line | Written from | FFI.hsc line |
|---|---|---|---|---|---|
| 1 | `_numIterations` | `size_t` | 23 | `place_iterations` | 120 |
| 2 | `_seed` | `size_t` | 26 | `place_seed` | 121 |
| 3 | `_pullStrength` | `float` | 29 | `place_pull` | 122 |
| 4 | `_pushStrength` | `float` | 32 | `place_push` | 123 |
| 5 | `_wallStrength` | `float` | 35 | `place_wall` | 124 |
| 6 | `_updateRate` | `float` | 38 | `place_update_rate` | 125 |
| 7 | `_display` | `BOOL` | 41 | `place_display` | 126 |

All 7 written.

**`CodeGenOptions`** (`options.h:44-273`), 67 fields, in declaration order:

| Ord | C field | C type | h line | Written from (Haskell) | FFI.hsc line |
|---|---|---|---|---|---|
| 1 | `_debug` | BOOL | 48 | `debug` | 127 |
| 2 | `_stripDebugSymbols` | BOOL | 51 | `strip_debug_symbols` | 128 |
| 3 | `_optimize` | size_t | 54 | `optimize` | 101 |
| 4 | `_assertNarrowingConversion` | BOOL | 57 | `check_narrowing_assignment` | 92 |
| 5 | `_dumpIR` | BOOL | 60 | `dump_IR` | 93 |
| 6 | `_dumpOpt` | BOOL | 63 | `dump_opt` | 94 |
| 7 | `_serializeIRPostOpt` | BOOL | 66 | `write_ir_post_opt` | 95 |
| 8 | `_serializeCIRCTIR` | BOOL | 69 | `write_circt_ir` | 96 |
| 9 | `_skipCirctLowering` | BOOL | 73 | `skip_circt_lowering` | 97 |
| **10** | **`_detectRaces`** | BOOL | **76** | **NEVER WRITTEN** | — |
| **11** | **`_controlInspection`** | BOOL | **79** | **NEVER WRITTEN** | — |
| **12** | **`_inspection`** | BOOL | **82** | **NEVER WRITTEN** | — |
| 13 | `_releaseAssert` | BOOL | 85 | `not no_asserts` | 112 |
| 14 | `_allowFifoStutter` | BOOL | 88 | `not no_fifo_stutter` | 108 |
| 15 | `_stallablePipelines` | BOOL | 91 | `stallable_pipelines` | 109 |
| 16 | `_clockGatingLevel` | size_t | 94 | `clock_gating` | 106 |
| 17 | `_clockGatingThreshold` | size_t | 97 | `clock_gating_threshold` | 107 |
| 18 | `_conditionalMemoryReads` | BOOL | 101 | `conditional_memory_reads` | 110 |
| 19 | `_semaphoreDelay` | size_t | 106 | `semaphore_delay` | 111 |
| 20 | `_codeCoverage` | BOOL | 109 | `code_coverage` | 114 |
| 21 | `_codeCoverageMuxThreshold` | size_t | 113 | `code_coverage_mux_threshold` | 115 |
| 22 | `_dumpMemoryMetadata` | BOOL | 116 | `dump_memory_metadata` | 116 |
| 23 | `_maxBlockRam` | size_t | 119 | `max_block_ram` | 72 |
| 24 | `_maxDeepRam` | size_t | 120 | `max_deep_ram` | 74 |
| 25 | `_minBlockRamUtilization` | size_t | 124 | `block_ram_util_threshold` | 73 |
| 26 | `_minDeepRamUtilization` | size_t | 125 | `deep_ram_util_threshold` | 75 |
| 27 | `_maxSelectInputs` | size_t | 128 | `max_mux_sources` | 76 |
| 28 | `_maxMulSrcWidth` | size_t | 131 | `max_mul_src_width` | 77 |
| 29 | `_maxThreadsDefault` | size_t | 134 | `max_threads_default` | 78 |
| 30 | `_maxThreadsLimit` | size_t | 138 | `max_threads_limit` | 79 |
| 31 | `_scheduleLimit` | size_t | 141 | `schedule_limit` | 80 |
| 32 | `_additionalLatency` | size_t | 144 | `additional_latency` | 68 |
| 33 | `_additionalResetCycles` | size_t | 148 | **`reset_duration_cycles`** | 69 |
| 34 | `_autoPipelineScale` | size_t | 155 | `auto_pipeline_scale` (flag `--auto-pipeline-ratio`) | 70 |
| 35 | `_autoPipelineCrossRegion` | size_t | 162 | `auto_pipeline_cross_region` | 71 |
| 36 | `_logicRegisterRatio` | size_t | 166 | `register_ratio` | 102 |
| 37 | `_maxLogicRegisterRatio` | size_t | 169 | `max_register_ratio` | 103 |
| 38 | `_resetCycles` | size_t | 172 | `reset_cycles` | 104 |
| 39 | `_resetFanOutCycles` | size_t | 175 | `reset_fan_out_cycles` | 105 |
| 40 | `_fifoWriteDelay` | size_t | 178 | `fifo_write_delay` | 98 |
| 41 | `_fifoMergeDistance` | size_t | 181 | `fifo_merge_distance` | 99 |
| 42 | `_carryChainWidthPerLogicLevel` | size_t | 184 | `carry_chain_width` | 81 |
| 43 | `_rmwMemoryWriteDelay` | size_t | 187 | `rmw_memory_write_delay` | 82 |
| 44 | `_rmwMemoryReadDelay` | size_t | 190 | `rmw_memory_read_delay` | 83 |
| 45 | `_rmwHardenedMemoryWriteDelay` | size_t | 194 | `rmw_hardened_write_delay` | 84 |
| 46 | `_rmwHardenedMemoryMaxWidth` | size_t | 200 | `rmw_hardened_max_width` | 85 |
| 47 | `_rmwHardenedMemoryMaxHeight` | size_t | 201 | `rmw_hardened_max_height` | 86 |
| 48 | `_rmwMemoryFavorArea` | BOOL | 205 | `rmw_memory_favor_area` | 87 |
| 49 | `_strictMemoryReadWrite` | BOOL | 210 | `strict_memory_read_write` | 88 |
| 50 | `_inputOutputRegisterDelay` | size_t | 213 | `io_register_delay` | 89 |
| 51 | `_memToRegThreshold` | float | 220 | `mem_to_reg` | 90 |
| 52 | `_fifoToDspThreshold` | size_t | 226 | `fifo_to_dsp` | 91 |
| 53 | `_verbosity` | size_t | 228 | `fromEnum <$> getVerbosity` (in `withOptions`, **after** `poke`) | 36 |
| 54 | `_maxStringLength` | size_t | 231 | **`identifier_length`** | 117 |
| 55 | `_stall` | size_t | 234 | `stall` | 129 |
| 56 | `_forceMemToArray` | size_t | 237 | `force_mem_to_array` | 130 |
| 57 | `_suppressDebugView` | BOOL | 240 | `no_debug_view` | 113 |
| 58 | `_logWorkListSize` | size_t | 244 | `work_list_size` | 118 |
| 59 | `_returnStages` | size_t | 247 | `return_stages` | 119 |
| 60 | `_maxStackDepth` | size_t | 250 | `max_stack_depth` | 133 |
| 61 | `_resetFanout` | BOOL | 253 | `not no_reset_fanout` | 134 |
| 62 | `_sparseRegOpt` | BOOL | 256 | `sparse_reg_opt` | 131 |
| 63 | `_carrySelect` | BOOL | 259 | `carry_select` | 132 |
| 64 | `_noVerilogHeader` | BOOL | 262 | `no_verilog_header` | 100 |
| 65 | `_enableTransactionSizeWarning` | BOOL | 265 | `Set.member TransactionSizeMissing (enabledWarnings warnings_state)` | 135 |
| 66 | `_enableDeprecationWarnings` | BOOL | 268 | `Set.member Deprecated (enabledWarnings warnings_state)` | 136 |
| 67 | `_warningsAsErrors` | BOOL | 271 | `warnings_as_errors` (`--Werror`) | 137 |

**`Options`** (`options.h:275-292`), 6 fields:

| Ord | C field | C type | h line | Written from | FFI.hsc line |
|---|---|---|---|---|---|
| 1 | `_frequency` | size_t | 278 | `frequency` | 67 |
| 2 | `_deviceName` | `const char*` | 281 | `target_device` (via `withCAString`) | 31, 38 |
| 3 | `_cmdArgs` | `const char*` | 284 | the `cmdArgs` string (§6) | 30, 37 |
| 4 | `_codegenOptions` | `CodeGenOptions` | 286 | (nested, above) | 33, 65 |
| 5 | `_placementOptions` | `PlacementOptions` | 288 | (nested, above) | 66 |
| 6 | `_fileNames` | `const char**` | 290 | NUL-terminated array of all parsed file paths | 32, 39 |

### 2.2 The three fields the Haskell NEVER writes — verified

Search of `compiler/hs/app/Options/FFI.hsc` shows **no** `#{poke}` for
`_detectRaces`, `_controlInspection`, or `_inspection`. The `Storable Options`
instance defines only `alignment`, `sizeOf` and `poke`
(`Options/FFI.hsc:60-138`; `{-# OPTIONS_GHC -Wno-missing-methods #-}` at `:1` silences
the missing `peek`), and the struct is allocated with **`alloca`**
(`Options/FFI.hsc:29`), i.e. **uninitialised stack memory**.

⇒ On entry to `SetupCodeGenConfig`, those three `BOOL`s (and all padding) hold garbage.
The C++ side compensates:

```cpp
    // Inspectables external  interface currently disabled for FOSS
    g_codeGenConfig._inspection = false;
    g_codeGenConfig._controlInspection = false;
    g_codeGenConfig._detectRaces = false;
```
— `compiler/cpp/config.cpp:140-143` (immediately after the struct copy at `config.cpp:136`
and `_isInitialized = true` at `:138`).

**Implication for the Rust compat binary:** there is no CLI flag, no environment variable
and no code path that can set these to true. A Rust rewrite should hard-code them to
`false` (or drop them entirely); it must *not* invent flags for them.

### 2.3 Fields written by `SetupCodeGenConfig`, not by the FFI

`compiler/cpp/config.cpp:136-153` copies the flat `CodeGenOptions` into the derived
`CodeGenConfig` (`compiler/cpp/config.h:90-129`) and then fills the derived-only members:

| `CodeGenConfig` member | config.h line | Value | config.cpp line |
|---|---|---|---|
| `_isInitialized` | 92 | `true` | 138 |
| `_cmdArgs` | 100 | `options._cmdArgs` | 145 |
| `_frequency[0]` | 94 | `options._frequency` | 146 |
| `_deviceName` | 103 | `options._deviceName` | 147 |
| `_moduleName` | 106 | hard-coded `"KanagawaCore"` | 148 |
| `_placementConfig` | 110 | `options._placementOptions` | 149 |
| `_bitsPerAlm` | 97 | hard-coded `64` | 150 |
| `_fileNames` | 108 | loop over the NUL-terminated `_fileNames` array | 152-153 |

### 2.4 Post-marshalling mutations and validation (must be reproduced)

All in `SetupCodeGenConfig`, `compiler/cpp/config.cpp`:

| Behaviour | Line |
|---|---|
| `throw "--reset-fan-out-cycles must be no greater than --reset-cycles"` when `_resetFanOutCycles > _resetCycles` | 162-165 |
| `throw "Logic to register ratio must be at least 1"` when `_logicRegisterRatio < 1` | 167-170 |
| `throw "No input files specified"` when `_fileNames` empty | 172-175 |
| `throw "Invalid -- clock gating level must be equal to 0"` when `_clockGatingLevel > 0` | 177-181 |
| `throw "Invalid -- stallable pipeline feature must be disabled"` when `_stallablePipelines` | 182-186 |
| **`_placementConfig._numIterations = 1` when `_optimize < 2`** (silently overrides `--place-iterations`) | 188-192 |
| `throw "Invalid --stall value not in [0,4]"` when `_stall > 4` (note: help text says `[0,3]`) | 194-195 |
| `throw "--max-mux-sources must be at least 2"` when `_maxSelectInputs < 2` | 197-198 |
| `throw "--max-mux-sources must be a power of 2"` | 200-201 |
| `throw "--max-threads-default must be a power of 2"` | 203-204 |
| `throw "--max-threads-limit must be a power of 2"` | 206-207 |

Derived at use time (not at setup):

| Behaviour | Line |
|---|---|
| `MaximumWorkListSize()`: if `_logWorkListSize == 0`, use `16` when `_optimize < 2` else `20`; `throw "Work list size is too large"` if `>= 64`; result is `1ull << log` | `config.cpp:95-118` |
| `PipelineClockGatingEnabled() = _clockGatingLevel > 0` | 70 |
| `ControlClockGatingEnabled() = _clockGatingLevel > 1` | 72 |
| `PathLengthReportEnabled() = !_debug` | 79 |
| `GetMaxBypassSlots() = 2 + _rmwMemoryReadDelay + max(_rmwMemoryWriteDelay, _rmwHardenedMemoryWriteDelay)` | 81-87 |
| `GetInvocationIndexSize() = Log2(_maxThreadsLimit)` | 120 |
| `GetMaxSelectIndexOptRecursion() = _optimize > 1 ? 32 : 4` | 122 |
| `AdjustCodeGenConfigForLargeAndFast()`: `++_fifoWriteDelay; ++_resetCycles; _carryChainWidthPerLogicLevel /= 2; ++_additionalLatency` | 226-239 |

### 2.5 Flags that are NOT marshalled into any C struct (20 of the 90)

These reach the backend by other channels or stay in the Haskell frontend:

| Flag | Where it goes |
|---|---|
| `--jobs` | `setNumCapabilities` (`Main.hs:62-68`) |
| `--import-dir` | `ParseOptions.importDir` → import search path (`Parser/IO.hs:107`) |
| `--define` | `ParseOptions.define` → `.cmdargs` module text (`Parser/IO.hs:49-55`) |
| `--using` | `ParseOptions.using` → `.cmdargs` module text (`Parser/IO.hs:49-55`) |
| `--no-implicit-base` | `ParseOptions.noImplicitBase` (`Parser/IO.hs:72-76`) |
| `--base-library` | `ParseOptions.baseLibrary` (`Parser/IO.hs:75`) |
| `--parse-docs` | lexer `Config` (`Parser/IO.hs:92`) |
| `--output` | `Codegen(...)` arg 2 (`ParseTree.hs:34,48`) |
| `--dgml` | `Codegen(...)` arg 4 (`ParseTree.hs:36`; `kanagawa.cpp:35-38,204-207`) |
| `--dgml-detailed` | `Codegen(...)` arg 5 (`ParseTree.hs:37`; `kanagawa.cpp:45-48,219-222`) |
| `--resource-usage` | `Codegen(...)` arg 3 (`ParseTree.hs:35`; `kanagawa.cpp:40-43,209-217`) |
| `--backend`/`--target` | `Codegen(...)` arg 1 as `show backend` = `"Sv"`/`"None"`, compared case-insensitively against `"sv"` (`ParseTree.hs:33`; `kanagawa.cpp:30-33`); also `.options::backend` |
| `--file-list` | `writeIfChanged … renderDeps` (`Main.hs:78-79`; §4.6) |
| `--dump-parse` | `Main.hs:81-82` |
| `--dump-types` | `Main.hs:83-84` |
| `--dump-program` | `Main.hs:92-93` |
| `--dump-source` | `Main.hs:94-95` |
| `--frontend-passes` | `frontend` (`Main.hs:80`) |
| `--template-iterations` | `frontend` (`Main.hs:80`) |
| `--template-passes` | `frontend` (`Main.hs:80`) |

Additionally, four *marshalled* options are **also** consumed by the frontend and injected
into the synthetic `.options` module: `--stall`, `--optimize`, `--max-threads-default`,
`--max-threads-limit`, plus `--backend` (`Parser/IO.hs:57-64`).

---

## 3. `--W<name>` / `--Wno-<name>` / `--Wall` / `--Werror`

### 3.1 The 6 warning kinds

```haskell
data WarningKind =
     Conversion |
     Deprecated |
     InitializerOverrides |
     RedundantImport |
     Shadow |
     TransactionSizeMissing
```
— `compiler/hs/lib/Language/Kanagawa/Warning.hs:20-27`

Wire names (`Warning.hs:29-35`):

| Constructor | `show` | Enable flag | Disable flag |
|---|---|---|---|
| `Conversion` | `conversion` | `--Wconversion` | `--Wno-conversion` |
| `Deprecated` | `deprecated` | `--Wdeprecated` | `--Wno-deprecated` |
| `InitializerOverrides` | `initializer-overrides` | `--Winitializer-overrides` | `--Wno-initializer-overrides` |
| `RedundantImport` | `redundant-import` | `--Wredundant-import` | `--Wno-redundant-import` |
| `Shadow` | `shadow` | `--Wshadow` | `--Wno-shadow` |
| `TransactionSizeMissing` | **`transaction-size`** | `--Wtransaction-size` | `--Wno-transaction-size` |

Name construction (`Warning.hs:40-44`):
```haskell
enableSwitch  = ("W"    ++) . show
disableSwitch = ("Wno-" ++) . show
```
Note `TransactionSizeMissing`'s wire name drops the `Missing` suffix (`Warning.hs:33`).

### 3.2 State accumulation

The field is a **list**, not a set:
`warnings_state :: [WarningState WarningKind]` (`compiler/hs/app/Options.hs:125`), where
`data WarningState a = Enable a | Disable a` (`Warning.hs:37`).

Defaults (first `enum` alternative, `&= ignore` ⇒ no flag of its own,
`CmdArgs.hs:112-117`):
```haskell
[ Enable RedundantImport, Enable Deprecated, Enable InitializerOverrides, Enable Shadow ]
```
⇒ **default-on: `redundant-import`, `deprecated`, `initializer-overrides`, `shadow`.
Default-off: `conversion`, `transaction-size`.**

`--Wall` (`CmdArgs.hs:118-121`) contributes `[Enable Conversion, Enable TransactionSizeMissing]`
— i.e. it turns on exactly the two that are off by default; it does **not** re-enable
anything already disabled earlier on the command line.

Resolution (`Warning.hs:46-50`):
```haskell
enabledWarnings :: [WarningState WarningKind] -> Set WarningKind
enabledWarnings = foldl (flip update) mempty
  where
    update (Enable a) = Set.insert a
    update (Disable a) = Set.delete a
```
Left fold ⇒ **last occurrence wins**, and the default block is at the head of the list.

⇒ Required semantics for a compat binary:
`enabled = fold over ([defaults] ++ concat(per-flag contributions in command-line order))`,
`Enable` = insert, `Disable` = remove.

**Corroborating evidence that the flags *append* to (rather than replace) the list:** the
logic-test suites pass `--Wall --Werror` (`build/cmake/test_helper.cmake:381-382`) *and*
`--Wno-initializer-overrides --Wno-shadow` (`test/logic/CMakeLists.txt:130-131`). If a later
flag replaced the list, the effective set would be `{}` (or `{shadow}`) and `--Wall
--Werror` would be inert; the tests would not be testing anything. With append + left-fold
the effective set is `{redundant-import, deprecated, conversion, transaction-size}`, which
is the evident intent. (The `cmdargs` mechanism producing the append is external and
therefore **UNVERIFIED**; §11 Q5.)

### 3.3 Where the warning set is consumed

1. **Frontend warnings** — `warnings (enabledWarnings warnings_state) untypedProgram`
   (`Main.hs:96`); each rendered with `errorBundlePretty` to **stdout** (`Main.hs:97`).
   Message shape (`compiler/hs/lib/Language/Kanagawa/Error.hs:142-145`):
   ```haskell
   message = "warning: " ++ msg ++ " [--" ++ disableSwitch kind ++ "]"
   ```
   ⇒ every frontend warning line ends with `` [--Wno-<name>] ``. The syntax harness
   depends on exactly this (`test/syntax/syntax-test.py:122`,
   regex `warning:.*\[--Wno-([^\]]*)\]$`).
2. **Backend warning enables** — only two of the six cross the FFI:
   `_enableTransactionSizeWarning` ← `TransactionSizeMissing ∈ enabled`
   (`Options/FFI.hsc:135`) and `_enableDeprecationWarnings` ← `Deprecated ∈ enabled`
   (`Options/FFI.hsc:136`). The other four are frontend-only.

### 3.4 `--Werror`

* Flag: `--Werror`, `&= explicit` (`CmdArgs.hs:110`), field `warnings_as_errors`, default `False`.
* **Frontend**: if any warning was emitted, `exitError "Error 1: Warnings treated as errors"`
  → stdout + exit 1 (`Main.hs:98-99`). This happens *before* `compile` is called, i.e.
  no backend runs and no output files are written.
* **Backend**: `_warningsAsErrors` (`options.h:271`) makes
  `Compiler::WarningStream` set `_compileError = true`
  (`compiler/cpp/compiler.cpp:181-188`), which later turns into a thrown
  `runtime_error` and a failed `Codegen`.

### 3.5 Backend-side warnings (independent of the 6 frontend kinds)

`enum class CompileWarning` (`compiler/cpp/errors.h:86-92`): `TooManyRams = 8`,
`LastWithoutTransactionSize = 10`, `NonConstGlobal = 12`, `FreeNonInlineFunction = 13`.
Rendered as `Warning <N> at: <file> (<line>): <msg>` or `Warning <N>: <msg>`
(`errors.h:113-123`) — note: **no** `[--Wno-…]` suffix, so the syntax harness's warning
regex never matches backend warnings.

---

## 4. Positional arguments and the source-resolution flags

### 4.1 Positional arguments

`files = def &= typFile &= args` (`CmdArgs.hs:20`). All non-flag arguments, in order, go
into `files :: [FilePath]`. Empty ⇒ `Missing source filename(s)` + exit 1 (`Main.hs:38-39`).

Each is parsed in list order by `forM (files opt) (parseFile opt symbolTable)`
(`Parser/IO.hs:69`). Resolution of a top-level file is relative to the **process CWD**
(`Parser/IO.hs:80-82`: `cwd <- getCurrentDirectory`, then `findFile (cwd:importDirs) file`
via `readFileContent`, `Parser/IO.hs:103`).

### 4.2 `--import-dir` (repeatable) and `--target-device`

```haskell
importDirs = map (</> "device" </> targetDevice opt) (importDir opt) ++ importDir opt
```
— `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:107`

⇒ The search order for `import X` inside a file located in directory `D` is:
```
D
<importDir₁>/device/<target-device>
<importDir₂>/device/<target-device>
…
<importDirₙ>/device/<target-device>
<importDir₁>
<importDir₂>
…
<importDirₙ>
```
(`findFile (dir:importDirs) file`, `Parser/IO.hs:103`; `dir` = directory of the importing
file, `Parser/IO.hs:94`). **All** `device/<target>` directories come before **any** plain
import dir. Miss ⇒
`"Can't find file: " ++ file ++ " in any of the import directories\n" ++ unlines importDirs`
(`Parser/IO.hs:106`).

`--target-device` default `"mock"` (`CmdArgs.hs:87`, matching
`Parser/Options.hs:47`). Device directories that exist in-tree
(`library/device/`): `agilex-7-f-023`, `agilex-7-i-041`, `arria-10-gx-1150`, `mock`,
`stratix-10-gx-2800`, `stratix-10-nx-2100`, `ultrascaleplus-xcu250`.
The value is also marshalled to `Options._deviceName` (`Options/FFI.hsc:31,38`).

Files are canonicalised (`canonicalizePath`, `Parser/IO.hs:96`) and memoised in a
state-monad cache keyed on the canonical path (`Parser/IO.hs:89-99`), so a file is parsed
once no matter how many times it is imported. A file that appears in its own ancestor
chain yields `"Circular dependency"` (`Parser/IO.hs:88`).

### 4.3 `--base-library` and `--no-implicit-base`

```haskell
    importBase = do
        let baseFile
              | noImplicitBase opt || null (importDir opt) = cmdargsSpecialModule <.> "k"
              | otherwise                                  = baseLibrary opt
        b <- parseFile opt emptySymbolTable baseFile
        return $ addImport Needed 0 Nothing emptySymbolTable . snd <$> b
```
— `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:71-76`

Two independent triggers suppress the base library: `--no-implicit-base`
**or an empty `--import-dir` list**. In that case the implicitly-imported "base" is the
synthetic `.cmdargs.k` module instead. Default `--base-library` is `"base.k"`
(`CmdArgs.hs:66`), resolved through the same `importDirs` search.

Whatever is chosen is parsed first and its symbol table seeds every subsequent file
(`Parser/IO.hs:67-69`).

In-tree usage: `--base-library=<root>/library/mini-base.k` (syntax tests,
`test/syntax/CMakeLists.txt:43`; CLI test, `test/compiler/cli/CMakeLists.txt:101`),
`--base-library=<root>/library/base.k` (`test/interface/circt/CMakeLists.txt:67`),
`--base-library helper/custom_base.k` **space-separated** (`test/syntax/CMakeLists.txt:91`),
and the abbreviation `--base=<...>/mini-base.k` (`tools/chkdoc/chkdoc.cpp:208`) — the only
in-tree evidence that `cmdargs` accepts unambiguous long-flag prefixes.

### 4.4 `--define` and `--using` → the `.cmdargs` module

```haskell
buildCmdargsSpecialModule opt = buildModule cmdargsSpecialModule [] (nameValue <$> define opt) (nameValue <$> using opt)
  where
    nameValue [] = ([], [])
    nameValue (x:xs')
       | x == '#' || x == '=' = ([], xs')
       | otherwise            = let (ys, zs) = nameValue xs' in (x:ys, zs)
```
— `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:49-55`

* Separator is the **first** `#` **or** `=` in the argument. Both are in use in-tree:
  `--define=verbose#true` (`build/cmake/test_helper.cmake:391`) and
  `--define=MemoryLength=32` (`tools/chkdoc/chkdoc.cpp:41`).
* An argument with no separator yields `(whole, "")` ⇒ emits `const auto NAME = ;`, a parse
  error. An empty argument yields `("", "")`.
* Module name: `.cmdargs` (`compiler/hs/lib/Language/Kanagawa/Parser.hs:41-42`).
* Generated text (`Parser/IO.hs:37-47`):
  ```
  module .cmdargs{module .cmdargs}
  const auto <name> = <val>;      -- one per --define, in command-line order
  using <name> = <type>;          -- one per --using,  in command-line order
  ```
* `.cmdargs.k` is intercepted before the filesystem is touched (`Parser/IO.hs:100-101`), so
  it can also be imported explicitly (`doc/programming-guide.md:4503-4534`).

### 4.5 The `.options` module

```haskell
buildOptionsSpecialModule ParseOptions{..} = buildModule optionsSpecialModule ["compiler.options"] options []
  where
    options = [ ("backend",             "Backend::" ++ show backend)
              , ("stall",               show stall)
              , ("optimize",            show optimize)
              , ("max_threads_default", show maxThreadsDefault)
              , ("max_threads_limit",   show maxThreadsLimit)
              ]
```
— `compiler/hs/lib/Language/Kanagawa/Parser/IO.hs:57-64`

Module name `.options` (`Parser.hs:44-45`), intercepted at `Parser/IO.hs:102`, never
imported implicitly. `Backend` is `Sv | None` (`Parser/Options.hs:16`,
`library/compiler/options.k:8-12`).

**Doc drift to be aware of:** `doc/programming-guide.md:4545-4546` claims `.options` also
exposes `target_device` and `target_device_name`. It does not — `Parser/IO.hs:59-64` lists
exactly five constants. Reproduce the code, not the doc.

### 4.6 `--file-list` and `list-deps`

Rewritten for the pin: upstream #136 (`ac73081`, 2026-08-27) moved the write and
shared it with the new `list-deps` mode. At `8e0a3fb` the list was written by
`updateFileList` *after* `compile`, whether or not codegen succeeded, and compared
line lists rather than whole contents.

```haskell
            -- Refresh the dependency manifest as soon as parsing succeeds, so
            -- that it is up to date even when the frontend or codegen fails.
            when (not (null file_list) && not (null fileNames)) $
                writeIfChanged file_list $ renderDeps fileNames
```
— `compiler/hs/app/Main.hs:76-79` (`compile`)

```haskell
renderDeps :: [FilePath] -> String
renderDeps = unlines . filter (('.' /=) . head) . sort

writeIfChanged :: FilePath -> String -> IO ()
writeIfChanged path content = do
    exists <- doesFileExist path
    old <- if exists
        then T.unpack <$> TIO.readFile path
        else return ""
    when (old /= content) $
        writeFile path content
```
— `compiler/hs/app/Main.hs:132-144`

Exact behaviour to reproduce:

1. In `compile`, runs as soon as every file parsed (`Main.hs:73-79`) — **before** the
   frontend, so the list is refreshed even when the frontend, `--Werror` or codegen then
   fails. A parse error exits first (`Main.hs:74`) and writes nothing.
2. Input is `fileNames` = the canonical paths of **every** parsed file, including the base
   library and every transitively imported file, in parse-completion order
   (`Main.hs:70`, `Parser/IO.hs:96`).
3. `sort` (byte-wise ascending), **then** `filter` out anything whose first character is
   `.`. This is precisely what removes the two synthetic modules, whose "canonical path"
   is literally `.cmdargs.k` / `.options.k` (`Parser/IO.hs:101-102` return `file`
   unchanged). It also removes any real file whose *path* starts with `.`
   (e.g. a relative `./foo.k` — but paths here are canonicalised, so in practice only
   the synthetic ones).
4. Write is **conditional**: only if the whole text differs from the existing file's (a
   missing file reads as `""`), so an unchanged list keeps its mtime. Output is
   `unlines` (trailing newline after every entry).
5. `head` on an empty path would `error`; unreachable in practice.

`list-deps` (`Main.hs:111-122`) runs the same parse, exits through `exitErrors` on a
parse error, and otherwise renders the same list: to stdout when `--file-list` is empty,
else through `writeIfChanged`; exit 0 either way. Upstream's own test runs both and
requires byte-identical lists (`test/compiler/cli/CMakeLists.txt:108-144`, §8.7), the
only in-tree callers of `--file-list`.

---

## 5. `--frontend-passes` / `--template-passes` / `--template-iterations`

### 5.1 Plumbing

```haskell
let desugared = foldr1 append $ frontend passes template_passes template_iterations exprs
```
— `compiler/hs/app/Main.hs:80`

```haskell
frontend frontendPasses templatePasses templateIterations = desugarPasses frontendPasses templatePasses templateIterations . map noteType
```
— `compiler/hs/lib/Language/Kanagawa/Frontend.hs:34-35`

### 5.2 `--frontend-passes=N` — verified: N selects a **suffix** of the list

```haskell
desugarPasses frontendPasses templatePasses templateIterations program = fst $ foldr ($) (program, False) $ drop (length passes - frontendPasses) passes
```
— `compiler/hs/lib/Language/Kanagawa/Frontend.hs:38`

`foldr ($) z [f₁,…,fₙ] = f₁ (f₂ (… (fₙ z)))`, so **the list is written in composition order
and executes bottom-to-top**: the *last* element runs *first*.

`drop (length passes - N) passes` keeps the last `N` elements — i.e. **the first `N`
passes in execution order**. The plan's claim is **confirmed**.

Edge cases:
* `N = maxBound::Int` (the default, `CmdArgs.hs:104`): `19 - maxBound` is a large negative
  `Int` (no overflow); `drop` with a negative count returns the whole list.
* `N = 0`: `drop 19` ⇒ `[]` ⇒ `foldr` returns `(program, False)` unchanged — **no frontend
  passes run at all**, and the program goes straight to codegen.
* `N > 19`: same as the default.
* Bare `--frontend-passes` with no value is legal (`&= opt (maxBound::Int)`,
  `CmdArgs.hs:104`) and means `maxBound`.

`passes` has **19** entries (`Frontend.hs:54-80`). Execution order (index 19 first):

| Exec order | List idx | Pass | Frontend.hs line |
|---|---|---|---|
| 1 | 19 | `checkPar $ desugar $ validateTypedLiterals . interpolatedStringPre . freeNonInlineFunctions . localMemberFunctions . redundantNameScope symbols` | 74-79 |
| 2 | 18 | `passPar . desugar' . higherOrderFunctions` (with symbols) | 73 |
| 3 | 17 | `passPar $ desugar $ captureThis . reifyEnums` | 72 |
| 4 | 16 | `passPar . deduceTemplateArgs` (with symbols) | 71 |
| 5 | 15 | `passPar $ intrinsics` | 70 |
| 6 | 14 | `pass $ inferType` | 69 |
| 7 | 13 | `pass $ trimExternClasses` | 68 |
| 8 | 12 | `instantiateTemplates templatePasses templateIterations` | 67 |
| 9 | 11 | `undefinedSymbols` (with symbols) | 66 |
| 10 | 10 | `passPar $ cata typeErrors` | 65 |
| 11 | 9 | `passPar $ unresolvedFunctionTemplateArgs` | 64 |
| 12 | 8 | `passPar . unresolvedTemplates` (with symbols) | 63 |
| 13 | 7 | `pass . validateExportableClasses` (with symbols) | 62 |
| 14 | 6 | `passPar . validateExtern` (with symbols) | 61 |
| 15 | 5 | `passPar $ desugar removeTemplate` | 60 |
| 16 | 4 | `passPar . postDesugar` (with symbols) | 59 |
| 17 | 3 | `passPar $ deduceAuto` | 58 |
| 18 | 2 | `passPar $ desugar interpolatedStringPost` | 57 |
| 19 | 1 | `passPar $ desugar' $ validateProgram symbols` (with symbols) | 55-56 |

Cross-check that execution really is bottom-to-top: `interpolatedStringPre` is inside
list-index 19 and `interpolatedStringPost` is list-index 2 — "pre" must run before "post",
which only holds if the last entry runs first.

Error short-circuiting: `pass`/`passPar` are no-ops once the `Bool` accumulator is `True`
(`Frontend.hs:42-46`); only `checkPar` (`Frontend.hs:48-51`) and `undefinedSymbols`
(`Frontend.hs:86-87`) *set* it. So the accumulator can only become `True` at exec-order 1
(`checkPar`, list idx 19) or exec-order 9 (`undefinedSymbols`, list idx 11), and after that
every remaining pass is skipped while the tree is passed through unchanged.

### 5.3 `--template-iterations=N` and `--template-passes=M`

```haskell
instantiateTemplates templatePasses templateIterations (program, hasErrors)
    | hasErrors = (program, hasErrors)
    | hasErrors' = (program', hasErrors')
    | templateIterations == 0 || Map.null decidedInstances = (parallel (desugar deduplicateInstances) program', False)
    | otherwise = instantiateTemplates templatePasses (templateIterations - 1) (inferType $ parallel (desugar (instantiate . resolve)) program', False)
  where
    program' = foldr ($) program passes'

    passes'
        | templateIterations == 0 = drop (length passes - templatePasses) passes
        | otherwise = passes
```
— `compiler/hs/lib/Language/Kanagawa/Template.hs:53-64`

`passes` here has **9** entries (`Template.hs:71-81`), again applied bottom-to-top by
`foldr ($)`:

| Exec order | List idx | Pass | Template.hs line |
|---|---|---|---|
| 1 | 9 | `parallel . desugar . addLambdaWrappers` (symbols with `NoTemplate` config) | 79-80 |
| 2 | 8 | `parallel $ desugarLambdas` | 78 |
| 3 | 7 | `parallel $ desugarShallow' abbreviatedFunctionTemplate` | 77 |
| 4 | 6 | `parallel $ moveFunctions` | 76 |
| 5 | 5 | `inferType` | 75 |
| 6 | 4 | `parallel $ desugar closureCalls` | 74 |
| 7 | 3 | `parallel . deduceTemplateArgs` (with symbols) | 73 |
| 8 | 2 | `inferType` | 72 |
| 9 | 1 | `parallel . templateTypeCheck` (with symbols) | 71 |

Exact semantics:

1. Each recursion runs all 9 desugaring passes, then decides whether to instantiate again.
2. `--template-iterations=N` bounds the number of instantiation rounds. The counter
   decrements on every recursive call (`Template.hs:58`). Recursion also stops naturally
   when no instance is fully resolved (`Map.null decidedInstances`, `Template.hs:57`),
   or immediately on error (`Template.hs:55-56`).
3. **`--template-passes=M` only has an effect on the round in which
   `templateIterations == 0`** (`Template.hs:63`). Since the default
   `template_iterations` is `maxBound::Int` (`CmdArgs.hs:105`), the counter never reaches
   0 in any realistic run ⇒ **`--template-passes` is a no-op unless
   `--template-iterations` is also given.** This is the single most surprising thing in
   this section.
4. When it does apply, `drop (9 - M) passes` again keeps the **last `M` list entries** =
   the **first `M` in execution order**, exactly like `--frontend-passes`.
5. `--template-iterations=0` ⇒ the very first call already has `templateIterations == 0`:
   only the `M`-truncated pass list runs, then `deduplicateInstances`, and no template is
   ever instantiated.
6. `--template-passes=0` with `--template-iterations=N>0` ⇒ the final round runs **zero**
   desugaring passes before `deduplicateInstances`.

In-tree usage: `--template-iterations=1` in the `base-test` syntax test
(`test/syntax/CMakeLists.txt:76`). `--template-passes` and `--frontend-passes` are never
used by the test suite.

---

## 6. The `_cmdArgs` string

### 6.1 Construction

```haskell
main = do
    args <- getArgs
    prog <- getProgName
    ...
        else handle options $ unwords $ prog : args
```
— `compiler/hs/app/Main.hs:34-40`

* `prog = getProgName` — GHC's `System.Environment.getProgName` returns the **basename of
  `argv[0]`**: everything up to and including the last path separator is dropped
  (`/` on POSIX; `/` or `\` on Windows). It does **not** strip an `.exe` suffix and does
  **not** resolve symlinks. (base library behaviour; **UNVERIFIED** against a vendored
  copy — not in this tree.)
* `args = getArgs` — `argv[1..]` **verbatim**, exactly as the OS delivered them.
* The string is `unwords (prog : args)` = space-joined, **with no quoting or escaping**.
  An argument containing spaces (e.g. `--import-dir=/My Libs`) is flattened
  irreversibly into the recorded string.
* The `--help=all` substitution when `args` is empty (`Main.hs:37`) uses `withArgs`, so it
  affects only option parsing; `args` (and hence `_cmdArgs`) stays `[]`. Irrelevant in
  practice because that path prints help and exits before `handle`.
* Note the mode word is included when present: `kanagawa compile foo.k` records
  `"kanagawa compile foo.k"`.
* `program "kanagawa"` (`CmdArgs.hs:168`) is **not** used here — it only names the program
  in `--help` output. If the binary is installed as `kanagawa-1.2.3`, `_cmdArgs` starts
  with `kanagawa-1.2.3`.

### 6.2 Transport

`handle opt@Compile{..} cmdArgs` (`Main.hs:61`) → `compile opt cmdArgs fileNames program`
(`Main.hs:101`) → `ParseTree.compile` (`compiler/hs/app/ParseTree.hs:31-38`) →
`withOptions opt cmdArgs allFiles initCompiler` (`ParseTree.hs:38`) →
```haskell
    withCAString cmdArgs $ \strCmdArgs -> …
      #{poke Options, _cmdArgs} ptr strCmdArgs
```
— `compiler/hs/app/Options/FFI.hsc:30,37`

`withCAString` = **Latin-1 truncating** narrow conversion (one byte per `Char`, high bits
dropped), not UTF-8. Non-ASCII characters in `argv` are therefore mangled in the recorded
string. Then `g_codeGenConfig._cmdArgs = options._cmdArgs;` (`compiler/cpp/config.cpp:145`,
copied into a `std::string`, `compiler/cpp/config.h:100`).

### 6.3 The single consumer

```cpp
    void Compile()
    {
        if (!GetCodeGenConfig()._noVerilogHeader)
        {
            _writer.Str() << "// This file was generated. Do not modify.";
            _writer.Str() << "// " << GetCodeGenConfig()._cmdArgs << std::endl;
        }
```
— `compiler/cpp/verilog.cpp:7372-7378`

That is the **only** read of `_cmdArgs` in the entire compiler
(grep over `compiler/`: `config.h:100`, `options.h:284`, `config.cpp:145`,
`verilog.cpp:7377`, `internal_tests.cpp:2520`, `Options/FFI.hsc:37`).

⇒ **Consequence for argv[0] handling in the compat binary:** the second line of every
generated `.sv` file is `// <basename(argv[0])> <args…>`. Reproducing generated RTL
byte-for-byte therefore requires (a) using the basename of `argv[0]` rather than a
hard-coded program name, and (b) joining the *original* argv with single spaces without
re-quoting. Suppressed entirely by `--no-verilog-header` (`verilog.cpp:7374`).

---

## 7. Exit codes

The Haskell driver only ever produces **0** or **1** (`exitSuccess` / `exitFailure`;
`System.Exit.exitFailure = exitWith (ExitFailure 1)`).

| Outcome | Where | stdout / stderr | Exit |
|---|---|---|---|
| No arguments at all ⇒ `--help=all` | `Main.hs:37` | full help to **stdout** | **0** |
| `--help` / `--help=<fmt>` | `cmdargs` | help to **stdout** | **0** |
| `--version` / `-V` / `--numeric-version` | `cmdargs` | version to **stdout** | **0** |
| Unparseable command line (unknown flag, bad value, ambiguous prefix) | `cmdargs` | error to **stderr** | **1** (UNVERIFIED, §11 Q6) |
| Positional file list empty after parsing | `Main.hs:38-39` | `Missing source filename(s)` to stdout | **1** |
| Parse error(s) in any source file | `Main.hs:49`, `Main.hs:74` → `Parser/IO.hs:113-117` | `Error 1: ` ++ concatenated `errorBundlePretty` to **stdout** | **1** |
| Desugar / frontend error(s) | `Main.hs:87` → `Parser/IO.hs:113-117` | `Error 1: …` to stdout | **1** |
| `--Werror` and ≥1 frontend warning | `Main.hs:98-99` | warnings, then `Error 1: Warnings treated as errors` to stdout | **1** |
| `SetupCodeGenConfig` validation failure (§2.4) | `parse_tree.cpp:6283-6298` | `e.what()` + newline to **stdout**, **no** `Error N:` prefix | **1** |
| Any `Compiler()`/`Init()` exception | `parse_tree.cpp:6291-6296` | `e.what()` to stdout | **1** |
| Backend semantic error (`Error <N> at: <file> (<line>): <msg>`) | `errors.h:106-111`, `compiler.cpp:173-178` | error text (in red) to **stdout**, then `Error: <what>` from `kanagawa.cpp:255-259` | **1** |
| Backend `std::runtime_error` (IR gen, scheduling, …) | `kanagawa.cpp:255-260` | `Error: ` ++ `e.what()` to stdout | **1** |
| Success | `Main.hs:102-103` | generated files | **0** |
| `list-deps`: parse error(s) | `Main.hs:115-116` | `Error 1: …` to stdout | **1** |
| `list-deps`: success | `Main.hs:118-122` | the sorted file list to stdout, or to `--file-list` | **0** |
| Unhandled non-`runtime_error` C++ exception | — | `std::terminate` | **134 / SIGABRT** (platform-dependent; UNVERIFIED) |
| Haskell `error`/`undefined` reached (e.g. `ParseTree.hs:71,74,77,…`) | GHC RTS | `<prog>: <msg>` + `CallStack` to **stderr** | **1** |

Notes for the compat binary:

* **There is no distinct exit code for "frontend error" vs "backend error".** The syntax
  harness distinguishes them purely by scraping stdout for `Error (\d+)`
  (`test/syntax/syntax-test.py:116-119`) and asserts
  `expected_error != 0 ⇒ exit_code == 1`, `expected_error == 0 ⇒ exit_code == 0`
  (`syntax-test.py:158-163`). It also treats a line whose stripped content is exactly
  `Call Stack:` as an "internal compiler error" (`syntax-test.py:126-127`).
* Frontend diagnostics are always `Error 1:` (the literal string is hard-coded at
  `Parser/IO.hs:114`). The **numbered** codes 2–84 come from the C++
  `enum class CompileError` (`compiler/cpp/errors.h:6-84`) and are printed as
  `Error <N> at: <file> (<line>): <msg>` (`errors.h:110`). The syntax corpus exercises
  codes: 0, 1, 5, 7, 9, 12, 13, 14, 16, 17, 18, 20, 21, 22, 24, 25, 26, 27, 29, 32, 33,
  34, 35, 36, 37, 53, 54, 56, 57, 62, 63, 64, 65, 67, 69, 75, 77, 79
  (extracted from `expected:<n>` markers in `test/syntax/*.k`).
* **Everything goes to stdout, not stderr** — errors, warnings and the informational
  `Running internal consistency checks.` line (`kanagawa.cpp:246`). The only stderr writer
  is `cmdargs`' own argument-parse failure path and the GHC RTS.
* `Codegen` explicitly flushes `std::cout` on both paths because the caller is Haskell and
  will not flush for it (`kanagawa.cpp:258, 262-264`).

---

## 8. Everything the test suite passes

### 8.1 The CMake driver

`add_kanagawa()` (`build/cmake/test_helper.cmake:24-121`) builds the command line as:

```
$<TARGET_FILE:kanagawa::exe> ${OPTIONS} --output "${OUTPUT_DIR}/${OUTPUT_PREFIX}" ${SOURCES}
```
— `test_helper.cmake:90-93`, run with `WORKING_DIRECTORY ${WORKING_DIRECTORY}`
(default `${CMAKE_CURRENT_SOURCE_DIR}`, `test_helper.cmake:43-45,99`), `VERBATIM`
(no shell), `COMMAND_EXPAND_LISTS`.

Two structural facts a compat binary must handle:

1. **`--output` always ends in `/` in the Verilator test path**, because
   `add_kanagawa_verilator_test` never sets `OUTPUT_PREFIX`
   (`test_helper.cmake:405-411`), so `KANAGAWA_OUTPUT = "${OUTPUT_DIR}/"`
   (`test_helper.cmake:83`). The output base name is a *prefix string*, not a stem:
   `_baseFileName = defaultFileName + FixupString(exportedName)`
   (`compiler/cpp/compiler.cpp:1976`), producing e.g. `<dir>/_test_runner_main.sv`.
2. **`--output` comes after the user options and before the sources**, so a user-supplied
   `--output` earlier in `OPTIONS` would be overridden (last-wins).

`add_kanagawa_verilator_test()` prepends a fixed option set to every test
(`build/cmake/test_helper.cmake:380-396`):

```
--Wall
--Werror
--check-narrowing-assignment
--parse-docs
--max-block-ram=10000000
--frequency=250
--reset-cycles=5
--register-ratio=3
--backend=sv
--import-dir=${CMAKE_SOURCE_DIR}/library
--define=verbose#true
```
Call this **BASE**. Per-test `OPTIONS` are appended after it (`test_helper.cmake:394-396`).

### 8.2 The 7 logic-test option suites

Every suite runs the same 100 sources (`test/logic/CMakeLists.txt:8-105`) with

**BASE** + **LOGIC-COMMON** (`test/logic/CMakeLists.txt:129-137`):
```
--Wno-initializer-overrides
--Wno-shadow
--define=ECC_SUPPORT#true
--using=COMPILE_PARAM_1#uint8
--define=COMPILE_PARAM_2#9
--define=tag#0
--import-dir=${CMAKE_CURRENT_SOURCE_DIR}     # = <root>/test/logic
```
+ the suite's own flags (`test/logic/CMakeLists.txt:179-185`):

| Suite | Extra flags | CMakeLists line | Net effect |
|---|---|---|---|
| `DEFAULT` | *(none)* | 179 | `-O2` (default), all defaults |
| `O0` | `-O0` | 180 | `_optimize=0`; also forces `_placementConfig._numIterations = 1` (`config.cpp:189-192`) and `MaximumWorkListSize()` → `1<<16` |
| `O1` | `-O1` | 181 | `_optimize=1`; same two side effects as `O0` |
| `RMWR1` | `--rmw-memory-read-delay=1` | 182 | `_rmwMemoryReadDelay=1` (default 0); raises `GetMaxBypassSlots()` by 1 |
| `MEM2REG` | `--mem-to-reg=100000` | 183 | `_memToRegThreshold=100000.0f` (default 0.0) |
| `MEM2ARRAY` | `--force-mem-to-array=64` | 184 | `_forceMemToArray=64` (default 0) |
| `STALL2` | `--debug --stall=2` | 185 | `_debug=1` (disables `PathLengthReportEnabled()`, enables debug symbols) and `_stall=2` |

Fully expanded, e.g. suite `STALL2` on `stall.k` is:

```
kanagawa --Wall --Werror --check-narrowing-assignment --parse-docs \
         --max-block-ram=10000000 --frequency=250 --reset-cycles=5 --register-ratio=3 \
         --backend=sv --import-dir=<root>/library --define=verbose#true \
         --Wno-initializer-overrides --Wno-shadow --define=ECC_SUPPORT#true \
         --using=COMPILE_PARAM_1#uint8 --define=COMPILE_PARAM_2#9 --define=tag#0 \
         --import-dir=<root>/test/logic --debug --stall=2 \
         --output <build>/test/logic/logic.STALL2.stall/ stall.k
```

CI runs each suite as its own job (`.github/workflows/logic-test.yml:18-77`,
targets `logic_tests_<SUITE>` / `run_logic_tests_<SUITE>`).

### 8.3 Interface tests

`test/interface/CMakeLists.txt:83-93` → BASE only, except:

| Test | Extra flags | Line |
|---|---|---|
| `external_class` | `--import-dir=${CMAKE_CURRENT_SOURCE_DIR}` | 131-136 |
| `external_class_template` | `--import-dir=${CMAKE_CURRENT_SOURCE_DIR}` | 138-143 |

All other interface tests (`simple_exported_class`, `exported_class_method_template`,
`times_five_contained_class`, `another_exported_class`, `protocol_attributes_class`,
`is_last_exported_class`, `export_class_no_instances`, `nested_export_class`,
`export_class_test_cases_1..3`, `ecc_mem`) pass **BASE** unchanged
(`test/interface/CMakeLists.txt:117-189`).

### 8.4 Library tests

`test/library/CMakeLists.txt:79-88` → BASE only, except:

| Test | Extra flags | Line |
|---|---|---|
| `processor_risc_v_1_hart` | `--define=mask#0x1FFF` | 200-204 |
| `processor_risc_v_2_hart` | `--define=mask#0x2FFF` | 206-210 |
| `processor_risc_v_mmio_1` | `--define=HARTS#1` | 221-229 |
| `processor_risc_v_mmio_2` | `--define=HARTS#2` | 231-239 |
| `processor_risc_v_mmio_4` | `--define=HARTS#4` | 241-250 |

### 8.5 RISC-V simulation executables

`add_riscv_simulation_exe` (`test/library/processor/risc-v/CMakeLists.txt:19-33`) →
BASE + :
```
--register-ratio=3               # duplicate of BASE's --register-ratio=3
--reset-cycles=2                 # OVERRIDES BASE's --reset-cycles=5
--define=HARTS#<1|2|4|8>
--rmw-memory-write-delay=0
--rmw-hardened-write-delay=0
--define=ENABLE_M#true | --define=ENABLE_M#false
```
Eight variants: `riscv_sim.{1,2,4,8}hart` and `riscv_sim.{1,2,4,8}hart_m`
(`test/library/processor/risc-v/CMakeLists.txt:176-211`).

**This is the load-bearing in-tree proof that repeated scalar flags are last-wins**
(`--reset-cycles=5` then `--reset-cycles=2`), and that repeated list flags accumulate
(`--import-dir` appears twice in every logic test).

### 8.6 Syntax tests

`test/syntax/CMakeLists.txt:40-45` composes an option **string** that
`syntax-test.py` splits on whitespace (`syntax-test.py:105`), then appends more:

```
--jobs=1
--Wno-redundant-import
--base-library=${CMAKE_SOURCE_DIR}/library/mini-base.k
--import-dir=${CMAKE_SOURCE_DIR}/library
<per-test OPTIONS>
--place-iterations=1
--import-dir <helperdir>              # space-separated form
syntax_test_temp_file.k
```
(`test/syntax/CMakeLists.txt:40-49,52,60`; `syntax-test.py:103-106`), run with
`cwd=<test-name>/` subfolder (`syntax-test.py:110`).

Distinct per-test option sets (`test/syntax/CMakeLists.txt:76-147`):

| Per-test OPTIONS | Tests | Line(s) |
|---|---|---|
| *(none)* | the remaining **59** of the 72 `add_syntax_test` calls | 77-147 passim |
| `--no-implicit-base=false --template-iterations=1` | `base-test` | 76 |
| `--Wconversion` | `return-type-polymorphism`, `conversion-warning` | 80, 99 |
| `--Wconversion --define=ELEVEN#11` | `literal-operand` | 82 |
| `--no-implicit-base=false --base-library helper/custom_base.k` | `custom-base` | 91 |
| `--Wredundant-import --backend=sv --stall=3 --target-device=ultrascaleplus-xcu250` | `options` | 93 |
| `--parse-docs` | `doc-comments` | 96 |
| `--Wall --Werror` | `all-warnings-as-errors` | 97 |
| `--target-device=mock` | `memory` | 98 |
| `--Wno-shadow` | `no-warning` | 100 |
| `--Wconversion --Werror` | `callbacks` | 101 |
| `--Wredundant-import --import-dir=${CMAKE_CURRENT_SOURCE_DIR}/imports` | `imports` | 123 |
| `--define=FOO_CONST#10` | `constexpr` | 129 |

Notable: `--no-implicit-base=false` (bool flag with an explicit `false` value,
lines 76 and 91) and `--base-library helper/custom_base.k` /
`--import-dir <dir>` (space-separated values, line 91 and `syntax-test.py:106`).

### 8.7 CLI tests (`test/compiler/cli`)

`add_cli_test` (`test/compiler/cli/CMakeLists.txt:66-72`):
```
$<TARGET_FILE:kanagawa::exe> ${OPTIONS} --output=${outdir}/${OUTPUT_PREFIX} ${SOURCES}
```
Note `--output=` with `=` here, and `OUTPUT_PREFIX` defaults to `test`
(`test/compiler/cli/CMakeLists.txt:37-39`), so `--output=<outdir>/test`.

Single test (`test/compiler/cli/CMakeLists.txt:96-106`), `skip_circt_lowering`:
```
--backend=sv
--skip-circt-lowering
--base-library=${CMAKE_SOURCE_DIR}/library/mini-base.k
--import-dir=${CMAKE_SOURCE_DIR}/library
--place-iterations=1
--output=<outdir>/test
<srcdir>/skip_circt_lowering.k
```
verified afterwards by `check_skip_circt_lowering.py`.

At the pin, three more (`test/compiler/cli/CMakeLists.txt:108-144`, upstream #136), outside
`add_cli_test` because `list-deps` writes no RTL:

* `cli.list_deps.run` — `kanagawa list-deps --base-library=<root>/library/mini-base.k
  --import-dir=<root>/library --file-list=<bin>/list_deps.txt <src>/list_deps.k`, and
  `cli.list_deps.golden`, which compares that file with `list_deps.golden.in`
  (`@KANAGAWA_SOURCE_DIR@` substituted; `add_golden_test`,
  `build/cmake/test_helper.cmake:502-529`) — the nine canonical paths, sorted;
* `cli.list_deps.compile_file_list` — `kanagawa compile` with the same base library, import
  dir and source, `--file-list=<bin>/compile_file_list.txt` and
  `--output=<bin>/list_deps_compile`;
* `cli.list_deps.compare_file_lists` — the two lists byte-identical.

### 8.8 CIRCT MLIR interface tests

`test/interface/circt/CMakeLists.txt:62-72`, test `esi_wrapper_ports` (at the pin also
`named_type_aliases`, `:74-84`, upstream #121, with the same flags):
```
--backend=sv
--skip-circt-lowering
--base-library=${CMAKE_SOURCE_DIR}/library/base.k
--import-dir=${CMAKE_SOURCE_DIR}/library
--place-iterations=1
--output=<outdir>/test
<srcdir>/esi_wrapper_ports.k
```

### 8.9 `chkdoc`

`tools/chkdoc/chkdoc.cpp:41-49,102` builds a **shell string** (`system()`):
```
<kanagawa> [--base=<root>/library/mini-base.k ] --define=MemoryLength=32 --target-device=mock \
           --import-dir=<root>/tools/chkdoc/lib --import-dir=<root>/library temp.k<N> [>> nul]
```
Argument sources: `argv[3]` and `argv[4]` are the two import dirs, `argv[4]` is also
`stdlibPath` (`chkdoc.cpp:254-260`); the CMake wiring is
`chkdoc <kanagawa> <chkdoc>/contexts <chkdoc>/lib <root>/library emptyfile.k emptyfunction.k <doc.md>`
(`tools/chkdoc/CMakeLists.txt:25-32`). Each markdown code block is compiled twice
(with and without `--base=…`) across all context files, first pass with output
redirected (`chkdoc.cpp:206-220`). Success is `returnCode == 0` (`chkdoc.cpp:110-112`).

This is the only in-tree user of `--define=<name>=<val>` (`=` separator) and of the
abbreviated `--base=`.

### 8.10 Complete set of distinct flags exercised by the whole test suite

`--Wall`, `--Werror`, `--Wconversion`, `--Wredundant-import`,
`--Wno-initializer-overrides`, `--Wno-redundant-import`, `--Wno-shadow`,
`--backend=sv`, `--base-library` (`=` and space forms) / `--base=`,
`--check-narrowing-assignment`, `--debug`, `--define` (`#` and `=` forms),
`--force-mem-to-array`, `--frequency`, `--import-dir` (`=` and space forms), `--jobs`,
`--max-block-ram`, `--mem-to-reg`, `--no-implicit-base=false`, `-O0`, `-O1`, `--output`
(`=` and space forms), `--parse-docs`, `--place-iterations`, `--register-ratio`,
`--reset-cycles`, `--rmw-hardened-write-delay`, `--rmw-memory-read-delay`,
`--rmw-memory-write-delay`, `--skip-circt-lowering`, `--stall`, `--target-device`,
`--template-iterations`, `--using`; at the pin also `--file-list`, and the `list-deps`
mode (§8.7).

= **33 distinct flags out of 103** (`-O0`/`-O1` are two values of one flag) at `8e0a3fb`,
**34** at the pin. The other **70 (69) flags have no test coverage at all** in this tree; a
Rust rewrite gets no behavioural oracle for them beyond the source. Breakdown: 6 of the 13
warning switches and 27 (28) of the 90 other flags are exercised; 7 warning switches and
63 (62) other flags are not.

---

## 9. Proposed `kg` multi-tool CLI (clap-shaped)

> **As built (2026-10-11).** `crates/kg-cli/src/modern/mod.rs` is the
> authority; this section is the design it started from. The differences:
>
> * The commands are `check`, `build`, `fmt`, `lsp`, `doc` (alias `docs`),
>   `dump` (`tokens|cst|ast|ir|instances` — the debugging window §9.3's
>   `--emit ast`/`typed-ast` rows proposed) and `explain` (takes `1`, `E27`,
>   `Warning 12` or a warning name; `--list`).
> * There is **no `kg compat`** (§9.4). The compat surface is the separate
>   `kanagawa` binary (`crates/kg-cli/src/legacy/`), all three modes and all
>   128 flag spellings; `kg` installed or symlinked under a name starting
>   with `kanagawa` dispatches to it on `argv[0]` (`dispatch_on_argv0`).
> * Legacy spellings are **not** hidden aliases on `kg build`/`kg check`:
>   `kg` takes only its own (`-I/--import-dir`, `-D/--define`,
>   `-U/--using`, `--base-library`, `--no-implicit-base`,
>   `--target-device`, `--parse-docs`, `-W <switch>`).
> * `-j/--jobs` is accepted and ignored: the frontend is single-threaded.
> * `--error-format` is `human`, `short` or `json`; exit codes are 0, 1, 2
>   (usage, or a program `build` cannot lower) and 3 (`build`: the backend
>   crashed in its worker process).
> * `kg build` takes `--backend sv|none` (and `trace`, the call sequence
>   `xtask ptdiff` grades), `-o`, `-O`, `--debug`; `--emit` and `-Z` are
>   parsed but read by nothing yet, and no other backend knob is reachable
>   from `kg` — the compat binary has them all.
> * `kg fmt` overrides `[fmt]` with `--line-width`, `--indent`,
>   `--use-tabs`, `--newline`, `--sort-imports`; `--check`, `--stdout`.

### 9.1 Shape

```
kg [GLOBAL] <COMMAND> [ARGS]

GLOBAL (clap: global = true, propagate to every subcommand)
  -C, --manifest-path <FILE>   kanagawa.toml to use (default: nearest ancestor)
      --no-manifest            ignore kanagawa.toml entirely
      --profile <NAME>         named profile from the manifest (default: "default")
  -j, --jobs <N>               rayon thread count; N<0 => num_cpus + N; 0 => num_cpus
  -v, --verbose...             -v = info, -vv = debug, -vvv = trace
  -q, --quiet
      --color <auto|always|never>
      --error-format <human|short|json>
      --version / -V
      --help / -h

COMMANDS
  kg build   [OPTIONS] [FILES]...   full compile to RTL           (legacy: default mode + --backend=sv)
  kg check   [OPTIONS] [FILES]...   parse/resolve/typecheck only  (legacy: default mode, no --backend)
  kg fmt     [OPTIONS] [FILES]...   format in place / to stdout   (legacy: `kanagawa pretty`)
  kg lsp     [OPTIONS]              language server over stdio    (no legacy equivalent)
  kg doc     [OPTIONS] [FILES]...   API docs                      (legacy: separate `sandcastle` binary)
  kg explain <CODE>                 explain E0027 / W0010         (no legacy equivalent)
  kg compat  -- <LEGACY ARGV>...    byte-compatible legacy CLI    (see §9.4)
```

Every `kg build`/`kg check` flag below is *also* accepted under its legacy spelling as a
hidden clap alias, so existing CMake glue keeps working; the canonical modern spelling is
what `--help` shows.

### 9.2 Shared source-resolution options (`build`, `check`, `fmt`, `doc`, `lsp` via manifest)

| `kg` flag | Legacy flag | Manifest key |
|---|---|---|
| `-I, --import-dir <DIR>` (repeatable) | `--import-dir` | `project.import-dirs` |
| `-D, --define <NAME=VAL>` (repeatable; `#` also accepted) | `--define` / `-d` | `project.defines` |
| `-U, --using <NAME=TYPE>` (repeatable; `#` also accepted) | `--using` / `-u` | `project.using` |
| `--base-library <FILE>` | `--base-library` | `project.base-library` |
| `--no-implicit-base` | `--no-implicit-base` | `project.implicit-base = false` |
| `--target-device <NAME>` | `--target-device` | `project.target-device` |
| `--parse-docs` | `--parse-docs` | `project.parse-docs` |
| `-W <name>` / `-W no-<name>` / `-W all` / `-W error` | `--W<name>` / `--Wno-<name>` / `--Wall` / `--Werror` | `[warnings]` table |

### 9.3 Complete legacy→`kg` mapping (all 103 flags)

Legend for the **Status** column:
`kg` = first-class modern flag · `alias` = accepted only as a legacy alias ·
`cfg` = expressible only in `kanagawa.toml` · **`none`** = no modern equivalent
(available only through `kg compat`).

#### Source resolution / frontend (11 flags + the positional file list)

| Legacy | `kg` | Status |
|---|---|---|
| positional `FILES…` | positional `FILES…` (also glob-expanded, also `--manifest` driven) | kg |
| `--import-dir` | `kg build -I` | kg |
| `--define` / `-d` | `kg build -D` | kg |
| `--using` / `-u` | `kg build -U` | kg |
| `--base-library` | `kg build --base-library` | kg |
| `--no-implicit-base` | `kg build --no-implicit-base` | kg |
| `--target-device` | `kg build --target-device` | kg |
| `--parse-docs` | `kg build --parse-docs` (implied by `kg doc`) | kg |
| `--jobs` | global `-j/--jobs` | kg |
| `--file-list` | `kg build --emit dep-list=<FILE>` | kg |
| `--output` | `kg build -o/--output <PREFIX>` | kg |
| `--backend` / `--target` | `kg build` implies `sv`; `kg check` implies none; `--emit` selects outputs | kg |

#### Warnings (14 flags: the 13 `warnings_state` switches + `--Werror`)

| Legacy | `kg` | Status |
|---|---|---|
| `--Wall` | `-W all` | kg |
| `--Werror` | `-W error` (and `--deny warnings`) | kg |
| `--Wconversion` … `--Wtransaction-size` (6) | `-W conversion` … `-W transaction-size` | kg |
| `--Wno-conversion` … `--Wno-transaction-size` (6) | `-W no-conversion` … `-W no-transaction-size` | kg |

#### Diagnostics / developer (13)

| Legacy | `kg` | Status |
|---|---|---|
| `--dump-parse` | `kg check --emit ast` | kg |
| `--dump-types` | `kg check --emit typed-ast` | kg |
| `--dump-program` | `kg check --emit desugared-ast` | kg |
| `--dump-source` | `kg check --emit desugared-source` | kg |
| `--dump-IR` | `kg build --emit ir` | kg |
| `--dump-opt` | `kg build -Z dump-opt` | kg |
| `--write-ir-post-opt` | `kg build --emit ir-json` | kg |
| `--write-circt-ir` | `kg build --emit mlir` | kg |
| `--skip-circt-lowering` | `kg build --emit mlir --no-lower` | kg |
| `--check-narrowing-assignment` | `-W error=conversion` + `kg build -Z assert-narrowing` | kg |
| `--frontend-passes` | `kg check -Z frontend-passes=N` | alias |
| `--template-passes` | `kg check -Z template-passes=N` | alias |
| `--template-iterations` | `kg check -Z template-iterations=N` | alias |

#### Backend / codegen tuning (54) — all `cfg` (manifest `[hw]` table) + `-Z hw.<key>=<v>`

`--additional-latency`, `--auto-pipeline-ratio`, `--auto-pipeline-cross-region`,
`--carry-chain-width`, `--carry-select`, `--rmw-memory-write-delay`,
`--rmw-memory-read-delay`, `--rmw-hardened-write-delay`, `--rmw-hardened-max-width`,
`--rmw-hardened-max-height`, `--rmw-memory-favor-area`, `--strict-memory-read-write`,
`--io-register-delay`, `--mem-to-reg`, `--fifo-to-dsp`, `--fifo-write-delay`,
`--fifo-merge-distance`, `--frequency`, `--max-block-ram`, `--block-ram-util-threshold`,
`--max-deep-ram`, `--deep-ram-util-threshold`, `--max-mux-sources`, `--max-mul-src-width`,
`--max-threads-default`, `--max-threads-limit`, `--schedule-limit`, `--register-ratio`,
`--max-register-ratio`, `--no-reset-fanout`, `--reset-cycles`, `--reset-fan-out-cycles`,
`--reset-duration-cycles`, `--return-stages`, `--stallable-pipelines`, `--clock-gating`,
`--clock-gating-threshold`, `--conditional-memory-reads`, `--semaphore-delay`,
`--sparse-reg-opt`, `--identifier-length`, `--debug`, `--dump-memory-metadata`,
`--strip-debug-symbols`, `--max-stack-depth`, `--no-fifo-stutter`, `--no-debug-view`,
`--no-verilog-header`, `--code-coverage`, `--code-coverage-mux-threshold`, `--optimize`,
`--work-list-size`, `--resource-usage`, `--dgml`.

Modeling note: `--optimize`/`-O` stays a first-class `kg build -O <0|1|2>`; `--debug`
becomes `kg build --debug`; `--resource-usage` and `--dgml` become
`--emit resource-usage=<FILE>` / `--emit dgml=<FILE>`. The rest live under
`[hw]` in the manifest with `-Z hw.<snake_key>=<value>` as the escape hatch, because
they are EDA-tuning knobs that belong in version-controlled project config, not in ad-hoc
command lines.

#### Legacy flags with **no modern equivalent** (11)

| Legacy | Why | Only reachable via |
|---|---|---|
| `--place-display` | opens an animated placement GUI (`options.h:41` `_display`); no headless meaning | `kg compat` |
| `--place-iterations` | placement is an internal heuristic; also silently overridden to 1 when `-O0/-O1` (`config.cpp:189-192`) | `kg compat` / `-Z` |
| `--place-pull`, `--place-push`, `--place-seed`, `--place-update-rate`, `--place-wall` | same: internal force-directed placement constants (`options.h:29-38`) | `kg compat` / `-Z` |
| `--stall` | deliberately injects random pipeline stalls for testing (`options.h:234`) | `kg compat` / `-Z` |
| `--force-mem-to-array` | help text literally says "For testing purposes only" (`CmdArgs.hs:79`) | `kg compat` / `-Z` |
| `--dgml-detailed` | dumps per-basic-block DGML into a directory; a debugging artefact of the C++ middle-end | `kg compat` |
| `--no-asserts` | toggles `_releaseAssert`, i.e. assertions **inside the compiler**, not in generated RTL (`options.h:85`) | `kg compat` |

Also with no modern equivalent, because they name things that do not exist in a Rust
rewrite: the three never-written `CodeGenOptions` booleans `_detectRaces`,
`_controlInspection`, `_inspection` (§2.2) — they have no flag today either.

**Coverage check:** 11 (source) + 13 (diagnostics) + 54 (backend) + 11 (none) + 1
(`--Werror`) = **90** one-flag fields, plus the **13** `warnings_state` switches = **103**.
Every `compile`-mode flag is accounted for exactly once.

### 9.4 `kg compat`

> **Not built** (see the box at §9's head): the `kanagawa` binary is the
> compat front end, and `kg` reaches it only by `argv[0]`.

`kg compat -- <legacy argv>` must be **argv-for-argv and byte-for-byte** equivalent to
`kanagawa <legacy argv>`, including:

* `getProgName`-style `argv[0]` basename in the generated `// <cmdline>` header (§6);
  `kg compat` therefore records `basename(argv[0])` of the **`kg`** process, so a
  drop-in replacement should be installed/symlinked as `kanagawa` and dispatch on
  `argv[0] == "kanagawa"` to the compat front-end automatically.
* Space-joined, unquoted argv reconstruction.
* stdout-only diagnostics, `Error 1:` prefix for frontend errors, `Error <N> at: <file>
  (<line>): ` for backend errors, exit codes 0/1 only (§7).
* Last-wins scalars, appending lists, `--flag=false` for booleans, unambiguous long-flag
  prefix matching (`--base` ⇒ `--base-library`).

---

## 10. Proposed `kanagawa.toml`

> **As built (2026-10-11).** `crates/kg-config/src/lib.rs` is the authority.
> Every table rejects a key it does not know (`deny_unknown_fields`), except
> `[warnings]`, whose kind names are a flattened map (an unknown name there is
> ignored), and `[hw]`, an open table. What it accepts, with defaults:
>
> | Table | Keys (default) |
> |---|---|
> | `[package]` | `name`, `version`, `edition` — all optional; `name` titles `kg doc`'s site, `edition` is reserved |
> | `[project]` | `sources` (`[]`), `exclude` (`[]`, read by nothing yet), `import-dirs` (`[]`, order significant), `target-device` (`"mock"`), `base-library` (unset: `base.k`, found on the import path), `implicit-base` (`true`), `parse-docs` (`false`), `[project.defines]`, `[project.using]` |
> | `[warnings]` | `all` (`false`), `error` (`false`), and one key per kind — `conversion`, `deprecated`, `initializer-overrides`, `redundant-import`, `shadow`, `transaction-size` — as `"allow"`, `"warn"` or `"deny"` (`deny` enables the warning; only `error = true` makes warnings errors) |
> | `[fmt]` | `line-width` (`100`), `indent` (`4`), `use-tabs` (`false`), `newline` (`"lf"`), `final-newline` (`true`), `group-imports` (`true`), `sort-imports` (`false`) — `ribbon`, `layout` and `tab-width` were not built |
> | `[lsp]` | `diagnostics` (`"on-type"` or `"on-save"`), `max-file-size-kb` (`4096`), `index-import-dirs` (`true`) |
> | `[hw]` | any key; accepted and stored, read by nothing yet |
> | `[profile.<name>]` | `project`, `warnings`, `fmt`, `hw` — deep-merged over the base tables when `--profile` or `KG_PROFILE` names it; an unknown profile is an error |
>
> Precedence is §10.3's, with `KG_PROFILE`, `KG_IMPORT_DIRS` (`:`-separated),
> `KG_TARGET_DEVICE`, `KG_BASE_LIBRARY` and `KG_COLOR` as the environment
> layer. Globs in `sources` are not expanded yet. `kg init --from-cmake`
> (§10.4) was not built.
>
> **The bundled library.** `kg` and the language server search one directory
> the manifest does not name: a release archive's `share/kanagawa/library`,
> beside the executable's `bin/`, appended last to the import directories
> when none of them provides `base.k` (`Settings::or_bundled_library`,
> `crates/kg-config/src/resolve.rs`). A file outside any project therefore
> checks against the library the toolchain shipped with, and a project that
> vendors its own keeps it. A source build has no such directory, so nothing
> is added there.

### 10.1 Design constraints

An LSP server needs, per workspace and *without* running a build:
(1) the import search path, (2) the target device (it changes the search path — §4.2),
(3) `--define`/`--using` (they synthesise the `.cmdargs` module that every file
implicitly imports — §4.4), (4) the base library and whether it is implicit,
(5) the enabled warning set (to decide which diagnostics to publish), and
(6) formatter settings. All six are below.

### 10.2 Schema

```toml
# kanagawa.toml — workspace root marker for `kg` and the language server.
# Discovery: nearest ancestor directory of the open file containing this file.

[package]
name    = "my-design"        # optional, informational
version = "0.1.0"            # optional
edition = "2026"             # reserved: language-version selector

[project]
# --- source set -------------------------------------------------------------
sources       = ["src/top.k"]              # entry points; globs allowed
exclude       = ["src/scratch/**"]

# --- import resolution (legacy --import-dir, repeatable, ORDER SIGNIFICANT) --
import-dirs   = ["library", "third_party/kanagawa-lib"]

# --- legacy --target-device; ALSO prefixes each import dir with
#     "<dir>/device/<target-device>" ahead of every plain dir (§4.2) ----------
target-device = "mock"

# --- legacy --base-library / --no-implicit-base -----------------------------
base-library  = "library/base.k"
implicit-base = true          # false == --no-implicit-base

# --- legacy --parse-docs ----------------------------------------------------
parse-docs    = false

# --- legacy --define; the synthetic `.cmdargs` module (§4.4).
#     Values are raw Kanagawa expression text, emitted as `const auto K = V;` --
[project.defines]
verbose       = "true"
COMPILE_PARAM_2 = "9"
mask          = "0x1FFF"

# --- legacy --using; emitted as `using K = V;` ------------------------------
[project.using]
COMPILE_PARAM_1 = "uint8"

# --- legacy --W.../--Wno-.../--Wall/--Werror --------------------------------
# Keys are the 6 wire names from Warning.hs:29-35. Values: "allow" | "warn" | "deny".
# `all = "warn"` reproduces --Wall (turns on conversion + transaction-size).
# `error = true` reproduces --Werror.
[warnings]
all                   = false
error                 = false
conversion            = "allow"    # default off
deprecated            = "warn"     # default on
initializer-overrides = "warn"     # default on
redundant-import      = "warn"     # default on
shadow                = "warn"     # default on
transaction-size      = "allow"    # default off

# --- formatter (legacy `kanagawa pretty`) -----------------------------------
[fmt]
line-width    = 80        # --line-width / -w
indent        = 4         # --indent
ribbon        = 1.0       # --ribbon
layout        = "pretty"  # "pretty" | "smart"  (--layout)
# modern additions with no legacy equivalent:
newline       = "lf"      # "lf" | "crlf"
final-newline = true
tab-width     = 4
use-tabs      = false
group-imports = true
sort-imports  = false     # off by default: import order is semantically load-bearing

# --- LSP-only knobs ---------------------------------------------------------
[lsp]
diagnostics       = "on-type"   # "on-type" | "on-save"
max-file-size-kb  = 4096
index-import-dirs = true        # index library sources for goto-def / completion

# --- backend / EDA tuning: the 54 legacy codegen flags (§9.3) ---------------
# Keys are the legacy long-flag name with dashes -> underscores.
[hw]
optimize                 = 2
frequency                = 250
register-ratio           = 3
reset-cycles             = 5
max-block-ram            = 10000000
rmw-memory-read-delay    = 0
# ... any of the 54

# --- named profiles: deep-merge over the tables above -----------------------
[profile.release]
hw = { optimize = 2, clock-gating = 0 }

[profile.test]
warnings = { all = true, error = true, initializer-overrides = "allow", shadow = "allow" }
hw       = { optimize = 0 }
```

### 10.3 Precedence

`built-in defaults` < `kanagawa.toml [project]/[warnings]/[fmt]/[hw]` <
`[profile.<name>]` (selected by `--profile`) < environment (`KG_*`) <
command-line flags. Within a single source, repeated scalars are last-wins and repeated
lists append, matching the legacy binary (§8.5).

### 10.4 Importing settings from an existing CMake project

Command: `kg init --from-cmake <BUILD_DIR|SOURCE_DIR>`.

Three ingestion routes, in order of fidelity:

1. **`compile_commands.json`-equivalent: `*.kanagawa.args`.** `add_kanagawa` already
   writes a normalised, generator-expression-resolved summary of every invocation to
   `${OUTPUT_DIR}/${target}.kanagawa.args` (`build/cmake/test_helper.cmake:55-65,68,80`),
   in the form
   ```
   OUTPUT_DIR=<dir>
   WORKING_DIRECTORY=<dir>
   SOURCES=
     <src>…
   OPTIONS=
     <opt>…
   ```
   This is the highest-fidelity source: glob `**/*.kanagawa.args` under the build tree,
   parse each into (working-dir, sources, options), then run the legacy→manifest mapping
   of §9.3. One manifest per distinct OPTIONS set; identical option sets across many
   targets collapse into one `[project]` + several `sources` entries.

2. **CMake File API / `--trace-expand`.** For projects that do not use `add_kanagawa`,
   run `cmake --trace-expand -S <src> -B <tmp>` and capture every
   `add_custom_command`/`add_test` whose COMMAND resolves to the kanagawa executable;
   extract the argv after generator-expression evaluation.

3. **Static scrape of `CMakeLists.txt`.** Lowest fidelity: regex `OPTIONS`/`add_kanagawa`
   blocks, leave `${...}` unresolved, and emit them as TODO comments in the manifest.

Mapping rules for route 1/2 argv → manifest:

| argv pattern | manifest |
|---|---|
| `--import-dir[=| ]<D>` | append to `project.import-dirs`, made relative to the manifest dir |
| `--define[=| ]<K>[#=]<V>` | `project.defines.K = "V"` |
| `--using[=| ]<K>[#=]<V>` | `project.using.K = "V"` |
| `--base-library[=| ]<F>` | `project.base-library` |
| `--no-implicit-base[=false]` | `project.implicit-base = true/false` |
| `--target-device[=| ]<N>` | `project.target-device` |
| `--parse-docs` | `project.parse-docs = true` |
| `--Wall` / `--Werror` | `warnings.all = true` / `warnings.error = true` |
| `--W<n>` / `--Wno-<n>` | `warnings.<n> = "warn"` / `"allow"` |
| `-O<n>` / `--optimize=<n>` | `hw.optimize` |
| `--backend=sv` | drop (implied by `kg build`) |
| `--output[=| ]<P>` | drop (a build-system concern, not a project concern) |
| any other `--x-y=<v>` | `hw.x-y = <v>` (typed by the §1.1 table) |
| non-flag argument | append to `project.sources` |

Conflicting values for the same key across targets (e.g. `--reset-cycles=5` in the base
set and `=2` for the RISC-V sims, §8.5) become **separate `[profile.*]` tables**, named
after the CMake target, with the majority value staying in the base table.

---

## 11. Open questions

Everything below is behaviour of the external `cmdargs` library (`>= 0.10.10`,
`compiler/kanagawa.cabal:44`), which is **not vendored** in this tree, or behaviour that
depends on GHC's `base`. Each item names a probe that settles it against a real
`kanagawa` binary. Until then, treat them as **UNVERIFIED**.

**Q1 — answered 2026-09-23** from the reference's own help
(`kanagawa compile --help`, `kanagawa pretty --help`, the reference's own build,
now `target/reference`, `scripts/build-reference.sh`). `cmdargs` assigned exactly two letters in `compile`,
`-j --jobs` and `-o --output`, beside the declared `-f -d -u -O` and the
built-ins `-? -V -v -q`. In `pretty` it assigned `-b --base-library`,
`-i --indent`, `-l --layout`, `-n --no-implicit-base`, `-p --parse-docs`,
`-r --ribbon` and `-t --target-device`, beside the declared `-w`, `-d` and
`-u`. The assignment is per mode: `-b`, `-n`, `-p` and `-t` name flags that
exist in `compile` too, with no letter there. `legacy::flags` records each
letter with the modes it holds in (`FlagSpec::short_modes`), and
`tests/legacy_flags.rs` checks it is refused elsewhere. The question as
first posed follows.

**Q1 — Auto-assigned short flags.** `cmdargs`' `explicit` annotation is documented as
"don't guess any names", which implies names *are* guessed, including short flags derived
from the first free letter of the long name. There are exactly 25 `&= explicit`
annotations in `compiler/hs/app/Options/CmdArgs.hs` (12 single-value fields at lines
21, 22, 23, 24, 51, 62, 63, 64, 104, 105, 106, 110 plus all 13 warning alternatives at
121-135), so **78 of the 103 `compile`-mode flags are not `explicit`** and may have
silently acquired a one-letter alias; the assignment
depends on declaration order (e.g. `-d` is explicitly claimed by `--define` at
`CmdArgs.hs:63`, before `--dgml` at `:69` and `--debug` at `:76`).

*Probe:* `kanagawa --help=all` and record the full flag table verbatim; additionally
`kanagawa -c foo.k`, `kanagawa -s foo.k`, `kanagawa -m foo.k` to see which single letters
bind.

**Q2 — Built-in flag set and spelling.** Does the binary accept `-?`, `-h`, `--help=all`,
`--help=bash`, `--version`, `-V`, `--numeric-version`, `--verbose`, `-v`, `--quiet`, `-q`?
Which exit code and which stream for each?

*Probe:* `for f in -? -h --help --help=all --help=bash --version -V --numeric-version -v -q; do kanagawa $f; echo "rc=$?"; done` with stdout and stderr separated.

**Q3 — Derived long name for `dump_IR`.** Does `cmdargs` lowercase derived names, or only
map `_`→`-`? I.e. is the flag `--dump-IR` or `--dump-ir` (or both)? No in-tree caller uses
it, and no doc mentions it.

*Probe:* `kanagawa --dump-ir --help` / `kanagawa --dump-IR --help`, or grep the `--help=all` output for `dump`.

**Q4 — Defaults for `PrettyPrint`'s 9 unassigned record fields.** `pretty` assigns only 4 of
13 fields (§1.3). What values does `cmdargs` synthesise for `base_library`, `define`,
`files`, `import_dir`, `no_implicit_base`, `parse_docs`, `target_device`, `using`,
`dump_parse`? Is `base_library` `""` (so the base import fails) or `"base.k"`? Do the
corresponding flags actually appear in `kanagawa pretty --help`?

*Probe:* `kanagawa pretty --help`; then `kanagawa pretty --import-dir=library foo.k` vs
`kanagawa pretty foo.k` on a file that imports from the base library.

**Q5 — `enum` on a list-typed field: append or replace?** §3.2 argues from the test suite
that `--Wall`/`--W…`/`--Wno-…` must **append** to `warnings_state`. Confirm directly.

*Probe:* on a file that triggers a shadow warning, run
`kanagawa --Wall --Wno-shadow f.k` (expect: conversion warnings still active, shadow
silent) and `kanagawa --Wno-shadow --Wall f.k` (expect: identical, since `--Wall` does not
contain `Shadow`), plus `kanagawa --Wno-conversion --Wall f.k` (expect: conversion
warnings **on** if append+left-fold, off if `--Wall` were a set-union of only its own
entries applied first).

**Q6 — Argument-parse failure: stream and exit code.** Does an unknown flag print to
stderr with exit 1, or to stdout, and is the message prefixed with the program name?

*Probe:* `kanagawa --no-such-flag foo.k 1>out 2>err; echo $?; cat out; cat err`.

**Q7 — Long-flag prefix matching rules.** `--base` resolves to `--base-library`
(`tools/chkdoc/chkdoc.cpp:208`). Is any unambiguous prefix accepted? What happens for an
ambiguous prefix such as `--max` (matches 8 flags) or `--dump` (matches 6)?

*Probe:* `kanagawa --max=1 foo.k`, `kanagawa --dump foo.k`, `kanagawa --imp=lib foo.k`.

**Q8 — Boolean flag value syntax.** `--no-implicit-base=false` is proved in-tree
(`test/syntax/CMakeLists.txt:76`). Which other spellings work — `=true`, `=yes`, `=on`,
`=1`, `=0`, and a **space-separated** `--no-implicit-base false` (which would then be
mis-read as a positional source file)?

*Probe:* `kanagawa --parse-docs=yes foo.k`, `kanagawa --parse-docs 1 foo.k`.

**Q9 — Enum value matching for `--backend` and `--layout`.** Is `--backend=sv` matched
case-insensitively against the constructor `Sv`? Are `--backend=SV`, `--backend=s`,
`--backend=none`, `--backend=NONE` accepted? Which is the primary long name for the
backend flag, `--backend` or `--target` (it affects `--help` output only)?

*Probe:* `kanagawa --backend=SV foo.k`, `kanagawa --backend=s foo.k`,
`kanagawa --target=sv foo.k`, and read the `--help=all` line for the flag.

**Q10 — `opt` (optional-value) flags.** Does bare `--frontend-passes` (no `=N`) really
mean `maxBound`, and does `--frontend-passes 3` (space form) bind `3` as the value or
treat `3` as a positional file?

*Probe:* `kanagawa --frontend-passes foo.k` (if `foo.k` is consumed as the value, the run
fails with "Missing source filename(s)"), and `kanagawa --frontend-passes=0 foo.k` (expect:
no frontend passes run, so a file with an obvious type error compiles or crashes).

**Q11 — `CMDARGS_*` environment variables.** `cmdargs`' `processArgs` is documented to
consult `CMDARGS_COMPLETE`, `CMDARGS_HELPER` and `CMDARGS_HELPER_<PROG>`. If set, the
binary's behaviour changes (shell completion / external helper). A compat binary probably
must ignore them, but the divergence should be a conscious decision.

*Probe:* `CMDARGS_COMPLETE=0 kanagawa foo.k`, `CMDARGS_HELPER=cat kanagawa foo.k`.

**Q12 — `@file` argument-file expansion.** `cmdargs`' `processArgs` calls `expandArgsAt`,
which expands `@<file>` arguments into the argv. If so, `kanagawa @args.txt` reads flags
from a file — and note this happens *after* `getArgs`, so `_cmdArgs` (§6) would record the
**unexpanded** `@args.txt`, not the expanded flags.

*Probe:* `printf -- '--backend=sv\nfoo.k\n' > a.txt; kanagawa @a.txt` and inspect the
`//` header line of the generated `.sv`.

**Q13 — `getProgName` on Windows.** Does it strip the `.exe` suffix? The `//` header of
generated RTL differs between `kanagawa.exe …` and `kanagawa …`.

*Probe:* on Windows, compile anything and read line 2 of the generated `.sv`.

**Q14 — Non-ASCII argv.** `withCAString` (`Options/FFI.hsc:30`) is a narrowing Latin-1
conversion. Confirm what lands in the `//` header for a UTF-8 path argument.

*Probe:* `kanagawa --import-dir=./libré foo.k` and read the generated header.

**Q15 — `--jobs=0`.** `Main.hs:63-67` maps `Just n | n > 0 -> n | otherwise -> numProc + n`,
so `--jobs=0` yields `numProc` and `--jobs=-numProc` yields `0`, which
`setNumCapabilities 0` rejects at runtime.

*Probe:* on an N-core machine, `kanagawa --jobs=-N foo.k`.
