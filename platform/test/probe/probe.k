// Every protocol an exported function can have, for the board tooling's
// own tests (test_probe.py): the interface description, the generated core's
// bridge, and kgbridge's two transports. Nothing binds to a board
// resource, so the bridge serves it all.

struct Pair
{
    uint8 a;
    int4 b;
}

class Probe
{
private:
    uint32 _count;

    // A callback with a handshake and no result, one with a result, and a
    // fixed-latency pulse.
    (uint8 c) -> void _note;
    (uint8 x, bool flag) -> uint8 _ask;
    [[latency(0)]] (uint8 c) -> void _pulse;

public:
    //| Adds x + y to the count, then tells the host x.
    void plain(uint8 x, int16 y)
    {
        atomic
        {
            _count = _count + cast<uint32>(x) + cast<uint32>(y);
        }
        _note(x);
    }

    //| The host's answer for x, plus x.
    uint16 handshake(uint8 x)
    {
        return _ask(x, x > 100) + x;
    }

    //| Pulses b to the host, never refusing.
    [[no_backpressure]] void nbp(bool b)
    {
        _pulse(b ? 1 : 0);
    }

    //| v + 1, never refusing.
    [[no_backpressure]] uint8 nbp_result(uint8 v)
    {
        return v + 1;
    }

    //| 256 + i, a cycle later.
    [[latency(1)]] uint9 lat1(uint3 i)
    {
        return 256 + i;
    }

    //| v ^ 0x5A, in the same cycle.
    [[latency(0)]] uint8 lat0(uint8 v)
    {
        return v ^ 0x5A;
    }

    //| The pair with a + 1 and b negated.
    Pair pair(Pair p)
    {
        return {p.a + 1, -p.b};
    }

    //| -v.
    int8 negate(int8 v)
    {
        return -v;
    }

    //| The count `plain` keeps.
    uint32 count()
    {
        return _count;
    }
}

export Probe;
