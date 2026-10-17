# Board tooling: `kg board`, and what builds on it

*Part of the [plans](README.md). Status: steps 1–7 built 2026-10-17: `kg
board` (§2), the interface description, the peripherals, the generated core
and its host bridge, the Python module, cocotb, and the virtual board (§3–§9).
Builds on [`platform-layer.md`](platform-layer.md), which holds the devices,
HALs and boards this drives. The user's guide is
[`platform/README.md`](../../platform/README.md).*

The goal is a short path from nothing to a design running on a hobby
board, or on a simulated one, without writing glue RTL:

```sh
kg board new hello --board virtual --example accel   # a project; no hardware needed
cd hello
kg board test                                        # cocotb tests on the design itself
kg board serve -- python3 test_accel.py              # the same tests, over the board's serial port
kg board serve                                       # the front panel, in a browser
kg board new hello-up --board upduino-v3 --example accel
cd ../hello-up && kg board bitstream && kg board flash
python3 test_accel.py --port /dev/tty.usbserial-XXXX # the same tests, on the board
```

Everything here is **opt-in**. A project without `[board]` in its
`kanagawa.toml` resolves, checks and builds exactly as before, and nothing in
`kg check`, `kg build` or the language server reads a board unless a manifest
names one.

## 1. Decisions

1. **The project is the unit, and the board is named in its manifest.**
   `[board]` names a board, or a board directory of the project's own. A
   board's examples are projects too, so `kg board new` copies one and
   renames the board. `scripts/board.sh` took a board and an example name.
   That has no equivalent for a user's own design, so it is gone.
2. **A board brings a device, a platform library and backend options.**
   Applying a board sets `target-device` and appends the platform's
   `library/` after every other import directory, the bundled standard
   library included, so the layer adds and never shadows
   ([`platform-layer.md`](platform-layer.md) §3.1). The board's `[hw]`, with
   its clock as `frequency`, sits under the manifest's `[hw]`. A project that
   names another device is refused, not overridden.
3. **The layer reaches `kg` only through `[board]`.** The platform plan's
   phase 2 had the installed platform library appended for every project
   (`platform-layer.md` §4). It is not. A project that wants a layer device
   without a board names `platform/library` in `import-dirs`, as before.
4. **The flows are Rust, and the vendor's own language only where the
   vendor requires it.** `kg-board` runs Verilator, sv2v, Yosys, nextpnr,
   icepack and the programmer as child processes. Vivado keeps its Tcl. A
   user needs neither zsh nor Python for the flows; Python is the host
   side's language (§7, §8), and cocotb is needed only by `kg board test`.
5. **`kg build` and `kanagawa` hand the backend the same record for the
   same options.** `[hw]` and `-Z hw.<key>=<value>` are written through the
   compat binary's own flag table into a `LegacyOptions`, and validated by
   its checks (`config.cpp:162-207`). Keys and spelling follow
   [`spec/cli.md`](../spec/cli.md) §9.3.
6. **The interface is two sources checked against each other** (§4): the
   class's declaration for names, signs, attributes and documentation, the
   generated module's ports for names, widths and protocols. A port no
   function explains, or a function whose ports are missing, is an error.
   Every generator downstream reads the one description.
