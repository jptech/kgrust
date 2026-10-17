#!/usr/bin/env python3
"""kgbridge: a Kanagawa design's exported functions, called from Python.

`kg board` writes, beside this file in a project's target/board/py/, a module
generated from the design's interface (target/board/interface.json): one class
for the exported class, one method per function. That class runs over any of
three transports, with the same API:

- the board itself, through the host bridge the generated core puts on its
  serial port (`Serial`, given the port);
- the design simulated by `kg board serve`, whose serial port is a
  pseudo-terminal (`Serial`, which finds it in BOARD_PTY);
- the generated module under cocotb, its ports driven directly (`Cocotb`).

A test written once runs in all three:

    from accel import Accel
    import kgbridge

    @kgbridge.test(Accel)
    async def adds(api):
        assert await api.add(2, 3) == 5

    if __name__ == "__main__":
        kgbridge.main()

`kg board test` runs it under cocotb; `kg board serve -- python3 test_accel.py`
against the simulated board; `python3 test_accel.py --port /dev/ttyUSB0` on the
board. A callback the design calls out through is answered by a Python
function: `api.on("_square", lambda x: x * x)`.

The bridge protocol (crates/kg-board/src/wrapper.rs, `bridge`): SLIP frames
over the serial port. The host sends `[i][args]` to call method i, `[0x80|j]
[result]` to answer callback j, and `[0xFF]` to ask which build it is talking
to. The board sends `[i][result]` when method i completes, `[0x80|j][args]`
to call callback j, `[0xFF][version][hash]` for hello and `[0xFE][code][op]`
to refuse a frame. Each value takes whole bytes, little-endian.

Standard library only, except the cocotb transport, which needs cocotb 2.
Serial ports: POSIX (termios).
"""

import asyncio
import inspect
import os
import struct
import sys
import time

PROTOCOL = 1

END, ESC, ESC_END, ESC_ESC = 0xC0, 0xDB, 0xDC, 0xDD
HELLO, REFUSED = 0xFF, 0xFE
REFUSALS = {1: "unknown function", 2: "wrong number of argument bytes", 3: "frame lost (sent before the last reply)"}


class BridgeError(Exception):
    """The board refused a frame, or did not answer."""


# --- Values -----------------------------------------------------------------


class Struct(dict):
    """A packed struct's value: its fields by name, also as attributes."""

    def __getattr__(self, name):
        try:
            return self[name]
        except KeyError:
            raise AttributeError(name) from None

    def __repr__(self):
        return "Struct(" + ", ".join(f"{k}={v!r}" for k, v in self.items()) + ")"


def pack(desc, value):
    """A value as the bits the design sees: an int of desc['width'] bits.

    Integers are range-checked against the declared type (signed or not); a
    struct is given as a dict (or anything with its fields as attributes),
    its first field lowest.
    """
    width = desc["width"]
    if desc["fields"]:
        if isinstance(value, int):
            return check(desc, value)
        bits, shift = 0, 0
        for field in desc["fields"]:
            v = value[field["name"]] if isinstance(value, dict) else getattr(value, field["name"])
            bits |= pack(field, v) << shift
            shift += field["width"]
        return bits
    if isinstance(value, bool):
        value = int(value)
    if not isinstance(value, int):
        raise TypeError(f"{desc.get('name') or 'value'}: {desc['type']} wants an int, got {type(value).__name__}")
    return check(desc, value)


def check(desc, value):
    width = desc["width"]
    lo, hi = (-(1 << (width - 1)), (1 << (width - 1)) - 1) if desc["signed"] else (0, (1 << width) - 1)
    if not lo <= value <= hi:
        name = desc.get("name") or "result"
        raise ValueError(f"{name}: {value} does not fit {desc['type']} ({lo}..{hi})")
    return value & ((1 << width) - 1)


