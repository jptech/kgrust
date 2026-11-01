// Edition 1.1 whole, read in the edition (`docs/editions/1.1.md`,
// "Together"): a call that skips a default by naming a later parameter,
// named arguments beside defaults in any order, a class template's arguments
// named past its defaults, and a trailing comma closing each list — calls
// that no one feature alone could write.

template <typename T, auto Depth = 4, bool Wide = false, auto Shift = Depth + 1,>
class Store
{
public:
    T put(T x, uint8 by = Shift, bool twice = false,)
    {
        T y = Wide ? x << by : x + by;
        return twice ? y + y : y;
    }
}

class Acc
{
    uint32 _total = 0;
    Store<uint8, Wide: true,> _store;
    Store<Shift: 2, T: uint16,> _other;

    uint32 add(
        uint32 value,
        uint8 shift = 0,
        bool saturate = false,
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
            value: a,
            saturate: true,
        );
        uint32 d = add(shift: 2, value: c,);
        uint8 e = _store.put(p, twice: true,);
        uint16 f = _other.put(by: 1, x: p);
        return d + e + f;
    }
}

export Acc;
