# The backend: where a whole build goes, and the options

*Part of the [plans](README.md). **Survey, 2026-10-11 — read and measured
only; nothing changed, nothing decided.** Asked by the owner: can the
end-to-end compile be made faster by backend work, by changing the
frontend→backend API, or by the frontend easing the backend's burden; is
there reason to rewrite the backend in Rust with CIRCT bindings, or to
re-architect part of it. The owner's 2026-09-06 decision still binds — the
backend is reached through the C ABI, its middle end is not rewritten, MLIR
is not emitted directly (`roadmap.md`, "Decisions, 2026-09-06") — and on
2026-10-11 the backend was "measured only, not worked on" (`performance.md`
§13). §6 is what reopening either would take.*

Sources: four read-only audits of `third_party/kanagawa/compiler/cpp` (the optimizer
and data-flow passes; the flow, the CIRCT output and placement; the API; a
rewrite's feasibility, with the Rust/MLIR ecosystem), each claim cited
`file:line`, the consequential ones re-read by the lead; and `sample`
profiles of the backend worker under `kg build` (`-O 2`, 3000 placement
iterations), raw in
[`measurements/2026-10-11/backend-profile/`](../measurements/2026-10-11/backend-profile/).
Paths below are `third_party/kanagawa/compiler/cpp/` unless given.

## 1. The picture

- **A whole build is the backend's**: 92–98% of `kanagawa`'s time
  (`performance.md` §13), and the same whichever frontend feeds it — stage
  for stage ours and the reference's agree, but for two passes whose cost
  is order noise (§2.2).
- **It runs on one thread**, but for two places: the early per-function
  optimisation (`optimize.cpp:5779`) and `LocalDataPropagation`'s
  branch-and-bound (`branch_and_bound.cpp:241`). The MLIR context is
  created with threading disabled (`compiler.cpp:34`). The main thread holds
  85–95% of the samples on `cache.k`, `bits.k` and `array.k`.
- **The API is not where the time goes.** What the backend does with what
  the frontend sends — its type check, the function instances, the parse
  tree to IR — is 0.06–1.03 s, 0.6–10% of the backend. The rest is its
  middle end and output, working on its own IR, which nothing the frontend
  sends can reach without changing the C++.
- **Each program's time is in a different pass** (share of the backend's
  samples, all threads but the idle):

| Program | backend | where | |
|---|---:|---|---|
| `cache.k` | 6–9.6 s | dead-code liveness (`RemoveDeadOperationsImpl`) 78% — `RemoveUnusedGlobals` 47%, `RemoveDeadOperations` 31%; output (`CompileVerilog`) 9% | order noise, §2.2 |
| `bits.k` | 4.8 s | `ScheduleBasicBlock` 44%; liveness 25%; `BitConstantPropagation` 9% | |
| `array.k` | 10.4 s | liveness 25%; `GlobalDataPropagation` 20%; `BitConstantPropagation` 18%; IR generation 5%; output 7% | GDP does nothing, §2.1 |
| `float.k` | 14.6 s | `LocalDataPropagation`'s branch-and-bound, ~50 s of CPU on the thread pool in ~8 s while the main thread waits; output 2.9 s | |
| `risc-v.k` | does not finish in 8 GB | from 60 s to at least 200 s entirely `BitConstantPropagation`'s data flow; the worker at 2.5 GB after 10 s, 8.9 GB after 60, 10.5 GB from 100 s on, flat | the reference: 2629 s, 13.4 GB (`performance.md` §10) |

- **Allocation is 10–17% of samples**, but not a lever: the worker with
  jemalloc preloaded ran `bits.k` 4% faster and `array.k` 6% slower
  (two interleaved runs each, `backend-profile/allocator/`).
- **Our backend build runs the consistency checks**:
  `KANAGAWA_SKIP_CONSISTENCY_CHECKS` is defined only for a `release` or
  `nightly` `BUILD_CHANNEL` (`CMakeLists.txt:74-76`), and
  `kanagawa-build`'s is empty — so every build timed, ours and the
  reference's, prints the design's MLIR and re-parses it
  (`VerifyRoundTrip`, `verilog.cpp:7481`: 1.4–2.2% of the main thread) and
  checks every operation's location after every global optimisation
  (`ir.cpp:14159`). A released compiler skips both.

## 2. What the audits found that matters most

