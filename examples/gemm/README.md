# A matrix-multiply accelerator

A complete Kanagawa design built and verified with kgrust: a GEMM engine that
computes `C = A · Bᵀ` in four number formats, tuned per FPGA, compiled with
`kg build`, and co-simulated in Verilator against a bit-exact model.

| Format | Element | Products | Accumulates in |
|---|---|---|---|
| `fp32` | IEEE single | float32, rounded | float32 |
| `bf16` | bfloat16 | exact, in float32 | float32 |
| `int8` | two's complement | exact | int32 |
| `mxint8` | [OCP MX] block: 32 × int8 under a shared power-of-two scale | exact per block | float32 |

It targets the boards the Kanagawa library has device support for:

| Target | Device | float32 is | Geometry (MAC/cycle) |
|---|---|---|---|
| `mock` | none: simulation | the mock HAL's models | fp32 32, bf16 64, int8 128, mxint8 128 |
| `agilex-7-f-023` | Intel Agilex 7 F | hardened DSP floating point | fp32 128, bf16 128, int8 128, mxint8 256 |
| `ultrascaleplus-xcu250` | Xilinx Alveo U250 | soft logic on DSP48 multipliers | fp32 16, bf16 32, int8 128, mxint8 128 |

[OCP MX]: https://www.opencompute.org/documents/ocp-microscaling-formats-mx-v1-0-spec-final-pdf

## Layout

```
kanagawa.toml           the project: sources, import directories
src/gemm/engine.k       the engine, generic over format and geometry
src/gemm/format.k       the formats: dot products, accumulation, bf16 and MX arithmetic
src/gemm_<format>.k     one exported top per format
config/gemm/config.k    the default geometry
config/device/<target>/gemm/config.k
                        a device's geometry, chosen by --target-device
sim/                    the co-simulation: port driver, bit-exact model, tests
run.sh                  build, verilate, simulate
```

## The engine

`A` (M × K, the activations) and `B` (N × K, the weights) both keep K
contiguous, as a linear layer's operands do, and both are held in on-chip
memories cut into chunks of `kv` elements along K. `B`'s rows are spread
over `lanes` memories, so a thread reads `lanes` rows at once:

```
          A chunk (kv)                 B chunks (kv), one per lane
               │                ┌───────────┬───────────┬───────────┐
               ├──────────────► × ×  …  ×   × ×  …  ×   × ×  …  ×     kv × lanes multipliers
               │                └──tree──┘  └──tree──┘  └──tree──┘     balanced adder trees
               │                     +           +           +         per-lane accumulators
               ▼                     ▲           ▲           ▲         (a thread's own locals)
       next chunk, same thread ──────┴───────────┴───────────┘
```

One *thread* computes one `lanes`-wide strip of one row of `C`. It walks K a
chunk at a time and keeps its sum in a local variable, so the dependency
through the accumulator belongs to that thread alone. `compute` starts one
thread per strip, a nested `pipelined_for` over rows and strips. The
threads follow each other through the loop body a cycle apart, so while one
waits on its adder the next is already multiplying. The engine sustains
90–99% of its peak on full-size problems without any explicit scheduling.

Everything specific to a format is a function in `format.k` that
`static if` selects by element type. Everything specific to a device is a
number in its `gemm/config.k`.

### Formats, exactly

* **bf16**: the product of two 8-bit mantissas fits float32 exactly, so
  `bf16_mul` assembles the float32 product from an 8 × 8 integer multiply,
  with no rounding. Only the adds round.
* **mxint8**: elements are `m · 2⁻⁶`, and a block is scaled by `2^(s − 127)`.
  The dot product of two blocks is an exact integer of at most 20 bits;
  `mx_dot` places it in float32 exactly and moves its exponent by both
  scales. Each block therefore costs 32 integer multiplies and one float32
  add.
