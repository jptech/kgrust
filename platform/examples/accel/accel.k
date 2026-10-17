// An accelerator called from Python, with no RTL written for it.
//
// Nothing here is named for a board resource except the LED callback, so
// the generated core serves every other function through the host bridge
// on the board's serial port, and `kg board` writes a Python module with
// one method per function (target/board/py/accel.py):
//
//     api = Accel()            # under `kg board serve`; Accel(port=...) on a board
//     api.add(2, 3)            # 5
//     api.on("_square", lambda x: x * x)
//     api.sum_of_squares(4)    # 30: the design called back into Python four times
//
// test_accel.py runs the same tests under cocotb (`kg board test`), against
// the simulated board (`kg board serve -- python3 test_accel.py`) and on
// the board itself (`python3 test_accel.py --port /dev/ttyUSB0`).
//
// Sized for an iCE40: what can be fixed-latency is, because a method with a
// handshake keeps a FIFO for its arguments and one for its result, in flip-
// flops at these widths, a few hundred cells each. Only `sum_of_squares`
// waits on the host, so only it needs the handshake.

struct Point
{
    int16 x;
    int16 y;
}

//| Arithmetic, a running total, a callback into the host, and the LED.
class Accel
{
private:
    uint32 _total;

    //| Asks the host for x squared.
    (uint16 x) -> uint32 _square;

    //| The board's LED: binds to the `led` resource by its name.
    [[latency(0)]] (uint3 rgb) -> void _led;

public:
    //| a + b, wrapping at 32 bits.
    [[latency(1)]] uint32 add(uint32 a, uint32 b)
    {
        return a + b;
    }

    //| a * b, in one of the UP5K's DSP blocks.
    [[latency(2)]] uint32 mul(uint16 a, uint16 b)
    {
        return a * b;
    }

    //| Adds x to the running total and returns the new total.
    [[latency(1)]] uint32 accumulate(uint32 x)
    {
        uint32 total;
        atomic
        {
            total = _total + x;
            _total = total;
        }
        return total;
    }

    //| 1² + 2² + … + n², each square asked of the host through `_square`.
    uint32 sum_of_squares(uint8 n)
    {
        uint32 sum = 0;
        uint9 i = 1;
        if (n != 0)
        {
            do
            {
                sum = sum + _square(i);
                i = i + 1;
            } while (i <= n);
        }
        return sum;
    }

    //| The point reflected through the origin: a struct each way.
    [[latency(1)]] Point negate(Point p)
    {
        return {-p.x, -p.y};
    }

    //| Lights the LED: {red, green, blue}.
    [[latency(0)]] void show(uint3 rgb)
    {
        _led(rgb);
    }
}

export Accel;
