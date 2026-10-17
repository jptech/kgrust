/*|
Characters for a serial console: hex digits each way, and the ASCII codes a
line protocol needs. Pure functions, so a design calls them anywhere.
*/
module periph.text
    { CR
    , LF
    , SPACE
    , is_hex
    , hex_value
    , hex_digit
    }

const uint8 CR = 0x0D;
const uint8 LF = 0x0A;
const uint8 SPACE = 0x20;

//| Whether `c` is 0-9, A-F or a-f.
inline bool is_hex(uint8 c)
{
    return (c >= 0x30 && c <= 0x39) || (c >= 0x41 && c <= 0x46) || (c >= 0x61 && c <= 0x66);
}

//| The value of a hex digit, either case; anything else gives nonsense, so
// check `is_hex` first.
inline uint4 hex_value(uint8 c)
{
    return cast<uint4>(c <= 0x39 ? c - 0x30 : (c & 0x5F) - 0x41 + 10);
}

//| The lower-case hex digit for `n`.
inline uint8 hex_digit(uint4 n)
{
    return n < 10 ? 0x30 + n : 0x61 + n - 10;
}
