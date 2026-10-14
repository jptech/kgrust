//| Geometry for the Xilinx Alveo U250 (UltraScale+ XCU250).
//
// UltraScale+ has no hardened floating point: every float32 multiply and add
// is built from LUTs and 18x18 DSP48 multipliers (four per multiply), so the
// float32 array is kept narrow and the integer array, which costs one DSP
// slice per multiply, is made wide.
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

const auto fp32_kv = 4;
const auto fp32_lanes = 4;

const auto bf16_kv = 8;
const auto bf16_lanes = 4;

const auto int8_kv = 16;
const auto int8_lanes = 8;

const auto mx_lanes = 4;
