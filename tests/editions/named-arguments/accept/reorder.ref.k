// `reorder.k` in edition 1.0: every argument in its parameter's place.

class Acc
{
private:
    uint32 _total = 0;

public:
    uint32 add(uint32 value, uint8 shift)
    {
        _total += value << shift;
        return _total;
    }
}

template <typename T>
class Box
{
public:
    T scale(T x, T by)
    {
        return x * by;
    }
}

inline auto pick(auto a, auto b, bool first)
{
    return first ? a : b;
}

class C
{
private:
    Acc _acc;
    Box<uint8> _box;

    uint16 sub(uint8 x, uint8 y)
    {
        return x - y;
    }

    uint8 g()
    {
        return 1;
    }

public:
    uint32 run(uint8 p, uint8 q)
    {
        uint16 a = sub(p, q);
        uint16 b = sub(p, q);
        uint32 c = _acc.add(a, 2);
        uint8 d = _box.scale(p, 3);
        uint8 e = pick(p, q, true);
        uint32 f = _acc.add(_acc.add(1, 0), p);
        uint16 h = sub(p, g());
        return a + b + c + d + e + f + h;
    }
}

export C;
