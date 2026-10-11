# Template elaboration without the text round trip

Status: **M0–M4 done, M5 mostly done, M6 done 2026-10-10** (sprint W3.7: the CST inference engine, the CST `ConstEval` and the second census are deleted; the loop is the one engine, the editor's included; `cargo xtask sem`, the shadow gate below, went with them); the rest continues in [`sprint-parity.md`](sprint-parity.md) (2026-09-27): the twin is deleted, the program-wide loop is the only elaboration; M5's `InstanceRegistry`, `TyId` interning and parallel rounds are sprint W1.1/W5.6. Kept as the design record. Earlier: (2026-09-25) M0's evaluator and key helpers landed with D1.1/D1.3/D1.4; M1 is complete: the IR engine (`kg-sem`) agrees with the reference on every node the reference can adjudicate (`cargo xtask sem --oracle`, a gate) — see §4's progress notes.

This plan grows `docs/history/audit-2026-09-22.md` §6.4 ("one IR, one fixpoint") into a design. It
does not replace it. Its companion is
[`single-source-of-truth.md`](single-source-of-truth.md), whose early steps
are this plan's foundations.

## 1. What we have, and why it has to go

Pass 8 (`instantiateTemplates`, `Template.hs:50-62`) is a fixpoint:

1. decide instances;
2. copy templates;
3. infer over the result;
4. repeat.

Materialisation is ported onto the IR (`kg-hir`). Everything that *reads*
the result is not: inference (`kg-cst-infer`, until sprint W8.3 `kg-ty/src/infer.rs`), constant folding
(`kg-const/src/eval.rs`) and all 19 numbered passes (`kg-db/src/passes/`)
walk the rowan CST.

So since 2026-08-14 (journal, "Print and reparse works…") every elaborated
module is rebuilt as a *twin*, one round at a time:

1. rename every unlexable name to `__kg_mN`;
2. print the IR to text;
3. reparse it with a twin grammar mode;
4. lower it again, twice or three times;
5. infer it with CST inference;
6. pair it back to the IR by a lockstep walk that refuses shapes it cannot
   match;
7. read the types back;
8. run every pass over the twin;
9. un-mangle diagnostics by text search and carry them home.

Measured costs (surveyed 2026-09-25; references in the journal and
`docs/history/audit-2026-09-22.md`):

- **Code.** About 4.8k non-test lines exist only for the round trip:
  - `rename.rs` 790;
  - `pretty.rs` 1.2k;
  - `readback.rs` 200;
  - `twin_diagnostics.rs` 350;
  - about 1.4k in `elaborate.rs`;
  - twin routes in `passes/types.rs` (about 300);
  - about 100 twin special cases across the passes;
  - the twin parser mode.
- **Correctness.** It is the source of a whole family of fragilities:
  - names the printer cannot spell: capture types, `$x0`, an unrolled
    `static for`'s qualified `i`, local types from another module;
  - `mint_*` declines ("cannot mint");
  - pairing refusals, which silence a whole subtree;
  - start-aligned-only diagnostic carrying;
  - `__kg_m` / `<|` string tests in production code;
  - a module that loses its twin to a parse error loses *everything*
    elaborated in it.

  The largest open `ptdiff` families are consequences of this. gaps.md
  lists them as A1 and A2: instances inside copies, and closure typing.
- **Performance.**
  - The worst keystroke is **1.46 s** (`fifo/transpose.k`, 2026-09-25),
    against a 16 ms frame.
  - Every pass re-prints, re-parses, re-lowers and re-infers every target
    module, even one whose inputs did not change.
  - `data/array.k` mints 1,039 aliases and peaks at **1.8 GB**. Its frontend
    CPU is 52–84 s, against the reference's 53 s for the whole compile.

The twin was the right call when it was made: it reused the one graded
inference engine. What the journal feared from the alternative — "two
engines that must agree forever" — has arrived anyway, spread across the
passes as twin-mode special cases.

## 2. Target architecture

**The sorted IR is the semantic tree.**

- It is total: 405/405 clean parses lower with zero `Unsupported`.
- It round-trips: 405/405.
- It matches the reference's own tree node for node on 122/124 library roots.

The reference's rules (`Type.hs`, `TypeCheck.hs`, `Constexpr.hs`) are written
over `ExpF`, which the IR mirrors constructor for constructor. Porting them
onto the IR is transcription. Porting them onto the CST was translation,
and the CST workarounds are the evidence:

- the `folded_*` emulation in `SymbolEnv`;
- `ParseOrder`/`Registered`;
- `reached_only_after_elaboration` (45 call sites);
- the dead/live `static if` lists.

```
              CST (kg-syntax) ── formatter, syntax-only IDE features
                  │ lower_sorted (per module, memoized)
                  ▼
 ┌────────────── Semantic program (kg-sem, new) ───────────────┐
 │  ModuleIr arenas (kg-hir)      TypeMap per arena (ExpId→TyId)│
 │  InstanceRegistry (program)    StaticDecisions per module    │
 │  one inference engine · one constexpr evaluator · passes     │
 └──────────────┬──────────────────────────────────┬────────────┘
                │ diagnostics at Src.head + `via` frames
                ▼                                  ▼
          kg-db queries / LSP                kg-ptbridge walk
```

### 2.1 Nodes and types

- **`TypeMap`**: the type of a node is keyed by the node. It is a dense
  `Vec<Option<TyId>>` indexed by `ExpId.index`, one per arena fork, with
  `TypeTable`'s monotone `resolve` / `set_type_of` / undo-journal semantics
  moved over unchanged. That logic is already substrate-free
  (`infer.rs:357-630`).
  - Donated nodes get types like any other node. Today they can only borrow
    one through a partner, which is the walk's `type_of` workaround.
- **`TyId`**: types are hash-consed in a program-wide interner. `Ty` is
  deep and cloned constantly today (`TyRef` is an `Arc`). With `TyId`,
  equality is an integer compare, a `TypeMap` entry is 4 bytes, and
  substitution and `resolve` stop allocating.
  - `kg-ty`'s `ty`/`width`/`subtype`/`convert` keep their `Ty` API behind a
    `Ty ↔ TyId` pair and migrate call site by call site.
- **The `TypeKey` projection stays**, as a read-only *view* for CST
  consumers (hover, inlay hints, completion, signature help):
  - an `Own` node's `(Src.head, kind, via)` maps to its `ExpId` through an
    index built once per lowering;
  - the IDE reads types without knowing about arenas;
  - the view is also how the migration's shadow gates compare the two
    engines, key for key.

### 2.2 One inference engine, on the IR

`infer_ir` ports `Algebra::infer` and `Round::visit` onto
`match arena[id].exp`:

- **Reused unchanged:** the rule arithmetic, `width::*`, `subtype::*` and
  `convert::*`, all substrate-free.
- **Deleted:** the CST navigation, which is about 55–60% of `infer.rs`'s code
  (kind dispatch, accessors, token-text parsing, scope derivation).
- **`SymbolEnv` becomes an IR symbol map.** kg-hir's `SymTab` / `SymbolMap` /
  `harvest_shapes` already restate `getSymbols` over the IR. Scopes come from
  `ScopeStack` via `Src`/`decl_scope`, never re-derived (the `$quasiN` gap,
  `infer.rs:846`).
- **The inference loop is the reference's, including its rewrites.** Each
  step is `infer = staticControl . evaluateConstexpr . inferExpType`
  (`Inference.hs:47-50`), and it *rewrites the tree*: a fold replaces the
  node with a literal under the same note, and a decided `static if` is
  replaced by its branch.
  - The IR's append-only, forkable arena can do exactly that: a replaced
    node is a fresh id whose `Src` is its donor's.
  - This deletes the CST emulation of both rewrites:
    - the `folded_*` `SymbolEnv` methods;
    - `facts::reached_*`;
    - live/dead lists;
    - the census's own `static if` region;
    - most of `kg-ptbridge`'s re-derivations — five of today's eight fixes
      were the walk re-deriving a rewrite the reference performs.
- **`TypeCheck`'s own rewrites become IR rewrites too.**
  - `checkConversion` turns an enum value meeting an integer into its
    literal, and wraps a narrowing in `Warning`.
  - `checkVarDecl` turns a function constant into `SeqF []`.

  The walk then emits what is there instead of reconstructing it.

### 2.3 One constexpr evaluator

`kg_const::ConstEval`'s semantics move behind a node view (`ConstNode`
trait) with two implementations:

