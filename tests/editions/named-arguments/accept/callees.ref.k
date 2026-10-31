// `callees.k` in edition 1.0.

template <typename T>
inline T diff(T first, T second)
{
    return first - second;
}

inline uint8 apply((uint8 a, uint8 b) -> uint8 fn, uint8 x)
{
    return fn(1, x);
}

inline uint32 main(uint8 p, uint8 q)
{
    auto sub = [](uint8 a, uint8 b) -> uint8 { return a - b; };
    uint8 r = sub(p, q);
    uint32 s = diff<uint32>(p, q);
    return r + s + apply(sub, p);
}