def unpack(desc, bits):
    """The Python value of `bits`: an int, signed as declared, or a Struct."""
    bits &= (1 << desc["width"]) - 1
    if desc["fields"]:
        out, shift = Struct(), 0
        for field in desc["fields"]:
            out[field["name"]] = unpack(field, bits >> shift)
            shift += field["width"]
        return out
    if desc["signed"] and bits >> (desc["width"] - 1):
        bits -= 1 << desc["width"]
    return bits


def nbytes(desc):
    return (desc["width"] + 7) // 8


def encode(descs, values):
    return b"".join(pack(d, v).to_bytes(nbytes(d), "little") for d, v in zip(descs, values))


def decode(descs, data):
    out, at = [], 0
    for d in descs:
        n = nbytes(d)
        out.append(unpack(d, int.from_bytes(data[at:at + n], "little")))
        at += n
    return out


# --- The device -------------------------------------------------------------


class Device:
    """Base of a generated class: its functions, called through a transport.

    A generated subclass sets INTERFACE (the description), HASH (what the
    bridge answers hello with), BRIDGED (the functions the bridge serves) and
    CLOCK_MHZ, and adds a method per function.
    """

    INTERFACE = {"module": "", "methods": [], "callbacks": []}
    HASH = 0
    BRIDGED = ()
    CLOCK_MHZ = None

    def __init__(self, transport=None, **serial):
        """`transport`: a `Serial` or `Cocotb`; without one, a `Serial` made
        from the keywords (port, baud, timeout)."""
        self._methods = {f["name"]: f for f in self.INTERFACE["methods"]}
        self._callbacks = {f["name"]: f for f in self.INTERFACE["callbacks"]}
        self._handlers = {}
        self._transport = transport if transport is not None else Serial(**serial)
        self._transport.attach(self)

    def _call(self, name, *args):
        f = self._methods[name]
        if len(args) != len(f["args"]):
            raise TypeError(f"{name}() takes {len(f['args'])} arguments, got {len(args)}")
        for desc, value in zip(f["args"], args):
            pack(desc, value)  # checked here, where the caller can see it
        return self._transport.call(f, args)

    def on(self, name, handler):
        """Answers the callback `name` with `handler(*args)`; usable as a
        decorator. The handler may be async under an async transport."""
        if name not in self._callbacks:
            raise KeyError(f"{self.INTERFACE['module']} has no callback {name}; it has {', '.join(self._callbacks) or 'none'}")
        self._handlers[name] = handler
        return handler

    def _handle(self, f, args):
        handler = self._handlers.get(f["name"])
        if handler is None:
            raise BridgeError(f"the design called {f['name']}{tuple(args)}, and no handler answers it (api.on({f['name']!r}, fn))")
        return handler(*args)

    def close(self):
        self._transport.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()


# --- Serial: the board, or `kg board serve` ---------------------------------


