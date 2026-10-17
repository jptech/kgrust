//| DSP intrinsics for the Artix-7 XC7A50T: the two `xilinx.artix7.config`
// names, as upstream's ultrascaleplus-xcu250 defines them. Both resolve to
// `runtime/rtl/hal/xilinx/hal_dsp.sv`, which infers DSP48 slices.
module hardware.dsp
    { umul18
    , imul18
    }

import compiler.hal as hal
import compiler.config as config

inline uint36 umul18(uint18 x, uint18 y)
{
    static hal::umul18<config::hal_device_family, 2> dsp;
    return dsp.op(x, y);
}

inline int36 imul18(int18 x, int18 y)
{
    static hal::imul18<config::hal_device_family, 2> dsp;
    return dsp.op(x, y);
}
