# Roadmap

Where kgrust stands, what comes next, the questions still open and the
decisions that bind. The numbers are [`status.md`](status.md)'s; the
reasoning behind past choices is in [`journal.md`](journal.md); how the
project got here is in [`history/`](history/README.md) — the parity sprint
([`history/sprint-parity.md`](history/sprint-parity.md)) and the handoff
written when it ended ([`history/handoff-2026-10-11.md`](history/handoff-2026-10-11.md)).

## Where it stands (2026-10-12)

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
  `cargo xtask dist` (and `cargo xtask install`, the same tree linked into
  `~/.cargo/bin`); the VS Code extension in `editors/vscode`, which
  packages (`npm run package`) and installs locally — completion after
  `::` (an enum's constants with their values) its latest feature.
- **Published** at [github.com/jptech/kgrust](https://github.com/jptech/kgrust),
  with CI on every push and pull request (`scripts/ci.sh`: the Rust code
  on Linux, and on macOS for `main`). The backend build and the parity
  gates are tested on macOS only; on Linux the backend's build compiles
  LLVM, MLIR and CIRCT and was not carried further ([`journal.md`](journal.md),
  2026-10-11).
- **Speed:** `kg check` is 12–46× the reference frontend per core, 8–16×
  against it on 16 cores ([`plans/performance.md`](plans/performance.md)
  §11), measured with the `release` build; the release archives, built
  with link-time optimisation, check 6–10% faster again. A whole build is 92–98% the shared C++ backend
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
2. **Publishing.** A release per platform (`cargo xtask dist`; on Linux the
   backend build wants finishing — at fewer jobs than Docker's 8 GB
   allowed — and the archive trying), the VS Code extension on a
   marketplace (it packages already; its publisher and name are open,
   below), doc tests for Markdown code blocks like upstream's `chkdoc`
   ([`plans/developer-tooling.md`](plans/developer-tooling.md)).
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
   abbreviated templates); three found trying the VS Code extension
   (2026-10-11) — hover inside a template defined in another file says
   "Type not yet inferred" (an open file is answered as its own root, and a
   module instantiates nothing; answering through the open files that
   import it would fix it), the outline lists an exported class twice (the
   class and its `export`), and the formatter half-splits a function body
   written on one line; moving the parser's template checks onto the
   sorted lowering (a verdict change, through `errors` and `conformance`);
   the 18 recovery positions where ours would mislead (divergence #30);
   the one-shot memory peak; `kg explain` prose for the backend's codes;
   settings accepted and not yet read (the manifest's `[project] exclude`;
   [`spec/cli.md`](spec/cli.md) §9–10); call hierarchy and code lenses
   ([`plans/developer-tooling.md`](plans/developer-tooling.md)).
7. **Devices and boards: the platform layer.** More devices, boards and
   libraries than upstream ships, in `platform/`, through the seams
   upstream already has: `--target-device` and the backend's device
   configuration. Two pilots (2026-10-17):
   - The tinyVision UPduino v3 (iCE40 UP5K) runs on the open toolchain from
     source to bitstream, with SPRAM reachable from Kanagawa, its examples
     simulated at RTL and gate level, and a design's size halved by fixed
     latency and a FIFO floor of 2.
   - The Numato Mimas A7 (Artix-7 XC7A50T) is built and simulated, but not
     yet through Vivado.

   `kg board` (2026-10-17) reaches them from `kg`, opt-in through a
   manifest's `[board]`: a project from an example, its testbench in
   Verilator, a bitstream, the programmer, the design in simulation behind
   a pseudo-terminal. It works from the release archive with nothing
   configured ([`plans/board-tooling.md`](plans/board-tooling.md) §2).
   Later that day, a design needs no glue RTL: an interface description
   checked against the generated ports, a core generated from the board's
   resources, a host bridge with a generated Python module, cocotb through
   the same API, SPI and I2C engines with a library over them, and a virtual
   board with a front panel in the browser (§4–§9 there).

   Next, in order:
   - the UPduino on a board, and the owner's own UP5K board;
   - the Vivado checklist;
   - what board-tooling §13 leaves open: the UP5K's hard SPI and I2C
     blocks, wider DSP products, the HAL's block-RAM FIFOs using more EBRs
     than the backend budgets.

## Ideas, undecided

A brainstorm (2026-10-29), kept so that it is not lost; nothing here is
scheduled, and each needs the owner's decision before it is. What
[`plans/developer-tooling.md`](plans/developer-tooling.md) already lists —
publishing, `kg test`, watch mode, the whole-run cache, call hierarchy, the
tree-sitter grammar, a playground — is not repeated.

Every idea obeys one rule: **existing code is untouched, new code opts
in.** Without the opt-in a program's verdicts, diagnostics and RTL are the
reference's; `kanagawa` never opts in.

**The mechanism for language changes: editions.** `[package] edition` in
`kanagawa.toml`, reserved and unread today (`kg-config`'s `Package`).
Per package, so a library and its users each keep their own; every
extension desugars inside the frontend to reference Kanagawa, so the
ParseTree walk and the backend never change, and `kg expand` can print the
reference program at any time. Planned in [`plans/editions.md`](plans/editions.md).

*Language, edition-gated, each desugared to reference Kanagawa:*

- **Named arguments**, `f(x: 1, y: 2)`, reordered to positional — the
  smallest, and the first proposed to prove the mechanism.
- **Tagged enums and `match`**: an enum, a union and a struct, and a
  `switch`, with an exhaustiveness check — decoders and instruction sets.
- **Arithmetic that says what it does**: wrapping and saturating operators
  (`+%`, `-|`) lowered to the casts the reference already accepts, for the
  width surprises the README names (`a - b` signed, `<<` widening),
  without changing what `+` and `-` mean.
- **Template requirements** (`requires Numeric<T>`), lowered to
  `static_assert`s, so a template's misuse is reported where it is used.

*Diagnostics:*

- **Why this width**: hover or `kg explain-width` showing the chain of
  inference that made a `uint17`. Frontend-only: the widths are already
  inferred, so it need not wait for the background backend
  [`developer-tooling.md`](plans/developer-tooling.md) §2.7 files it under.
- **Lints of kgrust's own**, in a `kg::` namespace under a `[lints]` table,
  never touching `--Wall` or the reference's six warnings: signedness and
  shift-widening surprises, unused exports and imports, oversized `atomic`
  blocks. The open question below on lints beyond the six decides it.
- **`kg fix`**: lints' machine-applicable fixes, and `--edition` to migrate
  a package between editions.

*Hardware insight, from reports the backend already writes
(`<base>PathLength.pdrpt`, `<base>RtlMap.json`):*

- **Hardware budgets**: this repository's per-item ratchet, offered to
  users — `[budget]` latency, stages, DSPs or memories per function,
  `kg budget --check` in CI and `--bless` to accept a change.
- **`kg diff-hw <rev>`**: two builds' latency, resources and path lengths,
  per function — what a commit cost in hardware.
- **Cost on save** in the editor ("+2 DSPs, +3 stages"), once the backend
  runs in the background.
- **Waveforms on the source**: a Verilator trace through the RTL map —
  pick a cycle, see each variable's value beside its line.

*Testing without the backend:*

- **`kg eval`**, an interpreter over the elaborated program for pure
  functions and constants (`kg-const` the seed): instant answers, doc tests
  that check their output, and later random inputs compared against the RTL
  simulation. Threads and pipelines are out of its first scope.
- **An interface lock**: `kg interface --check` against a committed
  `interface.lock`, so an exported block's ports do not change unnoticed;
  pairs with `[package] version`.

*Ecosystem:*

- **`[dependencies]`** with a lockfile and `kg add`, each dependency keeping
  its own edition; `import-dirs` unchanged.
- **`kg vendor --for-reference`**: a flat file list and `.kanagawa.args` so
  upstream's CMake builds a kgrust project — `kg init --from-cmake`
  ([`spec/cli.md`](spec/cli.md) §10.4) in reverse, and the proof that
  adopting kgrust locks no one in.

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
| Whether kgrust extends the language at all, behind editions (a direction beyond parity) | no extension lands; the mechanism is planned | [`plans/editions.md`](plans/editions.md) |
| Lints beyond the reference's six warnings (width surprises) | none | [`plans/developer-tooling.md`](plans/developer-tooling.md) §2.5 |
| The VS Code extension's publisher and display name (distinct from upstream's own extension) | unset | [`plans/developer-tooling.md`](plans/developer-tooling.md) §2.1 |
| Where the platform layer lives (here, or a repository of its own) and whether the release archive carries it | here, in `platform/`; shipped since 2026-10-17, so that an installed `kg board` works (two lines of `xtask/src/dist.rs`) | [`plans/platform-layer.md`](plans/platform-layer.md) §8 |
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
- **CI is the light tier only** (2026-10-11): free runners, this
  repository's Rust code (`scripts/ci.sh`); it never builds or runs the
  backend or the reference, and the parity gates stay local.

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
