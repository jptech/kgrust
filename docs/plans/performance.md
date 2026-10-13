# Performance: the loop, and the gates that wait on it

Status: **plan 2026-09-26; mostly done 2026-09-27** (§10 the progress,
§11 the standing against the reference, §13 a whole build by phase and
by round, with the optimisations triaged). What is left — Step 3's deferred
items and Step 4 — continues in [`../history/sprint-parity.md`](../history/sprint-parity.md)
W5 and W0.2. Written after the twin's
deletion, when the program-wide instantiation loop (`kg_sem::elaborate`)
became the only elaboration — and, measured, almost all of the frontend's
time. The testing loop is bound by the same cost: the gates elaborate
every corpus program, several times.

| Step | State |
|---|---|
| 1 incremental inference (1.1–1.4) | done, and carried across fixpoints; `KG_INFER_FULL=1` keeps the whole walk as the reference |
| 2 backend verdict cache | done, with over-budget verdicts |
| 3 copy less | in-place rewrites, `desugar` and the walk done; snapshots refreshed from what changed, the passes walking only what can change, 2026-10-10 (§10); compaction measured and not the memory's lever |
| 4 round count against the reference | not started |
| 5 one build for the gates | doc tests removed; the two profiles stay (tests 35 s at rest) |

## 1. Where the time goes (measured 2026-09-26)

