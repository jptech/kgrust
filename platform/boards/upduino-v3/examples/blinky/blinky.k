// UPduino v3 blinky: a free-running counter on the RGB LED.
//
// Written for the iCE40's size (platform-layer.md §8):
// - `[[latency(0)]]`: the board core calls `step` every cycle and reads the
//   result in the same cycle, so the method has no ready, no output valid and
//   no return FIFO.
// - No initialiser on `_count`: iCE40 flip-flops leave configuration at
//   zero, and an initial value would add reset logic to every bit.
//
// At 24 MHz bit 23 of the counter moves every 0.35 s, stepping the LED
// through its eight colours every 2.8 s.

class Blinky
{
private:
    uint26 _count;

public:
    //| One cycle's LED: {red, green, blue}.
    [[latency(0)]] uint3 step()
    {
        uint26 count;
        atomic
        {
            count = _count;
            _count = count + 1;
        }
        return cast<uint3>(count >> 23);
    }
}

export Blinky;
