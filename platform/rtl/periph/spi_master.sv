// An SPI master behind a Kanagawa callback `(uint8 mosi, bool hold) ->
// uint8 _spi`: each call shifts one byte out on MOSI, most significant bit
// first, and returns the byte shifted in on MISO at the same time. Chip
// select is asserted for the byte and stays asserted after it while `hold`
// is true, so a transaction of n bytes is n calls, the last with `hold`
// false.
//
// Timing, in half-periods of SCLK (CLK_DIV clocks each, SCLK = clk /
// (2 * CLK_DIV), CLK_DIV >= 1): every byte is one half-period of setup,
// sixteen SCLK edges a half-period apart, and one half-period after the
// last edge. A byte with `hold` false then raises chip select and waits one
// more half-period before returning, so chip select is high for at least a
// half-period between transactions. SCLK idles at CPOL. CPHA = 0 samples
// MISO on the leading edge of each pulse and changes MOSI on the trailing
// one, the first bit set up when the call starts; CPHA = 1 changes MOSI on
// the leading edge and samples on the trailing one. Outside a byte MOSI
// keeps its last value; a device ignores it there.
//
// The call side follows the backend's callback ports: a call is popped
// (`call_rden`) in the cycle the engine starts it, and its result is
// offered, gated by `result_rdy`, until it is taken. A call already queued
// starts in the cycle the previous result is taken.
//
// MISO is sampled without a synchroniser: it changes only in response to
// our own SCLK, a half-period before we sample it, so it is stable at the
// sampling clock edge as long as the round trip from SCLK to MISO is
// shorter than a half-period.

`default_nettype none

module spi_master
#(
    parameter int CLK_DIV = 2,
    parameter bit CPOL = 1'b0,
    parameter bit CPHA = 1'b0
)
(
    input  wire        clk,
    input  wire        rst,

    input  wire        call_empty,
    output logic       call_rden,
    input  wire  [7:0] call_mosi,
    input  wire        call_hold,
    output wire        result_valid,
    output logic [7:0] result,
    input  wire        result_rdy,

    output logic       sclk,
    output logic       mosi,
    input  wire        miso,
    output logic       cs_n
);
    localparam int DW = CLK_DIV > 1 ? $clog2(CLK_DIV) : 1;
    localparam logic [DW-1:0] RELOAD = DW'(CLK_DIV - 1);

    typedef enum logic [1:0] { IDLE, BUSY, DONE } state_t;
    state_t        state;
    logic [DW-1:0] count;   // clocks left in this half-period, less one
    logic [4:0]    step;    // half-periods ended so far in this byte
    logic          hold;
    logic [7:0]    shift;   // bits yet to send at the top, bits received at the bottom

    wire start = !call_empty && (state == IDLE || (state == DONE && result_rdy));
    // Steps 0..15 end with SCLK edges 1..16; an even step's edge is a
    // leading one. CPHA = 0 samples on leading edges, CPHA = 1 on trailing.
    wire sample = (step[0] == CPHA);

    assign call_rden = start;
    assign result_valid = (state == DONE) && result_rdy;
    assign result = shift;

    always_ff @(posedge clk) begin
        if (rst) begin
            state <= IDLE;
            sclk <= CPOL;
            mosi <= 1'b0;
            cs_n <= 1'b1;
        end
        else if (start) begin
            state <= BUSY;
            cs_n <= 1'b0;
            // CPHA = 0 needs the first bit a half-period before the first
            // edge; CPHA = 1 puts it out on that edge, like every other.
            if (!CPHA) mosi <= call_mosi[7];
            shift <= call_mosi;
            hold <= call_hold;
            step <= 5'd0;
            count <= RELOAD;
        end
        else if (state == DONE) begin
            if (result_rdy) state <= IDLE;
        end
        else if (state == BUSY) begin
            if (count != 0) count <= count - 1'b1;
            else begin
                count <= RELOAD;
                step <= step + 5'd1;
                if (step < 5'd16) begin
                    sclk <= !sclk;
                    if (sample) shift <= {shift[6:0], miso};
                    else mosi <= shift[7];
                end
                else if (step == 5'd16 && !hold) cs_n <= 1'b1;
                else state <= DONE;
            end
        end
    end
endmodule

`default_nettype wire
