# Roadmap

Where kgrust stands, what comes next, the questions still open and the
decisions that bind. The numbers are [`status.md`](status.md)'s; the
reasoning behind past choices is in [`journal.md`](journal.md); how the
project got here is in [`history/`](history/README.md) — the parity sprint
([`history/sprint-parity.md`](history/sprint-parity.md)) and the handoff
written when it ended ([`history/handoff-2026-10-11.md`](history/handoff-2026-10-11.md)).

## Where it stands (2026-10-11)

- **The frontend is at parity with the reference** on everything the
  reference's own corpus can grade: every decidable conformance assertion
  agrees (2,681 of 2,681, completeness claimed), upstream's syntax test
  runner passes all 72 files, the first error agrees but for 18 recorded
  parser-recovery positions, and the walk into the C++ backend matches the
  reference's call for call on all 2,422 programs.
- **It compiles through the reference's own code generator** to the same
  RTL, source-location comments aside (269 of 274 programs; the reference
  fails on the other 5), built from the `third_party/kanagawa` submodule
  by `scripts/build-backend.sh`.
- **The tools:** `kg check`, `build`, `fmt`, `lsp`, `doc`, `dump`,
  `explain`, and the argv-compatible `kanagawa`; a release archive from
  `cargo xtask dist`; the VS Code extension in `editors/vscode`.
- **Speed:** `kg check` is 12–46× the reference frontend per core, 8–16×
  against it on 16 cores ([`plans/performance.md`](plans/performance.md)
  §11). A whole build is 92–98% the shared C++ backend
  ([`plans/backend-survey.md`](plans/backend-survey.md)).

## Next

Independent options, roughly in the order recommended.

1. **Parity off the corpus — the fuzz residue.** `cargo xtask fuzz
   --differential` finds what the corpus does not hold; what is left: seed
   7's 41 buckets (13 where ours is silent and the reference is not — a
   stray `;` before `import`, a `void` interpolation in
   `library/debug/print.k`, a static assert over `<<`, names used as types,
   …); seed 20261010's five; divergences #39–#41. Each lands with
   `kg-oracle --errors` evidence and a mutant-proven test; then new seeds
   until a run's silent and false-alarm buckets are empty.
2. **Publishing.** A release per platform (`cargo xtask dist`; Linux
   untested), the VS Code extension on a marketplace (its publisher and
   name are open, below), doc tests for Markdown code blocks like
   upstream's `chkdoc` ([`plans/developer-tooling.md`](plans/developer-tooling.md)).
3. **The loop on more than one core, and its tail rounds.** The loop runs
   on one core where the reference gains 2.2–3.5× from sixteen; `array.k`
   and `float.k` spend half their loop in a tail of cheap rounds that
   rebuild the symbol maps and re-run deduction
   ([`plans/performance.md`](plans/performance.md) §13). Speeds `kg check`
   and the editor alike. Measure the parallel headroom first.
4. **The keystroke.** On the largest project, `risc-v.k`, the editor's
   first answer takes ~0.83 s and its last ~2 s. Carrying the library's
   work across edits takes at most ~40% off; the plan's next step (symbol
   maps per module) and the design after it (re-askable reads) are in
   [`plans/loop-across-edits.md`](plans/loop-across-edits.md) §6, §8.
5. **The backend, if taken up** (open, below): the address-ordered
   liveness work list (which swings `risc-v.k`'s backend from ~4 to 30+
   minutes), a no-op `GlobalDataPropagation`, a whole-run cache in
   `kg build` ([`plans/backend-survey.md`](plans/backend-survey.md) §3).