| Measurement | Value | Of which |
|---|---|---|
| `kg check test/library/data/array.k` | 16.6 s | loop 15.7 s — 31 rounds |
| `kg check test/library/processor/risc-v.k` | 26.3 s, 2.4 GB | loop ~25 s — 16 rounds |
| `kg check` of a trivial program (base library) | 0.14 s | loop 0.06 s |
| full `scripts/gates.sh` | 1229 s | `tests` 313, `output` 264, `backend` 183, `ir` 100, `ptdiff` 52 |
| `tests` stage | 313 s | ~250 s compiling a second profile; 60 s one test (`tests/query.rs`'s rooted door over whole library programs) |
| `scripts/fast.sh walk` | ~6.5 min | `backend` ~4.5 min of it |

A sampled profile of the loop over `array.k` (inclusive, 13.4k samples):

| Where | Share | What it is |
|---|---|---|
| `Program::infer` | 72% | the `inferType` fixpoint, run three times a round |
| — `Cx::tree` / `Cx::apply` | 40% / 28% | every node of every module visited, every iteration |
| — `Symbols` rebuilt | 25% | `Symbols::of` over every module, every iteration |
| — `unresolved` | 8% | a second full walk per iteration, for the stop test |
| — `Program::snapshot` | ~9% | every module's arena and type map copied, every iteration |
| everything else in a round | 28% | the rewrite passes (`round_prefix`, `deduce`, wrappers, census, instantiate), each forking whole arenas |

Self time is dominated by copying — `Vec`/`Exp` clones, arena drops,
`malloc`/`free` — not by the inference rules themselves.

Two structural facts behind the numbers:

- **Every iteration redoes the whole program.** A type, once resolved, is
  never changed (`resolveTypeOf`'s monotonicity, `Type.hs:1037`; the
  port's `TypeMap::resolve`), yet each iteration re-walks resolved subtrees,
  re-derives every declaration, and re-counts every node.
- **The snapshot is a copy.** `inferType` reads declarations "as the round
  found them" (`getSymbols p` before the pass), and inference rewrites nodes
  in place (`Cx::replace` → `node_mut`), so the port copies every arena and
  type map per iteration to have that view.

## 2. Principles

- **Exactness first.** Every step below is an optimisation of the same
  computation: the loop's final trees and types must be **byte-identical**
  before and after (§3.6's identity gate), and every ratchet neutral. A step
  that changes a result is a bug in the step, not a trade.
- **The bench stays a tripwire** ([`gates.md`](../gates.md)). These steps are scheduled
  now because the loop's cost has become the testing loop's cost; each is
  measured before and after, interleaved, on a quiet machine.
- **Per change, `scripts/fast.sh`**; the full gates at each step's end.

## 3. Step 1 — incremental inference (the big lever)

Target: the loop's 72% in `Program::infer`. Estimate 3–10× on the loop,
most of it on the large programs the gates and the editor stall on.

The fixpoint today (`crates/kg-sem/src/infer.rs`, `Program::infer`):

```text
loop {
    snapshot  = copy of every module's arena and types      // ~9%
    symbols   = Symbols::of(every module)                   // ~25%
    for each module: tree(root)                             // ~68%
    sig       = unresolved(every module)                    // ~8%
    stop if sig == previous sig
}
```

Four independent changes, each landed and verified on its own, in this
order (cheapest and safest first).

### 3.1 The snapshot by version, not by copy

What the snapshot answers: a declaration's `Exp` and type *as of the
iteration's start*. Both can be read from the live structures if each
change remembers when it happened.

- **Types.** `TypeMap` gains an epoch per entry (the iteration that wrote
  it). `decl_ty` reads "the entry, unless it was written in the current
  epoch — then the value it replaced". Since a resolved entry is never
  overwritten, "the value it replaced" is almost always *unresolved*, and a
  one-slot previous value per entry suffices; keep the full undo log only if
  measurement finds an entry rewritten twice in one iteration.
- **Nodes.** `Cx::replace` appends the old `Exp` to a per-iteration undo log
  keyed by `ExpId` before writing. `decl_exp` consults the log first. The log
  is cleared at the next iteration's start.
- `deduce.rs` and `closures.rs` call `Program::snapshot()` once per pass;
  they keep the copying version until 3.1 has proved itself in `infer`,
  then move to the same view.

Removes ~9% and most of the clone/drop/`malloc` self time. Risk: a read
that bypasses `decl_*` and reaches the live arena for a declaration — found
by the identity gate; the fix is to route it through the view.

### 3.2 The symbol map maintained, not rebuilt

`Symbols` is a map from qualified name to `Decl`, rebuilt from every module
per iteration because a declaration *can* change: `staticControl` takes a
`static if`'s branch (declarations appear, a `NestedScope` is spliced), and
an instantiation round adds copies.

- Build it per module: `Symbols` becomes the union of per-module maps, each
  with a **dirty** flag.
- A rewrite that can change what a module declares marks it dirty: the
  `static if`/`static for` arms of `static_control` and any `replace` whose
  old or new `Exp` is a declaration form (the set `symbols::Collect` walks).
  Everything else leaves the module's map as it was.
- Per iteration, rebuild only dirty modules' maps. Between rounds, the
  census's own `Symbols::with` (`elaborate.rs`, `deduce.rs`, `closures.rs`,
  `check.rs`) reuses the same per-module maps under their `Config`.

Removes most of the 25%. Risk: a declaration-changing rewrite not marked —
the identity gate, plus a debug-build assertion that recomputes a module's
map when it is *not* dirty and compares.

### 3.3 Walk only what can still change

The core step. A subtree need not be revisited when nothing in it can
change this iteration:

- every node in it is resolved, **and**
- nothing in it changed last iteration, **and**
- no declaration it read changed last iteration.

Mechanism:

- **Change marks.** Each iteration records the set of nodes whose type was
  set or whose `Exp` was replaced (the 3.1 log already holds the second).
  A node is *dirty* if it changed, and a dirty mark propagates to its
  ancestors (a subtree summary bit, kept in a side table keyed by `ExpId`,
  invalidated by any `replace` inside it).
- **Read dependencies.** `Cx::lookup_symbol` / `type_of_symbol` /
  `decl_ty` record `(reader subtree root, Decl)` pairs as they run. When a
  `Decl`'s type or node changes, every recorded reader is marked dirty for
  the next iteration. (Coarse is fine: record at the enclosing statement or
  declaration, not per node.)
- **The walk.** `Cx::tree` enters a child only if it is dirty or not yet
  fully resolved. Order is unchanged for the nodes it does visit, so the
  rules see what they saw before.
- **Iteration 1** of every `infer` call visits everything — nothing is
  known yet — and seeds the marks. A new round's copies arrive unresolved,
  so they are visited; untouched modules stay clean and cost a root check.

Removes most of `tree`/`apply`'s 68% on every iteration after the first,
and makes a round's cost proportional to what the round changed.

Risks, and what answers each:

- *A rule that reads something other than a declaration or its own
  subtree* — an ancestor's type, a sibling, the expansions interner. Audit
  `infer_exp_type`, `evaluate_constexpr` and `static_control` for every
  read that leaves the node's subtree (`self.ty(x)` where `x` is not a
  descendant) and record those as dependencies too.
- *A resolved node whose `evaluate_constexpr` fold only becomes possible
  later* — a fold reads a declaration's value, which is a recorded read, so
  the declaration's change re-dirties it.
- The identity gate (§3.6) is the arbiter: any divergence names the first
  module and node that differ.

### 3.4 The stop test without a second walk

The reference stops when the unresolved count repeats (`sig`,
`Type/Inference.hs:31-44`) — not when nothing changed, and the difference is
observable, so the count itself must be kept exact.

- The reference's `sig` is already per module (`parallel (para go)`), so keep
  a per-module unresolved count, adjusted as types are resolved
  (`-1` when an entry goes unresolved → resolved) and as `replace` adds or
  drops reachable nodes (count the subtree that leaves and the one that
  arrives — both are small).
- Until that bookkeeping is proven, compute `unresolved` only over dirty
  modules and reuse the last count for clean ones.

Removes the 8%.

### 3.5 Three inferences a round, as the reference has them

Each round runs `inferType` after the prefix passes, after deduction, and
after instantiation (`Template.hs:58`, `66-78`). That is the reference's shape and
stays. With 3.3, the second and third cost what their preceding pass
changed, not a whole-program walk. No separate change is needed; measure it.

### 3.6 Verification: the identity gate

The old fixpoint stays, behind `KG_INFER_FULL=1`, until the new one matches
it everywhere:

- `cargo xtask sem --identity`: for every corpus root, elaborate twice
  (full and incremental) and compare, module by module, the rendered final
  tree (`kg_hir::structure::render_exp`) and every node's type. Report the
  first differing node with both values. Must be 0 differences.
- Run per sub-step (3.1 → 3.4), then `scripts/fast.sh walk`, then the full
  gates at the end of Step 1.
- Unit tests for the mechanism itself: a program whose second iteration
  depends on a declaration resolved in the first (red with the dependency
  unrecorded), a `static if` that adds a declaration mid-fixpoint (red with
  the dirty mark unset), a fold that becomes possible only after a
  constant resolves.

Then `KG_INFER_FULL` and the full path are deleted.

### 3.7 Expected result

On `array.k`: 15.7 s → estimate 2–5 s (the per-round overhead of the
rewrite passes becomes the floor, §5). On the gates: `tests`' rooted-door
test (60 s) and every elaborating stage shrink proportionally. Keystroke
(bounded to four rounds) falls with it.

## 4. Step 2 — a backend verdict cache for the gates

Target: `backend` (~183 s in the full gates, ~4.5 min of `fast.sh walk`),
`output` (~264 s) and `repoint`, which rerun the C++ backend on every
program even when our walk of it has not changed. Most changes move a
handful of programs.

### 4.1 The seam

`kg_ptbridge::isolate::compile` already splits a build in two: the walk
into a `Recording` in-process, then `Worker::run(library, &recording)`
replays `recording.encode()` into the backend in a child process. The
recording is **everything the backend is told** — `InitCompiler`'s options
are one of its calls — so the backend's behaviour is a function of:

- the encoded recording,
- the backend library (`libkanagawa-backend.dylib`),
- the working directory the worker inherits (relative output paths),
- the environment, as far as the backend reads it.

### 4.2 The cache

In `Worker::run`, when `KG_BACKEND_CACHE=<dir>` is set:

- **Key**: a hash of
  - the encoded recording bytes,
  - the library's path, size and modification time (hashing the 50 MB
    library per program would cost more than it saves),
  - a cache format version.
- **Hit**: replay the stored stdout and stderr to this process's own, restore
  the stored output files into the working directory, and return the stored
  `Verdict`. No worker is started.