1. **`GlobalDataPropagation` does nothing under the default options.**
   Its cost test is `registerBits × _memToRegThreshold <= memoryBits →
   skip` (`lower.cpp:3011`), and the threshold's default is `0.0`
   (`compiler/hs/app/Options/CmdArgs.hs:34`; ours
   `kg-backend-sys/src/options.rs:158`), so every candidate is skipped; what
   follows the loop is guarded by flags only the loop body sets
   (`lower.cpp:3017-3107`). The analysis before it —
   `FindGlobalDataPropagationCandidates` → `GetDefinitionsRecursive`, a
   depth-first enumeration of backward paths, exponential in sequential
   branches (`lower.cpp:2162`) — is the 2.4 s, 20% of `array.k`'s backend.
   Upstream exercises the pass only through its `MEM2REG` suite,
   `--mem-to-reg=100000` (`test/logic/CMakeLists.txt:183`). A return when
   the threshold is not positive is exact.
2. **The two noisy passes visit blocks in heap-address order.** The
   liveness work list is a `std::set<BasicBlock*>` popped from its lowest
   address (`data_flow_work_list.h:28`, `.cpp:20-26`); when addresses
   happen to rise in program order a backward analysis meets each block
   before its successors and revisits it. Both passes built on it move by
   the same factor between runs (2.1–2.8×) while every other pass holds —
   `RemoveUnusedGlobals` 1.7–4.8 s and `RemoveDeadOperations` 0.9–2.5 s on
   `cache.k`, on both frontends. The result does not depend on the order
   (a liveness fixpoint is unique); the cost does. Popping in a fixed order
   (post-order for the backward analyses) is exact and takes the slow
   draw away.
3. **`RemoveUnusedGlobals` is whole-program and repeated.** Each call runs
   liveness over every function to a fixpoint of the live globals — at
   least two analysis rounds and an apply round, each rebuilding every
   function's control-flow graph, whose constructor copies each edge's
   renaming maps (`optimize.cpp:861-987`, `control_flow_graph.cpp:118`) —
   and the optimizer calls it at the start of every outer iteration of each
   of its five phases (`optimize.cpp:5577`), including the confirming sweep
   that changes nothing.
4. **`BitConstantPropagation` keeps bit facts in maps of maps.** A
   `std::map<size_t,bool>` per register per block, copied and intersected
   at every visit; the whole `Operation` copied per operation per visit
   (`optimize_lut.cpp:2110, 2291`; `ir.h:1241`). It is `array.k`'s 2.1 s
   and all of `risc-v.k`'s time from its first minute — the likeliest
   cause of the blow-up, measured by where the worker is while its memory
   grows; a second candidate the audit read is `CombineLuts` allocating
   three LUT tables per attempt, kept for the module's life even when the
   attempt fails (`optimize_lut.cpp:1194-1209`, `compiler.h:337`).
5. **`LocalDataPropagation`'s branch-and-bound copies.** A work list of up
   to 2^20 entries, each a vector over all the block's parameters, copied
   on every iteration; the bound re-summed from scratch; a new thread pool
   per subproblem (`branch_and_bound.cpp:137, 241`;
   `local_data_propagation.cpp:166, 302, 721`) — `float.k`'s 8 s.
6. **`ScheduleBasicBlock` is about N² log N a block**: a full reschedule
   and score per operation, with node-keyed `std::map` lookups
   (`schedule.cpp:213, 921, 2194-2380`) — `bits.k`'s 2.1 s.
7. **The default module is compiled and discarded.** In SV mode the
   backend runs the whole of IR generation and optimisation for the
   default pass and writes nothing (`kanagawa.cpp:137-141`); its share is
   unmeasured (the timers sum by name), probably small for the corpus's
   one-export programs.

## 3. The options, by what they touch

**No code at all.**

| Option | Effect | Output | Note |
|---|---|---|---|
| A whole-run cache for `kg build` — `kg_ptbridge::cache`, keyed by the recording and the library's content | an unchanged program rebuilds in its frontend's time: 0.05–2 s for 5–15 s | identical | the one large lever on our side; a moved location misses, locations reaching the output |
| The backend built as a `release` channel | drops the consistency checks: ~2–4% and the round-trip's second copy of the design | identical | both sides share the library: rebuild both |
| Independent builds in parallel (several roots or targets) | wall time ≈ the longest | identical | memory-bound: GBs a build |
| The backend in the background for the editor | backend verdicts in the editor, filling the cache | identical | a capability, not a speed-up: latency is a full backend run |
| A faster allocator | none measured | — | jemalloc −4% / +6% |
| `-O3`, ThinLTO for the library | 5–15%, unmeasured | identical (no fast-math) | |

