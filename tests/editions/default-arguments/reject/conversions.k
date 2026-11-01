// A default is judged as the argument it stands for, against its
// parameter's type: at the default, once — whether or not a call is given
// it, and not again at each call that is — and a template's in each
// instance, where its parameter has a type.
// expect-error 15:40: Invalid conversion from `bool` to `uint8`
// expect-error 20:39: Invalid conversion from `uint2` to `Mode`
// expect-error 26:26: Invalid conversion from `bool` to `uint8`

enum Mode : uint8
{
    Idle,
    Run
}

inline uint8 scale(uint8 x, uint8 by = true)
{
    return x << by;
}

inline uint8 unused(uint8 x, Mode m = 3)
{
    return x;
}

template <typename T>
inline T pick(T x, T y = true)
{
    return x;
}

class Main
{
public:
    uint8 main(uint8 p)
    {
        bool b = pick(true);
        return scale(p) + scale(p, 1) + pick(p) + (b ? 1 : 0);
    }
}
