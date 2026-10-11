# The loop across edits (sprint W7.5d)

*Part of the [plans](README.md). Drafted 2026-09-27; **reviewed by the
owner 2026-10-10: run §5's measurement, then follow its rule** — build D1
in §6's order if it keeps most of the library's work on most edits, drop
it with the number recorded if not. D2 is dropped (`sprint-parity.md` §8).
**Measured the same day (§5, "Measured 2026-10-10"): D1 pays, on one
condition — positions in the edited file carried through the edit. It is
to be built in §6's order; step 1's design is written there. Step 1 built
the same day (§6, "Built 2026-10-10"): the snapshot and its shadow, no
reuse; the memory it holds accepted by the owner (§7). Steps 2 and 3
built 2026-10-10/11, step 4 planned (§6). §8 (2026-10-11): what carrying
the library can and cannot win, the levers after it, and the salsa
question, with the owner's open questions.***

## 1. The question

The keystroke is the program-wide loop. After W7.2–W7.4, an edit
recomputes only the modules it touched *before* the loop: the lowering
(W7.3) and base inference (W7.4) cut off at the module. But `Db::looped`
is keyed on the program, the program is rebuilt on any item tree's
pointer, and the loop — passes 1–7 and every round of pass 8 over every
module — runs again from nothing. `risc-v.k`'s keystroke is 1.7 s,
`array.k`'s 0.38 s (`cargo xtask bench --filter phases`, the W7.4 agent's
reading, 2026-09-27, a loaded machine); the target in `ir-elaboration.md`
§3 was ≤ 400 ms after M5 and ≤ 100 ms after M6.

`ir-elaboration.md` §3.2 imagined a module's round as a function of its
own IR, the registry entries it is home to and its imports' interfaces,
skipped when those are unchanged. W7.5 measured that premise before
designing on it.

## 2. What the measurements say

- **a. Skipping modules that gained nothing is not the lever.** Idle
  visits are ~70% of visits and 5% of `risc-v.k`'s loop, 10% of
  `array.k`'s (`KG_LOOP_PROFILE=1`). A round's floor is the whole-program
  symbol maps (17%), inference (29%) and the prefix, and the visits to
  modules that *did* grow — walked whole for the few nodes they gained.
- **b. There are no extra rounds.** The reference takes the same number
  on every root, 321/321 (`loop-left`).
- **c. A library module's state depends on the root, through dating.**
  The parts a library module writes on its own agree across roots (35 of
  36, 37 of 38, 35 of 36 modules), but copies two roots both asked for
  were decided in different rounds (57 of 90, 4 of 13, 0 of 9), and the
  round is observable: `ExpansionData::round` orders copies and dates the
  mentions inside them. So "a memo per module", keyed on the module,
  cannot be exact.
- **W7.4.** A library body edit re-infers no importer's base table any
  more (`risc-v.k` 4/50 → 1/50 module inferences). The technique that
  made it exact: record every question the computation asked across the
  module boundary and its answer, and let the memo stand while every
  answer reads the same (`kg_db::passes::types::ImportsRead`).
- **How the loop's time splits between the root and the library**
  (`KG_LOOP_PROFILE=1 kg check`, one run each, 2026-09-27, the machine not
  checked quiet):

  | | loop | per-module passes | the root's | the library's | whole-program: symbol maps, `inferType` |
  |---|---|---|---|---|---|
  | `processor/risc-v.k` (50 modules, 16 rounds) | 5.18 s | 2.78 s | 1.37 s | 1.42 s (27%) | 0.86 s, 1.53 s |
  | `data/array.k` (39 modules, 31 rounds) | 2.80 s | 1.59 s | 0.27 s | 1.32 s (47%) | 0.61 s, 0.61 s |

  An edit to the root leaves the library's own work to be redone only
  where the root's requests into it changed. Its per-module passes are a
  quarter to a half of the loop, and its share of inference and of the
  symbol maps comes on top. That is the prize of any design below, and
  it is not the whole keystroke: the root's own rounds remain.

## 3. The dependency, stated

In round *r*, what the loop does to a library module *M* is a function
of:

1. *M*'s state at the end of round *r − 1* — its arena, its types, its
   inference units;
2. the copies the census decided *into M* in round *r* — each `(template,
   arguments)`, its expansion, the site that asked;
3. every answer *M*'s passes read from outside *M* in round *r*: symbol
   lookups that resolved into another module (the maps are program-wide,
   `Template.hs:84`), types of other modules' nodes the census and
   inference read, and the program-wide fixpoint's round signature.

(1) and (2) are what 5c measured apart; (3) is what W7.4 recorded for base
inference. If all three are equal between the old run and the new one,
*M*'s round-*r* state is equal too — by determinism, which `infer-identity`
and `edit-identity` already hold the loop to.

## 4. Three designs

### D1 — replay per module and round, keyed on what it read *(to build: §5, measured 2026-10-10)*

Keep, for the last run of a root set, each module's state after each
round, and the round's inputs (2) and (3) as the module read them. On the
next run, a round of a module whose state (1) is the kept one and whose
(2) and (3) answer the same is not run: its kept state is restored. The
first round in which any of the three differs, that module runs from its
kept state of the round before, and every later round of it runs.

- **Exact by construction**, the same way W7.3 and W7.4 are: a key is
  what the computation read, not what a design believes it reads.
- **Dating is an input, not a problem.** A copy decided into *M* in a
  different round is a different (2) in both rounds, and *M* runs from
  there.
- **Costs.** State per module per round: the arena is append-only, so a
  round's arena is a length; the types and the units are not (inference
  and `evaluateConstexpr` rewrite in place), so a round needs a diff of
  the `TypeMap` and of the units. Recording (3) needs the lookups made
  through one door, which the symbol maps and the census do not have
  today — W7.4's `ImportsRead` is the model and the largest piece of work.
  The whole-program symbol maps are rebuilt whole each round (17% of the
  loop) and would need building per module and merging, or D1 saves the
  per-module passes and not the maps.
