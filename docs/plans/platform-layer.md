# Platform layer: devices, boards and libraries beyond upstream's

*Part of the [plans](README.md). **Active, 2026-10-17: phase 1 built on
branch `platform-layer` with two pilots. The Numato Mimas A7 (Artix-7
XC7A50T) is checked as far as a machine without Vivado can. The tinyVision
UPduino v3 (iCE40 UP5K) goes end to end on the open toolchain, from source to
bitstream, with its examples checked in RTL and gate-level simulation, but not
yet on a board. Phases 2–4 proposed, nothing decided.** Checked against the
reference at `98a2d8c`.*

The upstream library ships seven devices (four Intel, one UltraScale+, a
mock) and no boards. This plan adds a layer in this repository that carries
more of both, plus libraries of our own, without editing the submodule, the
frontend or the backend: everything goes through seams upstream already
exposes. §1 is those seams, §3 the layer's shape and rules, §4 how a user
reaches it, phase by phase, §5 the Artix-7 pilot as built, §6 what is left
for a machine with Vivado, and §8 the iCE40 pilot: its device model, HAL,
open flow, SPRAM access and what makes a Kanagawa design small.

## 1. The seams upstream already has

**A device is an import-directory subtree.** `--target-device <name>` puts
`<dir>/device/<name>` for every import dir ahead of every plain import dir
(`importDirs = map (</> "device" </> targetDevice opt) (importDir opt) ++
importDir opt`, `Parser/IO.hs:110`; ported in `kg-config/src/resolve.rs:195`
and `kg-cli/src/legacy/options.rs:470`). A device directory provides three
modules: `hardware.config`, `hardware.dsp` and `numeric.float32.operator`
(no plain-library fallback exists for the last: `library/numeric/float32/`
holds only `internal`). Any import root, not only upstream's `library/`, can
hold a `device/` directory.

**The backend knows no device by name.** `compiler.config`
(`library/compiler/config.k`) reads every constant from
`compiler.device.config`, which imports `hardware.config`. The backend
extracts ~35 properties from that module by name
(`compiler/cpp/device_config_visitor.cpp:599-634`) and fails the build when
one is missing or not `const` (`Finalize`, same file). A new device needs no
C++ and no Rust.

**The vendor RTL is a HAL keyed by family.** Generated SystemVerilog
instantiates `hal_*` and `_hardware_dsp__*` modules with `DEVICE_FAMILY`
set to `hal_device_family` (`compiler/cpp/verilog.cpp:2648, 4245, 5489,
8225, 10582`). Upstream implements them per vendor in
`runtime/rtl/hal/{intel,xilinx,mock}`. The Xilinx set is built on the XPM
macros (`xpm_memory_*`, `xpm_fifo_*`, `xpm_cdc_*`), which Vivado supports
on 7-series as well as UltraScale+, and it never compares `DEVICE_FAMILY`.
`runtime/rtl/files.f` is the portable runtime, and `runtime/rtl/sim/` the
simulation-only part (a string table the generated RTL instantiates outside
synthesis, the testbench mailboxes).

**The memory model has two constraints.** Within each memory type,
configurations are listed deepest first, which `GetMemoryComposition`
asserts (`compiler/cpp/backend_common.h:457`). At most ten entries per type
are allowed (`config.h:80-84`). A type with no entries is never chosen,
which is how upstream's Intel devices state that they have no deep RAM.

**What kgrust added: nothing device-specific, and two gaps** (as surveyed;
both closed since — `dist` ships `runtime/rtl` and the platform, and `[hw]`
and `[board]` are read; see "Phase 2" below).
- `cargo xtask dist` shipped `library/` but not `runtime/rtl`, so an installed
  `kg build` wrote RTL that nothing in the archive could synthesise.
- The manifest's `[hw]` table was accepted and read by nothing
  ([`spec/cli.md`](../spec/cli.md) §10). There was no notion of a board.

## 2. Goals

- New devices, boards and libraries live here, and none of them edits the
  submodule, the frontend or the backend.
- From a `.k` file to a bitstream in one command per board, and to a
  vendor-free pass/fail in another, which can run in CI.
- The parity gates stay neutral: nothing on their import paths changes.

Not goals: a new HAL where upstream's fits a family; changing the backend.
An open-source flow was a separate decision (§4 phase 4); the owner took it
on 2026-10-17 for the iCE40 (§8).

## 3. The layer