* Results outside float32's normal range flush to signed zero (subnormals) or
  saturate to infinity. Subnormal inputs read as zero. NaN and infinity
  inputs are outside the contract.

### The interface

Each top exports five methods. In hardware each is a port group with a
valid/ready input and a FIFO output (see `sim/dut.h` for the handshake):

| Method | Does |
|---|---|
| `geometry()` | the engine's `kv`, `lanes` and buffer sizes, for a driver to discover |
| `load_a(row, kc, data)` | store chunk `kc` (elements `kc·kv …`) of a row of `A` |
| `load_b(row, kc, data)` | store chunk `kc` of a row of `B` |
| `compute(m, strips, chunks, accumulate)` | `C = A·Bᵀ` over the first `m` rows, `strips · lanes` columns and `chunks · kv` of K; with `accumulate`, `C += A·Bᵀ` |
| `read_c(row, strip)` | `lanes` elements of a row of `C` |

A problem larger than the buffers is tiled by the driver: across M and N by
addressing, and across K by `accumulate`. `run_engine` in `sim/main.cpp` is
such a driver.

## Targeting a device

```sh
kg check                                         # all four tops, against the mock device
kg check --profile u250                          # against the Alveo U250 (kanagawa.toml)
kg build --profile agilex -o out/ $PWD/src/gemm_fp32.k
kg doc $PWD/src $PWD/config --open               # the engine's documentation, every device's geometry
```

(The absolute paths work around a kgrust bug, open as of 2026-10-13: a
relative path such as `src/gemm_fp32.k`, combined with an import directory
that climbs two levels (`../../third_party/…`), loses one of the `..`.)

`--target-device` puts `<import-dir>/device/<target>/` ahead of every import
directory. The standard library uses this to choose a device's DSPs and
float32 operators, and this design uses it to choose its geometry. On the
U250, with no hardened floating point, the float32 array is narrow and the
integer arrays are wide. On Agilex 7 every float32 multiply and add is a DSP
block. In the generated RTL:

| Build | float32 multiply / add | Integer multiply |
|---|---|---|
| `fp32` on Agilex 7 | 128 `fmul32` + 128 `fadd32` DSP blocks | — |
| `fp32` on U250 | soft logic: 64 `umul18` DSP48s (4 per multiply) | — |
| `mxint8` on Agilex 7 | 8 `fadd32` | 256 `imul27` |
| `mxint8` on U250 | soft logic: 4 adders | 128 `imul18` |
| `int8` on Agilex 7 / U250 | — | 128 `imul27` / 128 `imul18` |

An integer `*` goes to the device's DSPs too: the library chooses the
multiplier from the operand widths (`integer_mul_src_width` in the device
configuration).

The configuration has an import directory of its own (`config/`) because the
importing file's own directory is searched before any device directory. A
default `gemm/config.k` beside `src/gemm_fp32.k` would win over every
device's override.

## Simulating

Needs Verilator (5.040 or later; tested with 5.052), a C++17 compiler, and
`kg` with its code generator (`target/release/kg` and
`target/backend/dist/bin` in a kgrust checkout, or a release archive).

```sh
./run.sh                                   # all four formats, mock device
./run.sh -d agilex-7-f-023 -f fp32         # one format on one device
./run.sh -d ultrascaleplus-xcu250 -- --stall 0.5 --seed 9
```

For each target and format, `run.sh` runs `kg build`, verilates the RTL with
the Kanagawa SV runtime, and runs the tests in `sim/main.cpp`. Vendor RAM
and FIFO primitives are replaced by the reference runtime's mock HAL. On
the U250 the HAL's DSP multiplier is the vendor's own (it is behavioural
Verilog), so the soft-float datapath simulated is the one that would be
synthesised.

Each test draws real matrices, quantises them to the format, multiplies them
on the engine, and compares **every element of C, bit for bit**, with a
model that does the engine's arithmetic in the engine's order: per chunk, a
balanced tree of products added to the running sum. It also reports
throughput, and the format's error against a float64 product of the
unquantised data. `--stall P` withholds every handshake at random, to test
back-pressure.

