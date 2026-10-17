#!/usr/bin/env python3
"""The monitor through the front panel's console, as a person at the
browser would type it: `kg board serve -- python3 check_panel.py`.

Types a write and a read into the console, reads the replies back from it,
checks the RGB LED shows green, then red after a bad command, and that the
line both ways is in order: each command, then its reply.
"""

import os
import sys
import time

sys.path.insert(0, os.path.join(os.getcwd(), "target", "board", "py"))

import kgbridge  # noqa: E402


def reply(panel, before, timeout=60):
    """The console's next CR LF line after `before` bytes."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        text = panel.console()[before:]
        if b"\r\n" in text:
            return text.split(b"\r\n", 1)[0].decode()
        time.sleep(0.05)
    raise kgbridge.BridgeError("no reply in the console")


def main():
    panel = kgbridge.Panel()
    led = panel.find("led")
    failures = []
    for line, want in [("w 0123 beef", "beef"), ("r 0123", "beef"), ("x", "?")]:
        before = len(panel.console())
        panel.type(line + "\r")
        got = reply(panel, before)
        if got != want:
            failures.append(f"{line!r}: {got!r}, want {want!r}")
        time.sleep(0.2)
        colour = panel.value(led)
        want_colour = 0b100 if want == "?" else 0b010
        if colour & 0b110 != want_colour:
            failures.append(f"after {line!r}: LED {colour:03b}, want red/green {want_colour >> 1:02b}")
    # Each command crossed before its reply, as the page's console shows them.
    ways = [way for way, _ in panel.transcript()]
    if ways != ["sent", "received"] * 3:
        failures.append(f"the line both ways: {ways}, want each command then its reply")
    for f in failures:
        print("FAIL", f)
    print(f"{'PASS' if not failures else 'FAIL'}: the monitor through the console, LED {panel.value(led):03b}")
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
