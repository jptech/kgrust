// Co-simulation of a GEMM engine against a bit-exact model.
//
// Built once per format (FORMAT_FP32, FORMAT_BF16, FORMAT_INT8,
// FORMAT_MXINT8) against that format's Verilated top. Each test draws
// matrices of reals, quantises them to the format, multiplies them on the
// engine through a tiling driver, and compares every element of C, bit for
// bit (a float32 zero of either sign matching the other), with a model that
// adds in the engine's order. It also reports throughput, and the error of
// the format against a float64 product of the unquantised data.
//
//   gemm_sim [--seed N] [--stall P] [--quick]
//
// `--stall P` withholds each handshake with probability P, to shake out
// back-pressure bugs; throughput figures then mean little.

#include <cinttypes>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "dut.h"
#include "formats.h"

#if defined(FORMAT_FP32)
#include "VGemmFp32.h"
using Top = VGemmFp32;
using Fmt = Fp32;
#elif defined(FORMAT_BF16)
#include "VGemmBf16.h"
using Top = VGemmBf16;
using Fmt = Bf16;
#elif defined(FORMAT_INT8)
#include "VGemmInt8.h"
using Top = VGemmInt8;
using Fmt = Int8;
#elif defined(FORMAT_MXINT8)
#include "VGemmMxInt8.h"
using Top = VGemmMxInt8;
using Fmt = MxInt8;
#else
#error "define one of FORMAT_FP32, FORMAT_BF16, FORMAT_INT8, FORMAT_MXINT8"
#endif

using Elem = Fmt::Elem;
using Acc = Fmt::Acc;

// A matrix of rows, each quantised along K: K is padded with zeros to a
// whole number of chunks.
struct Operand {
    unsigned rows, kelems;  // kelems: format elements per row (blocks, for MX)
    std::vector<Elem> e;
    const Elem* row(unsigned r) const { return &e[size_t(r) * kelems]; }
};

static Operand quantize(const std::vector<double>& x, unsigned rows, unsigned k, unsigned kpad) {
    Operand o{rows, kpad / Fmt::block, {}};
    std::vector<double> padded(kpad, 0.0);
    for (unsigned r = 0; r < rows; ++r) {
        std::copy(&x[size_t(r) * k], &x[size_t(r) * k] + k, padded.begin());
        auto q = Fmt::quantize(padded.data(), kpad);
        o.e.insert(o.e.end(), q.begin(), q.end());
    }
    return o;
}

struct Stats {
    uint64_t cycles = 0, macs = 0;
};

// C = A * B^T on the engine, tiled to fit its memories: across M and N by
// addressing, across K by accumulating onto C.
static std::vector<Acc> run_engine(Dut<Top>& dut, const Geometry& g, const Operand& a, const Operand& b, Stats& st) {
    const unsigned M = a.rows, N = b.rows, Ke = a.kelems;
    const unsigned chunks_total = Ke / g.kv;
    const unsigned max_chunks = g.max_k / g.kv;
    std::vector<Acc> c(size_t(M) * N);

    for (unsigned m0 = 0; m0 < M; m0 += g.max_m) {
        const unsigned mt = std::min(g.max_m, M - m0);
        for (unsigned n0 = 0; n0 < N; n0 += g.max_n) {
            const unsigned nt = std::min(g.max_n, N - n0);
            const unsigned strips = (nt + g.lanes - 1) / g.lanes;
            for (unsigned c0 = 0; c0 < chunks_total; c0 += max_chunks) {
                const unsigned ct = std::min(max_chunks, chunks_total - c0);
                for (unsigned r = 0; r < mt; ++r)
                    for (unsigned kc = 0; kc < ct; ++kc) {
                        Bits w(g.kv * Fmt::elem_bits);
                        for (unsigned k = 0; k < g.kv; ++k)
                            Fmt::pack(w, k * Fmt::elem_bits, a.row(m0 + r)[(c0 + kc) * g.kv + k]);
                        dut.load_a(r, kc, w);
                    }
                // Rows of B past N, up to a whole strip, are zero.
                for (unsigned r = 0; r < strips * g.lanes; ++r)
                    for (unsigned kc = 0; kc < ct; ++kc) {
                        Bits w(g.kv * Fmt::elem_bits);
                        if (n0 + r < N)
                            for (unsigned k = 0; k < g.kv; ++k)
                                Fmt::pack(w, k * Fmt::elem_bits, b.row(n0 + r)[(c0 + kc) * g.kv + k]);
                        dut.load_b(r, kc, w);
                    }
                dut.wait_loads();
                st.cycles += dut.compute(mt, strips, ct, c0 != 0);
                st.macs += uint64_t(mt) * strips * g.lanes * ct * g.kv * Fmt::block;
            }
            for (unsigned r = 0; r < mt; ++r)
                for (unsigned s = 0; s < strips; ++s) dut.read_c(r, s);
            auto results = dut.wait_reads();
            for (unsigned r = 0; r < mt; ++r)
                for (unsigned s = 0; s < strips; ++s) {
                    Bits w = results.front();
                    results.pop_front();
                    for (unsigned l = 0; l < g.lanes; ++l) {
                        unsigned n = s * g.lanes + l;
                        if (n < nt) c[size_t(m0 + r) * N + n0 + n] = Fmt::acc_from_bits(uint32_t(w.get(32 * l, 32)));
                    }
                }
        }
    }
    return c;
}

