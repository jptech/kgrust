# Plans

*Part of the [documentation map](../README.md).*

The live plans and surveys, one line each: what it is, and where it stands.
What comes next across all of them is [`../roadmap.md`](../roadmap.md); a
finished plan moves to [`../history/`](../history/README.md) with a status
line naming what replaced it.

| Plan | What | Status |
|---|---|---|
| [`loop-across-edits.md`](loop-across-edits.md) | The editor's keystroke: what an edit may keep of the program-wide loop (design D1), and its ceiling and levers (§8) | **active**: steps 1–3 built and exact (2026-10-10/11); step 4 (symbol maps per module) planned; the re-askable-reads design to be written |
| [`performance.md`](performance.md) | Speed and memory: incremental inference, the backend cache, copying less; the standing against the reference (§11), memory per use case (§12), a whole build by phase and round with the optimisations triaged (§13) | steps done 2026-09-27; §11–§13 current (2026-10-11) |
| [`backend-survey.md`](backend-survey.md) | The C++ code generator: where a whole build's time goes (92–98% of it), what the audits found, the options from no code to a Rust rewrite | **survey** 2026-10-11, read and measured; which options to take is open ([`../roadmap.md`](../roadmap.md)) |
| [`developer-tooling.md`](developer-tooling.md) | The developer tools: what exists and its state, the gaps grouped by whom they serve, a suggested order | **survey** 2026-10-11; the release archive, documentation in hover and `kg doc` since built |
| [`single-source-of-truth.md`](single-source-of-truth.md) | One owner per language construct; its register of the deliberate second implementations, each with the gate that compares the two | done 2026-10-10; the register is maintained; moving the parser's template checks onto the sorted lowering is open |

Finished plans, in [`../history/`](../history/README.md): the parity sprint
([`sprint-parity.md`](../history/sprint-parity.md)), the program-wide loop's
design ([`ir-elaboration.md`](../history/ir-elaboration.md)), the backend
bridge ([`ParseTree_Integration_And_Test.md`](../history/ParseTree_Integration_And_Test.md)),
the gap list ([`gaps.md`](../history/gaps.md)) and the first speed
comparison ([`compile-speed-2026-09-17.md`](../history/compile-speed-2026-09-17.md)).
