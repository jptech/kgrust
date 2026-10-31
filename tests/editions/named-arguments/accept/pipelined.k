// A pipelined function's first argument is the thread count, in the place
// of the thread's index; the parameters after it may be named.

class Main
{
    [[pipelined]] void Work(uint32 tid, bool flag, uint8 v)
    {
    }

public:
    void main(bool f)
    {
        Work(4, v: 3, flag: f);
    }
}