7. **A peripheral is RTL that speaks the callback protocol, and a library
   class that takes the design's callback** (§5). The pins cannot come out
   of the generated module through an object inside it, and a public
   nested object is refused in an exportable class ("Public field is not
   allowed in an exportable class"). A private object initialised with the
   exported class's own callback, `flash _flash = {._transfer = _spi};`,
   compiles, and the callback comes out as the module's ports.
8. **Functions bind to resources by name or by `connect`; what binds to
   nothing is served by the host bridge** (§6). A function whose shape fits
   no contract of its resource is refused with the contract spelled out,
   never wired approximately. A project naming its own core module in
   `[board] rtl` gets no generated one.
9. **One Python API, three transports** (§7, §8): the generated class runs
   over the bridge to the board, over the bridge to `kg board serve`'s
   simulation, and over the module's own ports under cocotb. A test written
   with `@kgbridge.test` runs in all three unchanged.
10. **The virtual board is a board** (§9): a `board.toml` with resources
    and a bridge testbench, whose C++ serves its own front panel. `kg`
    gains no network or GUI dependency; the page is one self-contained
    file.

## 2. Step 1 as built: `kg board` (2026-10-17)

### 2.1 The manifest

```toml
[board]
name = "upduino-v3"        # a board in the platform's boards/ …
# path = "boards/mine"     # … or a board directory, holding board.toml
# platform = "../platform" # where to look; default KG_PLATFORM, then the
                           # platform holding a `path` board, then the installation's
rtl = ["core.sv"]          # the project's RTL beneath the board's shell
testbench = "tb.sv"        # top `tb`, prints PASS
# top-module, core-module  # override the board's
```

`kg_config::board` (`crates/kg-config/src/board.rs`) holds:
- the schema of `[board]` and of `board.toml`, both refusing unknown keys;
- `resolve`, which finds the board, normalising `../..`;
- `apply`, which sets the device and appends the library;
- `Board::hw`, the board's backend options, with its clock as `frequency`.

`kg` (`crates/kg-cli/src/modern/settings.rs`, `resolve`) and the language
server (`crates/kg-lsp/src/global_state.rs`, `settings_for`) both apply it
after `or_bundled_library`. Each reads `KG_PLATFORM` once, at its front
door. In the editor, a board that cannot be found is reported once, and the
rest of the manifest still holds.

`board.toml` lost `[kanagawa] flags` (compat-binary argv) for `[hw]`, keyed
as the manifest's, and lost `[examples.*]`: each example directory is a
project with its own `kanagawa.toml` naming the board by `path = "../.."`.

### 2.2 `kg build` reads `[hw]`, `-Z` and `--emit`

Before this, all three were parsed and read by nothing
([`spec/cli.md`](../spec/cli.md) §9).

- **`[hw]` and `-Z hw.<key>=<value>`.** A key is a legacy long flag from
  `crates/kg-cli/src/modern/hw.rs`'s `KEYS`: §9.3's 54 backend options, less
  `resource-usage` and `dgml`, which are `--emit`, plus the placement
  constants, `stall` and `force-mem-to-array`. Values are typed as the flag
  is. A bare `-Z hw.<switch>` means `true`. An unknown key, a wrong type, or
  a `-Z` key outside `hw.` is a usage error (exit 2); before, it was
  silently ignored.
- **Precedence:** the reference's defaults, then the board's `[hw]`, then the
  manifest's (profile applied), then `-Z`, then `-O` and `--debug`. `-O` no
  longer defaults to 2 on the command line, so `[hw] optimize` can apply.
- **`--emit resource-usage=<FILE>` and `dgml=<FILE>`** reach the backend.
  The backend names a report per module (`<stem><Module>.<ext>`) and asserts
  on a name with no dot (`compiler.cpp:2140`), so `kg build` refuses such a
  name first. Other kinds (`ir`, `mlir`, `dep-list`) are refused as not
  built.
- **The backend's own warnings** (`_enableTransactionSizeWarning`,
  `_enableDeprecationWarnings`, `_warningsAsErrors`) now come from the
  project's warning settings, as `kanagawa`'s do. `kg build` used to pass
  the transaction-size warning on regardless, where the reference has it off
  by default (`CmdArgs.hs:111-117`).

**Identity with the compat binary** (2026-10-17). The monitor example was
compiled by `kanagawa` with the UPduino's five backend options as flags, and
by `kg build` with the same five as `-Z hw.*`. The resource reports, the
path-length report, the RTL map and `Monitor_types.sv` are byte-identical.
`Monitor.sv` differs in its command-line header and in the order of one
comment's source-location list. That order also differs between runs of
`kanagawa` itself: three runs gave two hashes. That is the backend's own
nondeterminism, not this change.

### 2.3 `kg board`

| Command | Does |
|---|---|
| `list [--platform DIR]` | the boards, their devices and examples |
| `info` | the project's board, device, HALs, runtime, and each tool its flows need, found or missing |
| `new DIR --board NAME [--example NAME]` | copies an example (default `blinky`); the copy's manifest names the board by name, and every path in it is re-expressed from the copy, or left out where the installation supplies it |
| `bundle` | builds, then writes `sources.f`, `constraints.f`, `board.env`, `board.tcl` for a flow run elsewhere |
| `sim` | builds, then runs the testbench under Verilator against upstream's runtime, the simulation HAL, the board's and the project's RTL |
| `netsim` | builds, synthesises the core, then runs the same testbench on the netlist with the vendor's cell models (open flows only) |
| `bitstream` | builds, then the vendor flow: sv2v, Yosys, nextpnr-ice40, icepack, with a utilisation and Fmax summary, or Vivado's batch Tcl |
| `flash` | the programmer (`[program] command`, or Vivado over JTAG) with the last bitstream |
| `serve [-- CMD…]` | builds, then the core under the board's bridge testbench with its serial port on a pseudo-terminal (`BOARD_PTY`); with a command, runs it and exits with its status |
| `area [--top M]` | builds, synthesises the core (or `M`), and prints its LUT4s, flip-flops, carries, EBR, SPRAM and DSP |

**Building.** Every command that needs RTL builds through `kg build`'s own
path, into `<project>/target/board/gen/`. It then repairs the generated RTL
(`kg_board::repair::generated`, the single-reset declaration of
`platform-layer.md` §8.5). Upstream's runtime RTL is found beside the
standard library the project imports, which is upstream's layout and the
installation's, or else under `KANAGAWA_ROOT`.

