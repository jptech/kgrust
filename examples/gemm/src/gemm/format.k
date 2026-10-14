//| The number formats the engine computes in, and what each one does inside
// a dot product.
//
// | Element  | Products            | Reduction and accumulation          |
// |----------|---------------------|-------------------------------------|
// | float32  | float32, rounded    | float32 adds, a balanced tree       |
// | bf16     | exact, in float32   | float32 adds, a balanced tree       |
// | int8     | exact, in int16     | exact, int32 (wraps past 2^31)      |
// | mxint8   | exact, in int16     | exact per block; float32 across     |
//
// The float32 multiplies and adds are the target device's: on a device with
// hardened floating point they are DSP blocks, elsewhere soft logic.
module gemm.format
    { bf16
    , mxint8
    , mx_block_size
    , dot
    , zero
    , acc_add
    }

import data.array
import data.bits

//| A bfloat16: float32's sign and 8-bit exponent above a 7-bit mantissa.
// Subnormal inputs read as zero, and NaN and infinity are outside the
// contract: the engine is for finite data.
struct bf16
{
    uint16 bits;
}

//| Elements in an MXINT8 block.
const auto mx_block_size = 32;

//| One block of an OCP Microscaling MXINT8 vector: 32 two's-complement
// elements, each read as `m * 2^-6`, under one E8M0 scale `2^(scale - 127)`.
// A scale of 255 (NaN) is outside the contract.
struct mxint8
{
    int8[mx_block_size] m;
    uint8 scale;
}

inline float32 fadd(float32 x, float32 y)
{
    return x + y;
}

inline int32 iadd(int32 x, int32 y)
{
    return cast<int32>(x + y);
}

//| `x * y` for two bf16 values, exactly: the product of two 8-bit mantissas
// fits float32's 24 with room to spare, so nothing is rounded. A product
// below float32's normal range flushes to a signed zero, as a zero or
// subnormal input does; one above it is a signed infinity.
inline float32 bf16_mul(bf16 x, bf16 y)
{
    const uint8 ex = cast<uint8>(x.bits >> 7);
    const uint8 ey = cast<uint8>(y.bits >> 7);
    const uint32 sign = cast<uint32>((x.bits ^ y.bits) >> 15) << 31;

    const uint8 mx = 0x80 | cast<uint7>(x.bits);
    const uint8 my = 0x80 | cast<uint7>(y.bits);
    const uint16 p = mx * my;

    // The product of two mantissas in [1, 2) lies in [1, 4): one bit of
    // normalisation, folded into the exponent.
    const bool carry = (p >> 15) != 0;
    const int11 e = ex + ey - 127 + (carry ? 1 : 0);
    const uint32 mantissa = carry ? cast<uint32>(cast<uint15>(p)) << 8 : cast<uint32>(cast<uint14>(p)) << 9;

    uint32 result;
    if (ex == 0 || ey == 0 || e <= 0)
    {
        result = sign;
    }
    else if (e >= 255)
    {
        result = sign | 0x7F800000;
    }
    else
    {
        result = sign | (cast<uint32>(e) << 23) | mantissa;
    }
    return cast<float32>(result);
}

//| The dot product of two MXINT8 blocks as a float32, exactly: the integer
// sum of products needs at most 20 bits of magnitude, which float32's
// mantissa holds, and the two scales only move the exponent. A result below
// float32's normal range flushes to zero; one above it is an infinity.
inline float32 mx_dot(mxint8 x, mxint8 y)
{
    int32[mx_block_size] p;
    static for (const auto k : mx_block_size)
    {
        p[k] = x.m[k] * y.m[k];
    }
    const int32 s = reduce(iadd, p);

    const uint32 sign = s < 0 ? 0x80000000 : 0;
    const uint20 magnitude = cast<uint20>(s < 0 ? -s : s);
    const auto lead = highest_one(magnitude);

    // value = s * 2^-12 * 2^(xs - 127) * 2^(ys - 127); with the leading one
    // at bit `lead`, the biased exponent is lead + xs + ys - 266 + 127.
    const int12 e = lead.value + x.scale + y.scale - 139;
    const uint32 mantissa = cast<uint32>(cast<uint23>((cast<uint43>(magnitude) << 23) >> lead.value));

    uint32 result;
    if (!lead.is_valid || e <= 0)
    {
        result = sign;
    }
    else if (e >= 255)
    {
        result = sign | 0x7F800000;
    }
    else
    {
        result = sign | (cast<uint32>(e) << 23) | mantissa;
    }
    return cast<float32>(result);
}

//| The additive identity of an accumulator.
template <typename Acc>
inline Acc zero()
{
    Acc result;
    static if (Acc == float32)
    {
        result = 0.0;
    }
    else
    {
        result = 0;
    }
    return result;
}

//| `acc + x` in the accumulator's own arithmetic.
template <typename Acc>
inline Acc acc_add(Acc acc, Acc x)
{
    Acc result;
    static if (Acc == float32)
    {
        result = acc + x;
    }
    else
    {
        result = cast<Acc>(acc + x);
    }
    return result;
}

//| The dot product of two `Kv`-element chunks. Products are summed by a
// balanced tree, pairs of neighbours first (`data.array::reduce`); a golden
// model must add in the same order to agree to the bit.
template <typename Elem, typename Acc, auto Kv>
inline Acc dot(Elem[Kv] x, Elem[Kv] y)
{
    Acc[Kv] p;
    static for (const auto k : Kv)
    {
        static if (Elem == float32)
        {
            p[k] = x[k] * y[k];
        }
        else static if (Elem == bf16)
        {
            p[k] = bf16_mul(x[k], y[k]);
        }
        else static if (Elem == mxint8)
        {
            p[k] = mx_dot(x[k], y[k]);
        }
        else
        {
            p[k] = x[k] * y[k];
        }
    }

    Acc result;
    static if (Acc == float32)
    {
        result = reduce(fadd, p);
    }
    else
    {
        result = reduce(iadd, p);
    }
    return result;
}
