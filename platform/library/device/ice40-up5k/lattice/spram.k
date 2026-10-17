/*|
The iCE40 UltraPlus single-port RAM: four SB_SPRAM256KA blocks of 16K words
of 16 bits (256 Kb each, 1 Mb in all), outside the compiler's memory model
because a Kanagawa `memory` needs a read and a write port at once.

`spram<Words, Width>` combines one to four blocks in depth (`Words` of 16K,
32K or 64K) and width (`Width` of 16, 32 or 64 bits). It has one port, so
one `access` per cycle: the method is fixed-latency and external, and the
compiler refuses a second call site ("Externally instantiated fixed-latency
function has multiple call sites"). Route every read and write through one
call, as the board example does (platform/boards/upduino-v3/examples/spram).

Only the UP5K has these blocks, so the module lives in its device directory:
`import lattice.spram` fails for any other `--target-device`.
*/
module lattice.spram
    { spram
    }

//| The blocks themselves, implemented by KanagawaIce40Spram
// (platform/rtl/hal/ice40/lattice_spram.sv). The template parameters become
// that module's Verilog parameters.
template <auto Words, auto Width>
class spram_banks
{
public:
    [[latency(1)]] uint<Width> access
        ( index_t<Words> addr
        , bool write
        , uint<Width> data
        , uint<Width / 4> nibble_mask
        );
}

[[name("KanagawaIce40Spram")]]
extern spram_banks;

//| Single-port RAM in the UltraPlus SPRAM blocks.
template
    < auto Words = 16384    //< Number of words: 16384, 32768 or 65536.
    , auto Width = 16       //< Bits per word: 16, 32 or 64.
    >
class spram
{
private:
    static_assert(Words == 16384 || Words == 32768 || Words == 65536);
    static_assert(Width == 16 || Width == 32 || Width == 64);
    //| The part has four blocks.
    static_assert((Words / 16384) * (Width / 16) <= 4);

    spram_banks<Words, Width> _banks;

public:
    using addr_t = index_t<Words>;
    using word_t = uint<Width>;
    using mask_t = uint<Width / 4>;

    //| Read or write one word. The result, one cycle later, is the word read;
    // after a write it is undefined. Each bit of `nibble_mask` enables the
    // write of four bits of `data`, as SB_SPRAM256KA's MASKWREN does.
    inline word_t access
        ( addr_t addr       //< Word address.
        , bool write        //< Write `data` rather than read.
        , word_t data       //< Word to write.
        , mask_t nibble_mask //< Nibbles of `data` to write.
        )
    {
        return _banks.access(addr, write, data, nibble_mask);
    }
}
