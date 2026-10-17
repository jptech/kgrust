#!/usr/bin/env python3
"""Checks a simulated board's screen from outside, through its front panel:
`kg board serve -- python3 check_screen.py [--frames N] [--expect RGB@X,Y ...]
[--column RGB@X ...] [--png FILE]`.

Waits for N whole frames, prints the frame rate and the simulated clock
rate, and checks the given pixels of the last frame (12-bit colours in hex),
or that a colour is somewhere in a column. kgbridge's `Panel` does the work;
any board with a `vga` resource has a screen to check.
"""

import argparse
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "python"))

import kgbridge  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--frames", type=int, default=3)
    parser.add_argument("--expect", action="append", default=[], help="RGB@X,Y: the 12-bit colour at a pixel")
    parser.add_argument("--column", action="append", default=[], help="RGB@X: the colour somewhere in column X")
    parser.add_argument("--png", help="save the last frame here")
    args = parser.parse_args()
    panel = kgbridge.Panel(timeout=120)
    first = panel.frame()
    start = time.monotonic()
    frame = first
    while frame.number < first.number + args.frames:
        frame = panel.frame(after=frame.number)
    seconds = time.monotonic() - start
    fps = (frame.number - first.number) / seconds if seconds > 0 else 0
    bad = 0
    for spec in args.expect:
        colour, at = spec.split("@")
        x, y = (int(v) for v in at.split(","))
        if frame.pixel(x, y) != int(colour, 16):
            bad += 1
            print(f"pixel ({x}, {y}): {frame.pixel(x, y):03x}, want {colour}")
    for spec in args.column:
        colour, x = spec.split("@")
        if not frame.rows(int(x), int(colour, 16)):
            bad += 1
            print(f"column {x}: no {colour}")
    if args.png:
        frame.save_png(args.png)
    mhz = panel.state()["mhz"]
    clock = panel.layout()["mhz"]
    print(f"{'PASS' if bad == 0 else 'FAIL'}: {frame.number} frames, {fps:.1f} frames/s, "
          f"simulating {mhz:.2f} MHz of {clock:g} ({mhz / clock * 100:.0f}% of real time)")
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
