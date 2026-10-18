/*|
Device configuration for the Lattice iCE40 UltraPlus UP5K (iCE40UP5K), any
package: the device on the tinyVision UPduino v3 (iCE40UP5K-SG48;
platform/boards/upduino-v3).
*/
module hardware.config
    { module lattice.ice40.config \ hardware.config
    , module hardware.config
    }

import lattice.ice40.config

//| Version of device configuration schema
const auto version = 1;

//| Unambiguous name of the target device
const string device_name = "ice40-up5k";

//
// DSP
//

// No multiplier intrinsic. With a zero width the backend emits every
// multiplication as the `lutmul` op, KanagawaLutMul (compiler/cpp/ir.cpp:
// 6551-6560), which the iCE40 HAL writes as `*` (rtl/hal/ice40/hal_lutmul.sv)
// for `synth_ice40 -dsp` to map onto the part's eight SB_MAC16 blocks (16x16)
// where a product fits. `compiler.hal` has no 16-bit intrinsic to name.

//| Width of source operands for an unsigned integer multiplication using a device DSP
const auto integer_mul_src_width = 0;

//| Name of the intrinsic operation for 1 unsigned integer multiplication using a device DSP
const string unsigned_integer_mul_name = "";

//| Name of the intrinsic operation for 1 signed integer multiplication using a device DSP
const string signed_integer_mul_name = "";

//
// Memory
//

//| Specifies the number of each type of memory resource that is available on the device.
// iCE40 UltraPlus Family Data Sheet (FPGA-DS-02008), table 1.1: 5,280 logic
// cells, 30 EBR. The four 256 Kb SPRAM blocks are single-port and outside
// this model; designs reach them through `lattice.spram`.
const MemoryResource[2] memory_resources =
{
    {
        .type_id = MemoryType::LUT,
        // One flip-flop per logic cell.
        .num_available = 5280
    },
    {
        .type_id = MemoryType::Block,
        .num_available = 30
    }
};
