// A default that makes a template's instance no other text makes: the default
// goes, and with it the only reason the program has for `Pair<1, 5>`, which the
// backend walks. The text checks clean in edition 1.0 and is another program,
// and only the walks can tell.
// expect-refusal 24:21: cannot write this program in edition 1.0: its text is another program, as the walks into the backend show

template <auto N = 1, auto M = 2>
struct Pair
{
    uint8[N] a;
    uint8[M] b;
}

inline uint32 grow(uint32 x, uint32 by = bitsizeof(Pair<M: 5>))
{
    return x + by;
}

class Main
{
public:
    uint32 run(uint32 p)
    {
        return grow(p);
    }
}

export Main;