**Retired:** `scripts/board.sh`, `platform/vendor/icestorm/build.zsh` and
`area.zsh`, and `platform/vendor/fix_generated.py`. The translate-off filter
stays as Python only beside the HAL's own check
(`platform/rtl/hal/ice40/test/strip_translate_off.py`), the one script that
still uses it.

**Installed.** `cargo xtask dist` ships `share/kanagawa/runtime/rtl` and
`share/kanagawa/platform`, leaving out any `target/` an example built. An
installed `kg` finds both with nothing configured.

### 2.4 Checked (2026-10-17)

- **Every flow of every example.** `platform/check.zsh` replaces
  `board.sh`'s per-example runs and passes everything:
  - each example's `sim`;
  - the UPduino examples' `netsim`, `bitstream` and `area`;
  - the monitor's `serve` with `host.py test --count 200 --stream`;
  - the HAL check, 8 PASS lines.

  The figures equal the `board.sh` figures (`platform-layer.md` §8.2):
  - blinky: 55 cells at 73.2 MHz;
  - spram: 159 cells at 59.3 MHz;
  - monitor: 380 cells (7.2%) at 42.8 MHz.

  The cores' area: blinky 41 LUT4 / 38 FF; spram 103 / 96 with 2 SPRAM;
  monitor 213 / 198 with 2 SPRAM.
- **A project outside the checkout.** `kg board new` from the
  scratchpad, then `kg board sim` on the monitor copy: PASS.
- **The release archive.** `cargo xtask dist` (13.3 MiB) was unpacked
  outside the checkout and run with `env -i` (only `HOME` and a `PATH` with
  the archive's `bin` and Homebrew's tools). `kg board list`, `kg board new
  work/blink --board upduino-v3`, `info`, `sim`, `bitstream` and `area` all
  worked: PASS; 55 cells at 73.2 MHz; 41 / 38. The new project's manifest
  holds no path at all.
- **Tests**, each proved with `scripts/mutant.sh` (§2.5):
  - `kg-config` `board::tests`: 9, covering resolution, precedence, apply,
    clock and typos;
  - `kg-cli`: `modern::hw::tests` (6), the backend record against
    `kanagawa`'s, the `--emit` and unknown-option refusals, and `[board]`
    resolution with its `[hw]` layers and without a board;
  - `kg-lsp`: `a_board_sets_the_device_and_the_platforms_library`;
  - `kg-board`: repairs, translate-off, file lists, ordering, the nextpnr
    summary, the Yosys cell count, `kg board new`'s manifest, and relative
    paths.

### 2.5 Mutants

`scripts/mutant.sh`, 39 mutants planted (2026-10-17), one per line a test
guards. All 39 are caught, each red a test failure:
- 7 in `kg-config/src/board.rs`;
- 6 in `hw.rs`;
- 4 in `kg build`'s record and `--emit` check;
- 2 in the CLI's `[board]` resolution;
- 2 in the language server;
- 18 in `kg-board`.

Three needed work first:
- `brd-space` survived: the pragma match lost its whitespace rule. A test now
  has `synopsys` and `translate_off` with prose between them.
- `cfg-both` and `brd-dedup` first went red as compile errors, not test
  failures. Each was replanted as a mutation that compiles.

## 3. What builds on it

| Step | What | Where | § |
|---|---|---|---|
| 2 | The interface description of the exported class | `crates/kg-board/src/interface.rs` | 4 |
| 3 | Peripherals: SPI and I2C engines, the UART, a library, device models | `platform/rtl/periph`, `platform/library/periph`, `platform/sim/models` | 5 |
| 4 | The generated core, and the host bridge it can carry | `crates/kg-board/src/wrapper.rs`, `platform/rtl/periph/bridge.sv` | 6 |
| 5 | The Python module, and `kgbridge` | `crates/kg-board/src/python.rs`, `platform/python/kgbridge.py` | 7 |
| 6 | `kg board test`: cocotb over the same API | `kg_board::test`, `kgbridge.Cocotb` | 8 |
| 7 | The virtual board | `platform/boards/virtual`, `platform/sim/vboard.*` | 9 |

The commands step 1 did not have: `kg board interface [--json]` prints the
description and the bindings; `kg board test [--python PATH]` runs cocotb.
`kg board new` and `list` take the platform's own examples
(`platform/examples/`) as well as a board's. The manifest's `[board]` gains
`connect` and `tests`; `board.toml` gains `[resources.*]`
([`spec/cli.md`](../spec/cli.md) §10).

## 4. The interface description

Every `kg board` build writes `target/board/interface.json` beside the
generated RTL, and `kg board interface` prints it. One entry per public
method and per callback, each with its name, index, documentation,
protocol, latency, arguments, result and the generated module's port for
each role:

