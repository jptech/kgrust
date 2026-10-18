# Changelog

Notable changes to kgrust, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `kg board`, opt-in through a manifest's `[board]`: `list`, `info`, `new`
  (a project from a board's example), `sim` (the project's testbench under
  Verilator), `netsim` (on the synthesised netlist), `bitstream` (sv2v,
  Yosys, nextpnr and icepack for iCE40; Vivado's batch flow for Artix-7),
  `flash`, `serve` (the design in simulation with its serial port on a
  pseudo-terminal), `area` and `bundle`. Two boards: the UPduino v3 (iCE40
  UP5K) and the Numato Mimas A7 (Artix-7, not yet through Vivado). The
  release archive ships the boards and upstream's runtime RTL.
- `kg build` reads the manifest's `[hw]` and `-Z hw.<key>=<value>` (the
  backend's options, as the legacy flags name them) and `--emit
  resource-usage=<FILE>` / `dgml=<FILE>`; an unknown key is an error.
- A design reaches a board with no glue RTL. A board declares its
  resources (`[resources.*]` in `board.toml`: LEDs, buttons, a serial port,
  SPI, I2C, VGA), and `kg board` generates the board-level core, binding the
  exported functions by name or by the manifest's `[board] connect`, and
  refusing a function whose shape fits no resource with the shapes that
  would. Whatever binds to nothing is served over the serial port by a host
  bridge. Every build writes the class's interface description
  (`target/board/interface.json`; `kg board interface`) and a Python module
  with one method per function (`target/board/py/`, over
  `platform/python/kgbridge.py`).
- `kg board test`: the project's cocotb tests against the generated module.
  A test written with `@kgbridge.test` runs unchanged under cocotb, against
  the simulated board over its serial port, and on the board.
- A peripheral library: SPI and I2C engines, the UART (`platform/rtl/periph`),
  Kanagawa classes over them (`periph.spi.flash`, `periph.i2c.bus`,
  `periph.text`), and models of a SPI flash and an I2C sensor for
  simulation, in SystemVerilog and in Python.
- A front panel for any board: `kg board serve` generates its testbench from
  the board's resources and opens a page served by the simulation itself,
  with live LEDs, buttons and switches, a serial console, and, for a board
  with a `vga` resource, a live screen (about 27 frames a second for pong,
  half real time, with the clock driven from C++). The panel shows what the
  design uses and nothing else, each widget naming the function that serves
  it with its documentation, and lists the rest of the board as unused.
  Under the host bridge it shows each function's calls live. The console
  keeps the line both ways in order. Pause, Reset (the design's reset line)
  and Restart (from power-on, at the same address). `kgbridge.Panel` drives
  it from a script: LEDs, buttons, console, transcript, pause, frames, PNGs.
- The virtual board: no hardware, every resource kind. `kg board new …
  --board virtual` starts a project anyone can run.
- Examples for any board: `accel` (called from Python, with a callback into
  Python), `spi-flash` and `i2c-sensor` (the peripheral library on a board's
  buses) and `pong` (VGA, buttons
  and LEDs on the virtual board). `kg board new` and `list` offer them.

- `AGENTS.md`, the working rules for people and coding agents alike, and
  `CONTRIBUTING.md`; `docs/gates.md`, every gate in one place.
- CI on GitHub Actions, sized for the free runners: `scripts/ci.sh`, the
  tests and corpus gates in one job and clippy and the links in another, on
  Linux for every push and pull request and on macOS for `main`. It builds
  neither the backend nor the reference compiler. It covers the workspace
  but `kg-ptrace` — `xtask`'s own tests too — and checks the generated
  files are current (`codegen --check`). Actions are pinned by commit, the
  token is read-only, and pull requests reuse `main`'s cache.
- Completion after a path's `::`: an enum's constants, each with its value
  and documentation (`mode::` → `pass = 0`, `smooth = 1`); what a class,
  struct or union declares; what a `using` alias's type declares; what an
  aliased import exposes. Typing `:` asks for it. Hover shows an enum
  constant's value too.
- The VS Code extension builds to a `.vsix` with `npm run package`, and
  <kbd>F5</kbd> in its folder starts an Extension Development Host
  (`.vscode/launch.json`); its README covers installing, an isolated
  profile, and which `kg` to point it at.
- `scripts/build-backend.sh --fetch-boost` installs Boost 1.88's headers
  where the system's are older (Ubuntu 24.04 ships 1.83).
- Repository metadata on every crate; `publish = false` across the
  workspace. The VS Code extension gains its repository, license, bug
  tracker and a `kanagawa.trace.server` setting.

### Changed

- The UPduino's `blinky` and `monitor` examples use the generated core;
  `spram` keeps its own, as the example of replacing it. The UPduino's
  shell gains an SPI bus (header pins 44-47) and an I2C bus (2 and 3).
- The UPduino and the virtual board set `max-threads-default = 4`: a call
  waiting on a callback saves every waiting thread, and 512 does not fit a
  UP5K.
- `kg board` synthesises iCE40 designs without Yosys's `-dsp`, which crashes
  Yosys 0.69 on the backend's pipelined products; the HAL instantiates the
  DSP blocks itself.

- `kg build` passes the backend's own warnings (transaction size,
  deprecation, warnings as errors) as the project's warning settings have
  them, as `kanagawa` does; it had the transaction-size warning on
  regardless. `-O` no longer defaults to 2 on the command line, so `[hw]
  optimize` applies; the default is still 2.

- On macOS, the `dev` and `quick` profiles keep debug info in the binaries
  (`split-debuginfo = "off"`): the default, `unpacked`, left every object
  file beside the build output and never collected them, so `target/quick`
  grew without bound.
- The documentation reorganised for readers outside the project: a map by
  audience with a note on how to read the pre-publication record, a
  roadmap of where it stands, what is next and the open questions, and the
  finished sprint plan, the IR design and the 2026-10-11 handoff moved to
  `docs/history/`.
- Every spec opens with a status line — the upstream commit it was read at,
  what changed upstream since for the files it cites, and where the code
  supersedes a proposal — and its drifted citations are re-derived at the
  pin. `spec/cli.md` gains "as built" boxes for `kg` and `kanagawa.toml`.
- `kg --help` names `build` and `doc`. The VS Code extension needs VS Code
  1.82 or later (its language client's minimum).
- Release archives are built with link-time optimisation (`cargo xtask
  dist` uses a new `ship` profile: fat LTO, one codegen unit). `kg check`
  runs 6–10% faster, `kg` is 30% smaller (11.9 → 8.3 MB), and its output
  over the corpus is unchanged.

### Fixed

- The iCE40 HAL's block-RAM and dual-clock FIFOs dropped almost_full the
  cycle reset fell, which the runtime asserts against; a design with a deep
  FIFO stopped in simulation.
- No UP5K design reached the DSP blocks: every product is the backend's
  `KanagawaLutMul`, which the iCE40 HAL built in logic. It now instantiates
  an `SB_MAC16` for products up to 16x16.
- A `kanagawa.toml` whose `[fmt]` or `[lsp]` table set only some keys was
  refused ("missing field `indent`"); the rest now take their defaults.
- `cargo xtask status-md` refuses a partial gate run; `docs/status.md` had
  been published with most of its headline as `?`.
- `cargo xtask` with `KANAGAWA_ROOT` unset doubled the reference's path
  (2,088 conformance failures); a relative root is now made absolute.
- CI's cache is keyed on `Cargo.toml`'s profiles and `scripts/ci.sh` too.
  A profile change had left it matching in full but stale: every run
  rebuilt every dependency, and the cache was never saved again.
- The VS Code extension's `.vsix` left out its language client
  (`node_modules` was excluded), so a packaged extension would not load.
- A timing test in `kg-db` failed on a loaded machine (a 20 ms sleep took
  56); it now holds however long a sleep takes.
- `scripts/vs-haskell.sh` runs on Linux (GNU `time`, `find`, `sed`).
- On Linux, a gate's child over its memory or time budget lost only its
  leader: procps's `kill` took `kill -KILL -<group>` as two options and
  signalled nothing, so a grandchild (a backend worker) ran on.

## 0.1.0 — first published state, 2026-10-11

Not tagged as a release. What the repository held when it was published:

- **The frontend**, at parity with the reference compiler on everything the
  reference's own corpus grades: 2,681 of 2,681 decidable conformance
  assertions, upstream's syntax test runner 72 of 72, the walk into the
  C++ backend identical call for call on 2,422 programs.
- **The tools:** `kg check`, `build`, `fmt`, `lsp`, `doc`, `dump`,
  `explain`, and `kanagawa`, the reference's command line, argv-compatible.
- **The language server:** diagnostics in two layers, quick fixes,
  definition and type definition, references and rename across a project,
  hover and completion with types — per instance inside a template — and
  with documentation, signature help, inlay hints for widths, symbols,
  folding, selection ranges, semantic tokens, formatting.
- **The backend built from source** (`scripts/build-backend.sh`) from the
  `third_party/kanagawa` submodule, and **release archives**
  (`cargo xtask dist`).