6. **Smaller:** the editor's rename follow-ups (members through lambdas and
   abbreviated templates); moving the parser's template checks onto the
   sorted lowering (a verdict change, through `errors` and `conformance`);
   the 18 recovery positions where ours would mislead (divergence #30);
   the one-shot memory peak; `kg explain` prose for the backend's codes;
   settings accepted and not yet read (`kg build --emit` and `-Z`, the
   manifest's `[hw]` and `[project] exclude`; [`spec/cli.md`](spec/cli.md)
   §9–10); call hierarchy and code lenses
   ([`plans/developer-tooling.md`](plans/developer-tooling.md)).

## Open questions

Each blocks only its own item; the work assumes the first answer meanwhile.

| Question | Assumed meanwhile | Where it is argued |
|---|---|---|
| The parity bar for generated RTL: identical modulo source locations, or byte-identical | modulo locations | [`history/sprint-parity.md`](history/sprint-parity.md) §8; divergence #25 |
| `basics.k::4,5`: the reference binary (which crashes on them) or the corpus (`expected:0`) is the reference | the binary — the walk declines them | [`history/sprint-parity.md`](history/sprint-parity.md) §8 |
| The conformance gate's `--Wall` reading (fixing it edits the scoreboard) | as it is, recorded in `xtask/src/options_agree.rs` | [`history/sprint-parity.md`](history/sprint-parity.md) §8 |
| Whether the C++ backend is in scope for changes (upstreamed, or a local patch set) | measured only | [`plans/backend-survey.md`](plans/backend-survey.md) §6 |
| Moving the reference pin to upstream's current backend (three commits ahead, one changes the SV) | pinned at `98a2d8c` | [`plans/backend-survey.md`](plans/backend-survey.md) §6 |
| The backend library's channel: `release` (no consistency checks, as upstream ships) or `local` | `release` | [`architecture.md`](architecture.md); [`plans/backend-survey.md`](plans/backend-survey.md) §7 |
| The keystroke's target (proposed: first answer ≤ 400 ms, last ≤ 1 s on `risc-v.k`'s project) | none set | [`plans/loop-across-edits.md`](plans/loop-across-edits.md) §8 |
| Whether the reference's round order binds after parity (it decides whether a demand-driven elaboration, possibly on salsa, is open) | it binds | [`plans/loop-across-edits.md`](plans/loop-across-edits.md) §8 |
| The query layer's substrate: hand-rolled, or salsa | hand-rolled | [`plans/loop-across-edits.md`](plans/loop-across-edits.md) §8 |
| Rename in a project with no manifest: refuse, or scan the declaring file's directories | searches the open buffers' programs | [`history/handoff-2026-10-11.md`](history/handoff-2026-10-11.md) |
| Lints beyond the reference's six warnings (width surprises) | none | [`plans/developer-tooling.md`](plans/developer-tooling.md) §2.5 |
| The VS Code extension's publisher and display name (distinct from upstream's own extension) | unset | [`plans/developer-tooling.md`](plans/developer-tooling.md) §2.1 |
| mimalloc as the binaries' allocator | kept | [`journal.md`](journal.md) |

## Decisions that bind

- **The backend goes through the ParseTree C ABI** (2026-09-06), exactly as
  the Haskell frontend does: `kg build` walks the elaborated program into
  `libkanagawa-backend` through the 85 `extern "C"` functions
  [`spec/abi.md`](spec/abi.md) specifies. Rewriting the C++ middle end is
  not the plan; emitting MLIR directly is not either. Reaffirmed
  2026-10-11: no Rust rewrite of the backend
  ([`plans/backend-survey.md`](plans/backend-survey.md) §3).
- **Developer-experience features are pulled forward** (2026-09-06): after
  each frontend slice, one tooling slice lands, rather than all of them
  waiting.
- **No port of the reference's pretty-printer** (2026-10-09): `kanagawa
  pretty` and `--dump-source` are booked divergences; `kg fmt` is the
  formatter.
- **The editor answers twice** (2026-10-10): the loop's fourth round first,
  then the same run's fixpoint — `kg check`'s diagnostics.
- **Memory for the loop across edits is accepted on the condition of
  efficiency** (2026-10-10), watched per use case by `cargo xtask mem`.
- **The toolchain is pinned** (`rust-toolchain.toml`, 1.92.0).
- **Branches, squash-merged** (2026-10-11): `main` is the published
  history, one commit per piece of work ([`AGENTS.md`](../AGENTS.md)).

The journal's index ([`journal.md`](journal.md)) lists every decision that
binds the code, each with its entry.

## Rules that outrank the order above

1. **`repoint` is the honest measure.** Assertion counts can rise while
   upstream's own runner sits still; when they diverge, believe `repoint`.
2. **Benchmark any new pass.** Pass 4 once landed correct and made a
   keystroke take 1.9 s with every correctness gate green
   ([`AGENTS.md`](../AGENTS.md)).
3. **`--strict` may never rise.** Conformance below 100% is a regression,
   not a trade.
