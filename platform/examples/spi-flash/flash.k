// A SPI flash read from Python: the peripheral library's `flash`
// (platform/library/periph/spi.k) over the board's `spi` resource.
//
// The design declares the callback for the bus, named for the resource, so
// the generated core wires it to the SPI engine (platform/rtl/periph); the
// library class takes the callback and speaks the flash's commands. The
// public methods bind to nothing on the board, so the host bridge serves
// them.
//
// Under `kg board serve` the board's bus has a flash model on it
// (platform/sim/models); under cocotb (`kg board test`) the callback comes
// to Python, where kgbridge.SpiFlash answers it the same way.
// test_flash.py runs in both, and on a board with a W25Q-style flash.

import periph.spi

class Flash
{
private:
    //| The board's SPI bus.
    (uint8 mosi, bool hold) -> uint8 _spi;

    flash _flash = {._transfer = _spi};

public:
    //| The flash's JEDEC ID: manufacturer, type, capacity.
    uint24 id()
    {
        return _flash.jedec_id();
    }

    //| Four bytes of the flash from `addr`, the first lowest.
    uint32 read4(uint24 addr)
    {
        return _flash.read(addr, 4);
    }
}

export Flash;
