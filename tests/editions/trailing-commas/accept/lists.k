// A trailing comma in every list that may hold one: enum constants, template
// parameters and arguments, parameters, a function type's parameters, call
// arguments, and both shapes of initializer list. Written one element a line,
// as a list that grows a line at a time is.

enum Mode : uint2
{
    Idle,
    Run,
    Halt,
}

struct Pair
{
    uint8 lo;
    uint8 hi;
}

template <
    typename T,
    auto Scale,
>
class Scaler
{
public:
    T scale(
        T x,
        T offset,
    )
    {
        return x * Scale + offset;
    }
}

inline uint8 apply(
    (uint8, uint8,) -> uint8 f,
    uint8 x,
)
{
    return f(x, 1,);
}

class C
{
private:
    Scaler<
        uint8,
        3,
    > _scaler;

public:
    uint8 run(uint8 p)
    {
        uint8[3] xs = {
            p,
            1,
            2,
        };
        Pair pair = {
            .lo = p,
            .hi = xs[1],
        };
        Mode m = Mode::Run;
        uint8 a = _scaler.scale(
            pair.lo,
            pair.hi,
        );
        uint8 b = apply(
            [](uint8 x, uint8 y,) -> uint8 { return x + y; },
            a,
        );
        return m == Mode::Run ? b : xs[2];
    }
}

export C;
