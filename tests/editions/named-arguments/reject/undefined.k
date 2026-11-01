// A name that names nothing in a call that names its arguments is the
// ordinary error at the name, as the reference reports the call written in
// either order — not the order of evaluation's, which cannot class what an
// argument reads while it does not know what it names.
// expect-error 21:24: Symbol name not in scope: `nothing`
// expect-error 22:16: Symbol name not in scope: `nowhere`

inline uint8 g()
{
    return 1;
}

inline uint8 sub(uint8 x, uint8 y)
{
    return x - y;
}

inline uint8 f(uint8 p)
{
    uint8 a = 0;
    a = sub(y: g(), x: nothing);
    a = sub(y: nowhere, x: g());
    return a;
}
