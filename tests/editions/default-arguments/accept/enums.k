// An enum's constant as the default of a parameter that is no enum: an
// integer parameter takes the constant as it takes the argument
// (`TypeCheck.hs:155-156`), and a call short of it is given the constant
// by its enum's name — from a method, from a class template's instance,
// and short of the last alone.

enum Mode : uint2
{
    Idle,
    Run,
    Halt
}

inline uint8 code(uint8 base, uint8 mode = Mode::Halt, uint8 step = 1)
{
    return base + mode + step;
}

template <auto Offset>
class Shifted
{
public:
    uint8 coded(uint8 p)
    {
        return code(p + Offset);
    }
}

class Main
{
private:
    Shifted<2> _shifted;

public:
    uint8 run(uint8 p)
    {
        return code(p) + code(p, 2) + _shifted.coded(p);
    }
}
