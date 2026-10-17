/*!
Device configuration with defaults for Lattice iCE40 FPGAs (LP, HX, UltraPlus).

The iCE40 fabric differs from every family upstream models, and the values
below follow from three of its facts:

* No distributed RAM. A logic cell is a LUT4, a flip-flop and a carry; a
  memory the compiler would put in LUT-RAM can only be flip-flops and LUTs.
  The `LUT` memory type is therefore modelled as flip-flop storage, one bit
  per atom: the backend starts every FIFO and output-registered memory in
  `LUT` and moves the ones that save most into block RAM while block RAM
  lasts (`MapMemoriesAndFifosToRams`, compiler/cpp/compiler.cpp:930-985).
  Leaving `LUT` empty instead makes its cost `SIZE_MAX`, which the same pass
  then sums (compiler.cpp:955).
* Block RAM (EBR, `SB_RAM40_4K`) is 4 Kb with one read and one write port
  (TN1250): no true dual-port mode, no ECC, no hardened bypass.
* The fabric is small, 1–5 K logic cells, so FIFOs sit at the smallest depth
  that keeps throughput rather than the 32 entries upstream pads them to.

Devices of the family supply `memory_resources` and the DSP choice (the
UltraPlus parts have `SB_MAC16`; LP and HX have none). Values marked
PROVISIONAL await nextpnr utilisation on more designs than the pilot's
(docs/plans/platform-layer.md §8.6).
*/
module lattice.ice40.config
    { module lattice.config \ lattice.ice40.config
    , module lattice.ice40.config
    }

import lattice.config

//| Name of the device family into which the target device belongs, for example 'Agilex 7'
const string device_family = "ice40";

//| Name of the device family that is significant to the Vendor RTL (or simulation libraries).
// Passed to the HAL as DEVICE_FAMILY; platform/rtl/hal/ice40 ignores it.
const string hal_device_family = "ice40";

//
// FIFO
//

//| Minimum depth for a single clock fifo
// A floor only: the backend sizes each FIFO for its own pipeline first and
// uses this to avoid depth 1 (compiler/cpp/lower.cpp:768-791). Upstream's 32
// exists to fill an MLAB or LUT-RAM; here every entry is flip-flops.
const auto min_fifo_depth = 2;

//| Minimum depth for a dual clock fifo
const auto min_dual_clock_fifo_depth = 16;

//| Minimum almost_full value for a fifo. This is the minimum value needed to overcome fifo
// read->almost_full latency
// The mock HAL's value: platform/rtl/hal/ice40 builds its flip-flop FIFOs on
// the same runtime FIFO (KanagawaShowAheadRegisterFifo) the mock HAL does.
// Lower is not a size knob; too low overflows a FIFO.
const auto min_almost_full_depth = 4;

//| Minimum value for the almost empty threshold for a single-clock FIFO
const auto almost_empty_depth = 0;

//| Minimum width of LUTram based FIFO structures below which there are no area savings.
// Used in calculations for the local data propagation optimization
// Flip-flop FIFOs save area at any width or depth.
const auto fifo_width_alignment = 1;

//| Minimum depth of LUTram based FIFO structures below which there are no area savings.
// Used in calculations for the local data propagation optimization
const auto fifo_depth_alignment = 1;

//| Fixed cost of a FIFO used in calculations for the local data propagation optimization
// Used in calculations for the local data propagation optimization
// PROVISIONAL: the pointers, counters and flags of a small FIFO, in
// register-equivalents.
const auto fifo_fixed_cost = 16;

//| The number of bits in an implicit fifo that has the same area cost as 1 pipeline register.
// Used in calculations for the local data propagation optimization
// A flip-flop FIFO bit costs a flip-flop and a share of the read mux: no
// cheaper than the pipeline register it would replace.
const auto fifo_bits_per_register = 1;

//| `true` if it makes sense to use KanagawaInternalBufferFifo for basic blockswith
// start conditions as an Fmax optimization
// An area cost for Fmax, which the targets of this family do not need.
const bool use_internal_buffer_fifo_optimization = false;

//| true if basic block thread rates should be used when computing fifo depths.
// When set to false, a thread rate of 1 is assumed for sizing. This feature is
// used to work around an EDA tool bug for certain Intel devices.
const bool use_thread_rate_in_fifo_sizing = true;

//
// Miscellaneous
//

//| The Verilog pragma used to tell the synthesis tool not to merge a register with others
// Yosys's attribute.
const string verilog_dont_merge_pragma_name = "keep";

//| Multiple LUTs of this size can fit into 1 resource (ALM, CLB)
const auto small_lut_size = 4;

//| true if the platform supports a RAM-based shift register.
// Used in calculations for the local data propagation optimization
const bool lut_based_shift_register_available = false;

//| true if control signals should have power-on value specified
// iCE40 flip-flops come out of configuration at their INIT value (zero, or
// one through Yosys's inversion), so control signals need no power-on value
// beyond what the reset sequence sets.
const bool registers_require_power_on_initial_value = false;

//
// Memory
//

//| If the block RAMs in the device support pipeline bypass (aka memory forwarding) for at least 2 stages
const bool block_ram_supports_hardened_bypass = false;

//| true if the device supports ECC for block RAMs
const bool block_ram_supports_ecc = false;

//| true if the device supports block RAMs in a true dual-port configuration (i.e. two independent read/write ports).
const bool block_ram_supports_true_dual_port = false;

//| For data propagation memories, the minimum width of 1 memory
// The widest SB_RAM40_4K configuration.
const auto global_data_propagation_ram_alignment = 16;

//| Valid memory configurations
// Deepest first within each type (compiler/cpp/backend_common.h:457).
const MemoryConfiguration[5] memory_configurations =
{
    // Flip-flop storage, one bit per atom (see above). The cost is one
    // register; PROVISIONAL against the read mux it also needs.
    {
        .type_id = MemoryType::LUT,
        .cost = 1.0,
        .width = 1,
        .depth = 1
    },
    // SB_RAM40_4K's four aspect ratios (TN1250). PROVISIONAL cost: a block
    // RAM against 64 flip-flops, so a memory of more than 64 bits prefers it
    // when one is free.
    {
        .type_id = MemoryType::Block,
        .cost = 64.0,
        .width = 2,
        .depth = 2048
    },
    {
        .type_id = MemoryType::Block,
        .cost = 64.0,
        .width = 4,
        .depth = 1024
    },
    {
        .type_id = MemoryType::Block,
        .cost = 64.0,
        .width = 8,
        .depth = 512
    },
    {
        .type_id = MemoryType::Block,
        .cost = 64.0,
        .width = 16,
        .depth = 256
    }
};

//| This value is multiplied by the number of muxes necessary to implement an array when estimating.
// the relative cost of synthesizing a memory structure as an array (logic) versus using a RAM primitive.
// PROVISIONAL: the mock device's, whose LUT4 fabric is the nearest model.
const float32 array_mux_cost_factor = 0.5;

//| This value is multiplied by the number of registers necessary to implement an array when estimating.
// the relative cost of synthesizing a memory structure as an array (logic) versus using a RAM primitive.
const float32 array_register_cost_factor = 0.0;

//| This value is multiplied by the number of write ports necessary to implement an array when estimating.
// the relative cost of synthesizing a memory structure as an array (logic) versus using a RAM primitive.
const float32 array_write_port_cost_factor = 0.25;
