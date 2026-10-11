# Handoff — paused 2026-10-11

**Status: current** — rewritten at each pause; the last one is in `git log
-- docs/handoff.md`. Where the work stopped, what it left, how to pick it up,
and the options from here. Numbers carry their date and command; the
generated ones are in [`status.md`](status.md).

## Where things stand

`main` holds everything; no work is in flight. Every agent worktree is
merged and removed; the checkouts under `target/` (`baseline-wt`,
`identity-base`, `snapshot/tree`) are the scripts' own and stay.

The milestone run at `1ebc57a` (`scripts/gates.sh --snapshot`, 2026-10-11,
361 s, quiet) is green on every stage and ratchet:

| | |
|---|---|
| conformance | **2681/2681** decidable (241 decided only by the C++ backend); `--strict` failing **0**; graded complete (`FRONTEND_COMPLETE`) |
| `errors` | first error's position and message: 701 agree, **18** disagree — all parse-recovery positions, divergence #30 — 0 silent |
| `ptdiff` | **2422/2422** programs walk into the backend call for call (structural) |
| `backend` | **3267/3270** agree through the real C++ backend; 1 "crash" is `risc-v.k`, which the backend cannot build inside the gate's 8 GB / 900 s (nor can the reference's); 2 declined are `basics.k::4,5` (owner, below) |
| `output` | **266/274** RTL outputs identical modulo locations; the other 8 are programs the reference itself fails on — 0 differ |
| `repoint` / `cli` | **72/72** upstream test files; **25/25** upstream CLI tests |
| `loop-left` | **321/321** roots leave what the reference leaves, rounds agree on all |
| `edit-identity` | **348/348** — the warm editor after every kind of edit equals a fresh `kg check` |
| `shadow` | the editor's kept loop: 0 unequal, 0 reuse-unequal |
| tests | 1885 passed; clippy 0 |

