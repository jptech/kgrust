<div align="center">

# kgrust

**A fast, modern toolchain for [Kanagawa](https://github.com/microsoft/kanagawa), the hardware design language — written in Rust.**

[![CI](https://github.com/jptech/kgrust/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/jptech/kgrust/actions/workflows/ci.yml)

[Install](#install) · [Quick start](#quick-start) · [Boards](#boards) · [Editor](#editor) · [Build from source](#build-from-source) · [Parity and speed](#parity-and-speed) · [Development](#development)

</div>

---

kgrust reimplements the Kanagawa compiler's frontend and builds the tools a designer
lives in around it — a language server, a formatter, a documentation generator — on one
shared core. It compiles every existing `.k` program, unchanged, to the same
SystemVerilog as the reference compiler, through the reference's own code generator, and
checks a design **8–16× faster** than the reference does on all cores, in a fraction of
the memory.

| Command | What it does |
|---|---|
| `kg check` | Parse, resolve and type-check a design; diagnostics with source excerpts |
| `kg build` | Compile to SystemVerilog through Kanagawa's code generator |
| `kg lsp` | The language server: live diagnostics, inferred bit widths, navigation, rename |
| `kg fmt` | Format sources, every comment kept |
| `kg doc` | A searchable documentation site from your doc comments |
| `kg explain` | The reference for an error code or a warning |
| `kg board` | A design on a board, or on a simulated one: generated wiring, a Python API, cocotb, bitstreams |
| `kanagawa` | A drop-in replacement for the reference compiler's command line |

## Install

Release archives will be published on the
[Releases page](https://github.com/jptech/kgrust/releases); until the first
one, [build one yourself](#build-from-source). Unpack it and put its `bin` on
your `PATH`:

```sh
tar -xzf kgrust-0.2.0-aarch64-apple-darwin.tar.gz
export PATH="$PWD/kgrust-0.2.0-aarch64-apple-darwin/bin:$PATH"
kg --version
```

An archive holds everything: `kg`, `kanagawa`, the code generator beside them,
Kanagawa's standard library and the boards of `kg board`, which `kg` finds on its own.
To make one for your platform, or to install your checkout the same way, see
[Build from source](#build-from-source).

## Quick start

```cpp
// adder.k
//| Adds one, every cycle.
class adder
{
public:
    //| `x + 1`, one bit wider so nothing is lost.
    uint33 add_one(uint32 x)
    {
        return x + 1;
    }
}

export adder;
```

```sh
kg check adder.k             # diagnose
kg build adder.k -o out/     # → out/adder.sv
kg doc adder.k --open        # browse its documentation
kg fmt adder.k               # format in place
```

A project keeps its settings in a `kanagawa.toml` at its root, which every command and
the editor read:

```toml
[package]
name = "my-design"

[project]
sources = ["src"]
import-dirs = ["lib"]        # searched before the standard library
target-device = "mock"

[warnings]
shadow = "deny"
```

`kg check` with no arguments then checks the whole project; `kg --help` shows the rest.

## Boards

`kg board` takes a design onto a board, or onto a simulated one when you have none. It
generates the board-level wiring from the board's resources, writes a Python class
with a method per exported function, and runs the same tests under cocotb, against
the simulation, and on the board:

You need Verilator 5 (`brew install verilator`) and, for the cocotb tests, Python with
cocotb 2 (`uv venv && uv pip install cocotb`). `kg board info` says what else is missing.

```sh
kg board new hello --board virtual --example accel
cd hello
kg board test                               # cocotb, on the design's own ports
kg board serve -- python3 test_accel.py     # the same tests over the simulated board's serial port
kg board serve                              # its front panel in the browser: LEDs, buttons, a screen
```

```python
from accel import Accel                     # generated from the design's interface
api = Accel()
api.add(2, 3)                               # 5, from the hardware
api.on("_square", lambda x: x * x)          # the design calls back into Python
```

To see a design on a screen, `kg board new pong --board virtual --example pong`, then
`kg board serve` in it and open the address it prints: pong, drawn from the simulated
VGA pins, played with W S ↑ ↓ ([the demo](platform/README.md#demo-pong-in-the-browser)).

It is opt-in: a project without `[board]` in its `kanagawa.toml` is untouched. There is
a virtual board, the UPduino v3 (iCE40 UP5K, through the open toolchain to a bitstream),
and the Mimas A7 (Artix-7). The guide: [`platform/README.md`](platform/README.md).

## Editor

The language server answers every editor feature from the compiler's own analysis, so
what it shows is what `kg build` will do:

- **Diagnostics as you type**, cancelled and recomputed on every edit
- **Inferred bit widths** as inlay hints and on hover — `a - b` is signed, `a << b` widens
- **Hover with documentation**, completion, signature help
- **Go to definition**, find references and rename across the project
- **Inside templates**, each instance's types, side by side
- Semantic highlighting, document and workspace symbols, folding, quick fixes, formatting

**VS Code:** the extension is in [`editors/vscode`](editors/vscode). Build it with
`npm install && npx @vscode/vsce package`, install the `.vsix`, and keep `kg` on
your `PATH`.

**Any other editor:** point its LSP client at `kg lsp` (stdio) for `*.k` files.

## Build from source

```sh
git clone https://github.com/jptech/kgrust.git && cd kgrust
git submodule update --init third_party/kanagawa   # upstream Kanagawa, pinned
cargo build --release                              # kg and kanagawa → target/release
```

`kg check`, `fmt`, `lsp` and `doc` need only Rust (`rust-toolchain.toml` pins 1.92;
rustup installs it) and a C compiler. A source build does not bundle the standard library
the way a release archive does: point it at the submodule's, with
`-I third_party/kanagawa/library` or `import-dirs` in your `kanagawa.toml`. `kg build`
also needs Kanagawa's code generator, a C++ library built from the submodule:

```sh
scripts/build-backend.sh                    # → target/backend/dist/bin/libkanagawa-backend.*
cargo xtask dist                            # → target/dist/kgrust-<version>-<target>.tar.gz
cargo xtask install                         # the same tree under ~/.local/share/kgrust, linked into ~/.cargo/bin
```

`cargo xtask install` is the way to put your checkout's `kg` on the `PATH` (`--prefix`
and `--bin-dir` move either half; run it again to update). `cargo install --path
crates/kg-cli` is not: it copies the two executables alone, so `kg board new` finds no
platform and `kg build` no backend. `kg` finds both, and the standard library, relative to
its own real path, which the links `install` makes preserve.

`build-backend.sh` compiles LLVM, MLIR and CIRCT with the backend — about half an hour on
16 cores the first time, incremental after — and needs CMake ≥ 3.30, Ninja and Boost ≥ 1.88
(`brew install cmake ninja boost`; where your Boost is older, as Ubuntu 24.04's is,
`--fetch-boost` installs its headers). It builds only what the backend links, never
Kanagawa's Haskell frontend. CI builds and tests the Rust tools on Linux and macOS; the
backend build is tested on macOS (Apple silicon), and reports from Linux are welcome.

## Parity and speed

kgrust is graded against the reference compiler on the reference's own test corpus, in
gates:

| | |
|---|---|
| Frontend verdicts | **2,681 / 2,681** decidable assertions agree |
| Upstream's own test runner | **72 / 72** syntax test files pass |
| Code generation | the same RTL as the reference on every program both compile, source-location comments aside |
| Command line | **25 / 25** upstream CLI tests pass with `kanagawa` |

Checking a design, against the reference frontend ([details](docs/plans/performance.md#11-standing-against-the-reference-re-measured-2026-10-11-one-core-every-core)):

| Program | Reference, 1 core | Reference, 16 cores | kgrust | Faster |
|---|---:|---:|---:|---:|
| `data/array.k` | 28.2 s | 12.8 s | 0.80 s | 16× |
| `numeric/float.k` | 30.4 s | 10.6 s | 0.66 s | 16× |
| `processor/risc-v.k` | 54.0 s | 24.6 s | 1.95 s | 13× |

Those are the `release` build's times. The release archives are optimised as one
program and check 6–10% faster again, with identical output.

A whole build is mostly the shared code generator, so `kg build` finishes 1.5–2× sooner
end to end ([where the time goes](docs/plans/backend-survey.md)). Deliberate differences
from the reference are listed, each with its reason, in
[`docs/known-divergences.md`](docs/known-divergences.md).

## Development

```sh
cargo test --profile quick                  # the tests
scripts/fast.sh                             # the gates a change can move
scripts/gates.sh                            # every gate, graded against the reference
```

The parity gates run the reference compiler itself, so they need it built
(`scripts/build-backend.sh --reference`, with GHC via [ghcup](https://www.haskell.org/ghcup/)),
its frontend as an oracle (`oracle/build.sh`) and its runs recorded (`cargo xtask ptrace`)
— [`CONTRIBUTING.md`](CONTRIBUTING.md) walks through it. [`AGENTS.md`](AGENTS.md) holds the
working rules, for people and coding agents alike; [`docs/README.md`](docs/README.md) maps
the documentation — [`roadmap`](docs/roadmap.md), [`status`](docs/status.md),
[`architecture`](docs/architecture.md) and the [`journal`](docs/journal.md) of decisions.
What changed release by release is in [`CHANGELOG.md`](CHANGELOG.md).

## License

[MIT](LICENSE) © 2026 Parker Mitchell and the kgrust contributors.

Kanagawa — the code generator and standard library kgrust builds from — is © Microsoft
Corporation, MIT. Release archives carry `THIRD-PARTY-NOTICES.md` for everything they
include. The whole toolchain is permissively licensed: `kg doc` renders Markdown with
`pulldown-cmark` rather than pandoc.
