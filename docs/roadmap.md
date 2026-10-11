# Roadmap

Ordered by what unblocks the most, not by what is easiest to demo. Current state
is in [`status.md`](status.md); the reasoning behind past choices is in
[`journal.md`](journal.md).

The ordering principle: **build the thing other work is waiting on, even when a
smaller item would move the scoreboard sooner.** Doing pass 8 before the
whole-program type environment means building the environment twice.

**What is where.** The current queue is below; the decisions that still bind
and the rules follow it. Everything older — the dated queues, the
numbered items §1–§3 (pass 8, the cheap passes, the formatter, the fold
sites), the backend bridge's pickup notes and the "Later" list — is in
[`history/roadmap-history.md`](history/roadmap-history.md), verbatim.

## The queue, 2026-09-27 — the parity sprint

**Paused 2026-10-11** at the milestone `1ebc57a`, every gate green:
[`handoff.md`](handoff.md) has where it stopped and the options from here,
the fuzz residue first.

[`plans/sprint-parity.md`](plans/sprint-parity.md) is the queue: template
elaboration parity (instance identity and display, instances left
undecided, round and copy order, the leftover arms), the remaining
diagnostics and walk declines, M6 (one engine, resilient mode), the
pending consolidation items, and the deferred performance work, in one
order with milestones S1–S6. Revised the same day: the query layer made
incremental and cancellable (W7), the foundations the tools stand on
(W8), and gates that are red when their instrument is absent (W0.4). It folds in the 2026-09-25 queue's two
plans ([`ir-elaboration.md`](plans/ir-elaboration.md),
[`single-source-of-truth.md`](plans/single-source-of-truth.md)) and what
is left of [`performance.md`](plans/performance.md); every plan's status
is in [`plans/README.md`](plans/README.md).

## Beyond the sprint

- ~~**Workspace-wide find-references, and the rooting question under it**~~ —
  done 2026-10-10: references and rename search every file of the project
  (the import directories, and the manifest's root) that spells the name
  and roots a program holding the declaring module, each under that
  program of its own — never one program over the workspace, which
  over-resolves (divergence #19) — and through a loop of its own for a
  member, held one at a time. Bounded at 512 files, and at 10 s for a
  reference list (partial past it, and logged) or 60 s for a rename (a
  refusal past it). Measured on the reference's corpus as one project:
  a function's 484 uses in 37 files in ~0.3 s; `optional`'s `is_valid`,
  36 loops, 13 s (`docs/journal.md`, "References and rename across the
  project"). Left: a file outside both, an unmanaged file's siblings, and
  a member use inside a lambda or an abbreviated template, which a rename
  refuses on. The analysis is in the history's "Later".
- **Inlay hints over copies' widths** — the payoff the language most needs
  (`README.md`). Done 2026-10-10 for a buffer that parses (sprint W3.5:
  hover, inlay hints and signature help list each instance's type inside a
  template's body); left: a buffer that does not parse, which reads no
  loop until W3.4's resilient mode.
- **A keystroke under 16 ms** — the sprint's W3/W5 take it to 100 ms; below
  that is incremental work within a file.

## Decisions, 2026-09-06

Two calls made by the owner after the review of that date. Both still bind.

1. **The backend goes through the ParseTree C ABI**, exactly as the Haskell
   frontend does today: `kg build` will walk the lowered program into
   `libkanagawa-backend` through the 85 `extern "C"` functions
   `docs/spec/abi.md` specifies (`kg-backend-sys` bindings, a `kg-ptbridge`
   safe wrapper). Rewriting the 90k-line C++ middle end is *not* the plan;
   emitting MLIR directly is not either. This is what makes the 241
   backend-decided assertions reachable and `kg build` real without a second
   compiler.
2. **DX features are pulled forward** to derisk them: after each frontend
   slice, one developer-experience slice lands rather than all of them
   waiting under "Later".

The queue those decisions implied (2026-09-06 to 2026-09-10, every slice
done or folded into the sprint) is in the history.

## Rules that outrank the order above

1. **`repoint` is the honest measure.** Assertion counts can rise while
   `repoint` sits still. When they diverge, believe `repoint`.
2. **Benchmark any new pass.** Pass 4 landed correct and made a keystroke take
   1.9 seconds with every correctness gate green. See
   [`CLAUDE.md`](../CLAUDE.md).
3. **`--strict` may never rise.** It falling is progress; `conformance` dropping
   below 100% is a regression, not a trade.
