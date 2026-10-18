/*|
SPI devices, over the callback a design declares for a board's `spi`
resource:

    (uint8 mosi, bool hold) -> uint8 _spi;

Each call sends one byte and returns the byte read meanwhile; chip select
stays low after the byte while `hold`, so a transaction of n bytes is n
calls, the last with `hold` false. The generated core wires the callback to
the SPI engine (platform/rtl/periph/spi_master.sv), which runs the bus at the
resource's `hz` and `mode`.

A class here takes the callback when declared, and speaks a device's
commands through it:

    (uint8 mosi, bool hold) -> uint8 _spi;
    periph.spi.flash _flash = {._transfer = _spi};

    uint24 id = _flash.jedec_id();

One transaction at a time: two threads calling into the same object would
interleave their bytes on the one bus.
*/
module periph.spi
    { flash
    }

//| A SPI NOR flash, with the commands every 25-series part shares (Winbond
// W25Q, Macronix MX25, ISSI IS25…).
//
// Each method calls the bus from one place, in a loop over its bytes: a
// callback's call site costs a context saver and its FIFOs, so a sequence
// written out call by call is several times larger (board-tooling.md §5).
class flash
{
private:
    //| The bus: one byte each way; chip select held while `hold`.
    (uint8 mosi, bool hold) -> uint8 _transfer;

public:
    //| The JEDEC ID (command 0x9F): manufacturer, memory type and capacity,
    // most significant first. A W25Q32 answers 0xEF4016.
    uint24 jedec_id()
    {
        uint24 id = 0;
        uint3 i = 0;
        do
        {
            uint8 b = _transfer(i == 0 ? 0x9F : 0, i != 3);
            id = cast<uint24>(id << 8) | b;
            i = i + 1;
        } while (i != 4);
        return id;
    }

    //| Status register 1 (command 0x05); bit 0 is set while the part is
    // busy writing or erasing.
    uint8 status()
    {
        uint8 status;
        bool first = true;
        do
        {
            status = _transfer(first ? 0x05 : 0, first);
            first = !first;
        } while (!first);
        return status;
    }

    //| Starts a read at `addr` (command 0x03); the bytes then come from
    // `read_next`, as many as wanted, the address counting up.
    void begin_read(uint24 addr)
    {
        uint32 out = concat(cast<uint8>(0x03), addr);
        uint3 i = 0;
        do
        {
            _transfer(cast<uint8>(out >> 24), true);
            out = cast<uint32>(out << 8);
            i = i + 1;
        } while (i != 4);
    }

    //| The next byte of the read `begin_read` started; `last` ends it.
    uint8 read_next(bool last)
    {
        return _transfer(0, !last);
    }

    //| `n` bytes from `addr`, 1 to 4, the first in the lowest byte.
    uint32 read(uint24 addr, uint3 n)
    {
        uint32 out = concat(cast<uint8>(0x03), addr);
        uint32 value = 0;
        uint4 i = 0;    // to 4 + n, which is 8 for four bytes
        do
        {
            bool command = i < 4;
            uint8 b = _transfer(command ? cast<uint8>(out >> 24) : 0, i != 3 + n);
            out = cast<uint32>(out << 8);
            if (!command)
            {
                value = (value >> 8) | (cast<uint32>(b) << 24);
            }
            i = i + 1;
        } while (i != 4 + n);
        return value >> (8 * (4 - n));
    }
}
