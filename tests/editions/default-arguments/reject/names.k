// A name in a default that names nothing is the ordinary error, at the
// name, as anywhere else — and the calls that lean on the default are not
// reported again.
// expect-error 9:37: Symbol name not in scope: `Shift`
// expect-error 14:35: Symbol name not in scope: `Step`

class Main
{
    uint8 scale(uint8 x, uint8 by = Shift)
    {
        return x << by;
    }

    uint8 add(uint8 x, uint8 by = Step + 1)
    {
        return x + by;
    }

public:
    uint8 main(uint8 p)
    {
        return scale(p) + scale(p, 1) + add(p);
    }
}
