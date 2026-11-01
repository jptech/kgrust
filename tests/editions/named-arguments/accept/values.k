// Callees that are values of function type take the names their declared
// type spells, not those of what is passed in: a function-typed parameter
// given a lambda, or a function, whose parameters have the same names in
// the other order; one whose type is an alias; and a template's function
// parameter given such a function. Placed by the lambda's names instead,
// every call here would subtract the other way round.

using Pair = (uint8 x, uint8 y) -> uint8;

inline uint8 sub(uint8 y, uint8 x)
{
    return y - x;
}

inline uint8 apply((uint8 x, uint8 y) -> uint8 g, uint8 v)
{
    return g(y: 3, x: v);
}

inline uint8 aliased(Pair g, uint8 v)
{
    return g(y: v, x: 5);
}

template <(uint8 x, uint8 y) -> uint8 F>
inline uint8 call(uint8 v)
{
    return F(y: 7, x: v);
}

inline uint32 main(uint8 p)
{
    auto swapped = [](uint8 y, uint8 x) -> uint8 { return y - x; };
    return apply(swapped, p) + apply(sub, p) + aliased(swapped, p) + call<sub>(p);
}
