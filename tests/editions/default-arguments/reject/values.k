// A default belongs to a declaration, not to a type: a call through a value
// of function type — a parameter, a template's parameter, a closure — is
// given none, whatever the function or lambda passed in declares, and one
// short of an argument is the reference's own count error, as in its twin
// with no default (a closure's count includes its capture).
// expect-error 12:14: Invalid number of arguments, expected 3
// expect-error 23:14: Invalid number of arguments, expected 2
// expect-error 29:43: Invalid number of arguments, expected 3

inline uint8 apply((uint8 x, uint8 y) -> uint8 g, uint8 v)
{
    return g(v);
}

inline uint8 sub(uint8 a, uint8 b = 1)
{
    return a - b;
}

template <(uint8 x, uint8 y) -> uint8 F>
inline uint8 call(uint8 v)
{
    return F(v);
}

inline uint8 main(uint8 p)
{
    auto f = [](uint8 a, uint8 b = 1) -> uint8 { return a - b; };
    return apply(f, p) + call<sub>(p) + f(p);
}
