// The GEMM engine in float32, accumulating in float32, exported as `GemmFp32`.
import gemm.config as config
import gemm.engine

//| The GEMM engine, `C = A * B^T`, as an exported class: see `gemm.engine`.
class GemmFp32
{
private:
    Engine<float32, float32, config::fp32_kv, config::fp32_lanes, config::max_m, config::max_n, config::max_k> _engine;

    const auto Kc = config::max_k / config::fp32_kv;
    const auto Nt = config::max_n / config::fp32_lanes;

public:
    Geometry geometry()
    {
        return _engine.geometry();
    }

    void load_a(index_t<config::max_m> row, index_t<Kc> kc, float32[config::fp32_kv] data)
    {
        _engine.load_a(row, kc, data);
    }

    void load_b(index_t<config::max_n> row, index_t<Kc> kc, float32[config::fp32_kv] data)
    {
        _engine.load_b(row, kc, data);
    }

    void compute(count_t<config::max_m> m, count_t<Nt> strips, count_t<Kc> chunks, bool accumulate)
    {
        _engine.compute(m, strips, chunks, accumulate);
    }

    float32[config::fp32_lanes] read_c(index_t<config::max_m> row, index_t<Nt> strip)
    {
        return _engine.read_c(row, strip);
    }
}

export GemmFp32;
