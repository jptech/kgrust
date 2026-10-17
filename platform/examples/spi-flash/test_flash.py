"""The spi-flash example's tests, in three places, unchanged:

    kg board test                                  # cocotb: kgbridge.SpiFlash answers the bus
    kg board serve -- python3 test_flash.py        # the simulated board: the SPI engine and a flash model
    python3 test_flash.py --port /dev/ttyUSB0      # a board with a W25Q32-like flash on its `spi`

The models agree byte for byte (kgbridge.SpiFlash and
platform/sim/models/spi_flash_model.sv); on a board, the bytes are its flash's.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "board", "py"))

import kgbridge  # noqa: E402
from flash import Flash  # noqa: E402


@kgbridge.test(Flash)
async def jedec_id_and_reads(api):
    # Under cocotb this answers the bus; over the bridge the board's own
    # engine does, and the handler is never asked.
    api.on("_spi", kgbridge.SpiFlash())
    assert await api.id() == 0xEF4016
    assert await api.read4(0x10) == int.from_bytes(bytes(a ^ 0xA5 for a in range(0x10, 0x14)), "little")
    assert await api.read4(0xFE) == int.from_bytes(bytes(a ^ 0xA5 for a in (0xFE, 0xFF, 0x00, 0x01)), "little")


if __name__ == "__main__":
    kgbridge.main()