**Against the Haskell frontend:** functional parity on everything the
reference's own tests and the gates can see. What is left is listed under
[next steps](#next-steps) and is small: recovery positions, fuzz-found
corners off the corpus, and decisions that are the owner's.

## What the 2026-10-10/11 session landed

70 commits on `main`, `fb0afc1` through the one adding this file; the
code a net **13.8k lines smaller** (+9.6k / −23.4k across `crates/`,
`xtask/`, `scripts/`, `git diff --shortstat fb0afc1~1`).

- **One engine (W3.7, S5).** The CST inference engine, its constant
  evaluator and the second census are deleted (21k lines); the
  program-wide loop answers `kg check`, the backend walk and every editor
  feature. Divergences #19 and #20 closed with it.
- **Consolidation (W4).** Every "copies today" row of
  [`single-source-of-truth.md`](plans/single-source-of-truth.md) is empty
  or a register entry with its reason and comparing gate; five pieces of
  dead code the twin left alive-looking went; `F(const auto x)` now gets
  the reference's error.
- **The loop across edits (W7.5d D1 steps 1–3).** The editor keeps each
  run's loop, keys every module-round by its copies and by what it read
  across the module boundary (step 2, the read door), and reuses a
  followed module's prefix pass (step 3) — all checked by `shadow` over
  every root. Step 4 is planned, not built
  ([`loop-across-edits.md`](plans/loop-across-edits.md) §6).
- **Memory, watched per use case.** `cargo xtask mem` holds `kg check`,
  the editor opening a file and after five edits to
  [`memory-budgets.json`](../memory-budgets.json) (peak footprint on
  macOS). `kg-alloc` gives every binary one allocator; the language
  server purges freed pages eagerly. The owner accepted D1's second
  program on the condition of efficiency (`performance.md` §12).
- **References and rename across the project** (`kg-lsp`). Every file
  that spells the name and whose own program holds the declaring module
  is searched, each under the program it roots (never one
  workspace-wide program, which would over-resolve); re-exports and
  exposure lists followed; bounded (512 files; 10 s for references,
  60 s for rename, which refuses past it rather than edit partially).
  Two pre-existing rename holes closed on the way (modules with a header,
  exposure lists).
- **Fuzz-found parity (22 fixes).** `cargo xtask fuzz --differential`,
  seed 20261010: the one false alarm, 14 of 18 ours-silent buckets, the
  parameter member-template gap; pass 10 in copies, `findSymbol`'s
  parse-time table, redefinition positions, `errorMsg`'s replacement in
  both directions. One regression they brought (a `decltype` verdict under
  the editor's truncated first answer) was found by a full `edit-identity`
  and fixed (`1ecf658`).
- **Tools.** `xtask mem`; the `shadow` ratchet; `scripts/vs-haskell.sh`
  (the comparison below, reproducible).

## Ready to publish (later 2026-10-11)

At the owner's request, the repository was made publishable and the first
two tooling gaps closed:

- **Upstream as a submodule**: `third_party/kanagawa`, pinned at the
  reference (`98a2d8c`). The gates, tests and scripts read it
  (`kg_base::reference_root`, `KANAGAWA_ROOT` still overrides); `../kanagawa`
  and `../kanagawa-build` are no longer needed.
- **The backend built from it**: `scripts/build-backend.sh` — LLVM, MLIR,
  CIRCT and `libkanagawa-backend`, never the Haskell — proven from scratch
  (~28 min on 16 cores, 6 GB tree, a 34 MB library needing only the
  system's C++ runtime); `--reference` adds upstream's own `kanagawa` for
  the parity gates (`target/reference`). On `cache.k` the reference and
  `kg` through the new library write the same RTL, source-location
  comments aside.
- **A release archive**: `cargo xtask dist` → `kg`, `kanagawa`, the
  backend, the standard library and the licenses in 14 MiB; a fresh unpack
  checks, builds and documents a design with nothing configured (`kg`
  searches the bundled library last where no import directory has
  `base.k`).
- **README and LICENSE**: rewritten for a public repository; MIT, "Parker
  Mitchell and the kgrust contributors", Kanagawa credited to Microsoft.
- **Doc comments in the editor**: hover and completion show a
  declaration's `//|`/`//<` documentation; an import's hover the module's.
- **`kg doc`** (alias `docs`): a static, searchable site from the doc
  comments, no pandoc — the standard library's 124 modules in 2 s.

**The gates, against all of it: green** (snapshot `b795e3a`, 1880 s,
after `cargo xtask ptrace` re-recorded the reference's runs — their keys
carry absolute paths, so the root move staled them; 17.5 min for 3,270
programs, `risc-v.k` alone with `KG_CHILD_SECS`/`KG_CHILD_MEM_MB` raised).
Every ratchet +0/−0 but `output`, up to 269/274 (`9320a96`): under the
release-channel library the reference compiles three `export-class`
programs it failed on, identically to ours. `risc-v.k` now goes through
the backend — the reference recorded it in 219 s where it took 2,629 s
under the local channel — but in a time that swings: 323 s on one run of
ours, over 20 minutes on the next, sampled entirely in
`RemoveUnusedGlobals`' address-ordered liveness (`backend-survey.md` §2.2;
§7 below).

## Performance

**Batch `kg check` against the reference** (`scripts/vs-haskell.sh`,
2026-10-11 at `1737ca6`, best of three, interleaved;
[`performance.md`](plans/performance.md) §11). The shipped reference runs
its frontend on every core (`app/Main.hs:62-68`); ours is single threaded,
so both are shown:

| Program | Haskell, 1 core | Haskell, 16 cores | Rust | faster: per core / as shipped | peak RSS MiB: Haskell 1 / 16 cores / Rust |
|---|---:|---:|---:|---:|---|
| a four-line function | 0.61 s | 0.62 s | 0.05 s | 12× / 12× | 171 / 2141 / 41 |
| `data/cache.k` | 4.62 s | 1.84 s | 0.22 s | 21× / 8× | 273 / 2230 / 101 |
| `data/bits.k` | 6.34 s | 2.12 s | 0.26 s | 24× / 8× | 262 / 2219 / 122 |
| `data/vector.k` | 20.95 s | 5.92 s | 0.67 s | 31× / 9× | 424 / 2260 / 256 |
| `data/array.k` | 28.22 s | 12.79 s | 0.80 s | 35× / 16× | 526 / 2253 / 304 |
| `numeric/float.k` | 30.38 s | 10.58 s | 0.66 s | 46× / 16× | 596 / 2315 / 296 |
| `processor/risc-v.k` | 54.03 s | 24.61 s | 1.95 s | 28× / 13× | 1068 / 2823 / 776 |

The comparison published earlier the same day (12–45×) was the one-core
column, unlabelled. On every core the reference holds 2.1–2.8 GB on every
program (`-A128M` per core).

**A whole build** (`performance.md` §13): the C++ backend is 92–98% of
it — `array.k` 0.87 s frontend, 10.4 s backend; `float.k` 0.69 s and
14.6 s — and costs the same whichever frontend feeds it, so our whole
build is 1.5–2× the reference's speed (`bits.k`, `float.k`, `array.k`;
`cache.k` within the backend's noise), not 8–46×. `risc-v.k`'s backend
did not finish inside 8 GB under the local channel on either side; under
the release channel it finishes, in 4–20+ minutes at ~10 GB (§ "Ready to
publish"; `backend-survey.md` §7). The backend is measured only, not
worked on.

**The backend, surveyed** ([`backend-survey.md`](plans/backend-survey.md),
2026-10-11, read and measured only): it runs on one thread; each program's
time is in a different pass; `GlobalDataPropagation` computes and changes
nothing under the default options (2.4 s of `array.k`); the two noisy
passes visit blocks in heap-address order; `risc-v.k`'s backend spends its
time in `BitConstantPropagation` and is at 10.5 GB by 100 s. Options from
no code (a whole-run cache in `kg build`) to a Rust rewrite (not
recommended); whether the backend is in scope is the owner's.

**The developer tooling, surveyed** ([`developer-tooling.md`](plans/developer-tooling.md),
2026-10-11): what exists and its state; the gaps — no release or published
extension, doc comments reaching no tool, no `kg test`, `kg doc` and `kg
init` specified but not built, backend errors only under `kg build` — and
a suggested order, the owner's to set.

**The editor** (the reference has no incremental mode to compare):

- the bench's keystroke (milestone run, the editor's first answer at four
  rounds): **379 ms** worst (`risc-v.k`); the same run's fixpoint 777 ms
  worst (`array.k`);
- `risc-v.k` under its project's settings (50 modules,
  `bench --filter keystroke-phases --recorded`, load 4–9): first answer
  ~0.83 s, last ~2.0 s; `array.k` last ~0.85 s;
- memory (`xtask mem`, milestone): `risc-v.k` `check` 768, editor open
  887, after five edits 1371 MiB footprint; all 15 cases within budget.

Where a library keystroke's time goes now (D1 step 3's profile,
`KG_LOOP_PROFILE`, `risc-v.k`, a 1.62 s loop): the library's inference
walk 0.375 s, `instantiate` 0.098, `deduceTemplateArgs` 0.085,
`addLambdaWrappers` 0.037, `closureCalls` 0.021; the edited root's own
prefix 0.146; `Program::symbols` 6.5% of the loop. Reusing the first four
needs the read door's items re-askable — a design, not a step.

