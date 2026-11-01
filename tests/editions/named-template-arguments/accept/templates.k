// Template arguments named by their parameters: in any order, skipping
// parameters with defaults — a default that reads an earlier parameter
// reads the argument it was given — for a class template, a member of one,
// and a function template whose other arguments are deduced.

template <typename T, auto Size = 4, bool Wide = false, auto Shift = Size + 1>
class Buffer
{
public:
    T get(T x)
    {
        return Wide ? x << Shift : x + Size;
    }
}

template <typename T, auto N = 2>
inline T scaled(T x)
{
    return x * N;
}

class Main
{
    Buffer<uint8, Wide: true> _a;
    Buffer<Size: 8, T: uint16> _b;
    Buffer<uint8, 3, Shift: 1> _c;
    Buffer<Shift: 2, Wide: true, T: uint32, Size: 6> _d;

public:
    uint32 run(uint8 p)
    {
        return _a.get(p) + _b.get(p) + _c.get(p) + _d.get(p) + scaled<uint8, N: 3>(p) + scaled<N: 5>(p);
    }
}
