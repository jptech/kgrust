// A call that names its arguments is counted against its callee's
// parameters: a positional argument past the last is an error at it (and
// a label after it names a parameter already given one), and a function
// of no parameters takes no label at all.
// expect-error 21:22: `sub` takes 2 arguments
// expect-error 21:25: the parameter `y` is given an argument twice
// expect-error 21:33: `none` takes no parameters to name

inline uint8 sub(uint8 x, uint8 y)
{
    return x - y;
}

inline uint8 none()
{
    return 1;
}

inline uint8 main(uint8 p, uint8 q)
{
    return sub(p, q, p, y: q) + none(x: p);
}
