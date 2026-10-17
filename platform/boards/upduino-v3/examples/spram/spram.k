// UPduino v3 SPRAM example: a memory test that never stops, sweeping two of
// the UP5K's SPRAM blocks (32K x 16) through `lattice.spram`.
//
// Each pass writes every word, then reads every word back and compares it
// with what was written; the pattern changes with each pass, so a stuck bit
// or a crossed address line shows within two passes. The LED is green while
// every read has matched and red, for good, after the first that did not;
// blue toggles every 128 passes (0.35 s at 24 MHz).
//
// Written for the iCE40's size (platform-layer.md §8):
// - `[[latency(2)]]`, the least the scheduler accepts here: the SPRAM read
//   takes one cycle, the compare and the latch the next. Called every cycle,
//   so the method has no ready, output valid or FIFO.
// - The SPRAM has one port, and `access` has one call site: the sweep's
//   write and read passes share it, `write` choosing which.
// - No initial values: the flip-flops leave configuration at zero, which is
//   address 0 of a write pass with no failure seen.

import lattice.spram

class SpramTest
{
private:
    spram<32768, 16> _ram;
    uint15 _addr;
    bool _reading;
    uint8 _pass;
    bool _failed;

    inline uint16 pattern(uint15 addr, uint8 pass)
    {
        return ((cast<uint16>(pass) << 8) | pass) ^ addr;
    }

public:
    //| One word of the sweep per call; the LED: {red, green, blue}.
    [[latency(2)]] uint3 step()
    {
        uint15 addr;
        bool reading;
        uint8 pass;
        atomic
        {
            addr = _addr;
            reading = _reading;
            pass = _pass;
            _addr = addr + 1;
            if (addr == 0x7FFF)
            {
                if (reading)
                {
                    _pass = pass + 1;
                }
                _reading = !reading;
            }
        }

        uint16 expected = pattern(addr, pass);
        uint16 word = _ram.access(addr, !reading, expected, 0xF);

        bool failed;
        atomic
        {
            failed = _failed || (reading && word != expected);
            _failed = failed;
        }

        return (failed ? 4 : 2) | (pass >> 7);
    }
}

export SpramTest;
