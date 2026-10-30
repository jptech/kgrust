# The gates

**Status: current.** How kgrust is graded against the reference: every
gate, what it grades and what it is blind to, how the scripts drive them,
and the rules for ratchets, caches and memory. The numbers are generated
into [`status.md`](status.md); the rules every change follows are in
[`AGENTS.md`](../AGENTS.md). A gate's code is the last word: each
`xtask/src/<gate>.rs` opens with what it grades.

## Running them

### The tiers

Pick the smallest tier that can see the change (the table is in
[`AGENTS.md`](../AGENTS.md#the-loop-fastest-first)):

- `scripts/probe.sh <program>` (~10 s) — every verdict for one program,
  named as a gate names it (`syntax::basics::110`,
  `file::test/library/data/array.k`) or by a corpus path; logs under
  `target/probe/`.
- `scripts/quick.sh <test filter>` (~30 s) — one test filter over the
  frontend crates, then `conformance` and `errors`.
- `scripts/fast.sh [--lint] [walk|loop]` — the gates a change can move,
  graded by their ratchets: ~35 s, ~80 s with the walk, ~4 min with the
  loop. Workspace tests are not run here; run them before committing.
- `scripts/gates.sh` — everything, below.

CI is a lighter tier beside these, not one of them:
[`scripts/ci.sh`](../scripts/ci.sh) — the quick tests, `corpus`,
`conformance`, `fmt-check`, `lint`, `codegen --check`, clippy, the links,
over the workspace but `kg-ptrace` — which GitHub runs on
every push and pull request, sized for its free runners. It covers this
repository's Rust code and builds neither the backend nor the reference, so
it runs none of the gates below that need them — `errors`, `ir` and
`loop-left` (kg-oracle), `ptdiff`, `backend`, `output`, `repoint`, `cli`
and `edit-identity` (the reference's build) — nor `resilience`, `bench` or
`mem`.

`probe.sh`, `quick.sh` and the diagnostics tier take no lock. The walk and
loop tiers, `gates.sh`, `scripts/identity.sh` and `scripts/build-backend.sh`
take the machine-wide heavy lock ([Memory](#memory)). Every script runs
from any worktree of the repository (`scripts/worktree.sh`).

The gates run the binaries built under `target/`, never a `kg` on `PATH`, so a
stale `cargo xtask install` cannot skew a verdict.

### `scripts/gates.sh`, stage by stage

`scripts/gates.sh [--no-backend] [--no-bench] [--snapshot[=<commit>]] >
target/gates.log 2>&1` exits non-zero on any regression and ends on a
one-screen summary; `target/gates/status.json` holds it as data and
`target/gates/<stage>.log` each stage's output.

1. **build** — `cargo build --release -p xtask -p kg-cli`.
2. **tests** — `cargo test --profile quick --workspace` (opt-level 1, debug
   assertions on; 34 s for the workspace where the dev profile took 818 s).
3. **clippy** — `--workspace --all-targets -- -D warnings`.
4. **`codegen --check`**, then **`lint`**.
5. Together, each over threads of its own: **`fmt-check`**,
   **`conformance`**, **`conformance --strict`**, **`errors`**, **`corpus`**,
   **`editions`**.
6. Together: **`strict-ledger`** (checked against both conformance logs),
   **`ir`**, **`resilience`**, **`ptdiff`**, **`backend`**, **`repoint`**,
   **`cli`**.
7. One after another: **`loop-left`**; **`edit-identity`**, which writes
   the `shadow` ledger; **`output`**, alone — the reference compiler, when
   it records, peaks at 13.7 GB across the gate's four workers.
8. **`bench`** (a tripwire, never failed) and **`mem`** (fails over budget).
9. **The summary**: `cargo xtask ratchet` over every ledger, with each
   stage's outcome.

Red means any of: the build or a test failed; clippy warned; a generated
file is out of date; `fmt-check`, `corpus`, `editions`, `ir` or
`edit-identity` failed;
conformance fell below 100%; `strict-ledger` disagrees with the
scoreboard's own logs; `mem` is over a budget; any ratchet has an item that
is newly failing.

`--no-backend` leaves out what needs the reference's build or recordings —
`ptdiff`, `backend`, `repoint`, `cli`, `loop-left`, `edit-identity` (and so
`shadow`) and `output`, whose ratchets are then not checked. Without it a
missing build is red, never quietly skipped. `--no-bench` leaves out
`bench` and `mem`.

**`--snapshot[=<commit>]`** runs the gates on a committed tree (default
`HEAD`) in a persistent worktree, `target/snapshot/tree`, which builds into
its own `target/` and links the data stores, so the checkout you are in
stays free to edit — the rule against editing under a running gate is about
the tree being graded. Uncommitted changes are not in the snapshot. Its
results land in `target/snapshot/tree/target/gates/`; a ledger names the
tree it graded by content, so while your checkout still matches the commit,
`cp target/snapshot/tree/target/gates/* target/gates/` lets `cargo xtask
ratchet` read or bless them there.

**How long.** ~6 min warm (361 s with `--snapshot`, 2026-10-11, a quiet
machine); 15–30 min cold — after a change to a base crate (the tests
recompile everything), with a cold backend cache, or after a re-record
(1,880 s on 2026-10-11, the first run after `ptrace` re-recorded). Building
the backend from scratch is ~28 min on 16 cores. Per-stage figures are in
the journal (["The gates, spread over
threads"](journal.md#the-gates-spread-over-threads)); a stage an order of
magnitude slower than its figure there is a regression in the driver or
the compiler, and either way it is news. `gates.sh` holds idle sleep off
(`caffeinate`): a Mac that sleeps under a run makes its stage times
meaningless.

## Every gate

Those that read the reference take `--kanagawa-root` (default
`KANAGAWA_ROOT`, else the submodule); those that run the reference's build
take `--build-dir` (`KANAGAWA_BUILD`,
default `target/reference`). Most take `--filter <substring>`, and a
filtered run writes no ledger.

### `corpus`, `fmt-check`, `codegen --check`, `lint`

- **`corpus`** — every whole program under the reference's `library/` and
  `test/` round-trips through the lexer and CST byte for byte and parses
  with zero errors. `test/syntax/` is left to `conformance`: its files are
  snippet accumulators, not translation units.
- **`fmt-check`** — the formatter is idempotent and changes layout only,
  over the corpus.
- **`codegen --check`** — the generated syntax files, the editor's grammar
  and the ABI's bindings are what `cargo xtask codegen` would write; the
  bindings are generated from the pinned reference's headers (libclang)
  and checked in, so the tools build without the reference or libclang.
- **`lint`** — retiring spellings in production code (mangled text, a
  diagnostic's message as control flow, environment verdict switches, …),
  counted per occurrence: a count may fall, never rise.

### `editions`

`cargo xtask editions` grades the language beyond the reference
([`editions/`](editions/README.md); the design,
[`plans/editions.md`](plans/editions.md) §6), pass/fail, in ~6 s; in
`fast.sh`'s diagnostics tier and `gates.sh`:

- **neutral** — every `.k` file under the reference's `library/`, `test/`,
  `doc/` and `tools/` parses to the same tree, with the same errors, with
  every feature on as with none;
- **claims** — every fixture of `tests/editions/<feature>/` is rejected by
  `kg-oracle --errors` and by `kg` in edition 1.0;
- **twins** — each `accept/x.k` checks clean in its feature, its
  hand-written `x.ref.k` is accepted by the oracle and by edition 1.0, and
  the two walk through the trace backend call for call, locations aside;
- **rejections** — each `reject/x.k` gives exactly the errors its
  `// expect-error L:C: message` lines list, in order.

Blind to: a feature that acts after the parse — every feature so far acts
only where the parser built its construct, which is what makes a parse
identity a program identity; the first feature that does not (a stricter
check, say) needs the neutral check widened to diagnostics. The twins are
walked, not built: the backend's output is compared by hand
([`editions/1.1.md`](editions/1.1.md)). Each check proven red by a planted
mutation, 2026-10-30.

### `conformance` and `strict-ledger`

**Grades** the frontend's verdict — accept or reject, and `Error N` — for
every `expected:` assertion embedded in `test/syntax/`, split exactly as
upstream's `syntax-test.py` splits them; that includes the 406 in
`basics.k`, which upstream registers with no test and so never runs. Since
`FRONTEND_COMPLETE` was set (2026-10-10, the maintainer's decision) an
`expected:1` snippet that compiles cleanly is a failure, not undecidable,
so the plain run and `--strict` grade identically and must both fail
nothing: conformance below 100% is a regression, never a trade.
`strict-ledger` re-grades per assertion for the `strict` ratchet and checks
that it reproduces both runs' logs. (`--max-failures` dates from the era
when `--strict` was expected to fail.)

**Blind to** positions and messages — that is `errors` — and the 241
assertions only the C++ backend decides expect the frontend *not* to catch
what the reference's frontend lets through: in `kanagawa` compat mode the
division of labour is reproduced, not improved.

**Driven** over threads, a `Db` each; `--jobs 1` is the one-`Db` loop in
corpus order ([below](#a-gate-is-a-driver-over-threads)).

### `errors`

**Grades** our first error's position and message against `kg-oracle
--errors`, over every `expected:1` snippet — the class of bug `conformance`
cannot see. Its fail sets are `disagree`, `silent`, `oracle-silent` and
`skipped`. Where the reference's *parser* rejects and a later pass here
does, the line is booked to divergence #7, not failed; a hint list compares
as a set (#29).

**Blind to** everything after the first error, and to programs that compile.

**Driven** over threads; the oracle's runs replay from
`target/oracle-cache/`, keyed by the oracle binary, every `.k` file of the
reference's corpus, the arguments, the working directory and the text
(`KG_ORACLE_CACHE=0` bypasses it). Needs `oracle/build.sh`.

### `ir`

**Grades** the lowering to `kg-hir`: totality over every file the parser
accepts, the pretty → reparse → lower round trip (plain and sorted), the
provenance invariant, the rewrites, and a diff against the reference's own
lowered tree (`kg-oracle --tree`).

**Blind to** any deterministic lowering bug in the round trip, which has
our lowering on both sides and cancels it (reversing every ternary's arms
leaves it green) — the oracle diff is the only instrument whose two sides
do not share our lowering; and to a `Donated` node's provenance entirely
([below](#src-donors-need-a-unit-test)).

### `ptrace` and `ptdiff`

`cargo xtask ptrace` records what the reference frontend tells the backend
— every ParseTree call, through `kg-ptrace`, a shim library that logs and
forwards — for every registered assertion and every whole corpus file,
into `target/ptrace/`. A run is keyed by the program's text and flags, the
three binaries, and every `.k` file the program can import: a rebuilt
reference or an edited corpus clears the store, and `--force` re-records
regardless. No gate records; run it after building the reference (~17.5 min
for 3,270 programs on 2026-10-11). The keys carry absolute paths, so a moved
checkout re-records.

**`ptdiff` grades** our walk against the recording, per program: `exact`,
`structural`, or the first divergent call (`--dump <name>` writes both
traces). **Blind to** what the backend makes of the calls (`backend` and
`output` grade that); and `exact` also compares source positions, where our
spans and the reference's differ (divergence #25), which is why
`structural` is the bar. It refuses a recording made from another program
text or another reference tree.

### `backend`

**Grades** our `kanagawa` compat binary through the real C++ backend, per
recorded program, as `syntax-test.py` grades: the first `Error N` and the
exit code, and whether the first backend error line matches (`message`). A
program over the child budget is a `crash`. `--show <name>` prints one
program's command line and both outputs. **Blind to** what the backend
writes; that is `output`.

### `output`

**Grades** every file `kanagawa --backend=sv` writes, ours against the
reference's, over every program the reference compiles through codegen.
Parity is identical output modulo source locations (the maintainer's bar,
since 2026-09-23): the `equal` set. `exact`, byte identity, is the location
ratchet. Both compilers run in the same directory with the same arguments;
the reference's side is stored under `target/output/theirs/`, keyed by both
reference binaries, every importable `.k` file, the program's text and
flags, and the directory.

**Blind to** a program the reference writes nothing for and ours does (ours
never runs there), and to anything under `out/` that is not a file. A
location comment in a shape the normaliser does not know reads as a
difference, never as agreement.

### `repoint` and `cli`

- **`repoint`** — upstream's own `syntax-test.py` drives our compat binary
  over every registered test file: the cross-check `conformance` cannot
  perform on itself. Upstream's runner is not location-independent; bless
  it only in the primary checkout ([below](#ratchets-and-blessing)).
- **`cli`** — upstream's CLI tests (`test/compiler/cli`, run as the
  reference build's `CTestTestfile.cmake` runs them) against the compat
  binary, and every mode's `--help` and usage against the reference
  binary's, the version masked.

`backend`, `output`, `repoint` and `cli` grade the `kanagawa` that `cargo
build --release -p kg-cli --bin kanagawa` produces, running that build
themselves (a no-op on a fresh tree); their first line says whether the
binary was stale.

### `loop-left`

**Grades** what the program-wide loop leaves undecided, and its round
count, per root the reference compiles, against the reference's own loop
(`kg-oracle --loop`), under each root's recorded compile settings. Pass
sets `agree` and `rounds-agree`; fail sets `ours-only`, `theirs-only`,
`near-guard` and `failed`.

### `infer-identity`

**Grades** incremental inference against the whole walk over every root,
every fixpoint's trajectory included, both inference modes in one build,
and the editor's layered run beside them. Its exit code is its verdict; it
writes no ledger. `--dump DIR` writes each root's fingerprint to diff two
builds — `scripts/identity.sh [<commit>]` does that in one command,
building `<commit>` in `target/identity-base` — and `KG_INFER_CHECK=1`
compares every iteration and names the first node that parts. Not in
`gates.sh`: `fast.sh loop` runs it, and every optimisation lands behind it.

### `edit-identity` and `shadow`

**Grades** a warm `Db` after each kind of edit, and after its undo, against
a fresh one over the same text, every root; any difference is red. The warm
database is the editor's, so its row also says the editor's last answer is
`kg check`'s on every root. It is the gate every memo lands behind. Its
`shadow` ledger checks the editor's kept loop: each kept round whose keys
agree is restored and compared with the round the next run ran, and every
prefix pass the editor reused is also run on a fork and compared (`kept`, a
pass set; `unequal` and `reuse-unequal`, fail sets).

**Driven** whole by `gates.sh`. `--every N` samples every Nth root, and
`fast.sh loop` runs a quarter; a sampled run writes no ledger and is held
to the baseline's items for the roots it ran. A quarter is not the whole: a
change to a verdict about what the loop left standing needs the full run
([journal](journal.md#the-fuzzers-silent-buckets-the-parsers-table-and-the-copies-passes--2026-10-10),
its afterword).

### `resilience`

**Grades** hover and completion at sampled nodes of the corpus's
error-recovered files and 64 fuzzed broken buffers, as the editor answers
them: every answer recorded per node, so a change in what the editor says
is named, and why it says nothing, counted. About 2 s.

### `bench` and `vs-haskell.sh`

`bench` is a tripwire, never failed: stage timings and the worst keystroke,
to the editor's first answer (`keystroke`) and to its last (`fixpoint`).
Correctness gates cannot see cost: a pass that landed with every gate green
made `check` on `risc-v.k` take 1.9 s instead of 95 ms
([journal](journal.md#right-and-unusable)). Read absolute numbers with
suspicion — the bench is bimodal under load, and has read both 75/109 ms
and 93/138 ms from one tree; if a stage the change could not touch moved
too (`lex` rising with `check`), it is load. Record only interleaved A/B
runs on a quiet machine. `scripts/vs-haskell.sh` (~12 min, a quiet machine
only) is the standing against the reference frontend, on one core and on
every core, best of three, interleaved.

### `mem`

Peak memory per use case — `kg check`, the editor opening a file, the
editor after five edits — over five programs, one process each, against
`memory-budgets.json`; a case over budget fails the run. On macOS it reads
the peak footprint, not the resident set, which counts pages the allocator
has already purged. `--bless` resets each budget to its peak plus a quarter.

### Off the gates

- `cargo xtask passdiff --pass <name> [--oracle]` — a CST pass against the
  loop's version of it at each module's text, over every root and snippet;
  what a pass's move onto the loop lands behind.
- `cargo xtask fuzz --differential --seconds 240 [--reduce]` — mutated
  syntax snippets, each graded against `kg-oracle --errors`, bucketed under
  `target/difffuzz/`. Without `--differential`: crash invariants for the
  lexer, parser and formatter.
- `cargo xtask reduce F.k [--keep errors|accept] [--like <assertion>]` —
  delta-debugs a program while ours and the reference still disagree the
  same way.

## Ratchets and blessing

The ratcheted gates are `strict`, `errors`, `ptdiff`, `backend`, `output`,
`repoint`, `cli`, `loop-left`, `lint`, `resilience` and `shadow`
(`xtask/src/ratchet.rs`). Each, run unfiltered, writes a ledger,
`target/gates/<gate>.json` — its totals, its named sets of items
(assertions, snippets, programs, tests), its histograms — and `cargo xtask
ratchet` compares it with the gate's section of `baseline.json`:

- a **pass set** fails on an item that left it — an item not graded at all
  included, so an unrecorded or stale program is never a silent pass;
- a **fail set** fails on an item that entered it;
- a total named `graded` may not fall;
- histograms are reported as deltas and never fail.

A total that rose while one item fell is red. The ratchet is the gate's
verdict: `errors` and `repoint` do not fail merely because disagreements
exist.

**Blessing.** `cargo xtask ratchet --bless <gate>…` writes the named
ledgers into `baseline.json` — the one deliberate way to move a ratchet, in
either direction:

- in the same commit as the change; until then a later regression of the
  same item is invisible;
- after reading the baseline's diff: a bless accepts regressions too, and
  that diff, in that commit, names every item given up — never bless to get
  green without reading it;
- in the primary checkout only: on 2026-09-23 a `repoint` baseline blessed
  in a linked worktree disagreed with the primary checkout's on 21 tests
  (+11/−10) where the primary checkout's own runs agreed exactly — upstream's
  runner is not location-independent, reason unmeasured;
- from this tree's ledgers: each carries the fingerprint of the tree it
  graded, and `ratchet` refuses another tree's;
- never from a run under a verdict switch (`KG_LEFTOVERS`, `KG_INFER_FULL`,
  `KG_PTBRIDGE_SEED`), which xtask announces when one is set.

**`status.json` and `status.md`.** Every `cargo xtask ratchet` that is not a
bless rewrites `target/gates/status.json` with the gates it was asked about
— `gates.sh`'s summary with all of them and every stage, `fast.sh`'s with
its few. `cargo xtask status-md` renders `docs/status.md`'s headline from
that file, refusing a red run or another tree's. Render it only straight
after a complete green `gates.sh`, or the rows the file lacks read `?`.

**Across a pin move.** Assertions are named by position
(`syntax::basics::17`), so a snippet added upstream renames every one after
it, in every ratchet that names assertions. `cargo xtask ratchet --rebase
<old commit>` maps the old names to the new by file, expectation and text
and rewrites `baseline.json`; a snippet with no unique twin keeps its old
name and is listed. The whole checklist is in
[`AGENTS.md`](../AGENTS.md#checklists).

## The backend verdict cache

`backend`, `output` and `repoint` replay the C++ backend from a cache
(`kg_ptbridge::cache`, `target/backend-cache`): a program whose recording —
every call the backend is told, compared byte for byte — and backend
library are unchanged gets its stored verdict, streams and `out/` files
back without starting a backend. Crashes are never stored, save a run
stopped over the gates' own budget, which they grade a crash either way.
`gates.sh` re-runs one hit in 20 against the real backend
(`KG_BACKEND_CACHE_VERIFY`; `1` re-runs every hit) and a difference fails
the gate: the cache's premise, a deterministic backend, is re-proven rather
than assumed. `KG_BACKEND_CACHE=` (empty) turns it off. The replay code is
in the key by its meaningful text: editing a comment or a test in
`kg-ptbridge`'s replay files keeps the cache; editing code discards it
(~3 GB), and the next `backend` and `output` runs are cold.

## The reference-tree guard

Every xtask command that reads the reference first checks the pin
(`xtask/src/guard.rs`): `HEAD` is `reference.lock`'s commit, nothing under
`library/` or `test/` differs from it or is untracked, and every file there
hashes to the lock's `inputs`. Every such command is also bracketed by a
listing of those two directories, and a path that appeared during the run
is named and fails it. A gate that spawns the reference compiler leaves
`<base>PathLength.pdrpt` in its working directory, and 163 such reports
once reached the reference's `test/` before the gates' working directories
moved under `target/`
([journal](journal.md#two-gates-wrote-into-the-reference-and-a-root-read-twice)).
The guard never deletes what it finds: a file that should not be there is
evidence, and the gate that made it is the thing to fix.

## Memory

Every gate runs under a budget (`xtask/src/bounded.rs`), since gates and
side runs together ran a 64 GB machine out of memory on 2026-09-23
([journal](journal.md#the-machine-ran-out-of-memory--2026-09-23-night)):

- each compiler, oracle or runner a gate spawns runs in a process group of
  its own, backend worker included, killed past 8 GB or 900 s
  (`KG_CHILD_MEM_MB`, `KG_CHILD_SECS`) and graded as that program's crash;
- each xtask process stops itself past 16 GB (`KG_XTASK_MEM_MB`; exit 99);
- `gates.sh` kills its whole run below 10% free memory system-wide
  (`KG_GATES_MIN_FREE_PCT`), naming the largest processes first.

A green full run peaks near 17 GB, `ptdiff` 7.5 GB of it. So one heavy run
at a time, machine-wide: the walk and loop tiers, the full gates,
`identity.sh` and `build-backend.sh` wait for a lock in the primary
checkout's `target/` (`scripts/lib.sh`), because they peak high and share
the work directories under `target/ptrace/work` and `target/output/work`.
Never start a second heavy job — another gate, a `backend` run, a batch of
compiles — beside one. Measure one program with `/usr/bin/time -l`, which
reports its peak memory too; per use case, `cargo xtask mem`.

## A gate is a driver over threads

…and the driver is not the grade. Every corpus-walking gate —
`conformance`, `errors`, `ir`, `repoint`, `resilience`, and the corpus
walks in `kg-db/tests/query.rs` — hands its files or snippets to threads
that each own a `Db`, writes each result into a slot indexed by its
position, and builds the report from the slots in corpus order. The grading
functions (`check_snippet`, `score`, `judge`, the ledgers) are called
exactly as the one-`Db` loop called them, and every report was diffed byte
for byte against the sequential one before the change was kept
(2026-09-17). Keep it that way:

- **A new gate, or a new stage in one, is written this shape from the
  start.** One `Db` walking the corpus a file at a time is how five drivers
  came to cost nine minutes between them.
- **Results go in indexed slots, never a shared `Vec` pushed from
  threads.** The report's order is the corpus's, and a `take(5)` of
  failures names the same five on every run. Where a per-thread accumulator
  is unavoidable (`ir`'s `CountGrades`), the merge is a fold with no order
  in it and every failure list is stably sorted afterwards.
- **`cargo xtask conformance --jobs 1` is the one-`Db` loop in corpus
  order, and the reading to fall back to.** A verdict that differs between `--jobs 1` and the default
  depends on what the `Db` checked before it — a bug in a memo, never in
  the fan-out, and worth a journal entry before it is fixed.
- **A `Db` per thread, not one shared.** `Db` is `Sync` and the shared form
  works (the `query.rs` walks use it), but it scaled 2.5× on eight threads
  where a `Db` per thread scaled 7× — the bounded root-set memos under
  interleaved roots are the likely reason, unmeasured.

## Grading a rewrite against the reference

What porting pass 8's rewrites taught about gates; it holds for any check
specified before the code it checks.

### Specifications are hypotheses

A specification is a hypothesis about what a check will catch; only
running it says. Pass 8's provenance clause four was specified as "forbid
an `Own` node the lowering did not create", and as written it rejects the
first file it sees — an append-only arena cannot change a node in place,
so every legitimate replacement is a fresh index. Written to bite, it reads
"an appended `Own` node must not be structurally equal to a lowered node at
its own key". Two of the three gates specified for the pass-8 IR did not
pin what they claimed: the round trip cancels any deterministic lowering
bug, and the provenance invariant was vacuous against the very bug the
design named as its red test. Both were found by running the falsification,
not by reading it. Writing "break X and watch it go red" into a design is
worth little; running it is the whole value — the rule behind `mutant.sh`,
extended to gates
([journal](journal.md#two-gates-that-did-not-pin-what-they-were-specified-to-pin)).

### Survey before you schedule

A TODO, a plan's line or an earlier claim is a hypothesis too. The
constexpr-fold item said "the corpus pins all six sites" and named a file
and line for each. The citations were real; every one was an
already-passing accept assertion that pinned nothing, and one of the six
sites would have invented errors if converted. Read the reference and the
corpus yourself before doing work a note asks for — especially a note that
sounds well-evidenced
([journal](journal.md#six-fold-sites-zero-assertions-and-a-note-that-lied)).

### Src donors need a unit test

Every clause of `xtask ir`'s provenance gate but one fires on `written ==
Own`. The exception — "this `Src::head` names a live CST node" — fires on
every reachable node, so a `Donated` node's whole obligation is that its
range resolves to *some* CST node, which `Src::donated` makes true by
construction. Swapping two donors in a rewrite leaves the gate
byte-identical; so does hanging a synthesized type on the parameter's
*name* token. Measured, not argued. A corpus-wide equality gate cannot
replace it: our spans and the reference's are different objects and agree
for only 93.4% of nodes (divergence #25). So each rewrite lands with donor
assertions read off `kg-oracle --notes --roots-only --passes 8
--template-passes N --template-iterations 0`, verified by swapping two
donors and watching the test go red; `the_two_donors_inside_one_synthesized_type_param_differ`
in `kg-hir/src/rewrite/abbreviated.rs` is the model
([journal](journal.md#the-donor-gate-that-cannot-exist-and-the-66-that-says-why)).

### Know what the grade is blind to

A `rewrite:` line is a self-report: it says a pass ran and left the tree
sound. Grading a rewrite against the reference itself took two instruments,
and how they had to be built is the reusable part:

- a **constructor count** at a pass boundary — one the pass alone mints,
  guarded by one no pass in the window touches (`capture diff`: `ThisF`
  guarded by `LambdaF` at `--passes 3`). Cheap, and it works only while
  nothing unported runs inside the window, which was luck;
- a **per-item ledger** of fields each derived from one source node, so a
  foreign item is simply an entry dropped by name (`lambda diff`). It
  survives what the count cannot: `desugarLambdas`' unported sub-pass
  `addLambdaWrappers` mints the same constructors, so the reference's
  counts are unreachable by construction. Pair by adjacency, not by name;
  synthesized names recur.

Whatever the instrument:

- **Write down what it cannot see, having tried it.** `lambda diff` is
  blind to the position of a synthesized parameter (two unit tests catch
  that instead), and every `Donated` node's provenance is invisible to
  `xtask ir`. A gate whose blind spots are unstated reads as total coverage.
- **Check that two green numbers are independent evidence.** `capture diff`
  and `lambda diff` move to the same 298/351 under the same plant.
- **Grade the whole corpus, not the convenient part.** Both lines graded
  `library/` and printed 122/122 while covering 9% of the corpus's lambdas;
  extending them to `test/` — one call — found a defect in the gate on its
  first run.
- **When an unported pass forces a relaxed comparison, find the quantity it
  conserves**: a relaxation with nothing holding it down is
  indistinguishable from deleting the check. Here, a written lambda either
  becomes a ledger entry or survives, never both and never neither — which
  stops the relaxed path admitting a port that rewrites nothing.

The journal has both stories:
["`desugarLambdas`, and a gate technique that did not
generalise"](journal.md#desugarlambdas-and-a-gate-technique-that-did-not-generalise)
and ["`captureThis`, and the first rewrite the reference
grades"](journal.md#capturethis-and-the-first-rewrite-the-reference-grades).
