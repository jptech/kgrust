# Working on kgrust

The rules for anyone changing this repository, agent or human;
[`CONTRIBUTING.md`](CONTRIBUTING.md) summarises them for people.

## What this is

A Rust rewrite of the Kanagawa HLS compiler's frontend and its tools,
graded against the reference: upstream Kanagawa, the read-only submodule
`third_party/kanagawa/` pinned by `reference.lock` (`98a2d8c`) — Haskell
sources in `compiler/hs/lib/Language/Kanagawa/`, the corpus, the C++
backend. Overrides: `KANAGAWA_ROOT` (the reference), `KANAGAWA_BUILD` (its
build; default `target/reference`), `KG_BACKEND_DIR` (the backend library;
default `target/backend/dist/bin`).

## Non-negotiables

- **The reference is read-only.** Never write, edit, move or delete
  anything under `third_party/kanagawa/`, cleanup included, and never run a
  compiler or build with its working directory inside it (the build scripts
  copy what they compile out). Every gate fails a run that left a new file
  in the reference's corpus; never delete such a file — it is evidence, and
  the gate that wrote it is the bug. Move the pin only by the checklist below.
- **The scoreboard is off-limits.** `xtask/src/conformance.rs`,
  `xtask/src/corpus.rs` and `FRONTEND_COMPLETE` (`true` since 2026-10-10,
  the maintainer's decision): code being graded does not edit its grade.
- **Never weaken a test.** A test that encodes a wrong expectation may be
  corrected, citing the reference — never deleted or loosened to go green.
- **Prove every new test load-bearing**: `scripts/mutant.sh <file> <old>
  <new> -- <check>` plants the mutation, requires the check to go red and
  restores the file. Spell the check out after `--` (in a zsh variable it
  never runs, and reads as caught); confirm the red is the test's own.
- **Fail toward silence.** Where you cannot be exact, miss a diagnostic
  rather than invent one: a missed one fails one `expected:1` assertion; an
  invented one breaks an `expected:0` assertion — perhaps one of the 241 the
  reference leaves to the backend — and is a lie about a correct program.
- **An optimisation or a refactor lands exact or not at all**:
  byte-identical results, every ratchet neutral, the old path behind a
  switch until the identity check agrees everywhere — `cargo xtask
  infer-identity`, `scripts/identity.sh [<commit>]`, and `KG_INFER_CHECK=1`
  to find the first node that parts.
- **Ratchets are per item**: "9 up, 2 down" is red. Lock an improvement in
  with `cargo xtask ratchet --bless <gate>`, in the same commit as the
  change, and only after reading the baseline's diff — the same command
  accepts a regression, and the diff names every item given up. Bless in the
  primary checkout, never a linked worktree; from the ledgers of unfiltered
  runs on this exact tree; never from a run under a verdict switch
  (`KG_LEFTOVERS`, `KG_INFER_FULL`, `KG_PTBRIDGE_SEED`). Render
  `docs/status.md` (`cargo xtask status-md`) only after a complete green
  `scripts/gates.sh`: `cargo xtask ratchet <gate>…` — the read before a
  bless, the one `fast.sh` ends with — rewrites `target/gates/status.json`
  with the named gates alone.
- **One heavy run at a time, machine-wide.** The full gates peak near
  17 GB; they, the walk and loop tiers, `identity.sh` and the backend build
  queue on a lock in the primary checkout's `target/`. Start nothing heavy —
  a gate, a `backend` run, a batch of compiles — beside one.
- **Never edit the tree a gate is grading** — a long single gate or a
  running `mutant.sh` included. To keep editing during the full gates, run
  `scripts/gates.sh --snapshot`, which grades a committed tree elsewhere.
- **No bare `git stash`**: every worktree shares the stash. Use a WIP
  commit, or `git stash push -m <unique-tag>` and apply by its SHA.
- **Format by hand.** `cargo fmt` is disabled in `rustfmt.toml`; read why
  before proposing otherwise. Match the surrounding style, and never let a
  reformat ride inside a behaviour change.
- **Cite the reference as `File.hs:line`** for every rule ported. Comments
  say *why* — above all where the port diverges, which also gets an entry
  in `docs/known-divergences.md` — never what the code does.
- **Nothing ships without a caller.** A feature is done when something a
  user types reaches it: run the binary, speak to the language server over
  stdio. Tests do not count.
- **Performance is a goal; correctness is never its price.** Benchmark a
  new pass, not just its correctness (`cargo xtask bench`); measure before
  and after, optimise measured hotspots ([`performance.md`](docs/plans/performance.md)),
  and trust only interleaved A/B runs on a quiet machine.
- **Never commit** absolute local paths, usernames, or anything from `target/`.

Conventions: no doc tests (`doctest = false`; an example is a unit test); a
new crate joins `Cargo.toml`'s `default-members` by hand, or `cargo test`
skips it.

## Setup

**The light path**: `cargo build` and `cargo test --profile quick` (never
the dev profile: 34 s against 818 s) need only Rust (`rust-toolchain.toml`
pins 1.92.0) and a C compiler, and give `kg check`, `fmt`, `lsp` and `doc`.
A source build bundles no standard library: pass `-I
third_party/kanagawa/library` or name it in a `kanagawa.toml`. The submodule
(`git submodule update --init third_party/kanagawa`) holds that library, the
corpus (a test that finds none skips itself, except under the scripts, which
set `KG_REQUIRE_CORPUS=1`) and the backend's sources.

**The full gates, from a fresh clone**, in order:

1. `git submodule update --init third_party/kanagawa`.
2. `scripts/build-backend.sh --reference`: LLVM, MLIR, CIRCT and the
   backend (~28–30 min on 16 cores from scratch, a ~6 GB tree), then the
   reference's own compiler into `target/reference` (GHC 9.6.7 and cabal,
   via ghcup).
