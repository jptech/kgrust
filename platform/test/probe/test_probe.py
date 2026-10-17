"""Every protocol, through both kgbridge transports: `kg board test`
(cocotb) and `kg board serve -- python3 test_probe.py` (the bridge)."""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "board", "py"))

import kgbridge  # noqa: E402
from probe import Probe  # noqa: E402


@kgbridge.test(Probe)
async def handshake_methods_and_callbacks(api):
    notes = []
    api.on("_note", notes.append)
    api.on("_ask", lambda x, flag: (x * 2 if flag else x) & 0xFF)
    before = await api.count()
    await api.plain(7, -3)
    assert await api.count() == (before + 4) & 0xFFFF_FFFF
    assert notes == [7]
    assert await api.handshake(5) == 10
    assert await api.handshake(120) == (240 + 120)


@kgbridge.test(Probe)
async def no_backpressure(api):
    pulses = []
    api.on("_pulse", pulses.append)
    await api.nbp(True)
    assert pulses == [1]  # the callback reaches the host before the reply
    await api.nbp(False)
    assert await api.nbp_result(41) == 42
    assert await api.nbp_result(255) == 0
    assert pulses == [1, 0]


@kgbridge.test(Probe)
async def fixed_latency(api):
    assert await api.lat1(5) == 261
    assert await api.lat0(0x0F) == 0x55
    assert [await api.lat0(v) for v in (0, 0xFF)] == [0x5A, 0xA5]
    # The bytes SLIP escapes, 0xC0 and 0xDB, each way.
    assert [await api.lat0(v) for v in (0x9A, 0x81, 0xC0, 0xDB)] == [0xC0, 0xDB, 0x9A, 0x81]


@kgbridge.test(Probe)
async def structs_and_signs(api):
    p = await api.pair({"a": 255, "b": -8})
    assert (p.a, p.b) == (0, -8)  # -(-8) wraps in int4
    p = await api.pair({"a": 9, "b": 3})
    assert (p.a, p.b) == (10, -3)
    assert await api.negate(-128) == -128
    assert await api.negate(5) == -5


@kgbridge.test(Probe)
async def widths_are_checked_before_sending(api):
    for bad in (lambda: api.negate(128), lambda: api.lat1(8), lambda: api.add_missing()):
        try:
            bad()
        except (ValueError, AttributeError):
            continue
        raise AssertionError("an out-of-range argument was sent")


if __name__ == "__main__":
    kgbridge.main()
