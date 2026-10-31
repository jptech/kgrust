// What a template's named arguments can get wrong, each reported at the
// argument it is about: a name no parameter has, a parameter given twice,
// and a parameter with no default left out, at the instance.
// expect-error 27:19: `Buffer` has no template parameter named `Depth`
// expect-error 28:19: the template parameter `T` is given an argument twice
// expect-error 29:5: `Buffer` is missing a template argument for `T`
// expect-error 34:23: `scaled` has no template parameter named `M`

template <typename T, auto Size = 4, bool Wide = false>
class Buffer
{
public:
    T get(T x)
    {
        return Wide ? x : x + Size;
    }
}

template <typename T, auto N = 2>
inline T scaled(T x)
{
    return x * N;
}

class Main
{
    Buffer<uint8, Depth: 3> _a;
    Buffer<uint8, T: uint4> _b;
    Buffer<Size: 8> _c;

public:
    uint32 run(uint8 p)
    {
        return scaled<M: 3>(p);
    }
}
