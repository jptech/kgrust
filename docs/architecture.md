# kgrust architecture

A Rust compiler toolchain for the Kanagawa HLS language. One query-based core,
many front doors.

## Why this shape

The reference toolchain (`third_party/kanagawa`, a submodule) is a pipeline with no reusable middle:
Haskell owns the driver, lexer, parser, desugaring and type inference; C++ owns
everything after a stateful 85-function C ABI; and nothing between them can be
called by a tool. That is why the language has no language server, no real
formatter, and no incremental anything.

`kgrust` inverts it. The compiler is a set of libraries; the batch CLI is just
one caller.

```
                    ┌──────────── kg-cli ────────────┐
                    │  kg build / check / fmt / lsp  │   ┌── kanagawa (compat binary)
                    │  kg doc / dump / explain       │   │   ~92 legacy flags
                    └────────────────┬───────────────┘   └────────┬───────────
                                     │                            │
      kg-lsp ────────────────────────┤                            │
      kg-doc ────────────────────────┤                            │
      kg-fmt ──────┐                 ▼                            ▼
                   │   ┌──────────── kg-db (queries) ────────────────────┐
                   │   │ parse · resolve · the parser's checks · pass 1  │
                   │   │ · the loop, read by the checks and the editor   │
                   │   └──┬─────────┬──────────┬──────────┬─────────┬────┘
                   ▼      │         │          │          │         │
               kg-syntax  kg-resolve  kg-ty/kg-const  kg-hir ──▶ kg-sem
               kg-parser                          (IR, rewrites)  (inference, the
                                                                   program-wide loop,
                                                                   checks over copies)
                                                                        │
                        kg-ptbridge ◀────────────────────────────────────┘
                        the walk into the ParseTree C ABI, replayed in a
                        worker process into the reference's C++ backend
                        (kg-backend-sys; kg-ptrace/kg-pttrace record and
                        compare the reference's own calls)
```

## Crates