class Serial:
    """The bridge over a serial port: the board's, or the pseudo-terminal of
    `kg board serve` (BOARD_PTY, the default).

    Calls block until the design answers, serving its callbacks meanwhile.
    With `asynchronous=True` every call returns a coroutine instead, for a
    test written for cocotb too.
    """

    def __init__(self, port=None, baud=115200, timeout=None, check=True, asynchronous=False):
        import termios
        import tty

        simulated = port is None
        port = port or os.environ.get("BOARD_PTY")
        if not port:
            raise BridgeError("no serial port: pass port=..., or run under `kg board serve`, which sets BOARD_PTY")
        # A simulation runs some thousand times slower than the board.
        self.timeout = timeout if timeout is not None else (60.0 if simulated else 2.0)
        self.asynchronous = asynchronous
        self.check = check
        self.port = port
        self.fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        tty.setraw(self.fd)
        attrs = termios.tcgetattr(self.fd)
        speed = getattr(termios, f"B{baud}", None)
        if speed is None:
            raise BridgeError(f"no termios speed for {baud} baud")
        attrs[4] = attrs[5] = speed
        # CLOCAL: no modem lines; no HUPCL, so closing leaves DTR alone.
        attrs[2] = (attrs[2] | termios.CLOCAL | termios.CREAD) & ~termios.HUPCL & ~getattr(termios, "CRTSCTS", 0)
        termios.tcsetattr(self.fd, termios.TCSANOW, attrs)
        self._lower_modem_lines()
        termios.tcflush(self.fd, termios.TCIFLUSH)
        self.pending = bytearray()
        self.device = None

    def _lower_modem_lines(self):
        # On the UPduino, DTR# is the SPI flash's chip select and RTS# shares
        # its data output: both stay deasserted. A pseudo-terminal has no
        # modem lines and refuses; nothing to lower there.
        import fcntl
        import termios

        try:
            fcntl.ioctl(self.fd, termios.TIOCMBIC, struct.pack("I", termios.TIOCM_DTR | termios.TIOCM_RTS))
        except OSError:
            pass

    def attach(self, device):
        self.device = device
        if self.check:
            self.hello()

    def close(self):
        os.close(self.fd)

    # Frames.

    def send(self, frame):
        data = bytearray([END])
        for b in frame:
            data += bytes([ESC, ESC_END]) if b == END else bytes([ESC, ESC_ESC]) if b == ESC else bytes([b])
        data.append(END)
        import select

        while data:
            select.select([], [self.fd], [], self.timeout)
            try:
                del data[:os.write(self.fd, data)]
            except BlockingIOError:
                pass

    def receive(self, deadline):
        """The next frame, unescaped."""
        import select

        while True:
            while END in self.pending:
                raw, _, rest = self.pending.partition(bytes([END]))
                self.pending = bytearray(rest)
                if raw:
                    return unslip(raw)
            left = deadline - time.monotonic()
            if left <= 0:
                raise BridgeError(f"no answer from the design on {self.port} within {self.timeout:g} s")
            if select.select([self.fd], [], [], left)[0]:
                try:
                    self.pending += os.read(self.fd, 4096)
                except (BlockingIOError, OSError):
                    pass

    def hello(self):
        self.send(bytes([HELLO]))
        deadline = time.monotonic() + self.timeout
        while True:
            frame = self.receive(deadline)
            if frame[0] == HELLO and len(frame) == 6:
                break
        version, digest = frame[1], int.from_bytes(frame[2:6], "little")
        if version != PROTOCOL:
            raise BridgeError(f"the board speaks bridge protocol {version}; this kgbridge speaks {PROTOCOL}")
        if self.device is not None and digest != self.device.HASH:
            raise BridgeError(
                f"the board runs a different build of {self.device.INTERFACE['module']} (interface {digest:08x}, "
                f"this module {self.device.HASH:08x}): flash the bitstream this module was generated with, or rebuild"
            )

    # Calls.

    def call(self, f, args):
        steps = self._exchange(f, args)
        return _drive_async(steps) if self.asynchronous else _drive(steps)

    def _exchange(self, f, args):
        """The call as a generator: it yields a callback handler's awaitable
        result and is sent its value, so one body serves both modes."""
        device = self.device
        if f["name"] not in device.BRIDGED:
            raise BridgeError(f"{f['name']} is wired to the board's own resources, not the bridge; under cocotb it can be called directly")
        self.send(bytes([f["index"]]) + encode(f["args"], args))
        deadline = time.monotonic() + self.timeout
        while True:
            frame = self.receive(deadline)
            op = frame[0]
            if op == f["index"]:
                if f["result"] is None:
                    return None
                return decode([f["result"]], frame[1:])[0]
            if op == REFUSED and len(frame) == 3:
                raise BridgeError(f"the board refused {f['name']}: {REFUSALS.get(frame[1], frame[1])}")
            if op & 0x80 and op != HELLO:
                cb = device.INTERFACE["callbacks"][op & 0x7F]
                result = device._handle(cb, decode(cb["args"], frame[1:]))
                if inspect.isawaitable(result):
                    result = yield result
                if cb["protocol"] == "handshake":
                    payload = b"" if cb["result"] is None else encode([cb["result"]], [result])
                    self.send(bytes([op]) + payload)
                deadline = time.monotonic() + self.timeout