3. `oracle/build.sh`: `kg-oracle`, the reference frontend as a library
   ([`oracle/README.md`](oracle/README.md)); `errors`, `ir` and `loop-left`
   need it.
4. `cargo xtask ptrace`: records the reference's runs (~17.5 min). Its keys
   carry absolute paths, so a moved checkout re-records. `risc-v.k` needs a
   larger budget: `KG_CHILD_SECS=4500 KG_CHILD_MEM_MB=20000 cargo xtask
   ptrace --filter processor/risc-v.k`.
5. `scripts/gates.sh > target/gates.log 2>&1`; the first run is cold.

Prerequisites: zsh (every script); python3 (`repoint`, `cli`, `mutant.sh`,
`check-links.py`); libclang (`codegen --check`, `kg-ptrace`, so `cargo test
--workspace`); CMake ≥ 3.30, Ninja, a C++17 compiler and Boost ≥ 1.88;
≥ 32 GB RAM for the full gates, 64 GB recommended; ~25 GB of disk for the
backend and reference builds. macOS is the tested platform; Linux is untested.

## Workflow

1. **Branch** off `main`, named for the work (`doc-tests`, `worklist-order`).
2. **Survey, then change.** Read the reference clause and the corpus
   yourself and reproduce the gap (`scripts/probe.sh <program>`,
   `kg-oracle`); cite `File.hs:line`; prove each new test with `mutant.sh`.
3. **Run the smallest tier that can see the change** (below), and `cargo
   test --workspace --profile quick` before every commit.
4. **Commit per item**, the gate evidence in the message — which gate moved,
   from what to what — and any bless in the same commit.
5. **Record it**: a progress line in the relevant plan; a dated journal
   entry at the end of [`docs/journal.md`](docs/journal.md) for anything a
   later reader would otherwise rediscover, with an index row if it binds.
   Every number carries its date and the command that produced it.
6. **Merge**: `scripts/gates.sh --snapshot > target/snapshot-gates.log
   2>&1` green on the branch head, then one squash commit on `main` (`git
   merge --squash <branch>`, a message summarising the branch and its gate
   evidence), and delete the branch.

