// Arguments named in another order than their parameters, across the kinds
// of callee a class reaches: its own method, a member object's method, a
// template class's method (its parameters known once `Box<uint8>` is), an
// inline function of `auto` parameters, a call nested in a named argument,
// and positional arguments before named ones. `sub(y: g(), x: p)` calls a
// function after reading a parameter it is placed before, which no callee
// can tell: a parameter commutes with anything.

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
        uint16 a = sub(y: q, x: p);
        uint16 b = sub(p, y: q);
        uint32 c = _acc.add(shift: 2, value: a);
        uint8 d = _box.scale(by: 3, x: p);
        uint8 e = pick(first: true, b: q, a: p);
        uint32 f = _acc.add(shift: p, value: _acc.add(value: 1, shift: 0));
        uint16 h = sub(y: g(), x: p);
        return a + b + c + d + e + f + h;
    }
}

export C;
