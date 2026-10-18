#!/usr/bin/env python3
"""Plays pong through the front panel, as a person at the browser would:
`kg board serve -- python3 check_panel.py`.

Checks the panel shows what pong uses and nothing else, holds the left
player's buttons and watches the left paddle move on the screen, checks the
picture's ball, net and background, pauses and resumes the clock, and saves
the last frame to the project's target/board/pong.png. The widgets are found
by kind, so it runs on any board with a screen, buttons and LEDs, whatever
it names them.
"""

import os
import sys
import time

# The project `kg board serve` runs in: its build has kgbridge.py.
project = os.getcwd()
sys.path.insert(0, os.path.join(project, "target", "board", "py"))

import kgbridge  # noqa: E402

WHITE, NET, BACKGROUND = 0xFFF, 0x666, 0x013
LEFT_COLUMN = 20  # the left paddle's, 16 to 23


def paddle_top(frame):
    """The top of the longest white run in the left paddle's column: the
    paddle, not the ball passing through it."""
    best, start, run = None, None, 0
    for y in range(frame.height + 1):
        white = y < frame.height and frame.pixel(LEFT_COLUMN, y) == WHITE
        if white:
            start = y if run == 0 else start
            run += 1
        elif run:
            if best is None or run > best[1]:
                best = (start, run)
            run = 0
    return best[0] if best else None


def hold(panel, buttons, bit, frames):
    panel.set(buttons, 1 << bit)
    frame = panel.frame()
    for _ in range(frames):
        frame = panel.frame(after=frame.number)
    panel.set(buttons, 0)
    return frame


def main():
    panel = kgbridge.Panel(timeout=120)
    buttons = panel.find("button")
    # The score is on the widest row of LEDs.
    leds = max((w for w in panel.layout()["widgets"] if w["kind"] == "led"), key=lambda w: w["width"])["name"]
    failures = []

    # Only what pong uses: its screen, its buttons and its LEDs, whatever
    # else the board has.
    kinds = sorted(w["kind"] for w in panel.layout()["widgets"])
    if kinds != ["button", "led", "vga"]:
        failures.append(f"the panel shows {kinds}, want pong's button, led and vga alone")

    frame = panel.frame()
    if frame.pixel(319, 0) != NET or frame.pixel(100, 100) != BACKGROUND:
        failures.append(f"the picture: net {frame.pixel(319, 0):03x}, background {frame.pixel(100, 100):03x}")
    # The ball: white away from the paddles' columns.
    ball = sum(1 for y in range(frame.height) for x in range(24, 616) if frame.pixel(x, y) == WHITE)
    if ball == 0:
        failures.append("no ball on the screen")

    # Down for 20 frames (6 pixels each, or to the bottom), then up for 20.
    down = paddle_top(hold(panel, buttons, 1, 20))
    up = paddle_top(hold(panel, buttons, 0, 20))
    if down is None or up is None:
        failures.append("no paddle in the left column")
    elif not (down >= 100 and up <= down - 100):
        failures.append(f"the paddle did not follow the buttons: top at {down} after down, {up} after up")

    # Paused, nothing moves (the clock stops within 1,024 cycles); resumed,
    # the frames come again.
    panel.pause()
    time.sleep(0.2)
    held = panel.state()
    time.sleep(0.5)
    still = panel.state()
    panel.resume()
    if not still["paused"] or (still["frames"], still["cycles"]) != (held["frames"], held["cycles"]):
        failures.append(f"paused, the clock ran on: frame {held['frames']} to {still['frames']}, {still['cycles'] - held['cycles']} cycles")

    last = panel.frame()
    last.save_png(os.path.join(project, "target", "board", "pong.png"))
    score = panel.value(leds)
    if score == 0:
        failures.append("no score on the LEDs: the opening frame is a miss, so one side has a point")
    state = panel.state()
    for f in failures:
        print("FAIL", f)
    print(f"{'PASS' if not failures else 'FAIL'}: the paddle went to {down} and back to {up}; score {score >> 4}:{score & 15}; "
          f"{state['fps']:.1f} frames/s at {state['mhz']:.2f} MHz")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
