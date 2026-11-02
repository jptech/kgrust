# Changelog

Notable changes to kgrust, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Nightly prereleases: each night `main` has moved, a `nightly`
  prerelease of it for Linux (x86_64, arm64) and macOS (Apple silicon),
  without the code generator, so `kg check`, `fmt`, `lsp` and `doc` work
  and `kg build` needs `KG_BACKEND_DIR`. A nightly's `kg --version` names
  its commit (`0.2.0-nightly.20261102+abc1234`), and its notes list the
  pull requests merged since the last nightly. Not releases: those are
  still tagged by hand after the full gates.
- A release's notes can be drafted rather than typed:
  `scripts/release_notes.py draft` fills `.github/release-template.md` from the
  changelog, the gates' numbers, the archives and the pull requests since the
  last tag, and refuses an archive that reports another version. A person
  writes the highlights and publishes. `gh workflow run nightly.yml -f
  release=true` builds archives that report the plain version, and publishes
  nothing.
- A build from a checkout says what it is: `kg --version`, `kanagawa
  --version`'s second line and the language server's `serverInfo` report
  `0.2.0-dev+abc1234`, and only `cargo xtask dist --release` the plain
  `0.2.0`.
- `cargo xtask dist --label <LABEL>` names the archive
  `kgrust-<LABEL>-<target>`, and the archive's README states the version.
- A Neovim plugin (`editors/nvim`, Neovim 0.11+). `.k` files open as
  Kanagawa, with highlighting generated from the parser's keyword list,
  indentation that follows `kg fmt`'s layout, and `kg lsp` attached.
- `kg doc --check [PATH]...` checks the Kanagawa code blocks in your
  documentation and writes nothing. It reads fenced blocks tagged `kanagawa`,
  in Markdown files and in the doc comments `kg doc` renders; untagged blocks
  are passed over and counted, or read too with `--all-blocks`. A block may
  be a whole file or the body of a function. `<!-- invalid -->` on the line before the fence skips it, and
  `module a.b` on its first line lets later blocks import it. Errors point at
  the block's own line in the file that holds it, in `kg check`'s formats, and
  the exit code is 1. These are the conventions of upstream's `chkdoc`, and
  `--all-blocks` takes the blocks it takes. The check runs the frontend only,
  so what only the backend rejects still needs `kg build`. Upstream's four
  checked documents pass: 319 blocks, plus the 12 they mark invalid.
