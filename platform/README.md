# platform

Boards, devices and libraries for taking a Kanagawa design onto hardware,
or onto a simulated board when you have none, with `kg board`. It is all
opt-in: a project without a `[board]` table in its `kanagawa.toml` is
untouched by any of it.

- [Demo: pong in the browser](#demo-pong-in-the-browser)
- [Start with no hardware](#start-with-no-hardware)
- [How a design meets a board](#how-a-design-meets-a-board)
- [Talking to a design from Python](#talking-to-a-design-from-python)
- [Testing with cocotb](#testing-with-cocotb)
- [SPI, I2C and the serial port](#spi-i2c-and-the-serial-port)
- [The front panel, and the virtual board](#the-front-panel-and-the-virtual-board)
- [On a real board](#on-a-real-board)
- [Small designs on the iCE40](#small-designs-on-the-ice40)
- [Your own board](#your-own-board)
- [What is here](#what-is-here)

The design behind it, its measurements and what is still open:
[`docs/plans/board-tooling.md`](../docs/plans/board-tooling.md) (the
`kg board` tooling) and [`docs/plans/platform-layer.md`](../docs/plans/platform-layer.md)
(devices, HALs, boards).

## Demo: pong in the browser

The shortest way to see a design on a screen. With an installed `kg` (a
release archive, or `cargo xtask install` from a checkout with the backend
built; not `cargo install`, which carries no platform) and Verilator 5:

```sh
kg board new pong --board virtual --example pong
cd pong
kg board serve                     # prints `front panel: http://127.0.0.1:…/`; open it
```

From this checkout without installing, with the backend built (the top-level README's
"Build from source"):

```sh
cargo build --profile quick --bin kg
export KG_BACKEND_DIR=$PWD/target/backend/dist/bin
cd platform/examples/pong
../../../target/quick/kg board serve
```

The first run compiles the simulation, a minute or so. Then pong plays on a
640x480 screen drawn from the design's simulated VGA pins, about 27 frames a
second. W and S move the left paddle, ↑ and ↓ the right one (or click the
panel's buttons); a paddle left alone follows the ball, slowly enough to
miss. The score is on the row of LEDs, the left player's in the upper four.
P pauses, Restart starts a new game from power-on, and Ctrl-C in the
terminal stops it.

## Start with no hardware

You need `kg`, Verilator 5, and for the cocotb step, Python with cocotb 2
(`uv venv && uv pip install cocotb`, or `pip install cocotb`).

```sh
kg board list                                       # the boards, and the examples to start from
kg board new hello --board virtual --example accel  # a project of your own, from an example
cd hello
kg board interface                                  # what the design exports, and what serves each function
kg board test --python .venv/bin/python             # cocotb tests, driving the design's own ports
kg board serve -- python3 test_accel.py             # the same tests, over the simulated board's serial port
```

`accel` is a design with no RTL written for it at all. `accel.k` exports
a class with arithmetic methods and a callback; `kg board` builds it,
describes its interface, generates the board-level module around it, and
writes a Python class with one method per function:

```python
from accel import Accel                 # target/board/py/accel.py, generated
api = Accel()                           # connects to `kg board serve`'s simulation
api.add(2, 3)                           # 5, computed by the simulated hardware
api.on("_square", lambda x: x * x)      # answer the design's callback in Python
api.sum_of_squares(4)                   # 30: the design called Python four times
```

`kg board serve` with no command keeps the simulation running and prints
the address of the board's front panel. For `accel` it shows the RGB LED
and the bridge: each function, and its calls as `test_accel.py` makes them.
Try the `pong` example there for a live screen.

## How a design meets a board

A project names its board in `kanagawa.toml`:

```toml
[board]
name = "upduino-v3"                 # a board of the platform, or `path = "…"` for one of your own
connect = { step = "led" }          # optional: which function serves which resource
```

The board says what it offers in `board.toml`: its device, clock, flow and
**resources**, each on ports of the board's *core* module:

```toml
[resources.led]
kind = "led"
width = 3
pins = { out = "rgb" }

[resources.uart]
kind = "uart"
baud = 115200
pins = { rx = "uart_rxd", tx = "uart_txd" }
```

Each build generates that core module, `target/board/core/<core>.sv`, by
binding the design's exported functions to resources:
1. by `[board] connect`;
2. else by name: a method called `led`, a callback called `_uart_tx`;
3. else to the **host bridge**, which serves every remaining function over
   the board's serial port (next section).

Each kind of resource accepts certain shapes of method or callback (a `led`
takes a `[[latency(0)]]` method returning the LEDs' bits, called every cycle,
or a callback that sets them). A function of the wrong shape is refused with
the shapes that would fit. The full table:
[`board-tooling.md`](../docs/plans/board-tooling.md) §6.

`kg board interface` prints each binding; the build prints them too. To
write the core yourself, name it in `[board] rtl = ["core.sv"]` and no core
is generated: `boards/upduino-v3/examples/spram` does that.

## Talking to a design from Python

Every build writes `target/board/py/<module>.py` and `kgbridge.py` beside
it, both plain Python with no dependencies. The generated class has a method
per exported function, with its documentation and its arguments
range-checked against their declared types. Structs go in as dicts and come
back with their fields as attributes, and signed values come back signed.

The functions bound to nothing on the board are served by the **host
bridge**, a small dispatcher the generated core puts on the serial port
(SLIP frames; [`board-tooling.md`](../docs/plans/board-tooling.md) §7). The
same script talks to the simulated board and to the real one:

```sh
kg board serve -- python3 my_script.py                  # the simulation (BOARD_PTY)
python3 my_script.py                                     # with Accel(port="/dev/tty.usbserial-XXXX")
```

On connecting, the Python side checks that the board runs the build it was
generated from, and says so if not.

## Testing with cocotb

`kg board test` runs the project's `test_*.py` under cocotb against the
generated module itself: every function is reachable, bound to a board
resource or not. Write tests with `kgbridge.test` and they run in three
places unchanged:

```python
import kgbridge
from accel import Accel

@kgbridge.test(Accel)
async def adds(api):
    assert await api.add(2, 3) == 5

if __name__ == "__main__":
    kgbridge.main()
```

```sh
kg board test --python .venv/bin/python            # cocotb, cycle by cycle
kg board serve -- python3 test_accel.py            # the simulated board, over its serial port
python3 test_accel.py --port /dev/ttyUSB0          # the board
```

## SPI, I2C and the serial port

A design uses a bus by declaring the callback for it, named for the
resource, and handing it to a class from the peripheral library
(`library/periph/`):

```kanagawa
import periph.i2c

class Sensor
{
private:
    (uint2 op, uint8 data) -> uint9 _i2c;   // binds to the board's `i2c`
    bus _bus = {._i2c = _i2c};
public:
    uint9 config() { return _bus.read_reg(0x48, 1); }
}
```

| Library | Classes |
|---|---|
| `periph.spi` | `flash`: JEDEC ID, status, reads from any 25-series SPI flash |
| `periph.i2c` | `bus`: `probe`, `write_reg`, `read_reg`, `read_regs` |
| `periph.text` | hex digits and line endings, for a serial console |

The engines under them (`rtl/periph/`) are vendor-neutral SystemVerilog.
Under `kg board serve` the boards have a SPI flash and an I2C sensor on their
buses (`sim/models/`). Under cocotb, where the callback comes to Python,
`kgbridge.SpiFlash` and `kgbridge.I2cTarget` answer the same way. The
`spi-flash` and `i2c-sensor` examples read them.

A serial port binds like any resource: a method taking each received byte,
and a method the transmitter asks for the next byte (the UPduino's `monitor`
example), or callbacks the other way round.

## The front panel, and the virtual board

`kg board serve` puts your design, on a simulated board, in your browser:
it prints the address of a front panel the simulation serves itself. The
panel shows what the design uses and nothing else: a widget for each
resource its generated core binds, each naming the function that serves it,
with that function's `//|` documentation. The board's other resources are
listed underneath as unused. A project that brings its own core gets every
resource, since nothing says which it uses.

- **LEDs** light, with their value in hex beside them. **Buttons** and
  **switches** click; the first buttons also answer W S ↑ ↓ and 1–9.
- **A serial port** is a console: what you type and what the design sends,
  in order, what you sent in blue. Enter's line ending is yours to choose,
  and ↑ recalls earlier lines. The same port is a pseudo-terminal, named
  in the console's header and given to a command as `BOARD_PTY`, so
  everything that runs against a board runs against `serve` too.
- **Under the host bridge**, the port carries frames rather than text, so
  the console gives way to the bridge's functions. Each is listed with its
  signature and documentation, and a live count of calls as a host program
  makes them. The Python to start from is printed below them.
- **A bus** shows the device simulated on it (a SPI flash, an I2C sensor)
  and counts its transactions.
- **A screen** shows live, about 27 frames a second for pong, half of real
  time. The camera saves a frame as a PNG, and F fills the screen.
- **Pause** (P) holds the clock. **Reset** holds the design's reset for 16
  cycles, as a board's reset button would: a register the design never
  resets keeps its value, as on the FPGA. **Restart** starts again from
  power-on, every register at its initial value, at the same address; the
  page reconnects by itself. Restart is there when `serve` runs alone, since
  a command's `BOARD_PTY` would go stale.

`boards/virtual` is a board with no hardware: the UPduino's iCE40 UP5K at
25 MHz, with an RGB LED, eight LEDs, four buttons, eight switches, a serial
port, SPI and I2C with devices on them, and a 640x480 screen. Try the `pong`
example there.

**Any board can have a screen.** Declare a `vga` resource on the core's
ports, and route them to the connector in the board's shell and
constraints:

```toml
[resources.vga]
kind = "vga"
pins = { hsync = "vga_hsync", vsync = "vga_vsync", rgb = "vga_rgb" }   # 12-bit {r, g, b}
```

The generated core drives it from the design's `vga_pixel` method, and
`serve` reads it back off the pins for the panel. `test/vga-board` is such a
board, with no serial port at all.

**From a script**, `kgbridge.Panel` does what a person at the browser would:

```python
import kgbridge
panel = kgbridge.Panel()                    # BOARD_PANEL under `kg board serve`
panel.set(panel.find("button"), 0b10)       # hold the second button
print(panel.value("led"))                   # an LED resource's bits
panel.type("r 0010\r")                      # into the serial console
frame = panel.frame()                       # the next whole frame of the screen
frame.save_png("screen.png")
```

A project's `check_panel.py` runs that way: `kg board serve -- python3
check_panel.py`. Pong's plays the game through the panel and checks the
paddle follows the buttons; the monitor's types commands into the console.

## On a real board

```sh
kg board new blink --board upduino-v3            # the board's own `blinky`
cd blink
kg board info                                    # the board, its resources, which tools are found
kg board sim                                     # the testbench, in Verilator
kg board netsim                                  # the same testbench on the synthesised netlist
kg board bitstream                               # sv2v, Yosys, nextpnr, icepack
kg board flash                                   # iceprog writes the board's flash
kg board area                                    # the core's cells after synthesis
```

The iCE40 flow needs `sv2v`, `yosys`, `nextpnr-ice40` and `icestorm`
(Homebrew has all four, as does OSS CAD Suite); `kg board info` says which
are missing. Output lands in the project's `target/board/`.

On the UPduino, DTR# is the SPI flash's chip select: open the serial port
with DTR and RTS deasserted, as `kgbridge` and the monitor's `host.py` do
(with picocom: `--lower-dtr --lower-rts`).

In this checkout, run `kg` built here (`cargo build --profile quick --bin
kg`) with `KG_BACKEND_DIR` naming the backend, and set `KG_PLATFORM` to this
directory for `list` and `new` outside it. An installed `kg` (an archive's, or
`cargo xtask install`'s) carries the platform and needs neither; `kg board:
no platform` means a `kg` that was copied alone, as `cargo install` does.

## Small designs on the iCE40

The UPduino's page, [`boards/upduino-v3/README.md`](boards/upduino-v3/README.md),
records each choice made for the part (its pins, thread limit, DSP use,
synthesis options) with the reason and how to change it.

The UP5K has 5,280 logic cells. What decides whether a design fits
(with the numbers in [`board-tooling.md`](../docs/plans/board-tooling.md) §5 and
`platform-layer.md` §8.3):
- **`[[latency(N)]]` where nothing waits.** A method with a handshake keeps a
  FIFO for its arguments and one for its result. A fixed-latency or
  `[[no_backpressure]]` method has neither. The UPduino blinky is 95 LUT4s
  with a handshake `step` and 41 with `[[latency(0)]]`.
- **Few threads.** A call that waits on a callback saves every waiting
  thread's live variables, in a FIFO as deep as the thread limit. The
  UPduino and the virtual board set `max-threads-default = 4`; ask for more
  with `[[max_threads(N)]]` where it pays.
- **One call site in a loop**, not a call per step, when a method calls a
  callback many times; the peripheral library is written that way.
- Leave state uninitialised where zero will do: iCE40 flip-flops start at
  zero, and an initial value costs reset logic per bit.
- Route each SPRAM through one `access` call: the block has one port.

## Your own board

A board is a directory with a `board.toml`: its device, clock, flow,
constraints and resources, and the shell RTL that puts the core on the pins.
A board like the UPduino with other pins needs only its own `board.toml` and
constraints, reusing the UPduino's shell by path. A project names it with
`[board] path = "boards/mine"`. The recipe is `platform-layer.md` §7.

## What is here

| Directory | Holds |
|---|---|
| `boards/<board>/` | `board.toml`, constraints, the shell, the `serve` testbench, the board's own examples |
| `examples/` | examples for any board with the resources they use: `accel`, `spi-flash`, `i2c-sensor`, `pong` |
| `library/` | a Kanagawa import root in upstream's layout: devices, vendor defaults, `periph` |
| `rtl/hal/<hal>/` | a HAL for a family upstream's does not serve (`ice40`), with its check |
| `rtl/periph/` | the UART, SPI and I2C engines, the host bridge's link, VGA timing; their tests |
| `sim/` | the `serve` runtime: the front panel, the pseudo-terminal, the VGA capture, the clock, and models of SPI and I2C devices |
| `python/kgbridge.py` | the host side: the bridge, the cocotb driver, device models, the test runner |
| `test/` | the tooling's own tests: every protocol (`probe`), a second board with a screen (`vga-board`), a screen checker (`screen`) |
| `vendor/vivado/` | the Vivado flow's Tcl; the iCE40 flow is `kg board`'s own |
| `check.zsh` | every example through every flow, on every board it fits |

| Board | Device | Status |
|---|---|---|
| [`upduino-v3`](boards/upduino-v3/README.md) | `ice40-up5k` (`iCE40UP5K-SG48`) | every flow passes in simulation, gate level and synthesis; not yet run on a board |
| `virtual` | `ice40-up5k`, simulated | `sim`, `serve` with the front panel and a live screen, and `test` |
| `mimas-a7` | `artix7-xc7a50t` (`xc7a50tfgg484-1`) | `kg board sim` passes; not yet through Vivado or on hardware |

| Device | Part | Status |
|---|---|---|
| `ice40-up5k` | Lattice iCE40 UltraPlus UP5K | end to end on the open toolchain; values marked `PROVISIONAL` await more designs |
| `artix7-xc7a50t` | AMD Artix-7 XC7A50T | builds; values marked `PROVISIONAL` await Vivado reports |

A layer device also works without a board, from any `kg build`: name
`platform/library` after the standard library in `import-dirs` (or `-I`),
with `--target-device ice40-up5k`.
