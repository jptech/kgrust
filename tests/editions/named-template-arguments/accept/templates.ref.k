// `templates.k` in edition 1.0: every template argument written by
// position, each default left out written as it is declared.

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
    Buffer<uint8, 4, true> _a;
    Buffer<uint16, 8> _b;
    Buffer<uint8, 3, false, 1> _c;
    Buffer<uint32, 6, true, 2> _d;

public:
    uint32 run(uint8 p)
    {
        return _a.get(p) + _b.get(p) + _c.get(p) + _d.get(p) + scaled<uint8, 3>(p) + scaled<uint8, 5>(p);
    }
}
