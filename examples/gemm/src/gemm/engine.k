//| A matrix-multiply engine: `C = A * B^T`, the shape of a linear layer, in
// which `A` (M x K, activations) and `B` (N x K, weights) are both stored
// with K contiguous.
//
// Both operands live in on-chip memories, cut into chunks of `Kv` elements
// along K. One thread computes one `Lanes`-wide strip of one row of `C`: it
// walks K a chunk at a time, reading one chunk of `A` and the matching chunk
// of `Lanes` rows of `B` each iteration, and feeds `Kv * Lanes`
// multipliers.
//
// The accumulator is a thread's own local variable, so the loop-carried
// dependency through the adder is a thread's alone. The adder's latency is
// hidden by running many threads through the same loop body at once, one
// per strip of `C`, each a cycle behind the last: `compute` starts them with
// a nested `pipelined_for` over the rows of `C` and the strips of each row.
module gemm.engine
    { Engine
    , Geometry
    }

import gemm.format

//| What a driver needs to know to lay data out for an engine.
struct Geometry
{
    uint16 kv;
    uint16 lanes;
    uint16 max_m;
    uint16 max_n;
    uint16 max_k;
}

template
    < typename Elem //< The element type of `A` and `B`.
    , typename Acc  //< The accumulator and the element type of `C`.
    , auto Kv       //< Elements of K per memory word.
    , auto Lanes    //< Columns of `C` computed side by side.
    , auto MaxM     //< Rows of `A` and of `C` the memories hold.
    , auto MaxN     //< Rows of `B`, columns of `C`.
    , auto MaxK     //< Columns of `A` and of `B`.
    >
class Engine
{
private:
    const auto KcMax = MaxK / Kv;
    const auto NtMax = MaxN / Lanes;

    static assert(MaxK % Kv == 0);
    static assert(MaxN % Lanes == 0);

    // A: row i, chunk kc at i * KcMax + kc.
    memory<Elem[Kv], MaxM * KcMax> _a;

    // B: row n lives in memory n % Lanes, so that the Lanes rows a thread
    // needs are read in one cycle, one from each; chunk kc of row n is at
    // (n / Lanes) * KcMax + kc.
    memory<Elem[Kv], NtMax * KcMax>[Lanes] _b;

    // C: row i, strip jt (columns jt * Lanes ...) at i * NtMax + jt.
    memory<Acc[Lanes], MaxM * NtMax> _c;

    inline void strip(index_t<MaxM> i, index_t<NtMax> jt, count_t<KcMax> kcount, bool accumulate)
    {
        const auto c_addr = i * NtMax + jt;

        // Accumulating onto the C that is already there lets a driver split K
        // into several calls to `compute`.
        const Acc[Lanes] previous = _c[c_addr];
        Acc[Lanes] acc;
        static for (const auto l : Lanes)
        {
            acc[l] = accumulate ? previous[l] : zero<Acc>();
        }

        // Every thread runs the same number of iterations, so threads leave
        // the loop in the order they entered it, and the loop's reorder
        // buffer never holds one back.
        for (const index_t<KcMax> kc : kcount)
        {
            const Elem[Kv] a = _a[i * KcMax + kc];
            static for (const auto l : Lanes)
            {
                const Elem[Kv] b = _b[l][jt * KcMax + kc];
                acc[l] = acc_add<Acc>(acc[l], dot<Elem, Acc, Kv>(a, b));
            }
        }

        _c[c_addr] = acc;
    }

public:
    //| The engine's shape, for a driver to discover at run time.
    Geometry geometry()
    {
        return {Kv, Lanes, MaxM, MaxN, MaxK};
    }

    //| Store chunk `kc` (elements `kc * Kv ...`) of row `row` of `A`.
    void load_a(index_t<MaxM> row, index_t<KcMax> kc, Elem[Kv] data)
    {
        _a[row * KcMax + kc] = data;
    }

    //| Store chunk `kc` of row `row` of `B`.
    void load_b(index_t<MaxN> row, index_t<KcMax> kc, Elem[Kv] data)
    {
        const auto addr = (row / Lanes) * KcMax + kc;
        static for (const auto l : Lanes)
        {
            if (row % Lanes == l)
            {
                _b[l][addr] = data;
            }
        }
    }

    //| `C = A * B^T` over the first `m` rows of `A`, the first
    // `strips * Lanes` rows of `B` and the first `chunks * Kv` columns of
    // both; with `accumulate`, `C += A * B^T`. Returns once all of `C` is
    // written.
    void compute(count_t<MaxM> m, count_t<NtMax> strips, count_t<KcMax> chunks, bool accumulate)
    {
        pipelined_for(m, [strips, chunks, accumulate](index_t<MaxM> i)
        {
            pipelined_for(strips, [i, chunks, accumulate](index_t<NtMax> jt)
            {
                strip(i, jt, chunks, accumulate);
            });
        });
    }

    //| Strip `strip` of row `row` of `C`: columns `strip * Lanes ...`.
    Acc[Lanes] read_c(index_t<MaxM> row, index_t<NtMax> strip)
    {
        return _c[row * NtMax + strip];
    }
}
