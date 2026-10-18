// KanagawaHALShowAheadFifo for iCE40. Interface: upstream's HAL
// (runtime/rtl/hal/mock/hal_show_ahead_fifo.sv).
//
// USE_LUTRAM comes from the backend (ShouldFifoUseLutRam): a FIFO it left
// out of block RAM is flip-flops here, built on the runtime's register FIFO
// exactly as the mock HAL builds it, so its almost_full timing is the one
// min_almost_full_depth (lattice.ice40.config) was taken from. A FIFO the
// backend put in block RAM is KanagawaIce40BlockRamFifo, below.

`default_nettype none

module KanagawaHALShowAheadFifo
#(
    parameter DEPTH = 16,
    parameter LOG_DEPTH = $clog2(DEPTH),
    parameter WIDTH = 32,
    parameter ALMOSTFULL_ENTRIES = 1,
    parameter ALMOSTEMPTY_VAL = 1,
    parameter USE_LUTRAM = 0,
    parameter OVER_UNDER_FLOW_PROTECTION = 0
)
(
    input wire                       clock,
    input wire                       rst,

    input wire                       wrreq,
    input wire [WIDTH-1:0]           data,
    output logic                     full,
    output logic                     almost_full,
    output logic [LOG_DEPTH:0]       usedw,

    input wire                       rdreq,
    output logic                     empty,
    output logic                     almost_empty,
    output logic [WIDTH-1:0]         q
);
    generate
        if (USE_LUTRAM != 0) begin : gen_registers
            KanagawaShowAheadRegisterFifo
            #(
                .LOG_DEPTH                  (LOG_DEPTH),
                .DEPTH                      (DEPTH),
                .WIDTH                      (WIDTH),
                .ALMOSTFULL_ENTRIES         (ALMOSTFULL_ENTRIES),
                .ALMOSTEMPTY_VAL            (ALMOSTEMPTY_VAL),
                .OVER_UNDER_FLOW_PROTECTION (OVER_UNDER_FLOW_PROTECTION),
                .ASSERT_ON_OVER_UNDER_FLOW  (OVER_UNDER_FLOW_PROTECTION != 0 ? 0 : 1),
                .MUX_ON_READ                (1)
            ) fifo
            (
                .clock          (clock),
                .rst            (rst),
                .wrreq          (wrreq),
                .data           (data),
                .full           (full),
                .almost_full    (almost_full),
                .usedw          (usedw),
                .rdreq          (rdreq),
                .empty          (empty),
                .almost_empty   (almost_empty),
                .q              (q)
            );
        end
        else begin : gen_block_ram
            KanagawaIce40BlockRamFifo
            #(
                .DEPTH                      (DEPTH),
                .LOG_DEPTH                  (LOG_DEPTH),
                .WIDTH                      (WIDTH),
                .ALMOSTFULL_ENTRIES         (ALMOSTFULL_ENTRIES),
                .ALMOSTEMPTY_VAL            (ALMOSTEMPTY_VAL),
                .OVER_UNDER_FLOW_PROTECTION (OVER_UNDER_FLOW_PROTECTION)
            ) fifo
            (
                .clock          (clock),
                .rst            (rst),
                .wrreq          (wrreq),
                .data           (data),
                .full           (full),
                .almost_full    (almost_full),
                .usedw          (usedw),
                .rdreq          (rdreq),
                .empty          (empty),
                .almost_empty   (almost_empty),
                .q              (q)
            );
        end
    endgenerate
endmodule

// A show-ahead FIFO in block RAM. SB_RAM40_4K reads synchronously, so the
// head of the FIFO is the RAM's read register itself: a read is issued the
// cycle the head is popped (or found empty), and the next entry is on `q`
// the cycle after, sustaining one entry per cycle. Occupancy counts the RAM
// and the head together, and flags are combinational from the counters, so
// almost_full is never later than the register FIFO's.
//
// The RAM is never read at the address being written: a read needs an entry
// the RAM already holds, and a write is refused while it is full.
module KanagawaIce40BlockRamFifo
#(
    parameter DEPTH = 256,
    parameter LOG_DEPTH = $clog2(DEPTH),
    parameter WIDTH = 16,
    parameter ALMOSTFULL_ENTRIES = 1,
    parameter ALMOSTEMPTY_VAL = 0,
    parameter OVER_UNDER_FLOW_PROTECTION = 0
)
(
    input wire                       clock,
    input wire                       rst,

    input wire                       wrreq,
    input wire [WIDTH-1:0]           data,
    output logic                     full,
    output logic                     almost_full,
    output logic [LOG_DEPTH:0]       usedw,

    input wire                       rdreq,
    output logic                     empty,
    output logic                     almost_empty,
    output logic [WIDTH-1:0]         q
);
    (* ram_style = "block" *) logic [WIDTH-1:0] mem [0:DEPTH-1];

    logic [LOG_DEPTH-1:0] wrptr, rdptr;
    logic [LOG_DEPTH:0]   in_ram;       // entries written and not yet read out of the RAM
    logic                 head_valid;   // `q` holds the oldest entry

    wire push = wrreq & ((OVER_UNDER_FLOW_PROTECTION == 0) | ~full);
    wire pop  = rdreq & ((OVER_UNDER_FLOW_PROTECTION == 0) | ~empty);

    // Refill the head when it is empty or leaving, from an entry in the RAM.
    wire fetch = (in_ram != '0) & (~head_valid | pop);

    always_ff @(posedge clock) begin
        if (push) mem[wrptr] <= data;
        if (fetch) q <= mem[rdptr];
    end

    always_ff @(posedge clock) begin
        if (rst) begin
            wrptr <= '0;
            rdptr <= '0;
            in_ram <= '0;
            head_valid <= 1'b0;
        end
        else begin
            if (push) wrptr <= wrptr + 1'b1;
            if (fetch) rdptr <= rdptr + 1'b1;
            in_ram <= in_ram + push - fetch;
            if (fetch) head_valid <= 1'b1;
            else if (pop) head_valid <= 1'b0;
        end
    end

    // Full through reset and the cycle after it: the runtime holds its
    // interface FIFOs in reset through start-up to refuse requests, and
    // asserts almost_full the cycle after rst (write_delay_fifo.sv:74), as
    // the register FIFO's registered flag has it.
    logic rst_q;
    always_ff @(posedge clock) rst_q <= rst;

    // The head is a slot beyond the RAM's DEPTH, but usedw must not exceed
    // DEPTH: the backend sized almost_full against DEPTH entries.
    assign usedw = in_ram + head_valid;
    assign empty = ~head_valid;
    assign full = (usedw >= DEPTH) | rst | rst_q;
    assign almost_full = (usedw >= (DEPTH - ALMOSTFULL_ENTRIES)) | rst | rst_q;
    assign almost_empty = (usedw <= ALMOSTEMPTY_VAL);
endmodule

`default_nettype wire
