// The twin of `enums.k`: every call written whole.

enum Mode : uint2
{
    Idle,
    Run,
    Halt
}

inline uint8 code(uint8 base, uint8 mode, uint8 step)
{
    return base + mode + step;
}

template <auto Offset>
class Shifted
{
public:
    uint8 coded(uint8 p)
    {
        return code(p + Offset, Mode::Halt, 1);
    }
}

class Main
{
private:
    Shifted<2> _shifted;

public:
    uint8 run(uint8 p)
    {
        return code(p, Mode::Halt, 1) + code(p, 2, 1) + _shifted.coded(p);
    }
}