- **CST**, for the IDE and for the migration;
- **IR**, the compiler.

All arithmetic goes through `fold_binary_values`. The four IR mini-folders
are deleted: `exp_value`, `Dependent::value`, `kg-hir width::value` (its own
`i128` arithmetic) and `walk.rs literal_arithmetic`. This is
[`single-source-of-truth.md`](single-source-of-truth.md) step D1.3; it is
needed first because §2.2 folds by rewriting through it.

### 2.4 One program-wide instantiation loop

This mirrors `instantiateTemplates` (`Template.hs:50-62`) and replaces:

- per-module twins;
- `elaborate_twin_round`'s rounds 2..8;
- the phase-2 program passes;
- `ALIAS_GUARD`.

```
state: per-module arenas + TypeMaps, InstanceRegistry, StaticDecisions
loop (until no new instance, or the copy budget):
  1. census (per module, parallel)   decided instances from current tree+types
  2. registry.intern(instances)      content-addressed InstanceId → home module,
                                     mangled name, copy root (once per program)
  3. instantiate (per home module)   copy template bodies into the home arena,
                                     Src.via = the instance frame
  4. infer (per dirty module)        infer_ir fixpoint incl. constexpr/static
                                     rewrites, over new nodes only where possible
  5. deduce / wrappers / closures    the type-reading rewrites, on IR types
then: passes (IR), diagnostics at Src.head with `via` frames
```

- **`InstanceRegistry`** replaces `ForeignSpec`, `TwinDecision`, `RenamePool`,
  `homes` and the alias maps.
  - One entry per instance, program-wide: args as structured `TArg`s, the
    mangled name, the home module, the copy's root id, the deciding round.
  - A foreign instance is a registry entry its home module picks up, not a
    minted-and-printed spec.
  - Display names come from the entry's args. They are never parsed back out
    of mangled text, which cannot be done for function types — `$fun`
    concatenates parameters and return with no count, e.g. `callbacks.k::38`,
    fixed today by a special case.
- **Nothing is renamed.** Names are the mangled originals throughout.
  `rename.rs`, `__kg_mN`, `unrename_*` and `is_alias_spelling` go.
