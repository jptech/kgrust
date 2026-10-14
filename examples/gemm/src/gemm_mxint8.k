// The GEMM engine in OCP MXINT8, accumulating in float32, exported as
// `GemmMxInt8`. Its element is a whole 32-element block, so `geometry`, the
// chunk indices and `max_k` count K in blocks.
import gemm.config as config
import gemm.engine
import gemm.format

//| The GEMM engine, `C = A * B^T`, as an exported class: see `gemm.engine`.
class GemmMxInt8
{
private:
    const auto Kb = config::max_k / mx_block_size;
    const auto Nt = config::max_n / config::mx_lanes;

    Engine<mxint8, float32, 1, config::mx_lanes, config::max_m, config::max_n, Kb> _engine;

public:
    Geometry geometry()
    {
        return _engine.geometry();
    }

    void load_a(index_t<config::max_m> row, index_t<Kb> kc, mxint8[1] data)
    {
        _engine.load_a(row, kc, data);
    }

    void load_b(index_t<config::max_n> row, index_t<Kb> kc, mxint8[1] data)
    {
        _engine.load_b(row, kc, data);
    }

    void compute(count_t<config::max_m> m, count_t<Nt> strips, count_t<Kb> chunks, bool accumulate)
    {
        _engine.compute(m, strips, chunks, accumulate);
    }

    float32[config::mx_lanes] read_c(index_t<config::max_m> row, index_t<Nt> strip)
    {
        return _engine.read_c(row, strip);
    }
}

export GemmMxInt8;
