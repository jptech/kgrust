// A default spelled by its enum's name, written into a call that stands where
// the name means another enum: read back, the text is a different constant of a
// different type. The proof finds it, and the call is refused.
// expect-refusal 28:21: cannot write `Color::Green`, the argument this call of `code` is given, in edition 1.0: Invalid conversion from `const Main::Color` to `Color`

enum Color : uint2
{
    Red,
    Green
}

inline uint2 code(Color c = Color::Green)
{
    return cast<uint2>(c);
}

class Main
{
    enum Color : uint2
    {
        Green,
        Red
    }

public:
    uint2 run()
    {
        return code();
    }
}

export Main;