- **Miss**: run the worker with stdout and stderr piped (teed to this
  process's own as they arrive, so the terminal still sees them), snapshot
  the working directory's file list before and after, and store the verdict,
  both streams and every file created or changed. Crashes are **not**
  cached: a budget kill or signal is not a function of the recording.

The gates set `KG_BACKEND_CACHE=target/backend-cache` for the processes
they spawn (`xtask/src/backend.rs`, `output.rs`, `repoint.rs`); nothing
else does, so `kg build` for a user behaves exactly as today.

### 4.3 Verification and invalidation

- `KG_BACKEND_CACHE_VERIFY=1` runs the worker even on a hit and compares
  the verdict, the streams and the files with the stored entry, reporting
  any difference. The milestone gate run uses it, so the cache's premise —
  the backend is deterministic given the recording — is re-proven at every
  milestone rather than assumed.
- A rebuilt backend changes the library's key: every entry misses.
- The cache is bounded (size cap, least recently used out); `cargo xtask
  backend --no-cache` bypasses it.
- The output files are stored per entry. `output` compares them, so the
  restored bytes must be exactly what was written — the verify mode checks
  that too.

### 4.4 What it does not change

- The frontend still runs for every program: the recording is what the key
  is made of. That part is the loop's cost, which Step 1 addresses.
- Stream interleaving: on a miss the two streams are captured separately.
  The gates read the first `Error N` from stdout and the exit code, which
  interleaving cannot affect; a user without the variable sees no change.

### 4.5 Expected result

For a typical change (a few programs' walks move): `backend` and `output`
fall to roughly their frontend-only cost — about `ptdiff`'s 52 s for all
3270 programs — plus the handful of misses. Estimate: `fast.sh walk` ~6.5
min → ~2 min, the full gates ~7 min shorter.

## 5. Step 3 — copy less

After Step 1, the rest of a round (28% today) is the floor:

- The rewrite passes return a **new** `ModuleIr` each (`round_prefix`,
  `pipeline_tail`, `resolve_instances_everywhere`, `instantiate_templates`,
  `deduplicate_instances`), each starting with `ir.fork()` — a whole-arena
  copy per module per pass per round. The arena is append-only except for
  inference's in-place rewrites; a pass that only appends and re-roots can
  work on the module's own arena. Convert the passes to `&mut ModuleIr`.
- `Cx::tree` clones each node's `Exp` to match on it (`self.exp(id).clone()`);
  most arms need only its child ids. Borrow instead.
- **Compaction.** Replaced subtrees stay in the arena forever; `risc-v.k`
  peaks at 2.4 GB. Between rounds, drop nodes unreachable from the root
  (renumbering, with the side tables that key by `ExpId` remapped). Faster
  traversal and the memory the backend gate's budget kill needs.

## 6. Step 4 — the round count against the reference

`array.k` takes 31 rounds and `risc-v.k` 16, and every round costs three
inferences. Compare with the reference's own count
(`kg-oracle --template-iterations N`, the smallest `N` that reaches the
fixpoint). If the reference decides more per round, the difference is a
fidelity gap — a census that decides one level per round where the
reference decides several — and closing it cuts rounds and multiplies with
Step 1. If the counts agree, record it here and move on.

## 7. Step 5 — one build for the gates

`gates.sh` builds the workspace twice: `cargo test --profile quick` for the
`tests` stage, `--release` for everything the gates run. Most of the
`tests` stage's ~313 s is that second compile. Either run the tests under
the release profile, or run the gates' binaries from the quick profile —
whichever is faster in total, measured.

## 8. Order and milestones

| Step | Change | Verified by | Expected |
|---|---|---|---|
| 2 | backend verdict cache | verify mode at the milestone | `fast.sh walk` ~6.5 → ~2 min |
| 1.1 | snapshot by version | identity gate | loop −10% |
| 1.2 | symbol map maintained | identity gate + debug assertion | loop −25% |
| 1.3 | walk what can change | identity gate + unit tests | loop 3–10× overall |
| 1.4 | stop test without a walk | identity gate | loop −8% |
| — | full gates; delete `KG_INFER_FULL` | milestone | |
| 3 | copy less, compaction | fast tier | memory ÷, loop floor lower |
| 4 | round count vs the reference | oracle | fewer rounds, or a recorded match |
| 5 | one build | full gates' timing | `tests` stage −200 s |

Step 2 goes first because it is independent, contained, and shortens the
testing loop every later step uses.

## 9. Measuring

- `KG_LOOP_TIME=1 kg check FILE` prints the loop's laps; the phase split in
  §1 is summed from it.
- `KG_LOOP_PROFILE=1 kg check FILE` prints the loop by pass (all, idle,
  the root's) and, since 2026-10-11, by round (§13); `KG_PHASES=1 kg
  check|build FILE` the frontend by phase, and under `kg build` the walk
  and the backend after it. `kanagawa --verbose` prints the backend's own
  stage timers. All trace only.
- `sample <pid> <secs>` (macOS) for the profile; the inclusive-time
  aggregation used for §1 is worth keeping as a script
  (`scripts/profile-loop.sh`), added with Step 1.
- Before/after pairs are interleaved on a quiet machine
  ([`AGENTS.md`](../../AGENTS.md), "Performance is a goal").
- Record each step's before/after here, under the step.

## 10. Progress

- **Step 2, 2026-09-26** (`db62936`). The cache landed as designed, with
  verification sampled rather than all-or-nothing (`gates.sh` re-runs one
  hit in 20). Measured on the backend gate: cold 954 s, warm 945 s wall —
  CPU 1084 s → 507 s, reports identical — the wall being one program,
  `test/library/processor/risc-v.k`, whose backend runs to the 900 s budget
  on every run. The reference itself takes over 25 minutes and 8.3 GB on
  it (`--place-iterations=1`), so no version of ours finishes it inside the
  gate's budget; see the over-budget verdict below.
- **Step 1.1 + 1.2, 2026-09-26.** `KG_INFER_TIME=1` prints each
  iteration's split. On `array.k`: 96 `inferType` calls, 384 iterations,
  of which 107 changed nothing and most of the rest a handful of the
  ~142k nodes walked; inference 10.9 s of 17.2 s (walk 5.4, symbols 2.6,
  sig 1.6, snapshot 1.3). The snapshot is now an overlay — each
  `TypeMap`'s epoch journal, and the expressions `Cx::replace` overwrote —
  and the symbol map is kept until a rewrite changes what its subtree
  files (`symbols::contributions`, compared before and after). `array.k`
  17.0 → 12.9 s. `cargo xtask infer-identity` (both modes, every corpus
  root, by `elaborate::fingerprint`): 304/304 identical in 41 s; each
  mechanism's mutant is caught (142 roots for a map never rebuilt).
- **Step 2, the over-budget verdict, 2026-09-26.** Timed alone, the
  reference compiles `risc-v.k` in 2629 s at a 13.4 GB peak — past any
  budget the gates can afford, so ours can only ever crash there, and did,
  at 8 GB (alone) or 900 s (under a full gate's load, when macOS compresses
  memory). Under the cache `kg` now stops the worker itself — at
  `KG_BACKEND_CACHE_BUDGET_SECS` (600) or at the gates' own group budget,
  sampled every 200 ms to their 500 — and keeps that crash, budget in the
  key. `risc-v.k` in the backend gate: 98 s cold, 22 s warm (its frontend),
  where it had cost up to 15 minutes on every run.
- **Step 1.3, 2026-09-27.** Declarations are units, skipped while
  nothing they own or read has changed. The first version finished
  identical on every root yet drifted mid-way — `array.k`'s unresolved
  counts parted from the whole walk's at one fixpoint's fourth iteration
  and met again later — which the fingerprint could not see. Two tools
  came of it: the fingerprint now carries every fixpoint's trajectory
  (`Report::infer_trace`), and `KG_INFER_CHECK=1` runs, beside each
  incremental iteration, a whole-walk one from the same state and names
  the first differing node with its path, units and owners. They found
  what reasoning had missed: the tree is a DAG (`fold_named_value` shares
  an initializer's children), so a node has every unit that walked it or
  read it as a child for owners, and a write reaches a later sharer the
  same iteration; a count is kept per unit with the units that counted
  each node, so sharing cannot stale it; and a read is remembered by
  node, not by its owner — a declaration in a body not yet walked is read
  before any walk owns it. `array.k`: 16.2 s whole, 11.3 s incremental,
  trajectories identical; `infer-identity` 304/304. Mutants: readers
  ignored 304 roots, first owner only 189, counts never invalidated 304;
  the mid-iteration notice and child ownership are corpus-silent and cost
  3%, kept as the argument requires.
- **Where `array.k` stands (11.3 s).** Inference 6.1 s, of which the
  first iteration of each of 96 fixpoints is 4.5 s — the passes between
  them rebuild the trees, so every fixpoint starts from nothing — and the
  rest 1.6 s. The other passes 5.2 s. Next: the first iteration's cost
  per node, carrying units across fixpoints, and Step 3.
- **Hot spots and Step 3's first half, 2026-09-27.** A profile of the
  incremental build found `ExpArena::resolves` scanning the fork ancestry
  on every index (a fifth of the samples; now a binary search), the
  tracing switches read from the environment per node (6%; now once per
  process, `kg_sem::env_flag!`), and `tResolved` recomputed per ask (now
  kept per entry). Then the loop's per-pass copies went: the rewrites only
  append, so `pipeline_tail`, `round_prefix`, instance resolution,
  instantiation and deduplication append into each module's own arena
  (`*_in_place`), and `desugar` compares against the node it rebuilt
  rather than a second clone; the symbol map's walk is iterative over one
  stack. `array.k`: 11.3 → 5.5 s; the whole corpus elaborates in 11 s
  (8 workers). These changes move no inference mode, so
  `infer-identity` cannot see them; `infer-identity --dump DIR` writes
  every root's fingerprint (symbols spelled out — the numbering depends on
  what a worker's interner saw first) for `diff -r` against another
  build's: 304/304 identical against the commit before, and a mutant
  (children collected in reverse) differs on 224.
- **Units across fixpoints, 2026-09-27.** With the rewrites appending in
  place, a node's id is stable for the whole loop, and the passes between
  two `inferType` calls neither change an existing node nor type one they
  did not mint. So the units carry from one fixpoint to the next: an epoch
  opened as a fixpoint ends catches any type written in between (corpus-
  silent — the argument's guard), and every symbol-map lookup a unit made
  is remembered with its answer and asked again of each map built afresh
  (within a fixpoint too, replacing "a rebuilt map walks everything"); a
  unit whose answer moved is walked. `array.k` 5.5 → 4.8 s; inference is
  now 1.7 s of it (symbol maps 1.0, walk 0.5, count 0.1), the other passes
  3.7 s. Identity 304/304 with trajectories, the cross-build dump
  identical; lookups never re-asked differs on 178 roots.
- **Lookups, intrinsics, finer units, 2026-09-27.** `Symbols::in_scope`
  built a path per prefix tried (16% of `risc-v.k`); `SymbolPath` now
  borrows as `[Symbol]`. A rebuilt map is diffed against the one the
  remembered lookups were answered from and only lookups probing a changed
  key are asked again. `intrinsics` looks only at new nodes. The mid-pass
  notice logs only writes that change a value, and never marks a unit the
  pass has already reached. Units now include statements and calls: a pass
  that rewrites one call rebuilds every node above it, so a rebuilt
  function is a new unit and its kept statements are what can be skipped.
  `risc-v.k` 10.6 → 6.8 s, `array.k` 4.7 → 3.4 s; the corpus elaborates in
  7.8 s. `test/library/processor/risc-v.k` began the day at 26 s, `array.k`
  at 17 s.
- **The testing loop, 2026-09-27.** `scripts/fast.sh walk` 6.5 min → 95 s
  (the backend replayed from its cache: 40 s warm, where it had been
  183–900 s); workspace tests 85 → 35 s at rest (no doc tests); the full
  gates' last run 1384 s, of which 600 s was re-verifying the over-budget
  entry, since exempt.
- **Step 3, second half, 2026-09-27.** `desugar` and `desugarShallow'`
  gathered each node's children through `Exp::children_mut`, a vector per
  node of every pass; the combinators now stack the child ids on one
  buffer the `Rewriter` keeps and write slots back only where an answer
  differs. Inference's walk reads the few fields its arms need instead of
  cloning each node. `risc-v.k` 6.8 → 6.2 s, `array.k` 3.4 → 3.0 s; the
  corpus elaborates in 7.1 s. Cross-build dump identical.

  **Deferred, measured.** (1) The snapshot `deduce` and the wrappers
  take — every arena copied, 6.5% of `risc-v.k` — goes only with reads of
  the module being rewritten routed through its `Rewriter`: `deduce`'s
  `Template` holds the snapshot's arena across `deduce_instance`, which
  appends. (2) Compaction: the arenas keep every superseded node
  (`risc-v.k` peaks at 1.9 GB, from 2.4); dropping them means remapping
  every side table keyed by `ExpId` — the type maps, the incremental
  walk's units, owners, readers and remembered lookups — between rounds.
  Where `risc-v.k` stands (profile): `desugar` 39% (its algebras' clones
  and compares; every pass is a whole-program `cata` per round, as the
  reference's are), the symbol maps' builds 17%, `round_prefix`'s walks
  19%, inference 28%.
- **The round's floor, 2026-10-10** (sprint W5.2–W5.5; journal, "The
  loop's floor, kept rather than recomputed"). Seven commits, each graded
  by `infer-identity`, its `--dump` against `f8e8c72`'s (304/304
  identical every time) and `scripts/fast.sh loop`:
  - *The symbol maps* (W5.4): `kg_sem::symbols::Filed` keeps what each
    subtree files, per config and module; a build walks only the nodes
    made since the last and copies the rest. Inference vets each write in
    place (`symbols::harmless`); any other write discards the module's
    store. The three per-round maps 0.45 s → 0.06 s on `risc-v.k` (0.87
    s with the two snapshots their timers then included), inference's own
    builds 552 → 107 ms.
  - *The walk* (W5.3, measured first): clone/compare/drop were a quarter
    of each rewrite pass, the walk the rest — so `kg_hir::facts` keeps,
    per node, the shapes its subtree holds (forgotten along every
    `node_mut` through recorded parent edges), and every pass whose
    algebra acts on a few shapes (`desugar_within`,
    `ShallowRewrite::needs`) passes over a subtree holding none: the
    prefix, `resolve`, `instantiate`, `closureCalls`, the wrappers, the
    census, deduction. Prefix 1.15 → 0.25 s, instantiate 0.77 → 0.22,
    census 0.17 → 0.02, deduction 0.38 → 0.20.
  - *The memory* (W5.2, measured): `array.k`'s 1.4 GB footprint was
    210 MB live and 1.2 GB of freed large blocks macOS's allocator kept
    ("Malloc Large (empty)"; `MallocLargeCache=0` gave 191 MB) — the
    snapshots, every arena forked twice a round and freed, each round's
    larger than the last. The snapshot's storage is now kept
    (`ExpArena::fork_into`, `TypeMap::clone_into`) and refreshed from
    the arena's and the type map's write logs: deduce's and the
    wrappers' snapshots 0.41 s → 0.03 s. Compaction would drop ~40 MB of
    superseded nodes on `array.k`: not the lever.

  Interleaved A/B against `f8e8c72`, release `kg check`, peak footprint
  (`/usr/bin/time -l`): `array.k` 3.10 s / 1.41 GB → 0.95 s / 0.24 GB
  (48.5 → 14.3 G instructions); `risc-v.k` 6.22 s / 1.74 GB → 2.65 s /
  0.68 GB (94.4 → 39.7 G). Both under the reference's peaks (0.55, 1.1
  GB) and W5's time targets (1.5 s, 3 s). Loop profile on `risc-v.k`:
  5.45 s → 1.92 s, inference 1.04 s of it.

  **Next, measured.** Inference is now over half the loop: the first
  iteration of each fixpoint walks every unit a pass touched. Outside
  the loop, `undefined_symbols` and the sorter's fingerprints are a fifth
  of `kg check`. The snapshot is still a copy, kept current: an arena
  with a frozen, shared prefix and its appends beside it would make it
  free, at a branch per node read.
- **The keystroke, 2026-10-10** (journal, "The keystroke, by phase").
  Measured first: `cargo xtask bench --filter keystroke-phases` (new)
  breaks the `keystroke` stage's own number down — `rooted_settings`,
  the editor's four rounds, a trailing space alternated, N keystrokes
  after a warm-up — and `--keystrokes 10` less `--keystrokes 0` under
  `/usr/bin/time -l` counts their instructions. Seven commits, each
  graded by `infer-identity`, its `--dump` against `1df2e17`'s (304/304
  identical every time) and `scripts/fast.sh loop` (every ratchet
  +0/−0), each lever read off a `sample` profile of the keystroke:

  | `risc-v.k`, one keystroke | before (`1df2e17`) | after (`9344f68`) |
  |---|---:|---:|
  | wall, median of 10 (interleaved, load 7–13) | 616 / 595 ms | 311 / 317 ms |
  | the loop | 344 ms | 227 ms |
  | sorted module (the edited file's lowering) | 125 ms | 9 ms |
  | program (`mark_used_imports`) | 44 ms | 15 ms |
  | CST passes | 44 ms | 22 ms |
  | copies' check, module inference, the rest | 55 ms | 46 ms |
  | instructions, one keystroke of each file | 13.1 G | 6.9 G |
  | peak footprint after ten keystrokes | 1.29 GB | 0.54 GB |

  The levers, in the order measured: the harvest's fingerprints
  deferred until `nub` reads one (18% of the keystroke); `resolve`
  rendering argument lists only under a seeded base and once a pass;
  `mark_used_imports` reading each scope once (it was quadratic in a
  class's length) and `declared_anywhere` asked once a name;
  `moveFunctions`' two catas testing a name's last segment before
  building a key or a qualified name; deduction looking up only a
  callee a `Template` is filed under; `intrinsics` passing a name on
  its leaf; and mimalloc for the binaries (−18% instructions, and the
  footprint bounded: macOS's allocator kept ~90 MB a keystroke). The
  bench's keystroke stage, interleaved on a quiet machine: 598.6 /
  623.5 → 310.3 / 323.1 ms. `kg check`: `array.k` 1.05 s / 14.3 G
  instructions → 0.86–0.97 s / 11.3 G; `risc-v.k` 2.81–2.94 s / 39.8 G
  → 2.18–2.28 s / 27.0 G (peaks 0.24 → 0.29 GB and 0.70 → 0.78 GB:
  mimalloc's one-shot cost). Not taken, measured: fat LTO with one
  codegen unit (−8% instructions, −7% cycles, but a release rebuild
  after a one-file edit 17 s → 107 s) and thin LTO (−6%, 31 s) — the
  gates rebuild release every run.

  **Next, measured** (per `risc-v.k` keystroke, `KG_LOOP_PROFILE`):
  inference 69 ms (the first iteration of each fixpoint walks and
  re-counts every unit the passes made — `Units::count` was ~30 ms of
  a keystroke before mimalloc, 80% of it the root's first iterations),
  the round prefix 50 ms (`moveFunctions`' catas still walk each
  parent's body twice), deduction's walk 20 ms, instantiate with
  `resolve` 18 ms; outside the loop, the CST passes, the copies' check
  and the program's assembly ~50 ms between them. All of it re-runs
  from the edited file's lowering on every keystroke: ≤ 100 ms (M6) is
  W7.5d's, carrying the loop across edits, not more of this.

  **The stage is not the editor's program.** Under `risc-v.k`'s
  recorded settings — the full base library, 50 modules, what a
  project's `kg lsp` serves — a keystroke is ~0.88 s (`bench --filter
  keystroke-phases --recorded`, 25 keystrokes, load ~3), its loop
  ~0.69 s, and inference ~45% of that loop: every keystroke re-infers
  the whole library from fresh trees. The phase table there, before
  → after on a quiet machine: body edit 1.11 → 0.89 s, signature 1.16
  → 0.84 s, library body 1.05 → 0.78 s; the release `kg lsp` driven
  over stdio on a copy of `risc-v.k` under a manifest importing
  `library/`, ~1.35 → ~0.95 s an edit. What would bring that under
  400 ms is the library's inference carried across edits — W7.5d again.
- **The shipped binaries, 2026-10-12.** `cargo xtask dist` builds the
  `ship` profile, `release` with `lto = "fat"` and `codegen-units = 1`:
  `kg check` 6–10% faster in an interleaved A/B, `kg` 30% smaller, its
  output over the corpus identical (§11; journal, 2026-10-12). The gates
  and `cargo xtask` stay on `release`, which builds a minute faster.

## 11. Standing against the reference (re-measured 2026-10-11: one core, every core)

The reference frontend is `kg-oracle --errors` — the reference's own
Haskell library (`oracle/build.sh` copies it) run through every frontend
pass — built for this comparison with the shipped executable's options
(`-threaded -feager-blackholing -with-rtsopts=-A128M`, library at cabal's
default `-O1`, `compiler/kanagawa.cabal:24-29`), plus `-rtsopts` so its
cores can be set per run. Ours is `kg check` from the release build.

**Three columns.** The shipped compiler gives its runtime one capability
per processor (`app/Main.hs:62-68`, `--jobs` unset), and its frontend runs
its passes per module in parallel (`Template.hs:57-78`, `Symbols.hs:41`):
on this 16-core machine it uses every core. `oracle/driver` sets none, so
until 2026-10-11 this comparison timed the reference on one core without
saying so. It is now timed both ways — `+RTS -N1`, one core as `kg` runs
(`kg` is single threaded, `loop-across-edits.md` §8), and `+RTS -N`, every
core as a user of the shipped compiler gets — against ours.

Same machine, best of three, interleaved per program (one core, every
core, ours), `-I library`, each run's cwd outside the reference tree; time
and peak resident set from `/usr/bin/time -l`. Measured at `1737ca6` (the
frontend `1ebc57a`'s, plus trace flags), load 3.6 at the start, the
reference's own every-core runs raising it; every run within 3% of its
program's others, ours within 10%.

Raw runs: [`measurements/2026-10-11/vs-haskell-three-columns.txt`](../measurements/2026-10-11/vs-haskell-three-columns.txt).

| Program | lines | Haskell, 1 core | Haskell, 16 cores | Rust | faster than 1 core | faster than 16 cores | RSS: Haskell 1 / 16 cores / Rust |
|---|---:|---:|---:|---:|---:|---:|---|
| a four-line function (base library only) | 4 | 0.61 s | 0.62 s | 0.05 s | 12× | 12× | 171 / 2141 / 41 MB |
| `test/library/data/cache.k` | 812 | 4.62 s | 1.84 s | 0.22 s | 21× | 8× | 273 / 2230 / 101 MB |
| `test/library/data/bits.k` | 787 | 6.34 s | 2.12 s | 0.26 s | 24× | 8× | 262 / 2219 / 122 MB |
| `test/library/data/vector.k` | 758 | 20.95 s | 5.92 s | 0.67 s | 31× | 9× | 424 / 2260 / 256 MB |
| `test/library/data/array.k` | 1191 | 28.22 s | 12.79 s | 0.80 s | 35× | 16× | 526 / 2253 / 304 MB |
| `test/library/numeric/float.k` | 1058 | 30.38 s | 10.58 s | 0.66 s | 46× | 16× | 596 / 2315 / 296 MB |
| `test/library/processor/risc-v.k` | 4084 | 54.03 s | 24.61 s | 1.95 s | 28× | 13× | 1068 / 2823 / 776 MB |

- **Per core, 12–46× faster; against the shipped compiler as it runs,
  8–16×.** The reference gains 2.2–3.5× from 16 cores (none on the
  four-line function); its CPU time stays about its one-core time
  (`risc-v.k` 69 s of CPU in 25 s of wall time).
- **Its memory on every core is 2.1–2.8 GB** on every program, the
  four-line one included: `-A128M` is an allocation area *per core*.
  Against that ours is 4–50× less; against its one-core run, as before,
  less on every program.
- The one-core column agrees with the first reading of the day (at
  `1ebc57a`, `risc-v.k` 53.2 s) within 3%, and with 2026-09-27's within 4%.
- Multi-threading is therefore an opportunity on our side too (§13's
  triage): the reference's own gain says the per-module passes split.

(MB here is MiB, as `/usr/bin/time -l`'s bytes divided by 2^20.) The
one-core Haskell column agrees with 2026-09-27's to within 4% (`risc-v.k` 54.2 s
then), so the gain is ours. The Rust column against its earlier readings:

| Program | 2026-09-27 | 2026-10-10, loaded (load 7–11) | 2026-10-10, quiet, after the keystroke round | 2026-10-11, quiet |
|---|---:|---:|---:|---:|
| `array.k` | 3.05 s / 1367 MB | 1.01 s / 246 MB | 0.86–0.97 s / 291 MB | **0.79 s / 301 MB** |
| `risc-v.k` | 6.08 s / 1857 MB | 2.79 s / 710 MB | 2.18–2.28 s / 781 MB | **1.92 s / 797 MB** |

Since the last quiet reading the CST inference engine, the second census
and the twin's leftovers were deleted (W3.7, W4); the time is not
attributed commit by commit. The peak is 10–16 MB higher than that
reading — under mimalloc a one-shot run peaks a little higher than under
the system allocator, and D1 step 1's two larger structs (journal, "The
editor keeps its loop") account for part, unmeasured; it is still a
quarter under the reference on `risc-v.k` and under half of it on the
rest. Reproduce with `scripts/vs-haskell.sh` (about twelve minutes on a
quiet machine; `N=1 scripts/vs-haskell.sh cache.k` for a subset): it
builds the timing oracle once in `target/oracle-bench`, apart from the
gates' oracle and its cache, and prints each program's best.

Read with care:

- **The frontends are not doing identical work.** The comparison is of
  the whole `check` each offers, not of a pass. Since 2026-10-10 ours is
  graded complete (`FRONTEND_COMPLETE`, conformance 2681/2681, `--strict`
  0) and runs one engine, the loop; it still computes what the editor
  reads (positions, provenance) that the reference does not keep.
- **Memory is where we lose** on the largest programs: the arenas keep
  every superseded node (compaction is deferred, §10) and the incremental
  walk keeps owners, readers and lookups per node. On the small and middle
  programs we use less. — *Not so, 2026-10-10* (§10, "The round's
  floor"): the RSS was the allocator keeping freed snapshot copies, and
  with those gone `array.k` peaks at 0.24 GB and `risc-v.k` at 0.68 GB,
  in 0.95 s and 2.65 s.
- **The day's change**: `array.k` was 17 s and `risc-v.k` 26 s at the
  start of 2026-09-26's work — 1.6× and 2.1× the reference's speed; the
  ratio was 9× by that day's end, and is 12–46× per core on 2026-10-11,
  8–16× against the reference on all 16 cores (the table).
- **Ours is the `release` build.** The release archives are built with
  the `ship` profile (§10, 2026-10-12), which checks 6–10% faster again
  with identical output — an interleaved A/B against `release`, nine
  rounds: `array.k` 0.80 → 0.74 s, `float.k` 0.67 → 0.61 s, `risc-v.k`
  1.93 → 1.81 s. The table is not re-measured with it.

## 12. Memory per use case (2026-10-10)

The owner accepted on 2026-10-10 that the editor keep a second copy of a
program between edits for sprint W7.5d's D1, on the condition that memory
stays efficient and is watched across use cases. `cargo xtask mem` is the
watch (`xtask/src/mem.rs`). It runs each program through three use cases,
one fresh process each, under its recorded compile settings:

- `kg check`'s `Db::check`;
- the editor opening the file (`Db::diagnostics_layered` under
  `PassBudget::EDITOR`, to the fixpoint);
- the same after five edits, each run to the fixpoint.

It reads each peak resident set off `/usr/bin/time` and compares it with
`memory-budgets.json`, each budget its blessed peak plus a quarter. A peak
over its budget fails `scripts/gates.sh`; `status-md` shows the largest
program's row.

First reading, `b4ee408` on main before D1, under load 15–26 (MiB, peak
resident set):

| Program | `check` | editor open | after 5 edits | kept by the edits |
|---|---:|---:|---:|---:|
| a four-line function (the base library alone) | 42 | 46 | 78 | +32 |
| `test/library/data/cache.k` | 98 | 121 | 234 | +113 |
| `test/library/data/array.k` | 284 | 335 | 471 | +136 |
| `test/library/numeric/float.k` | 304 | 363 | 568 | +205 |
| `test/library/processor/risc-v.k` | 775 | 907 | 1522 | +615 |

The same tree read `risc-v.k`'s last column 1391 to 1522 MiB over three
runs. The other columns moved less than 3%.

**The editor's memory grows with edits, then levels off.** The
four-line program's peak resident set after `n` edits:

| `n` | 1 | 2 | 5 | 10 | 20 | 50 | 100 | 200 |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| kept (MiB) | +16 | +24 | +32 | +40 | +49 | +58 | +75 | +83 |

Traced after each edit, the resident set levels off: by edit 40 on the
four-line program (~120 MiB), and by edit 15 on `array.k` (~670 MiB).

**Not a leak.** A counting allocator in the child gave `array.k`'s live
heap after every edit as 133 MiB, flat from the first edit to the
twentieth. Its peak during an edit was 313 MiB. The rest is the
allocator's, in two parts:

- **The resident set overstates on macOS.** `vmmap` of the same process
  after twelve edits showed mimalloc's regions at 631 MiB resident and
  379 MiB dirty. The other 252 MiB is pages already purged, which the
  kernel takes back only under memory pressure. Activity Monitor shows
  the footprint, the dirty and compressed pages. On macOS `xtask mem`
  now grades the peak footprint (`/usr/bin/time -l`), and the resident
  set elsewhere.
- **mimalloc returns a freed page a second after it is freed**
  (`purge_delay`), and only while it is allocating. An idle editor
  therefore keeps whatever its last check freed.

What each change measured on `array.k`, twelve edits each run to the
fixpoint:

| | footprint after the edits | peak footprint | instructions |
|---|---:|---:|---:|
| default | 380 MB | 490 MB | — |
| `mi_collect` after each check | 211 MB | 484 MB | — |
| `purge_delay` 0 | 199–212 MB | 383–385 MB | +1.1% |

The same setting on `risc-v.k`'s editor-after-five peak: ~1187 →
~1070 MB, for +1.3% instructions. On `kg check` it saves 6% of the peak
for +1.6% instructions; there it stays at the default, because a
one-shot process returns everything when it exits.

Setting the option at run time does what the environment variable does
(199 MB / 385 MB). `kg lsp` and the `kg-lsp` binary now call
`kg_alloc::purge_eagerly()` before they serve, and `xtask mem`'s editor
cases call it too. The `kg-lsp` binary had been running on the system
allocator, which grew ~90 MB a keystroke; it now uses mimalloc like
every other binary. The allocator is now chosen in one place,
`kg-alloc`; three binaries had copied the declaration.

Under the footprint, with the server purging (`22463b7` plus the
change; MiB):

| Program | `check` | editor open | after 5 edits | kept |
|---|---:|---:|---:|---:|
| a four-line function | 37 | 38 | 65 | +27 |
| `cache.k` | 93 | 109 | 186 | +77 |
| `array.k` | 279 | 293 | 382 | +89 |
| `float.k` | 288 | 308 | 459 | +151 |
| `risc-v.k` | 772 | 858 | 1106 | +248 |

What an edit's own work can still save is the gap between its peak live
heap (313 MiB on `array.k`) and what it keeps (133 MiB). That gap is the
loop's transient: the rounds' forks and the program being built while
the old one is served.

## 13. A whole build, by phase and by round (2026-10-11)

Raw output: [`measurements/2026-10-11/`](../measurements/2026-10-11/README.md).

What a user waits for is the whole build, not `kg check`. Measured at
`1737ca6` (`kg` with the trace flags below), one run each unless noted,
load 3–5, the release binaries; every run's cwd outside the reference
tree; `-I library` (`--import-dir`), and the build's defaults — `-O 2` and
3000 placement iterations, what a user gets (the gates use
`--place-iterations=1`).

- `KG_PHASES=1 KG_LOOP_PROFILE=1 kg check -I library <file>` — the
  frontend by phase (`kg_db::stats`, each exclusive of those inside it),
  the loop by pass and, new, by round;
- `KG_PHASES=1 kg build -I library -o out/<name> <file>` — the frontend,
  then the walk and the backend;
- `kanagawa --verbose --import-dir library --backend=sv -o out/<name>
  <file>`, ours and the reference's (`kanagawa-build/dist/bin`) — the
  backend's own stage timers (`CompileTimer`, `kanagawa.cpp:54-202`),
  printed at `--verbose`. Its `Middle-end (inclusive)` encloses all of
  the backend's work, output included, so a run's frontend is its wall
  time less that figure. The stages nest and some add up across the
  backend's threads, so they do not sum to its wall time.

### Frontend against backend

| Program | our frontend | backend | our build | the reference's build (16 cores) | its frontend |
|---|---:|---:|---:|---:|---:|
| a four-line function | 0.05 s | ~0 (nothing to generate) | 0.06 s | 0.69 s | ~0.69 s |
| `data/cache.k` | 0.22 s | 6.0–9.6 s over runs | 6.2–9.8 s | 6.5–11.5 s | ~2.0 s |
| `data/bits.k` | 0.27 s | 4.8 s | 5.1 s | 7.4 s | ~2.5 s |
| `data/array.k` | 0.87 s | 10.4 s | 11.4 s | 22.8 s | ~13.9 s |
| `numeric/float.k` | 0.69 s | 14.6 s | 15.4 s | 28.6 s | ~13.1 s |
| `processor/risc-v.k` | 2.0 s | did not finish inside the gates' 8 GB / 900 s (local channel), the reference's neither; release channel: 4–20+ min, ~10 GB (`backend-survey.md` §7) | — | — | — |

- **The backend is 92–98% of our build.** The frontend is 2–8% of it.
- **The backend's time does not depend on which frontend feeds it.**
  Stage for stage it agrees with the reference's own run on the same
  program, within the backend's own noise. On `cache.k` two dead-code
  passes swing between runs on both sides — `RemoveUnusedGlobals`
  1.7–4.8 s, `RemoveDeadOperations` 0.9–2.5 s (three runs each side) —
  so `cache.k`'s whole build is 6–11.5 s whichever frontend feeds it.
- **The backend uses several cores** on its own (`float.k`: 64 s of CPU
  in 15 s of wall time), as the reference's frontend does (below).
- Its largest stages (ours; the reference's agree): `cache.k` the two
  dead-code passes and `Write Output` 1.0 s; `bits.k`
  `ScheduleBasicBlock` 2.1 s, `RemoveUnusedGlobals` 0.9, `RemoveDeadOperations`
  0.6, `BitConstantPropagation` 0.5; `array.k` `GlobalDataPropagation`
  2.4 s, `BitConstantPropagation` 2.1, the dead-code passes 1.4–3.0,
  `Generate IR` 0.6; `float.k` `Pipeline` 8.1 s (`LocalDataPropagation`
  inside it), `Write Output` 2.9, `ScheduleBasicBlock` 0.9. The backend
  is the reference's C++ and is measured only, not worked on (owner,
  2026-10-11).

### Our frontend by phase

| Program | wall | the loop | CST passes | copies' check | parse, item trees, program, lowering |
|---|---:|---:|---:|---:|---:|
| `cache.k` | 0.22 s | 0.153 | 0.038 | 0.007 | 0.023 |
| `array.k` | 0.87 s | 0.776 | 0.027 | 0.038 | 0.019 |
| `float.k` | 0.69 s | 0.507 | 0.078 | 0.044 | 0.057 |
| `risc-v.k` | 2.01 s | 1.695 | 0.115 | 0.122 | 0.057 |

### The loop by pass

Each pass with its whole-program steps (its symbol map, its snapshot);
inference with its per-module walks. Seconds.

| Program | rounds | inference | deduction | instantiate | lambda wrappers | prefix | closureCalls | census | in all |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `cache.k` | 16 | 0.043 | 0.031 | 0.033 | 0.015 | 0.003 | 0.005 | 0.002 | 0.132 |
| `array.k` | 31 | 0.344 | 0.146 | 0.079 | 0.109 | 0.013 | 0.026 | 0.019 | 0.735 |
| `float.k` | 30 | 0.223 | 0.101 | 0.058 | 0.048 | 0.003 | 0.014 | 0.009 | 0.455 |
| `risc-v.k` | 16 | 0.943 | 0.181 | 0.161 | 0.127 | 0.142 | 0.038 | 0.025 | 1.616 |

The symbol maps the census, deduction and the wrappers rebuild every
round are 0.122 s of `array.k`'s loop (17%), 0.042 of `float.k`'s, 0.062
of `risc-v.k`'s.

### The loop by round

`KG_LOOP_PROFILE` now prints a table per round as well, the passes
grouped and the root module's share beside them (`kg_sem::profile`). In
bands, ms:

| Program | before round 0 | rounds 0–3 | rounds 4–8 | rounds 9 on | the root's |
|---|---:|---:|---:|---:|---:|
| `cache.k` | 9 | 40 | 40 | 43 (8 rounds, 5 each) | 11 (9%) |
| `array.k` | 8 | 136 | 172 | **418** (23 rounds, 18 each) | 73 (10%) |
| `float.k` | 44 | 103 | 94 | **214** (22 rounds, 10 each) | 28 (6%) |
| `risc-v.k` | 27 | 520 | **734** | 336 (8 rounds, 42 each) | **585 (36%)** |

Two shapes. `risc-v.k`'s time is in its middle rounds, a third of it the
root module's own (round 2 is the dearest, 187 ms, two thirds of it
inference). `array.k` and `float.k` spend half their loop in a long tail
of cheap rounds, each re-running deduction over every module (2.3–3.4
ms a round), rebuilding the symbol maps (1–5 ms) and settling inference
(1–15 ms) for the few copies the round adds. The number of rounds is the
reference's (`loop-left` 321/321), so a round cannot be skipped; each can
be made cheaper.

### Optimisation opportunities, triaged (2026-10-11; nothing decided)

Frontend, ours; ordered by what the numbers say a user would feel:

0. **The backend** — the larger share of a whole build — is surveyed in
   [`backend-survey.md`](backend-survey.md) (2026-10-11): where each
   program's backend time goes and the options, none taken.
1. **Multi-threading the rounds** (`../history/sprint-parity.md` W5.6; §11's
   three-column table measures the reference's own gain from it). Within
   a round the per-module passes are independent given the round's
   starting state; ours run on one core. Speeds `kg check` and every
   editor answer alike, independent of the editor's reuse. Risks to
   exactness: ids and expansions minted in order (keep the order, or mint
   per module), inference reading other modules mid-iteration, the
   interner's one `Mutex`. Measure the headroom (which passes split
   cleanly, and their share) before designing.
2. **The tail rounds' floor** (above): the symbol maps merged
   incrementally rather than rebuilt (`loop-across-edits.md` §6 step 4,
   which then pays in a batch run too: ~17% of `array.k`'s loop), and
   deduction visiting only what a round changed.
3. **The editor's reuse** (`loop-across-edits.md` §8): re-askable reads
   for the library's passes; reuse inside the edited module, whose share
   of `risc-v.k`'s loop is the 36% above.
4. **Outside the loop**: the CST passes and the copies' check, 0.04–0.24
   s (12% of `risc-v.k`'s `kg check`).

The backend: numbers only for now. A whole build is 92–98% backend, so
its time is the user's; if the owner later takes it up, the first
readings would be its two dead-code passes' nondeterministic cost and
`risc-v.k`, which it cannot build inside 8 GB.
