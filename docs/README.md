# Documentation map

Every document in the repository, what it is for, and whether it is
**current** (kept true), **generated** (rendered by a command), a
**record** (appended, never rewritten) or **history** (superseded, kept for
the why).

## Use kgrust

| Document | Kind | Read it for |
|---|---|---|
| [`../README.md`](../README.md) | current | what kgrust is, installing, the quick start, the editor, building from source |
| [`../editors/vscode/README.md`](../editors/vscode/README.md) | current | the VS Code extension and what the language server provides |
| [`known-divergences.md`](known-divergences.md) | current | every deliberate difference from the reference compiler, each with what removing it would cost |
| [`editions/`](editions/README.md) | current | the language editions: `1.0` is the reference, later ones opt-in per package; each edition's page lists everything that differs from the one before |
| [`spec/cli.md`](spec/cli.md) §9–10 | current, with an as-built box | the command line and `kanagawa.toml` |
| [`../CHANGELOG.md`](../CHANGELOG.md) | record | what changed, release by release |

## Understand it

| Document | Kind | Read it for |
|---|---|---|
| [`status.md`](status.md) | generated headline + current prose | where things stand: every gate's number, what each frontend pass runs on, the tools |
| [`roadmap.md`](roadmap.md) | current | where it stands, what is next, the open questions and the decisions that still bind |
| [`architecture.md`](architecture.md) | current | the crate graph, the pipeline, the load-bearing decisions, how the backend is built |
| [`plans/performance.md`](plans/performance.md) | current (§11–13) | speed and memory against the reference, and a whole build by phase and round |
| [`plans/backend-survey.md`](plans/backend-survey.md) | survey | the C++ code generator: where a build's time goes, and the options |

## Contribute

| Document | Kind | Read it for |
|---|---|---|
| [`../CONTRIBUTING.md`](../CONTRIBUTING.md) | current | setting up, the pull-request flow, reporting a divergence |
| [`../AGENTS.md`](../AGENTS.md) | current | the rules of working here, for people and coding agents alike — correctness first, exact optimisations, fail toward silence, the testing loop, house style. Binding |
| [`gates.md`](gates.md) | current | every gate: what it grades, what it is blind to, how it runs |
| [`plans/README.md`](plans/README.md) | current | the live plans and surveys, each with its status |
| [`spec/`](spec/) | reference | ~19,000 lines of the reference compiler's behaviour, every claim cited `file:line` (each spec's status line says which upstream commit it was read at): [`lexer`](spec/lexer.md), [`grammar`](spec/grammar.md), [`types`](spec/types.md), [`resolve`](spec/resolve.md), [`frontend-passes`](spec/frontend-passes.md), [`abi`](spec/abi.md) (the backend's C ABI), [`cli`](spec/cli.md), [`diagnostics`](spec/diagnostics.md), [`conformance`](spec/conformance.md) (the test oracle), [`style`](spec/style.md) and [`style-measured`](spec/style-measured.md) (for `kg fmt`) |
| [`../oracle/README.md`](../oracle/README.md) | current | `kg-oracle`, the reference's own frontend built as a library, which several gates grade against |
| [`../grammar/kanagawa.ungram`](../grammar/kanagawa.ungram) | current | the declarative grammar, input to `cargo xtask codegen` |

## Records

| Document | Kind | Read it for |
|---|---|---|
| [`journal.md`](journal.md) | record | decisions, findings and dead ends a `git log` cannot carry; its index at the top names the decisions that still bind. Read the relevant entry before changing anything that looks arbitrary |
| [`measurements/`](measurements/2026-10-11/README.md) | data | raw output behind the performance tables, one folder per date, each file labelled with its commit, command and load |
| [`history/`](history/README.md) | history | superseded documents — the parity sprint's plan, the IR design, the 2026-10-11 handoff, the 2026-09-22 audit, earlier status pages — each naming what replaced it |
| [`patches/`](patches/README.md) | history | work measured and deliberately not landed |

## About the record

kgrust was developed privately from 2026-08-06 and published on 2026-10-11
as a single commit; most of it was written by AI coding agents working
under [`AGENTS.md`](../AGENTS.md) for the maintainer. The records above
were written during that time, and read accordingly:

- **Commit hashes** in documents dated before 2026-10-11 — and "`main`"
  in them — name the pre-publication history, which is not published.
  `98a2d8c` is different: it is the upstream Kanagawa commit the
  `third_party/kanagawa` submodule is pinned at.
- **Paths:** `../kanagawa` and `../kanagawa-build`, before 2026-10-11, are
  today's `third_party/kanagawa` and `target/reference`; `CLAUDE.md`'s rules
  are today's [`AGENTS.md`](../AGENTS.md), and `docs/handoff.md` and
  `docs/plans/sprint-parity.md` are in [`history/`](history/README.md).
- **Words:** "the owner" is the maintainer; "the lead", "an agent" and "a
  session" are the agent workflow that wrote the docs. `W0`–`W8` are the
  parity sprint's workstreams and `S1`–`S6` its milestones
  ([`history/sprint-parity.md`](history/sprint-parity.md)); `D1`, `M5`,
  `§n` and the like are items of the plan a passage cites.
- **Numbers** carry their date and the command that produced them; a later
  measurement supersedes an earlier one, and [`status.md`](status.md)'s
  headline is the current one.

## Keeping this map true

- **A number is rendered, never typed.** `status.md`'s headline comes from
  `cargo xtask status-md`, from a complete green gate run; anywhere else,
  quote a number with its date and the command that produced it.
- **A new plan** goes in `plans/`, gets a row in [`plans/README.md`](plans/README.md),
  and — if it is what comes next — is named by the roadmap.
- **A finished or superseded document** gets a status line at its top
  naming what replaced it. If it would mislead a reader looking for the
  current state, move it to `history/`, add a row there, and update what
  cites it (`grep` the tree — code comments cite docs by path).
- **After moving or renaming a doc**, run `scripts/check-links.py`.
