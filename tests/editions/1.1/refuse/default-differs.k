// A default that names the class template's parameter is another value in
// each instance, and the one call in the template's body leans on it: `put(x)`
// is `put(x, 5)` in one and `put(x, 6)` in the other, which no one text says.
// expect-refusal 17:19: cannot write this list in edition 1.0: the template instances that compile it give it different arguments — `put(x, 5)` in `Window<5>` and `put(x, 6)` in `Window<6>` — and one text cannot say both

template <auto N>
class Window
{
public:
    uint32 put(uint32 x, uint32 by = N)
    {
        return x + by;
    }

    uint32 again(uint32 x)
    {
        return put(x);
    }
}

class Main
{
    Window<5> _five;
    Window<6> _six;

public:
    uint32 run(uint32 x)
    {
        return _five.again(x) + _six.again(x);
    }
}

export Main;
