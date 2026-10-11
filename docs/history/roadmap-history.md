# Roadmap history, 2026-08 to 2026-09-26

*Moved out of [`../roadmap.md`](../roadmap.md) on 2026-09-27, verbatim and in
their original order, when the parity sprint became the queue. Code comments
and journal entries that cite "the roadmap" §1–§3, a queue item, or the
backend bridge's pickup notes mean the sections below. The decisions that
still bind stayed in the roadmap.*

## The queue, 2026-09-25 — structural upgrades, after the row work lands

Two plans, proposed 2026-09-25, to start once the current ptdiff/backend rows
are at a natural stopping point:

- [`plans/single-source-of-truth.md`](../plans/single-source-of-truth.md) —
  one owner per language construct. D1 is gate-neutral consolidation (names
  and mangling, one constant evaluator, node → type lookup, written types);
  D2 unifies copies that drift today (template index, import visibility,
  shadowing, `static if` decisions, template arguments, conversions).
- [`plans/ir-elaboration.md`](ir-elaboration.md) — pass 8 without the
  text round trip: IR inference with node-keyed, interned types, one
  program-wide instantiation loop with an `InstanceRegistry`, passes on the
  IR, and dependency-tracked, parallel rounds. Migration M0–M6 behind shadow
  gates (`ir-infer`, `copy-types`), each step gate-green, with performance
  targets (keystroke 1.46 s → ≤ 100 ms).

## The queue, 2026-09-24 (evening)

The triaged plan of 2026-09-23 (P0–P4) has been worked through; what is
left of it is architectural, and it is where the next effort pays:

1. **The closure deduction stack** (`gaps.md` §A2). The single most shared
   decline site is `control.loop`'s `pipelined_map` — `static
   pipelined<I, decltype(fn)> loop;` over a local capturing lambda — postponed
   because `Closure<decltype(__kg_m3), __kg_m3, __kg_m1>` waits on a
   `make_capture1` over another closure (7 programs; `data/array.k:829`'s
   `reduce`, `data/function.k`'s `compose2`/`curry` are the same family).
   The journal's 2026-09-20 entries "What the closure deduction is actually
   waiting for" and "The alias that stayed behind" hold the diagnosis.
   **2026-09-24:** the `const` closure's `decltype` binds and mints
   (journal, "Roadmap 1, first link") — the `pipelined` copy is made.
   The chain the item names resolves; its programs now part from the
   reference at copy order, the top `ptdiff` row — a wrapper's `Closure` a
   round late and a deduction over closures (`compose2`) two rounds early,
   which must land together (journal, "What a wrapper's closure waits
   for").
2. ~~**Copies of member function templates in the frontend**~~ — done
   2026-09-24 (journal, "Roadmap 2").
3. ~~**When a lambda's type resolves**~~ — done 2026-09-24 (journal,
   "Roadmap 3"): a copy's frame records the round that decided it; 5 of the
   12 static-numbering programs agree, the rest differ further on.
4. ~~**Deduction over the fold**~~ — done 2026-09-24 (journal, "Roadmap
   4: deduction over the fold").
5. ~~**A function default mangles bare**~~ — done 2026-09-24 (journal,
   "Roadmap 5").
6. **§6.4 steps 2–5** (inference on the IR); §8's developer layers.

## The queue, 2026-09-23

After `audit-2026-09-22.md` was acted on (journal, "The audit, acted on — 2026-09-23").
Every gate is now enforced per item against `baseline.json`
(`scripts/gates.sh`, `cargo xtask ratchet`), and the headline in
`status.md` is rendered from the last green run (`cargo xtask
status-md`); numbers are not repeated here.

1. **The open false alarms** (`gaps.md` §G). "Never a false alarm" is what
   `kg --help` promises. Rerun `cargo xtask fuzz --differential` with new
   seeds after each fix; confirm every bucket with the full reference
   compiler before it counts.
2. **Backend crashes, then isolation (D6).** 42 programs abort in the
   backend with a C++ exception; each is a walk that hands the backend a
   call sequence the reference never does. Then `kg build` gets a worker
   process.
3. **The walk's rows** — the ranked `ptdiff`/`backend` histograms below
   (missing copies, `ParseIdentifier / ParseIdentifier`, …) — and the
   `unit::test` chain, which the fixpoint budget now reaches.
4. **The silent snippets** (`errors --verbose`, 84 of 834 since every
   snippet is graded) and the 28 disagreements.
5. **§6.4 steps 2–5** (inference on the IR), once the strict curve
   flattens; §8's developer layers alongside.

Decided by the owner, 2026-09-23: output parity for `kg build` is
**identical modulo source locations**, with location agreement a second
ratchet (`audit-2026-09-22.md` §6.1; journal, "Output parity, decided and graded").
`cargo xtask output` grades it over every program the reference writes RTL
for.


## Picking up the backend bridge (item 6) — state on 2026-09-17

Read, in this order: the three journal entries dated 2026-09-17 ("The
boundary is observable", "The walk exists", "`kg build` compiles"), then
`docs/ParseTree_Integration_And_Test.md`'s progress note, then
`docs/spec/abi.md` for any call you touch.

The crates: `kg-abi-gen` (headers → bindgen), `kg-backend-sys` (the ABI,
loaded by `dlopen`; `KG_BACKEND_DIR`, `none` for no backend), `kg-pttrace`
(the trace format), `kg-ptrace` (the recorder shim), `kg-ptbridge` (the
walk: `walk.rs` is `ParseTree.hs` arm for arm, `program.rs` is `compile`).
The drivers: `xtask/src/ptrace.rs`, `ptdiff.rs`, `backend.rs`.

The loop that moved every number this far:

```
cargo xtask ptdiff                         # the histograms are the work list
cargo xtask ptdiff --dump syntax::basics::8   # both traces under target/ptrace/diff/
# read ~20 lines either side of the printed seq, find the reference clause
# (Frontend.hs / Constexpr.hs / Type.hs), port it into walk.rs with its
# citation, run ptdiff again; conformance must not move.
cargo xtask backend                        # what the backend says about it, end to end
```

Ranked, as of 2026-09-20 later (2,422 compared, 1696 structural, 1855
walked and 2576 backend agreements, 45 backend disagreements, 44 crashes,
0 rejections; the journal's last entries have what moved it from 890/1114),
rows by `(theirs, ours)` at the first divergence. **Re-measured 2026-09-22**
(1742 structural, 1907 walked, 2643 agreements, 42 disagreements, 42
crashes): the missing-copies row is 152, `ParseIdentifier /
ParseIdentifier` 121 (half of it `higher-order-functions.k`, closure
machinery), `ParseFunctionModifier / ParseAppendList` 30, `ParseFunction /
ParseFunction` 26, `ParseNamedType / ParseNamedType` 24 (scattered scope
lists, no one mechanism), `ParseUintType / ParseBaseList` 24. The rows
below are the 2026-09-20 reading:

* **a copy the reference makes and ours lacks** (172, `ParseIdentifier /
  ParseBaseList`. Grouped by the *template* whose copy is missing — the
  useful reading — it is `tuple2` 25, `Foo` 13, `optional` 10, `pipelined`
  8, `c1` 7 and a long tail. `tuple2` reduces to nine library-free lines in
  the journal, 2026-09-20 "Where the missing copies actually are": a
  higher-order function template called with a **lambda**, where deduction
  binds the capture parameter to `TUnresolved` because the argument's type
  is a `decltype` of a call to an `auto`-returning function whose return
  type is a struct declared inside its own body. That is gaps.md row 2 seen
  from the deduction end. Led also by `template-template-parameter.k::4`'s `T1<>`, led by `template-template-parameter.k::4`'s `T1<>`, whose
  argument is a template given by default — the fill is written and
  measured in the journal (2026-09-19), and blocked behind the instance
  note of a copy whose argument is a template name); the bulk is
  still **the closure machinery**, at call 8: the reference opens
  `data.closure.core` with its `Closure` copies and ours lacks one there.
  A lambda's copy, a captured `this` (`make_capture1<Foo&>`) and a member
  function's wrapper are made since 2026-09-19. What is known to be left:
  ~~a class *template*'s member given to a higher-order function~~ (done
  2026-09-19: the twin did not parse, because a method called through a
  receiver kept its class qualifier — `this-capture.k::18` and
  `callbacks.k::63` now walk the reference's copies name for name); `tuple2`/`optional` library copies;
* ~~`same_types<…>` over attributed function types~~ (done 2026-09-19;
  the rest of that row is other copies' names, `ParseIdentifier /
  ParseIdentifier`);
* copies ordered differently at a module's end, a function's copy before a
  class's, or a wrapper's `Closure` before its lambda's (`ParseBaseList /
  ParseIdentifier`, `this-capture.k::9` — `Template.hs:115` puts each copy
  beside its template; `callbacks.k::0`'s 44 were not this, see the
  journal's callback entry);
* a member of a copy typed through a foreign alias — `T::X` in `D<Foo>`'s
  copy, where `Foo` is the *decider's* class and the helper's twin cannot
  see it, so the declaration carries no node type (`type-identifier.k::8`;
  the rest of that row was the narrowing stamp, done 2026-09-19);
* a written template that is also abbreviated, `template <typename T>
  inline T Fn_T_auto(auto x)` (`ParseFunctionModifier / ParseNamespace`, 64
  programs, 38 of them whole files under `test/logic`); the copy deduced
  from a context type through a twin's alias landed 2026-09-19, and the
  folded width (same day) closed the small reproductions but none of the
  38. **Diagnosed, not fixed**: in `test/logic/auto_inline.k` every
  argument of `unit::assert_equal(tag, 4, x)` reads `Unresolved` from the
  base table — the *whole* call site is untyped, so nothing deduces and
  `assert_equal<|$uint@32|>`, its `reinterpret_cast` chain and eight other
  copies are never made. Not the alias-qualified callee, not the class
  method, not the literal argument and not the class reached only through
  a template argument: each of those types cleanly in a minimal program
  (2026-09-19). **Found**: `unit::tag_t` is `using tag_t = newtype<uint32>`
  — an alias to an instance of an *imported* template, which the base
  inference types `Unresolved`, so `visit_function` postpones every body
  that takes one (`Inference.hs:54-59`). A settling round (one extra
  inference round that visits such a body anyway) is worth 13 divergences
  and 3 backend agreements and newly rejects nine whole files on two
  *pre-existing* checks it exposes — a parameter-dependent `static_assert`
  and the arity of a member called inside a lambda. The first landed
  2026-09-19; the second is reduced to 23 lines in the journal, and the
  type it misreads comes from the route's read-back, not from the check.
  Then the settling round, then this row;
* **a re-read lvalue has no type** (41, `atomic.k::1`,
  `static-if.k::17`): `g[idx]++` lowers its lvalue a second time and
  donates the copy (`kg_hir::lower`'s `reread`), and the walk reads a type
  only from an `Own` node. Two blanket rules measured *worse* than the
  baseline (journal, 2026-09-19 "Three rows measured and not landed") —
  read the recorded traces for which occurrences the reference stamps,
  then give `Written` a third case;
* **an enum constant through a foreign alias** (`type-identifier.k::9`):
  the copy's `__kg_m0::One` names the deciding module's enum through a
  leaf alias the twin's import view answers only for its own first
  segment. A path whose head is a foreign leaf alias is the path the alias
  stands for;
* ~~default template arguments~~ (done 2026-09-19, calls and type
  mentions; `crate::defaults` evaluates a default over the arguments
  before it). What is left of that chain: `pair<uint5, uint5>`'s display
  name (an alias copy's), and program passes beyond four
  (`MAX_PROGRAM_PASSES`) once they pay for themselves — see the journal;
* ~~a `static for`'s iterations are one instance each~~ (done
  2026-09-19: an unrolled module is printed even when round one decided
  nothing, and round two's census keys them — `literal-operand.k::0`).
  What is left of that row: the iterations are still not *typed* apart, so
  the walk declines at the first fold inside one (`static-for.k::0`'s
  `uint32[i+1]`; 9 programs);
* ~~a `static if` the walk splices keeps its `$quasi0`~~ (done
  2026-09-19; the `post` map is still keyed with it);
* smaller rows with an example each and no diagnosis yet:
  `memory.k::17` (39), `dependent-template.k::3` (23),
  `memory.k::8` (20), a folded `-1` typed `int1` where the typed
  evaluator says `int2` (20, `scheduling.k::5`).

Declined: `deduce: Unresolved` (33), `fold:static
assert` over instance types (26), interpolation holes (27 — `walk.rs`,
`interpolated_string`), `fold:array dimension` (12) and `fold:attribute`
(4) — a number the backend reads as one, which the fold here missed: an
imported class member through an alias (`constexpr.k::126`; the fold is
one file wide), or a dimension the reference's frontend rejects and ours
does not (`constexpr.k::166`). `fold:int<N>` fell from 98 to 2 when sizes
began to fold inside the type fixpoint. Exact agreement is positions only
and is its own campaign (end = next token's start, doc comments excluded,
`FunctionModifier`/`ModuleDecl` unknown).

Crashes (80 in `backend`): each is a divergent walk the backend cannot
survive, and a decline is the honest verdict for any of them. When a
change moves programs from declined to walked, diff the crash list against
the tree before; the 2026-09-19 night entry is the worked example.

No corpus program the walk completes exports a class; the first `.sv`
needs `base.k`'s closure walked (`file::library/base/system.k` declines
at one template instance).

**The queue, as of 2026-09-22** (the reasons are the journal's entries of
that date):

1. **Make a program pass cheaper.** The `unit::test` chain (46 programs)
   needs a fifth pass; six passes cost the worst keystroke 1384 → 2038 ms
   for one agreement today. Re-elaborate only the modules whose inputs —
   handed specs, the overlaid maps they read — changed since the last pass,
   then raise `MAX_PROGRAM_PASSES`.
2. **The 44 silent must-reject snippets** (`xtask errors --verbose`): several
   are one parse-time rule each (`using.k::37`'s alias naming itself,
   `template-template-parameter.k::23`), others need `deduceAuto`'s failure
   verdict (`Could not deduce function return type`) or a copy's type check.
3. **The missing-copies row** (152) and the closure chain behind it.
4. **A local class's member template**: its lambda's member reference is
   re-rooted to the class before the lambda is hoisted
   (`local-classes.k::15,16`).
5. Pass 16's typed fallback for a cast through an alias
   (`static assert(5000 == cast<id_t>(5000))`, oracle evidence recorded);
   pass 18 onto the hole nodes' types.

**The queue, as of 2026-09-20.** The settling round and the stamp it
needs are *measured and put back* — `docs/patches/settling-round.patch`,
and the journal entry "The settling round, measured whole and not landed"
has the table, the three reasons and the two pre-existing defects it
found (both landed). What that leaves, in order:

1. ~~**The base inference's reading of imported names**~~ — **done**
   2026-09-20, the way this row said it had to be done: `ModuleTypes::
   overlay` lets a twin's resolved name beat the base map's unresolved
   one, a module whose only template use is an undeducible call gets a
   twin (`calls_a_known_template`), and the census folds a substituted
   width. `ptdiff` 1583 → 1590, `backend` 2506 → 2514. It left two
   costs, and they are now the work: the worst-case **keystroke 578 ms →
   1289 ms** (phase 2 re-elaborates every target module on every program
   pass; `check` did not move), and **`ir`'s `type agreement` 30/34 →
   21/38** on one cause — an alias to a copy comes home as a struct named
   after the copy where the reference holds a `TInstance`, so it needs
   `setTypeOfInstance`'s note. Both are written up in the journal.
2. ~~**A hoisted lambda inside a copy keeps its pre-hoist scope**~~ —
   **done** 2026-09-22, both halves: the patch's declaration-scope half and
   `fixScopedName` reading the references un-aliased, which was exactly
   what this row said it needed first. See the journal, "A `static if` over
   a type, decided where the reference decides it". Left behind by it: a
   *local* class's member template, whose lambda's member reference is
   re-rooted to the class before the lambda is hoisted
   (`local-classes.k::15,16`).
3. ~~**The copy-order row.**~~ **Done** 2026-09-22: a minted leaf's
   spelling is read back segment by segment (`ExpOrd::name`). The original
   note: `higher-order-functions.k::7` emits one
   module's two `Closure` copies in the opposite order. The sort is not
   the bug (dropping the round costs 23 programs, measured); what differs
   is the `Closure` copies' arguments, which are alias leaves — the
   comparison unaliases, and what it compares after that is where to look.
4. **The re-read lvalue's stamps** (41), **the `same_types` copy-name
   row** (123, whose biggest family is the closure chains), **a copy's
   member typed through a foreign alias** (30), and the nested lambda's
   own closure.
5. **The `unit::test` chain** (46 whole-file programs): since 2026-09-22
   `unit::fixture<…>()` makes its closure copy; each now declines at the
   root's `unit::test<1>(…)` call (`template instance`), the closure
   deduction of "What the closure deduction is actually waiting for".



---


## ~~1. Whole-program type environment~~ — **done**

`Db::type_table` resolves through the import graph into the converged map of
every module a file imports, in `Program::files` order, memoized per module on
`(item tree, the imports' maps)`. `conformance` 2022→**2027/2027**, `--strict`
584→**579**, batch `check` 14.1→**12.3 s** over the corpus. A *keystroke* got
96.9→**101.3 ms** slower: the memo cannot help an edit that invalidates it, and
the passes downstream now have resolved types to work on. The 91.6→70.6 ms first
reported here was a benchmark artifact — see [`journal.md`](../journal.md).

Three of the four symptoms closed; the fourth — the deduced type becoming the
call expression's type — turned out to need the *rewrite*, not the environment,
and is now wholly inside #1 below. Details and the two named narrowings are in
[`status.md`](../status.md) and in `kg_db::passes::types`.

## 1. Pass 8 — `instantiateTemplates`

~110 assertions, which it **unlocks rather than delivers** — see the correction
below — and it is the reference's own second call site for the deduction that
already landed
(`Template.hs:71-81`): the inner pass list runs `inferType`, `closureCalls`,
`deduceTemplateArgs`, `inferType`, `templateTypeCheck`. That code exists and is
waiting for a driver — and the environment it needs now exists too, which is
what the item above was for.

The second `inferType` in that list is the whole of the fourth deduction
symptom: it is what puts the deduced type back on the call node.

**It is also why four fifths of the library's `auto` bindings have no inlay hint
— but pass 8 alone will not deliver them, and that is now measured rather than
assumed.** 962 bindings, 203 hinted, 759 not; 536 of the 759 are in ten
template-heavy files. The cause is real: inference never enters a template body
(`Inference.hs:63`, reproduced by `facts::reached_only_after_elaboration`), so
even `const auto x = 3;` inside one is untyped. But a hint on the *template body*
comes from the reverse index over instance nodes, and the instance set is empty
for a file opened on its own, because every handler roots at `&[snap.file]` while
instances are decided by the files that **import** the template. Pass 8 plus a
rooting change is worth ~536 hints; pass 8 alone is worth close to zero of them.
See [`journal.md`](../journal.md).

It also unblocks the 44 remaining failures in `template-argument-deduction.k`,
including the 21 `static Class o; o.Fn_T_T(…)` member-call forms, whose
`rewrite` clauses are guarded on the receiver's class type
(`Type.hs:1390-1393`).

Own fixpoint, own sub-pipeline, sets the error flag. The largest single piece of
work remaining in the frontend.

### ~~1a. `templateTypeCheck`~~ — **done**

`conformance` 2027→**2041/2041**, `--strict` 579→**565**. One of the nine
sub-passes; `crates/kg-db/src/passes/template_type_check.rs` owns it and its
five silences. It costs `check` **+1.6 ms** and a keystroke **+1.9 ms**
(interleaved A/B, four pairs, non-overlapping) for a second walk of the file.

It unlocks **nothing else**. It is diagnostic-only, it creates no instance, and
every symptom the item above lists — the deduced type reaching the call node,
the `auto` inlay hints, the member-call deduction — is still waiting on the
eight sub-passes below.

### 1b. The other eight sub-passes

`foldr ($)` again, so the list executes bottom-up (`Template.hs:71-81`):

| order | sub-pass | kind | what it needs here |
|---|---|---|---|
| 1st | `addLambdaWrappers` | **rewrite** | wraps a bare function name passed as a call argument in a lambda (`Function.hs:366-423`). Built under `getSymbols' NoTemplate` — a *different* symbol map from the one the other eight see (`Template.hs:79-80`) — **and it reads types**: `tIsReference $ typeOf o` (`:371`), `tIsFunction (theType t)` (`:390`), and two `expFromType` calls (`:384`, `:392`). So it is **not** a syntactic rewrite and cannot land before IR inference; see the correction below |
| 2nd | ~~`desugarLambdas`~~ | **rewrite** | **done** — `kg_hir::rewrite::lambdas`, **1289 of the corpus's 1306 lambdas** rewritten, graded by the reference at `xtask ir`'s `lambda diff` line, 351/351 over `library/ + test/`. Prepends a capture alias and a `LambdaFunction` into the **enclosing target's** `Seq` and leaves a `cast<Closure<…>>` at the use site (`Function.hs:189-262`). Not "the enclosing block": `lambdaDefs` stops only at `Function`/`StaticIf`/`StaticFor`/`Lambda`, so a lambda in an `if` lands in the *function* body while its `declScopeName` still says `$quasi0`. It does **not** hoist — this row said "lifts each lambda body to a sibling declaration" until the oracle showed the lambda still inside `main` at inner pass 2 and hoisted only at 4. The hoist and the `main@?lambda0` rename are `moveFunctions` |
| 3rd | ~~`abbreviatedFunctionTemplate`~~ | **rewrite** | **done** — `kg_hir::rewrite::abbreviated_function_template`, wired into nothing, graded by `xtask ir`'s `rewrite:` line at 65 declarations. Turns `inline void F(auto x)` into a `TemplateF` wrapping the same function **and keeps the original** — a `SeqF` of two declarations from one, the second load-bearing until `Frontend.hs:300-302` deletes it seven passes later. One synthetic `typename x$T` per `auto` parameter, indexed among `auto` parameters only |
| 4th | ~~`moveFunctions`~~ | **rewrite** | **done 2026-08-13** — `kg_hir::rewrite::move_functions`, 2027 lines, **1215 declarations hoisted and mangled**, self-reported by `xtask ir`'s `moves:` line and graded by the reference at `move diff: 339/351` (12 booked to two unported passes, 0 DIFF). Hoists functions and classes declared inside a function to the parent scope, renaming them (`Function.hs:84-128`, plus its only helper `moveToParentScope` at **`:28-82`**, which is the bulk of it — **not** `:84-160`, which omits 44 of the 78 non-blank lines and runs into `typeParam` and `resolvedFuncParams`; the third wrong line range found in this table). Explicitly *defers* inside a `Template`, a `static if`/`static for`, an abbreviated template and a local class — those are moved only after instantiation. Reads **no type**, and `Template.hs:76` passes it bare — but "and no symbol map" was wrong, corrected 2026-08-13 by survey: `moveToParentScope` builds one **inside the pass, per node**, from `getSymbols'' AllSymbols $ body parent` (`Function.hs:33`), and `memberAccess` (`:52-64`) discriminates on it. It must **not** be `Db::global_symbols` — `body parent` excludes the parent's own parameter list, so a whole-program map qualifies a name the reference leaves bare. The dependency argument survives intact, because what it needed was "no *whole-program* map and no type" |
| 5th | `inferType` | analysis | already ported (`passes::types`); it is a fixpoint over the *rewritten* trees, so it is the consumer, not the blocker |
| 6th | ~~`closureCalls`~~ | **rewrite** | **done 2026-09-19** — `kg_db::elaborate::closure_calls`: replaces a call through a closure value with a call to the closure's function plus a cast of the capture (`Closure.hs:19-26`). It reads types, so it runs at the start of the round after the one that made the copy, over that round's twin types — the reference's order too |
| 7th | `deduceTemplateArgs` | rewrite | already ported as a *reporter* (pass 4). Its rewrite — the resolved argument list — is what the tree needs |
| 8th | `inferType` | analysis | as the 5th |
| 9th | `templateTypeCheck` | **diagnostic** | done, above |

**Rows 6 and 7 were swapped until 2026-08-13.** `passes` is consumed by
`foldr ($)` (`Template.hs:60`), so the list reads backwards and the sixth thing to
run is `Template.hs:74`, `closureCalls`, not `:73`. Measured as well as read: the
`TemplateInstanceF` jump that `deduceTemplateArgs` alone produces lands at
`--template-passes 6→7`. `--template-passes N` keeps the **last** N entries
(`:63`), which is the **first** N in execution order — so `tp=3` vs `4` isolates
`moveFunctions` exactly, and the gates built on that boundary are unaffected.

Five are genuine tree rewrites; two are `inferType`, which exists; one is the
rewriting half of a pass already ported as a reporter. **None of the five is
diagnostic-only**, so none can be sliced off the way `templateTypeCheck` was.

> **Resized 2026-08-12**, by five independent surveys each run against the
> oracle rather than against a reading. Three corrections that change the plan:
>
> * **The ~110 is downstream.** These five emit almost no diagnostics — measured,
>   `closureCalls` delivers **+1** and the other four **zero, by construction**.
>   The ~110 belongs to passes 11, 12, 15, 17, to `deduceTemplateArgs`' rewrite
>   half and to the second `inferType`, all of which need a tree only these five
>   can produce. Count by blocking cause, not by filename — the same correction
>   §2 already records for passes 18 and 5.
> * **Inference over the IR is a dependency of exactly one of the five.**
>   `desugarLambdas`, `abbreviatedFunctionTemplate` and `moveFunctions` read no
>   type and take no whole-program symbol map (confirmed three ways, including
>   that the oracle produces their whole output below both `inferType` calls).
>   *Whole-program* is load-bearing in that sentence: `moveFunctions` does build
>   a symbol map, per node, over one body — see row 4.
>   Only `closureCalls` is dead without it, and nothing in pass 8 consumes what
>   it emits. So four syntactic rewrites can land before the hard part starts.
>
>   **Wrong, corrected 2026-08-13: it is two of the five, and therefore three
>   syntactic rewrites, not four.** The three named above are right and two of
>   them have landed. But `addLambdaWrappers` was never checked and it reads
>   types in both of its resolution branches — `tIsReference $ typeOf o` for a
>   member access and `tIsFunction (theType t)` for a function-typed variable,
>   plus `expFromType` to spell a parameter type back as an expression. It is
>   as blocked on IR inference as `closureCalls` is. The ordering consequence
>   is immediate: **`moveFunctions` is the next slice, not `addLambdaWrappers`**,
>   even though `addLambdaWrappers` runs first in execution order.
> * **The risk is inverted.** These five put ~271 `expected:0` assertions and 137
>   of 300 rewritten files at risk to gain about one. Pass 8's rewriting half can
>   lose several hundred assertions and gain almost none.
>
> **Materialisation splits in two, and only the second half is blocked.** The
> mangler (`Mangle.hs:17-64`) and `resolve` (`Template.hs:158-164`) create no
> declarations — they rewrite *references*, turning a decided
> `TemplateInstance` into `Holder<|$uint@8|>`. No copy enters the arena, so
> the `TypeKey` collision never arises and this half is unblocked today. It is
> also the cheapest strong gate in pass 8: a byte-for-byte string comparison,
> no spans to reconcile and no `Own`/`Donated` question. `makeInstance` is
> where the copies appear and where §1d's fifth field becomes mandatory —
> and where it must land, since a discriminator with nothing to discriminate
> is the fifth instance of this repo's recurring failure.
>
> **The IR-inference route is decided, 2026-08-14: print and reparse.** Built
> far enough to verify — a materialised copy printed, reparsed and inferred by
> the existing engine returns the reference's types node-for-node, and the
> fixpoint closes because round 2's decided set falls out of running the
> existing census over round 1's printed output while round 2 materialises on
> the *original* arena. ~800-1,200 new lines against 3,500-5,000 for the engine
> port, but the reason is not typing: the CST engine stays either way, since it
> is what every keystroke runs on, so porting it leaves **two engines that must
> agree forever**. Two costs nobody had priced and one gap nobody had counted
> are in [`journal.md`](../journal.md), "Print and reparse works".
>
> Sequence, reordered twice on 2026-08-13: ~~substrate +
> `abbreviatedFunctionTemplate`~~ (**done**) → ~~`captureThis` +
> `desugarLambdas`~~ (**done**) → ~~`moveFunctions`~~ (**done**) → **the mangler
> and `resolve`** → `TypeKey`'s expansion field + `makeInstance` →
> IR inference → `addLambdaWrappers` + `closureCalls`. **Six slices, five
> predicting zero.** Four to
> six thousand lines for the rewrites, plus 1,000–1,500 for materialisation.
> Full reasoning, the four substrate contradictions and the `written` rule that
> resolves them, in [`journal.md`](../journal.md).
>
> **Slice 1 landed 2026-08-12** and corrected three things this entry asserted.
> The two `Typename` nodes are two nodes, not one shared — so there is no DAG in
> `abbreviatedFunctionTemplate` and the "proposes to violate the no-sharing
> rule" claim is withdrawn. The blast radius is **35 files / 102 declarations /
> 125 `auto` parameters**, of which 65 fire and 37 revert — not "18 files and 88
> synthetic parameters"; the 18 is reproducible as an artifact of reading
> `tail -1` of the oracle's output for a file inside base's import closure. And
> gate clause four had to be *weakened* to bite at all: "forbid an `Own` node the
> lowering did not create" rejects every legitimate replacement, because an
> append-only arena has no way to change a node in place. It reads "an appended
> `Own` node must not be structurally equal to a lowered node at its own key".
>
> **Slice 6a landed 2026-08-13**: `captureThis` (`kg_hir::rewrite::capture_this`),
> **648 lambdas** — every one lexically inside a `class`. This entry's forecast
> was right that no existing gate could see it and wrong about which instrument
> could: not the tree differential (passes 1 and 2 have run by `--passes 3` and
> neither is ported, so the shapes differ for unrelated reasons) but a **count**
> differential, `ThisF` and `LambdaF` at `--passes 3`, which isolates the pass
> because nothing before pass 8 else mints a `ThisF`. `capture diff: 122/122`
> at landing, **351/351** once slice 6c widened it past `library/` —
> every library root the oracle accepts, and the first whole-corpus line on
> which a rewrite is graded by the reference rather than by this port. Three
> claims elsewhere in the repo about pass 3 were wrong and are corrected; see
> [`journal.md`](../journal.md).
>
> **Slice 6b landed 2026-08-13**: `desugarLambdas`, 1,849 lines, **1289 of the
> corpus's 1306 lambdas**. The line-range citations above were both wrong and
> are corrected — `:189-262` not `:189-202` (which lands mid-`lambdaRef`), and
> `addLambdaWrappers` is `:366-423` not `:366-380`. Two predictions in the
> design were also wrong: `deep_copy_with_donor` is **not** needed (the two
> `makeCapture` results are independent constructions in the reference too, so
> the head-donor difference is a call-site parameter), and the `Rewriter` name
> builders stayed private to the pass — they have one caller until
> `addLambdaWrappers` lands. **Zero substrate additions.**
>
> The gate is the part that generalises, and it is *not* the `capture diff`
> count trick: `addLambdaWrappers` is sub-pass 1, unported, and mints the same
> constructors, so the reference's `+141 AliasF` can only ever be 114 on our
> side and wrapper declarations land inside *other* lambdas' bodies. What works
> is a **per-lambda ledger** — `(name, make_captureN arity, param count,
> generic?, uncapture field indices)` — with `$wrapper$` entries dropped by name
> and pairing by adjacency, guarded by the 5 lambdas the reference leaves
> standing under `static for`. `lambda diff: 122/122` at landing, **351/351**
> after the widening, +42s.

### ~~1b-i. The donor differential~~ — **withdrawn 2026-08-13; built, measured, rejected**

> The flag this section asks for shipped as `--roots-only`. The differential was
> then built on it, measured at **93.4% node agreement**, and abandoned — our
> spans and the reference's are different objects and three relational encodings
> each failed for a different reason. What replaced it is a **house rule**: every
> rewrite lands with donor assertions read off `--notes`, proven by swapping two
> donors and watching the test go red. See `CLAUDE.md` and the journal entry "The
> donor gate that cannot exist, and the 6.6% that says why". The text below is
> kept because its diagnosis of *why* `xtask ir` is blind to donors is still the
> reason the house rule exists; its **proposal is dead work**.


`xtask ir` is **silent about `Src` donors**. Every clause of `check_provenance`
but one fires only on `written == Own`; the exception, "this `Src::head` names a
live CST node", fires on every reachable node, so a `Donated` node's whole
obligation is that its range resolves to some CST node — which `Src::donated`
makes true by construction. Measured, not argued: swapping the two donors in
`abbreviated_function_template` (`Function.hs:296` vs `:307`) leaves the gate
byte-identical, and so does hanging both synthesized nodes on the parameter's
*name* token. One unit test is the only thing in the tree that catches it.

The instrument exists and has been run in both directions: compare
`kg dump ir --rewrite` against `kg-oracle --notes --passes 8 --template-passes N
--template-iterations 0`, resolving each note to the **CST node it names**
rather than comparing offsets — ours are trivia-trimmed, the reference's include
trailing whitespace, so a byte comparison cannot work.

What it needs to be affordable: a driver flag that emits notes for the **root
module only**. `--notes` today prints the whole transitive import closure on one
line — 11 MB and 3.4 s for `library/codec/crc.k`, against 0.7 s for `--tree` —
which is ~10× more than the comparison needs. With that flag, and fanned out the
way `oracle_trees` already fans out, this lands in the same 15–30 s the existing
`oracle diff` line costs.

Worth doing **before** the next rewrite, not after: the same comparison grades
all five, because what it checks is "which node donated", and each of the four
remaining rewrites synthesizes more nodes than this one did.

### 1c. `decidedInstances`

`Template.hs:85-105`. A `Map QualifiedName (Map ArgSeq Note)` — for every
template, the set of distinct argument lists it is instantiated with, which is
what `instantiate` then expands (`Template.hs:111-120`).

The paramorphism has four rules and each is load-bearing:

* a `TemplateInstance a (Seq args)` **whose children contributed nothing**
  (`all Map.null x`) and which passes `templateResolved` contributes itself —
  the "nothing nested inside me is still undecided" test;
* an instance whose arguments are resolved but which *does* contain nested
  instances contributes the union of its children **plus** `indirectInstances`:
  `resolveTemplateParam` is applied to each of the template's parameters and the
  result re-walked, which is how `T1<S1>` also decides `S1<uint32>` from inside
  `T1`'s body;
* `StaticIfF` contributes **only** its condition, `StaticForF` only its limit —
  an untaken branch must not decide an instance that elaboration will delete;
* `TemplateF` contributes **only** `templateParams`, never `templateDecl` — the
  same exclusion `crate::facts::reached_only_after_elaboration` already encodes
  for inference.

Computing it here needs, on top of what exists: (a) a whole-program instance
walk rather than a per-file one — `Db::program`'s file list plus
`Db::global_symbols(roots, AllSymbols)` — **refuted by measurement
2026-08-13: rooting at the whole corpus, or at all of `library/`, returns
*zero* templates and zero instances, because `decidedInstances`' error gate
(`Template.hs:55,84`) is whole-program and all-or-nothing and one broken file
empties the set. Restricted to the 233 individually-clean files, one root set
still decides fewer than the union of per-file root sets — 23/47 against
26/49. The per-file rooting is not a limitation to be lifted. It also means
§2's LSP rooting change would, done naively, disable the instance census for
any workspace holding one erroring file**; (b) a *key* for an argument list, which
in the reference is the `Seq` node compared structurally and here would have to
be the normalised argument types; and (c) `resolveTemplateParam`, which is
`inferType' . desugar resolveTypeParam` — a rewrite. (a) and (b) are a
week-shaped piece of work on top of the query layer. **(c) is the architectural
question**, and nothing in `decidedInstances` can be computed without it.

> **Landed 2026-08-12, without (c).** (a) and (b) are `Db::body_ir` /
> `Db::module_instances` / `Db::instances` (`kg_db::instances`), reached by
> `kg dump instances` and gated by `cargo xtask ir` — rules 1, 3 and 4, with
> rule 1's guard restated over the decided set instead of `resolve`'s rewrite,
> iterated round by round. The sentence above overstates (c): it blocks only
> rule 2's `indirectInstances`, which is declined toward silence, not the other
> three rules. What the corpus can decide without it is now measured — 49
> distinct instances, nesting depth 1 — and the journal entry of that date
> records why the depth-2 machinery is corpus-unreachable until instantiation
> itself lands.

### 1d. The architectural question

The reference materialises an instantiated copy of each template body — a fresh
tree with `TypeParam` nodes replaced by arguments and every declared name
mangled — and re-runs `inferType` over it (`Template.hs:122-139`). This port's
CST is lossless and is never rewritten, so the copy has to live somewhere else.
Two candidates:

**(a) Virtual instantiation.** No new tree. `kg_ty::infer::TypeKey` gains an
instantiation dimension, so one syntax node holds a different type under each
instantiation of the template that encloses it.

> **Decided 2026-08-13, and (a) and (b) are not alternatives.** The fifth
> `TypeKey` field is required *inside* (b) as well. Measured: plant
> materialisation-shaped copies into the arena and **163–169 of the 174
> template-bearing corpus files fail `check_provenance`** — the faithful
> `makeInstance` shape worst of the three, on ordinary `RETURN_STMT`/`BLOCK_STMT`
> body statements that are copied byte-identical and wear `Own`. So a
> materialised copy **keeps `Own`** — demoting it to `Donated` empties the
> `written == Own` index precisely inside the template bodies where the 536
> unannotated `auto` hints live — and the key grows an **expansion**
> discriminator, derivable from the `SrcStack` frame `Src::expanded` already
> pushes. Not `ExpId`: `arena.rs` forbids an arena index as a key on purpose.
> It does **not** settle divergence #24 — refuted by measurement the same day,
> because 201 of 201 collisions there have every claimant at `Expansion::ROOT`.
> #24 takes the *other* of its two resolutions, demoting the inner leaf —
> **landed 2026-08-13**, which took the provenance and rewrite gates off
> `lower()` and onto `lower_sorted()`; see divergence #24, now closed. And
> the field lands **with `makeInstance`**, never before — `Src::expanded`,
> `ExpansionInterner`, `ExpansionData` and `InstanceId` have zero production
> callers today, which is this repo's recurring failure already landed a fifth
> time. Full reasoning in [`journal.md`](../journal.md).

*Evidence it fits.* The key is already a value, not a pointer —
`TypeKey { file, offset, end, kind }` (`kg_ty::infer`) — deliberately, so that
results survive the tree being rebuilt between rounds. Adding a fifth field is
mechanical. Inference is already keyed and memoized per module
(`Db::type_table`), and `passes::types`' `Walk` already carries a `SymbolPath`
scope it could extend with the instantiation. `crate::facts::
reached_only_after_elaboration` already knows a template body is unvisited; the
change is to visit it once per instantiation instead of never.

*What it costs.* Every consumer of a type becomes instantiation-relative. The
diagnostic passes have no instantiation to report *under*, so each one needs a
rule for which instantiation's diagnostics reach the user and how duplicates
across instantiations are merged — the reference gets that for free because the
copies are distinct nodes with distinct ranges, and here they are the same
range. The LSP is worse: hover and inlay hints on a node inside a template body
would have several answers, and picking one is a product decision, not a port.
The memo table grows by the number of distinct instantiations, and the
whole-corpus `check` is already 68 ms.

**(b) A lowered IR that may be rewritten.** The CST stays untouched; pass 8
lowers to a second representation and instantiates *that*.

*Evidence it fits.* This is the only option that reproduces the reference's
algorithm rather than approximating it — `moveFunctions`, `desugarLambdas`,
`abbreviatedFunctionTemplate`, `addLambdaWrappers` and `closureCalls` all
*create declarations that were never written*, and there is no key-space trick
that makes a synthetic declaration appear in a tree that must round-trip byte
for byte (`xtask corpus`, 479/479). The crate graph already has the seam: every
pass takes `&ast::SourceFile` plus tables, so a pass reading an IR instead
changes one call site in `passes::run`. `kg_resolve::lower_file` is already a
lowering, and `Db` is already the place where a per-module derived structure is
memoized.

*What it costs.* The IR is a new artefact to keep in sync with the grammar, and
every diagnostic it produces needs a range that maps back to real source — which
means the IR carries provenance on every node, and a synthetic node carries the
provenance of whatever it was expanded from.

> **Corrected 2026-08-12.** This paragraph used to continue: "that mapping is the
> entire difficulty, and it is what makes an error inside `Foo<int32>` point at
> the instantiation rather than at the template." **Both halves are false.** The
> reference points at the **template body**, in the template's file, and renders
> the instantiation chain as `while compiling` notes — `desugarWithNote` puts the
> copied node's own note on the *left* of `n' <> n` (`Desugar.hs:92-94`), `<>` is
> a stack push whose head never moves (`Parser/Syntax.hs:623-626`), and every
> consumer reports the head (`Error.hs:182,197,201`). And there is no mapping to
> invent: synthesis goes through `newNote` (`Type.hs:983-984`), which hands the
> new node a *donor's* range verbatim. **Provenance is a copy, not a mapping.**
> See [`journal.md`](../journal.md). The nine passes that read the CST
today keep reading it; only pass 8 and its downstream consumers (11, 12, 15, 17)
move.

**Assessment.** (b) fits this codebase and (a) does not, and the argument is
stronger than what follows knew: `resolve` (`Template.hs:159-163`) replaces a
decided `TemplateInstance` node with its mangled name and the fixpoint applies it
as `desugar (instantiate . resolve)` (`Template.hs:58`), which is a rewrite
*inside* `decidedInstances`' own convergence argument. The decisive evidence
is not performance, it is that five of the eight sub-passes *add declarations*.
Virtual instantiation can give an existing node a second type; it cannot give a
program a function that was never written, and `desugarLambdas` and
`abbreviatedFunctionTemplate` do exactly that on every corpus file that uses a
lambda or an `auto` parameter. (a) would therefore cover `inferType` and
`templateTypeCheck` — the two entries that are already done — and stop.

**The entry exists** — see [`journal.md`](../journal.md), "Provenance is a copy, not
a mapping". The representation is `Src`, the reference's own type transcribed,
with the stack tail interned and one field added (`Written::Own | Donated`) that
the reference does not need because it never answers a position question. Three
things it establishes that change this section:

* the correction quoted above — the reference points at the template body;
* **rule 1 of `decidedInstances` cannot be ported literally.** `all Map.null x`
  is true only because `resolve` replaced the inner instance on the *previous*
  round; a tree that is never rewritten finds `optional<fixed<I, F>>` still
  nested forever and never decides the outer instance. 424 argument lists across
  87 of 469 corpus files contain another user template, so this is the library,
  not an edge case;
* **the scope boundary below is wrong by at least three passes.**
  `higherOrderFunctions`, `captureThis` and `trimExternClasses` all execute
  before pass 8 and all end up on the IR.

And one claim this roadmap makes elsewhere is now in doubt: pass 8 may **not** be
what unblocks the library's unannotated `auto` inlay hints. Every LSP handler
roots at `&[snap.file]` (`hover.rs:69`, `inlay_hints.rs:98`, `definition.rs:60`)
and `Program::files` is the import closure *downward*, while a template's
instances are decided by the files that **import** it. Opening
`library/numeric/float32.k` yields an empty instance set. If that holds, the
unlock is a rooting change and this roadmap should stop attributing the hints to
pass 8. Measuring it is the first experiment listed in the entry.

### 1e. How the materialised tree gets a type — **decided 2026-08-13**

§1d chose (b), a lowered IR that may be rewritten. That leaves one question it
did not ask: the reference re-runs `inferType` over the copy it materialises
(`Template.hs:122-139`), and this port's inference engine (`kg-ty`, `kg-const`)
runs over `ast::Expr`, not over the arena. So *how* does a materialised subtree
get a type?

**(a) Port `inferExpTree` onto the IR.** A second inference engine beside
`kg-ty`'s, which also has to re-target `kg-const`'s fold from `ast::Expr` to the
arena — a second port hiding inside the first.

**(b) Print the materialised subtree, reparse it, and let the existing frontend
infer it.** Chosen, and proved end to end against the oracle before it was
chosen: the fixpoint closes, and printing costs ~1 ms per file per round.

Two costs, both priced in advance rather than discovered:

* **No mangled or synthesized name lexes.** `<|` never lexes at any length —
  the 5,040-character mangled name is therefore a non-question, not a limit —
  and `$this` and `?lambda0` fail as identifiers. Post-pipeline trees carry
  `?lambdaN$capture` and `$quasiN` too, so a **bijective rename layer** is
  mandatory for the real pass-8 composition, not only for mangles. It needs a
  reverse substitution over `Ty`, because `Ty::Reference`/`Struct`/`Class`/
  `Instance` all carry qualified names and a type read back from the synthetic
  file spells the alias.
* **The read-back.** Types inferred over the synthetic file are keyed at
  `(SYNTH file, twin range, twin kind, ROOT)` and have to be written at
  `(node.src.head, node.kind, node.src.via)` — which is what the fifth
  `TypeKey` field in §1d is for. A lockstep DFS over the two arenas is the side
  map.

Landed for it: `sorted trip: 403/403` (`xtask ir`), the round trip over
`lower_sorted` — the tree the rewrites actually run on. It read **188/394** when
the route was chosen, and closing it meant exact qualifier printing rather than a
heuristic, because a print-and-reparse route is only as sound as its printer.
And `pretty_with_source`, guarded by `ModuleIr::lowered_len` rather than by
`Src::written == Own`: `Own` means *sole claimant of a `(range, kind)` in its
tree*, neither "synthesized" nor "rewritten", so an `Own`-guarded slice prints
the original spelling back over a rewritten name and silently un-rewrites it.
A fresh arena index is the only reliable "a rewrite made this", and it bounds
the whole subtree.

**The rename layer landed 2026-08-14** — `kg_hir::rename`, `__kg_mN`, with the
`Ty` back-translation, graded by `xtask ir`'s `rename: 351/351 (1771 symbols)`.
Printing the same materialised rounds *unrenamed* reads 196/351, which is the
line's own falsification. Its refusals are the load-bearing half: a rename
freshens every ancestor past `lowered_len`, so renaming a name the printer
never emits costs the printer its slices for the whole enclosing subtree and
changes nothing the parser sees.

**The census constexpr fold landed the same day** — `kg_db::fold`,
`evaluateConstexpr`'s half of `inferExpTree` *asked* rather than rewritten, so
`W<8>` and `W<bitsizeof(uint8)>` are one instance. Scheduled regardless of
route, and under (b) it was a census extension rather than a port, since the
printed copy is real source and the existing `ConstEval` machinery applies to
it — an asymmetry that is part of why (b) won. It also corrected the decline
accounting: of the declines that remain, **122 are `optional<decltype(…)>`**
and belong to the type-dependent-rewrite class, not to this one. `conformance`
2085→**2087**, `--strict` 521→**519**, 26/49 templates and instances →
**28/53**.

**The read-back landed 2026-08-14** — `kg_hir::readback` (positional side map,
five-field writes, `unrename_ty` on the way home), `kg_db::elaborate` (the
production route, wired into `Db::check` and the language server), and
`deduceTemplateArgs`' rewrite half for fully-deduced scalar-type calls of
same-module templates. `conformance` 2087→2088, `--strict` 519→518, graded by
`xtask ir`'s `type agreement:` line against a new `kg-oracle --typed` mode.
What remains of this section, in dependency order:

1. ~~**The cross-module twin program.**~~ Landed 2026-09-06 — see the
   queue at the top and the journal.
2. **`checkConversion'`'s error clause and the `checkReturn` site**
   (`TypeCheck.hs:77-87,161-178`) — 15 of `template-argument-deduction.k`'s 41
   are `Invalid conversion` and move on this, not on more instantiation. The
   journal entry "The read-back landed…" has the measurement.
3. ~~**`setTypeOfTemplateCall` without a written argument list**~~ — landed
   2026-09-06: the segment of a bare callee is stamped with the expected
   type, so deduction's third source reports the conflict the reference
   reports (`Could be `bool` or `E``).
4. The deduce slice's declines, each counted in `ModuleReport`: `TIntArg`
   (needs the census's re-stamp on a minted literal), defaults
   (`substituteDefault` rewrites the default expression), explicit prefixes,
   named-type arguments (key/mangle agreement with the sorter's spelling).

## ~~2. The cheap passes~~ — **four of them are done; the estimate was wrong in an instructive way**

| # | Pass | Estimated | Landed |
|---|---|---|---|
| ~~18~~ | `interpolatedStringPost` | ~13, "self-contained" | **3** — and it is *not* self-contained |
| ~~5~~ | `intrinsics` | "pass 9 already special-cases the same six names" | **0**, by construction — the clause has no `desugarError` in it |
| ~~3~~ | `reifyEnums`, `captureThis` | | **0** — already ported, no module written |
| ~~17~~ | `deduceAuto` | | **5** |
| ~~12~~ | `unresolvedTemplates`, GT arm | ~34, but wants pass 8 first | **6** — the "Too many arguments" arm alone, which wants pass 8 *not at all* (`TypeCheck.hs:324`: an over-long list can never resolve, so it always survives to pass 12). The other four arms hold the remaining ~43 and do want pass 8, plus a pass-15-shaped account of the leftovers it deletes — `passes::unresolved_templates` has the per-arm measurements |
| 11 | `unresolvedFunctionTemplateArgs` | | survey: 2, both by pre-empting pass 16's `clog2` clause |
| 7 | `trimExternClasses` | | survey: 0 |
| 15 | `removeTemplate` | | survey: 0 new; it is the *suppressor* the other arms of 11/12 wait on |

Eight assertions for four passes, against an estimate of "~13 from pass 18
alone". Each miss has a different cause and each is worth carrying forward to
the four rows still open:

- **Pass 18 was mis-blamed.** The `unwired` table said "needs inference". The
  real constraint is the lexer: a whole `"…"` is one `STRING` token and
  `Event::Token` can only merge tokens, never split one, so the interpolated
  expression **is not a node** and there is no `TypeKey` to ask about. A
  whole-program type environment would move this pass by zero. Re-splitting the
  string token in the parser is the unlock, and it is parser work, not pass work.
- **Pass 5 was never going to move anything and the table implied it would.**
  "Pass 9 special-cases the same six names" was read as *this pass makes that
  special case redundant*. It does not: pass 9 here asks four symbol-**map**
  questions and pass 5 writes a **type table**. Removing the list costs 15
  assertions, measured. The two now share `intrinsics::NAMES` so they cannot
  drift, and that is the whole of the reconciliation.
- **Pass 3 needed a reader, not a writer.** Its numbering was ported into
  `scope::enum_constant_value` months ago and both of `reifyEnums`' consumers
  were already done. The right output was a file that does not exist.
- **Pass 17 does not close the inlay-hint gap**, which is what it looked like it
  would. Pass 15 `removeTemplate` deletes template bodies at execution position
  15, *before* 17, so `deduceAuto` never sees one. The library's unannotated
  `auto` bindings are reached only through pass 8's instantiations.

The pattern: the estimate came from counting assertions in a file whose name
matched the pass. Three of the four times, most of that file's failures belonged
to a *different* pass. Count by blocking cause, not by filename — the per-file
breakdown in [`status.md`](../status.md) now says which cause for the two biggest
clusters.

### What is left in pass 18, and why it is not more of the same

Six of the ten remaining are pass 18's own clauses and want a name → `Ty`
lookup. Four would fall out of a `SymbolEnv`-shaped parameter on `passes::run` —
the same seam `run`'s doc comment already describes for passes 9, 10 and 17 —
and three of those additionally want a function's return type. The other four
are not pass 18's at all: two are `Parser.hs:900` parse errors inside the format
spec, two are pass 9.

## ~~3. Profile the formatter~~ — **done, and the answer was "leave it alone"**

`kg-fmt` is still the slowest stage — **97.1 ms** worst case against **74.6 ms**
for a full check — and it is on the format-on-save path, so it is user-facing
latency. It had never been profiled, which was the whole reason it was listed.

It has been now. A quarter of formatting was parsing: one file was parsed four
times, once at the top, once in each of `realign`'s two anchor passes, and once
in the verifier. `realign`'s second parse is now taken only when the first pass
actually moved something, and the input tree's two anchor sets are gathered in
one walk instead of two. 104.2 → 99.6 ms in the controlled A/B, about 4.5%.

The rest is structural and the profile argues for keeping it: lowering is 27%
and is the actual work, verification is 21% and is the only reason a formatter
that moves comments can be trusted, and the two surviving parses are of two
different strings. The framing "the formatter is the slowest stage" invited the
reading that something was wrong with it. Nothing is.

The lever that would move it is **a faster parser** — 40 MiB/s, and it appears
four times here. That is a separate project, and it would pay off in `check` and
`keystroke` too. It is not scheduled.

## ~~2. Close the remaining constexpr-fold sites~~ — **done, and the item was built on a false premise**

Four sites converted — `PARAM_INT_TYPE`, the two array bounds, `FAN_OUT_EXPR`
and the `IntAttr` attribute arm. **Zero assertions moved**, and the survey that
preceded the work is why that was expected rather than disappointing: every
corpus line this item cited as a "pin" is an already-passing accept. The gain is
that `uint<FOO>` diagnoses like `uint<32>`, hovers are right, and the rules
downstream of a resolved type are reachable at all.

Three sites are deliberately **not** converted, and this is now a rule in the
code rather than a backlog entry:

| Site | Why not |
|---|---|
| `deduce_int` | folding a template formal's extent binds an argument the reference leaves unbound; `reconcile` answers a conflict with a `TError` (`Type.hs:1500-1505`) and the instance mangling shifts. Two ways to break an `expected:0`. |
| `infer_binary`'s guard and literal rules | test `isUntypedIntLiteral` (`Type.hs:1092-1095`); a `ConstInt` cannot tell `4` from `4u8`, and the wrong branch computes a wrong width |
| `infer_mux`'s selector | a `ConstValue::Enum` must not select an arm; `folded_int` flattens `Enum` into `Int` |

The last two want a value-returning `folded_value` companion rather than
`folded_int`. That is a real piece of work and it is **not scheduled**, because
it also moves no assertion — see [`journal.md`](../journal.md).

Where the decidability at this seam actually lives is `fold_divergent`
(`passes/type_errors.rs`), which today suppresses every pass-10 error under any
node containing a foldable leaf. Narrowing it is the item worth having; it is
not this one.


## Later

- **The C++ backend bridge** — 241 assertions. Carries the trap in
  [`known-divergences.md`](../known-divergences.md): a frontend that diagnoses
  *earlier* than the reference **fails** those assertions.
- **Language server**: completion landed 2026-09-06; rename and signature
  help 2026-09-07 (`prepareRename` declines what `definition_at` declines and
  refuses, with a sentence, what the search cannot promise to have seen;
  signature help reads the written declaration, so a template answers).

- **Workspace-wide references, and the rooting question under it.** Today
  `buffers_of_this_project` (`handlers/references.rs:117`) is *open buffers*, so
  find-references misses every file nobody has opened. The obvious fix — root at
  the workspace — is not obviously right, and the reason is not performance:

  `Db::definition_at(&roots, …)` resolves names in the program `roots` entails,
  and `handlers/definition.rs` says why that is the open file: "an editor is
  looking at one buffer, and the program a jump is answered against is the one
  that buffer entails." Rooting at the whole workspace lets a name resolve
  through a module the file never imports. That is **over-resolution**, the same
  hazard divergence #19 records, and it would change goto-definition answers, not
  just widen a search.

  So the two candidates trade off against each other. Search more files, each
  still rooted at itself — correct, and costs one `Db::program` per file.
  Or root once at the workspace — one program, and wrong. `occurrences`
  (`references.rs:180`) takes one root per plan entry, so either shape is a
  contained change to `kg-lsp`; the decision is semantic, not structural.

  Unmeasured, and the measurement has to be in-process: a CLI proxy is dominated
  by process startup (12 separate `kg check` runs cost 0.43 s against 0.31 s for
  one program over all 124 library files, which says nothing about the marginal
  cost of the 124th `Db::program` inside one warm database).

  **Not the same item as the inlay hints.** That payoff — ~536 — needs pass 8's
  materialisation before rooting matters at all, because the instance set is
  empty either way until instances exist.
- ~~**Struct and union fields**~~ — resolve since 2026-09-06 (see the queue
  above).
- **Getting a keystroke under 16 ms.** Now 107.7 ms, and it went *up* with the
  type environment. Parsing is 9% of a check, so an incremental parser is not the
  lever — the frontend is. Memoizing inference per module took the cross-file
  half, which is why batch `check` fell 13%; what is left is all inside the
  edited file, so this needs incremental work *within* a file. Nobody has
  profiled where the 107 ms goes — and the formatter is the argument for
  profiling it before touching anything: the assumption that the slowest stage
  must be hiding a large win did not survive contact with the profile.

## The queue of 2026-09-06 (moved from the decisions of that date)

The queue, in order:

| # | slice | why here |
|---|---|---|
| 1 | ~~`cargo xtask errors`~~ — **done**, 131 → 43 disagreements the same day | the instrument for the class of bug `conformance` cannot see; built before the work it grades |
| 2 | ~~struct and union fields lowered in `kg-resolve`~~ — **done**, same day | goto-definition, highlight, hover and references on `s.x`, resolved through the base type; fields are items, never symbols |
| 3 | ~~the cross-module twin program~~ — **done** 2026-09-06, conformance 2137 → 2144, strict 469 → 461 | arguments re-minted in the declaring module, one alias pool per program, the twin's imports overlaid with elaborated maps; §1e item 3 (`setTypeOfTemplateCall` without a written argument list) fell with it |
| 4 | ~~completion: names in scope, members after `.`~~ — **done**, same day | `kg_db::Db::completions_at`; driven over stdio before it was called done |
| 5 | ~~`checkConversion'`'s error clause and `checkReturn`~~ — **done**, same day: conformance 2098 → 2137, strict 508 → 469 | the largest class of snippets we compiled cleanly; guarded by `returnVoid`, which the corpus pins with two backend-code assertions |
| 6 | `kg-backend-sys` + `kg-ptbridge`, walking a template-free program — **in progress** 2026-09-17: the ABI loads (`kg-backend-sys`), `cargo xtask ptrace` records the reference per corpus program (`kg-ptrace`), `kg-ptbridge` walks pass 8's output into a `Backend` trait with a trace-writing and a real implementation, and `cargo xtask ptdiff` grades the walk call by call: **1742 of 2422** programs structurally identical to the reference's walk on 2026-09-22 (1696 on 2026-09-20, 1672 earlier that day, 1579 on 2026-09-19 night, 1119 that morning, 890 on 2026-09-17, 580 that morning; the journal's entries since "An abbreviated function is a template" have the rows that moved it). `kg build` and the compat `kanagawa` compile through the real backend, and `cargo xtask backend` grades that end to end: the backend gives the same verdict for **1907 of 2422** walked programs, 2643 of 3270 overall, on 2026-09-22 (1855 on 2026-09-20, 1783 on 2026-09-19 night, 1372 that morning, 1114 on 2026-09-17, 912 that morning). No corpus program the walk completes exports a class yet, so the first `.sv` waits on the instances the route declines in `base.k`'s closure. See [`ParseTree_Integration_And_Test.md`](ParseTree_Integration_And_Test.md) and the journal's three entries of that date | the first `kg build` that produces RTL |
| 7 | ~~the remaining `errors` classes~~ — **done** 2026-09-08 for everything but parse recovery: 43 → 30 disagreements, 18 of them parse positions | the reference's `hasErrors` checkpoints after passes 1, 8 and 9 (`Frontend.hs:38-52`), `inScope` for hints, and eight one-line verdicts (mux type arms, attribute arguments, widths with errors inside, the bracket, `clog2` as an argument, member callees, named supertypes, "Failed to instantiate template"). Left: 18 megaparsec recovery positions, one hidden behind item 8's closure half, `umul27`, an undecodable file |
| 8 | ~~function-typed template parameters~~ — **done** 2026-09-08, both halves | `validate typ@FunctionType{}` (`TypeCheck.hs:269-275`) for a named function against a written function type, earlier `typename` arguments substituted, a method refused; and the closure half — the callback's closure type against the declared one with the capture class first, spelled as `make_captureN` (`Function.hs:313-345,365-392`) — read off the tree at pass 4 rather than through the twin, which the row had assumed necessary. Still silent: a `const auto` holding a lambda, a lambda returning `auto` |
| 9 | ~~deduction's two failure verdicts~~ — **done** 2026-09-07: conformance 2144 → 2161, strict 461 → 444, `errors` 222 → 239 agree at the same 43 | member-template calls `o.Fn_T(x)` deduce through the receiver's class (`Type.hs:1390-1391`), and pass 12's "Could not deduce all arguments" is reported from pass 4's own `sequence` for calls and from the written list for class and alias templates. Two guards keep it honest: a verdict only when every input was decided, and only on formal shapes whose deduction clauses are ported whole |
| 11 | ~~rename~~ — **done** 2026-09-07, driven over stdio | the references search with a replacement attached; refuses — an unopened importer, an interpolation hole, the base library, a non-identifier — rather than edit less than every use. Every refusal is a sentence the client shows |
| 12 | ~~two more of the silent classes~~ — **done** 2026-09-07: conformance 2161 → 2182, strict 444 → 423, `errors` 239 → 260 agree at the same 43 | `tInitializedType`'s error slots reported on their element (`TypeCheck.hs:153`), and `static assert` over two types folded through the type table (`Constexpr.hs:244-249`) — with `decltype` of a folded expression declined, found by a `warning:0` snippet the first cut broke |
| 13 | ~~signature help~~ — **done** 2026-09-07, driven over stdio | the written parameter list of the declaration goto-definition lands on, spans as UTF-16 offsets, active parameter by commas; a template function answers because its parameters are text before its type exists |
| 10 | ~~diagnostics from inside a template body~~ — **done** 2026-09-07: conformance 2182 → 2196, strict 423 → 409, `errors` 260 → 271 agree at the same 43 | the passes run over the twin and what they raise inside a copy is carried home through the side map to the body's own range, with each instantiation site as a `while compiling` note (`Desugar.hs:92-94`, `Error.hs:176-195`) — rendered by both CLIs and published as related information. Found and fixed on the way: the copy now reads `validateArg`'s re-stamped literal (`TypeCheck.hs:281-283`), and pass 16 folds a trusted `bitsizeof`. Still silent, and each its own blocker: a member template of a class instance (depth two), a deduced call into a helper with a value argument (`ArgShape::Opaque`), pass 13 over an exported helper instance, and pass 9 inside a copy |
| 14 | **the cross-module copies the route still declines** — six clauses and the member-call type **done** 2026-09-08 | surveyed by mechanism (2026-09-08): six were unported clauses, not cross-module (enum base type, exported template, cross-module `export`/`extern`, the abbreviated LT, `deduceAuto`'s return arm, the LT frame), and one was a member call having no type at all (`method`, `Type.hs:1222`). Still declined: instances mentioned *inside a copy* — `Bar<T, …>(x)` in `Foo<uint32>::Do`, `c.Foo<uint16>(x)` on a `T1<bool>`, `Foo<bool>::S<false>` — which is a second elaboration round over the rewritten IR; foreign arguments the route cannot mint (a named type from the importing module, a function value); template-template parameter checks ("Failed to instantiate template"); `undefinedSymbols` inside a copy |
| 15 | ~~the small families of the verbose errors list~~ — **done** 2026-09-10: conformance 2267 → 2296, strict 338 → 309, `errors` 349 → 374 agree at the same 26 | nine verdicts a written tree plus the type table decide outright, each probed against the oracle first: argument counts (`TypeCheck.hs:91`), `[[reset]]` (`:73-82`), the three `auto`s no pass replaces (`:51`), a function-typed variable through an alias or `decltype` (`:140`), a global closure's capture (`:146-148`), non-integer and unfoldable attribute arguments (`:198-199`), `bytesizeof`/`bitoffsetof` folded through the typed evaluator (`Constexpr.hs:162-166,274-275`), enum constants naming each other (`:205-215`). Found on the way: a parser lookahead gap for a local function-typed `const`, and a stamp in `kg-ty` that broke the fixpoint's monotonicity under debug assertions. Next by size: the name-not-in-scope class (24; pass 9's file-wide under-approximation and a bogus second segment on a resolved class), `symbol is ambiguous` (7) and the import qualifier (6), both `findSymbol` (`SymbolTable.hs:229-250`) |

Rule for the queue: a DX slice ships when a user can *type something* that
reaches it (`CLAUDE.md`, "The recurring failure"), not when its tests pass.

