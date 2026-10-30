// Callees whose parameters come from a type rather than a declaration: a
// closure, a function-typed parameter (the names its type spells), and a
// function template given its arguments explicitly.

template <typename T>
inline T diff(T first, T second)
{
    return first - second;
}

inline uint8 apply((uint8 a, uint8 b) -> uint8 fn, uint8 x)
{
    return fn(b: x, a: 1);
}

inline uint32 main(uint8 p, uint8 q)
{
    auto sub = [](uint8 a, uint8 b) -> uint8 { return a - b; };
    uint8 r = sub(b: q, a: p);
    uint32 s = diff<uint32>(second: q, first: p);
    return r + s + apply(x: p, fn: sub);
}
