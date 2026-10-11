# Kanagawa in Rust

A rewrite of the Kanagawa compiler's frontend, developer-tools first.

Two trees, and the distinction matters more than anything else here:

- `third_party/kanagawa/` — upstream Kanagawa as a git submodule, pinned at
  the commit `reference.lock` names: the Haskell reference implementation, the
  conformance corpus, and the C++ backend `kg build` links. **Read-only.**
  Never write, edit, move or delete anything under it, nor commit a new pin
  but on purpose (`cargo xtask pin-reference`). It is the oracle; a change
  there is a change to the answer key. Sources live at
  `third_party/kanagawa/compiler/hs/lib/Language/Kanagawa/`. `KANAGAWA_ROOT`
  points elsewhere; until 2026-10-11 the reference was `../kanagawa/`.
- everything else — this rewrite.

## Start here

[`docs/README.md`](docs/README.md) maps every document — what it is for,
and whether it is current, generated, a record or history. The six to read
first:

| | |
|---|---|
| [`docs/handoff.md`](docs/handoff.md) | where the last session paused, what it left, and the options from here — read it first when resuming |
| [`docs/status.md`](docs/status.md) | where things stand: every gate's number (generated), each frontend pass's state, the tools |
| [`docs/plans/sprint-parity.md`](docs/plans/sprint-parity.md) | **the active plan**: workstreams, milestones, targets, the gaps partitioned |
| [`docs/roadmap.md`](docs/roadmap.md) | what is next, the decisions that still bind, the rules that outrank the order |
| [`docs/architecture.md`](docs/architecture.md) | the crate graph, the pipeline, every gate and what it isolates |
| [`docs/journal.md`](docs/journal.md) | decisions and dead ends a `git log` cannot carry; its index names those that bind |

