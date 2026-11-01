// A label on a call of a function template names a parameter of the
// template's function, whether its template arguments are still to be
// deduced or written; one that names none, or a parameter given an
// argument twice, is an error at it, as on a call of a function — never
// that the template is no function whose parameters have names.
// expect-error 31:23: `scaled` has no parameter named `y`
// expect-error 32:25: `fixed` has no parameter named `y`
// expect-error 33:34: `pick` has no parameter named `c`
// expect-error 34:27: the parameter `first` is given an argument twice

template <typename T, auto N = 2>
inline T scaled(T x)
{
    return x * N;
}

template <auto N>
inline uint8 fixed(uint8 x)
{
    return x * N;
}

template <typename T>
inline T pick(bool first, T a, T b)
{
    return first ? a : b;
}

inline uint32 main(uint8 p)
{
    uint32 a = scaled(y: p);
    uint32 b = fixed<3>(y: p);
    uint32 c = pick(first: true, c: p, a: p);
    uint32 d = pick(true, first: false, a: p, b: p);
    return a + b + c + d;
}
