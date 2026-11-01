# Measurements, 2026-11-01

Raw output behind [`performance.md`](../../plans/performance.md) §11's
re-measurement of our column and the [README](../../../README.md)'s
"Parity and speed" table. Each file is the verbatim output of the command
named below. No gate generated any of them.

**Machine:** the one [2026-10-11](../2026-10-11/README.md)'s files were
measured on (16 cores, 64 GB, macOS, Darwin 27). **Binaries:** two release
`kg` builds, timed side by side: this branch's at `5685cc4` (`rust`) and
`main`'s at `094fefc` (`rust-main`). The reference was not re-run. It is
unchanged since 2026-10-11 (the pin is still `98a2d8c`), and §11 keeps
that day's columns for it.

## Files

| File | What | Load |
|---|---|---|
| `kg-check.txt` | **the reading of record**: every run, then the best of three per program and binary | 2.48 → 2.69 |
| `kg-check-earlier.txt` | the same, run a minute before; kept because the two disagree by up to 9% on `array.k` | 2.45 → 3.06 |

**The command** is the Rust side of `scripts/vs-haskell.sh`: the same
`measure`, the same seven programs in the same order, including the
four-line `inline` function. Each run is `kg check -I library <file>`,
timed by `/usr/bin/time -l` from a fresh directory outside the reference
tree. The two binaries alternate run by run, three runs each per
program.

## Reading them

- **`main` against this branch.** The record's best of three (`main` →
  this branch):
  - `array.k` 0.82 → 0.68 s
  - `float.k` 0.66 → 0.63 s
  - `risc-v.k` 1.71 → 1.66 s
  - `vector.k` 0.67 → 0.63 s
  - `bits.k` 0.25 → 0.24 s
  - `cache.k` and the four-line function equal

  The peak resident set moves by at most 4 MiB.
- **Against 2026-10-11's column** (at `1737ca6`): `main` alone already
  runs `risc-v.k` in 1.71 s rather than 1.95 s, the work merged since then.
  Its other programs are within 0.02 s of that day's.
- **Between the sessions,** this branch's `array.k` is 0.68 against
  0.74 s, and `main`'s 0.82 against 0.88 s. Within each session, the gap
  between the two binaries holds at 16–17%.
- **The four-line function's** 0.04 s is at the resolution of `time`'s
  10 ms, so its ratios move by a quarter with one tick.
- MiB is `/usr/bin/time -l`'s bytes / 2^20, as on 2026-10-11.
