// Number formats: how an element is packed onto a port, made from a real
// number, and multiplied and accumulated, bit for bit as src/gemm/format.k
// does it. Built with -ffp-contract=off, so that no multiply and add fuse.
#pragma once

#include <algorithm>
#include <cfloat>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <vector>

// A little-endian bit vector, the shape of every wide Verilator port.
struct Bits {
    std::vector<uint32_t> w;
    explicit Bits(unsigned nbits = 0) : w((nbits + 31) / 32, 0) {}
    void put(unsigned lsb, unsigned width, uint64_t v) {
        for (unsigned i = 0; i < width; ++i) {
            unsigned b = lsb + i;
            if (b / 32 >= w.size()) w.resize(b / 32 + 1, 0);
            uint32_t m = 1u << (b % 32);
            w[b / 32] = ((v >> i) & 1) ? (w[b / 32] | m) : (w[b / 32] & ~m);
        }
    }
    uint64_t get(unsigned lsb, unsigned width) const {
        uint64_t v = 0;
        for (unsigned i = 0; i < width; ++i) {
            unsigned b = lsb + i;
            if (b / 32 < w.size() && ((w[b / 32] >> (b % 32)) & 1)) v |= uint64_t(1) << i;
        }
        return v;
    }
};

inline uint32_t f2u(float f) { uint32_t u; std::memcpy(&u, &f, 4); return u; }
inline float u2f(uint32_t u) { float f; std::memcpy(&f, &u, 4); return f; }

// The engine's adders work on normal numbers: a subnormal result is flushed
// to zero on the soft-float devices. Data here stays clear of that range,
// and the model flushes too, so it agrees either way.
inline float ftz(float x) { return std::fpclassify(x) == FP_SUBNORMAL ? std::copysign(0.0f, x) : x; }
inline float fadd(float x, float y) { return ftz(ftz(x) + ftz(y)); }
inline float fmul(float x, float y) { return ftz(ftz(x) * ftz(y)); }

// data.array::reduce: neighbours paired first, an odd last element carried.
template <typename T, typename F>
T tree_reduce(std::vector<T> v, F f) {
    while (v.size() > 1) {
        std::vector<T> next;
        for (size_t i = 0; i + 1 < v.size(); i += 2) next.push_back(f(v[i], v[i + 1]));
        if (v.size() % 2) next.push_back(v.back());
        v = std::move(next);
    }
    return v[0];
}

// ---------------------------------------------------------------------------

struct Fp32 {
    static constexpr const char* name = "fp32";
    static constexpr unsigned elem_bits = 32;
    static constexpr unsigned block = 1;  // real values per element
    using Elem = float;
    using Acc = float;

    static void pack(Bits& b, unsigned lsb, Elem e) { b.put(lsb, 32, f2u(e)); }
    static Acc acc_from_bits(uint32_t u) { return u2f(u); }
    static uint32_t acc_bits(Acc a) { return f2u(a); }
    static Acc zero() { return 0.0f; }
    static Acc add(Acc x, Acc y) { return fadd(x, y); }
    static Acc product(Elem x, Elem y) { return fmul(x, y); }
    static double acc_value(Acc a) { return a; }

    static std::vector<Elem> quantize(const double* x, size_t n) {
        std::vector<Elem> r(n);
        for (size_t i = 0; i < n; ++i) r[i] = float(x[i]);
        return r;
    }
    static double value(const Elem* e, size_t i) { return e[i]; }
};

struct Bf16 {
    static constexpr const char* name = "bf16";
    static constexpr unsigned elem_bits = 16;
    static constexpr unsigned block = 1;
    using Elem = uint16_t;
    using Acc = float;

    static void pack(Bits& b, unsigned lsb, Elem e) { b.put(lsb, 16, e); }
    static Acc acc_from_bits(uint32_t u) { return u2f(u); }
    static uint32_t acc_bits(Acc a) { return f2u(a); }
    static Acc zero() { return 0.0f; }
    static Acc add(Acc x, Acc y) { return fadd(x, y); }
    static double acc_value(Acc a) { return a; }

    // format.k's bf16_mul, bit for bit.
    static Acc product(Elem x, Elem y) {
        int ex = (x >> 7) & 0xFF, ey = (y >> 7) & 0xFF;
        uint32_t sign = uint32_t((x ^ y) >> 15) << 31;
        uint32_t p = (0x80u | (x & 0x7F)) * (0x80u | (y & 0x7F));
        bool carry = (p >> 15) != 0;
        int e = ex + ey - 127 + (carry ? 1 : 0);
        uint32_t mantissa = carry ? (p & 0x7FFF) << 8 : (p & 0x3FFF) << 9;
        if (ex == 0 || ey == 0 || e <= 0) return u2f(sign);
        if (e >= 255) return u2f(sign | 0x7F800000);
        return u2f(sign | uint32_t(e) << 23 | mantissa);
    }