def _drive(steps):
    """Runs an exchange to its result, an async handler's with asyncio."""
    try:
        pending = next(steps)
        while True:
            pending = steps.send(asyncio.run(_await(pending)))
    except StopIteration as done:
        return done.value


async def _drive_async(steps):
    try:
        pending = next(steps)
        while True:
            pending = steps.send(await pending)
    except StopIteration as done:
        return done.value


async def _await(awaitable):
    return await awaitable


def unslip(raw):
    out, escaped = bytearray(), False
    for b in raw:
        if escaped:
            out.append(END if b == ESC_END else ESC if b == ESC_ESC else b)
            escaped = False
        elif b == ESC:
            escaped = True
        else:
            out.append(b)
    return bytes(out)


# --- Cocotb: the generated module's ports, driven directly ------------------


class Cocotb:
    """The generated module under cocotb: every function, bound to a board
    resource or not, is called by driving its ports with its protocol.

    One coroutine runs the ports a cycle at a time. At each falling edge it
    drives what this cycle asks; once the cycle has settled (ReadOnly) it
    reads what the design answered, so a transfer seen there happens at the
    next rising edge.
    """

    def __init__(self, dut, clock_period_ns=10):
        self.dut = dut
        self.period = clock_period_ns
        self.asynchronous = True
        self.device = None
        self.cycle = 0
        self._ports = {}

    def attach(self, device):
        self.device = device
        self.queues = {f["name"]: [] for f in device.INTERFACE["methods"]}  # calls waiting to start
        self.waiting = {f["name"]: [] for f in device.INTERFACE["methods"]}  # started, waiting for results
        self.answers = {f["name"]: [] for f in device.INTERFACE["callbacks"]}  # host answers, in call order
        self.popping = {f["name"]: False for f in device.INTERFACE["methods"] + device.INTERFACE["callbacks"]}
        self.held = {}  # a fixed-latency callback's last answer

    def port(self, name):
        # By name, not attribute: cocotb hides attributes named with a leading
        # underscore, as every callback's ports are.
        handle = self._ports.get(name)
        if handle is None:
            handle = self._ports[name] = self.dut[name]
        return handle

    async def start(self):
        """Clocks, resets and starts the design; call before any function."""
        import cocotb
        from cocotb.clock import Clock
        from cocotb.triggers import RisingEdge

        dut = self.dut
        cocotb.start_soon(Clock(dut.clk, self.period, unit="ns").start())
        for f in self.device.INTERFACE["methods"] + self.device.INTERFACE["callbacks"]:
            for role in ("valid_in", "rden_in"):
                if f["ports"].get(role):
                    self.port(f["ports"][role]).value = 0
            if not f["callback"]:
                for p in f["ports"]["args"]:
                    self.port(p).value = 0
            elif f["ports"].get("result") and f["protocol"] != "async":
                self.port(f["ports"]["result"]).value = 0
        dut.stall_rate_valid_in.value = 0
        dut.stall_rate_in.value = 0
        dut.rst.value = 1
        for _ in range(16):
            await RisingEdge(dut.clk)
        dut.rst.value = 0
        while not int(dut.rst_and_startup_done_out.value):
            await RisingEdge(dut.clk)
        cocotb.start_soon(self._run())

    def close(self):
        pass

    def call(self, f, args):
        return self._call(f, args)

    async def _call(self, f, args):
        from cocotb.triggers import Event

        slot = {"event": Event(), "bits": [pack(d, v) for d, v in zip(f["args"], args)]}
        self.queues[f["name"]].append(slot)
        await slot["event"].wait()
        if "error" in slot:
            raise slot["error"]
        return None if f["result"] is None else unpack(f["result"], slot["result"])

    async def _run(self):
        import cocotb
        from cocotb.triggers import FallingEdge, ReadOnly

        device = self.device
        methods, callbacks = device.INTERFACE["methods"], device.INTERFACE["callbacks"]
        due = []  # (cycle, method, slot): fixed-latency results
        while True:
            await FallingEdge(self.dut.clk)
            self.cycle += 1
            driven = {}
            # Drive.
            for f in methods:
                p, queue = f["ports"], self.queues[f["name"]]
                go = bool(queue)
                self.port(p["valid_in"]).value = int(go)
                # Arguments only with their call, zero between: a result read
                # at the wrong cycle then reads wrong, not by luck right.
                for i, port_name in enumerate(p["args"]):
                    self.port(port_name).value = queue[0]["bits"][i] if go else 0
                driven[f["name"]] = go
                if p.get("rden_in"):
                    self.port(p["rden_in"]).value = int(self.popping[f["name"]])
            for f in callbacks:
                p = f["ports"]
                if p.get("rden_in"):
                    self.port(p["rden_in"]).value = int(self.popping[f["name"]])
                if f["protocol"] == "fixed" and p.get("result"):
                    self.port(p["result"]).value = self.held.get(f["name"], 0)
                if f["protocol"] == "handshake":
                    answers = self.answers[f["name"]]
                    ready = bool(answers) and answers[0]["done"]
                    self.port(p["valid_in"]).value = int(ready)
                    if ready and p.get("result"):
                        self.port(p["result"]).value = answers[0]["bits"]
            # Read, settled.
            await ReadOnly()
            for f in callbacks:
                p, name = f["ports"], f["name"]
                taken = None
                if f["protocol"] in ("handshake", "async"):
                    if self.popping[name]:
                        taken = [int(self.port(a).value) for a in p["args"]]
                    self.popping[name] = not int(self.port(p["empty_out"]).value) and not self.popping[name]
                    if f["protocol"] == "handshake":
                        answers = self.answers[name]
                        if answers and answers[0]["done"] and int(self.port(p["rdy_out"]).value):
                            answers.pop(0)
                elif int(self.port(p["valid_out"]).value):
                    taken = [int(self.port(a).value) for a in p["args"]]
                if taken is not None:
                    args = [unpack(d, b) for d, b in zip(f["args"], taken)]
                    slot = {"done": False, "bits": 0}
                    if f["protocol"] == "handshake":
                        self.answers[name].append(slot)
                    # A plain handler answers at once, before any method
                    # completing in this cycle wakes its caller; an async
                    # one, when it is done.
                    result = self.device._handle(f, args)
                    if inspect.isawaitable(result):
                        cocotb.start_soon(self._answer(f, result, slot))
                    else:
                        self._settle(f, result, slot)
            for f in methods:
                p, name = f["ports"], f["name"]
                if driven[name]:
                    accepted = f["protocol"] != "handshake" or int(self.port(p["rdy_out"]).value)
                    if accepted:
                        slot = self.queues[name].pop(0)
                        if f["protocol"] == "fixed":
                            due.append((self.cycle + (f["latency"] or 0), f, slot))
                        else:
                            self.waiting[name].append(slot)
                if f["protocol"] == "handshake":
                    if self.popping[name]:
                        self._complete(f, self.waiting[name].pop(0) if self.waiting[name] else None, p.get("result"))
                    self.popping[name] = not int(self.port(p["empty_out"]).value) and not self.popping[name]
                elif f["protocol"] == "no_backpressure":
                    if p.get("valid_out"):
                        if int(self.port(p["valid_out"]).value) and self.waiting[name]:
                            self._complete(f, self.waiting[name].pop(0), p.get("result"))
                    elif self.waiting[name]:
                        self._complete(f, self.waiting[name].pop(0), None)
            for entry in [d for d in due if d[0] == self.cycle]:
                due.remove(entry)
                self._complete(entry[1], entry[2], entry[1]["ports"].get("result"))

    def _complete(self, f, slot, result_port):
        if slot is None:
            return
        if result_port:
            slot["result"] = int(self.port(result_port).value)
        slot["event"].set()

    async def _answer(self, f, pending, slot):
        try:
            self._settle(f, await pending, slot)
        finally:
            slot["done"] = True

    def _settle(self, f, result, slot):
        if f["result"] is not None:
            slot["bits"] = pack(f["result"], result)
            if f["protocol"] == "fixed":
                # Driven from the next cycle on: a fixed-latency answer
                # cannot wait for Python, so it is the last one given.
                self.held[f["name"]] = slot["bits"]
        slot["done"] = True


