# Documentation map

Every document in the repository, what it is for, and whether it is
**current** (kept true), **generated** (rendered by a command), a
**record** (appended, never rewritten) or **history** (superseded, kept for
the why). Read top to bottom the first time; after that, go straight to the
row you need.

## Start here

| Document | Kind | Read it for |
|---|---|---|
| [`../CLAUDE.md`](../CLAUDE.md) | current | the rules of working here — correctness first, exact optimisations, fail toward silence, the testing loop, house style. Binding |
| [`handoff.md`](handoff.md) | current | where the last session paused: what landed, performance and quality in one place, how to resume, the options from here and the owner's open decisions |
| [`status.md`](status.md) | generated headline + current prose | where things stand: every gate's number, what each frontend pass runs on and its state, the tools |
| [`roadmap.md`](roadmap.md) | current | what is next and the decisions that still bind; its queue names the active plan |
| [`plans/sprint-parity.md`](plans/sprint-parity.md) | current | **the active plan**: workstreams W0–W8, milestones S1–S6, targets, the gaps partitioned (§4), what yields if it runs long (§7) and what the owner decides (§8) |
| [`architecture.md`](architecture.md) | current | the crate graph, the pipeline, the three load-bearing decisions, [every gate and what it isolates](architecture.md#the-gates) |

## Plans

[`plans/README.md`](plans/README.md) indexes them, each with its status:
the active sprint, and the three plans it folds in —
[`ir-elaboration.md`](plans/ir-elaboration.md) (the IR and the program-wide
loop; M0–M4 done), [`performance.md`](plans/performance.md) (mostly done;
§11 is the standing against the reference) and
[`single-source-of-truth.md`](plans/single-source-of-truth.md) (one owner
per language construct; three of fourteen items done).

## Reference

| Document | Kind | Read it for |
|---|---|---|
| [`spec/`](spec/) | current | ~19,000 lines of the reference compiler's behaviour, every claim cited `file:line`: [`lexer`](spec/lexer.md), [`grammar`](spec/grammar.md), [`types`](spec/types.md), [`resolve`](spec/resolve.md) (modules, imports, scopes), [`frontend-passes`](spec/frontend-passes.md), [`abi`](spec/abi.md) (the backend's C ABI), [`cli`](spec/cli.md), [`diagnostics`](spec/diagnostics.md), [`conformance`](spec/conformance.md) (the test oracle), [`style`](spec/style.md) and [`style-measured`](spec/style-measured.md) (for `kg fmt`) |
| [`known-divergences.md`](known-divergences.md) | current | every deliberate difference from the reference, each with what removing it would cost |
| [`../oracle/README.md`](../oracle/README.md) | current | `kg-oracle`, the reference's own frontend built as a library: `--errors`, `--tree`, `--typed`, `--passes N` |
| [`../grammar/kanagawa.ungram`](../grammar/kanagawa.ungram) | current | the declarative grammar, input to `cargo xtask codegen` |

## Records

| Document | Kind | Read it for |
|---|---|---|
| [`journal.md`](journal.md) | record | decisions, findings and dead ends a `git log` cannot carry; its index at the top names the decisions that still bind. Read the relevant entry before changing anything that looks arbitrary |
| [`history/audit-2026-09-22.md`](history/audit-2026-09-22.md) | record | the 2026-09-22 assessment; what was acted on is in the journal |
| [`measurements/`](measurements/2026-10-11/README.md) | data | raw output behind the performance tables, one folder per date, each file labelled with its commit, command and load (2026-10-11: the reference on one core and every core, a whole build by phase and by round) |
| [`patches/`](patches/README.md) | history | work measured and deliberately not landed; both patches predate the twin's deletion |
| [`history/`](history/README.md) | history | superseded documents, each naming what replaced it |

## Keeping this map true

- **A number is rendered, never typed.** `status.md`'s headline comes from
  `cargo xtask status-md`; anywhere else, quote a number with its date and
  the command that produced it.
- **A new plan** goes in `plans/`, gets a row in [`plans/README.md`](plans/README.md),
  and — if it is the queue — is named by the roadmap.
- **A finished or superseded document** gets a status line at its top
  naming what replaced it. If it would mislead a reader looking for the
  current state, move it to `history/`, add a row there, and update what
  cites it (`grep` the tree — code comments cite docs by path).
- **After moving or renaming a doc**, run `scripts/check-links.py`.