| Crate | State | Responsibility |
|---|---|---|
| `kg-base` | done | `FileId`, `TextRange`, `LineIndex` (byte ⇄ UTF-16), the string interner, names and mangling |
| `kg-diag` | done | The diagnostic model and the legacy terminal renderer the test harness greps |
| `kg-vfs` | done | Virtual file system with in-memory overlays for unsaved buffers |
| `kg-config` | done | `kanagawa.toml`: import dirs, target device, defines, warnings, fmt |
| `kg-parser` | done | Lexer + error-recovering grammar → a flat event stream; no tree dependency |
| `kg-syntax` | done | rowan CST + the typed AST generated from `grammar/kanagawa.ungram`; the documentation each doc-carrying element carries (`file_docs`), as the reference's generator reads it, and its Pandoc Markdown as CommonMark (`to_commonmark`) |
| `kg-resolve` | done | Module graph, import resolution, scopes, name binding |
| `kg-const` | done | Constant values and the fold's tables — `evalBinary` over two literals, a cast's target (`kg_hir::constexpr` holds the rules once, D1.3); the CST evaluator went with the CST engine (sprint W3.7) |
| `kg-ty` | done | The type model (width algebra, subtyping, conversions) and `TypeKey`, a node of the text by position, kind and expansion — what every consumer of a `Ty` links |
| `kg-db` | done | The query layer: parse, resolve, the per-module run at a module's text — the parser's own checks and pass 1 on the CST, every later pass the loop's verdicts admitted in its place (`kg_sem::check`, since 2026-10-10) — and `looped`, the program-wide loop's result as the backend walk, the checks and the editor read it. One engine since sprint W3.7 (M6): the CST inference engine, `kg-cst-infer`, and the second census are deleted. Under the editor's budget each root set keeps its last loop's snapshot — the final program, shared with its answer, and its history per module and round — until the next run reaches its own (`kg_db::snapshot`, sprint W7.5d step 1), and the next run takes from it every prefix pass it can (step 3) |
| `kg-hir` | done | The IR: an append-only arena with `Src` provenance, the sorted lowering, the rewriting combinators (`desugar`), and pass 8's rewrites (`*_in_place`) |
| `kg-sem` | done | Inference over the IR (`infer`, incremental), `getSymbols`, the census, `deduce`, the closure passes, `elaborate` — the program-wide instantiation loop — `check`, the passes after instantiation over the copies and at each module's text, `instance`, how a copy is spelled in a message, `snapshot`, the loop's state per module and round as a run records it and the shadow that checks a kept run against the next, `reads`, the read door: what each module's round read across its boundary, keyed beside the copies decided into it, and `reuse`, a kept run's pass taken in by the next run where every key before it agreed (the prefix, D1 step 3) |
| `kg-ptbridge` | done | The walk of the elaborated program into the ParseTree C ABI through a `Backend` trait (trace, recording, real; generated, checked in), the worker process that replays it, and the gates' verdict cache |
| `kg-backend-sys` | done | Runtime-loaded bindings to `libkanagawa-backend` (generated, checked in), refused at load unless the library defines what the headers declare; the options marshalling |
| `kg-abi-gen` | done; not in a plain `cargo build` | bindgen over the reference's C headers, once: the generators `xtask codegen` writes the two crates above with, and the configuration `kg-ptrace`'s build script uses |
| `kg-ptrace` | done; not in a plain `cargo build` | The interposer: a shim backend logging every ParseTree call the reference makes; its build script needs the reference and libclang |
| `kg-pttrace` | done | The call-trace format both sides of `ptdiff` read and write |
| `kg-cli` | done | `kg` (check, build, fmt, lsp, doc, dump, explain) and the argv-compatible `kanagawa` |
| `kg-doc` | done | `kg doc`: a static site from the doc comments — a page per module in the reference generator's structure, linked signatures, folded examples, a search over every declaration; Markdown by `pulldown-cmark` |
| `kg-lsp` | done; latency open | Language server on `lsp-server`: diagnostics, quick fixes, goto-definition and type definition, hover (with the declaration's documentation), references, rename, completion (documentation on resolve), signature help, inlay hints, symbols (document and workspace), folding, selection ranges, highlight, semantic tokens. Queries never block the message loop. Diagnostics land twice where the program-wide loop runs past four rounds — its fourth round's, then its fixpoint's, `kg check`'s — from one run on a thread of its own, which an edit cancels; every other feature reads the newest of the two ready (`PassBudget::Layered`, `kg_db::layered`, sprint W7.6) |
| `kg-fmt` | done | Comment-preserving formatter over the CST, with a verifier that refuses any edit that changes the program |
| `kg-alloc` | done | The binaries' allocator (mimalloc), declared by `kg`, `kanagawa`, `kg-lsp` and `xtask`; `purge_eagerly`, which the language server calls once so an idle editor returns what its last check freed. No library names an allocator |
| `xtask` | — | The gates (below), codegen, bench, fuzz, reduce |

The back end is not a rewrite: `kg build` drives the reference's C++ middle
end through its ParseTree C ABI — the 85 `extern "C"` functions of
`compiler/cpp/parse_tree.h`, specified in `docs/spec/abi.md` — exactly as the
Haskell front end does (decided 2026-09-06, [`roadmap.md`](roadmap.md)).

**What building needs** (sprint W8.4). The tools — `kg`, `kanagawa`,
`kg-lsp` and every crate they are made of, which are the workspace's
`default-members` — build from this checkout alone, with stable Rust and a
C compiler for mimalloc (the binaries' allocator since 2026-10-10:
macOS's held a warm editor database's freed memory, ~90 MB a keystroke;
`performance.md` §10): the ABI's bindings
and the `Backend` trait are generated sources checked in
(`kg-backend-sys/src/bindings.rs`, `kg-ptbridge/src/backend.rs`), so no
build script reads the reference's headers and nothing links libclang.
`cargo xtask codegen` writes them from the pinned reference's headers
through `kg-abi-gen`, and `--check`, a stage of `scripts/gates.sh`,
regenerates and compares; each names the reference commit and a digest of
the headers it came from. What still needs the reference beside this
checkout: `kg-ptrace` (its build script runs bindgen over the headers; it
records the reference's own runs, so it is only ever wanted where the
reference is), therefore `--workspace` builds and the gates; `codegen`;
every corpus test and gate (the corpus is the reference's `library/` and
`test/`); the oracle (`oracle/build.sh` builds the reference's frontend);
and the C++ backend itself, which `kg build` loads at run time from
`KG_BACKEND_DIR` or beside the executable, refusing a library that lacks
a function the headers declare (sprint W8.5) — the library carries no
version to compare, so a changed signature under the same name is held
only at generation.

**The backend, built here** (2026-10-11). `scripts/build-backend.sh`
builds `libkanagawa-backend` from the submodule with a CMake project of
its own (`backend/CMakeLists.txt`): upstream's `thirdparty/` settings for
LLVM, MLIR and CIRCT, upstream's `compiler/cpp/CMakeLists.txt` unchanged,
the one target `kanagawa_lib` — never the Haskell frontend, whose ghc and
cabal upstream's configure requires. The `release` channel by default, as
upstream ships: no consistency checks, no "Running internal consistency
checks." line. About half an hour from scratch on 16 cores, a 6 GB tree, a
34 MB library depending on the system's C++ runtime alone.
`--reference` then builds upstream's own `kanagawa` against that library
(`scripts/build-reference.sh`: cabal over a copy of `compiler/`, and
upstream's CMake configured — not built — for its test definitions), which
is what the parity gates record and run (`KANAGAWA_BUILD`,
`target/reference`). `cargo xtask dist` packages a release: `kg`,
`kanagawa` and the library beside them in `bin/`, the standard library in
`share/kanagawa/library` — which `kg` and `kg lsp` search last when no
import directory provides `base.k` (`Settings::or_bundled_library`) — and
the licenses, `THIRD-PARTY-NOTICES.md` generated from the submodule and
`cargo metadata`.

## The three load-bearing decisions

### 1. The parser has no symbol table

The reference calls `findSymbol` *during* parsing to decide whether `Foo` is a
type and whether `Foo<X>` is a template instantiation
(`Parser.hs:668-680, 712-726`). That coupling is what prevents error recovery,
parallel parsing, and a language server.

It is avoidable here — unlike in C++ — because `<` and `>` are declared
`no_assoc` (`Parser.hs:915-916`), so `a < b > c` is *not a valid expression at
all*, and because `<`, `>`, `<<`, `>>` are removed from the operator table used
inside template arguments (`Parser.hs:913`). Therefore an angle region that
closes at depth zero without crossing a statement boundary is *always* a
template argument list, decidable by a bounded token scan.

`crates/kg-parser/src/grammar/lookahead.rs` is that scan, plus the equivalent
predicate for `T x;` versus `expr;`. Everything the symbol table used to decide
becomes a name-resolution diagnostic later — which is still `Error 1`, so the
conformance suite cannot tell the difference.

### 2. `<` and `>` are always single tokens

The reference's `_angle` combinator matches a single `<` not followed by `=`
(`Parser/Lexer.hs:272-273`), while its binary `<` additionally forbids a
following `<` (`Parser/Lexer.hs:262`). So `Foo<uint<8>>` closes two argument
lists with no `>>` splitting, and `Foo<X>=y` does *not* parse.

The lexer therefore never merges an angle bracket with anything. `<=`, `>=`,
`<<` and `>>` are built by the *parser* from glued token pairs
(`Parser::at_composite`), reproducing the reference's `notFollowedBy` rule
exactly from the adjacency bit the lexer records.

### 3. The scoreboard precedes the work it scores

`test/syntax/` carries **2,843 `expected:<code>` and 79 `warning:<name>`
assertions** over ~2,900 micro-programs, and the expected results are embedded
in the test files themselves — so conformance is measurable without running the
reference compiler at all. Of those, 1,768 expect a clean compile and 834 expect
a frontend rejection: **91.5% of the suite is decided by the frontend alone.**

⚠️ The frontend/backend error split is itself observable. The remaining 241
assertions expect a specific C++ error code, which requires the Rust frontend to
*not* catch errors that the reference frontend lets through. A "better" frontend
fails those tests. The division of labour must be reproduced, not improved —
in `kanagawa` compat mode. `kg check` is free to diagnose earlier and better.

## Working on this

```
cargo test --workspace --profile quick   # ~35 s at rest; the dev profile is far slower on the corpus tests
scripts/probe.sh <program>               # every verdict for one program (~10 s)
scripts/fast.sh [--lint] [walk|loop]     # per change: the gates a change can move (a few minutes)
scripts/gates.sh [--snapshot] > gates.log 2>&1   # every gate: ~6 min warm (361 s, 2026-10-11), 15–30 cold;
                                         #   --snapshot grades a commit in its own worktree
scripts/mutant.sh <file> <old> <new> -- <check>  # prove a test load-bearing
scripts/worktree.sh add <path> | link    # a second working copy with the gates' stores linked
scripts/check-links.py                   # every relative Markdown link resolves
cargo xtask codegen [--kanagawa-root R]  # regenerate the AST layer from the ungrammar, and the
                                         #   ABI's bindings from the pinned reference's headers (libclang)
```

The loop, and when to use which tier, is in [`../AGENTS.md`](../AGENTS.md); every gate in [`gates.md`](gates.md).

### The gates

Each isolates one thing; `scripts/gates.sh` runs them all and the ratcheted
ones are graded per item against `baseline.json` (`cargo xtask ratchet`).

| Gate | Command | Isolates |
|---|---|---|
| **L0** round-trip, acceptance | `xtask corpus` | lexer and CST losslessness; every whole program parses with zero errors |
| **L1** conformance | `xtask conformance` (and `--strict`, `strict-ledger`) | frontend accept/reject and `Error N`, per embedded assertion |
| **L2** errors | `xtask errors` | the first error's position and message against `kg-oracle --errors` — what L1 is blind to |
| **L3** ir | `xtask ir` | the lowering's totality, the pretty → reparse → lower round trip, provenance, and the rewrites against the reference's own frontend |
| passdiff | `xtask passdiff --pass <name> [--oracle]` | a CST pass after instantiation, recorded in its place in the CST run, against the loop's verdicts at each module's text, over every corpus root and snippet — the check each pass's move off the CST lands behind (sprint W3 step 1), and each of the parser's own checks' move off the CST engine's table (sprint W3.7); `--oracle` settles a snippet's difference by `kg-oracle --errors`; not in `gates.sh` |
| infer-identity | `xtask infer-identity` (`--dump DIR`) | incremental inference against the whole walk, trajectories included, the incremental run the editor's layered and recorded one, its first answer graded against the loop bounded where it was taken; `--dump` diffs two builds — the exactness check for any optimisation |
| loop-left | `xtask loop-left` | what the program-wide loop leaves undecided and its round count, per root the reference compiles, against the reference's own (`kg-oracle --loop`) |
| edit-identity | `xtask edit-identity` | a warm `Db` after each kind of edit and its undo against a fresh one — the check every memo lands behind; the warm one is the editor's, and its row says the editor's last answer is `kg check`'s on every root and its first shows nothing the last does not; its snapshot's shadow restores each kept round whose keys agree — the copies decided into the module and what it read across its boundary — relocated, against the round the next run ran, and every prefix pass the editor reused is run on a fork as well and compared (the `shadow` ledger: `kept` steps, `unequal` modules, `reuse-unequal` passes; sprint W7.5d) |
| resilience | `xtask resilience` | hover and completion at sampled nodes of the 76 error-recovered corpus files and 64 fuzzed broken buffers, as the editor answers them from the loop: every answer recorded per node, so a change in what the editor says is named; why it says nothing, counted. Until sprint W3.7 it graded the loop's answers against the CST engine's — the measure that engine's deletion waited on |
| lint | `xtask lint` | retiring spellings in production code (mangled text, a diagnostic's message as control flow, environment verdict switches), per occurrence |
| ptdiff | `xtask ptdiff` (`ptrace` records the reference) | our ParseTree calls against the reference's, call by call |
| backend | `xtask backend` | the compat binary through the real C++ backend, graded as `syntax-test.py` grades |
| output | `xtask output` | every file `--backend=sv` writes, ours against the reference's, modulo source locations |
| repoint | `xtask repoint` | upstream's own `syntax-test.py` against the compat binary |
| cli | `xtask cli` | upstream's own CLI tests (`test/compiler/cli`, as the reference build's `CTestTestfile.cmake` runs them) against the compat binary, and every mode's `--help` and usage against the reference binary's, the version masked |
| fmt-check | `xtask fmt-check` | the formatter is idempotent and layout-only over the corpus |
| bench | `xtask bench` | a tripwire, never failed: stage timings and the worst keystroke, to the editor's first answer (`keystroke`) and to its last (`fixpoint`) |

`xtask corpus` excludes `test/syntax/` from the acceptance check: those files are
snippet accumulators full of deliberately invalid fragments, not translation
units. They are covered by `xtask conformance`, which splits them at their
`expected:` markers exactly the way `syntax-test.py` does.

### A finding worth recording

`test/syntax/basics.k` — at 406 assertions the **largest** file in the corpus —
has no `add_syntax_test` registration and is therefore never run by ctest. The
live upstream oracle is 2,516 assertions, not 2,922. Those 406 assertions are
perfectly good and simply orphaned, so `xtask conformance` runs them anyway and
labels them; that is 16% more signal than upstream gets.

The corpus lives in the `third_party/kanagawa` submodule and is
**read-only**, pinned by `reference.lock` (commit and content hash; every
xtask command checks it) and by the submodule's own commit, the same one.
Tests locate it via `KANAGAWA_ROOT` or the submodule
(`kg_base::reference_root`) and skip themselves when it is absent — except under the gates, which set
`KG_REQUIRE_CORPUS=1` and fail such a test instead (`kg_base::corpus_absent`).

`docs/spec/` holds the extracted behavioural specification of the reference
compiler — lexer, grammar, types, resolution, frontend passes, CLI, diagnostics,
conformance. Every claim there carries a `file:line` citation. When a question
about semantics comes up, the answer is in there or is a probe program away; it
is not a matter of opinion.