- `kg expand` shows the reference program a later edition's program stands
  for: your own text, with only the edition's constructs rewritten, and
  comments and layout left alone. One file goes to stdout; `--out DIR` writes
  every file of the project. What cannot be written exactly is refused, naming
  the construct, with nothing written, and every expansion is checked in
  edition 1.0 and walked into the backend against the program before it is
  given. A program using nothing of a later edition comes back byte for byte,
  unchecked. See [`docs/editions/`](docs/editions/README.md#kg-expand).

### Changed

- Archives are stripped (`strip = true` on the `ship` profile). Debug info was
  most of Linux's binaries: `kg` goes from 59.5 to 9.6 MiB, and a Linux archive
  from 24–26 to 6–7. A panic still names its file and line; a backtrace no
  longer names functions.
- Inference does less over again, with identical results. It walks again
  only the declarations that read what a rebuilt symbol map moved, and it
  forgets the ones no walk reaches any more. On `risc-v.k`, `kg check`
  takes 1.94 → 1.73 s and peaks at 786 → 753 MiB; in the editor, with the
  full base library, the last answer after a keystroke comes 2.07 →
  1.82 s, and opening the file takes ~5% less memory.
- The symbol maps the loop builds again and again are repaired rather than
  rebuilt when inference folds a constant in place, with identical
  results: a fold walks again only what can reach it. `kg check` of
  `array.k` takes 0.86 → 0.71 s, and its last answer in the editor after a
  keystroke comes ~16% sooner; `float.k` and `risc-v.k` gain 2–4%, with
  memory unchanged.

### Fixed

- A library file outside your project, opened by going to a definition in it,
  now belongs to the project that imports it, so it no longer reports false
  errors on a source build, and hover reaches the program that instantiates its
  templates.
- `kg fmt` no longer half splits a too-long one-line `if`, `else`, `for`, `do` or
  lambda, or leaves a `class` or `struct` body over the width. As for a function,
  the line stays whole where it fits, and is laid out Allman where it doesn't.
- `kg fmt` settles a too-long one-line `enum` in one run, not two, and no longer
  writes `uint32 x ;` in a one-line `struct` or `union`.
- The Neovim plugin's lazy.nvim install sets `submodules = false`, and its
  vim-plug install points at a local checkout: either would otherwise fetch
  the whole reference compiler, with CIRCT and LLVM inside it.
- `cargo xtask dist` and `install` read `KG_BACKEND_DIR`, as everything else
  does, so they work from a linked worktree without `--backend-dir`.
- `--edition` and `--feature` reach a file named through a symlink from
  another directory, with no manifest above it.
- `kg check` and `kg build` ignored `--edition` and `--feature` for a project
  named through a symlink, and read a later edition's program as edition 1.0.
- A `kanagawa.toml` saved while a check was finishing could have its error
  marked again, in the wrong place, before the fix cleared it: the mark was
  placed by reading the file a second time.
- Hover inside a template defined in another file said "Type not yet
  inferred": a file is answered as its own program, which instantiates
  only what the file itself uses. Where that program has no type to show,
  the hover now reads the program of another open file that imports it,
  lists that program's instances, and names the file. That loop is read,
  never run: with `risc-v.k` open, a hover in the library's `core.k`
  answers in 0.88 ms (median of 50), as its "not inferred" did in 0.86.
- The outline listed an exported class twice, the class and its `export`
  (and an `extern` one likewise). The class is listed once, `export` in its
  detail; an export of an instance (`export Ring<4>;`), or of a type
  declared in another file, keeps an entry of its own.
- `kg fmt` (and editor formatting) half split a function body written on
  one line. Where the line was too long, it broke the parameters and left
  the body on the `)` line, `) { return x + 1; }`; it now keeps the line
  whole where it fits and lays the body out Allman where it does not. And
  an `else`, a `switch`'s brace or the declaration after `template <…>`
  inside a one-line body went to a line of their own, which the next run
  then laid out differently again; they stay on the line.

## 0.2.0 — language editions, 2026-10-31

The first release since the published state. Its headline is language
editions: a package may opt into edition `1.1`, a preview of call sites
that say what they mean, while every program that does not opt in reads
exactly as the reference reads it. Also new: `kg board` and `cargo xtask
install`.

### Added

- Language editions, opt-in per package: `[package] edition` in
  `kanagawa.toml` (`"1.0"`, the reference, by default), single features
  under `[language] features`, and `--edition` / `--feature` for one run
  (`check`, `build`, `doc`, `fmt`, `dump`). A file is read in its own
  package's language, found from the nearest `kanagawa.toml` above it: the
  standard library, which has none above it, is `1.0`, and the `kanagawa`
  binary reads no manifest and is always `1.0`. Edition `1.1` (a preview)
  adds trailing commas, `f(1, 2,)`, in every comma-separated list; named
  arguments, `f(y: 2, x: 1)`, in any order; default arguments,
  `uint8 f(uint8 x, uint8 shift = 1)`, which a call may leave out; and
  named template arguments, `FIFO<uint8, Size: 4, DequeueBlocking: false>`,
  skipping any default — through `kg check`, `kg build`, `kg fmt`, `kg doc`
  and the language server, where a label navigates to its parameter,
  renames with it (or the rename is refused, naming the label, where a
  method call cannot be placed), lights it in signature help and is
  offered as a completion, hover shows a parameter's default, and an inlay
  hint shows the defaults a call is given ([`docs/editions/`](docs/editions/README.md)).
  Named arguments are placed in their parameters' order only where no two
  arguments could tell — a constant commutes with a call — and a reorder
  that could change which side effect comes first is an error; a call
  through a value of function type takes the names its declared type
  spells, and no defaults. A default is judged where it is written.
  Graded by a new gate, `cargo xtask editions`, which CI runs too
  (`--no-oracle`). A file that uses a feature its edition lacks is told,
  once a use, which edition has it and how to ask for it, where the
  reference's cascade of parse errors said nothing of editions.
- A dependency's `kanagawa.toml` that does not load — an unknown edition,
  say — is reported by `kg check`, `kg build` and `kg doc`, which exit 2,
  and marked in the editor, rather than its files read in `1.0` without a
  word.
- The language server marks a `kanagawa.toml` it refuses — an unknown
  edition or feature, a syntax error, a misnamed key — at the value at
  fault, and says so once in a message; it had only logged it, so a
  mistyped edition left its files in 1.0 with nothing on screen saying
  why. The message also reached the log one reload late.

- `cargo xtask install`: the release archive's tree under a prefix
  (`~/.local/share/kgrust`) with `kg` and `kanagawa` linked into
  `~/.cargo/bin`, so a source build finds the platform, the standard library
  and the backend as a release does. `kg` now looks for the backend beside its
  real path, symlinks followed.
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

- References, highlight and rename read each file once for every use of
  the name, rather than once a use: in `risc-v.k`, highlighting `Trap` (177
  uses) took 635 ms and takes 6 ms, its references 680 ms and 17 ms. The
  editor highlights at every move of the cursor.
- `kg check`, `kg build` and the editor's diagnostics read each scope of a
  file once for the parse-time checks, rather than every enclosing scope at
  every name: on `risc-v.k` those checks take 60 ms rather than 114, and the
  editor's first answer after a keystroke 319 ms rather than ~365.
- Two programs the reference rejects as parse errors are now one error
  each, in every edition, rather than a cascade: a label in a call of a
  keyword (`mux`, `lutmul`, `concat`, `static`, `cast`, `fan_out`) is
  `` `mux` takes no named arguments ``, and a default on a function type's
  parameter is `a function type's parameter takes no default`
  ([`docs/known-divergences.md`](docs/known-divergences.md) #42).
- `kg fmt` leaves a broken list that ends in a trailing comma (edition
  1.1) with its commas trailing, one item a line, rather than leading.
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