- **Positions** (found by §5's measurement): a library module's state
  holds positions in the root — the sites and argument subtrees of the
  copies the root asks for — so an edit that only moves the root's text
  changes the module's inputs as written. D1 compares and restores a kept
  round with its positions in the edited file carried through the edit,
  or it keeps little of a root edit on the largest programs.
- **Gate.** `edit-identity` (every root, every edit kind, and a new kind:
  a body edit that changes the root's requests), with mutants: a lookup
  left out of (3); a copy's round left out of (2); a module's kept state
  restored one round early.

### D2 — carry the whole program and treat the edit as a pass

5c's second suggestion: keep one root set's whole `Program` and continue
its fixpoint with the edited root substituted. **Not exact in general**:
copies the old root asked for and the new one does not stay; copies the
new one asks for are dated after every old one; the rounds' order, which
`ptdiff` and `output` see, differs from a run from nothing. It could be
made exact only for edits that provably change no request and no
lookup's answer — which is D1's key, checked for the whole program at
once, with less saved when it fails. Not recommended.

### D3 — copies keyed by content

`ir-elaboration.md` §3.5: a copy is made and typed once per `(template's
declaration fingerprint, arguments, the environment its body reads)`,
across runs; the rounds still run. It saves the copying and the typing of
copies — `instantiate` is 0.59 s of `risc-v.k`'s 5.18 s — and none of the
walks, maps or passes over grown modules. Cheap relative to D1, and
compatible with it; worth doing only if §5 finds the copies' own cost
large.

## 5. Before building: one measurement

D1's saving is bounded by the library's share of a *keystroke*, which is
not the batch run above: the editor stops at `INTERACTIVE`'s round cap,
and a root edit may change the root's requests in round 0, sending the
library down D1's slow path at once. So:

- **5d-0.** Over `edit-identity`'s edits (body, signature, import, library
  body — every root), count for each edit the first round in which each
  library module's (2) differs between the old run and the new. The
  number is the fraction of the library's per-round work D1 would keep.
  `cargo xtask loop-modules` (5c) already compares per-module fingerprints
  between two programs; it extends to "before and after an edit" and
  "per round".

If D1 would keep most of the library's work on most edits, build D1 in
the order below. If not — if a typical edit changes the root's requests
in round 0 — the keystroke's lever is not across edits at all: it is
W5.3–W5.4's work inside a run (the algebras, the symbol maps built per
module) and the root's own rounds, and D1 is dropped with the number
recorded.

### Measured 2026-10-10

`cargo xtask loop-rounds` (`xtask/src/loop_rounds.rs`; one row per edit
in `target/loop-rounds.tsv`) runs `edit-identity`'s edits on its roots —
348 roots, 1123 edits: body 304, signature 304, import 271, library body
244 — the loop to its fixpoint before and after each, and finds for each
library module (every module but the edited one) the first round whose
copies (2) differ. A copy is spelled as `instantiate` reads it: template,
argument key, stamps, and the occurrence it is made from — module,
position, expansion, and the argument subtree with every node's position,
provenance, kind, expansion and type; an expansion as its frames
(instance, site, round), a file as its path. Time is
`kg_sem::profile::record`'s per-module clock in the run after the edit:
each pass's visit to the module, inference's walk of it included. A
module's passes before the loop count as kept unless it is new; the round
it first differs in counts as lost whole, though its passes before the
census ran on agreeing inputs.

Four readings of "differs": **as written**; **relocated**, positions in
the edited file carried through the edit first; relocated with the
module's own mentions added (what `resolve` renames there — within 1.3
points of relocated everywhere, left out below); and **state checked**:
relocated, and a round kept only while the module's whole state (every
node, its provenance and type, positions relocated) at each census before
it, and at the end, is the old run's — which sees what (3) did to the
state, though not what it read.

The library's work kept, time-weighted and pooled over the corpus
(module-rounds in brackets):

| edit | as written | relocated | state checked | state checked, rounds 0–3 |
|---|---:|---:|---:|---:|
| body | 50.6% (82.6%) | 99.8% (99.9%) | 99.8% (99.9%) | 99.9% |
| signature | 53.4% (82.2%) | 93.5% (97.0%) | 92.1% (96.6%) | 97.2% |
| import | 54.0% (74.7%) | 82.2% (85.7%) | 80.5% (85.5%) | 91.5% |
| library body | 86.2% (93.6%) | 100% (100%) | 100% (100%) | 100% |
| all | 61.8% (84.2%) | 95.5% (96.8%) | 94.9% (96.7%) | 97.7% |

- **Per edit**, state checked: the median edit keeps all of the library's
  work for every kind but import (93.2%); quartiles 100 / 100 / 100% over
  all edits; 99.5% of edits keep more than half (rounds 0–3: 99.9%). As
  written the median is 95.8% and 91.2% keep more than half — but the
  pooled figure is 61.8%, because the programs whose keystroke is slowest
  keep least: the body edit keeps 9% of `risc-v.k`'s library work as
  written and 100% relocated, `array.k` 8% and 100%, `hash_table.k` 13%
  and 100%.
- **Why**: a root edit moves the sites and the argument subtrees of every
  copy the root asks for after it, and those positions are in the library
  modules' state (the copies' nodes carry their `Src`). The root's requests
  already differ in round 0 on 44% of edits as written, 9.8% relocated —
  where relocated and "what they ask for" (template and key, positions
  dropped) agree exactly: body 0.7%, signature 3.3%, import 36%, library
  body 0%. A typical edit moves the root's requests; it does not change
  them.
- **The share of the loop**, after an edit: the library's per-module
  passes 62.5%, the edited module's 11.3%, the whole-program steps (the
  symbol maps, inference's bookkeeping, the census's union) 26.2%. So D1
  would keep 59.3% of the loop pooled, state checked (relocated 59.7%, as
  written 38.6%); on `risc-v.k`'s root edits the library is 41%, the root
  36% and the whole-program steps 22% of a 1.8 s loop.
- **Reads across the boundary, (3)**, are not measured as reads. The
  state-checked reading bounds what they did: on 44 edits (30 signature,
  14 import, no body or library-body edit) a module's state parts before
  its copies do, 0.6 points pooled. Most are `library/test/unit.k` (22):
  `fixture<TestN>` calls the root class's `run()`, whose signature the
  edit changed, through the symbol map — the class's type holds its
  fields, not its methods, so the copy's key does not move. The other
  check agrees: of the library modules a reading keeps whole, those that
  end as they ended before are 35367 of 35402 relocated (99.3% of their
  time), 32043 of 32193 as written.