```
platform/
  library/                      a Kanagawa import root, upstream's layout
    <vendor>/config.k           vendor defaults: module <vendor>.config (lattice)
    <vendor>/<family>/config.k  family defaults: module <vendor>.<family>.config
    device/<device>/            hardware/config.k, hardware/dsp.k,
                                numeric/float32/operator.k, and device-only
                                libraries (device/ice40-up5k/lattice/spram.k)
    <library>/…                 device-neutral libraries of our own (none yet)
  rtl/hal/<hal>/                a HAL for a family upstream's cannot serve (ice40),
                                with its test/ (check.zsh)
  boards/<board>/
    board.toml                  device, part, HAL, tool, clock, backend options
                                ([hw]), programmer, constraints
    rtl/                        a shell the board's designs share (optional)
    constraints/                pin and timing constraints for the shells
    examples/<name>/            a project: kanagawa.toml naming the board, a
                                design, its glue RTL, a self-checking testbench
  vendor/vivado/                the Vivado flow, {build,program}.tcl
  check.zsh                     every example through every flow (`kg board`)
```

`kg board` (`crates/kg-board`, [`board-tooling.md`](board-tooling.md))
runs the flows: the icestorm flow, the repairs of malformed backend output
(§8.5), simulation and programming. It replaced `scripts/board.sh`,
`vendor/icestorm/{build,area}.zsh` and `vendor/fix_generated.py`
(2026-10-17).

### 3.1 Rules

1. **Additive only.** The layer comes after upstream's library on the import
   path (`-I <upstream>/library -I platform/library`), so the first match
   is always upstream's. A layer module with an upstream module's path
   would be dead, and a layer device named like an upstream device would be
   shadowed. A test should refuse both: no relative path under
   `platform/library` may exist under upstream's `library/` (phase 2).
2. **Upstream's naming.** A family's defaults are `<vendor>.<family>.config`,
   importing the vendor's (`xilinx.artix7.config` imports `xilinx.config`,
   as `xilinx.ultrascaleplus.config` does). A device directory is
   `<family>-<part>` (`artix7-xc7a50t`, beside `ultrascaleplus-xcu250`).
3. **Every number cited or marked.** A device value cites its datasheet or
   user guide. Where it is an estimate it says `PROVISIONAL`, and phase 1's
   Vivado checklist (§6) replaces it with a measured one.
4. **Board facts cite the vendor's file and revision**, and constraints
   cover only the ports a shell uses: Vivado raises a critical warning for
   a constraint on a port the design lacks. Vendor files are cited, not
   copied.
5. **Every board example has a vendor-free check.** `kg board sim`, in the
   example's directory, compiles it, then simulates its RTL and testbench in
   Verilator against upstream's portable runtime and the board's simulation
   HAL (`sim-hal`: the board's own where it is plain SystemVerilog, as
   iCE40's is, upstream's mock where it needs vendor libraries, as Xilinx's
   XPM does). Where the flow is open, `kg board netsim` runs the same
   testbench on the synthesised netlist; `platform/check.zsh` runs every
   example's checks. A new test is proved load-bearing with
   `scripts/mutant.sh`, as anywhere else in the repository.
6. **The backend decides storage.** Where a HAL can tell, it honours the
   backend's `USE_LUTRAM`/`USE_BRAM` rather than letting synthesis choose:
   the backend budgets block RAM against the device's count, and a synthesis
   tool promoting memories on its own spends block RAM the budget gave
   elsewhere (§8.5).

## 4. How a user reaches it

**Phase 1 (built): flags and a script.** Any `kg build` reaches a layer
device with `-I <upstream>/library -I platform/library --target-device
artix7-xc7a50t`, or with a manifest's `import-dirs` and `target-device`.
`scripts/board.sh` did this for a board, with the compat binary, until
`kg build` read backend options.

**Phase 2 (built 2026-10-17, differently): `kg board`.** A manifest's
opt-in `[board]` sets the device and appends the platform's library;
`kg build` reads `[hw]` and `-Z`; `kg board` runs the flows; the release
archive ships the platform and the runtime RTL. Where it departs from the
proposal below: the layer reaches `kg` only through `[board]`, never for
every project (item 2), and the key is a `[board]` table, not
`[project] board`. [`board-tooling.md`](board-tooling.md) §1–§2 has it.
The proposal, as written:
1. `cargo xtask dist` ships `share/kanagawa/platform/` (`library`, `boards`,
   `vendor`) and `share/kanagawa/runtime/rtl`. The second is needed with or
   without the layer.
2. `Settings::or_bundled_library` (`kg-config/src/resolve.rs:163`) also
   appends `share/kanagawa/platform/library` after the bundled library when
   it exists, so an installed `kg build --target-device artix7-xc7a50t`
   works with no flags. Only `kg` (`kg-cli/src/modern/settings.rs:147`) and
   the language server (`kg-lsp/src/global_state.rs:625`) call it. The
   gates build their own settings, so they are neutral by construction;
   `scripts/gates.sh` confirms it.
