/*!
Device configuration with defaults for Lattice FPGAs.

The vendor-wide values, as upstream's `xilinx.config` and `intel.config`
hold them; a family's module (`lattice.ice40.config`) supplies the rest.
*/
module lattice.config
    { module lattice.config
    , module compiler.device.schema
    }

import compiler.device.schema

//| Name of the vendor that manufactures the target device, for example 'Intel' or 'Xilinx'
const string vendor = "Lattice";

//| The format to use for memory initialization files
// $readmemh text, which Yosys reads for both inferred and soft memories.
const MemoryInitFileType memory_init_file_type = MemoryInitFileType::Mem;

//
// Memory
//

//| true if the device supports block RAMs in a simple quad-port configuration
const bool block_ram_supports_quad_port = false;