- **The instrument**: each root measured twice without an edit keeps
  every library round, and every module's state agrees at every census,
  on 348/348 roots — nothing a run mints is spelled. Two full runs gave
  identical module-round counts and time-weighted fractions within 3
  points (as written 63.0 → 61.8%, relocated 95.3 → 95.5%); 16 cores at
  load 12–17, six workers, 335 s, 4.8 GB peak.

What the number does not cover: (3) as read — a round whose reads across
the boundary changed but whose state did not is kept here, and D1 keyed on
its reads (§6 step 2) runs it again, so state checked is still an upper
bound; the whole-program steps (26% of the loop), which D1 keeps only
with §6 step 4, inference's bookkeeping being whole-program as well; the
edited module's own rounds; the keystroke outside the loop (the lowering,
the CST passes, the copies' check); and edits other than these four
kinds at these places — one that rewrites a request (`Foo<8>` to
`Foo<9>`) parts the module that holds `Foo` in the round it is decided
(`loop_rounds` tests), and how often an editor's edits do that is not
measured.

**By §5's rule, D1 is built.** It keeps most of the library's work on
most edits — 94.9% of the library's time pooled, all of it on the median
edit, more than half on 99.5% of edits, 97.7% within the editor's first
four rounds — **provided positions are relocated**. As written it keeps
61.8% pooled and almost nothing of a root edit on the largest programs,
so relocation is part of step 1's design, not a later optimisation.

## 6. If D1: the order

