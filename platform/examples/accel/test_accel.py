"""Tests of the accel example that run in three places, unchanged:

    kg board test                                  # cocotb, on the generated module
    kg board serve -- python3 test_accel.py        # the simulated board, over its serial port
    python3 test_accel.py --port /dev/ttyUSB0      # the board

accel.py and kgbridge.py are generated into target/board/py by any `kg board`
build; `kg board test` and `serve` put them on the path.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "board", "py"))

import kgbridge  # noqa: E402
from accel import Accel  # noqa: E402


@kgbridge.test(Accel)
async def arithmetic(api):
    assert await api.add(2, 3) == 5
    assert await api.add(0xFFFF_FFFF, 2) == 1  # wraps at 32 bits
    assert await api.mul(1234, 567) == 1234 * 567


@kgbridge.test(Accel)
async def running_total(api):
    first = await api.accumulate(10)
    assert await api.accumulate(5) == first + 5


@kgbridge.test(Accel)
async def callbacks_into_python(api):
    asked = []

    def square(x):
        asked.append(x)
        return x * x

    api.on("_square", square)
    assert await api.sum_of_squares(4) == 1 + 4 + 9 + 16
    assert asked == [1, 2, 3, 4]
    assert await api.sum_of_squares(0) == 0


@kgbridge.test(Accel)
async def structs_and_signs(api):
    point = await api.negate({"x": 300, "y": -7})
    assert (point.x, point.y) == (-300, 7)


if __name__ == "__main__":
    kgbridge.main()
