/*|
I2C targets, over the callback a design declares for a board's `i2c`
resource:

    (uint2 op, uint8 data) -> uint9 _i2c;

`op` is one of the constants here: START (a repeated start if the bus is
already the design's), STOP, WRITE `data` (bit 8 of the result is set if the
target did not acknowledge), READ a byte (`data` bit 0: acknowledge it, as
for every byte but the last; the result is the byte). The generated core wires
the callback to the I2C engine (platform/rtl/periph/i2c_master.sv), which
runs the bus at the resource's `hz` and waits for a target stretching the
clock.

`bus` takes the callback when declared, and adds the register reads and
writes most sensors want:

    (uint2 op, uint8 data) -> uint9 _i2c;
    periph.i2c.bus _sensor = {._i2c = _i2c};

    uint9 config = _sensor.read_reg(0x48, 1);
    uint17 temperature = _sensor.read_regs(0x48, 0, 2);

One transaction at a time: two threads calling into the same object would
interleave them on the one bus.
*/
module periph.i2c
    { START
    , STOP
    , WRITE
    , READ
    , NACK
    , bus
    }

const uint2 START = 0;
const uint2 STOP = 1;
const uint2 WRITE = 2;
const uint2 READ = 3;

//| Bit 8 of a result: the target did not acknowledge.
const uint9 NACK = 0x100;

//| A bus with targets addressed by seven bits.
//
// Each method calls the bus from one place, in a loop over the steps of its
// transaction: a callback's call site costs a context saver and its FIFOs,
// so a transaction written out call by call is several times larger
// (board-tooling.md §5).
class bus
{
private:
    //| The engine: one operation, its result.
    (uint2 op, uint8 data) -> uint9 _i2c;

    inline uint8 address_byte(uint7 address, bool read)
    {
        return cast<uint8>(address << 1) | (read ? 1 : 0);
    }

public:
    //| Whether a target acknowledges `address`.
    bool probe(uint7 address)
    {
        uint9 nack = 0;
        uint2 step = 0;
        do
        {
            uint2 op = step == 0 ? START : (step == 1 ? WRITE : STOP);
            nack = nack | (_i2c(op, address_byte(address, false)) & NACK);
            step = step + 1;
        } while (step != 3);
        return nack == 0;
    }

    //| Writes `value` to the register `reg` of the target at `address`;
    // true if the target acknowledged every byte.
    bool write_reg(uint7 address, uint8 reg, uint8 value)
    {
        // START, the address, the register, the value, STOP.
        uint9 nack = 0;
        uint3 step = 0;
        do
        {
            uint2 op = step == 0 ? START : (step == 4 ? STOP : WRITE);
            uint8 data = step == 1 ? address_byte(address, false) : (step == 2 ? reg : value);
            nack = nack | (_i2c(op, data) & NACK);
            step = step + 1;
        } while (step != 5);
        return nack == 0;
    }

    //| Reads `n` registers from `reg` on, 1 or 2, the first as the high
    // byte (a 16-bit sensor reading); NACK's bit, bit 8, moved to bit 16.
    uint17 read_regs(uint7 address, uint8 reg, uint2 n)
    {
        // START, the address to write, the register; START again, the
        // address to read, the bytes (each acknowledged but the last); STOP.
        uint1 nack = 0;
        uint16 value = 0;
        uint3 step = 0;
        do
        {
            bool reading = step >= 5 && step < 5 + n;
            uint2 op = (step == 0 || step == 3) ? START : (step == 5 + n ? STOP : (reading ? READ : WRITE));
            uint8 data = reading
                ? (step + 1 < 5 + n ? 1 : 0)
                : (step == 1 ? address_byte(address, false) : (step == 2 ? reg : address_byte(address, true)));
            uint9 r = _i2c(op, data);
            if (reading)
            {
                value = cast<uint16>(value << 8) | cast<uint8>(r);
            }
            else if (op == WRITE)
            {
                nack = nack | cast<uint1>(r >> 8);
            }
            step = step + 1;
        } while (step != 6 + n);
        return concat(nack, value);
    }

    //| Reads the register `reg` of the target at `address`: the byte, with
    // NACK set if the target did not acknowledge.
    uint9 read_reg(uint7 address, uint8 reg)
    {
        uint17 r = read_regs(address, reg, 1);
        return concat(cast<uint1>(r >> 16), cast<uint8>(r));
    }
}