    // Round to nearest, ties to even; subnormals flush.
    static std::vector<Elem> quantize(const double* x, size_t n) {
        std::vector<Elem> r(n);
        for (size_t i = 0; i < n; ++i) {
            uint32_t u = f2u(ftz(float(x[i])));
            r[i] = uint16_t((u + 0x7FFF + ((u >> 16) & 1)) >> 16);
        }
        return r;
    }
    static double value(const Elem* e, size_t i) { return u2f(uint32_t(e[i]) << 16); }
};

struct Int8 {
    static constexpr const char* name = "int8";
    static constexpr unsigned elem_bits = 8;
    static constexpr unsigned block = 1;
    using Elem = int8_t;
    using Acc = int32_t;

    static void pack(Bits& b, unsigned lsb, Elem e) { b.put(lsb, 8, uint8_t(e)); }
    static Acc acc_from_bits(uint32_t u) { return int32_t(u); }
    static uint32_t acc_bits(Acc a) { return uint32_t(a); }
    static Acc zero() { return 0; }
    static Acc add(Acc x, Acc y) { return int32_t(uint32_t(x) + uint32_t(y)); }
    static Acc product(Elem x, Elem y) { return int32_t(x) * int32_t(y); }
    static double acc_value(Acc a) { return a; }

    // Symmetric per-tensor scaling would sit in the driver; the engine sees
    // integers, so the data is drawn as integers: x in [-1, 1) -> round(127 x).
    static std::vector<Elem> quantize(const double* x, size_t n) {
        std::vector<Elem> r(n);
        for (size_t i = 0; i < n; ++i) r[i] = int8_t(std::lround(std::fmax(-127.0, std::fmin(127.0, x[i] * 127.0))));
        return r;
    }
    static double value(const Elem* e, size_t i) { return e[i] / 127.0; }
};

struct MxInt8 {
    static constexpr const char* name = "mxint8";
    static constexpr unsigned elem_bits = 264;
    static constexpr unsigned block = 32;
    struct Elem {
        int8_t m[32];
        uint8_t scale;
    };
    using Acc = float;

    static void pack(Bits& b, unsigned lsb, const Elem& e) {
        for (unsigned i = 0; i < 32; ++i) b.put(lsb + 8 * i, 8, uint8_t(e.m[i]));
        b.put(lsb + 256, 8, e.scale);
    }
    static Acc acc_from_bits(uint32_t u) { return u2f(u); }
    static uint32_t acc_bits(Acc a) { return f2u(a); }
    static Acc zero() { return 0.0f; }
    static Acc add(Acc x, Acc y) { return fadd(x, y); }
    static double acc_value(Acc a) { return a; }

    // format.k's mx_dot: exact integer dot, exact rescale, flush below
    // float32's normal range, infinity above it.
    static Acc product(const Elem& x, const Elem& y) {
        int32_t s = 0;
        for (unsigned i = 0; i < 32; ++i) s += int32_t(x.m[i]) * int32_t(y.m[i]);
        if (s == 0) return 0.0f;
        double v = std::ldexp(double(s), int(x.scale) + int(y.scale) - 266);
        if (std::fabs(v) < double(FLT_MIN)) return std::copysign(0.0f, float(s));
        if (std::fabs(v) >= std::ldexp(1.0, 128)) return std::copysign(INFINITY, float(s));
        return float(v);  // exact: at most 20 significant bits
    }

    // OCP MX v1.0 quantisation: the shared scale is floor(log2(amax)) (MXINT8's
    // largest element exponent is 0), elements are x / 2^scale in 1.6 fixed
    // point, rounded to nearest and clamped to [-127, 127].
    static std::vector<Elem> quantize(const double* x, size_t n) {
        std::vector<Elem> r(n / 32);
        for (size_t b = 0; b < n / 32; ++b) {
            double amax = 0;
            for (unsigned i = 0; i < 32; ++i) amax = std::fmax(amax, std::fabs(x[32 * b + i]));
            int e = amax == 0 ? -127 : int(std::floor(std::log2(amax)));
            e = std::max(-127, std::min(127, e));
            r[b].scale = uint8_t(e + 127);
            for (unsigned i = 0; i < 32; ++i) {
                double q = std::nearbyint(std::ldexp(x[32 * b + i], 6 - e));
                r[b].m[i] = int8_t(std::fmax(-127.0, std::fmin(127.0, q)));
            }
        }
        return r;
    }
    static double value(const Elem* e, size_t i) {
        const Elem& blk = e[i / 32];
        return std::ldexp(double(blk.m[i % 32]), int(blk.scale) - 127 - 6);
    }
};
