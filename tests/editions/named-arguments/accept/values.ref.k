// `values.k` in edition 1.0.

using Pair = (uint8 x, uint8 y) -> uint8;

inline uint8 sub(uint8 y, uint8 x)
{
    return y - x;
}

inline uint8 apply((uint8 x, uint8 y) -> uint8 g, uint8 v)
{
    return g(v, 3);
}

inline uint8 aliased(Pair g, uint8 v)
{
    return g(5, v);
}

template <(uint8 x, uint8 y) -> uint8 F>
inline uint8 call(uint8 v)
{
    return F(v, 7);
}

inline uint32 main(uint8 p)
{
    auto swapped = [](uint8 y, uint8 x) -> uint8 { return y - x; };
    return apply(swapped, p) + apply(sub, p) + aliased(swapped, p) + call<sub>(p);
}