Results on 2026-10-13 (`./run.sh -d <target>`, seed 1, no stalls; `eff` is
MAC/cycle over peak):

| Target | Format | Peak | full (M×N×K) | eff | tiled | eff | error vs f64 |
|---|---|---|---|---|---|---|---|
| mock | fp32 | 32 | 32×32×64 | 93% | 67×37×121 | 84% | 9.8e-08 |
| mock | bf16 | 64 | 32×32×64 | 93% | 67×37×113 | 86% | 2.1e-03 |
| mock | int8 | 128 | 32×32×64 | 90% | 67×37×113 | 81% | 5.5e-03 |
| mock | mxint8 | 128 | 32×32×64 | 90% | 67×37×97 | 80% | 5.6e-03 |
| agilex-7-f-023 | fp32 | 128 | 64×64×128 | 96% | 131×69×241 | 91% | 1.0e-07 |
| agilex-7-f-023 | bf16 | 128 | 64×64×128 | 96% | 131×69×241 | 91% | 2.1e-03 |
| agilex-7-f-023 | int8 | 128 | 64×64×128 | 98% | 131×69×241 | 95% | 5.7e-03 |
| agilex-7-f-023 | mxint8 | 256 | 64×64×128 | 95% | 131×69×225 | 88% | 5.6e-03 |
| ultrascaleplus-xcu250 | fp32 | 16 | 64×64×128 | 96% | 131×69×253 | 91% | 1.6e-07 |
| ultrascaleplus-xcu250 | bf16 | 32 | 64×64×128 | 92% | 131×69×249 | 85% | 2.1e-03 |
| ultrascaleplus-xcu250 | int8 | 128 | 64×64×128 | 99% | 131×69×241 | 95% | 5.7e-03 |
| ultrascaleplus-xcu250 | mxint8 | 128 | 64×64×128 | 97% | 131×69×225 | 92% | 5.5e-03 |

A full run of all twelve takes about four minutes on an M-series Mac. Most
of it is the U250's soft-float tops: `kg build` writes 93,000 lines of SV for
fp32 and 160,000 for bf16, which Verilator takes 25 s and 104 s to compile.
Every other build verilates in seconds.

The `tiled` figures include the driver's reloads of `A` and `B` between
tiles. The int8 error is quantisation's, at a scale of 1/127. The engine's
integer arithmetic is exact.

### How the tests were checked

Bugs were planted, one at a time, with `scripts/mutant.sh`, and the
co-simulation caught every one by mismatching elements of C: a left fold
for the adder tree (1–2 ulp apart, in 1,642 of 2,479 elements), a lost
chunk index on `B`, `accumulate` ignored, every row of `B` stored to lane 0,
int8 accumulation by subtraction, a bf16 mantissa off by a bit, MX's
exponent bias off by one (in the engine and in the model).

The RTL that kgrust writes for this design was also compared with the
reference Kanagawa compiler's, with the same options. For all twelve
target-format pairs it is the same, apart from source locations.

## Taking it to hardware

This is the compute core. A board build also needs:

* the vendor HAL: `runtime/rtl/hal/xilinx/` or `hal/intel/` from the
  Kanagawa checkout, in place of `hal/mock/`, plus the vendor's libraries
  (XPM on Xilinx);
* a shell: host access to the five port groups (an AXI-Lite or AXI-Stream
  bridge), clocks, and constraints. The `kanagawa` compatibility binary takes
  every backend option (`--frequency`, `--register-ratio`, …); `kg build`
  takes the common ones.

Ways to take it further: pack two int8 products per DSP (the Agilex library
exposes `umul18x2`), give bf16's 8 × 8 mantissa multiply to LUTs
(`lutmul`) rather than a 27 × 27 DSP, and double-buffer `A` and `B` so that
loads overlap `compute`.
