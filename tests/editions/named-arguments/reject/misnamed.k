// What a call that names its arguments can get wrong, each reported once,
// at the argument it is about (`docs/editions/1.1.md`, "Errors").
//
// expect-error 30:30: `sub` has no parameter named `z`
// expect-error 31:20: `sub` is missing an argument for `x`
// expect-error 32:27: the parameter `x` is given an argument twice
// expect-error 33:32: in `sub`'s parameter order `x` would be evaluated before `y`, which is written first, and one of them calls a function: write them in the parameters' order, or bind one to a local first
// expect-error 34:31: in `sub`'s parameter order `x` would be evaluated before `y`, which is written first, and one of them calls a function: write them in the parameters' order, or bind one to a local first
// expect-error 35:20: `p` is not a function whose parameters have names
// expect-error 36:17: `tid` is the thread's index in pipelined `Work`, which no call passes: the first argument, written by position, is the thread count
// expect-error 37:9: `Work` is missing an argument for the thread count

class C
{
private:
    uint32 _m;
    uint32 g() { _m++; return _m; }
    uint32 h() { return _m; }
    uint16 sub(uint8 x, uint8 y)
    {
        return x - y;
    }
    [[pipelined]] void Work(uint32 tid, bool flag)
    {
    }

public:
    uint32 run(uint8 p)
    {
        uint16 a = sub(y: p, z: p);
        uint16 b = sub(y: p);
        uint16 c = sub(p, x: p);
        uint32 d = sub(y: g(), x: h());
        uint32 e = sub(y: _m, x: g());
        uint32 n = p(x: 1);
        Work(4, tid: 1, flag: true);
        Work(flag: true);
        return a + b + c + d + e + n;
    }
}
