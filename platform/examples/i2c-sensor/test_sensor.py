"""The i2c-sensor example's tests, in three places, unchanged:

    kg board test                                  # cocotb: kgbridge.I2cTarget answers the bus
    kg board serve -- python3 test_sensor.py       # the simulated board: the I2C engine and a sensor model
    python3 test_sensor.py --port /dev/ttyUSB0     # a board with a register-file target at 0x48

The models agree (kgbridge.I2cTarget and
platform/sim/models/i2c_target_model.sv: sixteen registers reset to 0x10 + i).
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "board", "py"))

import kgbridge  # noqa: E402
from sensor import Sensor  # noqa: E402


@kgbridge.test(Sensor)
async def registers(api):
    # Under cocotb this answers the bus; over the bridge the board's own
    # engine does, and the handler is never asked.
    api.on("_i2c", kgbridge.I2cTarget(0x48))
    assert await api.probe(0x48)
    assert not await api.probe(0x49)
    assert await api.read(0x48, 3) == 0x13
    assert await api.write(0x48, 2, 0xAB)
    assert await api.read(0x48, 2) == 0xAB
    assert await api.read(0x49, 0) & 0x100  # nobody there


if __name__ == "__main__":
    kgbridge.main()
