//| The accelerator's geometry for a target without a tuned configuration,
// and for simulation against the mock device.
//
// A device can override this module: the compiler looks for
// `device/<target-device>/gemm/config.k` under each import directory before
// it looks here, exactly as the standard library picks a device's float32
// operators. `config/device/` holds the overrides this example ships.
//
// The configuration has an import directory of its own, `config/`, because
// the importing file's directory is searched before any device directory
// (`Parser/IO.hs:103`): a default beside the sources that import it would
// shadow every device's override.
module gemm.config
    { max_m
    , max_n
    , max_k
    , fp32_kv
    , fp32_lanes
    , bf16_kv
    , bf16_lanes
    , int8_kv
    , int8_lanes
    , mx_lanes
    }

//| The largest problem the on-chip buffers hold, in elements. A larger
// problem is tiled by the driver: across M and N by addressing, and across K
// with `compute`'s `accumulate` flag.
const auto max_m = 32;
const auto max_n = 32;
const auto max_k = 64;

//| Elements of K consumed per lane per cycle (`kv`), and output columns
// computed side by side (`lanes`): `kv * lanes` multiply-accumulates a cycle.
const auto fp32_kv = 8;
const auto fp32_lanes = 4;

const auto bf16_kv = 16;
const auto bf16_lanes = 4;

const auto int8_kv = 16;
const auto int8_lanes = 8;

//| MXINT8 moves one 32-element block per lane per cycle.
const auto mx_lanes = 4;
