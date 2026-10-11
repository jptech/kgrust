# The differential oracle

The reference frontend, built as a library, printing its own lowered tree.

**Status (2026-10-11): current, and a gate dependency.** `kg-oracle` is the
reference's own frontend (`third_party/kanagawa/compiler/hs/lib`, copied in
at build time) behind a small driver, `driver/Main.hs`. Three gates run it
and fail, rather than skip, when it is not built: `errors` (`--errors`),
`ir` (`--tree` and `--notes`, both `--roots-only`) and `loop-left`
(`--loop`). The by-hand instruments ask it too: `cargo xtask fuzz
--differential`, `cargo xtask reduce` and `cargo xtask passdiff --oracle`
(all `--errors`), and `scripts/probe.sh` through its `errors` comparison.
`scripts/vs-haskell.sh` builds a separate, optimised copy
(`target/oracle-bench`) to time the reference. Every gate's run is cached in
`target/oracle-cache` (`xtask/src/oracle_cache.rs`), keyed by the oracle
binary's bytes, every `.k` file under the reference's `library/` and
`test/`, the arguments, the working directory and the file's text;
`KG_ORACLE_CACHE=0` bypasses it. Citations into the reference's executable
(`Main.hs`, `Options/CmdArgs.hs`) are at the pin, `98a2d8c`; its frontend
library, which every other citation here names, has not changed since
`8e0a3fb`.

```
./build.sh                                            # about two minutes; never writes to the reference
./dist-newstyle/.../kg-oracle -I <libdir> --base <libdir>/base.k FILE
./dist-newstyle/.../kg-oracle --no-base FILE          # a self-contained snippet
```

`build.sh` copies the reference's `compiler/hs/lib` into `lib/` (gitignored)
and builds with cabal from `~/.ghcup/bin`; `KANAGAWA_ROOT` points it at
another reference checkout. In a worktree, `scripts/worktree.sh link` links
the main checkout's `dist-newstyle/`, so the oracle is built once.

## Every flag

The driver parses its own command line; it is not the reference's `cmdargs`
surface. A value is the next argument (`--passes 8`, never `--passes=8`),
and **any argument it does not recognise is taken as a file**, so a misspelt
flag reads as a missing file rather than as an unknown flag.

**What it prints** — one mode; when several are given, the first of
`--loop`, `--errors`, `--typed`, `--notes`, `--tree` wins:

| flag | prints |
|---|---|
| *(none)* | each tree as source, by `prettySource 4` — what the reference's `--dump-source` prints (`Main.hs:94-95`) |
| `--tree` | each tree as the reference holds it, `show`n with notes elided (below) |
| `--notes` | `--tree` with each node's note — its source span — appended as ` @<note>`; what provenance and donor checks are read off |
| `--typed` | `--notes` plus each node's type by the reference's own `showPretty`, between the bytes `0x1F` and `0x1E`; needs a pass count, and without one prints as `--notes` |
| `--errors` | the frontend's errors over the whole program, deduplicated, each rendered by `errorBundlePretty`; nothing when there are none (below) |
| `--loop` | `left SCOPE\|QUALIFIER\|NAME` for every template instance the tree after pass 8 still holds outside every template, then `rounds R`, the smallest `--template-iterations R` that reaches the fixpoint (`>64` past the search); ignores the pass counts and `--roots-only` — `cargo xtask loop-left`'s input |

A file the reference cannot parse prints `PARSE-ERROR` on `stderr`, then
each parse error's bundle, in every mode.

**How far the frontend runs.** Without any of the three counts below, the
printing modes show the tree **as parsed**, before any frontend pass;
`--errors` runs every pass. Any count runs the frontend:

| flag | effect |
|---|---|
| `--passes N` | the first `N` frontend passes in execution order (below); omitted, all of them |
| `--template-passes N` | with `--template-iterations 0`, the first `N` inner sub-passes of `instantiateTemplates` (below) |
| `--template-iterations N` | `instantiateTemplates`' round budget |
| `--roots-only` | print only the trees of the files named on the command line; the whole closure is still parsed and run, because `frontend` needs it |

