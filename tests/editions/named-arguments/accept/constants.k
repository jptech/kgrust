// A constant reads nothing a call could write, so an argument that reads
// only constants and parameters may be placed before one written ahead of
// it that calls a function: a constant array or memory indexed, a constant
// structure's field, an enum's constant, a class template's constant —
// whether or not it has folded to a literal by the time the call is placed.

enum Mode : uint8
{
    Idle,
    Run
}

struct Pair
{
    uint8 a;
    uint8 b;
}

const uint8[3] Table = {1, 2, 3};
const Pair Both = {4, 5};
const memory<uint8, 4> Rom = {6, 7, 8, 9};

template <auto N>
class Scaled
{
private:
    uint8 _m;
    const uint8[4] Steps = {N, N * 2, N * 3, N * 4};

    uint8 g()
    {
        _m++;
        return _m;
    }

    uint16 sub(uint8 x, uint8 y)
    {
        return x - y;
    }

public:
    uint16 run(uint2 p)
    {
        return sub(y: g(), x: Steps[p]);
    }
}

class C
{
private:
    uint8 _m;
    Scaled<3> _scaled;

    uint8 g()
    {
        _m++;
        return _m;
    }

    uint16 sub(uint8 x, uint8 y)
    {
        return x - y;
    }

    uint16 pick(Mode m, uint8 y)
    {
        return m == Mode::Run ? y : 0;
    }

public:
    uint16 run(uint2 p)
    {
        uint16 a = sub(y: g(), x: Table[1]);
        uint16 b = sub(y: g(), x: Table[p] + Both.b);
        uint16 c = sub(y: g(), x: Rom[p]);
        uint16 d = pick(y: g(), m: Mode::Run);
        uint16 e = _scaled.run(p);
        return a + b + c + d + e;
    }
}

export C;
