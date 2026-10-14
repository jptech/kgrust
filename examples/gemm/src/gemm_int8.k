// The GEMM engine in int8, accumulating in int32, exported as `GemmInt8`.
import gemm.config as config
import gemm.engine

//| The GEMM engine, `C = A * B^T`, as an exported class: see `gemm.engine`.
class GemmInt8
{
private:
    Engine<int8, int32, config::int8_kv, config::int8_lanes, config::max_m, config::max_n, config::max_k> _engine;

    const auto Kc = config::max_k / config::int8_kv;
    const auto Nt = config::max_n / config::int8_lanes;

public:
    Geometry geometry()
    {
        return _engine.geometry();
    }

    void load_a(index_t<config::max_m> row, index_t<Kc> kc, int8[config::int8_kv] data)
    {
        _engine.load_a(row, kc, data);
    }

    void load_b(index_t<config::max_n> row, index_t<Kc> kc, int8[config::int8_kv] data)
    {
        _engine.load_b(row, kc, data);
    }

    void compute(count_t<config::max_m> m, count_t<Nt> strips, count_t<Kc> chunks, bool accumulate)
    {
        _engine.compute(m, strips, chunks, accumulate);
    }

    int32[config::int8_lanes] read_c(index_t<config::max_m> row, index_t<Nt> strip)
    {
        return _engine.read_c(row, strip);
    }
}

export GemmInt8;
