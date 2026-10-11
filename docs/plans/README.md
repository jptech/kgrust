# Plans

*Part of the [documentation map](../README.md).*

One line per plan: what it is, and where it stands. A plan that is done or
superseded stays — it is the record of why — with a status line at its top
pointing at what replaced it. New work goes in the plan the roadmap's queue
names, not in a finished one.

| Plan | What | Status |
|---|---|---|
| [`sprint-parity.md`](sprint-parity.md) | The current sprint: template elaboration parity, the remaining gaps, M6, consolidation, performance, the query layer (W7) and what the tools stand on (W8) | **active** (2026-09-27, revised the same day after the foundation survey) — the roadmap's queue. S1–S4 reached; the parity workstreams (W1, W2) and W8 are done (2026-10-10), the corpus at parity on every gate; W3 (one engine, M6) done 2026-10-10; open: W5, W7.5d–W7.6 |
| [`ir-elaboration.md`](ir-elaboration.md) | Pass 8 without the text round trip: IR inference, one program-wide loop, passes on the IR | M0–M4 and M6 done (M6 2026-10-10, sprint W3.7), M5 mostly; the rest in the sprint (W1.1, W5) |
| [`performance.md`](performance.md) | Incremental inference, the backend cache, copying less; the standing against the reference (§11) | mostly done 2026-09-27; the rest in the sprint (W0.2, W5) |
| [`developer-tooling.md`](developer-tooling.md) | The developer tools: what exists (CLI, formatter, language server, extension, manifest, explain, dump) and its state, the gaps grouped by whom they serve, and a suggested order | **survey** 2026-10-11; nothing decided |
| [`backend-survey.md`](backend-survey.md) | The C++ backend: where a whole build's time goes (92–98% of it), what the audits found, the options from no code to a Rust rewrite, and what each would take | **survey** 2026-10-11, read and measured only; nothing decided (§6 the owner's) |
| [`loop-across-edits.md`](loop-across-edits.md) | Sprint W7.5d: what an edit to the root may keep of the program-wide loop — the dependency stated, three designs, one measurement first | **reviewed** (2026-10-10); §5's measurement run the same day: D1 is to be built, positions relocated (§5, §6); step 1 built the same day — the snapshot and its shadow, no reuse (§6); step 2 the same day — the read door, what each module's round read across its boundary keyed beside its copies, no round of the shadow left unequal (§6); step 3's first pass 2026-10-11 — the prefix reused, exact, no keystroke won; step 4 planned (§6) |
| [`single-source-of-truth.md`](single-source-of-truth.md) | One owner per language construct (D1 consolidation, D2 drift fixes) | done 2026-10-10 (sprint W4): every copy consolidated or in the register; the parser's template checks onto the sorted lowering open |

Superseded or done, outside this directory: [`../gaps.md`](../history/gaps.md)
(superseded by the sprint's §4), [`../ParseTree_Integration_And_Test.md`](../history/ParseTree_Integration_And_Test.md)
(done), [`../compile-speed-2026-09-17.md`](../history/compile-speed-2026-09-17.md)
(superseded by `performance.md` §11), [`../history/audit-2026-09-22.md`](../history/audit-2026-09-22.md)
(a dated assessment; its §6.4 is done).
