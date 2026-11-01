// A call still gives every parameter without a default an argument; the
// count is the reference's error, as for a function with none.
// expect-error 15:20: Invalid number of arguments, expected 3

class Main
{
    uint8 mix(uint8 value, uint8 by, bool flip = false)
    {
        return flip ? value - by : value + by;
    }

public:
    uint8 main(uint8 p)
    {
        return mix(p);
    }
}