3. Boards in the manifest: `[project] board = "mimas-a7"` implies
   `target-device`, and `kg build --board <name>` writes the bundle that
   `scripts/board.sh bundle` writes today. A `kg boards` command lists the
   boards and devices. The manifest denies unknown fields, so this is a
   schema change ([`spec/cli.md`](../spec/cli.md) §10). The board's clock
   becomes the default `--frequency`, the first reader of `[hw]`.
4. The rule 1 test, and `kg check` over `platform/library` in CI.

**Phase 3: generated shells.** A board's `board.toml` names its peripherals
(clock, reset, LEDs, switches, buttons) and the pins behind them. `kg build
--board` then writes the shell from the design's exported interface: every
`<fn>_valid_in/_rdy_out/_rden_in/_empty_out` port has the protocol
upstream's own testbenches drive (`test/interface/simple_exported_class.sv`).
A design binds by name, for example a function `leds(uint8 switches)`, or
in the manifest. A board library module (`board.mimas_a7`: clock, LED
count) gives designs the same facts.

**Phase 4: breadth.** Other 7-series parts are one `hardware/config.k`
each: XC7A35T, XC7A100T, XC7A200T, then Spartan-7, Kintex-7 and Zynq-7000
programmable logic. Each needs only resource counts and the family
defaults. Libraries of our own go under `platform/library/<name>`, either
device-neutral or under `device/<d>/`. A vendor without XPM needs its own
HAL under `platform/rtl/hal`, as the iCE40 now has (§8). Other iCE40 parts
(UP3K, HX8K, LP1K) are one `hardware/config.k` each on the family module;
Gowin or ECP5 would be a new HAL on the same pattern.

## 5. The pilot as built (2026-10-17)

