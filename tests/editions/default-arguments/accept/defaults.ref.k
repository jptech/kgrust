// `defaults.k` in edition 1.0: every call written whole, each default
// written as it is declared.

enum Mode : uint2
{
    Idle,
    Run,
    Halt
}

const uint8 Step = 2;

inline uint8 scale(uint8 value, uint8 shift, bool round, Mode mode)
{
    return round && mode == Mode::Run ? value >> shift : value << shift;
}

template <auto Width>
class Window
{
public:
    uint8 clip(uint8 value, uint8 width)
    {
        return value > width ? width : value;
    }
}

class Main
{
    Window<5> _window;

    uint32 add(uint32 value, uint32 times)
    {
        return value * times;
    }

public:
    uint32 main(uint8 p)
    {
        uint8 a = scale(p, Step + 1, false, Mode::Run);
        uint8 b = scale(p, 1, false, Mode::Run);
        uint8 c = scale(p, 1, true, Mode::Run);
        uint8 d = scale(p, 1, true, Mode::Halt);
        return a + b + c + d + add(p, 1) + add(p, 3) + _window.clip(p, 5);
    }
}
