# The differential oracle

The reference frontend, built as a library, printing its own lowered tree.

```
./build.sh
./dist-newstyle/.../kg-oracle -I <libdir> --base <libdir>/base.k FILE
./dist-newstyle/.../kg-oracle --no-base FILE          # a self-contained snippet
```

## Why it exists

`xtask ir`'s round-trip gate cannot catch a deterministic lowering bug: the same
bug sits on both sides of `lower(parse(pretty(lower(parse(f)))))` and cancels.
Reversing the ternary arms — inverting every `a ? b : c` in the corpus — leaves
that gate at 403/403. This is the only instrument whose two sides do not share
the port's lowering.

It prints what `--dump-source` prints (`prettySource`, `Main.hs:90-91`), so a
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
`errorBundlePretty` (`Main.hs:82`). Combines with `--passes N`.

This reaches something the conformance gate structurally cannot. `xtask
conformance` compares exit codes and the first `Error N` and **never message
text** (`docs/spec/conformance.md` §1.7) — because the corpus only asserts that
much. So two compilers can agree on every assertion while disagreeing about what
they actually say, and until now there was no way to see it.

First run, first finding:

```
$ kg-oracle --no-base --errors err.k        # inline void main() { uint8 a = 1; a.nosuch(); }
err.k:4:7: Symbol name not in scope: `nosuch`      <- at the member name

$ kg check err.k
error: Invalid member access on type                <- at the expression
```

Same count, same code, different message and different column. Not yet
investigated — recorded here because it is the shape of thing this mode exists
to surface, and because "the scoreboard is green" was never evidence about
message text.

## The other modes

`--tree` dumps the tree as the reference holds it, notes elided —
`SeqF [SeqF [], FunctionF {declScopeName = [], ..., body = SeqF [...]}]`. This is
the one to compare against `kg dump ir`, because it is *structural*.

Without `--tree` it prints source, via the same `prettySource` the reference's
`--dump-source` uses. Source is lossy about structure: a switch prints
`case 0: { } break;;` whether or not the parser kept the `break`, so use it for
reading and `--tree` for grading.

## What it establishes

Ground truth for the tree-shaping `kg_hir::lower` reproduces. Verified:

| construct | reference produces | citation |
|---|---|---|
| `true ? a : b` | `MuxF cond (SeqF [else, then])` — **arms reversed** | `Parser.hs:968` |
| `x += 2` | `x = (x + 2)`, lvalue re-read as rvalue | `Parser.hs:631-638` |
| `x++` | `x = (x + 1)` | `Parser.hs:607-614` |
| a program | leads with a bare `SeqF []` — the redundant-import warning seed | `Parser.hs:234-241` |

That last row is a known divergence: `kg-hir` does not reproduce the seed,
because it exists only to carry a parser warning and slice 1 diagnoses nothing.
The oracle is how we know it is there.

## Limits

Corpus files under `test/syntax/` are snippet accumulators containing
deliberately invalid fragments; the oracle reports `PARSE-ERROR` for them, which
is correct. Whole programs — `library/`, `test/logic/` — parse.

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
file whenever the oracle binary is built; `differential.py` remains the
by-hand instrument for a single file's diff.

**116 of 124 library files agree node-for-node.** The other eight:

* **6 diverge only at interpolated strings** — the reference splits an
  interpolation into parsed child expressions; `kg-hir` keeps one token
  (divergence #15), and from the first interpolation the two pre-order
  streams cannot realign. The gate books these, keyed to the
  `InterpolatedString` at the divergence point, not to a file list.
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
of anything. The two divergences booked above are keyed to their mechanism —
an `InterpolatedString` at the divergence point, an empty oracle output — not
to file names.