1. The per-module, per-round snapshot, with the loop run from nothing
   and every snapshot checked against the next run's actual state (a
   shadow, like `infer-identity`'s) — no reuse yet.

   **The design, from 5d-0's measurement (2026-10-10):**
   - *What is kept*, per root set, from its last run: the final
     `Program` — every module's arena and `TypeMap`, and the
     `ExpansionInterner` — instead of freed; and per module and round
     the arena's length at the round's end and the round's undo log —
     each `ExpArena::node_mut` write's old node (the arena already logs
     the index: `writes()`) and each `TypeMap` write's old entry (the
     map's `journal`, which `begin` already records). The state after
     round *r* is the final arena cut to its length then, with the later
     rounds' logs undone newest first: nothing is copied per round.
   - *The key* per module and round: (2) as 5d-0 spells it
     (`loop_rounds::Speller::round`: the copies decided into the module —
     template, key, stamps, and the occurrence's position, expansion and
     argument subtree), hashed with its positions held aside as 5d-0's
     `Final` holds them, so that they can be carried through an edit.
     (3) joins it in step 2.
   - *Where it is taken*: in `kg_sem::elaborate_opts` — the key after
     each round's census (where 5d-0 observes, `profile::census`); the
     length, and the closing of the round's logs, after its `instantiate`
     and inference; the whole kept when the loop returns. The loop's
     output is unchanged; the snapshot is a second output, kept by the
     `Db` beside `Looped`.
   - *Relocation*: a kept round is compared, and later restored, only
     with its positions in the edited file carried through the edit —
     node heads (the root's arguments carried into a library copy, its
     occurrences there), expansion frames' sites, the key's held-aside
     positions. A position inside the changed text has no place, and its
     key fails. The map is the editor's change, or `kg_db::text_map`'s
     line diff; `xtask::edits::Splice::{start, end}` is the rule for one
     splice (a range ending where text is inserted stays, one starting
     there moves). Relocation assumes the passes compare positions only
     for equality and order within a file, which a monotone map keeps;
     5d-0's end-state check found nothing against it (every module kept
     whole relocated on a body or library-body edit ends as before,
     20016 of 20016).
   - *The shadow*, this step's gate: each run builds the snapshot, and
     the next run, still from nothing, compares every module's restored
     state at every round, relocated, with its actual one — 5d-0's state
     hash (`loop_rounds::Speller::final_state`), which `loop-rounds`
     already runs corpus-wide (348/348 roots measured twice agree in
     every module at every census).
   - *Memory*, from 5d-0's sizes after each edit at the loop's end:
     nodes median 61k, p90 116k, max 1.71M (`risc-v.k`) — at 112 bytes a
     node 7, 13 and 192 MB, and the live program around three times its
     nodes (`array.k`: 60 MB of nodes, 210 MB live; journal, "The loop's
     floor"). The logs after round 0 are small beside it: writes in place
     median 70, max 14.7k (1.6 MB at 112 bytes); type changes median
     2.7k, max 612k (~10 MB at 16 bytes). So D1 holds one loop's program
     per root set — on `risc-v.k` about what `kg check` already peaks at,
     0.68 GB — and two while a run that restores from it may still be
     cancelled.

   **Built 2026-10-10** (`kg_sem::snapshot`, `kg_db::snapshot`; the
   shadow in `cargo xtask edit-identity`, its ledger `shadow`). As
   designed, with what the build found:
   - *Kept* under the editor's budget only. The run's `Elaborated` is
     shared between its `Looped` answer and the `Db`'s snapshot of the
     root set (`Kept`; eight root sets, the `Looped` memo's bound), which
     outlives the answer's memo until the next run of the root set
     reaches its own fixpoint, with each module's parse and sorted
     lowering. A batch compile records and keeps nothing. The logs start
     at round 1 (no state before round 0's end is restored), hold only
     nodes that existed when the round began (one it appended is cut,
     not undone), and a type only where a write changes it strictly
     (`Ord`: `==` ignores a parameter's name).
   - *The key* holds the copies decided into the module that round **and
     the module's own mentions** (what `resolve` renames there, 5d-0's
     third reading), and a copy's site where its frames carry another
     than the occurrence's head — 5d-0's spelling (symbols left numbered:
     both runs intern into the database's one interner), moved into
     `kg_sem::snapshot` and used by `loop-rounds` too.
   - *Followed*: a module whose sorted lowering is the kept run's (the
     lowering memo's pointer), through every round whose key agreed —
     never the edited module.
   - *Relocation*: the one splice between the kept text and the new
     (`Splice::between`), covering both places an insertion beside a copy
     of itself could have been made; 5d-0's rule at its edges, a guess —
     a comment typed before a declaration is the declaration's — that
     costs reuse, not exactness, wherever a module's positions in the
     edited file are its key's. They are not always: below.
   - *The shadow* compares node by node, not by 5d-0's hash (which took
     `risc-v.k`'s run from 11 s to 158 s): at each round's end what
     either run changed that round, once the round before was equal;
     round 0, a round after an unequal one, and the end of both runs
     whole, the end with no log on either side.

   The shadow over every root (`cargo xtask edit-identity`, 348 roots,
   2246 steps after an edit — each edit and each undo; 134 s, peak
   footprint 7.5 GB against 7.3 GB before):

   | edit | steps kept whole | module-rounds whose key agreed | of those restored equal |
   |---|---:|---:|---:|
   | body | 604/608 | 203060/203230 (99.9%) | all |
   | signature | 500/608 | 194690/202001 (96.4%) | 193984 (706 not) |
   | import | 216/542 | 100086/151347 (66.1%) | 99866 (220 not) |
   | library body | 488/488 | 194442/194442 (100%) | all |
   | all | 1808/2246 | 692278/751020 (92.2%) | 691352 |

   Every unequal round is a signature or import edit and its undo — 46
   edits, 94 modules — and each a read across the module boundary that
   no key holds yet, (3): `library/test/unit.k` 48 times
   (`fixture<TestN>` calling the edited `run()`, as 5d-0 found),
   `closure.k` 18 and six modules more under a removed import,
   `runner.k` 8 and four more under a signature edit — one of them
   `library/data/array.k` under an edit to `function.k`, whose state
   held, at round 0, a parameter list *of the edited file*, copied in by
   `resolvedFuncParams` (`Function.hs:133-141`,
   `ExpArena::deep_copy_from`). So a read carries
   positions too, outside any key: step 2's read door keys its answers
   with their positions held aside and relocated, as the copies' are.
   The ledger books the 94 (`unequal`, a fail set) and the 1808 steps
   (`kept`, a pass set); `fast.sh loop`'s quarter is held to the
   baseline's items for its roots. Mutants, each red on that quarter
   against the baseline it is to be blessed with: a type write left out
   of the log (1215 items), a node write left out (588), every round
   keyed by round 0's key — a copy's round left out (326), a node
   restored a round early (586), relocation skipped (314).

   **Measured** (2026-10-10; two interleaved A/B runs against `b4ee408`
   in one build directory, its sources touched and rebuilt at every turn,
   load 15–33, so upper bounds; `cargo xtask bench --filter
   keystroke-phases --recorded`, seven keystrokes, the process's peak
   footprint by `/usr/bin/time -l`; `kg check -I library -I
   test/library`):

   | | main (`b4ee408`) | step 1 (`100e619`) |
   |---|---|---|
   | `risc-v.k`, first answer | 997, 1024 ms | 1016, 954 ms |
   | `risc-v.k`, last answer | 2290, 2370 ms | 2467, 2284 ms |
   | `array.k`, first answer | 274, 274 ms | 263, 256 ms |
   | `array.k`, last answer | 1056, 1052 ms | 1103, 1004 ms |
   | the editor's peak footprint, both files | 1.36, 1.33 GB | 1.78, 1.80 GB |
   | `kg check risc-v.k` | 2.46, 2.47 s; 824, 824 MB | 2.63, 2.33 s; 841, 841 MB |
   | `kg check array.k` | 0.91, 1.09 s; 313, 312 MB | 1.56, 0.94 s; 292, 311 MB |

   The answers are main's within the noise; the editor's peak footprint
   rises 0.45 GB, the run before's program held until the next reaches
   its fixpoint, where main freed it at the next run's first answer.
   Beside the program it shares with its answer, the kept snapshot holds
   5.8 MB on `risc-v.k` (a program of 1.71M nodes; logs of 8923 nodes and
   233950 types; keys holding 77763 positions) and 2.1 MB on `array.k`
   (531k; 3669, 41294, 75109). A first A/B, before two fixes, read the
   last answer 116–135 ms later on `risc-v.k` and 60–134 on `array.k`:
   the keys took 117 ms of a 1.85 s run — half of it resolving the
   symbols of every type spelled, which a key leaves numbered now — and
   the old program was freed before the answer went out, now after it
   (`KG_LOOP_PROFILE`: `snapshot keys` 48 ms on `risc-v.k`, 40 on
   `array.k`; `snapshot ends`, the logs, 0.000 s).

   A batch compile records and keeps nothing: `kg check`'s time is
   main's, and the `kg` binary's peak footprint on `risc-v.k` 17 MB higher
   in all four runs (824 → 841 MB; `array.k` unchanged), which this step
   does not account for — its batch path is the same code but for two
   structs a field larger, mimalloc's peak pages are the same (2.1K) and
   its pages over the run more (7.8K → 10.1K) — and which `cargo xtask
   mem`'s own `check`, the same query in one process, does not show
   (below). Over stdio (`kg lsp`, a project whose manifest imports the
   reference's `library` and `test/library`, `risc-v.k` opened and six
   keystrokes, two interleaved pairs, load 18–35): first answers 0.92–1.09
   s on both, last answers 2.08–2.46 s on main and 2.18–2.46 s with the
   snapshot (the second pair 0.15 s apart), the server's peak footprint
   1.37–1.48 GB → 1.87 GB.

   `cargo xtask mem`, main's watch, which landed beside this step: two
   interleaved runs against `df4b8b5` (load 10–17; one process per case;
   peak footprint, then the resident set, which counts pages the
   allocator has purged and the kernel not yet taken back), MiB:

   | | `check` | editor open | after 5 edits | after 5 edits, resident |
   |---|---|---|---|---|
   | `risc-v.k` | 771, 770 → 772, 767 | 902, 880 → 920, 920 | 1214, 1194 → 1586, 1610 | 1475, 1371 → 2137, 2087 |
   | `array.k` | 279, 279 → 275, 275 | 319, 326 → 330, 330 | 439, 437 → 557, 540 | 475, 474 → 591, 572 |
   | `float.k` | 288, 288 → 294, 294 | 343, 342 → 344, 344 | 497, 498 → 550, 559 | 552, 550 → 607, 597 |
   | `cache.k` | 93 → 93 | 113 → 114 | 202, 201 → 222, 226 | 210, 209 → 232, 232 |
   | four lines | 37 → 37 | 41 → 41 | 71 → 79 | 76 → 83 |

   `check` is main's; the editor after five edits holds one program more
   at its peak, `risc-v.k` +0.39 GB in footprint. Its resident set, 2087–
   2137 MiB, is over its budget (1910, main's peak and a quarter): the
   accepted cost, for the lead to bless (`cargo xtask mem --bless`).
2. The read door: every cross-module lookup the rounds make through one
   recorder; a shadow check that a run's recorded answers reproduce.
   Step 1's shadow says where it must reach: the 94 modules it books are
   its first test, and a read's answer can hold positions of the edited
   file (`resolvedFuncParams`), which the door holds aside and relocates
   as the keys' are.

   **Built 2026-10-10** (`kg_sem::reads`; the reads' key beside the
   copies' in `kg_sem::snapshot`; `edit-identity`'s `shadow` ledger). A
   recorded run hands every pass that reads across a module boundary one
   `Door`, and each module's round ends with a second key, (3), beside
   the first, (2). As designed, with what the build found:
   - *Through the door*: every lookup in a symbol map with its answer
     (inference, `addLambdaWrappers`, `deduceTemplateArgs`, pass 1's
     `redundantNameScope` and `resolvedFuncParams`); every node of another
     module read, its type or its source — inference's
     `Cx::snap_{exp,ty,is_set,src}`, through which the incremental walk
     already read every other module, a wrapper's declaration and the
     parameters it copies, a deduction's template signature (the template,
     its parameters, the declaration, a function's parameters and return
     type — not its body) and a struct template's fields, an alias and the
     function type `resolvedFuncParams` copies whole; and what the program
     decides as a whole — whether any module declares an alias a parameter
     can resolve through (pass 2 resolves through one only then), the
     `extern` classes (pass 7), and each spread match `resolve` makes
     against the decisions every module seeded. A read of the reader's own
     module is not recorded.
   - *An item* is a read with its answer, spelled where it was written: a
     node by its module's name, head, expansion frames, kind and
     provenance, its expression's shape (`Exp::hash_shape`) and its
     children's identities; a type by a strict hash (`kg_ty::hash_strict`,
     a parameter's name in); an answer in the reader's own module by its
     index. Not by id: a body edit renumbers the root, whose declarations
     the library's copies read all the time.
   - *The key* is the round's set of items, each tagged with its pass and
     the pass's place in the round, every position named by its rank among
     the round's distinct positions, beside those positions in order —
     relocation keeps order within a file, so the same reads carried
     through the edit are the same key. A set: within an inference
     iteration a module reads the others as the iteration found them, so
     a read is one item however often it is made, one answered otherwise
     later a second.
   - *Outcomes, where the read is a rewrite's*: a deduction and a wrapper
     keep what they read only where they changed the module
     (`Door::tentative`, `Door::settle`), and `resolve` keeps only the
     matches it makes, by walk and mention — what left a module alike moved
     nothing, and another run that left it alike ran the same, whatever it
     read. Built first without, the door parted 1864 module-rounds on their
     reads alone, 1774 of them equal all the same; nine in ten were a later
     round's deduction of a call it had already rewritten re-reading the
     template, or `resolve` asking of a template no module decided any
     more (a removed import). With outcomes: 228, 138 of them equal.
   - *Cost*, sampled over 40 s of the recorded keystroke bench (`risc-v.k`,
     then `array.k`; main and this step each): the door's own frames 1.9%
     of the loop's samples, the passes' shares beside main's +1 point all
     told (inference 54.9 → 55.4%, the reads' keys at the rounds' ends
     0.3%, the wrappers' +0.2, the deductions' within the noise) — some
     20–40 ms of `risc-v.k`'s two-second last answer. As first built it
     was 4.9%: SipHash, every read kept, a template's signature spelled
     for every reader every pass. What brought it down: a hasher of its
     own (a folded multiply); identities numbered once a run and named in
     an item by number, their positions ranked at the round's end; a walk's
     lookups shown once, from the list the incremental walk already keeps;
     a template's signature spelled once while its module stands
     (`Door::declaration`), and only for a deduction that changed
     something; a tried rewrite's reads held in one buffer, cut off where
     it changed nothing.
   - *The shadow* follows a module through the rounds whose two keys
     agree — the run in hand read what the kept run read, positions
     carried — and where its reads alone part it, compares the round all
     the same, to count what the door's caution costs
     (`ShadowReport::read_parted_equal`).

   The shadow over every root (348 roots, 2246 steps after an edit; 140 s,
   peak footprint 7.7 GB, against 134 s and 7.5 GB):

   | edit | steps kept whole | module-rounds whose keys agreed | restored equal | parted by reads alone (equal all the same) |
   |---|---:|---:|---:|---:|
   | body | 604/608 | 203060/203230 (99.9%) | all | 0 |
   | signature | 440/608 | 193938/202001 (96.0%) | all | 80 (16) |
   | import | 214/542 | 98848/151347 (65.3%) | all | 148 (122) |
   | library body | 488/488 | 194442/194442 (100%) | all | 0 |
   | all | 1746/2246 | 690288/751020 (91.9%) | all | 228 (138) |

   No unequal round is left. Step 1 followed 692278 module-rounds, 926 of
   them unequal; step 2 follows 690288, the door's caution costing 1064
   rounds that were equal (0.14%) — none on a body or library-body edit.
   The ledger: `unequal` 94 → 0; `kept` 1808 → 1746, 62 steps, 58 of them
   steps step 1 found a module unequal in; `read_parted` and
   `read_parted_equal` new totals — for the lead to bless. Mutants, over
   half the roots against that baseline: the reads left out of what a
   round agrees on brings 48 unequal modules back, inference's reads of
   another module's nodes left out 20 — caught. Each other site left out
   alone survives: lookups, the wrappers', the deductions', pass 1's, pass
   7's, `resolve`'s, and a tried rewrite's reads all dropped — on the
   corpus every read they make that moved a module is one inference makes
   too (step 1's `array.k` case stays parted with either `resolvedFuncParams`'
   reads or inference's left out). They stay: each is a read
   the round makes, and a key may hold more than the corpus needs, never
   less. Unit tests hold the rest: a read across the boundary parts a
   module its copies do not (red with the reads unkeyed, inference's reads
   unshown, their positions not carried or not ranked); a rewrite that
   changed nothing read nothing (red with `settle` inverted or an inner
   rewrite's reads kept past an outer one); a strict hash tells a
   parameter's name.

   **Measured** (2026-10-10; two interleaved A/B rounds against main,
   `4db41f1`, in one build directory, its sources touched and rebuilt at
   every turn, load 5–16, so upper bounds; `cargo xtask bench --filter
   keystroke-phases --recorded`, seven keystrokes, the process's peak
   footprint by `/usr/bin/time -l`):

   | | main (`4db41f1`) | step 2 (`54ea317`) |
   |---|---|---|
   | `risc-v.k`, first answer | 847, 967 ms | 848, 827 ms |
   | `risc-v.k`, last answer | 2104, 2308 ms | 2134, 2024 ms |
   | `array.k`, first answer | 212, 219 ms | 229, 215 ms |
   | `array.k`, last answer | 848, 895 ms | 927, 856 ms |
   | the editor's peak footprint, both files | 1.83, 1.85 GB | 1.72, 1.83 GB |

   The answers are main's within the noise; sampled, the door is 1–2% of
   the loop (above). `cargo xtask mem`, two interleaved runs against
   `4db41f1` (`6caf002`, the door before its last two economies, which
   hold less; peak footprint, MiB): `check` 769 → 772, both runs (the
   batch loop records nothing); the editor open 883 → 879–883; after five
   edits `risc-v.k` 1459–1463 → 1357–1459, `array.k` 478–479 → 481–507,
   `float.k` 513–514 → 505–507, `cache.k` 195 → 197–198 — all fifteen
   within budget. What the kept snapshot holds beside the program grows by
   the reads' keys: their positions, median 1160 a snapshot, at most 14918
   (`risc-v.k`, 179 KB); the snapshot beside its program median 69 KB, at
   most 5.9 MB.
3. Reuse, one pass at a time, the prefix first (the largest per-module
   pass), each behind `edit-identity` and its mutants.

   **The prefix, built 2026-10-11** (`kg_sem::reuse`; the snapshot's
   prefix boundary in `kg_sem::snapshot`; `kg_db`'s `Kept::reuse`,
   `Db::{set_reuse, take_reuse_report}`; `edit-identity`'s `shadow`
   ledger). The editor reuses, by default, every prefix pass it can of
   the kept run of its root set:
   - *When*: a module followed — its lowering the kept run's — whose keys,
     copies and reads, agreed through every round before, whose reads this
     round up to the pass are the kept run's
     (`ModuleRounds::reads_before_prefix`, the door's key taken where the
     pass begins), whose arena stands at the length and root the kept
     module's did when its pass began, and whose kept pass wrote nothing
     in place. The pass reads its module alone (`round_prefix_in_place`,
     round 0's `pipeline_tail_in_place`), so its output is the kept run's.
   - *What is taken in*: the nodes the kept pass appended, as it left them
     — the final arena's, with the round's later writes undone, for which
     the node log is now split at the prefix's end — each head carried
     through the edit, each expansion found in the run's interner (the
     same instance under the same parent, at the same site carried, in the
     same round; none is interned anew), each child named as the run's
     arena names the same node, in the same chain of forks
     (`ExpArena::translate`: an id a rewrite later compares is the id it
     would have built); the root; and its types, carried as `carry_types`
     carried them, by the kept pass's `(new, original)` pairs
     (`ModuleRounds::prefix_lineage`, only the pairs that gave a type) from
     the module as it stands. Kept as types instead, they held alive every
     type inference later replaced: `risc-v.k`'s editor 45–60 MiB larger
     after five edits. Anything that does not carry, and the module runs
     the pass itself.
   - *Checked*: with `Db::set_reuse(true, true)` — `edit-identity`'s — every
     pass taken is also run on a fork and compared, node by node, children
     by the arena that mints them, source, kind and type; a difference is
     booked to the ledger (`reuse-unequal`, a fail set).

   Over every root (348 roots, 2246 steps after an edit; 128 s with every
   reused pass run again on a fork to check it; peak footprint 7.8 GB, 7.6
   before): 348 identical; the prefix reused in
   700250 of the 700348 module-rounds offered, 20.8M nodes taken in, every
   one checked and none unequal; the shadow as before (690288 of 751020
   module-rounds agreed, 0 unequal). Unit tests: a kept prefix pass is the
   pass the run would make (red with a head not carried, an expansion or a
   child named otherwise, the types not taken in, or the check blind); a
   body edit takes the followed modules' passes from the kept run (red with
   the kept run not handed to the loop, or reuse off by default).

   **What it saves: almost nothing on a root edit.** `KG_LOOP_PROFILE` on
   `risc-v.k` (`kg check`, a run from nothing): the prefix is 0.167 s of a
   1.62 s loop, **0.146 s of it the root's own** — the module the keystroke
   edits, which is never followed. The library's prefix is 21 ms, and
   taking it in costs about what it saves (sampled: the prefix 7.3% of the
   loop on main, 6.5% plus 0.7% taking in with reuse). Interleaved A/B
   against main, `3e26911` (two rounds, load 4–9, `bench --filter
   keystroke-phases --recorded`): `risc-v.k`'s last answer 2017, 2027 →
   2036, 2044 ms, its first 829, 836 → 836, 835; `array.k`'s last 853, 844
   → 851, 846, its first 216, 216 → 220, 221 — main's (an earlier pair
   against `91389e7`, load 2–9: 2027, 2018 → 2015, 2030 and 847, 845 → 841,
   846). An edit to a library module, the root followed, would take the
   root's 0.146 s from the kept run instead. `cargo xtask mem`, two
   interleaved runs against `3e26911` (peak footprint, MiB): `check` 771 →
   770; the editor open on `risc-v.k` 883 → 887; after five edits
   `risc-v.k` 1396–1398 → 1415–1423, `array.k` 482–484 → 479–484 — all
   fifteen within budget. Beside a kept program the snapshot holds at most
   6.2 MB (`risc-v.k`), 0.3 MB more than step 2's: the prefix's boundary,
   its key and the pairs that carried a type.

   **Where the keystroke is.** The same profile, the library's share of
   each per-module pass: inference's walk 0.375 of 0.650 s, `instantiate`
   0.098 of 0.158, `deduceTemplateArgs` 0.085 of 0.139,
   `addLambdaWrappers` 0.037 of 0.072, `closureCalls` 0.021 of 0.043.
   `closureCalls` reads its module alone and could be taken in as the
   prefix is, for as little. Every other reads other modules, so a later
   run cannot know a pass's inputs are the kept run's without asking the
   kept run's questions of its own state: the door's items must become
   re-askable (a lookup's question kept as a question, a node read as the
   node's place in a followed module), and a pass is reused where every
   recorded question answers as it did. Inference, the largest, reads
   across the boundary inside a program-wide fixpoint — the last to
   reuse.
4. The symbol maps per module and merged, so a kept module's entries are
   not rebuilt.

   **Planned 2026-10-11, not built** (the owner pauses after this batch).
   What it costs now, sampled over `risc-v.k`'s recorded keystrokes:
   `Program::symbols` is 6.5% of the loop, two thirds of it
   `FiledModule::refresh` — walking the nodes each module made since its
   last build — and a third assembling the program's map from every
   module's entries at every build. Two pieces, measured first and taken
   in the order they pay:
   - *Carried across runs.* Keep, per module and config, its `Filed`
     entries as of the run's first build — the module after the passes
     ahead of the loop, a function of its lowering and of reads round 0's
     key holds — and seed a later run's `Filed` with them for every
     followed module whose reads up to that build agree, ids translated as
     the prefix's are (`ExpArena::translate`). Saves the first build's walk
     of every followed module; how much of `refresh` that is, measure
     before building.
   - *Merged incrementally.* Keep, per key, the modules that file it in
     program order, and at each build redo only the keys of modules whose
     entries changed; the map handed out stays one `FxHashMap`, so a lookup
     costs what it does now. Saves the assembly at every build.
   - *Exact* by `KG_SYMBOLS_CHECK=1` (every build against the whole walk)
     over `infer-identity` and `edit-identity`, the shadow and the
     prefix's check as before; worth at most the 6.5%.

Each step exact or not landed; `infer-identity`, its cross-build dump and
full `edit-identity` at every step.

## 7. What the owner is asked

- Whether §5's measurement is worth running before D1 is designed
  further (recommended), or whether the keystroke should be taken from
  W5.3–W5.4 inside a run first. — *Answered 2026-10-10: the measurement
  first; run the same day (§5, "Measured 2026-10-10").*
- Whether D2 may be dropped. — *Answered 2026-10-10: dropped.*
- Whether the memory D1 needs is acceptable for the editor: a round's
  state per module per root set held. — *Estimated from 5d-0 (§6 step
  1): one loop's final program per root set, kept rather than freed (on
  `risc-v.k` about `kg check`'s own 0.68 GB peak; a median program tens
  of MB), the per-round logs small beside it, and two programs while a
  restoring run may be cancelled.* **Answered 2026-10-10 by the owner:
  accepted** — "The extra memory cost is acceptable. We can document that
  and keep an eye on memory usage across use cases, but if we are
  efficient with our memory usage, then duplicating it during editing
  isn't rough." Step 1 measured it (§6, "Built 2026-10-10"): beside the
  program it shares with its answer, the kept snapshot holds 5.8 MB on
  `risc-v.k` (a program of 1.71M nodes) and 2.1 MB on `array.k` (531k);
  the editor's peak footprint over both files' recorded keystrokes is
  1.78–1.80 GB against 1.33–1.36 GB without it (`kg lsp` over stdio on
  `risc-v.k`: 1.87 GB against 1.37–1.48), the run before's program held
  until the next reaches its fixpoint. The condition is efficiency:
  the program a run holds must stay lean for twice it to be cheap. A
  check tracking memory across use cases — a batch `kg check`, `kg
  build`'s frontend, the editor after keystrokes — is being added on main
  as `cargo xtask mem`, so D1's cost stays visible.

## 8. After step 3: the ceiling, the levers, and the substrate

*Discussed with the owner 2026-10-11, at the pause; nothing here is built
or decided. The recommendations are the lead's; the questions at the end
are the owner's.*

### The ceiling of carrying the library

On `risc-v.k`'s root edits the library is 41% of the loop, the edited
module 36% and the whole-program steps 22% (§5, 5d-0's split of a 1.8 s
loop). Reusing every piece of the library's work perfectly therefore takes
at most about 40% off the loop. Applied to the editor's answers today
(~0.83 s first, ~2.0 s last; §6 step 3's A/B, `bench --filter
keystroke-phases --recorded`), that is roughly 0.5 s and 1.2–1.3 s — an
estimate by arithmetic, not a measurement, and an upper bound on the
saving, since the work outside the loop does not shrink with it.

So D1 continued cannot reach `ir-elaboration.md` §3's 400 ms on the real
project by itself, and cannot reach 100 ms at all: the edited module's own
rounds and the whole-program steps stay. The bench's keystroke (379 ms,
`rooted_settings`, a smaller program) meets 400 ms; the project's does not.

### The levers, cheapest and safest first

1. **Step 4, the symbol maps** (§6). At most 6.5% of the loop; planned,
   well understood, low risk.
2. **Parallel rounds** (`sprint-parity.md` W5.6; `ir-elaboration.md` §3
   item 3). Not built: the loop runs on one core. Within a round the
   per-module passes are independent given the round's starting state,
   which would speed `kg check` and the editor alike and is independent of
   reuse. Its exactness risks: ids and expansions are minted in order, so
   parallel work must keep the order or mint per module; inference reads
   other modules mid-iteration ("as the iteration found them", §6 step
   2), so it may not split the same way; and the interner is one global
   `Mutex` (`kg-base/src/interner.rs`). Measure the headroom before
   designing.
3. **Re-askable reads** (§6 step 3, "Where the keystroke is"). The door's
   items kept as questions — a lookup as its question, a node read as a
   place in a followed module — asked again of the new run's state at the
   same round and pass, and the pass reused where every answer reads as it
   did. This is salsa's revalidation ("deep verify": re-check a memo's
   recorded dependencies, reuse it if none changed), applied to reads of a
   mutable program mid-fixpoint rather than to other memos — which is why
   it needs a design, not a step. In order: `closureCalls` (reads its
   module alone, so taken in as the prefix is), `instantiate`,
   `deduceTemplateArgs`, `addLambdaWrappers`; inference last, as it reads
   across the boundary inside a program-wide fixpoint. Asking again must
   cost much less than running the pass, or nothing is won. Highest
   payoff of the library levers, highest risk.
4. **Reuse inside the edited module.** A body edit to one function redoes
   the module's other declarations today. The only route toward 100 ms
   that stays exact, and the hardest: D1's keys at a declaration's grain.

What weighs on all four: exactness is never traded — each lands behind
`shadow`, the **full** `edit-identity` (the fuzz regression showed a
quarter is not enough) and mutant-proven tests; memory — the editor holds
two programs already (`risc-v.k` 1.37 GB after five edits), the owner's
condition is efficiency, and kept questions add to it; complexity — each
reuse layer is machinery a contributor must understand, and an engine of
layers growing past the loop it serves is the signal to revisit the design
(below) rather than add a layer; and what a user feels — hover and
completion already read the newest answer and never wait, so what is slow
is the diagnostics' refresh on the largest projects.

**Recommended order** if the owner takes this up: measure first — an
end-to-end profile of a real keystroke on `risc-v.k`'s project (today's
per-pass split is a `kg check` run from nothing, not an edit after D1) and
a check of the parallel headroom; then step 4; then a written design for
re-askable reads, reviewed before anything is built.

### The substrate: salsa or hand-rolled

`sprint-parity.md` §8 has this as the owner's, hand-rolled meanwhile.
Re-examined here because re-askable reads is where the two meet.

- **What the hand-rolled layer is.** `kg-db/src/db.rs` keeps salsa's
  contract without the crate: inputs through `&mut self`, derived queries
  through `&self`; about six memo tables; cancellation
  (`kg_base::cancel`), a loop in flight shared rather than run twice,
  bounded memos. The two memos that matter incrementally are keyed by
  what they read, with the answers (`sorted::SortedMemo`,
  `passes::types::ImportsRead`), and the read door extends the idea into
  the loop. The gates hold the keys complete on the corpus
  (`edit-identity` 348/348, `shadow` 0 unequal).
- **What salsa would give.** Dependencies recorded by the framework rather
  than keys written by hand; early cutoff and backdating; durability (the
  library rarely changes, so need not be revalidated); eviction; parallel
  snapshots; and familiarity — contributors who know rust-analyzer or ty
  know salsa, a real developer-experience argument.
- **Why it does not fit where the time is.** The loop is a whole-program
  fixpoint rewriting mutable arenas in place, and the round a copy is made
  in is observable (`ExpansionData::round`, which `ptdiff` and `output`
  see). Salsa wants pure functions over immutable values compared by
  equality. To gain inside the loop we would split it into per-module,
  per-round queries with immutable state — persistent structures or stored
  diffs, where D1 keeps undo logs — and record dependencies at the door's
  grain, tens of thousands of reads a round, too fine for salsa's
  per-edge bookkeeping, so coarsened and less precise. That is D1 rebuilt
  in salsa's vocabulary, with exactness at risk throughout.
- **What salsa would not solve.** Positions: a shifted position
  invalidates a salsa value as surely as it parted 5d-0's keys as written;
  rust-analyzer answers with position-independent ids, D1 with relocation,
  and either is work whatever the substrate. Untracked reads: a read
  outside a query (shared state, the interner) goes unrecorded in salsa as
  surely as a missing hand-written key — the class of bug moves, it does
  not go, and the gates stay needed.
- **Salsa only at the edges** (parse, item tree, program, lowering) gains
  little — those already work and are cheap — and leaves two incremental
  systems side by side. Its API has also been redesigned more than once;
  a migration pins a version.

**Recommendation: stay hand-rolled; borrow salsa's algorithms deliberately,
not its crate.** And draw the repeated pattern — a memo keyed by the reads
it recorded, which `SortedMemo`, `ImportsRead` and the door each spell
their own way — into one internal primitive that every new query follows,
documented with the salsa idea each part corresponds to. That takes most
of salsa's developer-experience benefit without the migration.

**Revisit salsa when any of these holds:**
- after parity, the owner decides the reference's round-by-round
  semantics no longer bind (below) — a demand-driven design, copies made
  and typed per `(template, arguments)` as queries (D3 grown up), is
  salsa-shaped, and there salsa would pay;
- `edit-identity` or `shadow` start finding missed inputs regularly (the
  trigger `sprint-parity.md` §8 already names; none so far);
- keeping the hand-written keys becomes the bottleneck of adding queries.

### For the owner (2026-10-11)

1. **The keystroke's target**: which program and which answer. Proposed:
   the first answer ≤ 400 ms and the last ≤ 1 s on `risc-v.k` under its
   project's settings (50 modules), measured as the editor runs it — the
   bench's smaller program meets 400 ms already.
2. **Whether the reference's round semantics bind after parity.** If they
   do, the keystroke stays on D1-style reuse with the ceiling above and
   hand-rolled is right. If an output whose copies may come in another
   order is acceptable, a demand-driven design opens — possibly on salsa —
   with far better keystroke prospects, and `ptdiff` and `output` need a
   new definition of agreeing with the reference.
3. **How much salsa's familiarity weighs** for future contributors against
   the migration's risk.
