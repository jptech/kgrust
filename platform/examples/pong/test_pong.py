"""Pong's game, cycle by cycle under cocotb: `kg board test`.

The picture and the game are functions the board's screen calls
(`vga_pixel`, `vga_frame`), wired to the `vga` resource rather than the
bridge, so this runs under cocotb alone, calling them directly. The screen
itself is checked through the front panel by check_panel.py.
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "target", "board", "py"))

import kgbridge  # noqa: E402
from pong import Pong  # noqa: E402

WHITE, NET, BACKGROUND = 0xFFF, 0x666, 0x013


@kgbridge.test(Pong, only="cocotb")
async def serve_score_and_paddles(api):
    held = {"buttons": 0}
    lit = []
    api.on("_buttons", lambda: held["buttons"])
    api.on("_leds", lit.append)

    # The picture before any frame: net, background, the paddles at the top.
    assert await api.vga_pixel(319, 0) == NET
    assert await api.vga_pixel(100, 100) == BACKGROUND
    assert await api.vga_pixel(20, 10) == WHITE

    # The ball starts in the corner heading left: a miss, a point to the
    # right player, and a serve from the middle.
    await api.vga_frame()
    assert lit[-1] == 0x01, lit
    assert await api.vga_pixel(324, 244) == WHITE

    # Then it heads right, four pixels a frame.
    await api.vga_frame()
    assert await api.vga_pixel(328, 248) == WHITE
    assert await api.vga_pixel(321, 241) != WHITE

    # The left player's down button moves the left paddle six a frame.
    held["buttons"] = 2
    for _ in range(4):
        await api.vga_frame()
    assert await api.vga_pixel(20, 10) == BACKGROUND
    assert await api.vga_pixel(20, 30) == WHITE


if __name__ == "__main__":
    kgbridge.main()