// The engine's arithmetic, in the engine's order: per chunk of `kv`
// elements, a balanced tree of products, added to the running sum.
static std::vector<Acc> run_model(const Geometry& g, const Operand& a, const Operand& b) {
    std::vector<Acc> c(size_t(a.rows) * b.rows);
    for (unsigned i = 0; i < a.rows; ++i)
        for (unsigned j = 0; j < b.rows; ++j) {
            Acc acc = Fmt::zero();
            for (unsigned c0 = 0; c0 < a.kelems; c0 += g.kv) {
                std::vector<Acc> p;
                for (unsigned k = 0; k < g.kv; ++k) p.push_back(Fmt::product(a.row(i)[c0 + k], b.row(j)[c0 + k]));
                acc = Fmt::add(acc, tree_reduce(p, Fmt::add));
            }
            c[size_t(i) * b.rows + j] = acc;
        }
    return c;
}

static bool same(Acc x, Acc y) {
    if constexpr (std::is_same_v<Acc, float>) {
        if (x == 0 && y == 0) return true;  // +0 and -0
        if (std::isnan(x) && std::isnan(y)) return true;
    }
    return Fmt::acc_bits(x) == Fmt::acc_bits(y);
}

struct Case {
    std::string name;
    unsigned m, n, k;
    enum { Random, Identity } kind;
};

int main(int argc, char** argv) {
    uint32_t seed = 1;
    double stall = 0;
    bool quick = false;
    for (int i = 1; i < argc; ++i) {
        if (!std::strcmp(argv[i], "--seed") && i + 1 < argc) seed = uint32_t(std::stoul(argv[++i]));
        else if (!std::strcmp(argv[i], "--stall") && i + 1 < argc) stall = std::stod(argv[++i]);
        else if (!std::strcmp(argv[i], "--quick")) quick = true;
    }

    VerilatedContext ctx;
    ctx.commandArgs(argc, argv);
    Dut<Top> dut(&ctx, stall, seed ^ 0x5eed);
    dut.reset();
    const Geometry g = dut.geometry();
    const unsigned chunk = g.kv * Fmt::block;  // K elements per chunk
    const unsigned kmax = g.max_k * Fmt::block;
    std::printf("%s engine: %u lanes x %u-element chunks = %u MAC/cycle peak; buffers %u x %u x %u; seed %u%s\n",
                Fmt::name, g.lanes, chunk, g.lanes * chunk, g.max_m, g.max_n, kmax, seed,
                stall > 0 ? ", random stalls" : "");

    std::vector<Case> cases = {
        {"identity", g.max_m, g.max_n, std::min(kmax, g.max_m), Case::Identity},
        {"full", g.max_m, g.max_n, kmax, Case::Random},
        {"ragged", 5, 2 * g.lanes + 3, 3 * chunk - 7, Case::Random},
        {"one", 1, 1, 1, Case::Random},
    };
    if (!quick) {
        cases.push_back({"k-split", g.max_m / 2, g.max_n, 3 * kmax, Case::Random});
        cases.push_back({"tiled", 2 * g.max_m + 3, g.max_n + 5, 2 * kmax - chunk + 1, Case::Random});
    }

    std::mt19937_64 rng(seed);
    std::uniform_real_distribution<double> uni(-1.0, 1.0);
    std::printf("%-9s %-14s %9s %9s %6s %10s %12s\n", "test", "M x N x K", "cycles", "MAC/cyc", "eff", "mismatch",
                "err vs f64");
    int failures = 0;
    for (const Case& t : cases) {
        std::vector<double> A(size_t(t.m) * t.k), B(size_t(t.n) * t.k);
        for (unsigned i = 0; i < t.m; ++i)
            for (unsigned k = 0; k < t.k; ++k)
                A[size_t(i) * t.k + k] = t.kind == Case::Identity ? (i == k ? 0.5 : 0.0) : uni(rng);
        for (double& x : B) x = uni(rng);

        const unsigned kpad = (t.k + chunk - 1) / chunk * chunk;
        Operand a = quantize(A, t.m, t.k, kpad), b = quantize(B, t.n, t.k, kpad);

        Stats st;
        std::vector<Acc> got = run_engine(dut, g, a, b, st);
        std::vector<Acc> want = run_model(g, a, b);

        size_t bad = 0;
        double num = 0, den = 0;
        for (unsigned i = 0; i < t.m; ++i)
            for (unsigned j = 0; j < t.n; ++j) {
                size_t at = size_t(i) * t.n + j;
                if (!same(got[at], want[at])) {
                    if (bad < 5)
                        std::printf("  C[%u][%u]: engine %08x, model %08x\n", i, j, Fmt::acc_bits(got[at]),
                                    Fmt::acc_bits(want[at]));
                    ++bad;
                }
                double exact = 0;
                for (unsigned k = 0; k < t.k; ++k) exact += A[size_t(i) * t.k + k] * B[size_t(j) * t.k + k];
                double v = Fmt::acc_value(got[at]);
                if constexpr (std::is_same_v<Fmt, Int8>) v /= 127.0 * 127.0;
                num += (v - exact) * (v - exact);
                den += exact * exact;
            }
        failures += bad != 0;
        char shape[32];
        std::snprintf(shape, sizeof shape, "%ux%ux%u", t.m, t.n, t.k);
        double rate = st.cycles ? double(st.macs) / st.cycles : 0;
        std::printf("%-9s %-14s %9" PRIu64 " %9.1f %5.0f%% %10zu %12.2e\n", t.name.c_str(), shape, st.cycles, rate,
                    100.0 * rate / (g.lanes * chunk), bad, den > 0 ? std::sqrt(num / den) : 0.0);
    }
    std::printf("%s\n", failures ? "FAIL" : "PASS");
    return failures ? 1 : 0;
}
