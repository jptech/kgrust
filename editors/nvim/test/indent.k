//| Every layout `require('kgrust').indent` knows, as `kg fmt` writes it;
//| `smoke.lua` asks it for each line's indent and expects the line's own.
module indent_fixture
    { Base
    , clamp
    , Window
    }

import data.array

/*|
A block comment's text keeps its own indent,
        however deep.
*/

enum Base : uint2
{
    RV32I,
    RV32E
}

enum Mode : uint2
{ Idle = 0 //< Waiting.
, Run = 1  //< Running.
}

template
    < auto Width //< The window's width.
                 // Continued under its `//<`.
    , typename T
    >
class Window
{
public:
    T clip(T value)
    {
        uint8[4] table = { 1, 2,
            3, 4 };
        static for (const auto i : 4)
            if (i == 0)
                table[i] = 0;
            else
                table[i] = 1;
        return value > Width ? Width : value;
    }

private:
    T _last;
}

const uint8[2][2] pairs =
{
    { 1, 2 },
    { 3, 4 }
};

inline uint8 clamp(uint8 x)
{
    if (x > 4)
    {
        return 4;
    }
    return x;
}