| File | What |
|---|---|
| `platform/library/xilinx/artix7/config.k` | Artix-7 family defaults; UltraScale+'s where the fabric agrees, each change argued beside it |
| `platform/library/device/artix7-xc7a50t/` | XC7A50T: 480 LUT-RAM units (2,400 SLICEMs / 5, upstream's rule), 150 RAMB18 (DS180); `umul18`/`imul18`; soft float32 |
| `platform/boards/mimas-a7/board.toml` | `xc7a50tfgg484-1`, HAL `xilinx`, Vivado, 100 MHz on `CLK1` |
| `platform/boards/mimas-a7/constraints/mimas-a7.xdc` | clock, reset, 8 LEDs, 8 DIP switches from Numato's V3.0 constraints; buttons and seven-segment commented out; quad-SPI bitstream settings |
| `platform/boards/mimas-a7/examples/blinky/` | `blinky.k` (a counter XOR the switches), `top.sv` (the shell), `tb.sv` (the check) |
| `platform/vendor/vivado/{build,program}.tcl` | non-project synthesis to bitstream, reports, `.mem` files read in; JTAG load |
| `scripts/board.sh` | `sim`, `bundle`, `bitstream`, `program` (and since §8, `netsim`) |

**What departs from UltraScale+**, each argued in `config.k`:
- No deep RAM: 7-series has no UltraRAM.
- LUT-RAM entries halved: four LUTs per SLICEM to eight (`PROVISIONAL`).
- `hal_device_family = "artix7"`.

The FIFO costs are UltraScale+'s (`PROVISIONAL`). Auto-pipelining stays
off, as Vivado offers it only from UltraScale.

**Checked, 2026-10-17, at `0fd1ad1` plus this branch:**
- `kg build -I third_party/kanagawa/library -I platform/library
  --target-device artix7-xc7a50t` builds `blinky.k`. The backend extracted
  every device property (`Finalize` fails on a missing one).
- Twenty corpus programs (`test/logic/*`, `test/library/{data,numeric}/*`),
  built for `artix7-xc7a50t` and for `ultrascaleplus-xcu250`, give the same
  verdict for each. Nineteen build for both; one fails for both because
  the loop did not supply the sibling file it imports. The RTL differs
  where the memory model does (`data/datetime.k`). The submodule's
  `git status` was clean before and after.
- `scripts/board.sh mimas-a7 sim` prints PASS (Verilator 5.052, ~8 s
  including the build). The testbench covers power-on, the switches, the
  counter's bit 24 on LED0 and the reset button.
- Five mutants were planted and all caught: the counter's bit, the shell
  ignoring the result, a stuck reset, the button ignored, never calling the
  design.
- `scripts/board.sh mimas-a7 bitstream` writes the bundle (50 sources in
  compile order, the constraints, part and top). It stops with exit 3 and
  the Vivado command line, because Vivado is not installed.

**Not checked:** anything Vivado does — synthesis, XPM resolution, timing,
the bitstream, JTAG — and whether the device values match the silicon.

## 6. On a machine with Vivado

In order:
1. **Board revision.** Read the board's silkscreen. The constraints follow
   Numato's V3.0 file, and V4.0 moved at least `PROG_B`. Diff
   `mimas-a7.xdc` against Numato's file for the board in hand.
2. **`kg board bitstream`**, in `platform/boards/mimas-a7/examples/blinky`.
   - Non-project synthesis should pick up the XPM macros on its own; if
     not, add `auto_detect_xpm` to `build.tcl`.
   - Read `drc.rpt` and `timing_summary.rpt`: blinky should meet 100 MHz
     with a wide margin on a -1 part.
3. **`kg board flash`**, in the same directory.
   - Unknown: whether `hw_server` sees the FT2232H's JTAG channel without
     Numato's own configuration tool.
   - If it does not, record the tool that works instead.
   - Then watch the board: LED0 should blink at ~3 Hz, each switch should
     invert its LED, and S3 should restart the count.
4. **Calibrate the `PROVISIONAL` values.**
   - Build corpus designs that use memories and multiplies.
   - Compare the compiler's estimate (`kg build --emit resource-usage=<FILE>`) with
     `report_utilization`.
   - Adjust the LUT-RAM entries and the FIFO costs, and record the
     before-and-after numbers in the journal.
5. **DSP.**
   - Count the DSP48E1 slices one `umul18` takes: an 18×18 unsigned
     product does not fit a 25×18 signed multiplier.
   - Upstream accepts the same on UltraScale+, but a narrower intrinsic
     would need a new `compiler.hal` class, which is upstream's to add.
6. **Flash.** Identify the quad-SPI part, then add `write_cfgmem` to the
   flow and a `flash` subcommand.
7. **Frequency.** Find what a non-trivial Kanagawa design closes at on a -1
   Artix-7. That sets the board's default `--frequency` (phase 2, item 3).

## 7. Adding a device or a board

**A device** (one more Artix-7 part, say):
1. Create `platform/library/device/<family>-<part>/` from the nearest
   sibling.
2. Change `device_name` and `memory_resources`, citing the datasheet table.
3. Build a corpus sample for it and for the sibling, and compare verdicts.

**A new family:** add `<vendor>/<family>/config.k` first, with every
departure from the nearest upstream family argued beside it.

**A board:**
1. Create `platform/boards/<board>/` with `board.toml`, the constraints
   (vendor file and revision cited), and one example with a self-checking
   testbench.
2. In the example's directory, `kg board sim` must print PASS, and `kg
   board netsim` too where the flow is open; add nothing to
   `platform/check.zsh`, which finds the example itself.
3. Prove the testbench with `mutant.sh`.
4. Add the board's vendor checklist to §6 or §8.6, or to the board's own
   notes.

**A board like another, with other pins** (the owner's UP5K board, say):
the board's directory holds a `board.toml` and its constraints, and reuses
the other board's shell by path. It can live in the platform's `boards/`
or in a project of its own. Paths in `board.toml` are relative to the
board's own directory:

```toml
[board]
name = "my-board"
rtl = ["../upduino-v3/rtl/upduino_top.sv"]   # the UART and buses are platform/rtl/periph's
top-module = "upduino_top"
core-module = "upduino_core"

[constraints]
files = ["constraints/my-board.pcf"]          # the shell's port names, this board's pins

# Copied from the UPduino's board.toml, so `kg board` generates the core:
[resources.led]
kind = "led"
width = 3
pins = { out = "rgb" }
# … and uart, spi, i2c as there.
```

with `[fpga]`, `[clock]`, `[hw]` and `[program]` copied from
`upduino-v3/board.toml` (the package changes if the part's does). A
project then names it, `[board] path = "../my-board"` (or `name =
"my-board"` in the platform), and any UPduino example runs on it unchanged
— copy one with `kg board new` and change the manifest's board. A board
with other parts gets its own shell; the examples' cores keep working if it
offers the same `upduino_core` ports.

## 8. The iCE40 UP5K and the UPduino v3

The owner asked on 2026-10-17 for the Lattice iCE40 UltraPlus UP5K, end to
end on the open toolchain, with the tinyVision UPduino v3 as the reference
board, SPRAM reachable from Kanagawa, and a design as small as fixed latency
and fewer FIFOs can make it, at 24 MHz.

The part is small: 5,280 logic cells (a LUT4, a flip-flop and a carry each),
30 block RAMs of 4 Kb, four 256 Kb single-port SPRAM blocks, eight 16×16
multipliers, and no distributed RAM.

### 8.1 What was built

| File | What |
|---|---|
| `platform/library/lattice/config.k`, `lattice/ice40/config.k` | the vendor and family models: `LUT` memory as flip-flop storage, EBR as `Block` (four aspect ratios, no true dual port, no ECC), FIFO floor 2, `keep` as the don't-merge attribute |
| `platform/library/device/ice40-up5k/` | 5,280 cells, 30 EBR (DS-02008); no multiplier intrinsic, so every product is `KanagawaLutMul`, which the HAL puts on an SB_MAC16 (corrected 2026-10-17: `board-tooling.md` §10); soft float32; `lattice/spram.k` |
| `platform/rtl/hal/ice40/` | the HAL upstream's runtime and generated RTL instantiate, for Yosys: RAMs and FIFOs on the backend's storage decision, a block-RAM show-ahead FIFO, a dual-clock FIFO, `KanagawaLutMul` on SB_MAC16 up to 16x16 (in logic until 2026-10-17), `KanagawaIce40Spram`; `test/check.zsh` |
| `platform/vendor/icestorm/` (retired 2026-10-17 for `crates/kg-board`) | `build.zsh`: translate-off regions cut, sv2v, `synth_ice40 -dsp`, nextpnr, icepack, a utilisation and Fmax summary, or a gate-level netlist; `area.zsh`: one program's cells, no board |
| `platform/vendor/fix_generated.py` (retired likewise) | the backend's malformed single-reset declaration repaired (§8.5) |
| `platform/boards/upduino-v3/` | `board.toml`; `upduino-v3.pcf` from tinyVision's own, its serial pins traced in their PCB (§8.7); `rtl/upduino_top.sv` (SB_HFOSC at 24 MHz, power-on reset, SB_RGBA_DRV; since 2026-10-17 SPI and I2C buses too); `[resources]`; examples `blinky`, `spram` and `monitor`, each a design and a testbench, the core generated but for `spram`'s; `sim/uart_pty.sv`, the serial pins on a pseudo-terminal. The UART moved to `platform/rtl/periph/uart.sv` and the pseudo-terminal's C++ to `platform/sim/` (`board-tooling.md` §5) |
| `platform/boards/upduino-v3/examples/monitor/host.py` | the monitor's host side: read, write, dump, a random test (streamed or not), a shell |
| `scripts/board.sh` (retired 2026-10-17 for `kg board`) | iCE40 flows: `netsim`, the icestorm `bitstream`, `program` through `iceprog`; `serve`, a simulated core on a pseudo-terminal; per-board compiler flags and clock |

### 8.2 Checked (2026-10-17, at `0fd1ad1` plus this branch)

Commands from the checkout's root; times on this machine, warm. These ran
through `scripts/board.sh`; `kg board` reproduced every figure the same day
(`platform/check.zsh`, [`board-tooling.md`](board-tooling.md) §2.4).

| Command | Result |
|---|---|
| `scripts/board.sh upduino-v3 sim blinky` | PASS, ~7 s |
| `scripts/board.sh upduino-v3 netsim blinky` | PASS (Verilator on the Yosys netlist with Yosys's cell models) |
| `scripts/board.sh upduino-v3 bitstream blinky` | 55 / 5,280 cells, Fmax 73.2 MHz against 24, ~2 s |
| `scripts/board.sh upduino-v3 sim spram` | PASS, ~5 s (8.5 M cycles) |
| `scripts/board.sh upduino-v3 netsim spram` | PASS |
| `scripts/board.sh upduino-v3 bitstream spram` | 159 cells, 2 / 4 SPRAM, Fmax 59.3 MHz (160 and 60.1 before the shell stopped driving pin 16) |
| `platform/rtl/hal/ice40/test/check.zsh` | PASS: behaviour and seven storage mappings, ~10 s |
| `scripts/board.sh upduino-v3 sim monitor` | PASS: 261 reply bytes checked, ~4 s |
| `scripts/board.sh upduino-v3 netsim monitor` | PASS |
| `scripts/board.sh upduino-v3 bitstream monitor` | 380 cells (7.2%), 2 / 4 SPRAM, Fmax 42.8 MHz |
| `scripts/board.sh upduino-v3 serve monitor -- python3 …/host.py test --count 1000 --stream` | PASS: 1,000 words written and read back by the host script, through a pseudo-terminal, against the simulated design, ~10 s |
| `scripts/board.sh mimas-a7 sim` | PASS, after the switch to the compat binary |

Mutants, each planted with `scripts/mutant.sh` against the command above it:
- **blinky (3, all caught):** the counter's bit, the core never calling,
  reset never reaching the design.
- **spram (7, all caught):** wrong data written, the compare inverted, the
  compare removed, the SPRAM's bank select, read mux and nibble mask, reset.
  The removed compare first survived: a correct memory never mismatches. The
  testbench now inverts every stored word mid-run and requires red.
- **HAL (10; 8 caught):**
  - Caught: the block-RAM FIFO's fetch, `almost_full` and `usedw`, and its
    storage attribute; the RAM's output register and its storage attribute;
    `KanagawaLutMul`'s signed operands; the SPRAM's registered bank.
  - The RAM's storage attribute first survived, because the flow's own rule
    covered for it. The mapping checks now run without that rule.
  - Survived, and expected to: two dual-clock FIFO mutants, a synchroniser
    stage dropped and binary pointers crossing in place of Gray. Both are
    metastability robustness, which no zero-delay simulation can see. The
    check is blind there.

- **monitor (11; 10 caught, two of them after testbench additions; 1
  equivalent):** caught were the parser accepting short fields, upper-case
  hex in replies, the drop rule removed, writes on every byte, the address
  shift, the core polling `tx_next` while the transmitter is busy, the
  receiver's bit order and start-bit glitch check, the transmitter's stop
  bit, and blank lines answered.
  - Blank lines answered and the glitch check first survived: both blank
    lines in the testbench arrived while a reply was going out, and it sent
    no glitch. It now sends a blank line on an idle link, and a quarter-bit
    glitch.
  - One survivor turned out to be equivalent: dropping `busy` from the red
    LED's condition. A dropped reply is always a malformed line's (§8.7),
    so the term was removed.

**Not checked:** anything on a board — the bitstream on a UPduino, the
LED's colours and polarity through SB_RGBA_DRV, `iceprog`, the FT232H's
serial port.

### 8.3 What makes a design small

Measured with `platform/vendor/icestorm/area.zsh <program> <top> [flags]`
on 2026-10-17, since `kg board area` (the same synthesis and count). The
attribute rows change only the attribute on `step` in the board examples;
"board flags" are the five options of `upduino-v3/board.toml`, then its
`[kanagawa] flags`, now its `[hw]`. Cells after `synth_ice40 -dsp`:

| Design | No flags: LUT4 / FF | Board flags: LUT4 / FF |
|---|---:|---:|
| blinky, `[[latency(0)]]` (as shipped) | 49 / 43 | 41 / 38 |
| blinky, `[[no_backpressure]]` | 49 / 53 | 44 / 49 |
| blinky, no attribute (ready/valid, return FIFO) | 138 / 117 | 95 / 84 |
| spram, `[[latency(2)]]` (as shipped) | 105 / 95 | 99 / 91 |
| spram, `[[no_backpressure]]` | 103 / 108 | 100 / 101 |
| spram, no attribute | 192 / 172 | 190 / 164 |
| `test/interface/simple_exported_class.k` (upstream; ready/valid) | 519 / 682 | 513 / 677 |

The same three ready/valid designs with the family's FIFO floor at
upstream's 32 rather than 2 (board flags): blinky 203 / 164,
spram 259 / 216, `simple_exported_class` 983 / 1,193. The floor halves them.

What follows, in order of effect:
1. **Fixed latency at the boundary.**
   - `[[latency(N)]]` on an exported method removes its ready, its output
     valid and its return FIFO. The caller supplies a call per cycle and
     takes the result N cycles later (the core's job).
   - `[[no_backpressure]]` removes the ready and the FIFO's storage, and
     keeps an output valid.
   - Neither can be combined with `[[max_threads]]` (Error 18).
   - The least N is what the body's scheduling needs: the scheduler refuses
     less ("`[[schedule()]]` constraints are impossible to satisfy"). The
     SPRAM example needs 2.
2. **The FIFO floor.** `min_fifo_depth` is only a floor (`lower.cpp:768-791`).
   Upstream's 32 fills an MLAB; on iCE40 every entry is flip-flops, so the
   family uses 2. Throughput sizing is unchanged.
3. **No initialisers on state.** iCE40 flip-flops leave configuration at
   zero, and an initial value adds reset logic per bit (upstream's
   `effective-kanagawa.md`, "Remove initialization of static variables").
   Both examples rely on zero.
4. **Area flags for frequency.** Each saves area on its own, and together
   they save 5–25% (more on the ready/valid designs). At 24 MHz the shipped
   examples close at 60–73 MHz:
   - `--reset-cycles=1`
   - `--semaphore-delay=0`
   - `--additional-latency=0`
   - `--register-ratio=8`
   - `--no-reset-fanout`

   Each is a backend option and changes no result. `--max-threads-default`
   changed nothing measured and is left alone.
5. **What does not help.** Yosys's `-abc9` was larger and slower than the
   default mapper on both examples (56 / 161 cells and 67.4 / 53.6 MHz,
   against 55 / 160 and 73.2 / 60.1).

The irreducible cost of a Kanagawa module here is its reset sequencing,
about 12 flip-flops and 18 LUTs. Blinky past that is its counter.

### 8.4 SPRAM from Kanagawa

`import lattice.spram` (UP5K only: the module sits in the device's
directory) gives `spram<Words, Width>`. It combines the four SB_SPRAM256KA
blocks in depth (16K, 32K or 64K words) and width (16, 32 or 64 bits), and
`static_assert`s at most four. It has one method:

```
inline word_t access(addr_t addr, bool write, word_t data, mask_t nibble_mask)
```

`access` is fixed-latency 1: the word read arrives the cycle after the call,
and after a write the result is undefined. Behind it is an external class
template, `spram_banks`, mapped to `KanagawaIce40Spram`, whose port list is
the one the backend generates for a `[[latency(1)]]` external method.

The block has one port, and the compiler enforces it: a fixed-latency
external method with a second call site is an error ("Externally
instantiated fixed-latency function has multiple call sites"). A design
routes its reads and writes through one call, as the example's sweep does.
The SPRAM is outside the backend's memory model on purpose, because a
Kanagawa `memory` needs a read and a write port at once.

### 8.5 Problems found, and what was done

- **The backend's single-reset declaration is malformed.**
  - Cause: with exactly one reset replica,
    `InstantiateResetControlCIRCT` declares `reg_rst_delayed` as a
    one-bit integer, which CIRCT writes as a scalar `logic`
    (`verilog.cpp:2935`). `GetResetReplica` still writes
    `reg_rst_delayed[0]` (`verilog.cpp:6350`).
  - Effect: Yosys reads the bit-select as the bit; Verilator refuses it.
    It takes a design with a single reset replica: the UPduino blinky, a
    single basic block, has one, the SPRAM example two, the ready/valid
    blinky three.
  - What was done: `platform/vendor/fix_generated.py` declares the bit as
    `[0:0]`, for every flow alike. It is keyed on the exact text and
    leaves a file with two replicas alone.
  - Scope: the reference compiler emits the same, since the backend is
    shared, so this is no divergence of ours. A candidate for upstream.
- **Yosys promotes the runtime's register FIFOs into block RAM.** It folds
  the read-address register into a memory and maps a 16×8 FIFO to a whole
  EBR, against the backend's `USE_LUTRAM`. The icestorm flow marks every
  memory with no `ram_style` as logic, and the HAL marks its own memories
  explicitly (rule 6).
- **sv2v drops comments.** That includes the `translate_off` fences around
  the simulation-only code the backend and runtime emit (string tables,
  `real` arithmetic, dynamic assertions).
  `platform/vendor/icestorm/strip_translate_off.py` cuts those regions
  before sv2v, as a synthesis tool honours them.
- **This Verilator (5.052) drops an initialiser.** It ignores the
  declaration initialiser of a testbench variable that drives a netlist's
  input port, so the gate-level design never saw reset. The RTL testbenches
  had passed without reset ever asserting, so the reset path was untested.
  Testbenches now set such signals in `initial` blocks. Icarus Verilog
  simulated the same netlist correctly, which is how the cause was found.
- **First words are later in the block-RAM FIFO.** It shows a word pushed
  into an empty FIFO a cycle later than the register FIFO, as an XPM
  first-word-fall-through FIFO does; occupancy and `almost_full` count it at
  once. The protocol tolerates the delay (`empty` is polled), and the HAL
  test allows exactly that cycle.
- **Upstream's interface testbenches do not build under this Verilator**
  (class parameters without defaults in `sim_task_helpers.sv`). The HAL has
  its own test instead (§8.2).

### 8.6 Open

1. **On a UPduino.**
   - In `examples/spram`, `kg board bitstream`, then `kg board flash`,
     which writes the flash with `iceprog -d i:0x0403:0x6014`.
   - Expect green with blue blinking at ~1.4 Hz. Blinky cycles its eight
     colours every 2.8 s.
   - Confirm that green is RGB0 and red RGB2, as tinyVision's example has
     them.
   - The monitor: `kg board flash` in `examples/monitor`, then `host.py --port
     /dev/tty.usbserial-* test --stream`. First confirm DTR and RTS stay high
     with the port open, so the flash stays deselected (§8.7).
2. **The owner's board:** its constraints, per §7's recipe.
3. **`PROVISIONAL` values** (FIFO costs, the EBR-against-flip-flop cost):
   compare `kg board area` and nextpnr against the backend's resource report
   on larger designs.
4. **Dual-clock FIFOs in block RAM.** Today they are flip-flops; no pilot
   design crosses clocks.
5. **More examples:** a design whose memories go to EBR, to exercise the
   block-RAM FIFO and RAM on a board.
6. ~~**`kg build` reading backend options** (`-Z`, `[hw]`)~~ — built
   2026-10-17 ([`board-tooling.md`](board-tooling.md) §2.2).

### 8.7 The UART monitor

`examples/monitor` is the SPRAM read and written over the board's serial
port: `r AAAA` and `w AAAA DDDD`, a line each, one line back (`DDDD`, or `?`
for anything malformed). Fields are exactly four hex digits, so a reply is
never longer than its command. A host can therefore stream commands at the
line's full rate, and `host.py test --stream` does: 1,000 words, 2,000
commands, none waiting.

**The split.** The serial line is RTL, `rtl/uart.sv` (since 2026-10-17
`platform/rtl/periph/uart.sv`, and the core joining it generated): 8N1 at 208 clocks a
bit, which is smallest written by hand. The monitor is Kanagawa:
- `[[no_backpressure]] rx(uint8)` takes each byte and parses it. It runs
  the command through the SPRAM's single call site and stores the reply.
- `[[latency(0)]] tx_next()` gives the transmitter the next reply byte
  whenever it is idle.

The two share state with one writer per variable, so neither waits on the
other, and the interface has no FIFO. A reply that would overwrite one
still going out is dropped. That can only be a `?`, because no valid line is
shorter than the 6-byte reply before it.

**Size.** Each reply was first built as six bytes in `rx`: four hex
converters, a 48-bit buffer and a 6-way multiplexer for `tx_next`, 259 LUT4s
and 193 flip-flops. Keeping the 16-bit value and writing each byte in
`tx_next` as it goes out halves that, to 147 / 145 (`area.zsh` with the board
flags, 2026-10-17). The whole board design is 380 cells, 7% of the part.

**The serial pins, traced** in tinyVision's `Board/v3.0/UPduino_v3.0.kicad_pcb`
as shipped (R12, R13 fitted):
- FPGA pin 15 receives, from FT232H ADBUS0 (TXD).
- FPGA pin 14 transmits, to ADBUS1 (RXD) through R12.
- FPGA pin 16 shares net `/FT_SSn` with ADBUS4, the pin `iceprog` drives as
  the flash's chip select. In UART mode it is the FT232H's DTR# output, and
  the board's EEPROM configuration sets no other mode.

The shell therefore no longer drives pin 16; it is an input with the
pull-up. It used to drive it high, following the comment in tinyVision's
constraints, and so fought any terminal that asserts DTR. The host's part is
to keep DTR and RTS deasserted: DTR# low selects the flash, whose clock and
data lines are the serial lines, and whose output shares a net with RTS#.
`host.py` lowers both and leaves `HUPCL` off. With the board refitted for
CRAM programming (R11, R27), pin 14 reaches ADBUS2 instead and the serial
output is cut.

**Talking to the simulation.** `kg board serve`, in `examples/monitor`
(`scripts/board.sh upduino-v3 serve monitor` when this was written), runs
the core in Verilator with its serial pins on a pseudo-terminal
(`sim/uart_pty.cpp`, since moved to `platform/sim/`, through DPI-C). Any host program can open it as it
would the board's port. With a command after `--`, it runs the command
against the simulation and returns its status. So `host.py` is tested
against the RTL, not against a model of it.

That is how a host bug was found: the first streamed test sent every
command before reading a reply. About 1,200 unread bytes overflowed the
port's input buffer, which with no flow control loses data on a real port
too. `host.py` now drains replies while it sends.

## 9. For the owner

- **Where the layer lives:** in this repository (`platform/`, proposed), or
  a repository of its own pinned like the reference.
- **Whether the release archive carries it** (phase 2, item 1). It does
  since 2026-10-17, with `runtime/rtl`, so that an installed `kg board`
  works ([`board-tooling.md`](board-tooling.md) §2.3); revert if not wanted.
- **Whether `kg board sim` joins CI.** It needs Verilator 5 on the Linux
  runner, the corpus submodule for the runtime RTL, and the backend
  library. The last is the obstacle, since CI builds no backend today. The
  HAL's own check (`platform/rtl/hal/ice40/test/check.zsh`) needs no
  backend, only Verilator, sv2v and Yosys, and could join now.
- **Reporting the single-reset declaration upstream** (§8.5), with the
  two-line cause.
- **The manifest key and command names**: built as a `[board]` table and
  `kg board <command>` ([`board-tooling.md`](board-tooling.md) §2).
- **Vendor-derived data:** pin assignments are facts, cited and not copied.
  Confirm that is the line wanted before more boards arrive.
