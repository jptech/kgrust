# UPduino v3 (iCE40 UP5K)

tinyVision's [UPduino v3.0/v3.1](https://github.com/tinyvision-ai-inc/UPduino-v3.0):
an iCE40UP5K-SG48 (5,280 logic cells, 30 block RAMs, 4 SPRAMs, 8 DSP
blocks) with an FT232H for USB programming and a serial port. The reference
board for the platform's small parts; everything here runs on the open
toolchain (Yosys, nextpnr-ice40, icestorm). Not yet run on hardware.

```sh
kg board new blink --board upduino-v3       # the board's `blinky`
kg board new hello --board upduino-v3 --example accel
```

## What a design can use

| Resource | Kind | On | Notes |
|---|---|---|---|
| `led` | 3 LEDs | the RGB LED, `{red, green, blue}` | through `SB_RGBA_DRV`, lowest current |
| `uart` | serial port | the FT232H's UART, 115200 baud | the board's USB connection; the host bridge's port |
| `spi` | SPI bus | header pins 44 (SCLK), 45 (MOSI), 46 (MISO), 47 (CS#) | 1 MHz, mode 0 |
| `i2c` | I2C bus | header pins 2 (SCL), 3 (SDA) | 100 kHz, open drain, internal pull-ups |

The clock is the internal 48 MHz oscillator divided to 24 MHz (`SB_HFOSC`),
so no jumper is needed. `kg board info` lists the resources too.

## Decisions, and how to change them

Each was made for a reason measured on this part; each is a line to change
if your design wants otherwise.

**The SPI and I2C pins are a choice.** Header pins 44–47 and 2–3 were free
and nothing on the board uses them; no breakout dictated them. To move a
bus, change its `set_io` lines in `constraints/upduino-v3.pcf`; the shell
and the generated core name ports, not pins, so nothing else changes. A
board of your own (`[board] path`) brings its own constraints and keeps this
shell (`docs/plans/platform-layer.md` §7).

**I2C uses the iCE40's internal pull-ups.** They are weak (tens of
kilohms); a short breadboard bus at 100 kHz works, anything longer wants
4.7 kΩ resistors to 3.3 V on SCL and SDA. The shell drives both lines open
drain through `SB_IO` (`rtl/upduino_top.sv`).

**`max-threads-default = 4`** (`board.toml` `[hw]`). A call that waits on a
callback keeps the live variables of every thread that may be waiting, in a
FIFO as deep as the thread limit; at the backend's default of 512, a design
with SPI and I2C reads came to 155,798 LUT4s, thirty times the part. Four
keeps pipelined designs pipelined; a design that needs more names
`[[max_threads(N)]]` on the method, and one that runs one transaction at a
time sets `max-threads-default = 1` in its own `[hw]`, as the bus examples
do (`docs/plans/board-tooling.md` §5).

**The other `[hw]` options trade frequency for area**: one reset cycle, no
semaphore delay, no added latency, a register ratio of 8, no reset fan-out.
24 MHz leaves plenty of slack (blinky closes at 73 MHz, the monitor at
45); each was measured to save cells (`platform-layer.md` §8.3). A project's
own `[hw]` goes over them, and `-Z hw.<key>=<value>` over that.

**`block-ram-util-threshold` stays at the backend's 20%.** Lowering it to 0
moves small FIFOs from flip-flops into block RAM, which on this part is the
cheaper of the two, and saves a quarter of the logic in callback-heavy
designs. But the HAL's block-RAM FIFOs then use more blocks than the backend
budgets (40 against its 29 in one design), and the design fails to place.
Until the two agree it stays where it is; a design that fits either way can
try `-Z hw.block-ram-util-threshold=0`.

**Every multiplication goes to a DSP block when it fits.** The device names
no multiplier intrinsic, so the backend emits each product as
`KanagawaLutMul`, which the HAL implements as an instantiated `SB_MAC16` for
operands up to 16 bits (signed or not), and in logic beyond that.

**`kg board` synthesises without Yosys's `-dsp`.** Yosys 0.69's `ice40_dsp`
pass crashes (a segfault) on a 16x16 product followed by two register
stages, which is exactly the pipeline the backend builds after a multiply.
Every Kanagawa product reaches a DSP through the HAL's own instance, so
nothing is lost there; but a `*` written by hand in your core's RTL is built
in logic. Instantiate `SB_MAC16` (or `KanagawaLutMul`) there if you need the
block. When a Yosys release fixes the crash, `-dsp` can return
(`crates/kg-board/src/icestorm.rs`).

**The flash's chip select stays undriven.** The FT232H's UART shares nets
with the SPI flash: DTR# is the flash's chip select and RTS# shares its data
output. The shell leaves pin 16 an input with the pull-up, and the host
keeps DTR and RTS deasserted (kgbridge and `host.py` do; with picocom,
`--lower-dtr --lower-rts`). The trace is in `constraints/upduino-v3.pcf`.

**The clock is the internal oscillator**, not the 12 MHz crystal, which
reaches a global buffer only with R16 shorted. `CLK_MHZ` (48, 24, 12 or 6) is
the shell's parameter; change `[clock] mhz` in `board.toml` and every
generated baud rate and bus divider follows.

## Measured (2026-10-17, `platform/check.zsh`)

| Example | Logic cells | Fmax | Notes |
|---|---|---|---|
| `blinky` | 55 (1.0%) | 73.2 MHz | generated core |
| `monitor` | 375 (7.1%) | 45.0 MHz | generated core, 2 SPRAM, `host.py` streamed over `serve` |
| `spram` | 159 (3.0%) | 60.1 MHz | its own core |
| `accel` | 3,214 (60.9%) | 32.3 MHz | the host bridge, a callback into Python, 1 DSP |
| `spi-flash` | 3,985 (75.5%) | 30.3 MHz | one thread |
| `i2c-sensor` | 3,479 (65.9%) | 34.1 MHz | one thread |

## Files

| File | Holds |
|---|---|
| `board.toml` | the device, clock, flow, `[hw]`, resources, programmer |
| `constraints/upduino-v3.pcf` | the pins, with the traces behind the serial and flash nets |
| `rtl/upduino_top.sv` | the shell: oscillator, reset, RGB driver, I2C pads, and the core beneath |
| `examples/` | `blinky`, `monitor` (SPRAM over the serial port, with `host.py`), `spram` |