```json
{ "name": "rx", "callback": false, "index": 0, "doc": "One received byte.",
  "protocol": "no_backpressure", "latency": null,
  "args": [{ "name": "c", "type": "uint8", "width": 8, "signed": false, "fields": [] }],
  "result": null,
  "ports": { "valid_in": "rx_valid_in", "valid_out": "rx_valid_out", "args": ["rx_c_in"] } }
```

**The protocols are the backend's**, read off its port list and checked
against the declaration's attributes:

| | Ports | Backend |
|---|---|---|
| method, handshake | `<m>_valid_in`, `<m>_<arg>_in`, `<m>_rdy_out`; results through a show-ahead FIFO, `<m>_empty_out`, `<m>_rden_in`, `<m>_result_out` (a `void` method's FIFO carries completion alone) | `verilog.cpp:6465-6539` |
| method, `[[no_backpressure]]` | `<m>_valid_in`, arguments; `<m>_valid_out` with the result | same |
| method, `[[latency(N)]]` | `<m>_valid_in`, arguments; `<m>_result_out` N cycles later | same |
| callback, handshake | arguments in a FIFO, `<cb>_empty_out`, `<cb>_rden_in`, `<cb>_<arg>_out`; the answer pushed with `<cb>_valid_in`, `<cb>_result_in` while `<cb>_rdy_out` | `verilog.cpp:6582-6662` |
| callback, `[[async]]` | the argument FIFO alone | same |
| callback, `[[no_backpressure]]` | a pulse, `<cb>_valid_out`, with the arguments | same |
| callback, `[[latency(N)]]` | a pulse, and `<cb>_result_in` due N cycles later | `verilog.cpp:7529-7556` |

**Widths and layout are the ports'**, including a packed struct's, whose
first declared field is lowest (the backend writes `struct packed {logic
[3:0] b; logic [7:0] a;}` for `struct { uint8 a; int4 b; }`). **Signs,
names, types and documentation are the declaration's**, following `using`
aliases and into struct fields. The declaration is read from the project's
own sources with `kg-syntax`; a class declared elsewhere is refused by name.
A `[[reset]]` method has no ports and is left out.

Answered here (§10 of the plan as written): an exported class's callbacks
are its module's ports; a nested public object's are not, because the
frontend refuses public objects in an exportable class.

## 5. Peripherals

Each peripheral is an engine, a contract, a library and a model.

**Engines** (`platform/rtl/periph/`, in every board's sources through
`files.f`): vendor-neutral SystemVerilog that sv2v and Yosys read. The SPI
and I2C engines speak the callback protocol directly, so a generated core
connects their call side port for port.

| Engine | Contract a design declares | LUT4 / FF (UP5K, alone) |
|---|---|---|
| `uart.sv` (`uart_rx`, `uart_tx`), moved from the UPduino's `rtl/` | see the `uart` kind, §6 | 82 / 70 and 66 / 42 |
| `spi_master.sv` | `(uint8 mosi, bool hold) -> uint8 _spi`: a byte each way, chip select held while `hold`; all four modes, `CLK_DIV` half-periods | 40 / 21 |
| `i2c_master.sv` | `(uint2 op, uint8 data) -> uint9 _i2c`: op 0 start (repeated if the bus is ours), 1 stop, 2 write (result bit 8: not acknowledged), 3 read (`data` bit 0: acknowledge it); clock stretching | 79 / 34 |
| `bridge.sv` (`kg_bridge_link`) | the host bridge's link, §7 | 276 / 306 in the accel example |
| `vga_timing.sv` | 640x480 timing for the `vga` kind | — |

The plan's I2C contract had five operations in a two-bit op; READ's
acknowledge moved into `data` bit 0 to fit. The engines, their testbenches
(`platform/rtl/periph/test/`: `run.sh`, `mutate.sh`, `synth.sh`) and the
device models were written to this contract and checked alone: every
testbench passes, and all 12 planted bugs (7 SPI, 5 I2C) are caught
(2026-10-17, `mutate.sh`). The I2C netlist passes gate-level with Yosys's
cell models.

**The library** (`platform/library/periph/`): `periph.spi` has `flash`
(JEDEC ID, status, reads), `periph.i2c` has `bus` (`probe`, `write_reg`,
`read_reg`, `read_regs`), `periph.text` has hex digits for a console. Each
class takes the design's callback:

```kanagawa
import periph.spi

class Reader
{
private:
    (uint8 mosi, bool hold) -> uint8 _spi;   // binds to the board's `spi` by name
    flash _flash = {._transfer = _spi};
public:
    uint24 id() { return _flash.jedec_id(); }
}
```

**Models** of the far end: `platform/sim/models/spi_flash_model.sv` (a
W25Q32's JEDEC ID, READ, status) and `i2c_target_model.sv` (a TMP102-like
register file at 0x48, with optional clock stretching), on the UPduino's and
the virtual board's `serve` buses; and `kgbridge.SpiFlash` and
`kgbridge.I2cTarget`, the same two at the callback's level, for cocotb, where
the callback comes to Python. The `spi-flash` and `i2c-sensor` examples'
tests read the same bytes through either.

**What a callback with a result costs** was the step's first measurement.
Each call site that waits on a callback keeps the live variables of every
thread that may be waiting, in a context saver whose FIFO is as deep as the
function's thread limit, 512 by default (`--max-threads-default`). On a UP5K
(5,280 logic cells), with the board's earlier options (2026-10-17, `kg board
area`):

| Design | default threads | `max-threads-default` 4 | 1 |
|---|---|---|---|
| SPI and I2C together, library written call by call | 155,798 LUT4 | — | — |
| the same, library as loops (one call site a method) | 122,879 | 3,811 | 3,478 |
| `accel`, before its pure methods were fixed-latency | 3,164 | 2,964 | 2,891 |
| `accel`, as shipped (and the DSP fix, §10) | 1,882 | 1,721 | — |

Two buses with five methods and the bridge still did not place on the UP5K
at one thread (82% of the logic cells and every block RAM), so the example
split in two. `spi-flash` with one thread places at 3,992 cells (75.6%), 35.0
MHz.

So:
- **The thread limit decides it.** A board for a small part sets
  `max-threads-default` (the UPduino and virtual board now say 4, §6); a
  design that wants more names `[[max_threads(N)]]` where it needs them.
- **One call site a method.** The library's methods loop over their bytes
  with one call of the callback rather than one call a byte.
- **Fixed latency where nothing waits.** A handshake method keeps a FIFO for
  its arguments and one for its result; at 32 bits wide those are flip-flop
  FIFOs of a few hundred cells. `accel`'s pure methods became
  `[[latency(N)]]` and dropped 900 LUT4s.
- **One thread where nothing runs in parallel.** The bus examples set
  `max-threads-default = 1` in their own `[hw]`: the library runs one
  transaction at a time, so more threads buy nothing.

Three faults turned up on the way, none in this step's code but each in the
way of it (§10).

**The UP5K's hard SPI and I2C blocks** (`SB_SPI`, `SB_I2C`) stay later work,
behind the same contracts.

## 6. The generated core

A board declares what it offers a design in `board.toml`:

```toml
[resources.uart]
kind = "uart"
doc = "the FT232H's serial port, the board's USB connection"
baud = 115200
pins = { rx = "uart_rxd", tx = "uart_txd" }   # the core module's ports
```

When the board has resources and the project names no core module of its
own in `[board] rtl`, every build writes `target/board/core/<core>.sv`: the
generated module wired to an engine per resource, and the shell
instantiates it as it would a hand-written one. Unused resources sit idle.
The build prints each binding; `kg board interface` too.

**Binding.** By `[board] connect` (`rx = "uart.rx"`, `"bridge"`, or `"none"`
for a method left uncalled), else by name: `led`, `uart_rx`, `_uart_tx`
(leading underscores aside), `vga` for `vga.pixel`. Two functions on one role
are refused; so is a `connect` naming a function the class lacks, a resource
the board lacks, or a kind's role without saying which (`uart` has two).

**Kinds and their contracts.** Each role accepts the shapes listed, and a
function of another shape is refused with the list:

| Kind | Role | Accepts |
|---|---|---|
| `led` (`width` W) | the LEDs | a `[[latency(N)]]` method with no arguments returning up to W bits, called every cycle; or a callback taking up to W bits, of any protocol, latched |
| `button` (`width` W) | the buttons | a callback with no arguments returning W bits or more (fixed latency or handshake), answered at once; or a method taking W bits or more, called once started and on every change |
| `uart` | `rx` | a method taking a `uint8` per byte (`[[no_backpressure]]` or `[[latency]]`; with a handshake a byte arriving while it is not ready is dropped); or a callback `() -> uint8`, answered with the next byte (one held, as a UART overruns) |
| | `tx` | a `[[latency(0)]]` method `() -> uint9` asked while the transmitter is idle (bit 8: a byte to send); or a callback taking a `uint8` (with a handshake it waits; without, a byte sent while busy is dropped) |
| `spi` (`hz`, `mode`) | the bus | the callback `(uint8, bool) -> uint8`, handshake |
| `i2c` (`hz`) | the bus | the callback `(uint2, uint8) -> uint9`, handshake |
| `vga` | `pixel` | a `[[latency(N)]]` method `(uint10 x, uint10 y)` returning 12 bits `{r, g, b}` or 3; sync and blanking are delayed by N to match |
| | `frame` | a method with no arguments, called once a frame after the visible area |

**The examples.** The UPduino's `blinky` and `monitor` lost their
hand-written cores to `connect` lines; `spram` keeps its own, the example of
replacing the generated one. The generated cores' figures equal the
hand-written ones within placement noise (2026-10-17, `kg board bitstream`;
`area` for LUT4 / FF):

| Example | Before | Generated |
|---|---|---|
| blinky | 55 cells, 73.2 MHz; 41 / 38 | 55 cells, 73.2 MHz; 41 / 38 |
| monitor | 380 cells, 42.8 MHz; 213 / 198 | 375 cells, 45.0 MHz; 215 / 198 |

The UPduino's shell gained an SPI bus on header pins 44-47 and an I2C bus on
2 and 3 (open drain through `SB_IO`, pull-ups on), for its `spi` and `i2c`
resources.

**The board's backend options for a small part.** The UPduino's and the
virtual board's `[hw]` now add `max-threads-default = 4` (§5). It changes
blinky, monitor and spram not at all (identical cells, 2026-10-17, `kg
board area`). `block-ram-util-threshold = 0` was measured too: the backend
leaves a FIFO in flip-flops unless it would fill 20% of a block RAM, and on
an iCE40 the logic cells run out first. It takes accel from 1,721 LUT4s to
1,240, but the HAL's block-RAM FIFOs then use more EBRs than the backend
budgets (40 against its 29 for the two-bus example at 4 threads), and the
design does not place. It stays at its default until that is understood
(§13).

## 7. The host bridge and the Python module

**The bridge.** When functions bind to nothing and the board's `uart` is
free, the generated core carries a dispatcher over `kg_bridge_link`
(`platform/rtl/periph/bridge.sv`): SLIP frames over the serial port, one
frame held each way.

- Host to board: `[i][args]` calls method `i`; `[0x80|j][result]` answers
  callback `j`; `[0xFF]` asks hello.
- Board to host: `[i][result]` when method `i` completes; `[0x80|j][args]`
  calls callback `j` on the host; `[0xFF][1][hash]` answers hello;
  `[0xFE][code][op]` refuses a frame (1 unknown, 2 wrong length, 3 lost).
- Values take whole bytes each, little-endian, in argument order.
- Every protocol is served: a handshake method's result is read from its
  FIFO, a `[[no_backpressure]]` one's on its pulse, a `[[latency(N)]]` one's
  N cycles after the call. A handshake callback is forwarded and waits for
  the host's answer; a pulse callback is forwarded as it fires. A fixed-
  latency callback's result cannot wait for a host and is answered 0.
- Calls out go before replies, so a callback made during a method reaches
  the host before that method's reply.
- The hash is FNV-1a over the interface and the bridged functions; the
  Python module carries the same, and refuses a board running another build:
  "flash the bitstream this module was generated with, or rebuild".

**The Python module.** Every build writes `target/board/py/<module>.py`
(`accel.py` for `Accel`) and copies `kgbridge.py` beside it. One class, one
method per function with its documentation, the interface embedded,
arguments range-checked against their declared types, structs as dicts (or
anything with the fields as attributes) and returned as `Struct`s:

```python
from accel import Accel
api = Accel()                          # BOARD_PTY under `kg board serve`; Accel(port=...) on a board
api.add(2, 3)                          # 5
api.on("_square", lambda x: x * x)     # the design calls back into Python
api.negate({"x": 300, "y": -7})        # Struct(x=-300, y=7)
```

`kgbridge` is standard library only (POSIX termios), except its cocotb
transport. The port opens raw with DTR and RTS deasserted, as the UPduino's
flash wants. Calls block, serving callbacks meanwhile; with
`Serial(asynchronous=True)` each returns a coroutine instead (§8).

## 8. cocotb: `kg board test`

`kg board test` builds, then runs the project's cocotb test modules
(`test_*.py` beside the manifest, or `[board] tests`) against the generated
module itself, under Verilator, through cocotb 2's runner. Every function is
reachable, bound to a board resource or not. `--python` names the Python
with cocotb (a virtual environment's); without cocotb it says how to install
it, and nothing else in `kg` needs it.

`kgbridge.Cocotb` drives the ports one cycle at a time: at each falling edge
it drives what this cycle asks, and once the cycle has settled (`ReadOnly`)
it reads what the design answered, so a transfer it sees happens at the next
rising edge. It speaks every protocol of §4; a callback is answered by the
handler `api.on` registered, synchronously where it can be. Ports are reached
by name, since cocotb hides attributes beginning with an underscore, as every
callback's ports do.

**One test, three places.** A test is `async def test(api)` under
`@kgbridge.test(Device)`. Under cocotb the decorator makes it a cocotb test
(with a simulated-time limit, 10 ms by default, so a design that never
answers fails rather than hangs); run as a script, `kgbridge.main()` runs
every test over the serial bridge, to `kg board serve`'s pseudo-terminal or
to `--port`. `platform/test/probe` exercises every protocol of §4 through
both transports, and the bytes SLIP escapes each way.

## 9. The virtual board

`platform/boards/virtual` is a board with no hardware: the UPduino's
device, a 25 MHz clock, and every resource kind: an RGB LED, eight LEDs,
four buttons, eight switches, a serial port, SPI with a flash, I2C with a
sensor, and a 640x480 screen. `sim`, `serve` and `test` run; `bitstream`,
`flash`, `netsim` and `area` say it is simulated only.

Its bridge testbench (`sim/vboard.sv`) puts the serial port on a
pseudo-terminal, as the UPduino's does, so kgbridge and host programs run
against it unchanged. It reads the screen off the VGA pins as a monitor
would. `platform/sim/vboard.cpp` serves a front panel on 127.0.0.1 from
inside the simulation; `kg board serve` prints its address and passes it to
a command as `BOARD_PANEL`. The page (`platform/sim/vboard.html`, one
self-contained file) draws the screen, the LEDs, buttons (click, or W S ↑ ↓)
and switches, and a serial console that also types into the design.

**Speed.** The plan judged VGA at about 7 frames a second from blinky's
simulation speed. Pong (`platform/examples/pong`: no RTL, no frame buffer;
the picture a function of a few registers that `vga_frame` updates) runs at
9.2 to 10.4 frames a second, the simulation at 3.8 to 3.9 MHz, some 15% of
real time (2026-10-17, `kg board serve -- python3
platform/test/screen/check_screen.py`, Apple M-series, Verilator 5.052).

`platform/test/screen/check_screen.py` checks a panel from outside: whole
frames, pixels, the rates. Pong's paddles, net and background pass it.

## 10. Found on the way

- **The iCE40 HAL's block-RAM and dual-clock FIFOs dropped almost_full the
  cycle reset fell.** The runtime asserts it stays high that cycle
  (`write_delay_fifo.sv:74`), and the first design with a deep FIFO, `accel`'s
  callback loop, stopped on it. Both now hold full and almost_full for a
  cycle after reset, as the register FIFO's registered flag does; the HAL
  check asserts it (two mutants caught).
- **No UP5K design reached the DSP blocks.** The device's config said a zero
  intrinsic width makes the backend emit `*`; it emits `KanagawaLutMul`
  (`ir.cpp:6551-6560`), which the HAL built in logic. `KanagawaLutMul` now
  instantiates an `SB_MAC16` for products up to 16x16 (signed and unsigned,
  checked against Yosys's own model), and wider ones are `*`.
- **Yosys 0.69's `ice40_dsp` crashes** (exit 139) on a 16x16 product, inferred
  or instantiated, followed by two register stages: exactly the backend's
  pipeline. `kg board` now synthesises without `-dsp`; every Kanagawa product
  reaches the DSP through the HAL's instance, and a `*` written by hand in
  project RTL is logic. The HAL check keeps the crashing shape as a case.
- **`flash.read`'s counter was a bit short** (to 8 in a `uint3`); the cocotb
  run hung rather than failed, which is why kgbridge's tests now have a time
  limit.

## 11. Checked (2026-10-17)

`platform/check.zsh` (2026-10-17, with `KG_COCOTB_PYTHON` naming a
virtual environment with cocotb 2.1.0; Verilator 5.052, Yosys 0.69,
nextpnr-ice40): 39 steps pass, and one is skipped by design, pong copied to
the UPduino, which has no screen ("no resource `vga`").

| Project | Board | What passed |
|---|---|---|
| upduino-v3 `blinky` | upduino-v3 | sim, netsim; 55 cells, 73.2 MHz; 41 LUT4 / 38 FF |
| upduino-v3 `monitor` | upduino-v3 | sim, netsim (261 reply bytes); 375 cells, 45.0 MHz; 215 / 198, 2 SPRAM; `host.py test --count 200 --stream` under `serve` |
| upduino-v3 `spram` | upduino-v3 | sim, netsim; 159 cells, 60.1 MHz; 100 / 96, 2 SPRAM |
| mimas-a7 `blinky` | mimas-a7 | sim |
| `accel` | virtual; upduino-v3 copy | cocotb 4/4, bridge 4/4 on both; 3,214 cells (60.9%), 32.3 MHz; 1,669 / 2,067, 4 EBR, 1 DSP |
| `spi-flash` | virtual; upduino-v3 copy | cocotb 1/1, bridge 1/1 on both; 3,985 cells (75.5%), 30.3 MHz |
| `i2c-sensor` | virtual; upduino-v3 copy | cocotb 1/1, bridge 1/1 on both; 3,479 cells (65.9%), 34.1 MHz |
| `pong` | virtual | the front panel: 11 frames at 9.5 frames a second, 3.98 MHz simulated; the net, the background and both paddles' columns |
| `test/probe` | upduino-v3 | cocotb 5/5, bridge 5/5; 2,304 cells, 29.7 MHz |
| `rtl/periph/test` | — | 6 PASS lines (SPI, and I2C at five speeds and stretches) |
| `rtl/hal/ice40/test` | — | 10 PASS lines (the HAL, 9 mapping checks) |

Also checked:
- **Tests**: `cargo test --workspace --profile quick` and clippy with
  `-D warnings` pass. The new unit tests:
  - `kg-board`: the interface (7), the generator (5), the Python names, and
    `kg board new`'s manifest edits and shared examples;
  - `kg-config`: resources, `connect`, `tests`;
  - `kg-cli`: every board command parses.
- **The front panel in a browser's place**: the page served, its script
  parsed (`node --check`), `/state`, `/frame` (614,400 bytes, 640×480×2) and
  `/input` exercised over HTTP. A frame rendered to an image shows the
  paddles, ball and net.
- **The release archive** (`cargo xtask dist`, 13.5 MiB): unpacked outside
  the checkout and run with `env -i` (only `HOME`, and a `PATH` of the
  archive's `bin` and Homebrew's tools). `kg board list` shows the three
  boards and the four shared examples; `kg board new hello --board virtual
  --example accel` writes a manifest with no path in it; `interface` prints
  the bindings; `test` passes cocotb 4/4 (with `--python` naming the
  virtual environment) and `serve -- python3 test_accel.py` the bridge 4/4;
  pong's front panel passes its screen check at 9.4 frames a second; the
  UPduino's blinky simulates and builds to 55 cells at 73.2 MHz. The
  archive holds no `target/` and no `__pycache__` (`dist` now skips both).

## 12. Mutants (2026-10-17)

`scripts/mutant.sh`, one mutant per guarded line, the check spelled out
after `--` (2026-10-17). All 28 are caught, each red a failing test or a
failing run, none a compile error (three checked in the log):
- **Rust, 18**: in `kg-board`, 7 in the interface (struct field order, sign,
  alias, unclaimed ports, claimed arguments, a fixed-latency callback,
  array widths), 6 in the generator (the underscore in a name, a role bound
  twice, an LED's width, `started` gating, the bridge's call-first order,
  argument offsets), the Python names, and `kg board new`'s shared examples
  and edited `path`; one each in `kg-config` (`tests` paths) and `kg-cli`
  (`--python`'s default).
- **Platform, 6, each against a simulation**: the bridge link's byte
  placement and its SLIP decoding (the probe over the bridge), kgbridge's
  SLIP encoding (the same), the cocotb driver's fixed-latency timing (the
  probe under cocotb), the library's read counter (`spi-flash` under
  cocotb), and the front panel's horizontal timing (pong's screen check).
- **The HAL, 4**: almost_full after reset in both FIFOs, the DSP block's
  signedness, and the DSP branch itself (the mapping count).
- **Not counted here**: the peripheral engines' own 12 (`rtl/periph/test/mutate.sh`, §5).

One survived first: the cocotb driver read a fixed-latency result a cycle
late and the probe still passed, because the driver left a call's arguments
on the ports after it, so the late read saw the same answer. The driver now
drives idle arguments to zero, and the mutant is caught.

## 13. Open

- **On hardware.** Everything here is simulated, gate-level for the
  UPduino's netlists; nothing has run on a board yet.
- **The UP5K's hard SPI and I2C blocks**, behind the same contracts.
- **The HAL's block-RAM FIFOs use more EBRs than the backend budgets**
  (§6): 40 against 29 in one design. Until the two agree, a board cannot
  lower `block-ram-util-threshold`, which would save a quarter of the logic
  in callback-heavy designs.
- **Wider products** go to logic; splitting them across SB_MAC16s in the HAL
  is the next step if a design wants it.
- **A Kanagawa class declared outside the project's sources** cannot be
  described yet, nor a template instance (`export Foo<8>`).
- **The front panel for the UPduino's `serve`**: its bridge has LEDs to show
  and could use `vboard.cpp` as the virtual board does.
- **Whether `kg board sim` and `test` join CI**: they need Verilator 5,
  Yosys's cell models and cocotb on the runner, while CI builds no backend
  today (AGENTS.md).
- **The owner's custom UP5K board**: a board directory of the project's own
  (`[board] path`), reusing the UPduino's shell by path
  (`platform-layer.md` §7).

## Progress

- 2026-10-17: step 1 built and checked (§2).
- 2026-10-17: steps 2–7 built and checked (§4–§9, §11); three faults in the
  platform layer found and fixed on the way (§10).
