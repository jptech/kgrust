//| Geometry for the Intel Agilex 7 F-series AGF023.
//
// Agilex 7 has hardened IEEE-754 single precision in its variable-precision
// DSP blocks: the standard library maps float32 `*` and `+` onto them
// (`library/device/agilex-7-f-023/numeric/float32/operator.k`), so a wide
// float32 array costs DSP blocks rather than logic.
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

const auto max_m = 64;
const auto max_n = 64;
const auto max_k = 128;

const auto fp32_kv = 16;
const auto fp32_lanes = 8;

const auto bf16_kv = 16;
const auto bf16_lanes = 8;

const auto int8_kv = 16;
const auto int8_lanes = 8;

const auto mx_lanes = 8;