**Source resolution** — the `ParseOptions` fields the reference's
`getParseOptions` fills from its own command line (`Main.hs:146-162`), each
spelled as the reference's flag it stands for:

| flag | reference flag | effect |
|---|---|---|
| `-I DIR` | `--import-dir` | an import directory; repeatable, in order |
| `--base FILE` | `--base-library` | the implicitly imported base |
| `--no-base` | `--no-implicit-base` | no implicit base import |
| `--parse-docs` | `--parse-docs` | doc comments are parsed (`Parser/IO.hs:94`) |
| `--define NAME#VALUE` | `--define` | a constant in the synthetic `.cmdargs` module (`Parser/IO.hs:49-55`); `=` also separates |
| `--using NAME#VALUE` | `--using` | a type alias in the same module |
| `--target-device T` | `--target-device` | prefixes each import directory with `<dir>/device/T` (`Parser/IO.hs:110`) |
| `--backend sv\|none`, `--stall N`, `--optimize N`, `--max-threads-default N`, `--max-threads-limit N` | the same | the five fields of the synthetic `.options` module (`Parser/IO.hs:57-65`) a program reads as `options::stall` and the like |

**Warnings**, which only `--errors` reads:

| flag | effect |
|---|---|
| `--Werror` | when the program has no error, print every enabled warning and then, if there was one, `Error 1: Warnings treated as errors` — as the reference prints them before failing (`Main.hs:96-99`), so a must-reject snippet under `--Werror` is graded by its warnings |
| `--Wall`, `--W<kind>`, `--Wno-<kind>` | appended in order to the four enabled by default, as the reference builds `warnings_state` (`Options/CmdArgs.hs:111-136`); `<kind>` is `conversion`, `deprecated`, `initializer-overrides`, `redundant-import`, `shadow` or `transaction-size`; without `--Werror` they are accepted and change nothing |

## Why it exists

`xtask ir`'s round-trip gate cannot catch a deterministic lowering bug: the same
bug sits on both sides of `lower(parse(pretty(lower(parse(f)))))` and cancels.
Reversing the ternary arms — inverting every `a ? b : c` in the corpus — leaves
that gate at 403/403. This is the only instrument whose two sides do not share
the port's lowering.

It prints what `--dump-source` prints (`prettySource`, `Main.hs:94-95`), so a
disagreement is a real disagreement with the reference rather than with our
reading of it.

## `--passes N`: the tree after N frontend passes

`frontend` takes a pass count (`Frontend.hs:34-38`). `desugarPasses` folds with
`foldr ($)` over `drop (length passes - frontendPasses) passes`, so the *last*
entry runs *first* and `N` counts in **execution order** — `1` is
`redundantNameScope` and friends, `8` is through `instantiateTemplates`.

**Diffing `--passes 7` against `--passes 8` isolates one pass.** That is the
instrument for porting pass 8's rewriting sub-passes: rather than reading the
Haskell and inferring what a rewrite emits, look at it.

```
$ kg-oracle --no-base --passes 8 lam.k
inline uint8 twice<|$uint@8|>(uint8 x) { return (x + x); }   # instance materialized
template <typename T> inline T twice(T x) { ... }            # template kept
inline auto main@?lambda0(main::$::?lambda0$capture ?lambda0$capture_param,
                          uint8 v) { ... }                   # lambda lifted to a sibling
inline void main() {
    using ?lambda0$capture = decltype(make_capture0());       # synthesized alias
    const auto f = cast<Closure<decltype(main@?lambda0), main@?lambda0,
                                main::$::?lambda0$capture>>(make_capture0());
    uint8 r = twice<|$uint@8|>(3);                            # call rewritten to the instance
}
```

Every declaration in that output that was never written is one of the five
rewriting sub-passes at work, with its exact mangled spelling.

**Which pass did which is not obvious, and an earlier version of this file got
it wrong.** It credited `desugarLambdas` with lifting the lambda to a sibling.
It does not: it prepends the capture alias and a `LambdaFunction` into the
*enclosing block's own* `Seq`, still named `?lambda0` and still inside `main`.
The hoist to module scope and the `main@?lambda0` rename are `moveFunctions`.
You can see the handoff:

```
$ kg-oracle --no-base --passes 8 --template-passes 2 --template-iterations 0 lam.k
    inline auto ?lambda0(main::$::?lambda0$capture ?lambda0$capture_param, ...)

$ kg-oracle --no-base --passes 8 --template-passes 4 --template-iterations 0 lam.k
inline auto main@?lambda0(main::$::?lambda0$capture ?lambda0$capture_param, ...)
```

## `--template-passes N --template-iterations 0`: one inner sub-pass

`instantiateTemplates` honours `templatePasses` only when `templateIterations ==
0`, and then runs the last N entries of its own inner list (`Template.hs:62-64`),
which under `foldr ($)` execute first. So N counts inner sub-passes in execution
order and the N→N+1 diff is one sub-pass alone. That is how the attribution
above was settled.

## A bug this tool had, and what it invalidates

Until 2026-08-12 the driver used `evalStateT`, which keeps `parseProgram`'s
*return value* — the command-line roots. The reference uses `execStateT`
(`app/Main.hs:69`) and hands `frontend` the accumulated cache: every file parsed,
imports included. So the oracle was running the frontend over a program with no
imports, `undefinedSymbols` reported every library symbol missing, and no
cross-module symbol question had a valid answer.

Fixed. But **any whole-program conclusion drawn from this tool before that is
suspect**, and at least one was: a `Symbol name not in scope: make_capture0` that
looked like a gap in the closure library was an artefact of the missing imports.
Single-file `--no-base` results are unaffected.

## `--errors`: the reference's own diagnostics

Runs the frontend and prints what the reference reports, rendered by its own
`errorBundlePretty` (`Main.hs:85-87`) — without the `Error 1: ` prefix the
reference's `exitErrors` adds (`Parser/IO.hs:113-114`). The reference appends
the program into one tree before collecting, so an error in an imported module
is reported once for the program; the driver collects per tree and
deduplicates, which gives the same set. Combines with `--passes N`.

This reaches something the conformance gate structurally cannot. `xtask
conformance` compares exit codes and the first `Error N` and **never message
text** (`docs/spec/conformance.md` §1.7) — because the corpus only asserts that
much. So two compilers can agree on every assertion while disagreeing about what
they actually say.

First run, first finding (2026-08-12):

```
$ kg-oracle --no-base --errors err.k        # inline void main() { uint8 a = 1; a.nosuch(); }
err.k:4:7: Symbol name not in scope: `nosuch`      <- at the member name

$ kg check err.k
error: Invalid member access on type                <- at the expression
```

