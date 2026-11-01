// A value of function type takes the names its declared type spells — or
// none, where it spells none — and never the names of what is passed in:
// the lambda, the function given for a template's parameter.
// expect-error 12:14: `g` has no parameter named `b`
// expect-error 12:20: `g` has no parameter named `a`
// expect-error 17:12: `g`'s type gives its parameters no names, so a call through it passes them by position
// expect-error 23:14: `F` has no parameter named `a`
// expect-error 23:20: `F` has no parameter named `b`

inline uint8 named((uint8 x, uint8 y) -> uint8 g, uint8 v)
{
    return g(b: 3, a: v);
}

inline uint8 unnamed((uint8, uint8) -> uint8 g, uint8 v)
{
    return g(y: 3, x: v);
}

template <(uint8 x, uint8 y) -> uint8 F>
inline uint8 call(uint8 v)
{
    return F(a: 7, b: v);
}

inline uint8 sub(uint8 a, uint8 b)
{
    return a - b;
}

inline uint8 main(uint8 p)
{
    auto f = [](uint8 a, uint8 b) -> uint8 { return a - b; };
    auto h = [](uint8 x, uint8 y) -> uint8 { return x - y; };
    return named(f, p) + unnamed(h, p) + call<sub>(p);
}