# --- Device models, for callbacks no hardware answers -----------------------


class SpiFlash:
    """A SPI flash at the level of the `spi` callback, `(uint8 mosi, bool
    hold) -> uint8`: what platform/sim/models/spi_flash_model.sv answers on
    the pins, so a test reads the same bytes under cocotb, where the
    callback comes to Python, as under `kg board serve`, where the RTL
    engine and that model answer it.

        api.on("_spi", kgbridge.SpiFlash())
    """

    JEDEC_ID = (0xEF, 0x40, 0x16)

    def __init__(self, depth=256, memory=None):
        self.memory = list(memory) if memory is not None else [(i & 0xFF) ^ 0xA5 for i in range(depth)]
        self.transaction = []

    def __call__(self, mosi, hold):
        n = len(self.transaction)
        self.transaction.append(mosi)
        command = self.transaction[0]
        out = 0
        if command == 0x9F and 1 <= n <= 3:
            out = self.JEDEC_ID[n - 1]
        elif command == 0x03 and n >= 4:
            t = self.transaction
            address = (t[1] << 16 | t[2] << 8 | t[3]) + n - 4
            out = self.memory[address % len(self.memory)]
        if not hold:
            self.transaction = []
        return out


class I2cTarget:
    """An I2C target at the level of the `i2c` callback, `(uint2 op, uint8
    data) -> uint9`: platform/sim/models/i2c_target_model.sv's register file
    (sixteen bytes reset to 0x10 + i, behind a pointer) at `address`.

        api.on("_i2c", kgbridge.I2cTarget(0x48))
    """

    START, STOP, WRITE, READ = 0, 1, 2, 3
    NACK = 0x100

    def __init__(self, address=0x48):
        self.address = address
        self.registers = [0x10 + i for i in range(16)]
        self.pointer = 0
        self.state = "idle"

    def __call__(self, op, data):
        if op == self.START:
            self.state = "address"
        elif op == self.STOP:
            self.state = "idle"
        elif op == self.WRITE:
            if self.state == "address":
                if data >> 1 != self.address:
                    self.state = "ignored"
                    return self.NACK
                self.state = "read" if data & 1 else "pointer"
            elif self.state == "pointer":
                self.pointer, self.state = data & 15, "write"
            elif self.state == "write":
                self.registers[self.pointer] = data
                self.pointer = (self.pointer + 1) % 16
            else:
                return self.NACK
        elif op == self.READ:
            if self.state != "read":
                return 0xFF  # nobody drives the bus: the pull-up reads ones
            value = self.registers[self.pointer]
            self.pointer = (self.pointer + 1) % 16
            return value
        return 0


