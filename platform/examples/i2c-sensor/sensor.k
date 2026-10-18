// An I2C sensor read and written from Python: the peripheral library's `bus`
// (platform/library/periph/i2c.k) over the board's `i2c` resource.
//
// The design declares the callback for the bus, named for the resource, so
// the generated core wires it to the I2C engine (platform/rtl/periph); the
// library class takes the callback and runs the register transactions. The
// public methods bind to nothing on the board, so the host bridge serves
// them.
//
// Under `kg board serve` the board's bus has a register-file sensor at 0x48
// (platform/sim/models); under cocotb (`kg board test`) the callback comes
// to Python, where kgbridge.I2cTarget answers it the same way.
// test_sensor.py runs in both, and on a board with a target at 0x48.

import periph.i2c

class Sensor
{
private:
    //| The board's I2C bus.
    (uint2 op, uint8 data) -> uint9 _i2c;

    bus _bus = {._i2c = _i2c};

public:
    //| Whether a target answers `address`.
    bool probe(uint7 address)
    {
        return _bus.probe(address);
    }

    //| A target's register; bit 8 set if it did not acknowledge.
    uint9 read(uint7 address, uint8 reg)
    {
        return _bus.read_reg(address, reg);
    }

    //| Writes a target's register; true if it acknowledged.
    bool write(uint7 address, uint8 reg, uint8 value)
    {
        return _bus.write_reg(address, reg, value);
    }
}

export Sensor;
