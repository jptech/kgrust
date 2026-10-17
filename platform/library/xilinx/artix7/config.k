/*!
Device configuration with defaults for AMD (Xilinx) Artix-7 FPGAs.

Derived from upstream's `xilinx.ultrascaleplus.config` (UltraScale+), which
shares the parts of the fabric the compiler models: 6-input LUTs, SRL32 shift
registers, 18 Kb block RAM tiles with the same aspect ratios, and the XPM
macros the `runtime/rtl/hal/xilinx` modules are built on. Where 7-series
differs the value is changed and the reason given beside it. Every value
marked PROVISIONAL is an estimate awaiting a Vivado utilisation and timing
report (docs/plans/platform-layer.md §6).
*/
module xilinx.artix7.config
    { module xilinx.config \ xilinx.artix7.config
    , module xilinx.artix7.config
    }

import xilinx.config

//| Name of the device family into which the target device belongs, for example 'Agilex 7'
const string device_family = "artix7";

//| Name of the device family that is significant to the Vendor RTL (or simulation libraries).
// This value is passed into HAL modules as a parameter. The Xilinx HAL modules
// declare DEVICE_FAMILY but never compare it (runtime/rtl/hal/xilinx/*.sv at
// the pinned reference), so any name is safe; this one names the family.
const string hal_device_family = "artix7";

//
// DSP
//

// DSP48E1 multiplies 25x18 signed (UG479); UltraScale+'s DSP48E2, 27x18.
// Neither holds an 18x18 unsigned product in one slice, and upstream targets
// UltraScale+ with the 18-bit intrinsics anyway, leaving the split to Vivado's
// inference. The 18-bit pair is also the narrowest `compiler.hal` offers.

//| Width of source operands for an unsigned integer multiplication using a device DSP
const auto integer_mul_src_width = 18;

//| Name of the intrinsic operation for 1 unsigned integer multiplication using a device DSP
const string unsigned_integer_mul_name = "umul18";

//| Name of the intrinsic operation for 1 signed integer multiplication using a device DSP
const string signed_integer_mul_name = "imul18";

//
// FIFO
//

// The FIFOs are XPM macros on both families (xpm_fifo_sync/xpm_fifo_async,
// UG953), whose depth and threshold limits do not depend on the family.

//| Minimum depth for a single clock fifo
const auto min_fifo_depth = 32;

//| Minimum depth for a dual clock fifo
const auto min_dual_clock_fifo_depth = 32;

//| Minimum almost_full value for a fifo. This is the minimum value needed to overcome fifo
// read->almost_full latency
const auto min_almost_full_depth = 5;

//| Minimum value for the almost empty threshold for a single-clock FIFO
const auto almost_empty_depth = 5;

//| Minimum width of LUTram based FIFO structures below which there are no area savings.
// Used in calculations for the local data propagation optimization
const auto fifo_width_alignment = 8;

//| Minimum depth of LUTram based FIFO structures below which there are no area savings.
// Used in calculations for the local data propagation optimization
const auto fifo_depth_alignment = 32;

//| Fixed cost of a FIFO used in calculations for the local data propagation optimization
// Used in calculations for the local data propagation optimization
// PROVISIONAL: UltraScale+'s value.
const auto fifo_fixed_cost = 128;

//| The number of bits in an implicit fifo that has the same area cost as 1 pipeline register.
// Used in calculations for the local data propagation optimization
// PROVISIONAL: UltraScale+'s value.
const auto fifo_bits_per_register = 16;

//| `true` if it makes sense to use KanagawaInternalBufferFifo for basic blockswith
// start conditions as an Fmax optimization
const bool use_internal_buffer_fifo_optimization = true;

//| true if basic block thread rates should be used when computing fifo depths.
// When set to false, a thread rate of 1 is assumed for sizing. This feature is
// used to work around an EDA tool bug for certain Intel devices.
const bool use_thread_rate_in_fifo_sizing = true;

//
// Miscellaneous
//

//| The Verilog pragma used to tell the synthesis tool not to merge a register with others
const string verilog_dont_merge_pragma_name = "dont_touch";

//| Multiple LUTs of this size can fit into 1 resource (ALM, CLB)
const auto small_lut_size = 6;

//| true if the platform supports a RAM-based shift register.
// Used in calculations for the local data propagation optimization
// SRLC32E, as on UltraScale+ (UG474).
const bool lut_based_shift_register_available = true;

//| true if control signals should have power-on value specified
const bool registers_require_power_on_initial_value = false;

// `supports_auto_pipelining` keeps its default, false (compiler.device.config):
// Vivado's auto-pipelining (the AUTOPIPELINE_* attributes
// hal_auto_pipeline_chain.sv sets) is an UltraScale-and-later feature.

//
// Memory
//

//| If the block RAMs in the device support pipeline bypass (aka memory forwarding) for at least 2 stages
const bool block_ram_supports_hardened_bypass = false;

//| true if the device supports ECC for block RAMs
// RAMB36E1 has hardened ECC in 64-bit simple dual-port mode (UG473).
const bool block_ram_supports_ecc = true;

//| true if the device supports block RAMs in a true dual-port configuration (i.e. two independent read/write ports).
const bool block_ram_supports_true_dual_port = true;

//| For data propagation memories, the minimum width of 1 memory
const auto global_data_propagation_ram_alignment = 36;

//| Valid memory configurations
// Deepest first within each type: GetMemoryComposition asserts it
// (compiler/cpp/backend_common.h:457). No `Deep` entries: 7-series has no
// UltraRAM, and an empty list is how upstream's Intel devices say the same.
const MemoryConfiguration[8] memory_configurations =
{
    // PROVISIONAL: UltraScale+'s two LUT-RAM entries with width and cost
    // halved. A 7-series SLICEM holds four LUTs to UltraScale+'s eight
    // (UG474, UG574), so the same per-bit cost model at half the size.
    {
        .type_id = MemoryType::LUT,
        .cost = 1.0,
        .width = 4,
        .depth = 64
    },
    {
        .type_id = MemoryType::LUT,
        .cost = 1.0,
        .width = 8,
        .depth = 32
    },
    // RAMB18E1 aspect ratios, simple dual-port at 36 bits (UG473); the same
    // as RAMB18E2's, so the entries and cost are UltraScale+'s.
    {
        .type_id = MemoryType::Block,
        .cost = 36.0,
        .width = 1,
        .depth = 16384
    },
    {
        .type_id = MemoryType::Block,
        .cost = 36.0,
        .width = 2,
        .depth = 8192
    },
    {
        .type_id = MemoryType::Block,
        .cost = 36.0,
        .width = 4,
        .depth = 4096
    },
    {
        .type_id = MemoryType::Block,
        .cost = 36.0,
        .width = 9,
        .depth = 2048
    },
    {
        .type_id = MemoryType::Block,
        .cost = 36.0,
        .width = 18,
        .depth = 1024
    },
    {
        .type_id = MemoryType::Block,
        .cost = 36.0,
        .width = 36,
        .depth = 512
    }
};