# --- Tests that run everywhere ----------------------------------------------

_TESTS = []


def test(device_class, timeout_ms=10):
    """Declares `async def name(api)` a test of `device_class`: a cocotb test
    under `kg board test`, and one `main()` runs over the serial bridge.

    Under cocotb a test fails after `timeout_ms` of simulated time (a
    million cycles by default), so a design that never answers says so."""

    def wrap(fn):
        try:
            import cocotb

            simulating = cocotb.is_simulation
        except ImportError:
            simulating = False
        if not simulating:
            _TESTS.append((fn, device_class))
            return fn

        async def under_cocotb(dut):
            transport = Cocotb(dut)
            api = device_class(transport)
            await transport.start()
            await fn(api)

        under_cocotb.__name__ = under_cocotb.__qualname__ = fn.__name__
        under_cocotb.__doc__ = fn.__doc__
        under_cocotb.__module__ = fn.__module__
        return cocotb.test(timeout_time=timeout_ms, timeout_unit="ms")(under_cocotb)

    return wrap


def main(argv=None):
    """Runs every `@kgbridge.test` in the calling script over the serial
    bridge: the board (--port) or `kg board serve`'s simulation (BOARD_PTY).
    Exits 0 when every test passed."""
    import argparse
    import traceback

    parser = argparse.ArgumentParser(description="Run this file's kgbridge tests over the serial bridge.")
    parser.add_argument("--port", help="the board's serial port (default: $BOARD_PTY, under `kg board serve`)")
    parser.add_argument("--baud", type=int, default=None)
    parser.add_argument("--timeout", type=float, default=None, help="seconds to wait for each answer")
    parser.add_argument("-k", dest="match", help="only the tests whose names contain this")
    args = parser.parse_args(argv)
    failed = 0
    tests = [(fn, cls) for fn, cls in _TESTS if not args.match or args.match in fn.__name__]
    for fn, cls in tests:
        serial = Serial(args.port, baud=args.baud or getattr(cls, "BAUD", 115200), timeout=args.timeout, asynchronous=True)
        api = cls(serial)
        start = time.monotonic()
        try:
            asyncio.run(fn(api))
            print(f"PASS {fn.__name__} ({time.monotonic() - start:.2f} s)")
        except Exception:
            failed += 1
            print(f"FAIL {fn.__name__}")
            traceback.print_exc()
        finally:
            api.close()
    print(f"{'PASS' if failed == 0 else 'FAIL'}: {len(tests) - failed} of {len(tests)} tests")
    sys.exit(1 if failed else 0)


