# Sprint: parity, one engine, and speed

> **History — finished 2026-10-11.** The parity sprint's plan (W0–W8,
> milestones S1–S6): every milestone reached but S5's last item (the loop
> across edits, D1 step 4). What it left open is carried into
> [`roadmap.md`](../roadmap.md) — next steps and open questions — and the
> live work into [`plans/`](../plans/README.md). Kept as the record its
> workstream codes (`W7.5d`, `S5`, …) refer to throughout the code and docs.

*Planned 2026-09-27. The next sprint's basis: it folds the open end of
[`ir-elaboration.md`](ir-elaboration.md) (M5's leftovers, M6), the deferred
half of [`performance.md`](../plans/performance.md), the pending items of
[`single-source-of-truth.md`](../plans/single-source-of-truth.md), and every gap the
2026-09-27 gate run lists, into one ordered queue.*

*Revised 2026-09-27, after a read-only survey of the code, the gates and
the tools (journal, "The foundation survey — 2026-09-27"). It added what
the first draft had as a target without a mechanism — the query layer, W7
— and the foundations the tools stand on, W8; W0, W1.1, W3, W5.1 and W6
grew; §7 says what yields if the sprint runs long and §8 what the owner
still has to decide. Nothing was built or run for the survey: its findings
are read in the code and cited, and every new number below is W0's to
measure.*

## 1. Where we start (gate run `ad1d9fb`, all green)

| Measure | Now | Sprint target |
|---|---|---|
| `conformance` | 2571/2571, 351 undecidable | stays 100% |
| `conformance --strict` failing | 34 | ≤ 8 |
| `errors`: silent / disagree | 28 / 26 (18 of them recovery positions, divergence #30) | ≤ 5 / ≤ 20 |
| `ptdiff` structural | 2287/2422; 131 diverged, 4 declined | ≥ 2380 |
| `backend` | 3210/3270; 51 declined, 4 differ, 2 crash | declined ≤ 12 |
| `output` equal modulo locations | 204/274; 54 differ, 8 declined | ≥ 250 |
| `repoint` | 51/72 | ≥ 62 |
| instances left undecided (`Report::left`) | 3964 over 155 of 304 roots — *to be redefined, §3 W0* | 0 on every program the reference compiles |
| roots at the round guard | 1 (`test/library/data/string.k`, 32 rounds) | 0 |
| `kg check array.k` / `risc-v.k` | 3.0 s / 6.1 s, 1.4 / 1.9 GB | ≤ 1.5 s / ≤ 3 s, memory ≤ the reference's (0.55 / 1.1 GB) |
| keystroke, worst case | 1.46 s (`risc-v.k`) | ≤ 400 ms (M5's bar); ≤ 100 ms is M6's |
| `scripts/fast.sh walk` / full gates | 79 s / 871 s | ≤ 90 s / ≤ 900 s, held |
| what an edit inside one function body recomputes | the program: a new item tree, a new `Program`, every module lowered again, the loop from its first round (read in the code, §3 W7; the split by phase is W0.5's) | the edited module; an importer only when the module's interface changed |
| a check an edit has superseded | runs to its end (`kg-lsp/src/main_loop.rs:325-327`) | stops within 50 ms |
| the editor's rounds | four (`PassBudget::INTERACTIVE`), so its diagnostics may differ from `kg check`'s | the fixpoint, cancellable; the same diagnostics — when W7.5 says how (§3). *Done 2026-10-10 (W7.6): four rounds first, then the same run's fixpoint, `kg check`'s* |
| gates green with their instrument absent | `sem`, `ir`'s oracle half; and the compat binary compiles frontend-only, exit 0, when it finds no backend library | none: absent is red |
| the reference's commit | recorded nowhere (the checkout is `98a2d8c`, clean since 2026-09-27) | pinned in the tree, checked by the guard |
| production readers of mangled text | about a dozen (`walk.rs`'s splitters, `subtype.rs`'s three, `infer.rs`'s two); W0.4's lint counts them | 0 |

Against the reference's own frontend we are 5–9× faster and lose only on
memory for the largest programs ([`performance.md`](../plans/performance.md) §11).

## 2. Rules for the sprint

Unchanged from `CLAUDE.md`, restated because every workstream leans on them:

- **Correctness is never the price.** A behaviour change lands with ratchet
  evidence; a refactor or optimisation lands exact — `cargo xtask
  infer-identity` (both inference modes, trajectories included), the
  cross-build `--dump` diff, `KG_INFER_CHECK=1` — or it does not land.
- **Fail toward silence.** No diagnostic arm is switched on while it can
  invent an error on a program the reference compiles (§3 W1.4 is gated on
  exactly this).
- **Cite `File.hs:line`** for every rule ported; prove every new test
  load-bearing by mutation.
- **Per change `scripts/fast.sh`** (`walk` when the loop or the walk can
  move); **full gates at each milestone below**, and before blessing
  `output`/`repoint`.

Added by the revision:

- **An absent instrument is red.** A gate whose oracle, backend library or
  corpus is missing fails and says which; it never skips to green. Two
  gates and the compat binary do today (§3 W0.4), and a worktree's first
  snapshot run "graded nothing" that way (`87d5d63`).
- **The instrument precedes the mechanism.** Every step of W7 and W3 lands
  after the gate that can see it go wrong, and that gate is shown red by a
  mutant first (`CLAUDE.md`, "A gate's specification is a hypothesis").
- **What changes a verdict is a setting.** A switch that changes what the
  compiler decides or emits lives in `Settings`, inside the memo keys; an
  environment variable may only trace, or be read once at the front door
  and turned into a setting (§3 W8.2).

## 3. Workstreams

Nine. W0–W6 keep their numbers, which other documents cite; W7 and W8
were added by the revision, and **the order is §5's, not the numbering's**.
W1 is the sprint's centre; W2 and W8 are independent small work that fills
the gaps between W1's steps; W7 is the substrate the editor's targets need
and W3 moves the editor onto; W3–W5 are the architecture and performance
these enable; W6 keeps the instruments honest.

### W0 — Baselines, first (a day for 1–3; two or three with 4–7)

1. **Define `left` once.** `Report::left` counts `TemplateInstance`s outside
   template bodies in the final tree; `risc-v.k` reads 1039 under `xtask`'s
   check settings and 141 under the compat compiler's. Settle which
   settings the metric uses, classify what it counts (a decision the
   reference made and we did not, a mention the reference also leaves, a
   program the reference rejects), and make it a ratcheted gate
   (`cargo xtask loop-left`, or a row in `sem`).
   — **done 2026-09-27: `cargo xtask loop-left`.** The metric uses the
   recorded compile's settings (`--import-dir library`, the default base):
   `xtask`'s `rooted_settings` build against `mini-base.k`, which the
   reference's programs are not compiled against — `risc-v.k` is *rejected*
   there with 16 errors, hence 1039. And it is compared, not counted: the
   reference leaves instances too (a mention no round reaches), so both
   sides' leftovers are spelled `getScope|getQualifier|getName` —
   `kg-oracle --loop` for the reference, `kg_sem::elaborate::left_labels`
   here — and compared as multisets per root. First reading: 321 roots the
   reference compiles (27 it rejects, booked): 315 leave exactly what it
   leaves; ours 2701, the reference's 2557, all 2557 shared; **ours only
   144, over 6 roots**; theirs only 0. Ratcheted per item (`agree`,
   `ours-only`, `theirs-only`, `rounds-agree`, `near-guard`).
2. **Round counts against the reference** (`performance.md` Step 4):
   `kg-oracle --template-iterations N` for the smallest `N` reaching the
   fixpoint, per root, against `Report::rounds`. `data/string.k` at the
   32-round guard is the first case to read — non-convergence or a real
   count.
   — **done 2026-09-27, in `loop-left`**: the reference's count is the
   smallest `--template-iterations R` whose pass-8 tree equals the
   fixpoint's (`kg-oracle --loop` searches it; `Template.hs:52-57` makes the
   property monotone). Rounds agree on 316/321 roots — every root the
   guard does not stop. The five that differ are the guard's: the
   reference takes 33, 33, 34, 36 and 51 rounds on `data/memory/pipelined.k`,
   `data/vector.k`, `data/string.k`, `data/bitarray/banked.k` and
   `data/memory/bank/schedule.k`, and those five hold 143 of the 144
   instances only we leave. `data/string.k` is a real count (34), not
   non-convergence. W0.6 has the consequence.
3. ~~**Refresh `docs/status.md`'s prose and `docs/gaps.md`.**~~ — done
   2026-09-27: `status.md` rewritten for the IR era (the old prose in
   `docs/history/status-2026-09-26.md`), `gaps.md` moved to `docs/history/`,
   superseded by §4 below.
4. **The environment is part of the grade.**
   - The reference's commit is written in the tree (`reference.lock`:
     commit, and the hash of `library/` + `test/` the oracle cache already
     computes, `xtask/src/oracle_cache.rs:156-169`) and `xtask/src/guard.rs`
     refuses a run against another. The 163 `.pdrpt` files old gate runs
     had left in the checkout were moved out on 2026-09-27, at the
     owner's word, to `../reference-strays-2026-09-27/` (paths kept, a
     `MANIFEST.txt` beside them); `git status` there is clean at
     `98a2d8c`, so the guard can also fail on any untracked file.
   - `sem` fails when `find_oracle()` is `None` (it returns `[0; 4]` and
     passes, `xtask/src/sem.rs:117-120`); `ir`'s oracle differential fails
     rather than printing "skipped" (`xtask/src/ir.rs:1309`).
   - The compat binary under a gate never falls back to a frontend-only
     run: `repoint` sets `KG_BACKEND_DIR` as `backend` and `output` do
     (`xtask/src/backend.rs:67`; `repoint.rs` sets none), and a gate's
     child that finds no library is that program's failure
     (`kg-cli/src/legacy/run.rs:153`). The compiled-in third candidate
     (`kg-backend-sys/src/locate.rs:43-45`) does not exist beside a
     worktree — the likely reason for the 21 `repoint` tests a worktree's
     baseline disagreed on (`CLAUDE.md`, "reason unmeasured"). Re-measure
     it once the variable is set, and record the answer.
   - Corpus tests that return early without `../kanagawa`
     (`kg-db/tests/query.rs:526-529`) fail under the gates
     (`KG_REQUIRE_CORPUS=1`, set by `scripts/lib.sh`).
   - `xtask lint`, which `single-source-of-truth.md` names as D1's ratchet
     and which was never written: the retired spellings (`"<|"`,
     `replace('@'`, `TypeKey {`) outside their owner modules, and a
     diagnostic's message compared or searched in production code
     (`.message ==`, `.starts_with(`, `.contains(`; W8.1). It
     starts as a count per pattern in `baseline.json`, which may fall and
     not rise.

   **Progress 2026-09-27** — done:
   - `reference.lock` pins commit `98a2d8c…` and a hash of every file under
     `library/` and `test/`; every xtask command that reads the reference
     checks it first (HEAD, `git status` of those two trees read with
     `--no-optional-locks`, the hash) and names each difference;
     `cargo xtask pin-reference [--write]` reads or moves it. ~20 ms.
   - `sem --oracle` and `ir`'s differential fail without `kg-oracle`.
   - `KG_BACKEND_DIR`, when it names a directory, is the only place looked
     (`kg-backend-sys/src/locate.rs`); named and missing, the compat binary
     exits 1 instead of running frontend-only; `repoint` names it as
     `backend` and `output` do (`--build-dir`). **Re-measured** at the
     S1 milestone, whose gates run in a worktree (`target/snapshot/tree`):
     `repoint` 51/72 there, its pass set equal to main's baseline item for
     item — the worktree disagreement of 2026-09-23 does not recur once
     the library is named, which makes the compiled-in fallback its
     likely cause.
   - `kg_base::corpus_absent`: eleven corpus tests that returned early now
     fail under `KG_REQUIRE_CORPUS=1`, which `scripts/lib.sh` sets.
   - `cargo xtask lint`, ratcheted per occurrence, in every tier. First
     reading: `mangled-text` 13, `mangled-readers` 11, `message-test` 4,
     `env-verdict` 5, `type-key-literal` 1, `namespace-replace` 0,
     `rename-layer` 0. Two of the survey's citations were tests
     (`navigate.rs:1155`, the `.message ==` dedups compare two
     diagnostics, not a diagnostic with a text); the lint counts only a
     diagnostic's text tested against a literal.
5. **What a keystroke spends, by phase and by edit.** Per-phase timers in
   `Db` (parse, item tree, program, sorted lowering, the loop, the CST
   passes, the table overlay) and a count of memo misses per query, for
   four edits on the bench's worst files: inside a function body, a
   signature, an import, and a body in a library module the root imports.
   W7's targets are set from this table, not from the one number §1 has.
   — **done 2026-09-27.** `kg_db::stats`: every memo's hits and misses,
   and each phase's time exclusive of the phases inside it; `Db::stats()`
   snapshots subtract. `cargo xtask bench --filter phases` prints the
   table under the *recorded* compile's settings — the bench's `keystroke`
   stage runs under `rooted_settings`, where `risc-v.k` is a program the
   reference rejects, so §1's 1.46 s was measured on one. One pass,
   2026-09-27:

   | edit | `risc-v.k`, 4 rounds | at the fixpoint | `array.k`, 4 rounds | at the fixpoint |
   |---|---|---|---|---|
   | body | 1.82 s | 5.62 s | 0.38 s | 2.83 s |
   | signature | 1.69 s | 5.54 s | 0.35 s | 2.78 s |
   | import removed | 0.97 s | 2.37 s | 0.24 s | 0.98 s |
   | library body | 1.81 s | 5.67 s | 0.38 s | 2.84 s |

   Of a `risc-v.k` body edit in the editor, the loop is 1.31 s (72%), the
   root's CST passes 0.20 s, the sorted lowering of every module 0.18 s
   (W7.3's), `program` 0.03 s; every edit, a library body's included,
   misses `item_tree` once, `program`, `looped` and `sorted` once each,
   and every module's `file_scope` — the loop from round one.
6. **The round guard against the facts.** `ROUND_GUARD` is 32 and its
   comment says the corpus needs at most 12
   (`kg-sem/src/elaborate.rs:47-49`); `budget.rs` says `array.k` takes 31.
   W0.2's counts settle which is true today and the comment is corrected;
   a root within two rounds of the guard is reported by the gate.
   — **done 2026-09-27.** Neither was true: the reference's own count
   reaches 51 (`data/memory/bank/schedule.k`), and 33–36 on four more, so
   the guard of 32 cut five programs short — the budget, not a
   non-convergence. `ROUND_GUARD` is 128 (the reference has none,
   `template_iterations = maxBound`); `loop-left`'s `near-guard` set
   reports a root within two of it. Rounds agree 321/321; `left` agrees
   on 320/321 (the one left: `interface/external_class.k`'s
   `Adder::Add`, W1.2's); `backend` 3210 → 3215, its "template instance"
   and "deduce: Unresolved" declines at 0; `ptdiff` structural unchanged
   (two roots now walk to a name divergence, W1.1's).
7. **`known-divergences.md` made true.** #11 (`kanagawa` never exits 0)
   and "Not yet implemented" describe a compiler that no longer exists;
   #32 is used twice; #9 and #21 quote counts from August. The compat
   binary's real gaps (W8.6) are booked there.
   — **done 2026-09-27.** #11 closed (the binary compiles through the
   backend; `repoint` runs), its "gate on L1" section kept as history; the
   CLI's #32 (a backend crash is exit 3) renumbered #33 — every citation
   meant the six silences; #9's and #21's counts dated; "Not yet
   implemented" rewritten as what is absent today — passes 11/12's arms,
   pass 15, `deduceAuto`'s rewrite half, and the compat binary's flags
   (`--template-iterations`/`--template-passes`, `--file-list`, four
   `--dump-*`) that are parsed and read by nothing, `pretty`, `list-deps`
   — each checked in `kg-cli/src/legacy` before it was booked.

### W1 — Template elaboration parity (the centre)

The one mechanism behind most remaining rows: 77 of `ptdiff`'s 131
divergences, 29 of `backend`'s 51 declines, most of `output`'s 54
differences, 10 of the 28 silent snippets.

**W1.1 Instance identity and display** — `single-source-of-truth.md` D1.2
and D2.5, and M5's `InstanceRegistry` (`ir-elaboration.md` §2.4).

- Instances carry structured identity (`TArg` arguments) from decision to
  walk; the display the backend receives is rendered from it as the
  reference renders it (`Mangle.hs`, `PrettyPrint.hs`). Today a type
  argument is spelled module-qualified (`check_return_type<bool,
  data.optional::optional, …>` against the reference's `optional`), and a
  call trace in `$fatal` prints the mangled name
  (`pipelined_for<|$uint@9…|><uint10>` against `pipelined_for<uint9, …>`).
- Retires the walk's six mangled-name splitters and the three unmanglers
  D1.2 lists.
- **Identity is structure, from the census on.** Today the census keys an
  instance by `render_exp` of its arguments (`Decided::key: String`,
  `kg-sem/src/census.rs:75`), `InstanceId` is a hash of two strings
  (`elaborate.rs:366`), and `TArg::Expr` is printed text: two instances
  are one when a printer spells them alike. The registry's key is the
  template's declaration and its `TArg`s compared structurally; the
  mangled name and the display are both *rendered from* the entry and
  never read back. This is also what W7.5 stands on — the registry is the
  only channel between modules — so it is built as that, not as a name
  table.
- **Exit:** the `ParseFunction`/`ParseTypedef`/`ParseStruct`/`ParseClass`
  name rows of `ptdiff`'s first-divergence table at 0; `output`'s
  call-trace differences gone; no production code parses a mangled name
  (`xtask lint`'s count at 0 for the readers §1 lists); the census's key
  and `InstanceId` hold no rendered text. The gate rows alone can be
  reached by one more splitter — the structural half of the exit is what
  says they were not.

**Progress.**
- **Step 1 — 2026-09-27: the display is the note's.** The walk names a
  copy by `unmangleInstanceName` over its `TInstance` note
  (`ParseTree.hs:176-179`) before any reading of its mangled name. Three
  notes had to be made faithful first: a copy's root starts with its
  template declaration's note, so a copy of a copy is typed inside the
  first copy's instance (`desugarWithNote`, `Template.hs:127`; `setTypeOf`
  nests, `Type.hs:1050-1057`) — `Fn_T_auto<bool>`, not
  `Fn_T_auto<|$bool|><bool>`; a template named as an argument is
  `TArg (showPretty e)` (`Type.hs:437`), which leaves out a module-
  namespace qualifier (`PrettyPrint.hs:146-148,191-194`) — `memory_ecc`,
  not `data.memory::memory_ecc`; an enumerator is shown by its written
  name (`PrettyPrint.hs:98`) — `Denorm::Off`. `ptdiff` structural 2287 →
  2359 (+72, none lost); its `ParseFunction` name rows 79 → 21. The text
  path still answers the copies with no note — mostly wrappers hoisted
  out of a copy, which the reference makes as functions and this port by
  instantiating a generic wrapper — so the splitters stay until step 2.
- **Step 2 — 2026-09-27: the walk reads no mangled name.** A declaration
  is displayed iff the loop's own note at it — by node, not by the
  table's `TypeKey`, whose head a function hoisted out of a lambda shares
  with the `Closure` copy made over it — is an instance, and then from
  that note alone; aliases likewise; `unmangleQualifiedName`'s map is
  the loop's notes and is never answered from a name's shape. The six
  splitters, the display fix-ups and the two spelling tables are
  deleted. `ptdiff` structural 2359 → 2373 (+14, none lost);
  `ParseFunction`, `ParseTypedef`, `ParseStruct` and `ParseClass` name
  rows at **0**; `lint` `mangled-text` 13 → 7, `mangled-readers` 11 → 5
  (`walk.rs` at 0). What still reads mangled text is the diagnostics'
  `show_unmangled` (the reference's `showUnmangled` is `unmangle symbols`
  over the type, `TypeCheck.hs:36`) — step 3 — and the CST engine's
  deduction (`kg-ty/src/infer.rs`, `mangled_instance`), which runs before
  the loop and so has no instance note to read: it goes with that engine
  (W3.7).
- **Step 3 — 2026-09-27: no production code reads a mangled name.**
  Every message that names a type is `showUnmangled = show . unmangle
  symbols` (`TypeCheck.hs:36`, `Constexpr.hs:131`) over a map: the loop's
  passes ask their own symbols and notes (inference through its recorded
  lookups), the CST passes the loop's final program
  (`Looped::instance_named`, asked only when a message renders a type).
  `unmangle_text`, `unmangle_leaf`, `unmangle_item`, the map-less
  `unmangle_instance_name` and `show_unmangled` are deleted. The CST
  engine's `mangled_instance`/`mangled_arg` were dead since the twin went
  (that engine only ever types source) — removed, `sem` identical. `lint`
  `mangled-text` and `mangled-readers` **0**; `errors` unchanged (669
  agree): the structural spelling says what the text one said on every
  graded message.
- **The structural exit, as it stands.** The census keys an instance by
  the canonical *structural* encoding of its argument tree
  (`kg_hir::structure`, `Detail::Structure`: every child labelled, every
  scalar printed, no note) — the equality the reference's `Eq` on
  `NotedExp` is (`Parser/Syntax.hs:597-599`) — and `InstanceId` hashes it;
  no display printer is involved, and the display is rendered only from
  the note. Replacing the encoding's `String` by a hash of the same walk
  would change nothing it decides; left as is.

**W1.2 Instances left undecided** — triaged by W0's classification, one
mechanism per commit:

- the closure deduction chain — a wrapper's `Closure` a round late and a
  deduction over closures (`compose2`, `curry`) two rounds early, which
  must land together (journal, "What a wrapper's closure waits for");
  `control.loop`'s `pipelined_map`; `data.closure.core`'s copies;
- cross-module deduced calls with value arguments, an exported helper's
  instance, depth-two instances;
- `deduce: Unresolved` (6 backend declines, 2 `output`).
- **Exit:** `left` = 0 on programs the reference compiles; `backend`'s
  "template instance" and "deduce: Unresolved" declines at 0.

**Progress.**
- **A member template without a body — done 2026-09-27.** The one root
  whose leftovers were ours alone, `interface/external_class.k`'s
  `_adder.Add(a, b)`: an extern class's `template <typename T> T Add(T a,
  T b);` is a `Template` over a `FunctionDecl` (`Parser.hs:417-430`), whose
  `declScopeName`/`declName` `getQualifier`/`getName` read like a
  function's (`Parser/Syntax.hs:556-561,744,764`); `instantiate`'s
  declaration accessors did not admit it, so deduction never found the
  template. `loop-left` (`scripts/fast.sh loop`): ours-only 1 → 0 roots,
  `left` agrees 321/321 — every instance we leave, the reference leaves;
  `ptdiff` structural 2373 → 2378 and `backend` agree 3215 → 3220 (the
  program and four `extern-class` snippets). **Not the exit yet:** the
  walk still declines 15 programs at "template instance" (14 syntax
  snippets, e.g. `template-template-parameter::24`, which `loop-left`
  does not grade — it walks roots only — and `risc-v/testbench.k`, whose
  leftover the reference shares) and 4 at "deduce: Unresolved"
  (`return-type-polymorphism::19`, `dependent-template::16`).
- **`dependent-template::16` is the parser, not the loop — 2026-09-27.**
  In `AmbiguosSyntax`'s `else` branch `Do` is a variable when the parser
  meets `Do<a && b>(10)`, so the reference reads `(Do < a) && (b > 10)`
  (`kg-oracle --passes 1`), folds it to `true` and gives `Go` a `bool`;
  this parser commits to template arguments, the lowering leaves the path
  unsorted (`ArgsOnNonTemplate`), and `Go`'s return type never resolves
  ("deduce: Unresolved"). Needs the call re-read as a relational chain
  under the reference's precedence; not attempted.
- **`dependent-template::16` read as the reference reads it — 2026-10-10.**
  The lowering re-reads `Name<E>(C)` as the chain `Name < E > (C)` where
  the parse-time table holds no template `Name` (`Parser.hs:669-680`),
  re-associated over `E`'s looser operators (`opsTemplate`,
  `Parser.hs:913`), only where that is the whole reading (one argument
  with a looser operator at its top, one call argument, nothing around
  the call to take an operand first). `repoint` 71 → **72/72**, `ptdiff`
  declined 1 → 0 (2422/2422 structural), `backend` agree 3266 → 3267,
  declined 3 → 2 (the reference's own `basics.k::4,5` crash). **W1.2's
  exit met.** Journal: "The last corpus rows".

**W1.3 Round and copy order parity.** With W0.2's counts: copies decided
in the reference's round, declared in its order (the ~20 `ptdiff` rows
whose first divergence is a `ParseBaseList`/`ParseIdentifier`/`ParseClass`
ordering, e.g. `static-if::48`, `template::45`); `data/string.k` converges.
**Exit:** `ptdiff` ordering rows at 0; no root at the round guard.

**Progress.**
- **The premise, reproduced — 2026-09-27.** The first-divergence
  histogram (`cargo xtask ptdiff --verbose`, 37 diverged at `d5dcc6a`) is
  not mostly copy order. The largest shape, `ParseBaseList` against
  `ParseIdentifier` (9), is the parser's **redundant-import seed**:
  `checkRedundantImports` (`Parser.hs:234-240`) prepends to every file's
  `Seq` an empty `Seq` wrapped in one `WarningF` per import still
  `Redundant`, and the walk emits it — so which imports are redundant is
  a fact of the walk, and our replay of `updateNeeded`
  (`kg-db/src/imports.rs`) over-credited on purpose, a choice made when a
  missed warning cost only an undecidable assertion.
- **The seed, exact on 11 more programs — 2026-09-27.** Three causes:
  a member was probed by its bare name, where the reference looks a method
  up under its receiver's type — `methodSpecifier`, `[T, Go]`
  (`Parser.hs:729-757,652-655`) — and a field access or a designator not
  at all (`:811-819,863`); a class body's earlier member was not in scope
  (`findInScopes`, `SymbolTable.hs:256-259`); and a file with a header is
  `Seq [ModuleDecl …]` (`:188`), seeded in that outer `Seq` (`:235`) and
  so walked before the module's name, where the walk put it in the
  module's body. The receiver's type is resolved where the file alone
  answers (a variable of a plain or instantiated type the file declares);
  elsewhere every type-owned member an import exposes is probed, a
  superset of the reference's hits. `ptdiff` structural 2384 → 2395
  (`static-if::48`, `imports::13,43,49,50,75`, `enum::45`,
  `higher-order-functions::41,42,118,119`), diverged 37 → 26;
  `output` 264/274 and every other ratchet +0/−0 (`scripts/fast.sh walk
  --lint`, `cargo xtask output`). One test corrected:
  `a_nested_or_aliased_or_later_definition_is_unique`'s class-member case
  expected no diagnostic where the reference warns the import redundant
  (`kg-oracle --tree`).
- **The type-width rows, four mechanisms — 2026-09-27.** Seven programs
  whose first divergence was a width or a type's spelling; none was the
  walk's own typing, and none a copy-order question. (1) `validateArg`'s
  literal re-stamp sees through an alias of an instance (`tIsUnsigned`,
  `sizeOf`, `Type.hs:653-657,719`): `fifo_index_t = index_t<N>`,
  `UInt<16>` (`rendezvous/lenient.k`, `rendezvous/strict.k`,
  `test_cases_24.k`). (2) The loop unrolls a `static for` only once its
  limit *is* a literal (`Constexpr.hs:52`), a round after `2*N-1`'s
  two-step fold, so the induction variable is typed first (`data/array.k`,
  `data/string.k`). (3) A typed fold is typed by `inferBinaryOp` over the
  nodes' notes (`Constexpr.hs:200-202`): `-1 << 8u16` is `int10`
  (`optimization_test_cases.k`). (4) An alias spliced into a template
  argument is a copy, so the next round's resolve inside it does not
  rewrite the declaration (`data/cache/write_only.k`'s `word_t`).
  `scripts/fast.sh --lint loop`: `ptdiff` structural 2402 → 2409 (+7,
  none lost), diverged 19 → 12, width/spelling rows 7 → 0; every other
  ratchet +0/−0, `infer-identity` 304/304, `loop-left` rounds 321/321;
  `cargo xtask output` equal 264 → 265.

- **Twelve `ParseIdentifier`-family rows, eight mechanisms — 2026-09-27.**
  The rows booked to "an instance, a name or a declaration's order" were
  mostly not order: an instance never made (a float argument's mangle
  declined — `show :: Float`, `Mangle.hs:41`; `cast<float32>` and `~` over
  an enum not folded, `Constexpr.hs:262,306-343`), a value spelled
  differently (an enum constant naming an enum value, `Constexpr.hs:292`; a
  `return` converted to its function's type, `TypeCheck.hs:76,84`; a plain
  callback replaced by its initializer, `Constexpr.hs:287`), the
  redundant-import seed once more (a path's first name found in scope,
  `SymbolTable.hs:256-259`) — and one that *is* round order: `resolve`
  renames a decided key in every module and every body
  (`Template.hs:158-159`), where the loop spread it within the deciding
  module only, so a mention in another module's template waited for the
  census to reach its copy. `scripts/fast.sh --lint loop`: `ptdiff`
  structural 2402 → 2412 (diverged 19 → 9, none lost), `backend` agree
  3254 → 3258 (declined 15 → 11), `output` 264/274 (+0/−0, `cargo xtask
  output`); every other ratchet +0/−0. The three W1.4-gating leftovers
  (`type-check.k::35,36`, `constexpr.k::87,118`) are made now. Left of
  these rows: `risc-v.k` (now #394963: a class member's initializer names a
  member declared *later*, which the parser's scope does not hold yet, so
  the reference binds the enclosing class's `CustomDecode`) and
  `memory/pipelined.k` (now #38677: a type argument displayed through its
  alias, `pair<…>`, where ours shows `tuple2<…>`) — both new mechanisms.
  Journal: "Eight mechanisms behind twelve ordering rows".
- **The last two rows — 2026-10-10.** `risc-v.k` #394963: the walk re-ran
  pass 1 at walk time, inside copies against a table that never held
  their scope; it runs pass 16 only now (`kg_sem::pass1` runs pass 1 once,
  before the loop). `memory/pipelined.k` #38677: a copy keeps each node's
  note (`desugarWithNote`); ours re-inferred copied nodes from their text
  and lost `expFromType`'s `pair` note on a `tuple2<|…|>` spelling.
  `ptdiff` structural 2419 → 2421, diverged **0**; `output` equal 265 →
  **266/274**, differ **0**. **W1.3's exit met.**

**W1.4 The leftover arms, switched on.** Pass 12's inferential arms and
pass 11 ("Unresolved argument for the parameter", "Could not deduce all
arguments", "Failed to instantiate template"; `Frontend.hs:421-430`),
with pass 15 `removeTemplate` (`Frontend.hs:298-299`, wired at `:60`)
deleting what they would write on abbreviated
templates. The code exists behind `KG_LEFTOVERS=1`
(`kg_sem::check`); it invents errors today on whole library programs
because W1.2 is unfinished. **Gate to switch:** no `expected:0` assertion
and no corpus root the reference compiles gains an error. **Exit:** the
ten silent snippets of this family reported.

**Progress.**
- **The premise, measured — 2026-09-27.** `KG_LEFTOVERS=1 cargo xtask
  backend` (the compilers it spawns read the switch at their front door;
  the gates' in-process databases do not, W8.2) changes **no** verdict of
  the 3270 programs it grades — every corpus root and every recorded
  snippet: agree 3254, declined 15, crash 1, with the switch and without.
  So the arms as written no longer invent an error on a whole library
  program (the reason they were switched off; the round guard, W0.2, and
  W1.2's leftovers at 0 on roots removed it), and they close none of the
  remaining silences either: the twelve "template instance" declines are
  pass-12 verdicts at the module's own text — "Unresolved argument for
  the parameter" (`type-check.k::54`, over `cast<int32[2]>(1000)`),
  "Could not deduce all arguments", "Failed to instantiate template"
  (`Frontend.hs:421-430`) — which the loop's check does not judge, with
  the switch or without (the seam W2 row 10 named). The gate to switch
  is met on `backend`'s programs; `conformance` with the switch needs
  `conformance::settings_for` to pass it, the owner's change (S2's
  record).
- **Pass 12 at the module's own text — done 2026-09-27.** The loop's
  check judged a leftover instance's arms only inside copies ("at the
  module's text only `GT` speaks"), because asked unconditionally they
  spoke where the reference had already stopped: pass 9 sets its flag
  before pass 12 runs (`Frontend.hs:86-87`), and pass 9 at that text is
  the CST passes'. So the verdicts are split (`kg_sem::check::Checked`):
  the copies' seed the CST run as before, and pass 12's at the module's
  text (`late`) enter the CST run's sink only past its pass-9 checkpoint,
  before its pass 12 — which pass 17's return arm then reads. It could
  land only after W1.3's name rows: until the loop made `T4<0.0, 32>`
  and `Foo<float32, 1.0>`, four `expected:0` programs gained errors
  (measured, parked on a branch). `scripts/fast.sh loop --lint`:
  conformance 2594 → 2602/2602, `--strict` failing 8 → 0, `errors`
  silent 8 → 0 (disagree 26 → 25), `backend` agree 3258 → 3266, declined
  11 → 3; `ptdiff`, `loop-left`, `output` 265/274 and the identities
  +0/−0. **Exit met** (the ten silences of this family reported). One
  test corrected: `a_lambda_hoisted_out_of_a_member_finds_its_own_capture`'s
  stand-in `data.closure` lacked `make_capture1`, which the reference
  needs (it rejects the program without it), and the test's "no
  diagnostic" held only while these verdicts were silent. The copies'
  arms stay behind `leftovers`; with the switch, they change nothing
  either (above).
- **Switched on — 2026-09-28.** With the owner's leave to change the
  scoreboard, `conformance::settings_for` reads the verdict switches;
  under `KG_LEFTOVERS=1` then: conformance 2602/2602, `--strict`
  2681/2681, `errors` agree 694 / disagree 25, `backend`'s 3270 programs
  (every corpus root) the same sets — the gate to switch met on every
  instrument. `Verdicts::leftovers` is on by default, `KG_LEFTOVERS=0`
  the reading to fall back to. `scripts/fast.sh loop --lint` and
  `output`: every ratchet +0/−0. **W1.4 done.**

### W2 — The remaining diagnostics and declines (small, independent)

Each a few assertions, each a commit, each on the fast tier. The counts are
2026-09-27's.

| Item | Rows | Reference |
|---|---|---|
| `deduceAuto`'s rewrite half: "Could not deduce function return type" / "type" | 4 silent — **done** (Progress, row 1) | `Type.hs:1595`, wired at `Frontend.hs:58` |
| Folding inside `static for` and copies: array dimensions | 4 silent, 5 backend declines — **done** (Progress, row 2) | `Constexpr.hs` `evaluateConstexpr`, `TypeCheck.hs` dims |
| A `static if` predicate that does not fold to a boolean | 1 silent, 1 decline — **done** (Progress, row 3) | `Constexpr.hs:41-47` |
| A `static assert` that does not fold | 2 declines — **premise false** (Progress, row 4) | `Frontend.hs` pass 16 |
| An interpolation echo over a non-name | 3 declines — **done** (Progress, row 5) | `interpolatedStringPre`, `Frontend.hs:233` |
| Parse-time symbol-table shapes (`using X<…>`, a template-template parameter spelled wrong, a function template's `using`) | 4 silent — **done** (Progress, row 6) | `Parser.hs`, `SymbolTable.hs` |
| One-offs: a function-typed conversion, `unresolved` → `bool`, a capturing closure at global scope (backend Error 27 today), `ecc` operands, an undefined symbol inside an imported file | 5 silent — **done** (Progress, row 7) | per snippet |
| Warnings: `initializer-overrides`, and `--Werror` over the warning set | 5 strict — **done**, 6 (Progress, row 8) | `TypeCheck.hs:57-60`, `Warning.hs:23-32` |
| Walk declines: unsorted path, `auto`, a call through a variable | 11 — **done**, 6 of 9 plus 2 (Progress, row 9) | `ParseTree.hs` |
| Divergence #32: the six silences the twin's deletion left | 6 — two more **done**, three left (Progress, row 10) | `known-divergences.md` #32 |
| Two backend disagreements on `agilex-7` `dsp.k` (Error 2 against 1) | 2 — **done**; not the walk (Progress, row 11) | the walk |

**Exit:** `--strict` ≤ 8, `errors` silent ≤ 5, `backend` declined ≤ 12.
The 18 parse-recovery positions (divergence #30) stay booked as a design
decision.

**Progress.** (Readings are `scripts/fast.sh walk` in the row's worktree;
the baseline 2026-09-27 was strict failing 34, `errors` silent 28 /
disagree 26, `backend` declined 46, `ptdiff` structural 2373 / declined 5.)
- **Row 1 — 2026-09-27.** The four silences were not `deduceAuto`'s:
  pass 17's two function clauses were ported and waiting on verdicts
  upstream of them. `return-type-polymorphism.k::19` — pass 4 read a
  `return` as a context that informs a template call even in a bare-`auto`
  function, whose `return` is never stamped (`Type.hs:1121,1161-1166`), so
  the LT verdict at `Fn_T_N()` was declined and pass 17 had nothing to
  read. `type-constexpr.k::37` — pass 16 left `static if (decltype(main)
  == …)` alone, where `main`'s type is that `static if`'s (`Type.hs:
  1211-1217`): a cycle, now reported. `callbacks.k::68,69` are pass 19's
  `check AutoF` (`TypeCheck.hs:51`) in a class's copy — the argument
  `(uint32) -> auto` substituted into `using callback_t = T;` — now asked
  by the loop's check (`kg_sem::check`). strict failing 34 → 30, `errors`
  silent 28 → 24 (agree 669 → 673), `backend` declined 46 → 42 (agree
  3215 → 3219); `ptdiff` unchanged.
- **Row 2 — 2026-09-27.** `checkArraySize` (`TypeCheck.hs:187-196`) now
  reports a dimension nothing can fold, not only one that folds wrong: at
  the module's text a runtime name (`constexpr.k::166`, a class's mutable
  member) and a constant computed from a `static for` that stays a loop
  — `inductionVarDependency` finds no trigger, so `UnrolledForF`
  (`Constexpr.hs:50-84`; `kg_const`'s port had no caller until now)
  (`static-for.k::10`); in a copy, a dimension the loop folded to
  anything but a positive integer (`template.k::30`,
  `nested-templates.k::9`), at the `[` the text hands the loop's check
  (`looped::dimension_bracket`). The fifth decline, `constexpr.k::121`,
  is an accepted program: the walk left an enum constant bound to a
  copy's `auto N` unread; `Constexpr.hs:154-157` makes every dimension of
  an array with an enum one an untyped literal, which also fixes
  `enum.k::39`'s walk. strict failing 30 → 26, `errors` silent 24 → 20
  (agree 673 → 677), `backend` declined 42 → 37 (agree 3219 → 3224),
  `ptdiff` structural 2373 → 2376, declined 5 → 4.
- **Row 3 — 2026-09-27.** `static-if.k::26`: the loop's check already
  had the arm (`unresolved StaticIfF{}`, `Frontend.hs:406`) behind
  `KG_LEFTOVERS=1`, gated because a copy of `data.memory.bank.schedule`
  leaves a `static if` this loop cannot fold and the reference can. It is
  now asked where the predicate certainly reads a runtime value — a
  parameter, a local not declared `const`, under no absorbing operator or
  mux (`Cx::certainly_runtime`, the IR's `facts::certainly_runtime`) —
  which no round folds. The one other `static if` silence,
  `type-constexpr.k::37`, was row 1's. strict failing 26 → 25, `errors`
  silent 20 → 19 (agree 677 → 678), `backend` declined 37 → 36 (agree
  3224 → 3225); `ptdiff` unchanged.
- **Row 4 — 2026-09-27: premise false, nothing ported.** The two declines
  are `basics.k::4,5`, `static assert((3 << -1) == 1)` and `((3 >> -1)
  == 6)`, both `expected:0` — no pass-16 verdict exists for them. The
  reference *crashes*: GHC's `shiftL`/`shiftR` raise `overflowError` on a
  negative count (`Constexpr.hs:190-191`), and the recorded run is exit 1,
  no `Error` line, `kanagawa: arithmetic overflow` (`target/ptrace/traces/
  syntax/basics/4/run.json`; `kg-oracle --errors` the same). The corpus's
  `expected:0` presumes a GHC that shifted the other way. This port
  compiles the frontend clean and the walk declines the unfolded shift —
  the booked state (journal 2026-09-24, "Correction: a negative shift
  crashes the reference"). Moving it needs an owner's call on which
  reference is right: the binary, or the corpus. **Owner's decision,
  2026-09-27: deferred.** Match the reference where it succeeds; where it
  fails, as here, the case waits. The two declines stay booked and are
  not counted against W2's or S3's exit.
- **Row 5 — 2026-09-27.** `interpolatedStringPre`'s echo was spelled in
  the loop's pass 1 (`kg_sem::pass1`) for a name only, by its leaf. It is
  now `showPretty` (`PrettyPrint.hs:36-178`) over the pass-1 tree for
  literals, operators, sizes, `clog2` and calls of a name —
  `interpolated-string.k::16`'s `{intConst+3=}`, `test/logic/string.k`'s
  `{2+2=}` and `{bitsizeof val=}`, `test/library/data/bits.k`'s
  `{reduction_nor(x)=}` — and a name keeps the qualifier
  `redundantNameScope` leaves (`{E::A=}` is `E::A = `, where the leaf was
  printed). `ptdiff` structural 2376 → 2379, declined 4 → 1; `backend`
  declined 36 → 33 (agree 3225 → 3228); `fast.sh loop` green
  (`infer-identity` 304/304, `loop-left` and `edit-identity` unchanged).
- **Row 8 — 2026-09-27.** Two mechanisms. `overridingInitializerWarning`
  (`TypeCheck.hs:57-65`) is ported into pass 19 (`validate_program.rs`,
  `overriding_initializer_warning`): a designator naming a field an
  earlier one in the same list named warns at its `.`, the note at the
  nearest earlier one — byte-identical to `kg-oracle --errors --Werror`
  on `designated-initializer.k::41`. A list in a template body is left
  to the copies, which do not ask it (a silence: the reference warns once
  per instance). Conversion's `poisoned` now counts errors only
  (`errors` lists `ErrorF`, `Error.hs:88-92`), or the new warning hid
  `{.x = 10, .x = 11}`'s conversion error. `--Werror`
  (`Main.hs:93-99`) is the driver's, and `Db::check_snippet` — the
  snippet protocol the scoreboard calls — now returns a clean program's
  warnings as errors under it (the CLIs already printed the driver's
  line). Six, not five: `callbacks.k::10` is `--Wconversion --Werror`.
  `scripts/fast.sh --lint walk`: conformance 2580 → 2583, strict failing
  25 → 19; `errors`, `ptdiff`, `backend` unchanged.
- **Row 9 — 2026-09-27.** Nine declines, three mechanisms; the premise
  held for six, and three are programs the reference rejects.
  *Unsorted path*: `obj.template Go<uint32>` and `x.Do<uint32>` outside a
  call are `methodInstanceSpecifier` (`Parser.hs:766-770`, tried before
  `value` at `:849`), a `FunctionSpecifier` with its receiver — the
  lowering made a member access over an unsorted path, on the belief
  (`field_member`'s comment) that the form never parses; now it is the
  callee a call builds, the loop makes the instance, and
  `dependent-template.k::9,10`, `template-param-default.k::39` walk.
  *`auto`*: `isAbbreviatedTemplate` holds of a `FunctionDeclF`
  (`Parser/Syntax.hs:722-723`), so `removeTemplate` deletes `void Do(auto
  x);` too (`extern-class.k::15,37`). *A call through a variable*:
  `resolvedFuncParams` sees a function type through an alias
  (`Function.hs:133-141`); pass 2 now resolves one through the program's
  symbols (`kg_sem::pass1::heads`, `higher_order::apply_with`), so
  `higher-order-functions.k::22`'s `Do(Callback …)` is a template. The
  walk test that used that shape for the decline now uses `decltype(g)`,
  which no rewrite reaches (the reference rejects it; this frontend is
  silent). Left: `dependent-template.k::12`, `using.k::37`,
  `function-template.k::18` are rejected by the reference (the first a
  conversion, the other two its parser, `kg-oracle --errors`) — rows 6–7's
  silences; `dependent-template.k::12` is now reported, since the
  lowering types the value `() -> uint33` (errors agree). `callbacks.k::48`
  (a `deduce: Unresolved` decline) walks as well. `scripts/fast.sh --lint
  loop`: `ptdiff` structural 2384 → 2391, diverged 37 → 30; `backend`
  agree 3233 → 3241, declined 28 → 20; strict failing 19 → 18, `errors`
  silent 19 → 18 (agree 678 → 679); `infer-identity` 304/304,
  `loop-left` 321/321, `edit-identity` and `resilience` unchanged.
- **Row 10 — 2026-09-27.** Five remained after row 3.
  `template-template-parameter.k::36,37` — `FnFn<uint32, T, Do>` in `Go`'s
  copy with `T` a type — are closed: a type given for a template-template
  parameter fails `templateArgsResolved`'s `tIsTemplate`
  (`TypeCheck.hs:323-329`), so the instance is never made, while
  `unresolveArgError`'s `templateArgResolved` accepts the type
  (`Frontend.hs:423-426`) and pass 12 says "Failed to instantiate
  template". A type argument is final, so the loop's check asks that arm
  there without `KG_LEFTOVERS` (`kg_sem::check::Cx::never_instantiated`);
  identical to `kg-oracle --errors`, frames included. Left:
  `constexpr.k::132` (`index_t<N>` over a lambda's capture, "Unresolved
  argument") and `template-argument-deduction.k::143` (a method template
  of a class copy called at the module's text, LT) — pass 12 at the
  module's own text, which the loop's check does not judge even with
  `KG_LEFTOVERS=1`, and whose verdict would have to be dropped when the
  CST run's pass-9 checkpoint closes (`Frontend.hs:42-52`) — a seam of its
  own; `function-template.k::18` is the reference's parser (rows 6–7).
  `scripts/fast.sh --lint walk`: conformance 2584 → 2586, strict failing
  18 → 16, `errors` silent 18 → 16 (agree 679 → 681), `backend` agree
  3241 → 3243 (declined 20 → 18); `ptdiff` unchanged.
- **Row 11 — 2026-09-27: premise half false — not the walk.** The two
  are `library/device/agilex-7-f-023/hardware/dsp.k` and
  `stratix-10-gx-2800/hardware/dsp.k` compiled as roots under `mock`, and
  the reference rejects both at *parse time*: each declares `hardware.dsp`,
  which `base.k` also imports (from `device/mock/`), and exports an
  `fmul32` — `checkSymbols`' `findSymbol` (`Parser.hs:219-223`) is the
  module's own symbols then every import's (`SymbolTable.hs:228-237,261`),
  two definitions: "symbol is ambiguous", and so "undefined symbol", at
  the export. This frontend accepted them and the backend said "Duplicate
  function" (Error 2). The catch is `nub`, which compares the trees with
  notes ignored (`Parser/Syntax.hs:594-595`): `arria-10-gx-1150`'s and
  `stratix-10-nx-2100`'s declarations are `mock`'s own, one definition,
  and those compile to the backend's Error 2 in the reference too — a
  first port without it traded the two disagreements for those two.
  `kg_resolve::ModuleGraph::exposed_ambiguities` finds the candidates;
  `Db::exposure_ambiguities` decides them by the sorted lowering's
  position-free fingerprint (the sorter's own `nub`), lowering the program
  only when a candidate exists. Messages byte-identical to `kg-oracle
  --errors`. It also closes `imports.k::63` — `a1.a15` exports its own
  `memory` beside `data.memory`'s — a row-7 one-off ("an undefined symbol
  inside an imported file"). The library-acceptance test
  (`the_rooted_door_accepts_every_file_of_the_reference_library`) asserted
  every library file compiles; the reference rejects these two, and it now
  names them. `scripts/fast.sh --lint walk`: conformance 2586 → 2587, `backend` agree 3243 → 3246
  (backend disagreements 4 → 2, frontend 3 → 2), strict failing 16 → 15,
  `errors` silent 16 → 15 (agree 681 → 682); `ptdiff` unchanged.

- **Row 6 — 2026-09-27.** Three parser verdicts the table decides
  (`undefined_symbols::parse_time_find_symbol`, all `PARSE-ERROR`s, so
  booked "parse-time table"): `Foo<10>` where the innermost declaration
  the parse-time table holds is a nested non-template — a class's
  variable, alias or type member wherever written, since `class_` seeds
  the class scope from a body-skipping look-ahead (`Parser.hs:316-329`;
  `using.k::37`'s `using Foo = Foo<10>;` names the member it declares),
  a local before the mention, a parameter; a template template
  parameter's default that `templateSpecifier` finds no template for
  (`Parser.hs:368,696-702`; `template-template-parameter.k::23`); a
  function template instance as a variable's, parameter's or alias's
  type (`typeSpecifier`, `Parser.hs:712-721`; `function-template.k::18,
  19`). strict failing 25 → 21, `errors` silent 19 → 15 (parse-time
  table 52 → 56), `backend` agree 3233 → 3237 (declined 30 → 26);
  `ptdiff` unchanged (`scripts/fast.sh walk --lint`).
- **Row 7 — 2026-09-27.** Five mechanisms, one per snippet, each
  `kg-oracle --errors`-probed: `dependent-template.k::12` — `obj.template
  Go<uint33>` outside a call is `methodInstanceSpecifier`'s
  `FunctionSpecifier` (`Parser.hs:765-769`), which `kg-hir` lowered as a
  `MemberAccess` with an unsorted member; lowered as the method specifier,
  the loop instantiates and types it and the conversion is checked (and
  `::9,10`, `expected:0`, now walk); `basics.k::110` — `bitsizeof` of a
  class never folds (`tWidth TClass = Nothing`), so the source is
  `unresolved` however this table reads `==` (`Conversions::
  certainly_unresolved`); `higher-order-functions.k::195` — `reduceClosure`
  over a closure a *call* returns, in the loop's check (`TypeCheck.hs:
  146-148`); `memory.k::19` — pass 10 over the `Binary` an `op=` desugars
  to, which the text has no node for (the backend's crash on it is gone);
  `imports.k::63` — an exported name the module declares *and* an
  unaliased import exposes is two answers to `findSymbol`, so "undefined
  symbol" (`SymbolTable.hs:231-237`), `nub` merging declarations equal but
  for their notes, which one namespace is counted as. strict failing
  21 → 16, `errors` silent 15 → 10 (agree 678 → 683), `backend` agree
  3237 → 3244 (declined 26 → 23, crash 2 → 1), `ptdiff` structural
  2384 → 2386; `scripts/fast.sh loop --lint` green (`infer-identity`
  304/304, `loop-left` 321/321, `edit-identity` 87/87). Left: the ten
  other silences — `constexpr.k::1,132`, `type-check.k::54,55`
  ("Unresolved argument for the parameter …"),
  `template-argument-deduction.k::143,216` and
  `template-template-parameter.k::24,36,37,38` ("Could not deduce all
  arguments", "Failed to instantiate template": W1.4's arms).
- **Beside row 7 — 2026-09-27: alias- and module-qualified names.**
  `test/library/processor/risc-v/testbench.k` was a `backend` decline
  ("template instance") because pass 9 declined any path led by an import
  alias; `Config::HARTS` over `import .cmdargs as Config` with no
  `--define` is "Symbol name not in scope: `HARTS`" in the reference.
  `undefined_symbols::alias_qualified` asks what the aliased import exposes
  (`findInImport`, `SymbolTable.hs:272-279`) and walks members as
  `qualified_member` does; and a qualified value whose head nothing
  answers — `Foo::Bar`, a module path `data::bits::NOPE` — is reported by
  its last name, at the last segment for a value and at the path for a
  callee (`Frontend.hs:97-102`, the parser's notes), where it named the
  head. `backend` agree 3244 → 3245 (declined 23 → 22); the snippet gates
  unchanged (none spells either shape); `scripts/fast.sh walk --lint`
  green. `query.rs`'s test-library walk had called `testbench.k` clean
  and is corrected (the reference also reports its bare `ENABLE_M`, which
  this port does not).
- **The seven semantic `errors` disagreements — 2026-10-10.** Four
  mechanisms, each `kg-oracle --errors`-probed. `imports.k::2` — a file
  that exists but does not decode is GHC's read failure, `<path>:
  hGetContents: invalid argument (cannot decode byte sequence starting
  from 192)`, not "Can't find file" (`Parser/IO.hs:103-107`); the `Vfs`
  now tells an unreadable file from an absent one, and the compat binary,
  which compiled a non-UTF-8 command-line file clean, prints the
  reference's uncaught exception. `type-check.k::4,14,15` — a type in a
  width never folds, so `unresolved ParamIntF{}` replaces the width at
  pass 16 (`Frontend.hs:404-405`) and pass 19 never sees inside it; the
  replacement is a `cata` (`Desugar.hs:66-69`), so widths are judged
  innermost first and drop what they held. `constexpr.k::6,75` — a
  selector written as literals is a literal when the mux is first typed,
  so an out-of-range one types the mux `TUnresolved` (`Type.hs:1237-1238`)
  and only a name-folded selector reaches "Invalid selector"; pass 12's
  "Unresolved argument" is the error. `basics.k::173` — a memory's one
  dimension is no `many` element (`Parser.hs:529` against `:114,535`) and
  keeps its own note. `errors` agree 694 → 701, disagree 25 → 18 (+7/−0);
  the 18 left are divergence #30's parse-recovery positions. Conformance
  2602/2602, strict and `cli` unchanged (`scripts/fast.sh`);
  `infer-identity` 304/304. `fast.sh walk` not run (the mux change lets
  `uint32 x = mux(4, 0, 1, 2, 3);` compile, as the reference's frontend
  does). Beside them, the same decoder's `utf8_bom`: a file opening with
  a byte-order mark compiles in the reference and was rejected here (no
  corpus file has one). Journal: "Seven `errors` disagreements, four
  mechanisms — 2026-10-10".

### W3 — M6: one engine (`ir-elaboration.md` §2.5–2.6)

Where the editor stands (read 2026-09-27): hover, completion, signature
help, inlay hints and navigation read the CST engine's table and
`kg-db/src/navigate.rs`; only diagnostics read the loop. `kg-lsp` names
neither `kg_sem` nor `kg_hir`. The IR engine has no position index, no
resilient mode, and `Exp::Missing` is not a constructor yet
(`kg-hir/src/exp.rs` has `Error` and `Unsupported`). So every
type-reading feature is to be moved onto an engine that has never served
a broken buffer — the sprint's largest unknown for the tools, and the
reason steps 0 and 5 are built early, beside S2–S3, where they need
nothing from W1.

0. **The resilience gate, first and empty.** `cargo xtask resilience`:
   hover and completion per node over the 76 error-recovered files plus
   fuzzed broken buffers (`xtask fuzz`'s mutators), the CST engine's
   answers recorded as the ledger. The IR side starts at "no answer"
   everywhere; the number is the migration's progress and says early
   whether a corner needs §2.6's narrow editor path. Proven by a mutant
   (an answer swapped) before anything is graded by it.
1. **The base passes on the IR**, one per commit, each with a per-pass
   differential against its CST version over the base modules: 9
   `undefined_symbols`, 10 `type_errors`, 13, 14, 16, 17, 18, 19
   (`validate_program`, conversion, shadow). The copies' versions exist in
   `kg_sem::check`; this makes them the only versions.
2. **`NodeData.anchor`** for the few diagnostics that sit on a token (a
   call's closing parenthesis), filled by the lowering; `errors` grades it.
3. **Total lowering.** Error recovery lowers to `Exp::Error`/`Missing`
   keeping every child it can; 13 of the 76 error-recovered files still
   lower to `Unsupported` — target none.
4. **`Strict` and `Resilient` modes** of the one engine, and the
   **resilience gate** (§2.6): hover and completion per node over the 76
   error-recovered files plus fuzzed broken buffers, IR engine against the
   CST engine.
5. **One position index** (offset → innermost `Own` node) behind hover,
   signature help, inlay hints and completion. Built with step 0: the
   gate's IR side reads through it.
6. **The walk reads one engine.** Its fall-through to the CST table
   (`kg-ptbridge/src/walk.rs:871-897`) and `Db::module_fold`, the CST
   folder it still asks (`program.rs:91`), go: `KG_SEM_ONLY`'s `ptdiff`
   reading equals the default's, then the fall-through is deleted and
   `kg-ptbridge` no longer depends on `kg-db` for types.
7. **Delete** the CST inference engine and the CST `ConstEval` once the
   resilience gate is at parity — a crate removed, since W8.3 has split
   it from the type model by then — and with them the second census,
   `kg-db/src/instances.rs`, and the `xtask ir` lines that grade it (the
   register's "independent second reading" ends at M6,
   `single-source-of-truth.md`).

**Exit:** resilience gate at parity; `kg-ty`'s CST engine deleted;
`errors` positions unchanged; the walk's types from the loop alone.
**Met 2026-10-10** (step 7 below, with what parity was taken to mean).

**Progress.**
- **0 — done 2026-09-27.** `cargo xtask resilience` (`xtask/src/resilience.rs`):
  hover and completion at sampled nodes — at most 48 identifiers and 8
  carets per buffer, spread evenly in text order — of the 76
  error-recovered corpus files (every `.k` whose whole-file parse errs:
  73 `test/syntax/*.k` accumulators, one `imports/a1` module, two
  `chkdoc` contexts) and 64 of `xtask fuzz`'s broken buffers laid over
  clean files as editor overlays; the editor's settings bound
  (`mini-base`, `PassBudget::INTERACTIVE`), a `Db` per thread. The CST
  engine's answers — what `kg-lsp`'s handlers read: `definition_at` +
  `type_of_definition`, `completions_at` + each candidate's detail — are
  the `cst` set, the ledger's reference; `agree` (pass), `disagree`,
  `ir-only`, `failed` (fail). First reading: 5058 answers graded in
  ~1.5 s, <1 GB; the IR side answers nothing (hover 1716 IR-silent, 2360
  that neither types, 1422 declined by both; completion 974 IR-silent,
  8 neither). Proven red, in a throwaway copy with its own blessed
  baseline, by three mutants: the CST hover typed at the declaration's
  parent (2690 `cst` items left), the IR side answering `bool` (2604
  `disagree` + 2368 `ir-only` entered), every imported completion
  misspelled (965 left). Its first run found the completion order
  depending on the session's interning history (`complete.rs`, fixed; the
  journal). In `gates.sh` and `fast.sh loop`; **needs a first bless**.
- **5 — done 2026-09-27.** `kg_hir::position::PositionIndex`: `build(&ModuleIr)`
  over the `Own` nodes at ROOT in the module's file that the root
  reaches; `at(offset)` the innermost (smallest range, then deepest in
  the tree), `enclosing(offset)` outward from it, `node(range, kind)` the
  `TypeKey`-shaped lookup. Ranges are CST ranges, so they nest, and the
  query is a binary search and a walk up a parent chain. `Looped::index`
  builds one per module of the loop's final tree on first ask;
  `Db::ir_type_of_definition` reads the loop's type at a declaration
  through it — the gate's IR side. Seven unit tests in `position.rs`,
  two in `navigate.rs`, each proven by a mutant (ten mutants, all
  caught). Gate reading through it: hover agree 958, IR-silent 758
  (mostly declarations the loop's tree no longer holds — a redefinition
  in an accumulator, a declaration a mutant broke), IR-only 519, neither
  1841, disagree 0; completion agree 538, IR-silent 326, IR-only 101,
  disagree 9 (`basics.k`'s `auto y = x << amt`: `int63` against the
  loop's `uint63`; `constexpr.k`'s `c`: `const int8` against
  `const uint1`) — the owner's to adjudicate. The editor does not read
  the index yet; hover, signature help, inlay hints and completion move
  onto it with the base passes (steps 1–4). **Bless this reading, not
  step 0's**: against step 0's it is +1496 `agree` and 629 unadjudicated
  entries (the 9 disagreements, 620 IR-only).
- **5, the editor on it — 2026-10-10.** Hover, inlay hints and signature
  help read the loop first and the CST engine only where the loop has no
  type to show: `Db::editor_loop` (the loop the buffer's diagnostics run —
  the same memo, and none where they stop before it, a buffer that does
  not parse), then `Looped::answer_at` / `Db::ir_answer_of_definition`
  (the written tree's type, or inside a template's body each instance's
  copy's, through `kg_hir::position::CopyIndex`), `Looped::callee_at` for
  the instance a call compiles. Inside a template's body every distinct
  type is listed with the instances that give it — hover `uint9 w   //
  take<uint8>` over `uint17 w  // take<uint16>`, the hint `auto w: uint9 |
  uint17`, the signature's documentation `(uint16) -> void take  //
  take<uint16>` — and the list says when it may be short (an untyped copy,
  the editor's budget); `kg-lsp/src/handlers/instances.rs`. `Db::looped`
  runs once per program however many ask (an inlay hint racing the
  diagnostics waits for their loop), detached from the request that
  started it (`kg_base::cancel::detached`). Why the editor reads no loop on
  a broken buffer: a keystroke that breaks `risc-v.k`'s parse costs 57 ms
  of diagnostics and the loop there another 870–930 ms (the journal).
  Completion's details still read the CST engine. The 9 completion
  disagreements: the loop is right in all 9 (`kg-oracle --passes 19` per
  snippet; one mechanism: a key the broken accumulator declares twice,
  which the CST engine answers from a later snippet's declaration and the
  loop from the first, as `getSymbols''` does — the journal). Resilience, the hover
  IR side reading the editor's query: agree 1496 → 1497, the new pass set
  `copies` 0 → 134 (hovers inside a template's body the CST engine never
  types), every other set unmoved, `editor-reads-loop` 0 (every buffer
  fails to parse); **bless `resilience`**. Keystroke bench (`cargo xtask
  bench --filter keystroke`, one pass each under load average 11–19):
  risc-v 995 ms before, 957 ms after — the diagnostics run no more loop
  work. Driven over stdio (`kg lsp`, `test/library/data/array.k`): a
  hover on `TestPrefixSumHelper`'s `naturals` lists its six instances'
  `uint2[4]` … `uint5[24]` in 6 ms once the diagnostics' loop is in.
- **5, completion and member navigation on the loop — 2026-10-10.**
  Completion's details read the query hover reads
  (`Db::ir_answer_of_completion`: the loop's type at the candidate's
  declaration — inside a template's body every instance's, the detail
  `uint9 | uint17`, the documentation naming which instance gives which).
  Member lists (`s.`) and go-to-definition on `s.f` read the loop's type of
  the base (`Looped::members`: the aggregate's declaration in the loop's
  program, an instance's copy included, each member typed as that instance
  has it, mapped back to the declaration it was written as by
  `Db::definition_of_declaration`): `static Box<4> a; a.` offers `items`
  `uint32[4]` and `first` `() -> uint33`; `auto p = make<uint8>(1); p.`
  offers `Pair`'s fields and `p.lo` jumps to `Pair::lo` — the CST engine
  typed neither base and offered nothing; inside `take<T>`'s body, `b.`
  (`static Box<bitsizeof(T)> b`) offers `items` `uint32[8] | uint32[16]`. A
  field two instances declare apart is no jump. The CST engine answers
  where the loop has no type, on a buffer that does not parse, and —
  completion only — until the diagnostics have run the edit's loop:
  completion reads it once computed (`Db::editor_loop_ready`) and never
  waits, since waiting made a clean keystroke's completion on `risc-v.k`
  ~2.2 s against ~100 ms (stdio, 2026-10-10); navigation waits, as hover
  does. The resilience gate's completion side grades what the editor
  serves (the details by that query, member lists the loop's): agree 1497
  → 1499, completion ir-silent 326 → 324, every set otherwise unmoved, 9
  member lists only the loop fills (`completion-loop-only`, `--verbose`
  lists them), 1593 candidates only the copies type; **bless
  `resilience`**. The nine adjudicated disagreements stay in what the
  editor serves on those buffers: a buffer that declares a key twice never
  parses, so the editor reads no loop there until W3.4. The race W3.5 left
  is fixed: the parse, item-tree and program memos adopt what a racing
  thread stored first, one program per input state, one loop
  (`threads_loading_one_program_at_once_run_one_loop`). Keystroke bench,
  interleaved A/B (`cargo xtask bench --filter keystroke`, risc-v worst,
  load 7–16): before 981 / 941 ms, after 945 / 1040 ms — noise; the
  diagnostics run no new work. Driven over stdio (the release `kg lsp`,
  `members.k` and a copy of `risc-v.k` under a manifest importing
  `library/`). `scripts/fast.sh loop` green: conformance 2602/2602,
  `infer-identity` 304/304 and `edit-identity` 87/87 identical (the memo
  change exact), every ratchet neutral but `resilience` +2. Left before
  the CST engine can go, for the editor: the loop
  on broken buffers (W3.4) — a member list typed fresh, `a.` mid-statement,
  is still the CST engine's, since the buffer does not parse; method
  navigation (`c.Go()`, which neither engine answers); the CST table's
  remaining readers — `Db::type_of_definition` as every feature's
  fallback, `members_of`, `field_definition`'s second half, the inlay
  hints' fallback — which go with W3.6–W3.7 once the gate's `ir-silent`
  (757 hover, 324 completion) is at parity.

- **5, methods and members through the receiver — 2026-10-10.**
  Go-to-definition on a method after a `.` — which neither engine answered
  — reads the loop's type of the receiver (`method`, `Type.hs:1358-1362`)
  and lands on the class's text: `c.Go()`; `a.Get(0)` on a `Box<4>` and
  through `using Box4 = Box<4>` on `Box`'s `Get`; `fixture.Run<3>(t)` and
  `a.As<uint3>(i)` on the member template; a method holding a lambda,
  which `desugarLambdas` rebuilds `Donated` and `Looped::members` had
  dropped (completion after `test.` in `test/library/control/loop.k`
  offered `flag`, not `Run`). Where the receiver's instances reach several
  declarations — `Go` in each branch of a `static if`, reached inside
  `run<F>`'s body; a field two instances declare apart — `Db::definitions_at`
  lists each and go-to-definition answers `Location[]` in the text's
  order; `definition_at` declines there (references, rename, signature
  help). Hover on a member is the receiver's instance's member
  (`a.Items()` `() -> uint32[4]`, not `Box<9>`'s beside it; per enclosing
  instance in a template's body), a member template's call the instance it
  compiles (`(uint32) -> uint32 Run  // Run<3>`, shared with signature
  help), several declarations named and counted. References, highlight
  and rename find uses through receivers the loop types; a rename refuses
  where it cannot place one (a `.Go` reaching two declarations by
  instance; a method call in a buffer that does not parse) and for a
  member of an aggregate an unopened importer may hold. Navigation gates
  on the root buffer's diagnostics, not on the name's file (a references
  search ran the loop through a clean import under a broken buffer).
  Tests: 4 new in `navigate.rs` and one extended, 5 e2e sessions, 3 in
  `rename.rs`, each proven red by `scripts/mutant.sh` (22 runs, all
  caught; one first survived — the test's caret sat on the declaration,
  not the use — and the test was fixed). Driven over stdio (the release
  `kg lsp`, the four shapes' fixture and a copy of
  `test/library/control/loop.k` under `library/`): each request ≤ 6 ms once
  the diagnostics are in. `resilience` neutral and not extended: it
  grades hover at the declaration and completion, not where a jump lands,
  and its buffers never parse, so the editor reads no loop there.
  `scripts/fast.sh loop` green: conformance 2602/2602, `infer-identity`
  304/304 and `edit-identity` 87/87 identical, every ratchet +0/−0
  (`resilience` agree 1499, copies 134). Left: method navigation on a
  buffer that does not parse (W3.4 — the CST half answers fields only);
  a template body the editor's four rounds have not instantiated has no
  receiver type, so no jump there, and a rename misses a use in it
  (documented, not refused — the base library's uninstantiated bodies
  would refuse most renames).

- **6 — the walk's types are the loop's alone — 2026-10-10.** `type_of`
  no longer falls through to the CST table where the loop says
  "unresolved", and `note` asks the loop first, the table only for a walk
  with no loop behind it (none in production); `KG_SEM_ONLY`, the switch
  that measured the fall-through, is gone with it. Each step graded by
  `scripts/fast.sh walk`: `ptdiff` 2422 structural and `backend` 3267
  agree, +0/−0 — the table decided nothing. **Not the exit yet**: the CST
  constant folder (`Db::module_fold`) still answers `folded` where the
  loop's tree holds no literal — without it `constexpr.k::143,144`,
  `conversion-warning.k::14,15` and `enum.k::42` diverged (measured).
  The first two were the loop's own miss, closed the same day: `x & 0`'s
  `sizeOf y` is the zero's node's type (`Constexpr.hs:231-232`), and a
  substituted `const uint32 x = 0` wears 32 bits where the port read the
  value's minimal width. The last three were an enum constant converted
  at a declaration (`uint3 x = Foo::One;`), which the walk now reads off
  the loop's `EnumValue` (`Constexpr.hs:292`). **W3.6's exit met the same
  day**: the walk's `Module` takes the loop's module, required, and no
  longer the CST engine's table or folder; `Db::module_fold` is deleted.
  `scripts/fast.sh walk`: `ptdiff` 2422 structural, `backend` 3267
  agree, +0/−0.

- **3–4, the editor on a buffer that does not parse — decided
  2026-10-10.** Measured first (the journal, "The editor on a buffer that
  does not parse"): the broken buffer's own loop costs 234–273 ms on
  `risc-v.k` under the bench's settings and 680–687 ms under the recorded
  ones, against 23–26 ms of diagnostics for that keystroke, and completion
  — which never waits — could not use it at the `.` that asks. So **(b)**:
  every reader answers from the last loop computed for the program,
  positions mapped through a line diff (`kg_db::text_map`); a base typed
  fresh (`a.`) by its name's declaration; and **(a) once** for a buffer
  with no loop at all, which a hover runs.
- **3–4, built — 2026-10-10.** `kg_db::text_map` (a line diff refined by
  characters; a range maps where its first and last characters are
  unchanged), `kg_db::looped::EditorLoop` (a loop and each changed module's
  map, every question mapped in and every member mapped out), and three
  readers: `Db::editor_loop` (hints, signature help, navigation — the last
  loop on a broken buffer), `Db::editor_loop_ready` (completion — the
  newest loop, mapped, never waiting), `Db::editor_loop_or_own` (hover —
  and a buffer's own loop, once, where it has none); `Db::base_answer`
  types a base by its name's declaration (`typeOfSymbol`,
  `Type.hs:1243-1245,1336`). The diagnostics are untouched: a broken
  buffer's gutter is its syntax errors. Over stdio (release `kg lsp`,
  `risc-v.k` under a manifest importing `library/`): `MULHU.` typed in
  `DisasmTest` lists `Instr`'s six formats in 18–29 ms as the edit lands,
  where `211c2ea` answered `[]` in 44 ms; `a.` at the start of a
  `Runner<4>` method lists `Box<4>`'s members and the moved `g` hovers as
  `uint33  // Runner<4>`. The resilience gate's IR side is now what the
  editor serves (fuzzed buffers over their clean file's loop, the
  never-parsed ones through their own): hover agree 959 → 964, ir-silent
  757 → 764; completion agree 540 → 495, ir-silent 324 → 375; disagree 9
  → 9; `editor-reads-loop` 0 → 61, `editor-runs-own-loop` 79 — the 57
  `agree` items lost (49 completion lists, 8 hovers) all text a mutation
  wrote or reshaped, which no loop has seen; 10 of the 44 new `ir-only`
  checked against `kg-oracle --typed --passes 19`, all the reference's
  (the journal). **Bless `resilience`** (a regression in the ratchet's
  terms, +17/−57 `agree`: the gate's IR side was a loop no editor read).
  Keystroke bench interleaved against `211c2ea`: 303.0 / 306.6 ms before,
  304.4 / 312.2 ms after — noise. Tests: 7 in `text_map`, 3 in
  `complete.rs`, 1 in `navigate.rs`, 2 e2e sessions, and the three that
  pinned the old policy (a broken buffer reads no loop; an edit makes the
  loop unreadable) corrected to the new one; 13 mutants, all caught. A
  member's rename refuses while a buffer in its search reads its loop
  through an edit (a receiver the edit retyped reads as it last parsed;
  without the refusal the rename rewrote a use of another class's member). Not
  done: W3.3's total lowering — the never-parsed files are served by their
  own loop as they lower today (13 of the 76 with `Unsupported` holes);
  the `Strict`/`Resilient` modes are, as built, the gate before the passes
  and the loop past it: one engine, the diagnostics stopping where the
  reference does.

- **The editor's budget invents no error — 2026-10-10.** At four rounds
  `test/library/control/loop.k` showed 25 errors `kg check` does not —
  "Failed to instantiate template", "Could not deduce all arguments",
  "Unresolved argument …" — pass 12 judging instances the next round
  would have made; the leftover arms switched on in W1.4 had made the
  editor's truncation visible. `PassBudget` now tells the editor's latency
  bound (`Interactive`) from the reference's `--template-iterations`
  (`AtMost`, under which the reference does report these): a loop the
  former stops leaves pass 12's undecided arms and the standing-`static
  if` verdict unjudged (`kg_sem::check::checked`'s `truncated`), the
  undefined template, too many arguments and an instance no round makes
  still reported. `the_editor_s_round_budget_invents_no_error` (kg-db,
  over the corpus file) and the compat binary's `--template-iterations`
  test each go red under the other direction's mutant. Every ratchet
  +0/−0 (`fast.sh loop`).

- **1, the instrument and pass 14 `validateExtern` — 2026-10-10.**
  `cargo xtask passdiff --pass <name>` (`kg_db::passdiff`,
  `xtask/src/passdiff.rs`): over the 304 corpus roots and 2922
  conformance snippets, in every module whose CST run reaches the pass,
  the CST pass's verdicts — run in its place, over a copy of the sink as
  it stands there — against the loop's at that module's text
  (`Looped::text_for`), each diagnostic compared whole; red on any
  difference, ~9 s. Proven red before it graded anything: the loop's pass
  accepting a struct, 1 program cst-only; the CST side recording nothing,
  2 ir-only. The loop's pass 14 (`kg_sem::check::validate_extern`) is the
  reference's: `lookupSymbol` from the name's scope, a `Class` or
  `Template _ Class` kept, anything else refused, a failed lookup
  included, wherever `desugar` reaches — a template's own body, a
  standing `static if`'s branches, a copy (framed). **First reading:**
  reached in 36806 module runs of 3099 of 3226 programs, the CST pass
  giving 2 verdicts — 3099 identical, 0 differ. **Switched:** the
  loop's verdicts at a module's text are admitted in pass 14's place
  (`passes::FromLoop`, `judge_text`), behind the same checkpoints the CST
  pass ran behind; `KG_CST_TEXT` (`Verdicts::cst_text`) falls back to the
  CST pass until its deletion, the next commit. `scripts/fast.sh loop`
  green, every ratchet +0/−0 (conformance 2602/2602, `errors` agree 701,
  `ptdiff` 2422, `backend` 3267, `resilience` agree 1499,
  `infer-identity` 304/304, `edit-identity` 87/87). New over the CST
  pass, none in the corpus, each `kg-oracle --errors`' first error: an
  `extern` of a struct in a template's body, in `static if (true)`, in a
  copy; of a struct template's instance; of a template's parameter. The
  journal, "The passes after instantiation onto the loop, one at a time".

- **1, pass 10 `typeErrors`; pass 14's CST code deleted — 2026-10-10.**
  `passdiff --oracle` puts each difference in a snippet to `kg-oracle
  --errors` (the `errors` gate's reading, cached) and passes where the
  reference gives every verdict only the loop gives and none only the CST
  pass gives — proven red by a mutant (the adjudication inverted: 2
  unsettled). The loop's pass 10 (`kg_sem::check::type_errors`) is `cata
  typeErrors` over the loop's types at each module's text: the innermost
  error-typed node, a written one, as `showPretty` spells it, quiet above
  any error; its range moved out over enclosing parentheses where the
  text is (`looped.rs`'s `paren_note`, `Parser.hs:836,845`). **First
  reading:** the CST pass giving 47 verdicts, every one shared; 2
  programs differ, ir-only 2 — `auto-placeholder.k::33,34`'s `x / -1`,
  `x % -1`, "Division/Remainder with negative denominator is undefined",
  both the reference's second error after pass 17's: settled for the loop
  (`--oracle`, 0 unsettled). The CST pass had stood down there (its
  `fold_divergent`). **Switched**, the reading after the same. Pass 14's
  CST module (`passes/validate_extern.rs`) deleted, with its unit test
  (the loop's in `kg_sem::check` cover it); `FromLoop` now always carries
  the loop's text verdicts and says separately whether a moved pass whose
  CST code remains is the CST's (`cst`: the text door, `KG_CST_TEXT`).
  One test corrected with its citation (`deduce_auto`'s
  `a_division_by_a_non_positive_literal_has_no_type_to_deduce`: the
  reference's second error). `scripts/fast.sh loop` green, every ratchet +0/−0 (the two new
  verdicts are second errors: `errors` grades the first). A moved pass
  with no CST code left is refused by `passdiff`, which would read it
  identical for want of a CST side (`kg_db::passdiff::PROBED`, held to
  the CST run by a test).
  Not ported: pass 10 inside a copy — no version of it exists on either
  side (`kg-oracle --errors` reports `bool + 1` in `f<bool>`'s body,
  framed; neither engine does), a separate change with its own gate
  evidence.

- **1, pass 13 `validateExportableClasses`; pass 10's CST code deleted —
  2026-10-10.** The loop's pass 13 (`kg_sem::check::exportable_classes`)
  judges the module's text too, and notes each refusal as the reference
  stacks notes (`Src`'s `<>`, `Parser/Syntax.hs:623-626`; `mapWith`'s
  `old <> new` per module and `Map.unions` across them): at the member,
  then each public object that made its class exportable, then every
  `export`/`extern`, in order. A verdict is filed under the module whose
  `export` made the class exportable, where the CST pass gave it
  (`TextVerdict::home`), and its ranges start at the first token
  (`looped.rs`'s `token_note`: the IR wears a declaration's leading
  comment). `passdiff --oracle` now compares each `while compiling` note's
  position too. **First reading:** the CST pass giving 20 verdicts — 3085
  programs identical, 14 differ (cst-only 14, ir-only 24): every CST
  verdict lacked the reference's note at the `extern`/`export`
  (`extern-class.k::3,4,…`, `pipelined.k::2`, `basics.k::177`), and the
  CST pass never followed a public object (`export-class.k::17-21`'s
  members, helper ones included). The reference settles all 14 for the
  loop, notes and all; red with its notes unread (14 unsettled).
  **Switched**, both halves — the per-file one in pass 13's place, the
  imported-class one after the per-file passes, as before. Pass 10's CST
  module (`passes/type_errors.rs`, 362 lines, with `fold_divergent`)
  deleted; two tests moved with it: the supertype message is read at the
  rooted door (the text door runs no loop), and the paren test asserts
  the loop's verdict under the fallback too. `scripts/fast.sh loop` green,
  every ratchet +0/−0 (the new verdicts are no first error).

- **1, pass 18 `interpolatedStringPost`; pass 13's CST code deleted —
  2026-10-10.** The loop's pass 18 (`kg_sem::check::interpolated_strings`)
  judges each hole at a module's text by the type the loop gave its
  expression — `validateFormat` with and without a format, by the
  specifier `format` reads — and its alignment as `evaluateConstexpr` left
  it (`isIntLiteral`); an unresolved or error type says nothing. The CST
  pass scanned the string's text and declined every hole but a literal or
  a name it could type. **First reading:** the CST pass giving 9
  verdicts, 7 identical; 2 alike in everything the reference prints and
  apart in the range's end (`interpolated-string.k::46,47`: `"{ {} }"`,
  the CST's range taking in the space before the `}`, the loop's the
  expression's own) — `passdiff` now books such a pair, listed on every
  run and no difference, since nothing can settle it; 0 differ. New over
  the CST pass, none in the corpus (`kg-oracle --errors`): an alignment
  that reads a variable, `{x,x}`. **Switched.** Pass 13's CST module
  (`passes/exportable_classes.rs`, 331 lines) and its last helper
  (`facts::simple_type_name`) deleted; the imported-class half after the
  per-file passes is gone with it; its helper-module test kept at the
  rooted door (`tests.rs`). `scripts/fast.sh loop` green, every ratchet
  +0/−0.

- **1, stopped before pass 17: the undecided-type verdicts need the
  reference's program-wide flag — 2026-10-10.** A port of `deduceAuto`
  onto the loop was measured and not landed. Refusing every type
  `expFromType` cannot write (`Type.hs:1612-1650`), `passdiff --oracle`
  read 16 programs apart, 11 settled for the loop (`initializer-list.k::
  73-85`, `lambdas.k::13`'s capture, `memory.k::20`, `basics.k::36`,
  `type-identifier.k::4`) and 5 not — each an `auto` left undecided only
  because another module's error stopped instantiation:
  `test/library/data/datetime.k` under `mini-base` (19 verdicts),
  `template-param-default.k::13,14` (a refusal in the helper), where the
  reference's one flag stops the program before pass 17 (`Frontend.hs:
  38-52`) and the CST run's per-file checkpoints do not. Leaving undecided
  types to silence, 9 apart and 7 unsettled: `auto x;`, `const auto X =
  X;`, a `static for` over a runtime value, a return the LT verdict
  leaves undeduced — where undecided *is* the reference's verdict. The CST
  pass tells the two apart by syntax it trusts; the loop's cannot without
  knowing whether the program errs anywhere before pass 17, and that is
  decided today per module, by each CST run through pass 16 (passes 1–8,
  9, 12 and 16 at the text are still the CST's). Pass 16's `unresolved`
  arms — what a fold left standing — are the same shape. **For the
  owner:** (a) `Db::check` runs every module's passes up to a moved
  pass's checkpoint and admits its undecided-type verdicts only where no
  module erred — the reference's flag, at the cost of the editor's
  per-file diagnostics waiting on every module's earlier passes
  (memoizable; the per-file gate is a recorded choice, `passes::Early::
  checkpoint`); (b) the loop's check computes the flag from its own tree —
  an `Exp::Error`, a refusal, an undefined name or an error type anywhere
  — a proxy whose exactness `passdiff --oracle` would have to show; (c)
  keep the CST arms for undecided types and move only the rest, which
  retires little. Until then pass 18's CST code stays behind `KG_CST_TEXT`
  — its commit-after is the next switch's — so the instrument stays live.

  **Decided 2026-10-10 (the sprint's lead, not an owner call): (a),
  scoped.** Only the verdicts that can be another module's fallout — pass
  17's undecided types, pass 16's `unresolved` arms, the same shape in 19 —
  wait on the reference's program-wide flag; every other verdict keeps the
  per-file checkpoint the editor is built on. The flag is the reference's
  own (`Frontend.hs:38-52`), so it is exact by construction where (b) would
  be a proxy to prove; and gating these per file reports errors in a
  correct module because another one failed, the direction this port
  never takes. It lands only if the keystroke bench, interleaved, does not
  move — the other modules' early passes are memoized, so the editor pays
  them on first open, not per keystroke.

- **1, the flag and pass 17 `deduceAuto`; pass 18's CST code deleted —
  2026-10-10.** The flag is `Db::program_erred_through_pass_9`: the
  reference's `hasErrors` as pass 10 finds it (`Frontend.hs:44-52,85-86`,
  `Template.hs:55-56,84`), folded from every module's parse stage, the
  loop's own refusals and copies' pass 9 (`kg_sem::check::Checked::erred`,
  with a name a rewrite made that pass 9 rejects — `make_capture1` with no
  base library), and each module's CST run through pass 9 at its text. `kg
  check` reads the last from the runs it makes anyway (a module holding a
  verdict that waits runs last; nothing runs twice); the editor makes the
  other modules' runs through 9 only when its buffer holds such a verdict,
  on threads. The CST runs are memoized nowhere, so the plan's "paid on
  first open" was not available; asking lazily is what keeps a clean
  buffer's keystroke where it was. The loop's pass 17
  (`kg_sem::check::deduce_auto`) refuses every type `expFromType` cannot
  write, at the written type's note (a `for`'s from the `auto`,
  `uninitConst`, `Parser.hs:451`), skipping what pass 10 replaced
  (`basics.k::2`); a verdict on a type only *undecided* waits on the flag,
  except the two the syntax settles (`auto x;`, a `static for` over a
  runtime bound), and a truncated loop gives none. **passdiff
  deduceAuto --oracle:** the CST pass giving 20 verdicts — 3088 programs
  identical, 11 differ (ir-only 11: `initializer-list.k::73-85`,
  `lambdas.k::13`, `memory.k::20`, `basics.k::36`,
  `type-identifier.k::4`), all 11 settled for the loop, 0 unsettled (the
  stop's reading was 16 apart, 5 unsettled; with undecided silent, 7).
  Red under the flag never set (3 unsettled: `datetime.k`,
  `template-param-default.k::13,14`) and always set (6:
  `return-type-polymorphism.k::19`, `type-constexpr.k::37`,
  `auto-placeholder.k::7`, `lambdas.k::13`, `constexpr.k::0`,
  `basics.k::118`). **Switched**; `KG_CST_TEXT` falls back to the CST pass
  until its deletion. Pass 18's CST pass deleted (`pass_18_interpolated_
  string_post` and its scan, ~600 lines; `Whole::names` and the per-run
  declaration maps with it); its rooted-door tests kept, its literal ones
  ported there. `scripts/fast.sh --lint loop` green, every ratchet +0/−0:
  conformance 2602/2602, `--strict` 2681/2681, `errors` agree 701,
  `ptdiff` 2422, `backend` 3267, `infer-identity` 304/304, `loop-left`
  321/321, `resilience` agree 1459, `edit-identity` 87/87. Editor,
  interleaved against `c9f6f28`: `bench --filter keystroke` 316.9 / 352.0
  ms before, 310.7 / 336.2 after (`risc-v.k`, load 13 → 4); over stdio, the
  release `kg lsp` opening a copy of `risc-v.k` under a manifest importing
  `library/`: 984 / 997 / 1046 / 1052 / 925 ms before, 953 / 1053 / 1016 /
  947 / 907 after — noise. **The cost where the flag is asked**: a buffer
  holding an undecided verdict in a program the editor's four rounds
  converge on (`const auto ZZ = ZZ;` over `data.optional`, `data.string`,
  `data.vector` and the full library, 33–36 modules): 20–23 / 22 / 29–35 ms a
  keystroke before, 29–30 / 30–31 / 39–41 after (20–28 ms more in a row,
  before the threads). A program the budget truncates — `risc-v.k`,
  `array.k` under `library/` — gives no undecided verdict and asks
  nothing. The journal, "The program-wide flag, and pass 17 on the loop".

- **1, pass 16 `postDesugar`; pass 17's CST code deleted — 2026-10-10.**
  The loop's pass 16 (`kg_sem::check::post_desugar`) judges what the fold
  left standing at a module's text — a `static assert`, a `clog2`, an
  `int<…>`/`uint<…>` width, a `static if`, an offset (`Frontend.hs:
  393-407,446-449`) — each replacing its subtree as the `cata` does
  (`int<clog2(0)>` is the width's error alone), the verdict's kind kept for
  pass 19 and a width's admission taking an earlier verdict inside with it
  as the CST pass did. Undecided — anything but literals, runtime names
  and calls — waits on the flag. Silent where the reference never judges:
  an error passes 1–8 left inside (now part of the loop's `erred`, too), a
  shift of two literals (a negative count, on which the reference crashes,
  `basics.k::4,5`), a type where a `static assert`'s value goes (the
  reference's parse error, `type-check.k::9`). The CST pass still runs, on a
  copy of the sink: its folds raise the folder's own errors — pass 6's,
  which the loop's tree does not hold at the text — and where one did, no
  pass-16 verdict is admitted (the reference's flag is set by it,
  `constexpr.k::124`). **passdiff postDesugar --oracle:** the CST pass
  giving 25 verdicts, all shared — 3093 programs identical, 6 differ
  (ir-only 6: `type-check.k::1-3`, `constexpr.k::123`,
  `type-identifier.k::4`, `auto-placeholder.k::7`), all settled for the
  loop, 0 unsettled (the first reading, before the three silences, 4
  unsettled). Red with the fold-error rule cut (1) and the crash rule cut
  (2). **Switched.** Pass 17's CST module (`passes/deduce_auto.rs`, 882
  lines) deleted; its tests moved to the rooted door (`tests.rs`, 8),
  one corrected from the text door (`a_static_if_asking_the_type_it_
  decides_is_no_constant`: the text door has no pass 17). `scripts/fast.sh
  --lint loop` green, every ratchet +0/−0 (conformance 2602/2602,
  `--strict` 2681/2681, `errors` agree 701, `ptdiff` 2422, `backend` 3267,
  `infer-identity` 304/304, `loop-left` 321/321, `resilience` 1459,
  `edit-identity` 87/87). Keystroke, interleaved against `c9f6f28`: 340.8
  / 329.4 / 333.2 / 329.4 ms before, 334.5 / 338.4 / 318.8 / 313.4 after;
  `risc-v.k` over stdio 957 / 910 ms open before, 951 / 939 after — the
  first version, a recursive walk building a `Vec` per node, cost ~20 ms a
  keystroke and was rewritten as one stack before landing. Found, not
  fixed: `if (c) { return {0}; } return 1;` in an `auto` function is "Non-
  empty initializer list cannot initialize type uint1" here and clean in
  the reference — pass 19's, at `c9f6f28` too.

- **1, pass 19 in three pieces; `checkConversion` first — 2026-10-10.**
  `validateProgram` is one `cata` (`TypeCheck.hs:33-216`, `desugar'`
  is `cata`, `Desugar.hs:71-74`); the CST ported it to three modules, and
  it moves as those three: `checkConversion` (a call's arguments and
  count, a variable's initializer with the ECC clause, a top-level
  `return`), `checkShadowDecl`, and the rest. The loop's check already
  judged every conversion site at a module's text, as an unframed
  "seam" verdict *seeded* into the CST run — which closed that run at
  pass 1 (the text seeding gap: `uint8 x = s;` beside `static
  assert(clog2(0) == 1)` printed one error, the reference two). Now its
  text-site verdicts are `TextPass::CheckConversion`'s, admitted in pass
  19's place and seeding nothing, computed whatever the program's flag
  says (the copies' still only past it). One CST rule ported: a
  `bitsizeof` of a class is `unresolved` for good (`basics.k::110`).
  `passdiff --oracle` now asks the reference for warnings too (every
  kind on, `--Werror`; printed only where it raises no error).
  **passdiff checkConversion --oracle:** the CST pass giving 181
  verdicts — 3067 programs identical, 32 differ (cst-only 4, ir-only
  39), all 32 settled for the loop, 0 unsettled (first reading, before
  the warnings were asked and the rule ported: 6 unsettled, five of them
  narrowing warnings the reference does print). **Switched.**
  `scripts/fast.sh loop` green, every ratchet +0/−0 (conformance
  2602/2602, `errors` agree 701, `ptdiff` 2422, `backend` 3267,
  `infer-identity` 304/304, `loop-left` 321/321, `resilience` 1459,
  `edit-identity` 87/87). Keystroke, interleaved against `b221969`:
  329.1 / 357.3 ms before, 332.5 / 341.0 after — noise. **CST code
  deleted** the commit after (`passes/conversion.rs`, 1370 lines, and
  `Early`'s environment, which only it read): its 30 tests moved to the
  rooted door (`kg_db`'s `tests::conversion`), every one green against the
  loop — a closure call now counted as the reference counts it ("expected
  2", the capture prepended; the CST's table had it `Unresolved`), an
  undefined callee pass 9's error rather than a silence; three text-door
  tests that read a fold through a conversion now read it through a
  `static assert` (the `scope_of` mutant still caught) or at the rooted
  door. `fast.sh loop` green, +0/−0.

- **1, pass 19's `checkShadowDecl` — 2026-10-10.** The loop's (`Cx::
  check_shadow`, `TextPass::CheckShadowDecl`): a text-site variable with
  a scope, not a member, that `lookupInScope (init declScopeName)` finds
  as a scoped variable or a parameter, over the program's map at pass 19
  — `moveFunctions`' hoisting done, which the CST pass reconstructed
  (`survives_hoisting`) — warned at its name, noted at what it shadows;
  after `checkVarDecl`, and not on a variable it replaced. **passdiff
  checkShadowDecl --oracle:** the CST pass giving 25 verdicts — 3076
  programs identical, 23 differ (cst-only 25, ir-only 24); 21 settled for
  the loop; the 2 left are corpus roots, not asked (`test/logic/
  scope_test_cases.k`, `test_cases_5.k`), and the oracle cannot read them
  (its parser rejects `library/test/internal/assert.k:59`'s `>>`) —
  settled by a probe of each one's shape. 22 of the 23 are the note alone:
  the CST pass noted the shadowed *name*, the reference notes the
  declaration from its first token (`bool x;` at `bool`, a class member
  at its type — `kg-oracle --errors --Wshadow --Werror`); the 23rd,
  `static-if.k::18`, a warning only the CST pass gave. **Switched.**
  `scripts/fast.sh --lint loop` green, every ratchet +0/−0 (the notes
  are no first error's position). Keystroke, interleaved against
  `79d4756`: 331.3 / 350.5 / 350.6 / 405.6 ms before, 348.1 / 347.4 /
  334.3 / 343.3 after — noise. **CST code deleted** the commit after
  (`passes/shadow.rs`, 409 lines): its tests moved to the rooted door
  (`kg_db`'s `tests::shadow`, the lambda row re-probed — the text door's
  version captured the name it then redeclared, a redefinition at the
  rooted door), and its library canary joined the rooted library walk
  (`query.rs`, errors and `shadow` warnings; red under the self-shadowing
  mutant, 12758 reports). `token_note` no longer assumes a range in a
  synthetic module is in the first synthetic module's text (one file id
  for all of them; the mutant run panicked there). `fast.sh loop`: every
  ratchet +0/−0 but `lint`, graded 187 → 186 — one production file fewer,
  the deletion itself; **needs a `lint` bless** from the main checkout.

- **1, the rest of pass 19, `validateProgram` — 2026-10-10.** The loop's
  (`Cx::validate_program`, `TextPass::ValidateProgram`), every clause at
  a text node after its children, as the `cata` has them: a type as an
  operand (`checkInvalidType`) and in the parser's positions the CST
  pass ported (a statement, a `static assert`, a dimension, an integer
  attribute); an array's dimension — an enum constant by its value, and
  one no round folds (a runtime name, a `static for` kept a loop); an
  integer attribute's argument; an enum's base and constants (the closed
  chain left unfolded); a global's state and a function-typed variable;
  an `auto` `deduceAuto` did not rewrite, written by the text; a constant
  assigned; `[[reset]]`; a `default`'s designators; an overridden
  designator. Two rules the CST pass had no need of: nothing is judged
  inside what an earlier pass's error at the text replaced (`errorMsg`
  replaces the node, `Error.hs:147-148`) — passes 10–18's verdicts and
  pass 12's — so `1 + (uint32)` is pass 10's verdict alone and `clog2(
  uint32)` pass 16's, as `kg-oracle --errors` prints them, where the CST
  run printed a second error beside each; and a declaration among the
  statements is no expression. **passdiff validateProgram --oracle:** the
  CST pass giving 83 verdicts — 3096 programs identical, 3 differ
  (cst-only 1, ir-only 3); 2 settled for the loop (`static-for.k::10`,
  `higher-order-functions.k::195`'s closure a call returns, until now a
  seed); 1 booked: `name-attr.k::7`, `[[name("X")]] uint32 x;`, which the
  reference's *parser* rejects (divergence #7) — the loop says "Global
  state is not allowed" at the declaration, 2:1, where the parser's
  error is, the CST pass at the name. (First reading: 3099 apart — a
  `struct` declared in a body read as a type statement.) **Switched.**
  `scripts/fast.sh loop`: every ratchet +0/−0 but `lint`'s graded count,
  the shadow deletion's (above). Keystroke, interleaved against
  `d2095a4`: 349.8 / 339.3 / 357.0 ms before, 360.2 / 340.1 / 356.9
  after — noise. **CST code deleted** the commit after
  (`passes/validate_program.rs`, 1558 lines; its two helpers other code
  read moved to their readers — `dimension_note` to `looped.rs`,
  `type_operands` to `post_desugar.rs`): pass 19 has no CST code left,
  and `KG_CST_TEXT` no pass 19 to fall back to. Its 25 tests moved to the
  rooted door (`kg_db`'s `tests::validate_program`); three read the
  rooted door otherwise and were settled by `kg-oracle --errors`: a memory
  dimension `[(1 - 1)]` is the loop's at its `(` now (`dimension_bracket`
  looks out through the parentheses the IR has no node for), `foo_t<10>
  == uint4` is `uint10 == uint4`, which the rooted door's pass 16 folds to
  false as the reference does, and `mux(1, uint32, 2)` is the *parser's*
  error (`Parser.hs:823-828,870`) — the fold deletes the arm before pass
  19, so the rule went to pass 0 beside the call argument and the
  initializer, with a returned value (`return uint32;`). One post-desugar
  test corrected: `int<(1 ? 8 : int32)>` compiles in the reference; the
  text door's pass 19 had judged the arm the fold deletes. `fast.sh
  --lint loop`: every ratchet +0/−0 but `lint`, graded 187 → 185 (the two
  deleted files) — **needs a `lint` bless**.

- **The text seeding gap — closed 2026-10-10.** Every verdict the loop
  seeded into a module's CST run went in before pass 1, and one that erred
  closed the run at the pass-1 checkpoint: the module's passes 2 to 19 at
  its text were lost. Pass 19's move took the text's conversions off that
  path (above); the rest go now. The walk's last text-site seeds — a
  `static assert` (pass 16's, which `TextPass::PostDesugar` already
  judges) and an `op=`'s operation (pass 10's, now `TextPass::TypeErrors`)
  — are admitted in their passes' places; and the copies' verdicts carry
  the pass they are the reference's (`kg_sem::check::Stage`): a refusal,
  a round's or inference's error pass 8's, a copy's undefined name pass
  9's, everything else after 9. Each enters the sink at its pass
  (`passes::Carried`) — before the checkpoint after 8, before the one
  after 9, before pass 10 — and is shown only where the run got that far,
  as the reference's flag would have it (`Frontend.hs:38-52`). Probed
  with `kg-oracle --errors` and pinned by `tests::a_copys_verdict_enters_
  the_run_at_its_own_pass`: `f<uint32>()` over `T y = true;` beside
  `static assert(clog2(0) == 1)` prints both (was the conversion alone);
  beside `nosuch();` the undefined name alone (was the conversion);
  `C<3>` refused beside a non-`inline` free function the pass-1 error
  alone (was both). Red under each of its three mutants.
  `scripts/fast.sh --lint loop`: every ratchet +0/−0 but `lint`'s graded
  count (the deletions above); the corpus has none of these shapes.
  **Keystroke regressed: `risc-v.k` 324 → 440 ms** (each commit built in
  one directory, sources touched; `b221969`–`29f8bee` 324–335, `e8c2987`
  446). Under `mini-base` that file's copies err at pass 9 and after, and
  their seeds had closed its CST run at pass 1, hiding its own passes 2 to
  19 — `kg check` on it 16 errors then, 26 now (pass-9 verdicts in the
  modules whose runs had closed). The seeds back at pass 1 read 332 ms. The cost is the
  module's ordinary CST run; **for the lead**: accept it, or keep the gap
  for speed — no exact middle exists while the CST runs are unmemoized.
  The journal, "The text seeding gap". **Accepted (the lead, 2026-10-10).**
  The gap hid the reference's own errors behind an earlier pass's; closing
  it costs a module with errors the CST run a module without them already
  pays every keystroke, so the editor's cost on a correct program is
  unchanged and an erring one is no longer cheaper. The bench's worst file
  is one that errs: under `mini-base` the reference rejects `risc-v.k`
  while parsing (`isa.k:59`, divergence #7; `kg-oracle --errors`), so its
  16 or 26 grade nothing — the keystroke figure moves to ~440 ms by what
  the bench measures, not by what a correct program costs.

- **Pass 9's hint wording — 2026-10-10.** `hint name | moduleDefined name
  && not (inScope name)` (`Frontend.hs:144-147`): the CST pass spelled
  every module-scope candidate "`foo` defined in a", its own module's
  included, where the reference spells a name in scope bare — "Perhaps
  you meant: `foo`" — and only another module's with its module (both
  `kg-oracle --errors`). The loop's pass 9 in a copy already had it. Test
  `a_hint_names_the_module_only_of_another_modules_name`, red under the
  old rule. `scripts/fast.sh --lint`: every ratchet +0/−0 but `lint`'s
  graded count (no corpus first error carries such a hint).

- **1, pass 9 `undefinedSymbols` — measured, not moved, 2026-10-10.**
  A prototype admitted the loop's pass 9 at each module's text (the copy
  walk, `kg_sem::check::Undefined`, judging written nodes at the text
  too) in the CST pass's place. **passdiff undefinedSymbols --oracle:**
  the CST pass giving 635 verdicts — 3003 programs identical, 96 differ
  (cst-only 297, ir-only 6), 95 unsettled. The loop misses whole classes
  the CST pass gives: an undefined template's default (`Memory = memory`,
  `data/memory/byte_addressable.k:19` under `mini-base`), an undefined
  class template's instance as a type (`optional<uint32>`,
  `processor/risc_v/trace.k:681`), and `static_cast` out of scope —
  causes untraced (the first guess: nodes the loop types `TUnresolved`
  where pass 9 asks `TUndefined`). Not landed; pass 9 stays the CST's.

- **1, pass 9 `undefinedSymbols` — traced, fixed, switched, 2026-10-10.**
  The prototype rebuilt reads as recorded (96 apart, 95 unsettled); every
  difference traced to one of ten causes before anything changed (the
  journal, "Pass 9 on the loop: ten causes"). The loop's five: a value's
  name skipped as a rewrite's (the lowering donates a `QualifiedIdentifier`
  its `NamedValue`'s range — two thirds of the reading); a scoped type
  name noted at its start, not its last identifier, and a free call's
  qualified callee at its leaf, not the path; a qualified name's error not
  replacing its subtree's (`Backend::Sv`); a name a rewrite copied
  (`higherOrderFunctions`' `is_last`), now placed by the text that spells
  it (`kg_sem::check::CopiedName`; a minted one counts toward the flag
  alone, as before); and two of divergence #7's stand-ins — a path the
  sorter refused (`optional<uint32>` under `mini-base`), which now carries
  what the sorter knew (`kg_hir::Refused`) and is judged as a `ScopedNameF`
  with no qualifier, and a template template parameter's undefined
  default, a type name, once. The CST pass's five, each settled by a probe
  of its shape: a body inference never types (`Type/Inference.hs:55,58`),
  a template's body (a higher-order function's, a generic lambda's), every
  case and the selector of a standing `mux`, a hint partitioned by kind
  rather than type, and four snippets the reference settles outright.
  **passdiff undefinedSymbols --oracle:** the CST pass giving 635 verdicts —
  3083 programs identical, 16 differ (cst-only 21, ir-only 11); the 4
  snippets settled for the loop by the reference, the 12 roots (which the
  oracle cannot read under `mini-base`) by those probes, each named in the
  journal. **Switched**, the CST pass `KG_CST_TEXT`'s fallback until its
  deletion. The base run — the census's error gate, deliberately wider
  than `hasErrors'` — takes pass 9 at the text from the loop
  (`FromLoop::pass_9_of`): without it `xtask ir` went red on
  `memory_test_cases_3.k`, whose census the reference decides a round
  later; with it `ir` is unchanged (170 decide, 311 gated). `query.rs`'s
  test-library walk corrected to the reference's own reading (`mmio.k`'s
  four `HARTS`, not the six booked as a divergence; `testbench.k`'s
  `ENABLE_M`). Tests: 4 in `kg_sem::check`, 5 at the rooted door
  (`tests::undefined_symbols`), 1 in `instances::tests`; 17 mutant runs
  caught, one survived first (the head's parse-time answer, under
  `Do<x && y>(10)`, which the CST does not read as a path) and its test
  was rewritten (`Do<3>(1)`). `scripts/fast.sh --lint loop` green, every ratchet +0/−0
  (conformance 2602/2602, `--strict` 2681/2681, `errors` agree 701,
  `ptdiff` 2422, `backend` 3267, `infer-identity` 304/304, `loop-left`
  321/321, `resilience` 1459, `edit-identity` 87/87, clippy 0).
  **CST code deleted** the commit after (`pass_9_undefined_symbols` and
  its helpers, ~930 lines, with `Templates::is_alias`); its 35 tests moved
  to the rooted door (`tests::undefined_symbols`), every one green against
  the loop before the deletion; the parse-time table's checks stay, the
  CST's, before every pass. **Found while deleting, lost by the switch**:
  the CST pass stood down program-wide where any module holds a syntax
  error or an import failed to load (`Frontend.hs:48-52,85`,
  `Main.hs:73-75`), and the editor's per-file gate reported a name a failed
  import would have supplied as "not in scope"; `passdiff` could not see
  it, its rooted runs stopping at the parse stage. Restored
  (`undefined_symbols::stands_down`), red under each half's mutant.
  `fast.sh --lint loop` green, +0/−0; `xtask ir` unchanged. **Keystroke**
  (`cargo xtask bench --filter keystroke --json`, interleaved, each commit
  extracted into one build directory with every source touched, load 4–22):
  `01b2328` worst 440.5 / 436.4 / 441.2 ms (`risc-v.k`), corpus total
  7545 / 7496 / 7537 ms; the switch (`54ee70a`, both passes run) worst
  434.3 / 440.0, total 7838 / 7763; the deletion (`8a9af26`) worst 402.8 /
  417.2 / 397.4, total 7661 / 7615 / 7662 — the worst file 30–40 ms
  faster, the corpus total ~1.6% slower, cause unmeasured (the likeliest:
  modules whose run the CST pass's wrong verdicts closed after 9 — a
  template's body, an untyped body — now run passes 10 to 19, the
  ordinary cost the seeding entry priced). Still reaching `kg-cst-infer`:
  the CST type table (`Db::type_table`, `passes::types`, under the loop's
  overlay) that passes 0–8, pass 12's CST arms, pass 16's folds and the
  parse-time member-template check read, and `deduce_type`, `bin_op`,
  `from_integral_constexpr`, `TemplateParam` in passes 4, 6, 8, 12.

- **1, pass 6 `inferType`'s errors — 2026-10-10.** The loop's fold now
  makes the node it cannot fold the error, as `desugarError` does
  (`Constexpr.hs:181-186,274-275,349-355`): a division or remainder by
  zero, `bytesizeof` of a width that is not whole bytes, a `mux` selector
  a later fold put out of range — beside the illegal `decltype` and
  offsets it already made. Each such node at a module's text is
  `TextPass::InferType`'s (`kg_sem::check::fold_errors`: outside every
  copy, a template's parameters included, its body never), admitted in
  pass 6's place — before the checkpoint after 8 — and noted from the `(`
  of any parentheses around it (`paren_note`, as pass 10). The CST
  folder the later CST passes share raises nothing anyone reports once
  the loop speaks (`Early::folder_speaks`): its errors are recorded as
  the CST side, and pass 16's "a fold raised an error" stand-down is
  gone with it. The base run takes pass 6 from the loop as it takes
  pass 9 (`FromLoop::pass_9_of`); `xtask ir` byte-identical but timings.
  **passdiff inferType --oracle:** the CST pass giving 12 verdicts —
  3098 programs identical, 1 differs (ir-only 1: `constexpr.k::124`'s
  `uint<2 * bytesizeof 1>`, which the CST run raised only from pass
  16's fold), settled for the loop, 0 unsettled; red with the loop's
  verdicts dropped (13 unsettled). New over the CST pass, none in the
  corpus (`kg-oracle --errors`): a fold error in a copy, framed
  (`4 / N` in `f<0>`); a parenthesized one at its `(`. **Switched**;
  `KG_CST_TEXT` falls back to the CST pass until its deletion.
  `passdiff postDesugar --oracle` after: 9 differ, all settled, as
  before. Tests: 6 at the rooted door (`tests::infer_type`), 6 mutants
  caught. `scripts/fast.sh --lint loop` green, every ratchet +0/−0
  (conformance 2602/2602, `errors` agree 701, `ptdiff` 2422, `backend`
  3267, `infer-identity` 304/304, `loop-left` 321/321, `resilience`
  1459, `edit-identity` 87/87, clippy 0). Found, not fixed: a binary
  operand of another operator is noted at its operator (`binaryNoted`,
  `Parser.hs:78-89`; only an `expression`'s root is renoted, `:832`) —
  `1 + 4 / 0` at the `/`, `a + true + 1`'s pass-10 error at the first
  `+`; both engines report the operand's start (divergence #25).

- **Pass 16's tests at the rooted door, and pass 11's callee — 2026-10-10.**
  Porting pass 16's CST tests to the rooted door (`tests::post_desugar`,
  21 tests) found one rule both engines had wrong: a `clog2` that did not
  fold as a call's argument is pass 11's "Unresolved argument" only where
  the callee is still a template's instance (`unresolvedTemplateCall`,
  `Frontend.hs:437`) — `println(clog2(-1))`, `println` an abbreviated
  template — and pass 16's own verdict as a plain function's argument or
  an instance's a round made (`r(clog2(-2))`, `q<uint8>(clog2(0))`,
  `kg-oracle --errors --no-base`). The loop's pass 16 now asks the callee
  (`calls_an_instance`); red under the old rule. Three text-door tests
  read a name no module declares, which is pass 9's at the rooted door,
  and now ask a template's parameter in a body no instance reaches (clean
  in the reference); one hedged `accepts` is the reference's verdict now
  (`const auto o = 10 * (-20); static assert(bitsizeof o == 1)` is
  "static assertion failed"). `passdiff postDesugar --oracle` unchanged (9
  differ, all settled); `scripts/fast.sh --lint` green, +0/−0.

- **1, pass 12 `unresolvedTemplates` — 2026-10-10.** The loop's pass 12
  at a module's text (its arms over each instance left standing, until now
  admitted beside the CST's three arms and deduplicated) is
  `TextPass::UnresolvedTemplates`, admitted in pass 12's place; the CST
  arms and pass 4's "Could not deduce all arguments" — pass 12's verdict,
  which the CST pass 4 reported where the list is decided and before any
  checkpoint — are its CST side (`Early::held`), the latter recorded even
  where the run stops short of 12. **First reading:** the CST side giving
  37 verdicts — 3070 programs identical, 29 differ, 9 unsettled, four
  causes: (1) the template's `while compiling` note wore the declaration's
  documentation (`println`'s at `print.k:29`, the reference's at `:39`;
  `basics.k::3,20,21,23`) — now from its first token; (2) "Undefined
  template" at the text was left to pass 9, which never judges an
  instance's name (`Frontend.hs:96-97`): `template Go<bool>()` over a dead
  `static if`'s `Go` (`dependent-template.k::8`), `c.c1s.Foo<uint32>(x)`
  (`member-function-template.k::20`) — now pass 12's, at the name, placed
  by the text that spells it (`CopiedName`, with its pass: a `Closure<…>`
  `desugarLambdas` made with no base library is pass 9's
  `make_captureN`'s fallout and says nothing); (3) a template declared in
  a copy carries the copy's note after its own (`A<bool>::A2_t<2, 3>`,
  `template-instance-identifiers.k::7`); (4) an instance a rewrite minted
  is noted past the capture list's `[` (`lambdas.k::13`). **passdiff
  unresolvedTemplates --oracle after:** 3074 identical, 25 differ (cst-only
  9, ir-only 29), all 25 settled for the loop, 0 unsettled — the CST's
  "Too many arguments" without the reference's note (`struct-template.k::
  3`, `using.k::15-19`, `basics.k::49`), "Unresolved argument" the CST arm
  never judged (`constexpr.k::0,1,6,7,75,132`, `type-check.k::54,55`), and
  the four causes'; red with the loop's verdicts dropped (26 unsettled).
  New over the CST side, none in the corpus (`kg-oracle --errors`): an LT
  beside a pass-9 error is not shown (pass 9's flag stops pass 12).
  **Switched**; `KG_CST_TEXT` falls back to the CST arms until their
  deletion. Tests: 3 at the rooted door (`tests::unresolved_templates`),
  5 mutants caught. `scripts/fast.sh --lint loop` green, every ratchet
  +0/−0 (conformance 2602/2602, `errors` agree 701, `ptdiff` 2422,
  `backend` 3267, `infer-identity` 304/304, `loop-left` 321/321,
  `resilience` 1459, `edit-identity` 87/87, clippy 0).

- **1, pass 2 `higherOrderFunctions` — 2026-10-10.** The loop's pass 2
  (`kg_hir::rewrite::higher_order`) left a non-inline function taking a
  function as written and said nothing; it now records it
  (`HigherOrderFunctions::refused`, `Elaborated::higher_order`, through
  `pass1::heads`), with the alias resolution its rewrite already had
  (`resolvedFuncParams`, `Function.hs:133-141`), and `kg_sem::check`
  reports each one still standing in the final tree as
  `TextPass::HigherOrderFunctions`, admitted in pass 2's place, noted from
  the function's first token. **passdiff higherOrderFunctions --oracle:**
  the CST pass giving 0 verdicts — the corpus never writes one — 3099
  programs identical. So the probes decide (`kg-oracle --errors`): the
  reference notes the function at its first token (`void`, or `[[async]]`),
  the CST pass at its name; it refuses an alias-typed parameter's
  function (`uint32 Go(F f, …)`, `using F = (uint32) -> uint32`), which the
  CST pass could not see; and a refused member of a `static if` branch pass
  6 deletes is gone with the branch — the reference collects errors from
  the final tree — where the CST pass reported it. All three are the
  loop's now. **Switched**; `KG_CST_TEXT` falls back to the CST pass until
  its deletion. Tests: 3 at the rooted door (`tests::higher_order`), 4
  mutants caught; a fifth — the refusal left out of the loop's own part
  of the flag — survives, the module's run through pass 9 setting the
  flag the same. `scripts/fast.sh --lint loop` green, every ratchet
  +0/−0. Found, not fixed (pass 1, the CST's): a local function in a
  class's method is "Free functions must be declared `inline`" in the
  reference and clean here; a free function's note is its first token
  (`[[pipelined]]` at the `[[`), here its name.

- **1, the CST code of passes 2, 6, 12 and 16 deleted — 2026-10-10.**
  `passes/{higher_order,infer_type,unresolved_templates,post_desugar}.rs`
  (2463 lines), the judge that ran a moved pass's CST code beside the
  loop's (`judge_text`), `KG_CST_TEXT`'s reach into the passes
  (`FromLoop::cst`; the setting stays, with nothing left to fall back
  to), the CST run's shared folder (`Early::eval`) and the dead-branch
  list only pass 12 read, and the `template <…>` index only pass 12's
  arms asked (`Templates::{is_named_live, class_known}`, a walk of every
  root's declarations). Pass 4 still decides pass 12's LT verdict where
  the list is decided — its own control flow reads it (a verdict there
  is the call's whole diagnosis) — and the run drops it. The base run
  (the census's gate) takes pass 2 from the loop with passes 6 and 9,
  which the pass-2 switch had left out. `passdiff`'s `PROBED` is empty:
  what still judges a module's text on the CST is pass 1, passes 4 and 8,
  and the parser's checks, none moved. Tests: pass 6's 4 and pass 2's
  1 at the rooted door; pass 12's three arms' 22 and pass 4's 8 LT tests
  at the rooted door against the loop's pass 12 (`tests::
  unresolved_templates`), one asserting the LT alone now first of two —
  the reference's second, pass 11's "Unresolved argument" on a runtime
  `clog2`, has no version at a module's text; `scope.rs`'s fold tests
  read through a `static assert` at the rooted door; corrected to the
  reference: `auto x = 3; uint1 y = bytesizeof(x);` is "bytesizeof is
  undefined for a type with bit size of 2" (`kg-oracle --errors`; the CST
  stayed silent on an `auto` name). `scripts/fast.sh --lint loop`: every
  ratchet +0/−0 but `lint`, graded 185 → 182 (the deleted files) —
  **needs a `lint` bless**; `xtask ir` byte-identical but timings.
  **Still reading the CST type table** (`Db::type_table`, `passes::types`,
  `kg-cst-infer`): passes 4 and 8 (`deduce_type`, `Deduction`,
  `TemplateParam`, `from_integral_constexpr`, and `fold::ArgFold`'s
  argument folds), pass 0's type-as-a-value check
  (`early_normalization::pass_0_types_and_constants_as_values`), the
  parse-time member-template checks (`undefined_symbols`'s receiver
  type), the loop's per-module table overlay the CST runs read
  (`Looped::table_for`'s base), `Db::foreign_constants`' declined
  initializers, the second census (`Db::module_instances`, W3.7's), the
  editor's fallbacks (`Db::type_of_definition`, `members_of` in
  `complete.rs`, the inlay hints' in `kg-lsp`) and `xtask sem`.

- **1, passes 4 and 8 — measured, not moved, 2026-10-10.** A
  `TextPass::TemplateTypeCheck` — the loop's `validateArg` refusals and
  conflicting deductions at a module's text — against the CST passes 4
  and 8 run together in pass 8's place (they read nothing of the sink):
  **passdiff templateTypeCheck --oracle** read the CST side giving 89
  verdicts, 3067 programs identical, 32 differ, 20 settled for the loop
  (refusals the CST never made: a callback's closure type in a call the
  loop deduces, `memory.k::7,10,11`, `higher-order-functions.k::132,133,
  189`; conflicts, `template-argument-deduction-from-instance.k::6,8`),
  **12 unsettled**, every one a `validateArg` clause the loop's census
  lacks (`TypeCheck.hs:228-297`): a template-template parameter's kind
  match, "Expected a template with parameters: …" (`template-template-
  parameter.k::3,16,21,22,29`); `validate Auto InitializerListF{}`
  (`initializer-list.k::48`); the general clause over a float argument,
  "Expected a constant expression of type: int32 … float" (`type-check.
  k::25,28,41,53`, where the loop instead instantiates and reports a
  conversion inside the copy); and a typename parameter given a value with
  the dependent parameter after it (`type-check.k::19`, which the census
  leaves undecided — "Failed to instantiate template"). Each refusal
  changes what the census decides, so it lands behind `loop-left` and
  `ptdiff`, as its own change.

- **1, passes 4 and 8, `templateTypeCheck` — switched 2026-10-10.** The
  census's `validateArg` (`kg_sem::census::refusals`) now asks what it
  lacked, each clause read off `TypeCheck.hs:228-297`: every argument on
  its own (`zipWith validateArg`, `:226`), not the first;
  `validateTemplate`'s kind-by-kind match for a template-template
  argument, the wanted kinds substituted (`resolveTemplateParamType`) and
  both lists spelled as `showPretty` spells them, a literal default
  included (`census::pretty_param`); `validate Auto InitializerListF{}`;
  a float literal under the general clause; a type parameter bound to a
  value typed as `theType` of it, `TError "Expected a type"`
  (`resolveTypeParam` puts the argument's node there, `Type.hs:458-464`);
  a zero width a type, `int0` (`infer (ParamIntF x)`'s
  `fromIntegralConstexpr`, `Type.hs:1246`), and no re-stamp to it
  (`width > 0`, `:281`). Refusals and a round's conflicting deduction at
  a module's text are `TextPass::TemplateTypeCheck`'s, admitted in pass
  8's place; the CST passes 4 and 8, which read nothing of the sink, run
  together there as its CST side, their LT verdict still dropped; the
  base run takes them from the loop. **passdiff templateTypeCheck
  --oracle:** the CST side giving 89 verdicts — 3079 programs identical,
  20 differ (ir-only 20), all settled for the loop, 0 unsettled (first
  reading 12, then 2: `int0` and a dependent kind); 22 pairs apart only
  in a kind the reference does not print (the conflicts'
  `ConflictingDeduction`); red with the loop's verdicts dropped (88
  unsettled). **Switched**; `KG_CST_TEXT` falls back to the CST passes
  until their deletion. Tests: 5 at the rooted door
  (`tests::template_type_check`), 11 mutants caught. `scripts/fast.sh
  --lint loop` green, every ratchet +0/−0 but `lint`'s graded count (the
  deletion's): conformance 2602/2602, `errors` agree 701, `ptdiff` 2422,
  `backend` 3267, `infer-identity` 304/304, `loop-left` 321/321,
  `resilience` 1459, `edit-identity` 87/87, clippy 0 — the census decides
  every corpus root as before; `xtask ir` byte-identical but timings.

- **1, the CST code of passes 4 and 8 deleted — 2026-10-10.**
  `passes/deduce_template_args.rs` and `template_type_check`'s
  `validate` (about 2,350 lines of code net), what only they read — the
  CST run's fold environment (`Db::module_environment` per run, the
  `ArgFold` and the `env` parameter), the other modules' tables
  (`TableOf`), the loop's message names (`InstanceNames`), the program's
  inference in `CstProgram`, the twin's never-filled alias maps — and
  `KG_CST_TEXT`'s reach again (`FromLoop::cst`, `judge_text`; the
  setting stays, read by nothing). Kept: the `template <…>` index the
  parser's checks ask (`passes::templates`, with the scope walk), and
  `template_type_check::resolved_instance_args`, the CST reading of
  `templateResolved` `xtask ir`'s differential grades the second census
  against, until W3.7 deletes both. `passdiff`'s `PROBED` is empty: pass
  1 and the parser's checks are what still judge a module's text on the
  CST. **Tests:** pass 4's 16 and pass 8's 36 at the rooted door
  (`tests::{deduce_template_args, template_type_check}`); the scope
  walk's unit test with the index; `context_expects_a_type`'s unit test
  gone with it (the end-to-end `a_parenthesized_call_deduces_from_its_
  context` stays). Porting them found **three census gaps**, each fixed
  to `TypeCheck.hs:228-297` with its mutant caught: a template's name for
  a `typename` parameter is "Expected a type" (the census asked
  `tResolved` first, which `TTemplate` is not, `Type.hs:1002`); a
  template-template parameter is spelled, with the hard break `src
  (Template a b)` puts after its list (`PrettyPrint.hs:159`), so a
  mismatch naming one is reported, not dropped; an `auto` slot is
  `theType`'s `TUnresolved` (`Type.hs:1174`), matching a template-template
  parameter. None is in the corpus. **Corrected to the reference**
  (`kg-oracle --errors`, each the text door's premise): a callback's
  closure type with the `data.closure` stubs (`tests::closured_errors`);
  an unknown call is pass 9's "Symbol name not in scope" alone; explicit
  arguments leave pass 19's conversion; a lambda as a function-typed
  template argument is pass 11's "Unresolved argument"; a short list is
  completed and refused ("Expected a type"); a local `S1` shadowing the
  template is "undefined template" (the reference's parser rejects it,
  divergence #7). `instances::standalone` (`kg dump instances -`) gates on
  the base run (`Db::check_with`), as `Db::instances` does: the text door
  no longer sees pass 8's refusals (`a_program_with_an_error_decides_
  nothing` red otherwise). `tests/memo.rs`: every miss the same, fewer
  asks (`inference` 9 → 6, `file_scope` 8 → 5 on a body edit) — the
  inputs only the deleted code read. `scripts/fast.sh --lint loop` every
  ratchet +0/−0 but `lint`'s graded count, still 182 against the
  baseline's 185 (**needs the `lint` bless**): conformance 2602/2602,
  `errors` agree 701, `ptdiff` 2422, `backend` 3267, `infer-identity`
  304/304, `loop-left` 321/321, `resilience` 1459, `edit-identity`
  87/87, clippy 0; `xtask ir` byte-identical but timings.
  **Still reading the CST type table** (`Db::type_table`,
  `passes::types`, `kg-cst-infer`): the CST run's pass 0 (a type or a
  constant used as a value, `early_normalization`) and parse-time checks
  (a member template's receiver, an undefined name, `undefined_symbols`)
  through `Looped::table_for`'s base; the text door, for the same pass 0;
  `Db::foreign_constants`' declined initializers; the second census
  (`Db::module_instances`, `instances::{standalone, differential}`,
  `resolved_instance_args` with `kg_cst_infer::from_integral_constexpr`
  — W3.7's); the editor's fallbacks (`Db::type_of_definition`,
  `navigate`, `complete.rs`'s `members_of`, `kg-lsp`'s inlay hints);
  `xtask sem` and `edit-identity`'s table fingerprint.

- **7, one engine — done 2026-10-10 (W3.7, M6).** Eight commits on W7.6:
  - **One door** (790b9ab). `check_source` — `xtask bench`'s `check`
    stage, `xtask fuzz`, the crate's own tests — ran the parser's checks
    and pass 1 over one text with the CST engine's table and a one-file
    `FileScope`, and nothing else, every type-reading pass being the
    loop's. It is a `Db` over the text alone now (no base library, no
    import directory: `--no-implicit-base`), so the bench's `check`
    stage measures the frontend, loop included, where it measured two
    passes.
  - **The parser's two checks onto the parse and the loop** (6b9cb82,
    switched; ca5f3f0, the CST side deleted). `expression`'s "Invalid use
    of a type as an expression" (`Parser.hs:823-828`) asks what the
    parser built — `kg_hir::sort::is_type` over the loop's sorted tree
    (`kg_db::parsed`); `methodInstanceSpecifier`'s "undefined template"
    (`Parser.hs:766-770`) the sort's reading confirmed by the loop's types
    at the text. `xtask passdiff` takes the parser's checks as probes
    (`kg_db::passdiff::Probe`, the reference's `PARSE-ERROR` its
    verdicts): **typesAsValues** — the CST side giving 6 verdicts, 3099
    programs identical, 0 differ (first reading 1 ir-only: a template
    parameter whose kind the sort left an unsorted path); **memberTemplates**
    — 1 verdict, 3099 identical, 0 differ. Each red with the loop side's
    verdicts dropped (5 and 1 unsettled). With them the CST run reads no
    type table, and `Looped::table_for`'s overlay — the CST engine's
    inference of every module on every loop — goes: a body edit's check
    asks `parse` 107 → 79 and `item_tree` 84 → 58 times (`tests/memo.rs`).
    New over the CST side: a type as a value inside a template's body is
    the parser's error (the CST engine typed no template body).
  - **Pass 1's free function** (7f7a730), the two items this section
    had as found-not-fixed: `freeNonInlineFunctions` reads the parser's
    `funcKind`, so a non-inline function in a method's body is free, and
    the error is at the function's note (its first token past comments, an
    attribute's `[[`); the parser's own checks close the run before pass 1,
    as a `PARSE-ERROR` stops the reference's frontend (`Main.hs:73-75`).
  - **The second census deleted** (b56fd7b; 8,279 lines out). `kg dump
    instances` lists the loop's instances (`Elaborated::copies`), per
    template by its declaration's dotted name, each as a `while compiling`
    note spells it, with its round and site; a member template's filed by
    its class instance's note, never its mangled name (802fd0b, `lint`
    `mangled-text` 0). `xtask ir` loses the differential (405/405), the
    instances statistics, and `resolve diff` (352/353) and `instance
    diff` (353/353), whose input the census was; every other line
    byte-identical. The loop's resolve and instantiate stay graded end to
    end by `ptdiff`, `backend`, `output` and `loop-left`.
  - **The editor reads the loop alone** (5e8273e), below.
  - **The engine deleted** (26700d6; 12,494 lines out, 1,098 in): the
    crate `kg-cst-infer`, `passes::types`, `passes::intrinsics`,
    `scope` (`FileScope`), `fold`, `Db::type_table` and its memos,
    `Db::type_of_definition`; kg-const's `ConstEval` and `ConstEnv`
    (`fold_binary_values` and `CastTarget`, which the IR's fold reads,
    stay); kg-ty's `TypeTable` (`TypeKey` stays); `KG_CST_TEXT`; `cargo
    xtask sem`; `edit-identity`'s table fingerprint. Divergences #19 and
    #20 close with it — sibling roots share types and an imported
    constant limit types its induction variable `const uint3`, both as
    the reference does.

  **What parity means, and what the editor gave up.** The resilience gate
  measured it with both engines still there (5e8273e, its silences
  classified, `xtask/src/resilience.rs`'s `silence`): every answer the
  CST engine gave and the loop does not is one of
  - **a redefinition** — the declaration, or one it is written in,
    redeclares a name its scope holds; the reference's parser rejects the
    file there (`SymbolTable.hs:200-201`), so no program holds it, and the
    loop keeps one declaration per name. The snippet accumulators
    redeclare `main` once a snippet: 736 hovers, 96 completion lists;
  - **text edited since the loop the editor reads** — a deleted
    initializer, a function an edit made a variable — which the next loop
    over a buffer that parses answers: 7 hovers, 66 completion lists;
  - **the rest**, 5 hovers and 27 completion lists, each in code the
    reference rejects (`reorder bool x;`, the lowering's `Unsupported`; a
    class holding an `inline noinline` method; `[[max_threads(int32)]]
    void Go()`, a `PARSE-ERROR`) or a fuzzed declaration the classifier
    does not place.
  Parity is defined as that: **the loop answers wherever the CST engine
  did, except in a redefinition no program holds and in text edited since
  the loop ran**. Against main before W3.7: hover silences 764 → 748
  (agree 964 → 980), completion 375 → 189 (agree 495 → 533), the loop
  typing 195 more candidates the CST engine left bare. One real gap was
  closed to get there: a declaration a rewrite rebuilt (`main` minted
  again by `moveFunctions` round a class it lifted out; a method
  `desugarLambdas` rebuilt) had no `Own` node, so no answer —
  `kg_hir::position` now indexes the reachable `Donated` declaration that
  stands where one of its constructor was lowered, when no `Own` node
  claims that text (hover silences not in a redefinition 27 → 12,
  completion 265 → 94 lists). **Also given up**: a buffer opened broken,
  before any loop has run, gets no member list, completion detail or
  inlay hint until one runs (a hover runs its own); the 149 inlay hints on
  the accumulators' redefinitions; and member navigation — unchanged, 706
  of 1057 selectors unanswered before and after.

  **The resilience gate after the engine** grades the editor alone:
  `answered` (pass), every answer `key = answer`, and `failed`; why the
  editor shows nothing is a histogram by reason and declaration kind.
  First reading (26700d6, `scripts/fast.sh --lint loop`): hover 4100
  graded, answered 1542, copies 142, silent 2416 (the CST era's
  `neither` included: a template, a namespace, an `auto` never deduced),
  1398 declined; completion 992 lists graded, 11121 of 41723 candidates
  with a detail, 44 member lists the loop has no type for. **Needs the
  lead's bless**: its sets changed.

  **Keystroke**, interleaved A/B before and after the engine's deletion
  (`cargo xtask bench --filter keystroke`, two rounds each, both built
  from scratch, load 12–41): 802fd0b `risc-v.k` worst 423.7 / 417.3 ms,
  corpus total 8469 / 8425 ms; 26700d6 409.5 / 426.9 ms, total 7888 /
  8118 ms; the fixpoint row (`array.k`) 990 / 941 → 906 / 947 ms. Neutral
  within the load's noise, as it should be: nothing on a keystroke's path
  asked the engine after ca5f3f0 (the overlay) and 5e8273e (the editor's
  fallbacks). W3.7 whole against main (fb0afc1 → 26700d6, the same
  bench, load 11–18): `risc-v.k` worst 439.8 / 432.5 → 425.0 / 407.4 ms,
  corpus total 9013 / 8700 → 8099 / 7898 ms — about 10% off every
  keystroke's check, the CST engine's inference of every module that the
  overlay ran on every loop; the fixpoint row 887 / 884 → 928 / 884 ms,
  unmoved. `scripts/identity.sh b4ee408`: 304 roots' elaborated
  fingerprints identical to main's — the batch path unchanged.

  **Exit — met 2026-10-10.** The CST engine is deleted, `kg-cst-infer`
  with it; `errors` positions unchanged (agree 701, disagree 18 booked,
  +0/−0); the walk's types are the loop's alone (step 6); the resilience
  gate at parity as defined above.
  Found and not fixed: a member template called on a function parameter
  (`inline uint32 f(C c) { return c.Foo<uint32>(1); }`) is the
  reference parser's "`Foo`: undefined template" (`kg-oracle --errors`:
  its parse-time table does not type a parameter) and compiles here —
  silent, before W3.7 and after; no corpus assertion depends on it
  (conformance 2681/2681).

### W4 — Consolidation (`single-source-of-truth.md`)

Landed: D1.1 names, D1.3 constant evaluation, D1.4 node → type key. Open,
scheduled beside the work that touches them:

| Item | With |
|---|---|
| D1.2 reading instances back; D2.5 template arguments as `TArg` | W1.1 |
| D2.1 one `TemplateIndex`; D2.2 import visibility through the module graph | W1.2 (cross-module instances) |
| D2.4 one `StaticDecisions` per module | W2's folding rows, W1.3 |
| D2.6 one `check_conversion` | W2's conversions, W3's pass 19 |
| D1.5 written type → `Ty`; D1.6 literal typing; D1.7 small rules; D1.8 one `ScopeStack`; D2.3 one binder model | W3, as each CST copy is deleted |

**Exit:** every "copies today" column of D1/D2 empty, or naming a copy
kept on purpose in the plan's register with its reason.

**Progress.**
- **Survey after W3.7 — 2026-10-10.** Each open row's copies read again
  in the code, by `file:line` (the plan's tables, "Copies today"). D1.2
  had landed with W1.1 (steps 1–3, 2026-09-27) and D1.8 by deletion with
  the CST engine (`26700d6`); every copy D2.1, D2.2 and D2.5 listed is
  gone (M4's `81452c6`, W3.7's `b56fd7b`), and with them the drift
  D2.1–D2.4 were written against. What stands is of three kinds: code
  dead since the twin went (the twin's static control and import half,
  `InstanceSpec::known`, the walk's `extern_classes`, `kg_ty::convert`'s
  unused `check_conversion`); small rules still written twice or six
  times (D1.6, D1.7); and readings that answer another question than the
  owner's — the parser's table at a position (D2.3), the walk's spelling
  of pass 19's rewrites (D2.6).
- **The rows consolidated — 2026-10-10; W4's exit met.** One commit per
  row, each exact against its parent — `scripts/identity.sh` 304
  identical, `infer-identity` 304, `fast.sh --lint loop` every ratchet
  +0/−0, `edit-identity` 87, clippy 0, the quick tests green:
  - D1.6: `7940167`;
  - D1.7: `20ad143`;
  - D2.4: `1bf6b87`, the twin's static control and the walk's
    `extern_classes`;
  - D2.1/D2.2: `4edf1d0`, the twin's lookups;
  - D2.5: `ef9e443`;
  - D2.6: `088eba8`, the uncalled `check_conversion`;
  - D1.1/D1.5: `7bc82b8`, the instance bracket written once and `<|{` in
    the lint.

  Two commits change what a program off the corpus gets, and moved no
  gate:
  - `6b57574`: the walk declines a `static if` the loop left standing
    rather than fold it itself (0 firings over `ptdiff`'s 2422 programs);
  - `1c92997`: `check.rs` asks the owner whether a function is an
    abbreviated template. `F(const auto x)` now gets the reference's
    first error, `1:22 Could not deduce type`, where we were silent.

  Every "copies today" cell is now empty or names a register entry, each
  with its reason and comparing gate. The fallbacks that stayed were
  measured first. Not done, a D2 change of verdicts: moving the parser's
  template checks onto the sorted lowering's facts, which would retire
  `Templates`, `visible_through_imports` and `Binders`. Journal: "W4
  after one engine: what the copies had become".

### W5 — Performance

Measured and exact, as in [`performance.md`](../plans/performance.md). Ordered by
what the editor and the gates feel:

1. **Keystroke** (1.46 s → ≤ 400 ms, then M6's ≤ 100 ms): **moved to
   W7**, which is its mechanism — this item named memoized interfaces,
   cancellable refresh and units kept across edits, none of which the
   query layer has. What stays here is the loop's own cost per round,
   items 2–6, which a keystroke pays whenever W7 cannot skip the work.
2. **Memory, then compaction.** Drop superseded nodes between rounds,
   remapping every `ExpId`-keyed table: the type maps, the units' owners,
   readers, counts and remembered lookups. Target: at or under the
   reference's peak on every program (`array.k` 1.4 GB against 0.55 GB).
3. **`desugar`'s algebras** (39% of `risc-v.k`): an algebra that borrows
   the node and returns `Option<Exp>` for a change, so a pass that rewrites
   one call does not clone and compare every node. A signature change
   across about fifteen algebras; exact by the dump diff.
4. **Symbol maps** (17%): the map kept and patched with each rewrite's
   `contributions` delta instead of rebuilt per pass.
5. **`deduce`'s and the wrappers' snapshots** (6.5%): reads of the module
   being rewritten go through its `Rewriter`.
6. **M5's leftovers**: `TyId` interning, and modules of a round elaborated
   in parallel (dependency-tracked, `ir-elaboration.md` §3).

**Exit:** `array.k` ≤ 1.5 s, `risc-v.k` ≤ 3 s, memory ≤ the reference's,
keystroke ≤ 400 ms.

**Progress.** 2026-10-10 (`performance.md` §10, "The round's floor"):
2, 4 and 5 done, 3 done by another route — the time and memory exits
met: `array.k` 0.95 s / 0.24 GB, `risc-v.k` 2.65 s / 0.68 GB (from 3.1 s
/ 1.4 GB and 6.2 s / 1.7 GB; the reference 0.55 and 1.1 GB). **2**: the
memory was macOS's allocator keeping freed snapshot copies, not the
arenas; compaction is not needed for the exit. **3**: measured, the
clones and compares were a quarter of each pass and the walk the rest,
so the passes walk only subtrees holding a shape they rewrite
(`kg_hir::facts`); the `Option<Exp>` algebra is what remains of it.
**4**: the maps are kept per subtree (`kg_sem::symbols::Filed`). **5**:
the snapshots refreshed from the arenas' and type maps' write logs. The
keystroke exit is W7's.

**Triaged 2026-10-11** (`performance.md` §11, §13; not scheduled, the
owner's to pick): **6**'s parallel rounds have evidence now — the shipped
reference runs its frontend's passes per module on every core and gains
2.2–3.5× from sixteen (`risc-v.k` 54.0 → 24.6 s), while our loop runs on
one; measured against the shipped compiler as it runs we are 8–16× faster,
per core 12–46×. A second lever the per-round profile shows: `array.k` and
`float.k` spend half their loop in a tail of cheap rounds, each rebuilding
the symbol maps (17% of `array.k`'s loop) and re-running deduction over
every module — **4** carried further, the maps merged incrementally
across rounds (`loop-across-edits.md` §6 step 4, which pays in a batch run
too). The C++ backend is 92–98% of a whole build and is measured only.

### W6 — The instruments

- `left` and "roots at the round guard" as ratcheted rows (W0).
- ~~`infer-identity` in the per-change loop~~ — done 2026-09-27:
  `scripts/fast.sh loop`. The cross-build dump as one command — **done
  2026-10-10**: `scripts/identity.sh [<commit>] [--filter …]` builds
  `<commit>` (default `HEAD`) in `target/identity-base`, dumps every
  root's fingerprint on both sides and names each root that differs; red
  under a planted literal-width mutant (three `data/cache` roots).
- `status-md` renders the new rows; `status.md`'s prose rewritten (W0.3).
- ~~The output gate's cache stays warm across edits to the replay code~~ —
  done 2026-09-27: the key is the replay code's meaningful text; a comment
  or test edit keeps it.
- ~~Per-program triage, mutation checks, worktrees~~ — done 2026-09-27:
  `scripts/probe.sh`, `scripts/mutant.sh`, `scripts/worktree.sh`,
  `gates.sh --snapshot`, the heavy lock (`CLAUDE.md`, "The loop, fastest
  first").

Added by the revision:

- **Absent is red; the reference pinned; `xtask lint`** — W0.4.
- **The verdict functions get tests.** `conformance::score`,
  `ptdiff::compare` and `structural`, `backend::judge` and `first_code`
  decide every headline number and have no unit test (xtask has 49 tests,
  none on these). Tests are added beside them, each proven by a mutant;
  the functions themselves are not edited — the scoreboard stays
  off-limits (`CLAUDE.md`) — and a test that finds one wrong is the
  owner's to rule on. `first_code` reads a code only from a line that
  *starts* with `Error `, where `syntax-test.py:115` searches the line:
  booked, symmetric today.
  — **`ptdiff` and `backend` done 2026-09-27**: 30 tests beside
  `compare`/`structural` and `judge`/`first_code` (`judge` unchanged,
  a `/bin/sh -c` script standing in for the compiler), 45 mutants, all
  caught. Three more behaviours booked for the owner, each a test marked
  so: `Error 0` stops the search (`syntax-test.py:116` searches on), a code
  past `u32` is passed over (`:119` reads it), and `Call Stack:` anywhere
  on our stdout is a crash (`:126` wants the whole line), checked on our
  side only. `conformance::score` is still open (the scoreboard, the
  owner's to rule on).
- **One reading of the options.** `conformance::settings_for` and
  `kg-cli/src/legacy/parse.rs` parse the same flags twice and already
  differ (`--define` splits at `#` in one, at `#` or `=` in the other,
  `conformance.rs:781`, `legacy/options.rs:575-581`). The gate's reading
  is checked against the CLI's over every registered test's flags.
  — **done 2026-10-10**: `xtask/src/options_agree.rs` reads every
  registration's flags both ways and compares the settings that decide a
  verdict. Its first run found one more than the plan knew: under
  `all-warnings-as-errors`' `--Wno-redundant-import … --Wall` the gate
  turns `redundant-import` back on, where the reference's `--Wall` adds
  only `Conversion` and `TransactionSizeMissing` and its binary prints no
  such warning (probed); the compat binary agrees with the reference.
  Booked in the test for the owner (the scoreboard is theirs); no
  assertion moves today. The `--define` split differs in principle and
  no registration exercises it.
- **The backend's messages, graded.** `backend` compares the first
  `Error N` and the exit code; the text — `Error N at: file (line)`,
  whose line is ours to get right — is compared by nothing. A `message`
  set in the `backend` ledger, paths normalized, is the measure of what
  §8's first decision costs. — **done 2026-10-10**: of the 256 programs
  both compilers reject with the same backend code (N > 1), the first
  error line — `Error N at: <file> (<line>): <message>`, the directory
  each ran in cut — is the same in **204**; the 52 others keep the code
  and the message and differ in the line number (`string.k::0`'s `Error
  53` at line 5 against 6: divergence #25's spans showing through).
  `Error 1` lines are the frontend's positions, which `errors` grades with
  its bookings (#7, #30), and are not counted here. Ratcheted (`message`,
  a pass set); `--verbose` prints both lines of each difference.
- **The backend cache's key covers what replays.** It omits
  `kg-ptbridge/src/backend.rs`, which holds `replay_call` (generated by
  `cargo xtask codegen` since W8.4; `build.rs` generated it before), and
  `kg-backend-sys/src/options.rs`, which marshals `Options`
  (`kg-ptbridge/src/cache.rs:72`); an edit to either is seen only by the
  one hit in 20 re-run. — **done 2026-10-10**: both are in
  `REPLAY_SOURCES`, pinned by `the_key_covers_the_replayed_calls_and_the_options`
  (red with either left out; its needles are spelled in pieces, since
  `cache.rs` is a keyed source too and a whole one found itself — the
  first mutant survived on exactly that). The next `backend` and
  `output` runs after this commit are cold.
- **Ledger names survive a reference sync.** Assertions are named by
  position (`syntax::basics::17`, `conformance.rs:269`), so a snippet
  added upstream renames every one after it in six ratchets at once.
  Before the pin (W0.4) is ever moved: `ratchet --rebase`, which maps the
  old names to the new by snippet text and reports only what it cannot
  map. — **done 2026-10-10**: `cargo xtask ratchet --rebase <old commit>`
  (`xtask/src/rebase.rs`) reads the old `test/syntax` with `git archive`,
  expands both corpora as `conformance` does, pairs each assertion with
  its twin — same file, expectation and text, identical snippets in order
  — and rewrites `baseline.json`'s items. Against the pin itself: 0
  renamed, the file unchanged; against `7508b5b~1` (upstream deleting
  `removed-syntax.k`): 15 with no twin, none shifted, as it should.
- **`edit-identity`, `resilience`, the keystroke table** — W7.1, W3.0,
  W0.5 — rendered by `status-md` as rows. — **done 2026-10-10**:
  `resilience` from its totals, `edit-identity` and the keystroke's worst
  from the run's stages (`gates.sh` now records the bench as one); the
  `backend` row's disagreements and the `output` row's differences,
  printed `?` since no ledger keeps them, are derived from the totals
  that do (`status_md::derived_counts_and_stage_rows_render`).

### W7 — The query layer: incremental, cancellable (added by the revision)

The first draft asked for a keystroke of 400 ms and named no mechanism
the query layer has. What the code does today, read 2026-09-27:

- `Db::item_tree` is keyed on the parse's pointer and `ItemTree` cannot be
  compared (`kg-db/src/db.rs:636-661`, `kg-resolve/src/lower.rs:171`), so
  any edit that changes the green tree makes a new item tree;
- `Db::program` is keyed on the item trees' pointers (`db.rs:671-694`)
  and `Db::looped` on the program's (`:1250-1262`), so that edit makes a
  new program and a loop from round one, base library included;
- `sorted_program` is memoized nowhere and lowers every module on every
  call (`kg-db/src/sorted.rs:41-67`); `looped` then forks each
  (`looped.rs:106`);
- nothing in `kg-db` or `kg-sem` can be cancelled, so a superseded check
  holds the one diagnostics lane until it ends;
- the editor therefore stops the loop at four rounds, and its diagnostics
  are those of a program elaborated part-way.

The design exists and was never scheduled: the interface firewall is
`docs/spec/resolve.md` §11.2 and §11.6, and the loop's per-module memo is
`ir-elaboration.md` §3, items 2, 5 and 6.

**The substrate stays hand-rolled** (§8 has this as the owner's to
overrule). The gates run on a `Db` per thread and memos keyed by root
set, the loop carries its own incremental walk, and re-keying every query
onto salsa in the middle of a parity sprint moves all of it at once. What
follows is the three things salsa would have given this workload — early
cutoff, cancellation, tracked inputs — each behind a gate that checks it
against a from-scratch run.

1. **The instruments, before any mechanism.**
   - `cargo xtask edit-identity`: over every corpus root, a warm `Db` is
     given an edit — W0.5's four kinds, and their undo — and its
     diagnostics, its loop fingerprint (`kg_sem::elaborate::fingerprint`)
     and its type tables are compared with a fresh `Db`'s over the edited
     text. Any difference is a stale memo and names the query. It is to
     W7 what `infer-identity` is to incremental inference, runs in
     `fast.sh loop`, and is green today by construction — nothing is
     skipped yet — which is why each mechanism below lands with the
     mutant that turns it red (a key with one input left out).
   - Memo-miss counters per query, asserted by unit tests: "a body edit
     misses `item_tree` once and `program` never" is a test, not a
     timing.
   - The bench's `keystroke` stage reports W0.5's table; `cancel` is a
     new stage (the time from an edit to the superseded check's return).
2. **Cancellation.** A token a query carries — the revision it was asked
   at — checked by `kg_sem::elaborate_opts` between rounds and between
   the passes of a round, by `Program::infer` between iterations and
   between units, and by `Db::frontend_diagnostics` between modules. A
   cancelled computation returns `Cancelled` and **writes no memo**. The
   server cancels a project's running check when an edit arrives, and
   honours `$/cancelRequest` for a request already started. Exact: an
   uncancelled run is byte-identical (`infer-identity`, the dump).
   **Exit:** `cancel` ≤ 50 ms on `risc-v.k`; a test that a cancelled run
   leaves every memo as it found it.
   — **done 2026-09-27.** `kg_base::cancel`: a token raised without a
   lock, checked at checkpoints that unwind with `Cancelled` (salsa's
   choice: a memo is written after its value, so a query the unwind passes
   through writes nothing; one it had finished keeps its memo, a correct
   answer). Checkpoints: between rounds and passes, per module in every
   loop pass and in base inference and the sorted lowering, between CST
   passes, and one call in 1024 of the rewriting and inference walks. The
   server raises a project's token as it queues an edit and a running
   request's on `$/cancelRequest` (answered `RequestCanceled`, or
   `ContentModified` when an edit stopped it); a cancelled query keeps its
   project. `bench --filter cancel`: the raise to the check's return,
   `risc-v.k` at the fixpoint, worst 14–32 ms over three runs — after a
   cancelled program's memory is freed on another thread; freeing it on
   the unwind was most of the 56–71 ms it measured before. Exact: the
   cross-build dump identical on 304 roots. Tests: a cancelled loop writes
   no memo and the next answer is a fresh `Db`'s; an edit lowers the
   token; a running request stops; a cancelled query keeps its database.
3. **The lowering, memoized per module.** `sorted_program` becomes a
   per-module query. *The key is a first reading, to be confirmed by
   `edit-identity` and its mutants, not trusted.* What the lowering of a
   module reads beyond its own parse, read in `kg-db/src/sorted.rs:78-180`
   on 2026-09-27: the module graph's answer to `find_in_imports` (alias
   stripping and re-exports are the graph's); of each module that answer
   names, the item's qualified name, scope and kind from its item tree,
   and its harvested shape and fingerprint
   (`kg_hir::sort::harvest_shapes`); and the `ItemId`, as `nub`'s token —
   an index, so a declaration added above it moves it. The key is
   therefore the parse and, per module the graph can reach from this one,
   what it exposes and its harvest, compared by value. Whether a harvest's
   fingerprint moves with a body edit is not yet read; if it does, this
   step saves the lowering of what the edited module does not reach and
   no more. **Exit:** a body edit lowers one module (the counter);
   `edit-identity` green.
   — **done 2026-09-27, on a narrower key than the first reading.** The
   premise, read first: a harvest's fingerprint *does* move with a body
   edit (it hashes the declared tree, body included), and it is *read* —
   `nub` takes two identical declarations for one (`SymbolTable.hs:229-237`),
   so a library body edit can re-sort an importer whose text did not
   change (`memo.rs`, `an_imported_body_edit_can_re_sort_an_importer`).
   But a token and a fingerprint are read nowhere else than that `nub`
   (`SymTab::one`), and only for an answer that met another hit. So the
   key is what the lowering read, not what the graph can reach:
   `kg_db::sorted::SortedMemo` keeps each module's IR and harvest with its
   parse (by pointer — the parse memo makes that its value) and every
   distinct question the lowering put to the import door with its answer,
   and is valid when each question, asked again, reads the same — the
   hits' count, shapes and scopes always, their tokens and fingerprints
   where `nub` compared them (`ImportSymbols::compared`). Counter `sorted
   module`, one miss per module lowered. `memo.rs`: a root body, signature
   or import edit lowers one module (1/3, 1/3, 1/2), a library body edit
   one (1/3; it was all). `bench --filter phases`, the sorted lowering's
   exclusive time per keystroke, before → after on one machine:
   `risc-v.k` root edits 0.18 → 0.12 s (the root is most of the lowering),
   a library body 0.18 → 0.006 s (1 of 50 modules); `array.k` 0.027 →
   0.007 s. Mutants: the answers left out of the key turn `edit-identity`
   red (1 root of the quarter run, `library/compiler/device/config.k`'s
   import edit), the parse left out 77 of its 87 roots and `memo.rs`; the
   fingerprint, the shape, and `nub`
   never reporting a comparison each turn a `memo.rs` test red and survive
   `edit-identity`'s quarter — the corpus's four edits never change an
   importer's answers without its text, so those tests are the instrument.
   A token-only change stays unobserved by any test: a token decides `nub`
   only between two hits with no fingerprint, and it is compared anyway.
   Exact: `infer-identity` 304/304, the cross-build dump against
   `69c878c` identical on 304 roots, `fast.sh loop`'s ratchets +0/−0, the
   whole `edit-identity` 348 roots and 1123 edits green (323 s, 6.5 GB
   against 281 s, 6.2 GB — the memo keeps a lowered IR per module).
4. **The interface firewall.** *Also a first reading.* Two layers, read
   in `kg-db/src/db.rs:771-845` on 2026-09-27:
   - **Base inference is already per module**, keyed on the module's item
     tree and its direct imports' converged `ModuleTypes`, both by pointer
     (`TypeTableMemo`). What breaks the cutoff is only that a module
     inferred again hands out a new pointer, equal or not, and every
     importer follows. `ModuleTypes` is a map of names to `Ty`, and `Ty`
     holds no position (`kg-ty/src/ty.rs` names no range), so it can be
     compared by value — or the old `Arc` handed back when the new map
     equals it, as the parse does with its green tree.
   - **But the key is exact today only because pointers move with
     everything.** `infer_module` also reads the module's constant
     environment (`file_scope`, with the imports' constants,
     `db.rs:894`) and the program's graph (`Imports::new`), and neither is
     in the key. With `ModuleTypes` compared by value, an imported
     constant whose *value* changed and whose type did not would leave an
     importer's folds stale. So the constants an importer can read, by
     value, and what the graph exposes to it join the key before the
     comparison changes — this is the mutant `edit-identity` must turn
     red first.
   - **The program.** `Db::program` is rebuilt on any item tree's pointer,
     and `inference`, `looped` and the per-module scopes key on the
     program's. `ModuleInterface` as `resolve.md` §11.2 has it — what an
     importer can see, **holding no position**, since `Item::range` moves
     with any edit above it — is what `Db::program`'s graph and those
     memos key on instead. A message that prints a declaration's location
     reads the declaring module's item tree when it is rendered.

   **Exit:** a body edit in a library module recomputes no importer's
   base table; `edit-identity` green, with the two mutants (a visible
   name left out of the interface; a constant's value left out of the
   key) red.
   — **done 2026-09-27, keyed on what was read — the reading half
   false.** The premise, reproduced first (`4778c7b`, `db.rs:868-912`,
   and measured): base inference was per module, keyed on the tree and
   the direct imports' `ModuleTypes` by pointer, and a library body edit
   re-inferred the edited module's importers — true (`bench --filter
   phases`: `risc-v.k` `module inference` 4/50, `array.k` 11/39;
   `memo.rs` 2/3), and every module's `file_scope` with them (45/50,
   37/39: it was keyed on the program's pointer). `ModuleTypes` holds no
   position — true. But "compared by value" was false twice: the map
   holds every declaration, a body's locals too (`f::$::x`), so a body
   edit that declares one makes it unequal (probed: `memo.rs`'s local,
   `array.k`'s root body and signature edits; `risc-v.k`'s land in a
   template, which files nothing below it); and `Ty`'s `==` ignores a
   parameter's name (`Type.hs:126-129`), which an importer's table
   holds. The reads outside the key: the fold environment (`file_scope`,
   the imports' constants), as read; the graph through `Imports` — which
   also types a re-exported hit off its *declaring* module's map, not a
   direct import's, so direct imports compared by value would miss it;
   and one not named, `late_constants`, keyed on the scope's pointer
   alone while it folds against the module's table. `Db::program` moves
   with every item tree's pointer, and `inference`, `looped`,
   `global_symbols`, `ir_templates` and `instances` with it — still so.
   So the key is W7.3's shape: `passes::types::ImportsRead` keeps each
   question the fixpoint put to the import door with its answer, valid
   while each reads the same by `Ord`; `file_scope` keys on the tree and
   the imported constants by value (the program's pointer the cheap
   check) and hands back its old `Arc`; `infer_module` keys on the tree,
   that scope and the reads; `late_constants` on the scope and the table.
   Counters, `bench --filter phases` A/B against `4778c7b`, one loaded
   machine: a library body edit infers one module — `risc-v.k` 4/50 →
   1/50, `array.k` 11/39 → 1/39, `memo.rs` 2/3 → 1/3, its local-declaring
   edit too — and folds one scope (45 → 1, 37 → 1; every root edit the
   same); exclusive time `risc-v.k` 24 → 1 ms, `array.k` 13 → 1 ms, of a
   1.7 s and 0.38 s keystroke that is still the loop's. Mutants, each red
   in `memo.rs`: the answers never compared (the importer's reads out of
   the key) turns the imported signature, parameter-name and width tests
   red; compared by `==`, the parameter-name test; the imported
   constants' values out of the scope's key, and the scope out of the
   table's, `an_imported_constants_value_re_folds_an_importer`; the table
   out of the late fold's key, `an_imported_width_re_folds_a_late_constant`.
   `edit-identity`'s quarter stays green under the first and third,
   measured — its edits never change what an importer reads, so the unit
   tests are the instrument, as at W7.3. Not done, and not needed for the
   exit: `Db::program` keyed on a `ModuleInterface`. `looped` reads every
   body and reruns on a body edit whatever the program's key, and §11.2's
   interface maps names to `ItemId`s, which a local declared above them
   moves (W7.3) — a question for W7.5d. Exact: `infer-identity` 304/304;
   the cross-build dump against `4778c7b` identical on 304 roots; the
   whole `edit-identity` 348 roots, 1123 edits identical (318 s, 6.2 GB);
   `fast.sh loop`'s ratchets +0/−0.
5. **The loop across edits — a hypothesis, measured before it is
   designed.** `ir-elaboration.md` §3.2 has a module's round as a function
   of its own IR, the registry entries it is home to and its imports'
   interfaces. Nothing in today's loop is shaped that way: one `Program`
   holds every module, inference is one fixpoint over all of them under a
   program-wide symbol map, a round's passes walk every module, copies
   requested by a root are made in the library module that declares the
   template, and the round a copy was decided in is observable
   (`ExpansionData.round`, the copies' order in `ptdiff`). So a library
   module's elaborated state depends on the root, and a memo per module is
   not a refactor of what exists. The journal said as much before the
   loop was written — "Whole-program, with no import-order firewall …
   the per-module memo does not survive; the `(template, args)` instance
   memo recovers the copy and nothing else" (journal, "What this makes hard") — which makes
   the content-addressed copy (`ir-elaboration.md` §3.5), not the module,
   the likelier unit to keep across an edit.

   What one reading shows (`KG_LOOP_TIME=1 target/release/kg check -I
   library -I test/library FILE`, 2026-09-27, one run each, the machine
   not checked quiet — to be redone under W0.5):

   | | passes 1–7 | rounds | a round | the last four rounds |
   |---|---|---|---|---|
   | `test/library/data/cache.k` | 0.02 s | 17 | ~0.03 s | 0.11 s, deciding 4 instances |
   | `test/library/processor/risc-v.k` | 0.06 s | 17 | ~0.28 s | 0.72 s, deciding 5 |

   Two consequences. Keeping the state *before* the rounds across edits
   would save a percent or two: the cost is the rounds. And a round costs
   about the same whether it decides 287 instances or one — it is a floor
   of whole-program walks (the wrappers, the prefix, deduction, the
   census), paid per round. The editor's four rounds of `risc-v.k` are
   about a second of its 1.46 s.

   So, in this order, each a day or two and each ending in a number:
   - **a.** Where a round's time falls, per module: the root against the
     library, and how many nodes each round appends to each. If the floor
     is mostly walks of library modules that gained nothing, then a pass
     that skips a module with nothing new since it last ran is an exact
     optimisation *within* a run — graded by `infer-identity` and the
     dump, helping `kg check` and the gates as much as the editor, and
     needing no memo across edits at all. It is W5.3–W5.4's work, aimed.
   - **b.** W0.2's round counts: seventeen rounds where the reference
     takes fewer is the same floor paid more often.
     — **Premise false (2026-09-27, `loop-left`):** the reference takes the
     same count on every root, 321/321, once the guard stopped cutting
     five short. There are no extra rounds to remove; the floor is the
     cost per round (a, W5.3–W5.4).
   - **c.** Is a library module's state independent of the root, copies
     the root asked for aside? The fingerprints of the base library's
     modules under two roots, compared per module and per round. If the
     rounds' dating differs, the memo per module cannot be exact as
     designed, and what remains is carrying one root set's whole
     `Program` — units, symbol map, types — across an edit to the root
     alone, the edit treated as a pass that rewrote one module.
   - **d.** Only then the design, as a plan of its own
     (`docs/plans/loop-across-edits.md`), with `edit-identity` as its
     gate. It stands on W1.1's registry either way.

   Until **d**, the keystroke's dependable levers are W7.2 (a superseded
   check stops), **a**, **b**, and W5.2–W5.5.
6. **Layered answers, and the round cap deleted.** A module's own
   diagnostics are published as soon as its base passes end; what the
   loop adds arrives when it finishes, the last good answer served
   meanwhile. `PassBudget::INTERACTIVE` goes, and the editor runs to the
   fixpoint as `kg check` does, once the fixpoint is affordable — by 5's
   outcome, or sooner by cancellation alone if the owner prefers a late
   true answer to an early partial one (§8). **Exit:**
   the editor's diagnostics equal `kg check`'s on every corpus root (a
   row of `edit-identity`).
7. **The inputs the server does not see.** It watches `kanagawa.toml`
   alone (`main_loop.rs:315-323`, `editors/vscode/src/extension.ts:79`),
   so a branch switched under it leaves stale imports until restart:
   watch `**/*.k` and route the events to `Db::file_changed`, which was
   built for them. `[lsp] index-import-dirs` is parsed and read by
   nothing — read it or remove it. A file with no manifest loads no base
   library and says nothing (`kg-config/src/resolve.rs:94-110`): it says
   so. Small, independent, any time.

**Progress.**
- **1 — done 2026-09-27.** `cargo xtask edit-identity`: each root loaded
  into a `Db` kept warm across the corpus, each of the four edits
  (`xtask/src/edits.rs`) made in its overlay and undone, and after every
  step its diagnostics, loop fingerprint and every module's table
  compared with a fresh `Db`'s, file ids and symbols spelled out. First
  reading: 348 roots, 1123 edits and their undo, all identical, 281 s,
  6.2 GB — in the milestone run whole, in `fast.sh loop` every fourth
  root (77 s). Mutant: an item-tree memo that ignores its parse is red on
  every edit kind. The memo-miss counters are `kg_db::stats`, pinned by
  `crates/kg-db/tests/memo.rs` — each edit's misses as a line, today's
  answer; a W7 mechanism changes its line. The bench's table is W0.5's;
  `cancel` waits for W7.2.
- **5a — measured 2026-09-27, and the hypothesis is false.**
  `KG_LOOP_PROFILE=1` times each pass's visit to each module and calls a
  visit *idle* when the module's arena has not grown since that pass last
  visited it (an upper bound: inference rewrites in place). Idle visits
  are ~70% of visits and **5%** of `risc-v.k`'s loop (0.25 s of 4.86 s),
  10% of `array.k`'s, 27% of `data/cache.k`'s. Skipping them is not the
  lever. Where the time is: inference 29%, the three symbol maps rebuilt
  whole each round (`wrappers'`, `deduce's`, `census's`) 17%, the prefix
  (`desugarLambdas` … `moveFunctions`) 21%, and the visits to modules
  that *did* grow — the root, and the library modules that receive its
  copies — walked whole for the few nodes they gained. So the floor is
  W5.3–W5.4's (the algebras, the symbol maps) and a walk that visits only
  what changed within a module, not one that skips modules.
- **5b — premise false (W0.2 above).**
- **5c — measured 2026-09-27: dated apart.** `cargo xtask loop-modules
  A B` compares, per library module both programs hold, what the module
  wrote and the copies made in it. Written parts agree in 35 of 36
  (`array.k`/`vector.k`), 37 of 38 (`cache.k`/`risc-v.k`) and 35 of 36
  (`string.k`/`optional.k`) — each time one module differs by a node's
  type. But copies both roots asked for were decided in different
  rounds: 57 of 90, 4 of 13, 0 of 9. The round is observable
  (`ExpansionData::round` orders copies and dates the mentions inside
  them), so a memo per module is not exact as `ir-elaboration.md` §3.2
  designs it; what remains for **d** is carrying one root set's whole
  `Program` across an edit to that root, the edit treated as a pass that
  rewrote one module — or copies keyed by content (§3.5) with their
  round re-derived.
- **5d — drafted 2026-09-27, awaiting the owner's review.**
  [`loop-across-edits.md`](../plans/loop-across-edits.md): a library module's
  round is a function of its state, the copies decided into it that
  round, and the answers it read across the module boundary — so a
  replay per module and round keyed on those reads (W7.4's technique) is
  exact where "a memo per module" is not. The library's per-module passes
  are 27% of `risc-v.k`'s loop and 47% of `array.k`'s
  (`KG_LOOP_PROFILE=1`, one run each), the prize's upper bound before
  its share of inference and the symbol maps. One measurement first
  (5d-0: in which round an edit first changes each library module's
  requests); D2 (carry the whole program) recommended dropped.
- **5d-0 — measured 2026-10-10: D1 pays, with positions relocated.**
  `cargo xtask loop-rounds` (348 roots, 1123 edits, each loop run to the
  fixpoint before and after; `loop-across-edits.md` §5, "Measured
  2026-10-10"): the library's per-module time in rounds before the
  module's copies first differ, pooled — as written / relocated / state
  checked — body 51 / 99.8 / 99.8%, signature 53 / 94 / 92%, import 54 /
  82 / 81%, library body 86 / 100 / 100%, all 62 / 96 / 95%; rounds 0–3,
  state checked, 97.7%. Per edit, relocated or state checked, the median
  keeps all of it and 99.5% of edits keep more than half. As written a
  root edit moves the sites and arguments of every copy it asks for, and
  the slowest programs keep 8–13% of a body edit's library work
  (`risc-v.k`, `array.k`, `hash_table.k`; 100% relocated). The root's
  requests differ in round 0 on 9.8% of edits once relocated (44% as
  written), import edits most (36%). After an edit the library's
  per-module passes are 62.5% of the loop, the edited module's 11%, the
  whole-program steps 26%: D1 would keep 59% of the loop pooled, ~41% of
  `risc-v.k`'s on a root edit. Reads across the boundary ((3)) are not
  measured as reads; the state-checked reading bounds what they did to
  the state — 44 edits, 0.6 points — found through
  `library/test/unit.k`'s `fixture<TestN>` calling the edited `run()`. By
  §5's rule D1 is built in §6's order, relocation in step 1's design
  (written there, with its memory estimate). Two runs, load 12–17 on 16
  cores: module-rounds identical, time fractions within 3 points. The
  loop's hooks are trace only (`kg_sem::profile::record`): identity
  304/304 against `cea5957` (`scripts/identity.sh`), `infer-identity`
  304/304.
- **5d, step 1 — the snapshot and its shadow, built 2026-10-10; no
  reuse.** The editor's loop (`PassBudget::Layered`) records each
  module's length, root, undo log (the old node of every write in place
  after round 0, the old entry of every type a write changes, strictly)
  and key (the copies decided into it and its own mentions, 5d-0's
  spelling moved into `kg_sem::snapshot`, positions held aside) at every
  round; the `Db` keeps the run's final program with that history per
  root set (`kg_db::snapshot`), shared with its `Looped`, until the next
  run of the root set reaches its own. A batch compile records and keeps
  nothing. The shadow — `edit-identity`'s warm database, every step after
  the first — restores each kept round whose key agreed, relocates it
  through the one splice between the kept text and the new
  (`Splice::between`), and compares it node by node with the round run:
  over 348 roots and 2246 steps keys agreed in 92.2% of module-rounds
  (body 99.9%, signature 96.4%, import 66.1%, library body 100%) and
  691352 of those 692278 restored equal — every unequal one a signature or
  import edit's read across the module boundary, §3's (3), which no key
  holds until step 2 (46 edits, 94 modules; `library/test/unit.k` 48).
  One read carries positions: `resolvedFuncParams` copies another module's
  parameter list in, so step 2's door must relocate its answers as the
  keys' are. The ledger `shadow` books them (`unequal`, fail; `kept`,
  pass: 1808 steps), awaiting the lead's bless; `fast.sh loop`'s quarter
  is held to the baseline's items for its roots. Mutants, red on that
  quarter: a type write out of the log, a node write out, every round
  keyed by round 0's key, a node restored a round early, relocation
  skipped. Measured (two interleaved A/B runs against `b4ee408`, load
  15–33; `bench --filter keystroke-phases --recorded`): the answers main's
  within the noise once the keys cost a third of what they first did
  (`risc-v.k` 117 → 48 ms a run, symbols left numbered) and the replaced
  program is freed after the answer; the editor's peak footprint 1.33–1.36
  → 1.78–1.80 GB over both files (`kg lsp` over stdio on `risc-v.k`
  1.37–1.48 → 1.87 GB) — the run before's program, held to the next
  fixpoint; beside the program it shares, the snapshot holds 5.8 MB on
  `risc-v.k` and 2.1 MB on `array.k`. Accepted by the owner the same day
  (`loop-across-edits.md` §7). A batch compile's time is main's, and its
  peak main's under `cargo xtask mem` (`risc-v.k` 770–771 → 767–772 MiB
  footprint; the `kg` binary reads 17 MB more, unexplained, §6 there); the
  editor after five edits `risc-v.k` 1194–1214 → 1586–1610 MiB footprint,
  its resident set 2087–2137 MiB over its 1910 budget, for the lead to
  bless. Identity: `scripts/identity.sh` against `b4ee408` 304/304;
  `edit-identity`'s warm, recorded loop against the batch one 348/348.
- **5d, step 2 — the read door, built 2026-10-10; no reuse.** Every pass
  that reads across a module boundary in a recorded run shows one `Door`
  (`kg_sem::reads`) what each module read — lookups with their answers,
  another module's nodes, types and sources (inference's
  `Cx::snap_*`, a wrapper's declaration and the parameters it copies, a
  deduction's template signature, an alias's function type), and what the
  program decides as a whole — and each module's round ends with a second
  key beside its copies': the set of items, each spelled where it was
  written (not by id: a body edit renumbers the root), positions held
  aside by rank so relocation carries them. A deduction, a wrapper and
  `resolve`'s spread keep what they read only where it changed the module.
  The shadow follows a module only while both keys agree: over 348 roots
  and 2246 steps, none unequal (step 1: 94 modules, 926 module-rounds);
  keys agree on 91.9% of module-rounds (body 99.9%, signature 96.0%, import
  65.3%, library body 100%), 690288 against step 1's 692278 — the door's
  caution, rounds its reads alone part that were equal all the same, 138
  of 228. The ledger `shadow`: `unequal` 94 → 0, `kept` 1808 → 1746 (62 steps,
  58 of them steps step 1 found a module unequal in), for the lead to bless.
  Measured (two interleaved A/B rounds against `4db41f1`, load 5–16):
  the answers main's within the noise, the door 1–2% of the loop sampled;
  `cargo xtask mem` within the noise, all fifteen within budget; the
  reads' keys hold at most 179 KB beside a kept program. Identity:
  `scripts/identity.sh` against `4db41f1` 304/304, `infer-identity`
  304/304, `fast.sh --lint loop` green against the shadow's new baseline.
- **5d, step 3 — the prefix pass reused, built 2026-10-11.** The editor
  takes each followed module's prefix pass of each round from its kept run
  where every key before the pass agreed (`kg_sem::reuse`): the nodes the
  pass appended as it left them, heads carried through the edit,
  expansions found in the run's interner, children named through the same
  chain of forks (`ExpArena::translate`), the root, the types carried by
  the pass's own pairs. `edit-identity` runs every pass taken on a fork as
  well and compares: 700250 passes reused of 700348 offered over every
  root, 20.8M nodes, none unequal (`reuse-unequal`, a fail set; totals
  `prefix_offered`, `prefix_reused`); 348/348 identical; the shadow as
  before. Measured (two interleaved A/B rounds against `3e26911`, load
  4–9): the keystroke main's — the prefix is 0.146 of `risc-v.k`'s 0.167
  s the edited root's own, and taking in the library's 21 ms costs what it
  saves; `cargo xtask mem` +17–25 MiB after five edits on `risc-v.k`, the
  rest main's; at most 6.2 MB beside a kept program. The library's time is
  in inference's walk (0.375 s), `instantiate` (0.098), deduction (0.085):
  passes that read other modules, which need the door's reads made
  re-askable before a later run can reuse them (`loop-across-edits.md` §6,
  step 3). Step 4, the symbol maps, planned there, not built. Identity:
  `scripts/identity.sh` against `3e26911` 304/304, `infer-identity`
  304/304, `fast.sh --lint loop` green.
- **6, the owner's answer (c) — done 2026-10-10 (W7.6).** The editor
  publishes at round four and again at the fixpoint, from one run.
  `kg_sem::elaborate::elaborate_layered` hands the run's own state to a
  callback as it passes the bound (`Stopped`), whose `answer` forks every
  module and ends the fork as a bounded run ends itself; the run goes on,
  so rounds 0–3 are computed once. `PassBudget::Layered(4)`
  (`PassBudget::EDITOR`) replaces `Interactive`/`INTERACTIVE`: under it
  the loop runs on a thread of its own (`kg_db::layered`), each reader
  waits for the answer it wants and goes, and the first to want one
  builds its `Looped`. The lane publishes `Tier::First` then `Tier::Full`
  under one ticket (`Db::diagnostics_layered`); hover, inlay hints,
  signature help, navigation and code actions read the newest answer
  ready (`Db::looped_first`) — never waiting for the fixpoint, never a
  second run; an edit cancels the run (the database's token, and the
  run's own, which every input change raises), no last answer is
  memoized, and the first stays on screen until the edit's own first
  replaces it. **Exit met:** `edit-identity`'s warm database is the
  editor's, and its row reads the editor's last answer equal to `kg
  check`'s on 2585 of 2594 steps over all 348 roots — the nine others the
  per-file parse gate, by design — and each of 1437 first answers within
  its last. The row's first run found the round-four answer *gaining* an
  error: "Undefined template" on `fj.fork<0>` over an instance the next
  round makes (`test/library/control/async.k`, the two rendezvous
  tests), now not judged on a truncated loop. `infer-identity`'s
  incremental run is the layered one: 304/304, and 159 first answers
  each the loop bounded at four; `scripts/identity.sh` against `cea5957`
  304/304. Measured 2026-10-10 (`bench --filter keystroke-phases
  --recorded`, two interleaved A/B runs against `cea5957`, load 7–31):
  `risc-v.k` 1325–1353 ms at four rounds → first answer 926–942 ms, last
  2156–2171 ms; `array.k` 213–232 ms → 239–243 and 875–907 ms. The fork
  costs 25 ms a keystroke on `risc-v.k`, 7 on `array.k`; the editor's peak
  footprint on the two is 0.82 GB → 1.29–1.33 GB, the fixpoint's own
  state (a batch run of `risc-v.k` alone: 1.24 GB). The first answer is
  faster than main's four rounds because those judged "Undefined
  template" over instances not yet made and placed each by a full tree
  scan (`capture_list_note`, 0.45 s a keystroke). `bench`'s `cancel`
  stage, the editor's lane: worst 6.5 ms. The journal's "The editor
  answers twice: four rounds, then the fixpoint" has the rest.
- **7, the watcher and the setting — done 2026-10-10.** The VS Code
  client watches `**/*.k` beside `kanagawa.toml`, and the server routes a
  source's event to `GlobalState::disk_changed` — the disk cache
  dropped, every database told (`invalidate_everywhere`) — and
  republishes the open files: a branch switched under an open buffer
  reaches its diagnostics without a restart (e2e
  `a_source_changed_on_disk_reaches_the_open_files_diagnostics`, red with
  the event ignored or nothing republished). `[lsp] index-import-dirs` is
  read: workspace symbols search every `.k` under the import directories
  too, so a declaration nothing imports yet is found by name (`risc-v.k`
  under `library/`: 27 ms the first query, 4–6 ms after). A file with
  no manifest above it is said to have none, once, in the server's log —
  it loads no base library and resolves no import — and manifest
  problems are reported after the opening file's settings are resolved,
  not on the next open (`a_file_with_no_manifest_is_said_to_have_none_once`).

**Exit:** §1's four rows — what a body edit recomputes, the superseded
check, the editor's rounds, the keystroke — with `edit-identity` green
and each firewall's mutant red.

### W8 — What the tools stand on (added by the revision; small, independent)

Each gate-neutral — every ledger byte-identical — and each a commit; they
fill W1's gaps as W2 does, and 1–3 come before the work that would
otherwise build on what they replace.

| # | Item | Today | Before |
|---|---|---|---|
| 1 | **A diagnostic carries its kind.** A `DiagnosticKind` in `kg-diag`, one variant per message family of the reference, cited; the legacy renderer still prints `Error 1`; `--error-format json` and the server's `code` carry it | the message is the only identity, and it is control flow: a prefix decides whether the pass-9 checkpoint closes the pipeline (`kg-db/src/passes/mod.rs:172`), and the two engines' diagnostics are merged by message equality (`db.rs:1534`); also `deduce_auto.rs:387,404`, `navigate.rs:1155`, `kg-sem/src/check.rs:75` | W1.4 and W3, which both add merging between engines; `xtask lint` holds it |
| 2 | **Verdict switches are settings.** `KG_LEFTOVERS`, `KG_SEM_ONLY`, `KG_PTBRIDGE_SEED`, `KG_INFER_FULL` become fields the `Db`'s memo keys include; the variables are read once, at the front door | read from the environment inside the compiler (`check.rs:999`, `walk.rs:884`, `:1648`, `elaborate.rs:104`), invisible to every memo | W1.4, whose switch is one of them |
| 3 | **The type model apart from the CST engine.** `kg-ty` keeps `ty`, `width`, `subtype`, `convert` and `TypeKey`; `infer.rs` moves to a crate of its own, which `kg-hir`, `kg-sem` and `kg-ptbridge` do not depend on | whoever wants `Ty` links the engine M6 deletes; `kg-hir` names `kg_ty::infer::TypeKey` (`arena.rs:59`) | W3.7, which then deletes a crate |
| 4 | **The tools build without the reference.** The generated bindings are checked in and `xtask codegen --check` compares them with bindgen's over the pinned headers; the compiled-in developer path leaves `locate.rs` | building `kg` needs `../kanagawa` and libclang (`kg-abi-gen/src/lib.rs:57-66`), though `architecture.md` says the workspace "still tests standalone" | anyone else installs it |
| 5 | **The bindings know their library.** At load, the backend's version against the headers' the bindings were generated from; a mismatch is an error before the first call | trusted (`kg-backend-sys/src/lib.rs:113-122`); the layout tests check the headers, not the library | the reference's pin is moved |
| 6 | **The compat binary's gaps, closed or booked.** `--template-iterations` and `--template-passes` reach the loop's bounds (a registered test passes the first); `--file-list` writes the manifest (`Main.hs:78-79`); `list-deps`, `pretty` and the `--dump-*` flags are implemented or entered in `known-divergences.md` | parsed and ignored (`legacy/options.rs`); `pretty` exits 1 | the cutover is claimed |
| 7 | **Small honesty.** A `LICENSE` file for the `license = "MIT"` the manifest declares, and `repository` naming this project; `-j` does something or is refused; a formatter verification failure in the server is a message, not an empty edit (`kg-lsp/src/fmt.rs:68`); the stale headers (`kg-db/src/lib.rs:41`, `sorted.rs:12-13`, `kg-cli/src/lib.rs:14-26`, `editors/vscode/README.md`) | — | any time |

**Progress.**
- **7 — done 2026-09-27.** `LICENSE` (MIT, "The kgrust authors", as the
  manifest's `authors` field says); `repository` removed from the
  workspace manifest — it named the reference's repository, and this
  project has no remote to name; the server logs why it declines to format
  (a file with syntax errors, or a layout that would change the program)
  instead of sending an empty edit in silence; `kg-cli`'s header and
  `editors/vscode/README.md` describe what the tools do today (the README
  said formatting was absent until `kg-fmt` existed). `-j`: the
  reference's `--jobs` sets only GHC's capability count (`Main.hs:61-68`)
  — speed, never output — so the compat binary accepting it with no effect
  is faithful; `kg -j` is documented as having none.

**Exit:** `xtask lint`'s message-test and environment-read counts at 0;
`kg` and `kg-lsp` build from a checkout with no sibling; every ledger as
it was.

**Progress.**
- **1 — done 2026-09-27.** `kg_diag::DiagnosticKind`, three variants — the
  three families a pass decides by: `CouldNotDeduceAll`
  (`Frontend.hs:421`; the pass-9 checkpoint, `deduceAutoTypes`' return
  arm), `StaticIfUndecided` (`Frontend.hs:406`; the return arm),
  `ConflictingDeduction` (`Type.hs:1502-1506`; `kg_sem::check`'s round
  errors). `Diagnostic::kind`, set where each is made — pass 4's and pass
  12's LT, pass 16's `static if`, `kg_sem::check`'s two — and on the IR:
  `Exp::Error` holds a `kg_hir::ErrorNote` (message and kind), and
  `kg_sem::deduce`'s table carries `reconcile`'s kind into it. `--error-format
  json` prints `"kind"`, the server's `data` carries it; the legacy
  renderer is unchanged. `lint` message-test 4 → 0. Every ledger
  unchanged (`fast.sh loop`: conformance 2571/2571, every ratchet +0/−0),
  `infer-identity` 304/304 and its dump identical to `2a64ce7`'s. Mutants:
  each kind test and each tag red under a unit test, but for
  `kg_sem::check`'s — every conflict node is minted `Donated`, so no
  program reaches the test; the rule is kept, and says so. The two
  engines' diagnostics still merge by whole-diagnostic equality
  (`db.rs`'s `same`), which is identity, not a family test.
- **2 — done 2026-09-27.** `kg_config::Verdicts` — `leftovers`,
  `infer_full`, `sem_only`, `ptbridge_seed` — is `Settings::verdicts`, so
  a change is a settings revision and the program, the loop and its
  check are rebuilt (`a_verdict_switch_is_a_setting_every_memo_sees`,
  red with `set_settings` keeping the revision). `kg_sem` takes the
  switches as parameters (`check::diagnostics`' `leftovers`,
  `Program::full`), the walk through `ModuleCtx::with_switches`.
  `KG_LEFTOVERS`, `KG_INFER_FULL`, `KG_SEM_ONLY`, `KG_PTBRIDGE_SEED` are
  read by `Verdicts::read` at the front doors only — `kg`'s environment
  layer, `kanagawa`'s compile (`kg_cli::verdicts_from_env`), `kg-lsp`'s
  config store — and mean what they meant (a unit test per spelling).
  The gates' in-process databases grade the defaults; `xtask` warns when
  one is set (journal). `infer-identity` already passed `full`
  explicitly; its `--dump` now follows the settings. `lint` env-verdict
  5 → 0. Every ledger's sets identical to item 1's run, `infer-identity`
  304/304, the dump identical to `2a64ce7`'s.
- **3 — done 2026-09-27.** The engine is `kg-cst-infer` (the round, the
  algebra, deduction — `infer.rs` less its table, 81 tests); `kg-ty` keeps
  the model and gains `table.rs`: `TypeKey` as planned, and `TypeTable`
  too, which the plan did not name — it is data, not engine: the loop
  derives one per module for the walk (`Looped::table_for`), and
  `kg_hir::NodeTypes` reads one, so moving it would have taken `kg-hir`
  and `kg-ptbridge` with it (6 table tests moved with it;
  `type_key_separates_a_left_nested_operator_chain` rewritten over a bare
  parse). Only `kg-db` names the engine; `kg-hir` and `kg-sem` do not link
  it (`cargo tree`). **`kg-ptbridge` still does, through `kg-db`** — a
  transitive dependency a crate move cannot cut — and its walk still
  *reads* the engine's answers: `walk.rs`'s `type_of` falls through to
  the table where the IR engine is unresolved, and that table is
  `db.type_table` where the loop derived none. W8.2's measurement
  (`KG_SEM_ONLY=1`, the fall-through off: `ptdiff` the same 2373
  structural set, 44 diverged, 5 declined) says the fall-through decides
  nothing today, so W3.6/W3.7 can drop it; that is a verdict change to
  grade there, not part of a move. Evidence: `infer-identity --dump`
  from the pre-split release build against the split's, 304/304
  identical; `scripts/fast.sh loop --lint` every ratchet +0/−0.
- **4 — done 2026-10-10.** `kg-backend-sys`'s bindings and
  `kg-ptbridge`'s `Backend` trait are checked-in sources
  (`src/bindings.rs`, `src/backend.rs`) under an `@generated` banner
  naming the headers, the reference commit and a digest of the headers;
  `cargo xtask codegen [--kanagawa-root R]` writes them through
  `kg-abi-gen` (`bindings`, `backend`, beside the one bindgen
  `configure` that `kg-ptrace`'s build script still uses), refusing an
  unpinned reference or edited headers, and `--check` — `gates.sh`'s
  codegen stage, now given `$R` — regenerates and compares; without the
  headers `--check` is red and writing skips them, saying so. The bindings
  are formatted by `prettyplease`, not whatever `rustfmt` a build found.
  Behaviour unchanged: `backend.rs` below its banner is byte-identical to
  the build script's output, and `bindings.rs`'s bindgen part unparses to
  the old file's text. Both build scripts and their build-dependencies
  are gone; `default-members` leaves out `kg-ptrace`, `kg-abi-gen` and
  `xtask`. Evidence: `cargo tree -p kg-cli|kg-lsp -e normal,build` holds
  no bindgen, clang-sys, `kg-abi-gen` or `kg-ptrace`; a `git archive` of
  the branch in the scratchpad, with no `../kanagawa` beside it,
  `KANAGAWA_ROOT=/nonexistent/kanagawa`, `LIBCLANG_PATH=/nonexistent`:
  `cargo build --release --locked --offline -p kg-cli -p kg-lsp` green
  in 25 s, compiling neither bindgen nor clang-sys; a plain `cargo build`
  green; `--workspace` red at `kg-ptrace`'s build script only, as
  designed. The compiled-in `../kanagawa-build` candidate left
  `locate.rs` (the gates and scripts name the library;
  `KG_BACKEND_DIR` for a developer's run). Still needing the sibling:
  `kg-ptrace` and so `--workspace`, `codegen`, the corpus tests and
  every gate, the oracle, and the backend at run time. The cache key's
  four replay files are untouched but for one `//!` line; it still omits
  the generated `replay_call` (W6). `scripts/fast.sh walk` with items 4
  and 5 in: conformance 2602/2602, every ratchet +0/−0 (strict, errors,
  lint, cli, ptdiff 2422 structural, backend 3267 agree), `backend` 3270
  programs in 46.2 s — the cache warm, its key unmoved.
- **5 — done 2026-10-10.** `Backend::load` refuses, before any call, a
  library that does not define every function the headers declare but
  `ParseStages` (`Refused::Mismatch`, naming what it lacks, the commit and
  the headers' digest). The library carries no version — no ABI symbol,
  `LC_ID_DYLIB` 0.0.0, `RELEASE_VERSION` only in the Haskell executable —
  so the symbol set is what can be compared: it catches a library built
  from headers lacking a declared function and a file that is not the
  backend, and is blind to a changed signature, `Options` layout or enum
  numbering under the same names and to functions the library adds; those
  are held at generation (`codegen --check`, the layout tests). Driven:
  `KG_BACKEND_DIR` at another library under the backend's name, `kg
  build` exits 2 and `kanagawa --backend=sv` 1 with the refusal; the
  reference's backend compiles. `tests/smoke.rs` now fails, rather than
  skips, on a library found and refused. Mutants: the check disabled, and
  `ParseStages` not tolerated, each red.
- **6 — done 2026-10-09.** `list-deps` is a mode (`Main.hs:111-125`:
  parse and import resolution through `Db::parse_stage`, the files sorted
  and canonical) and `--file-list` writes the same list in `compile` as
  soon as the parse succeeds, an unchanged file left alone with its mtime
  (`Main.hs:76-79,135-144`). Every mode's `--help`, in every format, is
  `cmdargs` 0.10.22's rendering ported — byte-identical to the reference
  binary's but for the version; on the way, `--dump-IR` became the
  reference's `--dump-ir`, and a mode word prefix-matches as `lookupName`
  has it. `--template-iterations=N` bounds the loop at `N` rounds (`0`
  newly allowed, `PassBudget::AtMost(0)`); `base-test`, the registration
  that passes it, still passes `repoint`. A source or base library not
  found is the reference's uncaught exception on stderr, not an `Error 1`
  on stdout. Booked: `cmdargs`' argv error wording, `--help=bash|zsh` and
  the version (#34); `--frontend-passes` below 19 and `--template-passes`
  below 9 under a count, refused (#35); `--dump-parse/-types/-program`,
  said on stderr (#36); `pretty` and `--dump-source`, not ported by the
  owner's decision (#37); and, found while diffing whole streams, the
  diagnostics' blank lines and a CRLF excerpt (#38, open). New gate
  `cargo xtask cli` (upstream's `test/compiler/cli` tests as its
  `CTestTestfile.cmake` runs them, and 17 help and usage command lines
  against the reference binary): ours 25/25, the reference's 25/25,
  main's binary before this item 6/25; ~1.5 s, in `fast.sh`'s
  diagnostics tier and `gates.sh`. Bless from main: `cargo xtask cli &&
  cargo xtask ratchet --bless cli`.

## 4. The gaps, partitioned (replaces `gaps.md` §A–G)

| Class | Count | Workstream |
|---|---|---|
| Template elaboration — instances undecided, misnamed, misordered | 77 + ~20 `ptdiff` rows; 29 `backend` declines; most `output` diffs | W1.1–W1.3 |
| Leftover arms (passes 11, 12, 15) | 10 silent | W1.4 |
| Semantic clauses and folds | 18 silent / strict, 22 declines | W2 |
| Recovery positions | 18 disagreements | booked (#30) |
| Source spans (`output` byte-identical 24/274; locations 49%) | — | not this sprint: only if byte-identical output becomes a goal (#25) |
| Engine duplication (CST and IR) | — | W3, W4 |
| Speed and memory | — | W5 |
| The query layer: no cutoff past the parse, no cancellation, the loop from round one per edit | §1's four rows | W7 |
| The editor on the engine to be deleted | every type-reading feature | W3.0, W3.5 early; W3 |
| Gates that pass with an instrument absent; the reference unpinned | 2 gates, the compat binary | W0.4 |
| Identity by text: instances, diagnostics, verdict switches | §1's readers; 5 message tests; 4 switches | W1.1, W8.1, W8.2 |
| Backend messages and locations, graded by nothing | — | W6 measures; §8 decides |
| Accept/reject parity off the corpus: the differential fuzzer's buckets | seed 20261010: 1 false alarm, 18 ours-silent buckets | this sprint, 2026-10-10 (below) |

**Progress — differential fuzz, 2026-10-10.** `cargo xtask fuzz
--differential --seed 20261010` at `dc145e3` found one false alarm and
18 ours-silent buckets among 50. The false alarm is fixed
(`findSymbol`'s declaration order). 14 of the 18 are fixed: 6 with the
reference's own message and position, 8 as a pass's error against the
reference's grammar error (booked to divergence #7, as the `errors` gate
books them); a 15th rejects one column off. So are 14 of the 21
message/position buckets past the 10 recovery-position ones (#30's),
and `c.Foo<uint32>(1)` over a parameter, now exact. The rest are booked:
divergence #39 (parenthesized notes past pass 10 at the text, in
`looped.rs`), #40 (a local named as its type), #41 (the reference's copy
reading a local named as its function template). Gates unmoved
(conformance 2681, `errors` disagree 18, ptdiff 2422, backend 3267,
loop-left 321): the buckets are mutants, not corpus snippets. The
journal's entry "The fuzzer's silent buckets" has the rules.

## 5. Order and milestones

Full gates at each milestone; the fast tier between.

**S1 — reached 2026-09-27** (`gates.sh --snapshot` at `122cf5b`, all
green, 1014 s; ledgers adopted, `status.md` rendered): `loop-left` 320/321
and 321/321 rounds, none near the guard, ratcheted; `lint` ratcheted;
`edit-identity` 348 roots identical, its mutant red; the keystroke table
(W0.5); no gate green with its instrument absent. Beyond the plan:
`backend` 3210 → 3215 and `output` 204 → 206 from the round guard (W0.6).
The first run was red on one stage — the backend cache's self-check found
the C++ backend's own location comments varying run to run on
`test/logic/optimization_test_cases.k`; the verifier now compares such a
file modulo locations (`122cf5b`). Open for the owner: tests of
`conformance::score` (the scoreboard) and four `first_code`/`judge`
behaviours the W6 tests booked (the W6 progress line).

**S2 — reached 2026-09-27** (`gates.sh --snapshot` at `d5dcc6a`, all
green, 938 s; ledgers adopted, `output` and `repoint` blessed, `status.md`
rendered). The gate evidence: `ptdiff` name rows 0 (W1.1); `output`
264/274 identical modulo locations against a target of 230 (206 at S1;
`differ` 57 → 2); `lint`'s reader counts 0 (every rule but
`type-key-literal`, 1, at 0); `cancel` 14–32 ms (W7.2); a body edit
lowers one module (W7.3). Every other ledger since S1: conformance 2571
→ 2580 decidable, 100%; `--strict` failing 34 → 25; `errors` silent 28
→ 19, disagree 26; `ptdiff` structural 2287 → 2384, declined 5 → 1;
`backend` agree 3215 → 3233, declined 46 → 28; `repoint` 51 → 58;
`loop-left` 321/321 on both `left` and rounds; `edit-identity` 348
identical; risc-v's keystroke 904 ms in the bench. Also landed ahead of
S3: W1.2's one ours-only root (`external_class.k`). Premises found false
on the way (each in its item's progress line): W2 row 1 was upstream
verdicts, not `deduceAuto`; W2 row 4 is a reference crash; W8.3's
`TypeTable` is data, not engine, and `kg-ptbridge` still links the
engine through `kg-db`. Open for the owner, beside S1's: the gates'
in-process databases grade the verdict switches' defaults, and W1.4's
gate needs `leftovers` passed in `conformance::settings_for` (the
scoreboard); which is the reference for `basics.k::4,5`, the binary that
crashes or the corpus that says they compile (W2 row 4).

**S3 and S4 — reached 2026-10-09** (`gates.sh --snapshot` at `8f5939b`,
all green, 605 s; ledgers adopted, `repoint` blessed, `status.md`
rendered). S3's evidence: `loop-left` ours-only 0 and 321/321 rounds;
`backend` declined 3 (target ≤ 12: `basics.k::4,5`'s reference crash and
`dependent-template::16`); `ptdiff` structural 2419 (target ≥ 2380); a
library body edit recomputes no importer (W7.4). S4's: `errors` silent 0
(target ≤ 5), `--strict` failing 0, conformance 2602/2602 with the
leftover arms on — no invented error. `repoint` 58 → **71/72** had moved
unblessed since S2 (W1.3's rows and W1.4); the one failing file is
`dependent-template`, whose snippet 16 is the parser's relational
re-reading (W1.2's progress). What stays of parity on the corpus: that
snippet, `ptdiff`'s two rows (`risc-v.k` #394963, `memory/pipelined.k`
#38677), `output`'s one difference (the same `pipelined.k`), and
`errors`' 25 disagreements (18 recovery positions, divergence #30).

**S6's speed targets — met 2026-10-10, but for the keystroke**
(`gates.sh --snapshot` at `ea28b1d`, all green, 1066 s; ledgers
adopted, `status.md` rendered). W5.2–W5.5 (the performance journal entry
"The loop's floor, kept rather than recomputed"): `kg check` on
`array.k` 3.10 s → 0.95 s and 1.41 → 0.24 GB, `risc-v.k` 6.22 s →
2.65 s and 1.74 → 0.68 GB — under the targets and under the reference's
memory (0.55 / 1.1 GB); every step exact (`infer-identity` and its
cross-build dump 304/304). The bench's keystroke on `risc-v.k` 954 →
604 ms (target ≤ 400 ms: W7.5d's, the loop carried across edits, awaits
the owner's review). The same run: `repoint` 72/72, `ptdiff` 2422/2422
structural, `output` 266/274 with none differing, `cli` 25/25,
`errors` 18 (divergence #30's recovery positions), `resilience` with the
editor reading the loop (W3, 2026-10-10). — **The keystroke too, later
the same day**, without W7.5d: 598.6 / 623.5 → 310.3 / 323.1 ms on
`risc-v.k` (interleaved, quiet), every step exact (`performance.md`
§10, journal "The keystroke, by phase"); ≤ 100 ms (M6) stays W7.5d's.
Not yet graded by a milestone run.

**Milestone run at `aafe622` — 2026-10-10** (`gates.sh --snapshot`, all
green, **315 s** for the whole suite against 1066 s the morning's run):
after the keystroke round (risc-v's bench keystroke 604 → 306 ms, under
S6's 400 ms on the bench's program; the editor's real 50-module program
is ~0.9 s, W7.5d's), the editor on the loop (hover, inlay hints,
signature help, completion, member lists, method navigation), W3.6's
exit, W8.4/W8.5, and the editor's budget no longer inventing errors.
Every ratchet as blessed; `status.md` rendered.

**Milestone run at `4db41f1` — 2026-10-10** (`gates.sh --snapshot`, all
green, 450 s under load 36 from two agents building). It covers:

- **S5's one engine:** W3.7 deleted the CST inference engine and the
  second census (21k lines).
- **W7.6:** the editor answers at four rounds, then at the fixpoint.
- **W7.5d D1 step 1:** the kept loop and its `shadow` ratchet — kept
  1808, unequal 94, the reads step 2 keys.
- **`kg-alloc`:** the language server purges eagerly.
- **`cargo xtask mem`,** the owner's condition on D1's memory: on
  `risc-v.k`, `check` 769 MiB, editor open 883, after five edits 1480
  (peak footprint), every budget held.

Every other ledger was unchanged:

- conformance 2681/2681;
- `errors` 18;
- `ptdiff` 2422/2422;
- `backend` 3267;
- `output` 266/274;
- `repoint` 72/72;
- `cli` 25/25;
- `loop-left` 321/321;
- `edit-identity` 348 identical.

Tests 1862 (the CST engine's went with it). `status.md` rendered. S5's
keystroke ≤ 400 ms holds on the bench's program (393 ms worst, loaded);
what remains of S5 is W7.5d steps 2–4 and W4.

**Milestone run at `1ebc57a` — 2026-10-11** (`gates.sh --snapshot`, all
green, 361 s on a quiet machine; ledgers adopted, `status.md` rendered).
It covers:

- **W4's exit:** every "copies today" cell in
  `single-source-of-truth.md` empty or a register pair (11 commits,
  each exact against its parent).
- **W7.5d D1 steps 2 and 3:** the read door (`shadow` unequal 94 → 0,
  kept 1808 → 1746, the 62 kept only because their keys missed the
  reads) and the prefix pass reused (700250 of 700348 offered, each
  checked against the pass run, none unequal). Step 3 wins no keystroke
  time: the prefix is the edited root's (`loop-across-edits.md` §6).
  Step 4 is planned there, not built.
- **References and rename across the project** (`kg-lsp`): each file
  searched under the program it roots; `resilience` 2676 → 2684.
- **22 fuzz-found parity fixes** (difffuzz seed 20261010: the false alarm,
  14 of 18 ours-silent buckets, `param.k`'s member template), and the
  one regression they brought, found by a full `edit-identity` and fixed
  (`1ecf658`).

Every ledger as blessed: conformance 2681/2681; `errors` 18; `ptdiff`
2422/2422; `backend` 3267; `output` 266/274; `repoint` 72/72; `cli`
25/25; `loop-left` 321/321; `edit-identity` 348/348; tests 1885. The
bench's keystroke 378.70 ms worst (`risc-v.k`), its fixpoint 776.52 ms
(`array.k`); `mem` every budget held. What remains of S5 is D1 step 4
and the reuse of the passes that read across modules — a design of its
own (re-askable reads), not a step. Where the session paused, and the
options from here: [`handoff-2026-10-11.md`](handoff-2026-10-11.md).

| Milestone | Contents | Gate evidence |
|---|---|---|
| **S1 baselines** | W0 (W0.3 done 2026-09-27), W0.4–W0.7; W7.1's instruments; W7.5a–c; W6's tests of the verdict functions | `left` and round-guard rows ratcheted; no gate green with its instrument absent; `edit-identity` running, its mutant red; the keystroke table; `status.md` current |
| **S2 names** | W1.1 with D1.2/D2.5; W2's first half alongside; W8.1–W8.3; W7.2, W7.3; W3.0 and W3.5 (the gate and the index, empty) | `ptdiff` name rows 0; `output` ≥ 230; `xtask lint`'s reader count 0; `cancel` ≤ 50 ms; a body edit lowers one module |
| **S3 decisions** | W1.2, W1.3 with D2.1/D2.2/D2.4; W2's second half; W7.4 | `left` 0; `backend` declined ≤ 12; `ptdiff` ≥ 2380; a library body edit recomputes no importer |
| **S4 the arms** | W1.4 | silent ≤ 5; no invented error |
| **S5 one engine** | W3 with the rest of W4; W7.5d and W7.6 if W7.5a–c (run at S1–S2) found a path | resilience gate; CST engine deleted; keystroke ≤ 400 ms, or the measured reason it is not and the plan that follows from it |
| **S6 speed** | W5, beside S3–S5 where independent (compaction and `desugar` any time); W8.4–W8.7 where not yet done | the §1 targets |

What waits on what:

- W2, W8 and W5.2–W5.5 need nothing from W1 and fill its gaps. W7.1–W7.4
  and W7.7 need nothing from W1 either, and W3.0 and W3.5 nothing from
  anything: they are the second track, for a worktree of their own.
- W7.5a–c need nothing and are measurements; W7.5d waits on them and on
  W1.1's registry; W7.6 on W7.2 and on W7.5's outcome.
- W3 waits on S4 only for passes 11/12, whose IR versions it inherits; its
  deletion (W3.7) waits on W8.3 and on the resilience gate.
- W5.2's compaction remaps every table keyed by `ExpId`, W7.5 keeps those
  tables across edits: whichever lands second is graded by both
  `infer-identity` and `edit-identity`.

## 6. Risks

| Risk | Answer |
|---|---|
| Switching the leftover arms invents errors on correct programs | W1.4's gate: no `expected:0` and no reference-compiled root gains an error; `KG_LEFTOVERS` stays the switch until it holds |
| Instance display is spelled by rules nobody has written down | `kg-oracle --passes 8` shows every mangled and pretty spelling (`oracle/README.md`); port `Mangle.hs`/`PrettyPrint.hs` clause by clause, graded by `ptdiff` |
| Compaction breaks the incremental walk's identity | compaction runs between rounds, where the units are carried; `infer-identity` and `KG_INFER_CHECK` grade it before it lands |
| The resilience gate finds a corner the IR engine cannot match | §2.6's balance point: a narrow editor path taken on that data, not assumed |
| `left` turns out to count legitimate leftovers | W0 classifies before anything is ratcheted |
| A memo's key misses an input, and the editor shows a stale answer | `edit-identity` against a from-scratch `Db`, every corpus root, every kind of edit; each key lands with the mutant that leaves an input out |
| A cancelled run leaves half a result in a memo | a cancelled computation writes nothing; the test compares every memo before and after |
| The interface firewall hides a change an importer depends on (a constant's value, a default argument, an inline function's body) | the interface is defined by what the reference's importers read (`resolve.md` §2.4), widened wherever `edit-identity` finds a reader; too wide costs speed, too narrow is red |
| The sprint is too large to finish | §7's order of yielding; the milestones' gate evidence is per milestone, so a sprint that ends at S3 has ended somewhere graded |
| W7 and W1 edit the same files (`db.rs`, `elaborate.rs`) from two worktrees | W7.2's token is threaded first, as one small commit on `main`; after it the two tracks touch different functions |
| The IR engine cannot serve broken buffers | W3.0 says so at S2, three milestones before the CST engine's deletion depends on it |

## 7. If the sprint runs long

What yields, first to last. Nothing above the line is traded for anything
below it.

1. W2's tail: the one-offs, the two `agilex-7` disagreements, divergence
   #32's six.
2. W8.4–W8.7.
3. W5.6 (`TyId`, parallel rounds) and W7.5d–W7.6: the design of the loop
   across edits waits for a sprint of its own, and the round cap with it.
4. W5.5 (the snapshots, 6.5%).
5. W3.6–W3.7: the CST engine outlives the sprint, fenced by the
   resilience gate and the register.

---

Not to be cut: W0; W1.1's structural exit; W7.1–W7.4 and W7.5a–c, the
measurements; W5.3–W5.4, the round's floor, which the first reading of
W7.5 says is where a keystroke's time is; W8.1–W8.3; the instruments of
W6. They are what every later sprint builds on, and each
is cheaper now than after more code has been written against what it
replaces.

## 8. For the owner to decide

| Decision | The plan assumes | What the other answer costs |
|---|---|---|
| **Source locations** | "identical modulo locations" stays the parity bar (§4); W6 measures the backend's messages so the cost is a number — **measured 2026-10-10: the first backend error line is identical in 204 of the 256 programs both reject with one backend code; the other 52 differ in their line number alone** | locations reach the backend's error lines (`errors.h:110`), its debug symbols and, under `--code-coverage`, the names it generates (`ir.cpp:2210-2218`) — not only comments. Byte-identical means reconciling divergence #25's spans: a workstream of its own |
| **The query layer's substrate** | hand-rolled, with W7's firewalls. **Re-examined 2026-10-11** (`loop-across-edits.md` §8): the recommendation stays hand-rolled — the loop's cost is a whole-program fixpoint over mutable arenas whose rounds are observable, which salsa's pure, equality-compared queries do not fit; re-askable reads borrow salsa's revalidation instead, and the read-keyed memo pattern is to become one internal primitive | salsa gives tracked dependencies instead of hand-listed keys, durability, eviction and a vocabulary contributors know, at the price of re-keying every query and, inside the loop, D1 rebuilt in its terms; it would not solve positions (relocation) or reads outside a query. Worth asking again if `edit-identity` keeps finding missed inputs, if keeping the keys becomes the bottleneck, or if the round semantics stop binding after parity (row below) |
| **The editor during the migration** — *settled 2026-10-10, no answer needed* | the CST engine serves it until the resilience gate is at parity | an earlier switch shows the language's best feature sooner — widths inside copies (`roadmap.md`) — on an engine not yet proven on broken buffers. **Done:** every type feature reads the loop (W3.4), on a broken buffer the last loop mapped; the CST engine answers only where the loop has none |
| **The editor's rounds before W7.5 is settled** — *answered 2026-10-10: the third answer, layered; **done 2026-10-10** (W7 item 6, W7.6): first answer at four rounds, the same run's fixpoint after, `kg check`'s on every corpus root (`edit-identity`)* | the four-round cap stays until the fixpoint is affordable | with cancellation (W7.2) the editor could run to the fixpoint now and show the true diagnostics late — `risc-v.k`'s loop is about 5 s — instead of partial ones at about 1.5 s. **Re-measured 2026-10-10** (`cargo xtask bench --filter phases`, recorded settings, load 13–21, so upper bounds): a body edit to `risc-v.k` costs 0.88 s at four rounds and 2.0–2.3 s at the fixpoint, `array.k` 0.47 s and 0.85 s; the keystroke bench's four rounds are 306–330 ms. The four rounds are the fixpoint's own first four, so a third answer costs no more than the fixpoint: publish at round four and let the same run continue, cancellable, to the true diagnostics (W7.6's layering, without W7.5) |

**Answered 2026-10-10 by the owner:**

- *The editor's rounds:* the third answer — publish at round four, let
  the same run continue, cancellable, to the fixpoint, and republish
  (W7.6's layering). `PassBudget::INTERACTIVE` stops being a cap on what
  the editor finally shows.
- *W7.5d:* run 5d-0, then follow `loop-across-edits.md` §5's rule — build
  D1 in §6's order if it would keep most of the library's work on most
  edits, drop it with the number recorded if not. D2 is dropped.
- *`FRONTEND_COMPLETE`:* flip it, on `--strict` failing nothing.
- *The toolchain:* pin `rust-toolchain.toml` to `1.92.0`.

Still open: the gate's `--Wall` reading, the source-locations bar,
`basics.k::4,5`, mimalloc (kept meanwhile) and the query layer's
substrate (hand-rolled meanwhile).

**Added 2026-10-10** — decisions the parity and performance work surfaced:

| Decision | The work assumes | What the other answer costs |
|---|---|---|
| **`FRONTEND_COMPLETE`** (`xtask/src/conformance.rs`, the scoreboard) — *answered 2026-10-10: flip it; **done** (`5e34bf4`): 2681/2681, 241 undecidable* | left `false`: the scoreboard is the owner's | its own comment gates flipping it on `--strict` failing nothing, which holds since 2026-09-28 (0 of 2681); flipped, `conformance` grades what `--strict` grades today and the 320 "undecidable" become 241 backend codes the gate cannot see |
| **The gate's `--Wall` reading** (`conformance::settings_for`) | booked in `xtask/src/options_agree.rs` | the gate turns `redundant-import` back on after `--Wno-redundant-import … --Wall`, which the reference does not (probed); no assertion moves today; fixing it edits the scoreboard |
| **W7.5d, the loop across edits** (`loop-across-edits.md`) — *answered 2026-10-10: run 5d-0, then follow its decision rule* | 5d-0 measured 2026-10-10: D1 keeps 95% of the library's work across an edit once positions are relocated (62% as written), so it is to be built (`loop-across-edits.md` §5, §6). Step 1 built the same day: the editor keeps each run's snapshot and the next run's shadow checks it (keys agree on 92% of module-rounds, every one restored equal but 926 the reads across the boundary part, which step 2 keys). Step 2 the same day: what each module's round read across its boundary is keyed beside its copies (the read door), and no round the shadow follows is unequal (keys agree on 91.9%). Step 3's first pass, 2026-10-11: the prefix reused, exact and checked over every root, no keystroke won — the prefix is the edited root's; the next passes need re-askable reads, and step 4 is planned, not built (`loop-across-edits.md` §6). The memory it holds — the run before's program until the next reaches its fixpoint, and 5.8 MB beside it on `risc-v.k` — accepted by the owner the same day, on the condition of efficiency, which `cargo xtask mem` (added on main) keeps visible (§7 there) | the editor's keystroke on its real program (`risc-v.k` under the project's settings, 50 modules) is ~0.9 s, ~45% of it re-inferring the unchanged library every keystroke — only W7.5d removes that; the bench's program is at 306 ms, under the 400 ms target |
| **The editor's hover on a broken buffer** — *settled 2026-10-10 by W3.4, no answer needed* | answered by the CST engine, no loop run (W3.4 is measuring the alternatives) | running the loop there takes hover on `risc-v.k` from ~16 ms to ~900 ms after each edit. **Done:** the last loop, mapped through the edit (~16 ms); a buffer opened broken runs its own loop once, for a hover |
| **mimalloc** (the binaries' allocator, 2026-10-10) | kept: under macOS's allocator a warm editor database grew ~90 MB a keystroke (1.87 GB after 20); mimalloc holds 0.54 GB after 30, 18% fewer instructions | building now needs a C compiler; reverting is one commit (`120ef09`'s cherry-pick) |
| **`basics.k::4,5`** (W2 row 4) | the walk declines them, as the reference binary crashes on them | the corpus says they compile (`expected:0`); which is the reference — the binary or the corpus — is the owner's |

**Added 2026-10-11** — from the discussion of the substrate and the
keystroke (`loop-across-edits.md` §8):

| Decision | The work assumes | What the other answer costs |
|---|---|---|
| **The keystroke's target** | `ir-elaboration.md` §3's ≤ 400 ms / ≤ 100 ms on the bench's program, which meets 400 ms (379 ms) | proposed: the first answer ≤ 400 ms and the last ≤ 1 s on `risc-v.k` under its project's settings (today ~0.83 s and ~2.0 s). Carrying the library across edits takes at most ~40% off the loop, so that target needs step 4, parallel rounds or reuse inside the edited module as well; 100 ms needs the last |
| **The reference's round semantics after parity** | they bind: copies made in the reference's rounds and order, which `ptdiff` and `output` see | an output whose copies may come in another order opens a demand-driven elaboration — copies per `(template, arguments)` as queries, possibly on salsa — with far better keystroke prospects; `ptdiff` and `output` would need a new definition of agreeing |
| **Salsa's familiarity** | weighed below the migration's risk | if contributor familiarity weighs more, a migration of the outer queries first, the loop last or never |
