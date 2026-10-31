// Calls short of arguments, each given the defaults it leaves out: an
// integer folded from a constant, a `bool`, an enum's constant, a method's,
// and a class template's parameter.

enum Mode : uint2
{
    Idle,
    Run,
    Halt
}

const uint8 Step = 2;

inline uint8 scale(uint8 value, uint8 shift = Step + 1, bool round = false, Mode mode = Mode::Run)
{
    return round && mode == Mode::Run ? value >> shift : value << shift;
}

template <auto Width>
class Window
{
public:
    uint8 clip(uint8 value, uint8 width = Width)
    {
        return value > width ? width : value;
    }
}

class Main
{
    Window<5> _window;

    uint32 add(uint32 value, uint32 times = 1)
    {
        return value * times;
    }

public:
    uint32 main(uint8 p)
    {
        uint8 a = scale(p);
        uint8 b = scale(p, 1);
        uint8 c = scale(p, 1, true);
        uint8 d = scale(p, 1, true, Mode::Halt);
        return a + b + c + d + add(p) + add(p, 3) + _window.clip(p);
    }
}
