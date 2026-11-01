// Edition 1.1's call errors where features meet: a default skipped by name
// while a parameter with none is left out; a parameter given by position and
// again by name; a named template argument for a parameter given by
// position. Each is one error, at the call or the argument it names.
//
// expect-error 22:18: the template parameter `T` is given an argument twice
// expect-error 28:19: `put` is missing an argument for `x`
// expect-error 29:33: the parameter `x` is given an argument twice

template <typename T, auto Depth = 4, bool Wide = false>
class Store
{
public:
    T put(T x, uint8 by = 1, bool twice = false)
    {
        return twice ? x + by : x;
    }
}

class Acc
{
    Store<uint8, T: uint16> _bad;
    Store<uint8, Wide: true> _store;

public:
    uint8 run(uint8 p)
    {
        uint8 a = _store.put(twice: true);
        uint8 b = _store.put(p, x: p,);
        return a + b;
    }
}

export Acc;
