// UPduino v3 monitor: read and write the SPRAM over the board's serial port.
//
// The protocol is a line per command, ASCII, any mix of CR and LF ending
// it; a blank line is ignored:
//
//     r AAAA          read the word at AAAA, reply "DDDD"
//     w AAAA DDDD     write DDDD at AAAA, reply "DDDD"
//
// Addresses and data are exactly four hex digits, either case (the SPRAM
// is 32K words of 16 bits: the address's top bit is ignored). Every reply
// is one line ending CR LF; anything else on a line gets "?". Exactly four
// digits keeps a reply no longer than the command that caused it, so a host
// streaming commands at the line's full rate never outruns the replies.
// host.py beside this file speaks the protocol.
//
// The serial line itself is RTL (rtl/uart.sv); this class sees bytes:
// - `rx` takes each byte as it arrives. It parses, runs the command on the
//   SPRAM and writes the reply. `[[no_backpressure]]`: a byte arrives every
//   2,083 cycles at most, and nothing here ever needs to refuse one.
// - `tx_next` hands the transmitter the next reply byte whenever it is idle.
//   `[[latency(0)]]`: the core asks and takes the answer in the same cycle.
// - `leds` gives the RGB LED: green after a good command, red after a bad
//   one, blue toggling with each command.
//
// Each variable has one writer: `rx` owns the parser, the reply and the
// status, `tx_next` its place in the reply. Neither waits on the other. A
// reply ready while the previous one is still going out is dropped. Only a
// "?" can be: a line shorter than the reply still going out is under seven
// bytes, and no command is that short. So a drop always follows a bad line,
// and the LED is already red.
//
// A reply is kept as the 16-bit value it reports (or a flag for "?"), and
// `tx_next` writes each byte as it goes out: one hex converter rather than
// four, and 17 bits of state rather than six bytes and their multiplexer
// (platform-layer.md §8.7).

import lattice.spram

const uint8 CR = 0x0D;
const uint8 LF = 0x0A;
const uint8 SPACE = 0x20;
const uint8 QUESTION = 0x3F;

class Monitor
{
private:
    spram<32768, 16> _ram;

    // The line being parsed; zero is the state after a line ends.
    uint2 _command;         // 0 none yet, 1 read, 2 write, 3 malformed
    bool _second_field;     // a write's data, after its address
    uint3 _digits;          // in the current field
    uint15 _addr;           // the SPRAM's 32K words: a fourth digit's top bit falls off
    uint16 _data;

    // The reply: the value it reports, or "?"; toggled with each new one.
    uint16 _value;
    bool _malformed;
    bool _reply;

    // The transmitter's place: the reply it is on, and the bytes still to
    // send of it. Zero is idle, which is also how the flip-flops start.
    bool _sent;
    uint3 _remaining;

    // LED state.
    bool _bad;
    bool _activity;

    inline bool is_hex(uint8 c)
    {
        return (c >= 0x30 && c <= 0x39) || (c >= 0x41 && c <= 0x46) || (c >= 0x61 && c <= 0x66);
    }

    inline uint4 hex_value(uint8 c)
    {
        return cast<uint4>(c <= 0x39 ? c - 0x30 : (c & 0x5F) - 0x41 + 10);
    }

    inline uint8 hex_digit(uint4 n)
    {
        return n < 10 ? 0x30 + n : 0x61 + n - 10;
    }

public:
    //| One received byte.
    [[no_backpressure]] void rx(uint8 c)
    {
        bool line_end = (c == CR) || (c == LF);
        bool execute;
        bool reply;
        bool write;
        uint15 addr;
        uint16 data;

        atomic
        {
            uint2 command = _command;
            bool second = _second_field;
            uint3 digits = _digits;

            // A line's verdict at its end: run it, answer "?", or nothing
            // for a blank line.
            bool read_ok = command == 1 && !second && digits == 4;
            bool write_ok = command == 2 && second && digits == 4;
            execute = line_end && (read_ok || write_ok);
            reply = line_end && !(command == 0 && digits == 0);
            write = command == 2;
            addr = _addr;
            data = _data;

            if (line_end)
            {
                command = 0;
                second = false;
                digits = 0;
            }
            else if (command == 0 && (c == 0x72 || c == 0x52))
            {
                command = 1;
            }
            else if (command == 0 && (c == 0x77 || c == 0x57))
            {
                command = 2;
            }
            else if (c == SPACE && digits == 0 && command != 0)
            {
                // Spaces before a field's digits.
            }
            else if (c == SPACE && command == 2 && !second && digits == 4)
            {
                second = true;
                digits = 0;
            }
            else if (is_hex(c) && (command == 1 || command == 2) && digits < 4)
            {
                if (second)
                {
                    _data = cast<uint16>(_data << 4) | hex_value(c);
                }
                else
                {
                    _addr = cast<uint15>(_addr << 4) | hex_value(c);
                }
                digits = digits + 1;
            }
            else
            {
                command = 3;
            }

            _command = command;
            _second_field = second;
            _digits = digits;
        }

        // The SPRAM's one call site: every command shares it.
        uint16 word = _ram.access(addr, execute && write, data, 0xF);
        uint16 value = write ? data : word;

        if (reply)
        {
            atomic
            {
                // Still sending the last reply: drop this one.
                bool busy = _sent != _reply || _remaining != 0;
                if (!busy)
                {
                    _value = value;
                    _malformed = !execute;
                    _reply = !_reply;
                }
                _bad = !execute;
                _activity = !_activity;
            }
        }
    }

    //| The next byte to send, while the transmitter is idle: bit 8 says
    // there is one, bits 7:0 are it.
    [[latency(0)]] uint9 tx_next()
    {
        uint9 result;
        atomic
        {
            bool malformed = _malformed;
            uint3 length = malformed ? 3 : 6;
            uint3 remaining = _remaining;
            if (_sent != _reply)
            {
                // A new reply: all of it to send.
                remaining = length;
            }
            uint3 index = length - remaining;

            // "DDDD" CR LF, or "?" CR LF.
            uint8 c;
            if (malformed)
            {
                c = index == 0 ? QUESTION : (index == 1 ? CR : LF);
            }
            else
            {
                uint4 nibble = cast<uint4>(_value >> ((3 - index) * 4));
                c = index < 4 ? hex_digit(nibble) : (index == 4 ? CR : LF);
            }

            result = remaining != 0 ? (0x100 | c) : 0;
            _sent = _reply;
            _remaining = remaining != 0 ? remaining - 1 : 0;
        }
        return result;
    }

    //| The LED: {red, green, blue}.
    [[latency(0)]] uint3 leds()
    {
        return (_bad ? 4 : 2) | (_activity ? 1 : 0);
    }
}

export Monitor;