- **Diagnostics inside copies** are reported at `Src.head`. The `while
  compiling` chain comes from `Src.via` (`copy_frames`), which is already
  exact. Carrying home is no longer needed: there is no twin, so nothing was
  away from home.
- **Round dating.** `ExpansionData.round` and `InstanceId` stay as they are.
  They are already the reference's rounds.

### 2.5 Passes on the IR

Each numbered pass is ported as an IR pass over `(arena, TypeMap)` in
execution order, one per commit.

- **Priority:** the passes that must see *inside copies* come first —
  pass 9 `undefined_symbols`, 8 `template_type_check`, 10 `type_errors`,
  12 `unresolved_templates`, 19 `validate_program`/`conversion`/`shadow`,
  18 `interpolated_string_post`, 17 `deduce_auto`. They are why the twin
  still has to be a CST.
- **Positions:**
  - An `Own` node's `Src.head` *is* today's CST range, so most diagnostics
    land identically.
  - Sub-node positions (a declaration's name) need that sub-node as an `Own`
    child, which the IR has.
  - Token-only positions (a call's closing `)`) need one addition:
    `NodeData.anchor: Option<TextRange>`, filled by the lowering for the few
    constructs whose diagnostics sit on a token.

  `xtask errors` grades all of it.

### 2.6 The editor: one engine, resilient by design (decided 2026-09-25)

The language server needs four things from the frontend:

- **latency** — incremental, cancellable, never-blocking;
- **resilience** — half-typed buffers still get types, hover and completion;
- **position mapping** — cursor → node → type or definition;
- **graceful staleness** — the last good answer while a new one computes.

These are properties of *one* engine, not a reason for two. A dedicated
IDE engine is the drift this plan exists to remove. So:

- **Total, structure-preserving lowering.** Error recovery lowers to
  `Exp::Error`/`Missing` nodes that keep every child they can: the receiver
  of `a.`, the callee and present arguments of `f(x, )`. Today 13 of 76
  error-recovered files still lower with `Unsupported`; the target is none.
- **Policy, not a second engine.** The engine takes a mode:
  - `Strict` reproduces the reference's stop-at-errors exactly ("a program
    carrying an error instantiates nothing", `Template.hs:55,84`) for the
    compiler and every gate;
  - `Resilient`, for the editor, continues past errors, typing error nodes
    `Unresolved` without poisoning their siblings.

  The rules are single; only what counts as a stopping point differs.
- **Layered latency.**
  - Hover or completion inside a plain function needs only that module's
    inference plus memoized import interfaces.
  - Types that come from template copies arrive when elaboration finishes:
    cancellable, and served last-good with a background refresh.
- **One position index.** A CST offset maps to the innermost `Own` IR node
  with that range, then to the nearest ancestor. Type-reading IDE features —
  hover, signature help, inlay hints, completion — go through it. The
  formatter and syntax-only features (tokens, folding, selection ranges) stay
  on the CST, which is theirs.

**The balance point.** The current CST engine stays until a measured
**resilience gate** says the IR engine is at least as good for the editor.
The gate covers the 76 error-recovered files plus fuzzed broken buffers,
comparing hover and completion answers per node. If some corner cannot reach
parity, that is the evidence for a narrow editor-specific path, taken on data
rather than assumed.

## 3. Performance design

The user lifted the "correctness only" rule for templates and types at this
stage (2026-09-25). The targets, measured by the existing `bench` keystroke
stage and by new per-phase timers:

| Measure | Today | After step M4 | After step M6 |
|---|---|---|---|
| worst keystroke (`fifo/transpose.k`) | 1.46 s | ≤ 400 ms | ≤ 100 ms |
| `data/array.k` peak memory | 1.8 GB | ≤ 600 MB | ≤ 400 MB |
| `data/array.k` frontend CPU | 52–84 s | ≤ 20 s | ≤ 10 s |
| batch `check` worst file | ~95 ms | no regression | no regression |

Where each gain comes from:

1. **No print / parse / re-lower / re-infer-from-scratch per round.** A copy
   is typed once, as it is made. The reference's order is the same.
2. **Dependency tracking with early cutoff.**
   - Each module's round is a function of its input fingerprints: its own IR,
     the registry entries it is home to, and the exported interfaces of its
     imports.
   - A module whose fingerprints are unchanged is skipped. That is the
     journal's "per-import dependency tracking", blocked today because any
     twin may read any earlier twin.
   - With the registry as the only cross-module channel, the dependencies are
     explicit.
3. **Parallel rounds.** Within a round, modules are independent given the
   registry snapshot. The census, inference and passes fan out per module;
   the gates already use per-thread `Db`s.
4. **Types by index, interned.** The `TypeMap` is a dense `Vec`, `TyId`
   equality is an integer compare, and no 5-field key is hashed per lookup.
   The global interner mutex (`kg-base/src/interner.rs:41`, 180 call sites in
   kg-db) is replaced by a sharded or read-mostly interner.
5. **Copies are content-addressed.** Identical instances are copied and typed
   once program-wide. Across keystrokes, the per-module round memo reuses
   every copy whose inputs are unchanged. Body-only edits stop invalidating
   importers once `ItemTree`/`ModuleTypes` equality is structural rather than
   pointer identity (AUDIT §4.2).
6. **A copy budget** replaces `ALIAS_GUARD` and the `INTERACTIVE` pass cap. The
   language server bounds work by cancellation instead of by stopping short.

## 4. Migration — every step gate-green

Rules for every step:

- **Pure refactors are byte-identical** on every gate (`ptdiff`, `backend`,
  `output`, `errors`, `strict`, `repoint`, `ir`).
- **Behaviour changes are separate commits** with ratchet evidence.
- **Shadow gates** — new stages in `xtask ir` — compare the new path with the
  old one per node, and must reach zero before the switch that depends on
  them.

| Step | Work | Exit criterion |
|---|---|---|
| **M0 Foundations** | `single-source-of-truth.md` D1 (names/mangling, one IR folder, `TypeKey::of_src`, written-type → `Ty`), the `TypeMap`/`TyId` types, the `anchor` field | gates byte-identical |
| **M1 IR inference, base modules, shadow** | `infer_ir` over each module's sorted IR, *without* elaboration; new gate `ir-infer`: per `Own` node, IR type vs the CST table | `ir-infer` agreement 100% over the corpus, or each difference booked as a CST-side bug with a citation |
| **M2 IR types for copies, shadow** | inside today's `elaborate_module`, type the rewritten `ours` tree with `infer_ir` alongside the twin; new gate `copy-types`: IR type vs twin read-back per paired node | `copy-types` 100% on paired nodes; unpaired (refused/donated) nodes counted, not failed |
| **M3 Switch types** | elaboration, the census, deduction and the walk read `infer_ir`'s `TypeMap`; the twin runs *only* to feed the CST passes' diagnostics | all gates ≥ baseline; expected gains where the twin refused pairs |
| **M4 Passes on the IR** | port the §2.5 passes, one per commit, each with a per-pass differential against its CST version on base modules; when all inside-copy passes are ported, **delete the twin** (`rename.rs`, `readback.rs`, the twin parser mode, `pretty.rs` from production, `twin_diagnostics`' un-mangling, `Imports`' twin routes, `Whole`'s twin fields) | all gates ≥ baseline; errors gate positions unchanged |
| **M5 One loop** | §2.4's program-wide loop, `InstanceRegistry`, dependency tracking, parallel rounds; delete phase 2 and `ForeignSpec`/`TwinDecision` | gates ≥ baseline; keystroke ≤ 400 ms; `array.k` ≤ 600 MB |
| **M6 One engine** | the remaining base passes on the IR; resilient lowering and `Resilient` mode; the resilience gate (§2.6); then CST inference and the CST `ConstEval` implementation deleted, IDE features reading the position index | resilience gate at parity; keystroke ≤ 100 ms; `infer.rs`'s CST engine gone |

**Why this order.**

- M1 proves the engine on the 405 clean programs *before* it has to type
  copies. Its disagreements are cheap to read, and some will be CST-side bugs
  the shadow finds for free.
- M2/M3 move the most fragile machinery (read-back, pairing) off the critical
  path while the twin still guarantees diagnostics.
- M4 deletes the twin only once nothing needs it.
- M5 is the performance step, and it is only safe once nothing crosses a
  module boundary except the registry.

**Sizing, rough.**

| Step | Size |
|---|---|
| M1 | about 3k lines: the algebra, visit policy, IR symbol env |
| M2–M3 | about 1k lines |
| M4 | the passes are about 20k CST lines today; the IR versions should be markedly smaller, since the CST guards go |
| M5 | about 2k lines, net negative after deletions |

The twin deletion alone removes about 4.8k.

### Progress

- **2026-09-25, M1 foundation.** `kg-sem`: `TypeMap` (node-keyed, `TypeTable`'s
  write rules), `Symbols` (`getSymbols' TemplateDecl`, program-wide), and the
  fixpoint — `inferExpType`, the `evaluateConstexpr` rewrites (in place,
  under the node's own note), `staticControl` through `kg-hir`'s own
  `static_control_node`. `cargo xtask sem` compares every reachable `Own`
  node with the CST table: **2,168,843 / 2,260,852 (95.9%)** over 304 roots
  at first run. The largest groups read so far are CST-side deviations
  from the reference, not engine bugs: the CST types a template parameter's
  *declaration* with the rule for its *mention*; it never rewrites, so a
  comparison of two constants stays `TUnresolved` where the reference folds
  it and types the literal `bool`; its symbol map is per module, where the
  reference's is program-wide (`test/runner.k`'s `test_main`); and it sees a
  higher-order function pass 2 has already made a template as a function.
  `TyId` interning is deferred to M5, where it is measured.
- **2026-09-25, M1 complete.** The shadow gate grew an arbiter:
  `cargo xtask sem --oracle` puts every root-module disagreement to the
  reference itself (`kg-oracle --typed --passes 6`, the node of the same
  constructor and range, or start) and counts whose answer it is. Over the
  corpus: **engine 1258, CST 0, neither 0**, and 106 unmatched (a node
  whose span or shape the reference does not share, divergence #25). The
  gate fails on any `cst` or `neither` verdict and runs in `scripts/
  gates.sh`. Engine fixes it drove: an unsorted `Path` is `TUnresolved`,
  never `TUntyped`; `getName` of a qualified name is its leaf (method
  calls); a bare use of an imported function template holds the
  `TemplateInstance` answer the sorter's missing wrap would have given.
- **2026-09-26, M2.** `cargo xtask sem --copies` runs the engine over every
  module's *final* elaborated tree and compares each node with what the
  backend walk reads today (the twin partner's type, else the elaborated
  table). What made it work is the reference's own inputs to a copy, handed
  to the engine rather than re-derived: `makeInstance`'s
  `setTypeOfInstance` notes (`RewrittenModule::instance_notes`, every
  round's), the types of the leaves the rounds minted for names a twin
  cannot spell (`RewrittenModule::leaves`), and the tree **un-renamed** —
  the rename layer renames identifiers and never scopes, so a renamed tree
  names a copy one way at its mentions and another in its declarations'
  scopes. Result: **98.8% of nodes inside copies agree**, the note-only
  differences counted apart, and 1,030 disagreements booked to three
  deviations of the *twin* from the reference, each evidenced by
  `kg-oracle`: a hoisted lambda reparses as a free function (`twin-kind`),
  a `Closure` copy's `auto` parameter re-deduced (`twin-deduced`), an
  identifier the twin never typed (`twin-silent`). The rest is mostly the
  twin being silent where the engine types; M3's reference-graded gates
  judge that.
- **2026-09-26, M3, the walk.** The backend walk (`kg-ptbridge`) asks the
  engine first: `type_of` reads the node's `TypeMap` entry and `folded` the
  literal `evaluateConstexpr` left in the engine's tree, both over the
  trees `sem::elaborated_types` types once per program. The twin and the
  table remain the fallback for an entry the engine leaves `TUnresolved` or
  `TUndefined` — the second is load-bearing: a leaf the route minted for a
  name no declaration carries (`closure_calls`' capture cast) is undefined
  to a symbol lookup, and without the fall-through 56 closure programs
  regressed. The engine's notes are presented in the walk's convention
  (`copy_notes_stripped`: no `TInstance` on a class, struct, union or
  function copy, which the twin never wrote). Result: `ptdiff` 2049 → 2056,
  seven programs gained and none lost; the census and deduction still read
  the twin, which is the rest of M3.

  `KG_SEM_ONLY=1` turns the fall-through off, which measures how much of
  the walk still leans on the twin: 1983 at first. Seeding the route's
  minted leaves wherever the final tree names one (value position too, and
  through every parent of a shared argument; keyed by the twin alias,
  since the final tree holds the original) raised it to 2044. What is left
  is mostly `deduceTemplateArgs`' rewrites (`Type.hs:1390-1400`: a member
  access typed undefined on a class becomes a `FunctionSpecifier`), which
  the twin's CST pass performs and the engine's input tree does not — the
  program-wide loop's job.
- **2026-09-26, M5 begun: the loop, in shadow.** M3's census switch cannot
  be made inside the route — its census runs over a re-lowering of the
  twin's printed text — so the program-wide loop is built now, beside it:
  `kg_sem::elaborate` is `instantiateTemplates` over every module's IR at
  once (the pass list's syntactic sub-passes from `kg_hir`, `inferType`,
  the census `kg_sem::census` — `decided`, `templateResolved`,
  `constexprResolved`, `validateArg`'s stamps — then `kg_hir`'s `resolve`
  and `instantiate`, handed the decisions). A template's copies are made in
  the module declaring it; an argument written elsewhere is carried into
  that arena with its types. Rewrites keep notes through the arena's
  opt-in lineage (`ExpArena::track_lineage`); `instantiate` reads types
  through `kg_hir::NodeTypes`, which both the CST table and `TypeMap`
  answer. The engine grew what the loop exposed: `evaluateConstexpr`'s
  `TemplateInstanceF` arm (a function argument is its bare name, an alias
  its type — without it a function argument never mangles and the loop
  ran to its guard), the named-constant fold of strings, enum values and
  functions (carried across modules), and the folded enum literal's note.

  Two instruments: `cargo xtask sem --loop` compares the loop's final
  trees with the route's, top-level item by item (95.3% at first; the
  difference is not all the loop's — the reference expands an alias
  argument, `Box<|$uint@32|>` for `Box<bitindex_t<uint32>>` under
  `kg-oracle --passes 8`, and the route does not), and `KG_LOOP=1 cargo
  xtask ptdiff` walks the loop's trees: **1021** structural against the
  route's 2056 at first, six programs the route misses among them. Not yet
  in the loop: `addLambdaWrappers`, `closureCalls`, `deduceTemplateArgs`,
  `indirectInstances` and `resolveTemplateParamType`.
- **2026-09-26, M5: `deduceTemplateArgs` on the IR.** `kg_sem::deduce` is
  the whole pass — the seven rewrite arms, `deduceInstance`,
  `resolveTemplateArgs` with the defaults substituted across modules, the
  deduction with `reconcile` — over `kg_sem::from_type`'s `expFromType`,
  whose minted nodes are noted as the reference notes them (`copyNote`,
  `newNote`, `resetTypeOfExp`). It runs as frontend pass 4 and in every
  round's list. The shadow item agreement fell (95% → 87%) because the
  loop now deduces inside template bodies and the route does not; the
  reference does (`kg-oracle --tree --passes 8`: `twice`'s body holds
  `TemplateInstanceF id (SeqF [])`), and no backend walk sees a template
  body. The walk reads a declaration's raw note from the engine where no
  position key holds it (a loop copy's `setTypeOfInstance`). `KG_LOOP=1`
  ptdiff: 1021 → **1647**, 47 programs the route misses among them; what
  remains is led by the closure families (`addLambdaWrappers`,
  `closureCalls`).
- **2026-09-26, M5: the closure passes; the loop within reach of the
  route.** `kg_sem::closures` ports `addLambdaWrappers` (under the
  `NoTemplate` symbol map, `Symbols.hs:52-58`) and `closureCalls`;
  `kg_sem::pass1` ports `redundantNameScope`, without which a same-module
  function's wrapper is named `$wrapper$@ns$add` where the reference's is
  `$wrapper$add`. What the closure families then exposed, each fixed at
  its source rather than patched: the census's dependent parameter kind
  (`resolveTemplateParamType`, answered as "is the substituted kind
  resolved" — a parameter is its argument, an `auto` inside a function
  type is `TAuto`); the engine's `decltype` fold (`Constexpr.hs:304-305`),
  without which a `Closure<…, decltype(…)>` never mangles; the M1 rule that
  types a use of a function template `TUnresolved`, now for written names
  only (`desugarLambdas`' minted `Closure` argument is `TTemplate`); and
  lineage through the two rewrites that rebuild a declaration as `Donated`
  while the reference keeps its note (`moveFunctions`' parent,
  `abbreviatedFunctionTemplate`'s template and leftover —
  `Rewriter::donate_keeping_note`), without which a copy lost its
  `setTypeOfInstance` note. `KG_LOOP=1` ptdiff: 1647 → **2043** against the
  route's 2056 — 111 programs the route misses, 124 it passes.
- **2026-09-26, M5: the loop passes the route.** Three more fixes at their
  source: `deduceTemplateArgs` and the census read `getSymbols`
  (`AllSymbols`, template bodies entered), not inference's map — a local
  in a template body otherwise resolves to a template of its name and is
  "deduced" into an instance no round decides; `tArg` of a template
  argument (the route's `TNameArg TTemplate path` convention); and the
  one-literal folds of `evalBinary` (`x && false`, `x || true`, `x & 0`, an
  all-ones mask — `test/unit.k`'s `static(verbose) && succeeded`).
  `sem --loop` reports what each run leaves undecided outside a template
  body (`left`; `KG_LOOP_LEFT=1` names it). `KG_LOOP=1` ptdiff: **2160**
  against the route's 2056 — 177 programs gained, 73 still lost.
- **2026-09-26, M5: 2220.** `interpolatedStringPre`'s echo written in pass
  1, before a named constant folds away; `validateArg`'s stamp by the
  substituted kind (`T a` under `T = uint4` stamps 78 to 14); the engine's
  `bitoffsetof`/`byteoffsetof` folds; and the engine's `staticControl`
  making the reference's `UnrolledForF` of a loop whose variable reaches no
  constant position — the IR's own passes keep it a `StaticFor` for the
  twin's reparse, and the census, like `decided`, walks only an unrolled
  loop's body. `KG_LOOP=1` ptdiff **2220** (194 gained, 30 lost); the route
  itself gains one program from the engine's folds (2057).
- **2026-09-26, M5: 2236.** The census evaluates a dependent kind with its
  constants folded (`uint<clog2(N)>`, `uint<bitsizeof T>`); pass 3's
  `reifyEnums` numbers the constants with no value, each value its own
  copy of the previous one (shared, a chain folds in one inference round
  where the reference's takes a round a level, and a use of the last
  constant is typed as a literal where the reference typed it as the enum —
  `literal-operand.k::11`, found with the oracle across two modules); the
  engine's unrolling keeps an iteration's `const auto` written and carries
  the loop variable's note to it (`renote inductionVar a`). `KG_LOOP=1`
  ptdiff **2236** — 201 gained, 22 lost against the route's 2057.
- **2026-09-26, M5: no program lost.** `KG_LOOP=1` ptdiff **2272** against
  the route's baseline — 215 gained, **none lost**. The last families,
  each at its source: pass 7 `trimExternClasses` ported (`kg_sem::pass7`;
  the walk's own emulation is switched off on the loop's tree, where a
  `static if` decided later would make an `extern` count that the
  reference's pass 7 never saw); `deduplicateInstances` over every module
  each round and once more at the end (`Template.hs:57,142`); the census's
  `indirectInstances` and `resolveTemplateInstance` inside a kind (minted
  arguments, made where the template lives; a kind's instance resolves
  once its mangled copy exists); `resolve` over every module (a default
  substituted into a module carries its own file's heads); the engine's
  class-qualified scoped name (`Constexpr.hs:159-160`) and the fold of a
  constant whose type is an alias copy; and on the walk, `checkConversion`'s
  enum-value arm for the tree's own `EnumValue`, and a copy's argument read
  through its `TInstance`. The engine's folds lift the route too: 2062.

  `KG_LOOP=1 cargo xtask backend`: +167 / −8 against the route at first;
  after the kind fixes the syntax regressions pass, and
  `test/library/processor/risc-v.k` moves from *declined* (the route never
  reaches the backend) to *crash* — the budget's 900 s kill. The reference
  itself had spent 17 CPU-minutes and 7.6 GB on that file when stopped;
  the crash is the budget, not the program.
- **2026-09-26, M5: the walk reads the loop.** `kg build`'s walk now
  elaborates with `kg_sem::elaborate` — the program-wide loop on the IR, no
  twin — and `KG_ROUTE=1` walks the route's twin-typed trees for
  comparison. The route still runs, for the diagnostics alone (`Db::
  diagnostics` → `frontend_diagnostics`: the template index over twin
  roots, `table_for`'s read-back-extended tables, and the copies'
  carried diagnostics); that is M4's remaining work. What the switch took:
  `templateTypeCheck`'s `validateArg` in the census (`TypeCheck.hs:
  228-293`, the arms decidable off the engine's types — a type where a
  value is expected and the reverse, a method, a function not of the
  parameter's substituted type), so an instance the reference refuses is
  never made (`template-param-default.k::13`'s `Bar<uint14, uint12>`); a
  substituted function kind drops a `void` parameter (`Type.hs:1352-1353`);
  the function arm compares the parameters' flags and the function's
  folded attributes (`higher-order-functions.k::189`'s `[[last]] bool`),
  and the constant arm asks a written literal (`nested-templates.k::12`);
  the census descends into the kinds of a kind's own instance
  (`template.k::33`); and the engine folds a cast to an alias copy's type
  through its instance (`template.k::35`). Three walk tests that expected
  the route's declines were corrected to the reference's recorded walks
  (`closures.k::25`, `local-classes.k::15`, `type-constexpr.k::17`), each
  red under `KG_ROUTE=1`. Gates, blessed: ptdiff **2284** (from 2062, +222, none lost), backend
  agree **3195** (+175), output **204/274** (+28), repoint **51/72** (+14).
  `risc-v.k` is the backend's one new crash, blessed deliberately: killed
  at 8.3 GB resident, over the per-child memory budget — our walk peaks
  near 3 GB on it and the reference had taken 7.6 GB when stopped (above).
  The backend's `frontend` class holds four programs the reference rejects
  and this port compiles: two parser rejections (divergence #7) and
  `imports.k::63`, held before; and, new, `dependent-template.k::15`,
  whose `Invalid conversion` inside an initializer is pass 19's on the
  base module — M4's, where the route's walk declined it.
- **2026-09-26, M4 begun: the CST passes read the engine's table.**
  `Db::looped` runs the program-wide loop once per program (memoized on
  program identity, like `Db::elaboration`) for both readers: the backend
  walk's trees, and `kg_db::looped`'s per-module table — the base CST table
  with every engine type the loop's final tree holds at an `Own` node of
  the module's own text (`via == ROOT`) written at that node's key through
  `TypeTable::resolve`, the monotone write the read-back used. The route's
  read-back table (`Elaboration::table_for`) is no longer read, except
  under `KG_ROUTE=1`. The route itself still runs for the diagnostics: the
  template index over twin roots, the copies' carried diagnostics, and the
  label remap — the rest of M4.

  The engine types what the twin never did, and that found a gap in this
  port: `tIsValueType` includes `tIsClosure` (`Type.hs:686`), and
  `Ty::is_value_type` had left it out — `"{fn}"` over a lambda
  (`test/logic/string.k:802`) was rejected once the hole was typed. It
  takes the closure template's path now. One kg-db unit test was
  corrected: it ran a higher-order call with no `data.closure.core`, which
  the reference always has (`library/base.k:36`); the loop's `Closure`
  was then undefined.

  The bench's tripwire caught the cost: keystroke 1.45 s → **14.4 s**
  (`data/array.k`), the loop running to its fixpoint — 31 rounds, 15.9 s,
  three whole-program inferences a round — beside the route's bounded
  passes. The loop now takes the editor's bound as the route does
  (`PassBudget::INTERACTIVE`, four rounds, `kg_sem::elaborate::
  elaborate_with`), reported through `Db::budget_exhausted` like the
  route's; a batch compile still runs to the fixpoint. Keystroke: **2.66 s**
  worst (`risc-v.k`) — both engines run until M4 deletes the route. A
  batch `kg check` of `array.k` pays 17 s more (72.7 → 89.8 s).

  Gates: conformance **2564** (+2 decided), errors agree 660 (+3: one
  disagreement and two silences closed), strict 41 failing (−2), backend
  agree 3197 (+2), repoint 52 (+1); ptdiff and output unchanged, none lost.
- **2026-09-26, M4: one twin input left.** The walk's constant folder
  (`Db::module_fold`) reads the loop engine's table too, and the passes'
  template index is built over the base parses: the twins' copies in the
  index, their aliases and the label remap they needed moved no item of
  `conformance`, `--strict`, `errors` or `ptdiff`, and every ratchet
  graded the change byte-neutral. What the route still gives the
  diagnostics is the copies' carried diagnostics alone — and those carry
  a great deal: `KG_NO_CARRIED=1`, the instrument left in for it, loses 79
  `errors` items (75 fall silent, 4 disagree) and 82 decided assertions.
  By the reference's message: `Invalid conversion` 35, `static assertion
  failed` 10, `validateArg`'s arms 8, conflicting deduction 6, enum
  constant range 3, and a tail of single ones. They are the passes over
  the loop's copies still to port (§2.5's priority list), `checkConversion`
  first.
- **2026-09-26, M4: the loop's own diagnostics, in shadow.**
  `kg_sem::check` is the first pass after instantiation on the IR, and the
  census now says what it refuses: `validateArg`'s `argError` at the
  refused argument (`TypeCheck.hs:232`), and pass 19's `checkConversion`
  at arguments, initializers and returns (`:87-178`) over every node
  outside a template's own body — in a copy framed by its sites, at the
  module's own text unframed, yielding to the CST pass at the same node
  as the twin's "seam" did. The reference's `hasErrors` flag gates it:
  once a round refused, no later pass speaks (`Template.hs:55-56`,
  `Frontend.hs:44-46`). The census's constant arm is the reference's: an
  enum is no integer to `tIsUnsigned` (`Type.hs:599-605`), so a literal
  under an enum kind is refused (`enum.k::16`), and a folded enum value
  and an initializer list are asked too (`enum.k::17`,
  `struct-constexpr.k::4`). `KG_IR_CARRIED=1` carries these in place of
  the route's: of the 79 `errors` items only the twin's carried run
  decided, 30 are the loop's now, with conformance clean (2524 decided,
  none failing). Default off until the rest land; every ratchet neutral.
- **2026-09-26, M4: the diagnostics leave the twin.** `Db::frontend_diagnostics`
  carries the loop's own (`kg_sem::check`: pass 9 inside copies, pass 12's
  decidable arms, pass 13, pass 16's `static assert`, pass 19's conversions
  and enums, the census's `validateArg` refusals and the rounds' errors,
  gated by the reference's `hasErrors` flag); `KG_ROUTE=1` is the only way
  back. Nothing in `kg check` or `kg build` reaches the route by default.
  The census grew what the switch exposed: kinds through template-template
  parameters and through a parameter's member (`T::Foo::X`), and
  `validateArg`'s `typename` arm. Graded by the new per-change tier,
  `scripts/fast.sh walk` (six and a half minutes against the full gates'
  half hour): errors, `--strict` and backend each +14/−6, ptdiff +1,
  conformance 2569 decided (the route: 2564). The six given up are
  divergence #32 — silences all; the verdicts about what the loop *left
  undone* wait behind `KG_LEFTOVERS=1` for its parity on whole programs.
  Next: delete the twin.
- **2026-09-26, M4: the twin is deleted.** With nothing in production
  reaching it, the print-and-reparse route goes: `kg_db::elaborate` (6.5k
  lines, with its twin diagnostics, wrappers, defaults and closure calls),
  `kg_hir::readback`, the route's shadow instruments (`sem --copies`,
  `sem --loop`), `xtask ir`'s rename, read-back type-agreement and
  carried-home lines, `kg dump twin`, the walk's `TwinTyping` and the
  route's integration tests — about 11k lines. `PassBudget` moves to
  `kg_db::budget`, bounding the loop's rounds. Still standing: the rename
  layer (`kg_hir::rename`), threaded through the kg-hir rewrites as an empty
  map — its removal is a refactor of their signatures, not of behaviour.
  Graded by the milestone run: every ratchet unchanged but `repoint`'s
  `static-if` — upstream's runner fails the whole file on `static-if.k::26`,
  one of divergence #32's six. The full gates fell from about 1750 s to
  1229 s (`ir` 350 s → 100 s: its twin lines are gone), the worst
  keystroke from 2.66 s to 2.20 s (only one engine runs now).
- **2026-09-26, the twin's last residue.** The rename layer
  (`kg_hir::rename`, the `__kg_mN` aliases its print needed) is deleted
  and the kg-hir rewrites lose the map every caller passed empty; the CST
  inference loses what it kept for reading twin text (the `Imported`
  trait's hoisted scopes, `$this` aliases, closure and alias copies,
  foreign and hidden-namespace routes, alias leaves); pass 9 loses its
  twin-run fields; and the parser loses its twin mode (divergence #31,
  closed). About 3k lines more; every fast-tier ratchet byte-neutral.

## 5. Risks, and what answers each

| Risk | Answer |
|---|---|
| Two engines disagree during M1–M5 | that *is* the shadow gate; it is measured, not feared, and it ends at M6 |
| Diagnostic positions move | `Src.head` equals today's CST range for `Own` nodes; `anchor` covers tokens; `xtask errors` and conformance's positions are the gate |
| Scope/parse-order subtleties (`ParseOrder::Registered`) | the IR lowering is incremental in parse order already (`sort.rs`'s `SymTab`); the rule is ported once, into the IR symbol map, with the difffuzz corpus as its test |
| The reference's round structure is observable (static-instance numbering, copy order in `ptdiff`) | `InstanceId`/`ExpansionData.round` carry over unchanged; the M5 loop is `instantiateTemplates`' own shape, so order matches by construction rather than by `twin_round` dating |
| Scale of M4 | ported per pass, differential per pass; the twin stays until the last inside-copy pass lands |
| IDE quality on broken buffers (§2.6) | the resilience gate; the CST engine is not deleted until it reaches parity |

## 6. First concrete tasks (after the current row work)

1. `TypeKey::of_src` and a `NodeTypes` lookup service (partner-then-base),
   replacing the 33 hand-built `TypeKey { … }` literals. This is D1.4.
2. `kg_const::ConstNode`, and `ConstEval` generic over it; the IR
   implementation; delete the four IR mini-folders. This is D1.3.
3. `TypeMap` + `TyId` in `kg-ty`, with `TypeTable` becoming the CST view over
   it.
4. `infer_ir` skeleton over the sorted IR for literals, names, operators and
   calls; the `ir-infer` gate reporting agreement per constructor, so
   progress is visible from the first commit.