**Small, exact C++ changes** (each upstreamable; each proved by the RTL of
every corpus program, patched against unpatched):

| | Change | Expected | Effort |
|---|---|---|---|
| a | `GlobalDataPropagation` returns when the threshold is not positive (§2.1) | `array.k` −2.4 s (~20% of its backend) | one line |
| b | the liveness work lists popped in a fixed order (§2.2) | the two noisy passes at or under their best draw: `cache.k` 7.2 → ≤ 2.5 s for the pair; stable timings | small |
| c | skip the global passes at a phase's start when the last phase's confirming sweep left them nothing (§2.3) | ~40% of `RemoveUnusedGlobals` calls | small |
| d | `CombineLuts` allocates permanently only on success (§2.4) | memory; possibly GBs on `risc-v.k` | small |
| e | one thread pool for branch-and-bound (§2.5) | unknown | small |
| f | MLIR locations memoized per (file, line, column, call stack) (`circt_util.cpp:87-157`) | 5–15% of building the MLIR | small |
| g | the pass manager's verifier off outside a debug build (`circt_util.cpp:2885`) | 5–15% of output | trivial |

**Algorithmic rewrites inside the C++** (exact with care; weeks):
`RemoveUnusedGlobals` incremental — control-flow graphs built once, only
functions writing a newly live global re-analysed, analysis split from
apply (2–4× on the pass); `BitConstantPropagation` on known/value bitmasks
per register, no `Operation` copies (`array.k` 2.1 → ~0.5 s; the
`risc-v.k` candidate); branch-and-bound without copies, bounds kept
incrementally (`float.k` 8 → ~1–2 s, and its memory); the scheduler on
vectors, blocks in parallel (`bits.k` 2.1 → ~0.5 s); the optimizer driver
re-sweeping only the functions a change touched (25–35% of the
per-function passes); MLIR threading on (`compiler.cpp:34`; 20–40% of the
CIRCT share, with MLIR's own ordering kept).

**Re-architecture inside the C++**: dense per-function register numbering
with bit-set liveness; a persistent control-flow graph; functions and
modules optimised in parallel. Broad gains; blocked in part by design, not
language — a global `g_compiler` read everywhere (298 uses in
`parse_tree.cpp`, 167 in `ir.cpp`).

**A different API.** Least invasive first: sending less (unreachable
functions and globals pruned before the walk) saves tens of milliseconds,
loses backend type errors in unreachable code and may move register
numbering — not recommended; a batched or file entry point saves only
foreign-call overhead (our `Recording` already is the serialized form);
handing over facts the backend recomputes (names, types, instances) is at
most 0.1–1 s against a deep change to its input contract; per-function
incremental backend caching cannot be had without forking it — the middle
end is whole-module and rebuilt after every `Reset`.

**What the frontend can do.** Little on the costs that matter: our tree is
the reference's call for call (`ptdiff`), and the work the passes remove or
propagate comes from the library's semantics and the backend's own IR
generation, not from anything the frontend could leave out without
breaking that identity. Two exceptions, both owner decisions: coarser
source locations (each operation carries a call stack of up to 64 frames
into the MLIR, its verification and the output's comments — cheaper, but
the comments would no longer match the reference's, §8 of
`../history/sprint-parity.md`'s source-locations bar); and process-level choices —
the cache, parallel builds, a background backend. (Freeing the frontend's
memory before `Codegen` does not bear on `risc-v.k`: our backend runs in
its own process.)

**A rewrite in Rust.** Not recommended now.

- *Speed*: almost nothing from the language. The hot spots are algorithms
  (§2), fixable in C++; the CIRCT half would be the same CIRCT whoever
  calls it.
- *Cost*: about 45–65 person-months for parity-exact work — the backend is
  ~90k lines, eleven times the 8.4k-line Haskell frontend this project
  replaced, and graded by the byte rather than by the verdict.
- *Bindings*: Rust's MLIR bindings are alpha (melior 0.28, by its own
  account not type-safe; mlir-sys tracks LLVM releases where CIRCT pins
  trunk), and CIRCT has no Rust bindings beyond a 2024 stub — a C++ shim
  with a narrow C API would be needed, as CIRCT's C API covers HW, ESI and
  a little of Seq and SV but not the typed builders, the folds at creation
  or the Kanagawa dialect's types.
- *Identity hazards*: ExportVerilog's bytes follow op creation order and
  names; placement draws from `std::uniform_real_distribution<float>`,
  whose algorithm differs between C++ libraries, and its positions order
  FIFO sources and return routing in the RTL (`place.cpp:124-126`;
  `lower.cpp:3648, 5679, 7009`).
- *Upstream*: active where a port would hurt most — the CIRCT pin moved
  four times in thirteen months, and `verilog.cpp` and `circt_util.cpp`
  carry nearly all of the backend's churn since 2025-10 — and frozen where
  it would not: `lower.cpp` 27 lines, `ir.cpp` 9, `optimize.cpp` 3 in the
  past year. Our reference (`98a2d8c`) is already three backend commits
  behind upstream, one of which changes the generated SV.

A rewrite becomes worth reconsidering if upstream goes quiet or the
product stops tracking it; if the parity bar moves from bytes to semantic
or simulation equivalence; if a product needs an in-process, incremental
backend (timing or resources in the editor) that the global-state design
cannot give; or if Rust's CIRCT bindings mature. Even then: the middle end
first (frozen upstream), the CIRCT output last.

## 4. What it could add up to

Estimates by the audits, not measurements; each must be measured as it
lands. With a–c and the `BitConstantPropagation`, branch-and-bound and
scheduler rewrites, a whole build would go roughly: `array.k` 11.4 → 6–7 s;
`cache.k` 6–10 → 3–4 s; `bits.k` 5.1 → ~3.5 s; `float.k` 15.4 → 8–9 s; and
`risc-v.k` from not finishing to plausibly finishing, if
`BitConstantPropagation` is its cause. The cache makes any unchanged
rebuild the frontend's time alone.

## 5. Proving exactness

Any C++ change needs a gate the corpus does not have yet: every corpus
program's generated RTL (and backend verdict) from the patched library
against the unpatched one, byte for byte — the `output` gate's machinery
pointed at two libraries rather than at the reference. Exact changes keep
`output` and `backend` as they are; the reference compiler links the same
library, so a patched library must be built for both sides, or kept to our
side with the reference's recorded outputs as the oracle.

## 6. For the maintainer

1. **Is the backend in scope?** Today it is "measured only". Taking it up
   means C++ changes, best upstreamed to `microsoft/kanagawa`, otherwise a
   local patch set on the submodule's backend that `scripts/build-backend.sh` builds, rebased as upstream moves.
2. **If so, in this order** (lead's recommendation): the whole-run cache in
   `kg build` (no backend change); the `release` channel (*done
   2026-10-11: `scripts/build-backend.sh`'s default, §7*); a–c with the
   patched-against-unpatched gate (§5); then `BitConstantPropagation`
   (which may also make `risc-v.k` buildable), branch-and-bound and the
   scheduler; MLIR threading after.
3. **Moving the reference pin** to upstream's current backend first (three
   commits, one changing the SV), so fixes are upstreamed against what
   upstream has.
4. **No Rust rewrite** of the backend; the conditions above to revisit.
5. **Source locations**: coarser locations would ease the output stage but
   change the RTL's comments — part of the source-locations decision
   already open.

## 7. Later the same day: the release channel, measured

`scripts/build-backend.sh` builds the library as upstream ships it
(`release`: no consistency checks). Against it, with everything else the
same sources:

- **`risc-v.k` goes through.** The reference, recording it for the gates,
  finished in 219 s with `Codegen` succeeding — 2,629 s and 13.4 GB under
  the local channel (`performance.md` §10). Ours: 323 s and ~10 GB on one
  run; over 20 minutes on the next, where a 5 s sample put 100% of the
  time in `RemoveUnusedGlobals` → `RemoveDeadOperationsImpl`
  (`measurements/2026-10-11/backend-profile/` holds the earlier profiles).
  So on the largest program the heap-address worklist (§2.2) is not noise
  but the difference between minutes and tens of minutes: option **b**
  (§3) is the one that would make `risc-v.k` reliably buildable — still
  over the gates' 8 GB.
- **The reference compiles three more corpus programs** (`syntax::export-class`
  1, 2, 24): the `output` gate's "theirs failed" fell from 8 to 5, and
  both sides agree on all three.
- The consistency checks' own share on the smaller programs was a few
  percent (§1); on `risc-v.k` the local channel's 2,629 s against ~4–5
  minutes says they, or their interaction with the address-ordered passes,
  dominated there — not separated.