Same count, same code, different message and different column. **Since
settled, and the class is now a gate.** On 2026-09-06 `cargo xtask errors`
began grading our first error's position and message against `kg-oracle
--errors` over every must-reject snippet (`xtask/src/errors.rs`), and this
was among the first verdicts it settled: a call through a resolved receiver with no such
member is the member's name not in scope (`Frontend.hs:106-108`), as the
corpus's `test/syntax/basics.k` "calling a method on a non-class" snippet
asks — pinned by
`a_call_through_a_receiver_with_no_such_member_names_the_member`
(`crates/kg-db/src/tests/undefined_symbols.rs`; `docs/journal.md`, "The gate
the reference runs its passes behind, and ten small verdicts"). The gate's
count may fall and must not rise; its current value is in
`docs/status.md`.

## The other modes

`--tree` dumps the tree as the reference holds it, notes elided —
`SeqF [SeqF [], FunctionF {declScopeName = [], ..., body = SeqF [...]}]`. This is
the one to compare against `kg dump ir`, because it is *structural*.

Without `--tree` it prints source, via the same `prettySource` the reference's
`--dump-source` uses. Source is lossy about structure: a switch prints
`case 0: { } break;;` whether or not the parser kept the `break`, so use it for
reading and `--tree` for grading.

`--notes` keeps the note `--tree` elides: each node's source span, which is
what a rewrite's donor assertions are read off (`--notes --roots-only
--passes 8 --template-passes N --template-iterations 0`; `CLAUDE.md`, "A
rewrite's `Src` donors need a unit test read off `--notes`"). `--roots-only`
matters here: `--notes` on one library file prints about 11 MB, because every
file of its closure is printed.

## What it establishes

Ground truth for the tree-shaping `kg_hir::lower` reproduces. Verified:

| construct | reference produces | citation |
|---|---|---|
| `true ? a : b` | `MuxF cond (SeqF [else, then])` — **arms reversed** | `Parser.hs:968` |
| `x += 2` | `x = (x + 2)`, lvalue re-read as rvalue | `Parser.hs:631-638` |
| `x++` | `x = (x + 1)` | `Parser.hs:607-614` |
| a program | leads with a bare `SeqF []` — the redundant-import warning seed | `Parser.hs:234-241` |

That last row is a known divergence: `kg-hir` does not reproduce the seed. It
exists only to carry the parser's redundant-import warnings, and this port
reports those from the module graph instead (`kg_db::imports`), so the seed
would carry nothing; the `ir` gate drops it from the reference's side before
comparing (`drop_warning_seed_noted`, `xtask/src/ir.rs`). The oracle is how we
know it is there.

## Limits

Corpus files under `test/syntax/` are snippet accumulators containing
deliberately invalid fragments; the oracle reports `PARSE-ERROR` for them, which
is correct — the `errors` gate writes each snippet to a file of its own first.
Whole programs — `library/`, `test/logic/` — parse.

It is the reference's *frontend*, not its backend: a check the reference makes
only in code generation is invisible here, and an error from it reads as the
oracle accepting the program (`xtask/src/differential.rs`).

Requires GHC and cabal on `~/.ghcup/bin`. Nothing here is committed to the
reference tree, and `build.sh` never writes to it.

## `differential.py` — and the `xtask ir` gate

Compares the pre-order sequence of constructor names from `kg dump ir` against
`kg-oracle --tree`. Sequence, not fields: enough to catch a missing node, an
extra node, or reordered children — the whole symmetric-bug class — without
either side imitating the other's formatting.

Since pass 8 slice 3, paths **sort** (`kg_hir::lower_sorted` over
`kg_db::sorted::sorted_program`), and the comparison runs raw — the
spine-collapsing normalizer this section used to describe is gone. The same
comparison is wired into `cargo xtask ir` as a gate over every `library/`
file; `differential.py` remains the by-hand instrument for a single file's
diff.

**122 of 124 library files agree node-for-node** (2026-09-22, `docs/journal.md`,
"A string's holes are expressions"). When the gate landed on 2026-08-12 it was
116: six more diverged at interpolated strings, which the reference splits
into parsed child expressions and `kg-hir` then kept as one token. Divergence
#15 closed that on 2026-09-22, and the gate's booking for it went with it — a
divergence at an interpolated string now fails the gate. The other two:

* **2 are rejected by the oracle itself** (`PARSE-ERROR`):
  `device/agilex-7-f-023` and `device/stratix-10-gx-2800`'s `hardware/dsp.k`.
  Under `-I library` and the default `mock` target device, `base` re-exposes
  `device/mock/hardware/dsp.k`, and a device override whose text *differs*
  from mock's makes its own locals ambiguous (`findInScopes` concatenates
  import hits over scope hits, `SymbolTable.hs:257`). `registerSymbolError`
  rides megaparsec's deferred-error channel, so the file fails to parse.
  The other five device copies survive for a reason worth knowing:
  `findSymbol`'s `nub` compares symbol **trees** with notes ignored
  (`Parser/Syntax.hs:594-599`), so a declaration textually identical to the
  mock copy collapses with it and resolves. `kg-hir` reproduces that with a
  structural fingerprint (`kg_hir::sort::fingerprint`); `stratix-10-nx-2100`
  — identical in `fmul32`, different in others — agrees node-for-node only
  because the collapse is per-symbol.

Deliberately not tuned further. Each special case makes the two sides agree
more *by construction*, and a comparison tuned until it passes is not evidence
of anything. The one booking left is keyed to its mechanism — an empty oracle
output — not to file names.
