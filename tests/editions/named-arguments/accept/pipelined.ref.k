// `pipelined.k` in edition 1.0.

class Main
{
    [[pipelined]] void Work(uint32 tid, bool flag, uint8 v)
    {
    }

public:
    void main(bool f)
    {
        Work(4, f, 3);
    }
}