# --- `kg board test`: cocotb's runner over the board's sources --------------


def _cocotb(argv):
    """Builds the generated module with Verilator and runs cocotb test
    modules on it; `kg board test` calls this with the flow's sources."""
    import argparse

    parser = argparse.ArgumentParser(prog="kgbridge.py cocotb")
    parser.add_argument("--toplevel", required=True)
    parser.add_argument("--sources", required=True, help="a file listing the sources, one a line")
    parser.add_argument("--build-dir", required=True)
    parser.add_argument("--test-dir", required=True)
    parser.add_argument("--module", action="append", required=True, help="a test module; repeatable")
    parser.add_argument("--define", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        from cocotb_tools.runner import get_runner
        from cocotb_tools.check_results import get_results
    except ImportError:
        print("kgbridge: cocotb 2 is not installed for this python (`pip install cocotb`, or `uv pip install cocotb`)", file=sys.stderr)
        return 2
    with open(args.sources) as f:
        sources = [line.strip() for line in f if line.strip()]
    runner = get_runner("verilator")
    runner.build(
        sources=sources,
        hdl_toplevel=args.toplevel,
        defines={d: 1 for d in args.define},
        build_args=["-Wno-fatal", "-Wno-lint", "-Wno-style", "--timing"],
        build_dir=args.build_dir,
        always=True,
        log_file=os.path.join(args.build_dir, "build.log"),
    )
    here = os.path.dirname(os.path.abspath(__file__))
    path = os.pathsep.join(p for p in [here, args.test_dir, os.environ.get("PYTHONPATH", "")] if p)
    results = runner.test(
        hdl_toplevel=args.toplevel,
        test_module=args.module,
        build_dir=args.build_dir,
        test_dir=args.test_dir,
        results_xml=os.path.join(os.path.abspath(args.build_dir), "results.xml"),
        extra_env={"PYTHONPATH": path},
    )
    total, failed = get_results(results)
    print(f"{'PASS' if failed == 0 and total > 0 else 'FAIL'}: {total - failed} of {total} cocotb tests ({results})")
    return 0 if failed == 0 and total > 0 else 1


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "cocotb":
        sys.exit(_cocotb(sys.argv[2:]))
    print(__doc__)
