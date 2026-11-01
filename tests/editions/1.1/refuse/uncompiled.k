// Where nothing compiles a call, the loop never placed its arguments, and
// there is nothing to read their places off: a template no one instantiates,
// and a `static if` branch the program does not keep. A call by position that
// may lean on a default is the same, whose default is about to be deleted — and
// a call by position of a function with no default needs no edit at all.
// expect-refusal 26:19: cannot write this call of `add` in edition 1.0: the program compiles no copy of it — no template instance or `static if` branch it keeps holds it, or it was folded to a value — so nothing says where its arguments go
// expect-refusal 31:20: cannot write this call of `bump` in edition 1.0: the program compiles no copy of it — no template instance or `static if` branch it keeps holds it, or it was folded to a value — so nothing says where its arguments go
// expect-refusal 47:23: cannot write this call of `add` in edition 1.0: the program compiles no copy of it — no template instance or `static if` branch it keeps holds it, or it was folded to a value — so nothing says where its arguments go

inline uint8 add(uint8 x, uint8 y)
{
    return x + y;
}

inline uint8 bump(uint8 x, uint8 by = 1)
{
    return x + by;
}

template <typename T>
class Unused
{
public:
    uint8 named(uint8 p)
    {
        return add(y: p, x: 1);
    }

    uint8 leaning(uint8 p)
    {
        return bump(p);
    }

    uint8 plain(uint8 p)
    {
        return add(p, 1);
    }
}

class Main
{
public:
    uint8 run(uint8 p)
    {
        static if (false)
        {
            return add(y: p, x: 2);
        }
        return add(x: p, y: bump(p));
    }
}

export Main;