Never commit to `main` directly, and never force-push or rewrite it. Never
push the local `dev` branch (the pre-publication history, which older commit
hashes in the docs name) or a `worktree-agent-*` branch. CI
([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs the light
tier on macOS — build, clippy, `cargo test --profile quick`, `conformance`,
`corpus`, `fmt-check`, `lint`, `check-links.py`; the parity gates stay
local. After moving or renaming a doc, run `python3 scripts/check-links.py`;
the documentation's own rules close [`docs/README.md`](docs/README.md).

**Worktrees.** `scripts/worktree.sh add <path>`, or `scripts/worktree.sh
link` inside an existing one, links the gates' data stores from the primary
checkout; the scripts find the reference from any worktree. A worktree
nested inside the primary checkout needs `CARGO_WORKSPACE_DIR=$PWD` with
`UPDATE_EXPECT=1`, or expect-test rewrites the primary checkout's files.

## The loop, fastest first

| Command | Runs | Time |
|---|---|---|
| `scripts/probe.sh <program>` | every verdict for one program: conformance and `--strict`, `errors`, `ptdiff`'s first divergence, both compilers through the backend, `output` for a corpus file | ~10 s |
| `scripts/quick.sh <filter>` | one test filter over kg-db, kg-ty, kg-hir, kg-sem and kg-ptbridge, then `conformance` and `errors` | ~30 s |
| `scripts/fast.sh` | `conformance`, `errors`, `strict-ledger`, `lint`, `cli`, then their ratchets | ~35 s |
| `scripts/fast.sh walk` | … and `ptdiff`, `backend` (replayed from the cache) | ~80 s |
| `scripts/fast.sh loop` | … and `infer-identity`, `loop-left`, `resilience`, every fourth root of `edit-identity` — any change to inference, the loop, a memo or the editor's answers | ~4 min |
| `scripts/gates.sh [--snapshot]` | everything ([`docs/gates.md`](docs/gates.md)) — before a merge, a deletion, a bless of `output` or `repoint` | ~6 min warm; 15–30 min cold |

`--lint` adds clippy to any `fast.sh` tier. Converge with `probe.sh` in a
loop, not a gate per question; a change to what the loop leaves standing
needs the whole `edit-identity`, not `fast.sh loop`'s quarter. Exactness:
`cargo xtask infer-identity [--dump DIR]` (both inference modes, every
fixpoint's trajectory), `scripts/identity.sh [<commit>]` (every root's
fingerprint against `<commit>`'s build), `KG_INFER_CHECK=1`. Time one
program with `/usr/bin/time -l`, which also reports its peak memory.

## Gates at a glance

Details, blind spots and drivers: [`docs/gates.md`](docs/gates.md).
"Ratchet" means graded per item against `baseline.json`; "backend" means in
`gates.sh` but left out by `--no-backend`.

| Gate | Grades | Verdict | In `gates.sh` |
|---|---|---|---|
| tests, clippy, `codegen --check` | the workspace, `--profile quick`; `-D warnings`; generated files current | pass/fail | yes |
| `corpus`, `fmt-check` | every corpus file round-trips and parses clean; the formatter is idempotent and layout-only | pass/fail | yes |
| `conformance` | accept/reject and `Error N` per assertion; `--strict` grades the same, and must fail nothing | 100%; `strict` ratchet | yes |
| `errors` | first error's position and message against `kg-oracle --errors` | ratchet | yes |
| `ir` | lowering totality, round trip, provenance, the oracle diff | pass/fail | yes |
| `lint` | retiring spellings in production code | ratchet | yes |
| `resilience` | hover and completion on broken buffers | ratchet | yes |
| `ptdiff` | our backend walk against the reference's recorded one | ratchet | backend |
| `backend` | the compat binary through the real backend | ratchet | backend |
| `repoint`, `cli` | upstream's syntax runner and CLI tests against our binary | ratchet | backend |
| `loop-left` | what the loop leaves undecided, against the reference's | ratchet | backend |
| `edit-identity`, `shadow` | a warm `Db` after each edit against a fresh one; the editor's kept loop (`edit-identity` writes `shadow`) | pass/fail; ratchet | backend |
| `output` | every RTL file, modulo source locations | ratchet | backend |
| `bench`, `mem` | timings (never fails); peak memory per use case | tripwire; budget | yes (`--no-bench` drops) |
| `infer-identity` | incremental inference against the whole walk | pass/fail | no: `fast.sh loop` |

## Checklists

**Moving the reference pin.** Bump the submodule to the new upstream
commit; `cargo xtask pin-reference --write`; `cargo xtask ratchet --rebase
<old commit>` (assertion names are positional — read what it cannot map);
`cargo xtask codegen` if the backend's headers changed; rebuild the
reference and the oracle (`scripts/build-backend.sh --reference`,
`oracle/build.sh`); `cargo xtask ptrace --force`. `output`'s stored side and
the backend cache go cold on their own. Then the full gates, blessing what
moved in the commit that moves the pin.

**Releasing.** Set the version in `Cargo.toml`'s `[workspace.package]`; full
gates green on `main`; `scripts/build-backend.sh`; `cargo xtask dist`;
smoke-test the unpacked archive outside the checkout, nothing configured
(`kg check`, `kg build`, `kg doc` on a small design); add the
[`CHANGELOG.md`](CHANGELOG.md) entry; tag `vX.Y.Z` on `main`; attach each
platform's archive and `.sha256` to the release.

## Lessons

- **A gate's specification is a hypothesis**; only running its
  falsification says what it catches ([gates](docs/gates.md#specifications-are-hypotheses)).
- **Survey before you schedule**: a TODO is a claim — "the corpus pins all
  six sites" pinned none ([journal](docs/journal.md#six-fold-sites-zero-assertions-and-a-note-that-lied)).
- **Grade a rewrite against the reference**, over the whole corpus, and
  write down what the grade is blind to ([gates](docs/gates.md#grading-a-rewrite-against-the-reference)).
- **A rewrite's `Src` donors need a unit test** read off `kg-oracle
  --notes`; no gate sees a swapped donor ([gates](docs/gates.md#src-donors-need-a-unit-test)).
- **Correct is not usable**: a pass that passed every gate made one check
  20× slower ([journal](docs/journal.md#right-and-unusable)).
- **The recurring failure**: four modules shipped tested and with no caller
  — `WarningSet::filter`, `Db::check`, `Db::diagnostics`, the `kg-fmt`
  integration, while the editor greyed out "Format Document" for a working
  formatter ([journal](docs/journal.md#the-fourth-crate-with-no-caller)).

## Where to read

[`docs/README.md`](docs/README.md) maps every document. Start from
[`docs/roadmap.md`](docs/roadmap.md): where it stands, what is next (take it
in that order unless the maintainer says otherwise), the open questions,
the decisions that bind. [`docs/status.md`](docs/status.md)
has every gate's number, generated; [`docs/architecture.md`](docs/architecture.md)
the crates and the pipeline; [`docs/known-divergences.md`](docs/known-divergences.md)
every deliberate difference; [`docs/spec/`](docs/spec/) the reference's
behaviour, cited. Read [`docs/journal.md`](docs/journal.md) before changing
anything that looks arbitrary; the reason is usually written down there.
