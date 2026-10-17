// The Mimas A7 pilot: a free-running counter shown on the eight LEDs, each
// LED inverted by the DIP switch beneath it. The board shell
// (../../rtl/mimas_a7_top.sv) calls `step` whenever the function is ready,
// so at 100 MHz the top byte of the counter moves the slowest LED at ~3 Hz.

class Blinky
{
private:
    uint32 _count = 0;

public:
    uint8 step(uint8 switches)
    {
        uint32 count;
        atomic
        {
            count = _count;
            _count = count + 1;
        }
        return cast<uint8>(count >> 24) ^ switches;
    }
}

export Blinky;
