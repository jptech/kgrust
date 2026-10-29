# Contributing to kgrust

kgrust reimplements the frontend of [Kanagawa](https://github.com/microsoft/kanagawa),
the hardware design language, in Rust, and builds the tools a designer
lives in around it: `kg check`, `build`, `lsp`, `fmt`, `doc`, and
`kanagawa`, a drop-in for the reference compiler's command line. Its bar is
parity, graded against the reference compiler on the reference's own test
corpus: the same verdict on every assertion, the same calls into the shared
C++ code generator, the same RTL modulo source locations. Every deliberate
difference is listed, with its cost, in
[`docs/known-divergences.md`](docs/known-divergences.md).

Bug reports, divergences from the reference, fixes and tooling work are all
welcome. This page is the practical summary; [`AGENTS.md`](AGENTS.md) holds
the full rules, and they bind every change, whoever writes it.

## Building

The light path needs Rust (rustup installs the version
`rust-toolchain.toml` pins) and a C compiler:

```sh
git clone https://github.com/jptech/kgrust && cd kgrust
git submodule update --init third_party/kanagawa   # the reference: library and test corpus
cargo build
cargo test --profile quick
```

That gives `kg check`, `fmt`, `lsp` and `doc`. A source build bundles no
standard library: pass `-I third_party/kanagawa/library`, or list it in a
project's `kanagawa.toml`. `kg build` also needs Kanagawa's C++ code
generator, `scripts/build-backend.sh` (CMake ≥ 3.30, Ninja, Boost ≥ 1.88;
about half an hour on 16 cores, once). To use your build as the `kg` on your
`PATH`, with the standard library, the boards and the backend found as in a
release, run `cargo xtask install`; `cargo install` copies the binaries alone.

The parity gates run the reference compiler itself, and need far more: GHC
via ghcup, ≥ 32 GB of RAM (64 GB recommended), ~25 GB of disk and about an
hour of one-time setup — the steps are in [`AGENTS.md`](AGENTS.md#setup).
You do not need them to contribute. CI tests the Rust code on Linux and
macOS; the backend build and the parity gates are tested on macOS only so
far, and reports from Linux are welcome.

## Making a change

1. Fork, and branch from `main`, named for the work.
2. Make the change, and run the smallest check that covers it: `cargo test
   --profile quick`, then, if you have the parity setup, `scripts/fast.sh`
   ([the tiers](AGENTS.md#the-loop-fastest-first)).
3. Open a pull request whose description says what changed and the
   evidence: which test or gate moved, from what to what.
4. CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs
   [`scripts/ci.sh`](scripts/ci.sh) on Linux — the quick tests, `corpus`,
   `conformance`, `fmt-check`, `lint`, `codegen --check`, clippy and the
   link check, with no backend build — and checks `main` on macOS too. `scripts/ci.sh` runs the
   same on your machine. The maintainer runs the parity gates before
   merging.
5. Pull requests are squash-merged: `main` keeps one commit per piece of work.

## The rules, in brief

- `third_party/kanagawa/` is the reference and is read-only: never modify
  anything under it, nor run a build with its working directory inside it.
- A new test must be proven load-bearing — break the code it covers and
  watch it fail (`scripts/mutant.sh`). Never weaken a test to make it pass.
- The scoreboard (`xtask/src/conformance.rs`, `xtask/src/corpus.rs`) is
  never edited to change a grade.
- Fail toward silence: where the port cannot be exact, it misses a
  diagnostic rather than inventing one.
- Format by hand: `cargo fmt` is disabled (`rustfmt.toml` says why). Match
  the surrounding style, and keep reformatting out of behaviour changes.
- Cite the reference as `File.hs:line` for every rule ported; comments say
  why, not what.
- Optimisations and refactors are exact: identical output, every gate
  neutral.

## Reporting a divergence from the reference

If `kg` or `kanagawa` treats a program differently from the reference
compiler — accepts what it rejects, reports another error or position,
writes different RTL — that is a bug, unless
[`docs/known-divergences.md`](docs/known-divergences.md) lists it. Please
open an issue with:

- a minimal `.k` program that still shows the difference;
- the commands you ran, and what `kg check` (or `kanagawa`) printed beside
  what the reference printed;
- the kgrust commit, and the reference commit if it is not the one in
  `reference.lock`.

With the full setup, `kg-oracle --errors` prints the reference frontend's
own diagnostics for any file ([`oracle/README.md`](oracle/README.md)),
`scripts/probe.sh <program>` every gate's verdict for one corpus program,
and `cargo xtask reduce` shrinks a program while the two still disagree.

## The design record

[`docs/journal.md`](docs/journal.md) is the record of decisions, findings
and dead ends a `git log` cannot carry; its index names the decisions that
still bind. Read the relevant entry before changing anything that looks
arbitrary. [`docs/README.md`](docs/README.md) maps the rest, and
[`docs/roadmap.md`](docs/roadmap.md) says where the project stands and what
is next.

## How this code was written

Much of kgrust was written with AI coding agents, working under the rules
in [`AGENTS.md`](AGENTS.md): every change graded against the reference's
corpus, every new test proven load-bearing, and the evidence kept in the
commits and the journal. Contributions from people are held to the same
rules.

## License

kgrust is [MIT](LICENSE)-licensed, and contributions are accepted under the
same license. Kanagawa, in the submodule, is © Microsoft Corporation, MIT.
