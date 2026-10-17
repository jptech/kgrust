#!/usr/bin/env python3
"""Checks a virtual board's front panel from outside, as a browser would:
`kg board serve -- python3 check_screen.py [--frames N] [--expect RGB@X,Y ...]
[--column RGB@X ...]`.

Waits for N whole frames, prints the frame rate and the simulated clock
rate the panel reports, and checks the given pixels of the last frame
(12-bit colours in hex), or that a colour is somewhere in a column.
Standard library only.
"""

import argparse
import json
import os
import sys
import time
import urllib.request


def get(url):
    with urllib.request.urlopen(url, timeout=10) as r:
        return r.read()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--frames", type=int, default=3)
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--expect", action="append", default=[], help="RGB@X,Y: the 12-bit colour at a pixel")
    parser.add_argument("--column", action="append", default=[], help="RGB@X: the colour somewhere in column X")
    args = parser.parse_args()
    panel = os.environ.get("BOARD_PANEL")
    if not panel:
        sys.exit("check_screen: no BOARD_PANEL; run under `kg board serve` on a board with a front panel")
    start = time.monotonic()
    first = None
    while True:
        state = json.loads(get(panel + "state"))
        if first is None and state["frames"] >= 1:
            first = (time.monotonic(), state["frames"])
        if state["frames"] >= args.frames + 1:
            break
        if time.monotonic() - start > args.timeout:
            sys.exit(f"FAIL: {state['frames']} frames in {args.timeout:g} s")
        time.sleep(0.05)
    seconds = time.monotonic() - first[0]
    fps = (state["frames"] - first[1]) / seconds if seconds > 0 else 0
    frame = get(panel + "frame")
    pixels = [int.from_bytes(frame[i:i + 2], "little") for i in range(0, len(frame), 2)]
    bad = 0
    for spec in args.expect:
        colour, at = spec.split("@")
        x, y = (int(v) for v in at.split(","))
        got = pixels[y * 640 + x]
        if got != int(colour, 16):
            bad += 1
            print(f"pixel ({x}, {y}): {got:03x}, want {colour}")
    for spec in args.column:
        colour, x = spec.split("@")
        if int(colour, 16) not in pixels[int(x)::640]:
            bad += 1
            print(f"column {x}: no {colour}")
    print(f"{'PASS' if bad == 0 else 'FAIL'}: {state['frames']} frames, {fps:.1f} frames/s, "
          f"simulating {state['mhz']:.2f} MHz (real time: {state['mhz'] / 25 * 100:.0f}%), leds {state['leds']:08b}")
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
