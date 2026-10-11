# Compile speed, first look — 2026-09-17

> **Superseded 2026-09-27** by [`plans/performance.md`](../plans/performance.md)
> §11, measured with the reference's frontend built as it ships. A dated
> snapshot.

A general idea of where the Rust frontend stands against the Haskell one on
programs where the two produce the same backend calls, taken the day
`kg build` first compiled through the C++ backend. Not a benchmark: one
machine, medians of 3–10 runs, no warm-up discipline, no profiling behind
any number. `cargo xtask bench` is the tripwire; this is a snapshot.

## Small programs, full pipeline

Three syntax-test snippets whose walk is call-for-call identical to the
reference's (`cargo xtask ptdiff`: structural), each ~2,000–3,000 ParseTree
calls with `mini-base.k` as the base library, both binaries given the
runner's exact flags in the same directory, through `libkanagawa-backend`
with `--backend none`.

| | reference `kanagawa` | ours (`kanagawa`) |
|---|---|---|
| `--jobs=1`, as `syntax-test.py` runs it | ~120 ms | ~44 ms |
| reference at its default thread count | ~195 ms | ~44 ms |

Our time is the same with the backend loaded and with `KG_BACKEND_DIR=none`,
so it is process start plus the frontend over the base library; the
backend's default pass on these is not visible. About 2.5–4.5× faster end
to end.

## Whole files, frontend only on our side

The walk still declines these files' template closure, so ours is the
frontend alone (`kanagawa` with `KG_BACKEND_DIR=none`, which is `kg check`
plus the legacy CLI's rendering). The reference's frontend alone is the
`kg-oracle` driver the repo builds (`oracle/`); its full run is the shipped
binary with the backend.

| file | reference, full | reference frontend (`kg-oracle --errors`) | ours, frontend |
|---|---|---|---|
| `library/data/optional.k` (172 lines) | 668 ms | 640 ms | 218 ms |
| `library/numeric/float32.k` (3,905 lines) | 8.2 s | 12 s | 5.0 s |
| `test/library/processor/risc-v.k` (4,084 lines) | > 240 s (capped) | 46 s | 2.2 s |

Caveats, in order of weight:

* The `kg-oracle` column is the reference frontend under whatever
  optimization `cabal` gave it and single-threaded; read it as an upper
  bound on the Haskell frontend, not its speed.
* The reference's `risc-v.k` full run is almost entirely the C++ backend
  generating IR for a processor, which we do not run yet. That row compares
  nothing frontend-to-frontend beyond the oracle column.
* Our slow case is `float32.k`: the template route prints and reparses a
  twin of a module with many templates, several rounds. That is the one
  place the Rust frontend is within 2× of the reference rather than well
  ahead, and `cargo xtask bench`'s editor keystroke figure (~390 ms, worst
  file) points at the same route.

## Reading

Everywhere the two frontends agree, the Rust one is faster, by roughly 3×
on small programs and more on large ones outside the twin route. The known
hot spot is pass 8's print-and-reparse twins, not parsing, resolution or
inference. Nothing here argues for optimizing before the walk covers the
corpus (`docs/roadmap.md`, "Picking up the backend bridge");
`CLAUDE.md`'s rule stands — correctness first, bench as a tripwire.

Commands used: the reference at `../kanagawa-build/dist/bin/kanagawa`,
ours at `target/release/kanagawa`, both with
`--place-iterations=1 --import-dir ../kanagawa/library <file>` (the
snippets with the runner's flags from `test/syntax/CMakeLists.txt`), timed
by a Python loop around `subprocess.run`.
