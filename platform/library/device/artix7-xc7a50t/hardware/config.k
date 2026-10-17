/*|
Device configuration for the AMD (Xilinx) Artix-7 XC7A50T, any package and
speed grade: the compiler's model holds no timing. This is the device on the
Numato Mimas A7 (XC7A50T-1FGG484C; platform/boards/mimas-a7).
*/
module hardware.config
    { module xilinx.artix7.config \ hardware.config
    , module hardware.config
    }

import xilinx.artix7.config

//| Version of device configuration schema
const auto version = 1;

//| Unambiguous name of the target device
const string device_name = "artix7-xc7a50t";

//
// Memory
//

//| Specifies the number of each type of memory resource that is available on the device.
// Counts from DS180 (7 Series FPGAs Data Sheet: Overview), table 3.
const MemoryResource[2] memory_resources =
{
    {
        .type_id = MemoryType::LUT,
        // 600 Kb of distributed RAM at 256 bits per SLICEM is 2,400 SLICEMs;
        // divided by 5 as upstream's ultrascaleplus-xcu250 does.
        .num_available = 2400 / 5
    },
    {
        .type_id = MemoryType::Block,
        // 75 RAMB36, counted as 150 RAMB18: the memory configurations are
        // 18 Kb tiles.
        .num_available = 150
    }
};
