// A function template's parameter left to deduction, before one that is named:
// edition 1.0 must write it, and the two instances of the class deduce it
// differently, so `pick<B: uint8>(x)` has no one positional spelling.
// expect-refusal 18:20: cannot write this list in edition 1.0: the template instances that compile it give it different arguments — `pick<uint16, uint8>` in `Holder<uint16>` and `pick<uint32, uint8>` in `Holder<uint32>` — and one text cannot say both

template <typename A, typename B>
inline B pick(A a)
{
    return cast<B>(a);
}

template <typename T>
class Holder
{
public:
    uint8 low(T x)
    {
        return pick<B: uint8>(x);
    }
}

class Main
{
    Holder<uint16> _narrow;
    Holder<uint32> _wide;

public:
    uint8 run(uint16 x, uint32 y)
    {
        return _narrow.low(x) + _wide.low(y);
    }
}

export Main;
