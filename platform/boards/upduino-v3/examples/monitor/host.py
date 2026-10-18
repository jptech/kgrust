#!/usr/bin/env python3
"""Talk to the UPduino monitor (monitor.k) over its serial port.

    host.py [--port PORT] read ADDR
    host.py [--port PORT] write ADDR DATA
    host.py [--port PORT] dump ADDR COUNT
    host.py [--port PORT] test [--count N] [--seed S] [--stream]
    host.py [--port PORT] shell

PORT is the board's FT232H port (/dev/tty.usbserial-* on macOS,
/dev/ttyUSB* on Linux) or, for the simulated design, the pseudo-terminal
`kg board serve` prints in this directory, which it also exports as
BOARD_PTY, the default here. Addresses and data are hex.

The port is opened raw at 115200 baud with DTR and RTS deasserted, and left
that way: on the UPduino, DTR# is the SPI flash's chip select, and RTS# shares
a net with the flash's data output (constraints/upduino-v3.pcf). A terminal
program that asserts them on open selects the flash; with picocom, use
--lower-dtr --lower-rts.

Standard library only, POSIX only (termios).
"""

import argparse
import fcntl
import os
import random
import select
import struct
import sys
import termios
import time
import tty

WORDS = 32768


class MonitorError(Exception):
    pass


class Monitor:
    def __init__(self, port, baud=115200, timeout=2.0):
        self.timeout = timeout
        self.fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        tty.setraw(self.fd)
        attrs = termios.tcgetattr(self.fd)
        speed = getattr(termios, f"B{baud}")
        attrs[4] = attrs[5] = speed
        # CLOCAL: no modem lines needed; no HUPCL, so closing leaves DTR alone.
        attrs[2] = (attrs[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL & ~termios.CRTSCTS
        termios.tcsetattr(self.fd, termios.TCSANOW, attrs)
        self._lower_modem_lines()
        termios.tcflush(self.fd, termios.TCIFLUSH)
        self.pending = b""

    def _lower_modem_lines(self):
        # DTR# and RTS# high, so the flash stays deselected. A pseudo-terminal
        # has no modem lines, and refuses: nothing to lower there.
        try:
            fcntl.ioctl(self.fd, termios.TIOCMBIC, struct.pack("I", termios.TIOCM_DTR | termios.TIOCM_RTS))
        except OSError:
            pass

    def close(self):
        os.close(self.fd)

    def send(self, line):
        """Send a command line, taking in any reply bytes meanwhile."""
        data = line.encode("ascii") + b"\r"
        while data:
            readable, writable, _ = select.select([self.fd], [self.fd], [], self.timeout)
            if readable:
                self._take()
            if writable:
                try:
                    data = data[os.write(self.fd, data):]
                except BlockingIOError:
                    pass

    def _take(self):
        # Replies are read as they come even while sending: left unread they
        # fill the port's input buffer, and with no flow control the rest
        # are lost.
        try:
            self.pending += os.read(self.fd, 4096)
        except (BlockingIOError, OSError):
            pass

    def reply(self, timeout=None):
        """The next reply line, without its CR LF."""
        deadline = time.monotonic() + (timeout or self.timeout)
        while b"\r\n" not in self.pending:
            left = deadline - time.monotonic()
            if left <= 0:
                raise MonitorError(f"no reply within {timeout or self.timeout:g} s")
            if select.select([self.fd], [], [], left)[0]:
                self._take()
        line, self.pending = self.pending.split(b"\r\n", 1)
        return line.decode("ascii", errors="replace")

    def command(self, line):
        self.send(line)
        return self.reply()

    @staticmethod
    def _word(reply, what):
        if len(reply) != 4:
            raise MonitorError(f"{what}: the monitor answered {reply!r}")
        return int(reply, 16)

    def read(self, addr):
        return self._word(self.command(f"r {addr:04x}"), f"read {addr:04x}")

    def write(self, addr, data):
        return self._word(self.command(f"w {addr:04x} {data:04x}"), f"write {addr:04x}")


def hex_arg(text):
    return int(text, 16)


def run_test(mon, count, seed, stream):
    rng = random.Random(seed)
    addrs = rng.sample(range(WORDS), count)
    words = {a: rng.randrange(1 << 16) for a in addrs}
    start = time.monotonic()
    if stream:
        # Every command sent before any reply is read: the monitor must keep
        # up at the line's full rate (its replies are never longer than its
        # commands). Up to ~19 bytes a command, at 11,520 bytes/s on the board.
        for a in addrs:
            mon.send(f"w {a:04x} {words[a]:04x}")
        for a in addrs:
            mon.send(f"r {a:04x}")
        wait = mon.timeout + count * 0.05
        echoes = [mon.reply(wait) for _ in addrs]
        reads = [mon.reply(wait) for _ in addrs]
    else:
        echoes = [f"{mon.write(a, words[a]):04x}" for a in addrs]
        reads = [f"{mon.read(a):04x}" for a in addrs]
    bad = 0
    for a, echo, got in zip(addrs, echoes, reads):
        want = f"{words[a]:04x}"
        if echo != want or got != want:
            bad += 1
            if bad <= 10:
                print(f"  {a:04x}: wrote {want}, echoed {echo}, read {got}")
    seconds = time.monotonic() - start
    verdict = "PASS" if bad == 0 else "FAIL"
    print(f"{verdict}: {count} words written and read back, {bad} wrong, {seconds:.2f} s"
          f"{' (streamed)' if stream else ''}")
    return bad == 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--port", default=os.environ.get("BOARD_PTY"),
                        help="serial port (default: $BOARD_PTY)")
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("--timeout", type=float, default=2.0, help="seconds to wait for a reply")
    sub = parser.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("read")
    p.add_argument("addr", type=hex_arg)
    p = sub.add_parser("write")
    p.add_argument("addr", type=hex_arg)
    p.add_argument("data", type=hex_arg)
    p = sub.add_parser("dump")
    p.add_argument("addr", type=hex_arg)
    p.add_argument("count", type=int)
    p = sub.add_parser("test")
    p.add_argument("--count", type=int, default=64)
    p.add_argument("--seed", type=int, default=1)
    p.add_argument("--stream", action="store_true", help="send every command before reading a reply")
    sub.add_parser("shell")
    args = parser.parse_args()
    if not args.port:
        parser.error("no --port, and BOARD_PTY is not set")

    mon = Monitor(args.port, args.baud, args.timeout)
    try:
        if args.cmd == "read":
            print(f"{mon.read(args.addr):04x}")
        elif args.cmd == "write":
            print(f"{mon.write(args.addr, args.data):04x}")
        elif args.cmd == "dump":
            for row in range(args.addr, args.addr + args.count, 8):
                words = [mon.read(a % WORDS) for a in range(row, min(row + 8, args.addr + args.count))]
                print(f"{row:04x}: " + " ".join(f"{w:04x}" for w in words))
        elif args.cmd == "test":
            return 0 if run_test(mon, args.count, args.seed, args.stream) else 1
        elif args.cmd == "shell":
            print("r AAAA | w AAAA DDDD; end with EOF (^D)", file=sys.stderr)
            for line in sys.stdin:
                line = line.strip()
                if line:
                    print(mon.command(line))
    except MonitorError as e:
        print(f"host.py: {e}", file=sys.stderr)
        return 1
    finally:
        mon.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