## Quality

- **Exactness is gated, not argued.** Every optimisation and refactor
  this session landed byte-identical: `infer-identity` 304/304,
  `scripts/identity.sh` 304/304, `edit-identity` 348/348; every new test
  proven load-bearing with `scripts/mutant.sh`.
- **Ratchets per item** (`baseline.json`): nothing gave ground this
  session but `shadow`'s `kept` (1808 → 1746, blessed: those 62 steps
  were kept only because step 1's keys missed what a round read — the old
  set was unsound on them).
- **What the gates cannot see**, written down: conformance is blind to
  positions and messages (`errors` covers the first error of each
  must-reject snippet); `ptdiff` and `output` compare modulo source
  locations (the owner's bar); the fuzzer finds what the corpus does not
  hold — seed 7 left 141 disagreements in 41 buckets.
- **One lesson to keep:** `fast.sh loop` runs a quarter of
  `edit-identity`; a change to a verdict about what the loop left
  standing needs the whole run (journal, "The fuzzer's silent buckets",
  its afterword).

## How to resume

1. Read [`../CLAUDE.md`](../CLAUDE.md) — the rules bind — then this file,
   [`status.md`](status.md) and
   [`plans/sprint-parity.md`](plans/sprint-parity.md) §5 (the milestones)
   and §8 (the owner's decisions).
2. Work on a branch off `main`, squash-merged when done (`CLAUDE.md`,
   "Branches"); the history before 2026-10-11 is the local `dev`.
   The loop: `scripts/probe.sh` → `quick.sh` → `fast.sh [walk|loop]
   [--lint]`; the full gates only at a milestone, `--snapshot`. Bless a
   ratchet from the main checkout only, after reading its diff.
3. Parallel work goes to worktrees (`scripts/worktree.sh link` in each);
   one heavy run at a time machine-wide (the lock in `target/`); two or
   three agents building at once at most.
4. A fresh clone: `git submodule update --init third_party/kanagawa`,
   then `scripts/build-backend.sh --reference` (LLVM and CIRCT fetched
   shallow; ~30 min; needs CMake, Ninja, Boost, and ghcup for the
   reference) before the gates that run the backend.
5. Data kept under `target/` (not in git): the backend's build
   (`target/backend`) and the reference's (`target/reference`); the fuzz
   buckets of seeds 20261010 and 7 in `target/difffuzz-archive/`; the
   timing oracle in `target/oracle-bench/` (rebuilt by `vs-haskell.sh` when
   the reference changes).

## Next steps

Options, with a recommendation. Each is independent of the others unless
it says so.

1. **Recommended first: the fuzz residue** (parity off the corpus; small,
   independent items, each landed with `kg-oracle --errors` evidence and a
   mutant-proven test):
   - seed 7 (`--seconds 240 --jobs 4 --reduce 200`): 41 buckets, 13
     ours-silent in 9 — a stray `;` before `import`, "Could not deduce
     variable type" in a helper, a `void` interpolation in
     `library/debug/print.k` (pass 18 in a copy), a static assert over
     `<<`, a class-template body parse failure, names used as types;
   - seed 20261010's five left: [13] `b ? x : x && a` in a lambda
     (position), [21] an alias instance spelled `uint4` not `U<10>`, [28]
     a template-template argument's message, [39] one column off, [26] a
     reference crash (nothing to match);
   - divergences #39–#41: #39 (`paren_note` in copies, `looped.rs`) is
     unblocked now that no agent holds the file;
   - then new seeds, until a run's ours-silent and false-alarm buckets
     are empty.
2. **Multi-threading and the tail rounds** (`performance.md` §13's
   triage, 2026-10-11): our loop runs on one core where the reference
   gains 2.2–3.5× from sixteen; `array.k` and `float.k` spend half their
   loop in a tail of cheap rounds that each rebuild the symbol maps and
   re-run deduction. Both speed `kg check` and the editor alike. Measure
   the parallel headroom first.
3. **The keystroke (W7.5d, beyond D1 step 4).** The real 50-module
   keystroke is ~0.83 s to the first answer; the library's inference,
   instantiation and deduction are ~0.6 s of a 1.6 s loop and are redone
   every keystroke. Step 4 (symbol maps, at most 6.5%) is planned in
   `loop-across-edits.md` §6; the large win needs the read door's items
   re-askable so the kept run's questions can be asked of the new run's
   state — write that design first. Highest user-visible payoff, highest
   risk; `shadow` and `edit-identity` already hold it exact. **Discussed
   2026-10-11** (`loop-across-edits.md` §8): carrying the library takes at
   most ~40% off the loop (roughly 0.5 s first, 1.2–1.3 s last on
   `risc-v.k`'s project), so the target needs more levers — step 4,
   parallel rounds (W5.6, not built: the loop runs on one core), reuse
   inside the edited module. Recommended order: measure a real keystroke
   end to end and the parallel headroom, then step 4, then the
   re-askable-reads design for review.
4. **The editor's rename and references, the follow-ups:** member renames
   are refused where a receiver sits in a lambda or behind an abbreviated
   template's `auto` (no loop types it at the text); a member use inside
   a template body no instance reaches is not counted (documented, not
   refused); an `xtask mem` case for a project-wide search; the bounds as
   `[lsp]` settings if wanted.
5. **W4's deferred D2 item:** move the parser's template checks onto the
   sorted lowering's facts, retiring `Templates`, `visible_through_imports`
   and `Binders`. It changes verdicts, so it goes through `errors` and
   `conformance` like a port.
6. **`errors`' 18 recovery positions** (divergence #30): accepted per
   case; some are ours-better. Worth a pass only for the ones a user would
   find misleading.
7. **Batch memory:** the 10–16 MiB higher one-shot peak since D1 step 1,
   and arena compaction; nothing is over budget, and the reference uses
   more on every program.

**For the owner to decide** (each blocks only its own item; the work
assumes the first answer meanwhile):

- the source-locations bar — "identical modulo locations" stays, or byte
  identity (a workstream: divergence #25's spans);
- `basics.k::4,5` — the reference binary (crashes) or the corpus
  (`expected:0`) is the reference;
- the conformance gate's `--Wall` reading (fixing it edits the
  scoreboard), and tests beside `conformance::score`;
- the query layer's substrate — hand-rolled (meanwhile) or salsa;
  re-examined 2026-10-11 (`loop-across-edits.md` §8): the lead recommends
  staying hand-rolled, borrowing salsa's algorithms and drawing the
  read-keyed memo into one internal primitive, with three triggers to
  revisit; mimalloc kept meanwhile;
- **new, 2026-10-11** ([`backend-survey.md`](plans/backend-survey.md) §6):
  whether the backend is in scope — C++ changes, upstreamed or a local
  patch set — and if so the order there; moving the reference pin to
  upstream's current backend first;
- **new, 2026-10-11** (`sprint-parity.md` §8): the keystroke's target
  (proposed: first answer ≤ 400 ms, last ≤ 1 s on `risc-v.k`'s project);
  whether the reference's round semantics bind after parity, which decides
  whether a demand-driven, possibly salsa-based design is open; and how
  much salsa's familiarity weighs;
- **new:** rename in a project with no manifest — today only the open
  buffers' programs are searched, so a sibling file importing through its
  own directory can be missed: refuse such renames, or scan the declaring
  file's ancestor directories.
