// A default is a constant: not a member, nor another parameter.
// expect-error 8:44: the default of `shift` is not a constant a call can be given: a default is a constant integer, `bool`, `float` or enum value
// expect-error 13:44: the default of `other` is not a constant a call can be given: a default is a constant integer, `bool`, `float` or enum value

class Main
{
    uint8 _base = 1;
    uint8 scale(uint8 value, uint8 shift = _base)
    {
        return value << shift;
    }

    uint8 twice(uint8 value, uint8 other = value)
    {
        return value + other;
    }

public:
    uint8 main(uint8 p)
    {
        return scale(p) + twice(p);
    }
}
