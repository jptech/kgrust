// `together.k` in edition 1.0: every call and template argument list
// written whole, by position, each default written as it is declared.

template <typename T, auto Depth = 4, bool Wide = false, auto Shift = Depth + 1>
class Store
{
public:
    T put(T x, uint8 by, bool twice)
    {
        T y = Wide ? x << by : x + by;
        return twice ? y + y : y;
    }
}

class Acc
{
    uint32 _total = 0;
    Store<uint8, 4, true> _store;
    Store<uint16, 4, false, 2> _other;

    uint32 add(
        uint32 value,
        uint8 shift,
        bool saturate
    )
    {
        uint33 sum = _total + (value << shift);
        _total = saturate && sum > 0xFFFFFFFF ? 0xFFFFFFFF : cast<uint32>(sum);
        return _total;
    }

public:
    uint32 run(uint32 a, uint8 p)
    {
        uint32 c = add(
            a,
            0,
            true
        );
        uint32 d = add(c, 2, false);
        uint8 e = _store.put(p, 5, true);
        uint16 f = _other.put(p, 1, false);
        return d + e + f;
    }
}

export Acc;