Then, as needed: [`docs/known-divergences.md`](docs/known-divergences.md)
(deliberate differences, each with its cost), [`docs/spec/`](docs/spec/)
(~19,000 lines of the reference's behaviour, every claim cited), and
[`docs/plans/README.md`](docs/plans/README.md) (every plan with its status).
Superseded documents are in [`docs/history/`](docs/history/README.md);
after moving a doc, run `scripts/check-links.py`.

Read the journal before changing anything that looks arbitrary. Most of it looks
arbitrary for a reason that is written down there.

## How a piece of work goes

The active plan is [`docs/plans/sprint-parity.md`](docs/plans/sprint-parity.md);
take items in its order unless the owner says otherwise. For each one:

1. **Survey before you start** (below). Read the reference clause —
   `third_party/kanagawa/compiler/hs/lib/Language/Kanagawa/` — and reproduce the gap:
   `scripts/probe.sh <program>` shows every verdict for one program, and
   `kg-oracle` (`oracle/README.md`) shows what the reference makes of it.
2. **Change the code**, citing `File.hs:line` for every rule ported. Where
   you cannot be exact, fail toward silence.
3. **Prove each new test load-bearing** with `scripts/mutant.sh`: it must go
   red with the fix undone.
4. **Run the smallest tier that can see the change** ("The loop, fastest
   first", below): `probe.sh` while converging, then `fast.sh` — `walk` if
   the loop or the walk can move, `loop` for inference or elaboration —
   and `cargo test --workspace --profile quick` before every commit.
5. **An optimisation or a refactor lands exact or not at all**:
   `cargo xtask infer-identity` (both inference modes, every fixpoint's
   trajectory), `scripts/identity.sh [<commit>]` (every root's dump from
   this checkout against `<commit>`'s, default `HEAD`, built in a
   persistent worktree), and `KG_INFER_CHECK=1` to find the first node
   that parts. Ratchets neutral.
6. **Commit per item**, the evidence in the message: which gate moved, from
   what to what. Bless an improved ratchet in the same commit
   (`cargo xtask ratchet --bless <gate>`), from the main checkout only, and
   read the baseline's diff first.
7. **Record it.** A progress line in the plan's section; a journal entry
   (at the end, dated) for anything a later reader would otherwise have to
   rediscover, with an index row if it binds. Numbers carry their date and
   the command that produced them.
8. **At a milestone** (the plan's S1–S6), the full gates:
   `scripts/gates.sh --snapshot > target/snapshot-gates.log 2>&1`, in the
   background, while you keep working; adopt its ledgers when green and
   refresh `docs/status.md` with `cargo xtask status-md`.

**Branches** (since 2026-10-11). `main` is the published history: one
commit per piece of work. Work happens on a branch off `main` named for
it (`doc-tests`, `worklist-order`), committed per item as above; when it
is done and its gates are green it lands on `main` as one squash commit —
`git merge --squash <branch>`, then a message that summarises the branch:
what changed and the gates' evidence — and the branch is deleted. Never
commit to `main` directly, never rewrite it once pushed. The history before
2026-10-11, which the docs' older commit hashes (`9320a96`, …) name, is
the local branch `dev`; it is not published.

Housekeeping that has bitten before: `third_party/kanagawa/` is read-only,
and no compiler runs with its working directory inside it (the build scripts
copy what they compile out of it, `scripts/build-reference.sh`); the gates peak near
17 GB, so never run a second heavy job beside them; never a bare
`git stash` (worktrees share the stash); after moving or renaming a doc,
`scripts/check-links.py`. The documentation's own rules are at the end of
[`docs/README.md`](docs/README.md).

## Gates

```
cargo test --workspace --profile quick   # never the dev profile: 818 s against 34 s
cargo clippy --workspace --all-targets
cargo xtask corpus                  # losslessness + acceptance
cargo xtask conformance             # decidable assertions — must stay 100%
cargo xtask conformance --strict    # exits non-zero by design; must never rise
cargo xtask bench                   # lex/parse/format/check/keystroke
cargo xtask mem [--bless]           # peak memory per use case (kg check, the editor opening a file,
                                    #   after five edits), one process each, against memory-budgets.json;
                                    #   the footprint on macOS, where the resident set counts purged pages
scripts/vs-haskell.sh [prog.k…]     # the reference frontend on one core and on every core against `kg check`,
                                    #   best of three, interleaved (performance.md §11; ~12 min; quiet machine only)
cargo xtask repoint                 # upstream's own runner grades our compat binary
cargo xtask cli                     # upstream's CLI tests (test/compiler/cli) against our compat
                                    #   binary, and every mode's --help against the reference's (~2 s)
cargo xtask ir                      # kg-hir totality, round trip, provenance,
                                    #   the pass-8 rewrites, and the oracle diff
cargo xtask errors                  # first error's position + message vs kg-oracle --errors,
                                    #   over every expected:1 snippet — what conformance is blind to
cargo xtask fuzz --differential --seconds 240 [--reduce]
                                    # mutate syntax snippets (operators, unary minus, parens, `<`/`>`
                                    #   across arguments, reorder, rename, dup/delete), grade each mutant
                                    #   against kg-oracle --errors; buckets under target/difffuzz/
cargo xtask reduce F.k [--keep errors|accept] [--like <assertion>]
                                    # ddmin F.k while ours and the oracle still disagree the same way
cargo xtask ptrace                  # record the reference's ParseTree calls per corpus program
                                    #   (needs the reference build, below); a cache, replayed when unchanged
cargo xtask ptdiff                  # our walk against the recorded one, per program: exact /
                                    #   structural / first divergent call; --dump <name> writes both
cargo xtask backend                 # our kanagawa through the real backend, per program, graded as
                                    #   syntax-test.py grades (first `Error N`, exit code); needs the build
cargo xtask output                  # every file `--backend=sv` writes, ours against the reference's,
                                    #   identical modulo source locations (the owner's parity bar);
                                    #   `exact` is the location ratchet; needs the build
cargo xtask strict-ledger           # conformance and --strict per assertion, for the ratchet
cargo xtask ratchet [GATE…]         # every latest ledger against baseline.json, one screen,
                                    #   target/gates/status.json; --bless GATE… moves a baseline;
                                    #   --rebase OLD carries assertion names across a pin move
cargo xtask infer-identity [--dump DIR]
                                    # incremental inference against the whole walk, every root,
                                    #   trajectories included; --dump writes fingerprints to diff builds
cargo xtask loop-left               # what the loop leaves undecided and its round count, per root,
                                    #   against the reference's own (kg-oracle --loop); ratcheted
cargo xtask edit-identity [--every N]
                                    # a warm Db after each kind of edit and its undo against a fresh
                                    #   one, every root — the gate every memo lands behind
cargo xtask resilience              # hover and completion on broken buffers (error-recovered files,
                                    #   fuzzed mutants): the set the editor answers, ratcheted (~2 s)
cargo xtask lint                    # retiring spellings in production code, per occurrence; ratcheted
cargo xtask pin-reference [--write] # reference.lock: the reference every gate grades against
cargo xtask status-md               # docs/status.md's headline, from the last green run
scripts/build-backend.sh [--reference]
                                    # the C++ backend from the submodule (LLVM, MLIR, CIRCT, never
                                    #   the Haskell) → target/backend/dist/bin; --reference also the
                                    #   reference's own compiler → target/reference, for the parity gates
cargo xtask dist [--no-backend]     # a release archive: kg, kanagawa, the backend, the library,
                                    #   the licenses → target/dist/kgrust-<version>-<target>.tar.gz
```

**The ratchets are per item, not per total** (`xtask/src/ratchet.rs`).
`strict-ledger`, `errors`, `ptdiff`, `backend`, `output`, `repoint`, `cli`,
`loop-left`, `lint` and `resilience`, run unfiltered, each write `target/gates/<gate>.json` — which assertions,
snippets, programs or tests sit in which set — and fail on any item that
got worse against `baseline.json`, even when the total improved: "9 up, 2
down" is red. The ratchet is then the gate's verdict (`errors` and
`repoint` no longer fail merely because disagreements exist). Improvements
pass and ask to be locked in: `cargo xtask ratchet --bless <gate>` in the
same commit as the change. The same command accepts a regression, and the
baseline's diff in that commit names every item given up — that is the
point of it, so never bless to get green without reading that diff.
Bless from the main checkout only: on 2026-09-23 a baseline blessed in a
`.claude/worktrees/` checkout disagreed with main on 21 `repoint` tests
(26 against 27 passing, the sets +11/-10) where main's own runs agreed
with each other exactly — upstream's runner is not location-independent,
reason unmeasured.

**Every gate runs under a memory budget** (`xtask/src/bounded.rs`), since
a session's gates and side runs ran this 64 GB machine out of memory on
2026-09-23. Each compiler, oracle or runner process a gate spawns runs in
a process group of its own, its backend worker included, killed past 8 GB
or 900 s (`KG_CHILD_MEM_MB`, `KG_CHILD_SECS`) and graded as that program's
crash. Each xtask process stops itself past 16 GB (`KG_XTASK_MEM_MB`, exit
99). `scripts/gates.sh` kills its whole run below 10% free system-wide
(`KG_GATES_MIN_FREE_PCT`). A green run peaks at 17 GB, `ptdiff` 7.5 GB of it:
**do not start a second heavy job — another gate, a `backend` run, a batch
of compiles — while the gates run**, and time one program with
`/usr/bin/time -l`, which also reports its peak memory.

`backend`, `repoint` and `cli` grade the `kanagawa` cargo reports after `cargo
build --release -p kg-cli --bin kanagawa`, which they run themselves (a
no-op on a fresh tree); the first line of their output says whether the
binary was stale. `ptdiff` and `backend` refuse a recording made from
another program text or another reference tree (`ptrace.rs`, `load`).

Every one of them is bracketed by a check that the reference tree did not
*grow* while it ran (`xtask/src/guard.rs`): a gate that spawns the compiler
leaves `<base>PathLength.pdrpt` in its working directory, and 163 such
reports reached the reference's `test/` before the cwds were fixed. The check
names any file that appeared and fails the run; it never deletes one — a
file that should not be there is evidence, and the gate that made it is the
thing to fix.

**The loop, fastest first.** Pick the smallest tier that can see the change:

| Command | Runs | Time |
|---|---|---|
| `scripts/probe.sh <program>` | every verdict for **one** program — conformance and `--strict`, `errors`, `ptdiff`'s first divergence, `output`, both compilers' whole backend output | ~10 s |
| `scripts/quick.sh <test filter>` | one test filter over the frontend crates, then `conformance` and `errors` | ~30 s |
| `scripts/fast.sh` | `conformance`, `errors`, `--strict`, `cli`, then their ratchets | ~35 s |
| `scripts/fast.sh walk` | … and `ptdiff`, `backend` (replayed from the cache) | ~80 s |
| `scripts/fast.sh loop` | … and `infer-identity`, `loop-left`, a quarter of `edit-identity` — any change to inference, the program-wide loop or a memo | ~3.5 min |
| `scripts/gates.sh` | everything: the **milestone** run — a switch, a deletion, before blessing `output` or `repoint` | 4.5 min warm (2026-09-27, `--snapshot`); ~15 min after a deep rebuild or a cold backend cache |

`--lint` adds clippy to any `fast.sh` tier. Bless from the fast tier's
ledgers (`cargo xtask ratchet --bless <gates>`). Converging on a divergence
is `probe.sh` in a loop, not a gate per question; proving a new test
load-bearing is `scripts/mutant.sh <file> <old> <new> -- <check>`, which
plants the mutation, requires the check to go red and restores the file on
every exit. Running the full gates per commit had become the bottleneck
(2026-09-26); they are the milestone run only.

**Edit while the milestone gates run: `scripts/gates.sh --snapshot`.** It
runs the gates on a committed tree (default `HEAD`) in a persistent
worktree, `target/snapshot/tree`, which builds into its own `target/` and
links the data stores, so this checkout stays free — the rule against
editing under a running gate is about the tree being graded, and the
snapshot is not this one. Ledgers name their tree by content: while this
checkout still matches the commit, `cp target/snapshot/tree/target/gates/*
target/gates/` lets `cargo xtask ratchet` read or bless them here.

**Parallel work in worktrees.** Independent items — the sprint's W2 rows,
say — can go to agents in isolated worktrees (the Agent tool's
`isolation: "worktree"`, or `scripts/worktree.sh add <path>`). In a new
worktree run `scripts/worktree.sh link` once: it links the gates' data
stores (the reference's recordings, the backend and oracle caches,
`output`'s stored side) from the main checkout's `target/`. Every script
finds the reference checkout from any worktree. The first build in a
worktree takes a few minutes; later ones are incremental. **One heavy run at
a time, machine-wide:** the walk and loop tiers and the full gates take a
lock in the main checkout's `target/` and wait for it (`scripts/lib.sh`),
because they peak near 17 GB and share work directories; `probe.sh`,
`quick.sh` and the diagnostics tier do not take it. Keep to two or three
agents building at once. Work comes home as commits on the worktree's
branch, merged or cherry-picked into `main` after its fast tier is green
there; bless `baseline.json` only from the main checkout.

**No doc tests.** Every library sets `doctest = false`: the workspace had
none, and rustdoc still cost 49 s of every `cargo test --workspace`. An
example is a unit test.

**`backend`, `output` and `repoint` replay the C++ backend from a cache**
(`kg_ptbridge::cache`, `target/backend-cache`): a program whose recording —
every call the backend is told, byte for byte — and backend library are
unchanged gets its stored verdict, streams and `out/` files back without a
worker. Crashes are never stored. `gates.sh` re-runs one hit in 20 against
the real backend (`KG_BACKEND_CACHE_VERIFY`, `1` for every hit) and a
difference fails the gate: the cache's premise, a deterministic backend, is
re-proven rather than assumed. `KG_BACKEND_CACHE=` (empty) turns it off.
The replay code is in the key by its meaningful text — editing a comment or
a test in `kg-ptbridge`'s replay files keeps the cache; editing code
discards it (~3 GB; the next backend and output runs are cold).

`scripts/gates.sh > gates.log 2>&1` runs all of them and **exits non-zero on
any regression** — a test, a clippy warning, conformance below 100%, a
ratchet — ending on a one-screen summary (`target/gates/status.json` holds
it as data). `--no-backend` leaves out `ptdiff`, `backend` and `repoint`,
which need the reference build's recordings and library (`target/reference`,
`scripts/build-backend.sh --reference`; `KANAGAWA_BUILD` points elsewhere);
without the flag a missing build is red, not skipped. Measured 2026-09-27 on a quiet machine:
871 s in all — tests 328 (a full recompile after a `kg-base` change; 35 s
at rest), clippy 100, the first batch 17, the second (`ir`, `sem`,
`ptdiff`, `backend`, `repoint`) 72 with the backend cache warm, `output`
239 with it cold, bench 104. Timings per
stage are in `docs/journal.md`, "The gates, spread over threads"; a stage that
takes an order of magnitude longer than the figure there is a regression in
the driver or the compiler, and either way it is news.

## A gate is a driver over threads, and the driver is not the grade

Every corpus-walking gate — `conformance`, `errors`, `ir`, `repoint`, and the
two corpus walks in `kg-db/tests/query.rs` — hands its files or snippets to
threads that each own a `Db`, writes each result into a slot indexed by its
position, and builds the report from the slots in corpus order. The grading
functions (`check_snippet`, `score`, `judge`, the ledgers) are called exactly
as the one-`Db` loop called them; only which thread calls them changed, and
every report was diffed byte-for-byte against the sequential one before the
change was kept (2026-09-17). Keep it that way:

- **A new gate, or a new stage in one, is written this shape from the start.**
  One `Db` walking the corpus one file at a time is how five drivers came to
  cost nine minutes between them; there is no reason to share a `Db` across
  *files* that is not equally a reason to give each *thread* one.
- **Results go in indexed slots, never a shared `Vec` pushed from threads.**
  The report's order is the corpus's order, and a `take(5)` of failures has
  to name the same five on every run. Where a per-thread accumulator is
  unavoidable (`ir`'s `CountGrades`), merge is a fold with no order in it and
  every failure list is stably sorted by root afterwards.
- **`cargo xtask conformance --jobs 1` is the one-`Db` loop in corpus order,
  and the reading to fall back to.** A snippet whose verdict differs between
  `--jobs 1` and the default means the verdict depends on what the `Db`
  checked before it — a bug in a memo, never in the fan-out, and worth a
  journal entry before it is fixed.
- **Per-thread `Db`s, not one `Db` shared across threads, for the gates.**
  `Db` is `Sync` and the shared form works (the `query.rs` walks use it), but
  it scaled 2.5× on eight threads where a `Db` per thread scaled 7× in the
  same session — the bounded root-set memos (64) under interleaved roots are
  the likely reason, unmeasured.

The reference's frontend also builds here — `oracle/build.sh`, about two minutes
— and `oracle/.../kg-oracle --tree FILE` prints the tree as the reference holds
it. It is the only instrument whose two sides do not share our lowering, so it
grades `kg dump ir` against the reference rather than against our reading of it.
The executable needs a C++ backend; the frontend library does not, and that
distinction is why "there is no GHC here" was believed for so long.

Current values are in [`docs/status.md`](docs/status.md).

`--strict` is *expected* to fail — it counts what the frontend cannot yet
decide. It falling is progress. `conformance` dropping below 100% is a
regression, never a trade.

`errors` is the instrument for what `conformance` cannot see: the harness
compares exit codes and `Error N`, never a position or a message. `errors`
runs `kg-oracle --errors` on every must-reject snippet and grades our first
error against the reference's — its disagreement count may fall and must not
rise. Where the reference's *parser* rejects and a later pass here does, the
line is booked to divergence #7, not failed; a hint list compares as a set
(divergence #29).

## The scoreboard is off-limits

`xtask/src/conformance.rs`, `xtask/src/corpus.rs` and `FRONTEND_COMPLETE` are the
scoreboard. Code being graded does not edit its own grade. If a conformance
number moves, it moves because the compiler changed. (`FRONTEND_COMPLETE` is
`true` since 2026-10-10, by the owner's decision: an `expected:1` snippet
that compiles cleanly is a failure, not undecidable.)

## Test discipline

- **Never weaken a test to make it pass.** A test encoding a wrong expectation
  may be *corrected*, with a citation to the reference explaining why — never
  deleted, never loosened to go green.
- **Prove a new test is load-bearing**: break the code it covers, watch it go
  red, restore — `scripts/mutant.sh <file> <old> <new> -- <check>` does all
  three and restores the file on every exit. A test that passes with and without the fix pins nothing. This
  has caught real mistakes twice — an overflow boundary test that probed line 0,
  where the offset is 0 and nothing can overflow; and a performance test that
  passed against the quadratic code it was written to catch, which was deleted
  rather than kept.
- **Where you cannot be exact, fail toward silence.** A missed diagnostic
  fails one `expected:1` assertion and says what is missing. An invented one
  breaks an `expected:0` assertion — and possibly the 241 the reference defers
  to the backend — and is a lie about a correct program.

## Benchmark a new pass, not just its correctness

`cargo xtask bench` reports worst-case whole-corpus `check` and `keystroke`.

Pass 4 landed correct — 961 tests green, clippy clean, conformance up seven
assertions — and made `check` on `test/library/processor/risc-v.k` take **1.9
seconds instead of 95 ms**, because its shadow guard walked each enclosing
scope's subtree once per call. Every correctness gate passed for as long as the
regression existed. The pass was right, and unusable, and only the benchmark
could tell.

**Performance is a goal, correctness is never the price** (since
2026-09-26; before that the bench was only a tripwire). The program-wide loop
made the compiler's cost the testing loop's cost — `array.k` 16 s, full gates
20 minutes — and a slow frontend defeats the tooling it exists for.
`docs/plans/performance.md` is the plan. Every optimisation is **exact**:
byte-identical results, every ratchet neutral, the old path kept behind a
switch with an identity check until it agrees everywhere — for inference and
the loop, `infer-identity`, its `--dump` diffed across builds, and
`KG_INFER_CHECK=1` (step 5 of "How a piece of work goes"). Measure before and
after; optimise measured hotspots, and do not complicate a port for
milliseconds nobody measured. An exact optimisation that falls out of the
algorithm — the `null selected` guard is `Function.hs:30`'s own semantics —
is always free; take those.

Read absolute bench numbers with suspicion: this machine is bimodal under
load and has produced both 75/109 ms and 93/138 ms from the same tree. If a
stage that could not have been affected moved too — `lex` rising with
`check` — it is load. Interleaved A/B on one quiet machine is the only
reading worth recording.

## Formatting

Hand-formatted; `cargo fmt` is disabled in `rustfmt.toml`. Read that file before
overriding it.

## House style

Doc comments cite the reference as `File.hs:line`. Comments explain *why* —
especially where this port diverges from the reference, or from what a reader
would reasonably expect. Do not narrate what the code does.

## A gate's specification is a hypothesis

Landed twice now. The second time: pass 8's provenance clause four was specified
as "forbid an `Own` node the lowering did not create", and landed as written it
rejects the first file it sees — an append-only arena cannot change a node in
place, so every legitimate *replacement* is a fresh index. Written the way that
bites, it reads "an appended `Own` node must not be structurally equal to a
lowered node at its own key". A specification is a hypothesis about what a check
will catch; only running it says.

Writing "break X and watch it go red" into a design is worth little; *running* it
is the whole value. Two of the three gates specified for the pass 8 IR did not
pin what they claimed — the round trip cancels any deterministic lowering bug
(both sides share the lowering), and the provenance invariant was vacuous against
the very bug the design named as its red test. Both were found by running the
falsification, not by reading it. Extends the test-discipline rule below to gates
specified before the code exists.

## Survey before you schedule

A TODO note in the code is a previous session's *claim*, not a fact. The
constexpr-fold roadmap item said "the corpus pins all six sites" and named a
file and line for each. The citations were real; every one of them was an
already-passing accept assertion that pinned nothing, and one of the six sites
would have invented errors if converted. Read the reference and the corpus for
yourself before doing work a note tells you to do — especially a note that
sounds well-evidenced.

## A rewrite's `Src` donors need a unit test read off `--notes`

Every clause of `xtask ir`'s provenance gate but one fires on `written == Own`.
The exception — "this `Src::head` names a live CST node" — fires on every
reachable node, so a `Donated` node's whole obligation is that its range resolves
to *some* CST node, which `Src::donated` makes true by construction. Swapping
two donors in a rewrite leaves the entire gate byte-identical; so does hanging a
synthesized type on the parameter's *name* token. Measured, not argued.

A corpus-wide equality gate cannot replace it: our spans and the reference's are
different objects and agree for only 93.4% of nodes (divergence #25).

So each rewrite lands with donor assertions read off
`kg-oracle --notes --roots-only --passes 8 --template-passes N
--template-iterations 0`, verified by swapping two donors and watching the test
go red. `abbreviated.rs`'s `the_two_donors_inside_one_synthesized_type_param_differ`
is the model.

## Grade a rewrite against the reference, and know what the grade is blind to

`rewrite:`-style lines are **self-reports**: they say a pass ran and left the
tree sound. Two lines now grade a rewrite against the reference itself, and the
difference between how they had to be built is the reusable part.

The cheap instrument is a **constructor count** at a pass boundary — one the
pass alone mints, guarded by one no pass in the window touches. `capture diff`
is `ThisF` guarded by `LambdaF` at `--passes 3`, and it is 122/122. It works
only when nothing *unported* runs inside the window, which was luck.

`desugarLambdas` has no such window: `addLambdaWrappers` is sub-pass 1, unported,
and mints the same constructors — so the reference's counts are unreachable by
construction and its output lands inside other lambdas' bodies. The instrument
that survives that is a **per-item ledger** of fields each derived from one
source node, so a foreign item is simply an entry dropped by name. Pair by
adjacency, not by name; synthesized names recur.

Whatever the instrument, **write down what it cannot see**, having tried it:
`lambda diff` is blind to the *position* of a synthesized parameter (two unit
tests catch that instead), and every `Donated` node's provenance is invisible to
`xtask ir` entirely (the rule above). A gate whose blind spots are unstated
reads as total coverage. Two green numbers side by side imply independent
evidence; check whether they are. `capture diff` and `lambda diff` move to the
*same* 298/351 under the same plant.

**Grade it over the whole corpus, not the convenient part.** Both lines graded
`library/` and printed 122/122 while covering 9% of the corpus's lambdas.
Extending them to `test/` — one `collect` call — found a defect in the gate on
its first run. And when an unported pass forces you to relax a comparison,
**find the quantity it conserves**: a relaxation with nothing holding it down is
indistinguishable from deleting the check. Here it is "a written lambda either
becomes a ledger entry or survives, never both and never neither", which is what
stops the relaxed path admitting a port that rewrites nothing.

## The recurring failure

Four times, a module was written, tested, and shipped with **no caller**:
`WarningSet::filter`, `Db::check`, `Db::diagnostics`, and the whole `kg-fmt`
integration — the editor greyed out "Format Document" for a working formatter,
for phases.

**A crate is not shipped until something a user types reaches it. Tests do not
count.** Drive it the way a user would — run the binary, speak to the language
server over stdio — before calling it done.
