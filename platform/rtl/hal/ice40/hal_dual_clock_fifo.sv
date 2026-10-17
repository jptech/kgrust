// KanagawaHALDualClockFifo for iCE40: the classic asynchronous FIFO, Gray
// pointers crossing through two-flop synchronisers, the storage read
// combinationally (show-ahead). Interface: upstream's HAL
// (runtime/rtl/hal/mock/hal_dual_clock_fifo.sv). The storage is flip-flops:
// a show-ahead read of SB_RAM40_4K would need the prefetch of
// KanagawaIce40BlockRamFifo on the read clock, and no pilot design crosses
// clocks; a deep one is a candidate for that work (platform-layer.md §8.6).
//
// `full`, `almost_full` and `usedw` are on the write clock and conservative:
// the read pointer they see is two write-clock cycles old.

`default_nettype none

module KanagawaHALDualClockFifo
#(
    parameter DEPTH = 32,
    parameter LOG_DEPTH = $clog2(DEPTH),
    parameter WIDTH = 32,
    parameter USEDW_WIDTH = LOG_DEPTH + 1,
    parameter ALMOSTFULL_ENTRIES = 1,
    parameter USE_LUTRAM = 0,
    parameter OVER_UNDER_FLOW_PROTECTION = 0
)
(
    input wire                       wrclk,
    input wire                       wrrst,
    input wire                       wrreq,
    input wire [WIDTH-1:0]           data,
    output logic                     full,
    output logic                     almost_full,
    output logic [USEDW_WIDTH-1:0]   usedw,

    input wire                       rdclk,
    input wire                       rdrst,
    input wire                       rdreq,
    output logic                     empty,
    output logic [WIDTH-1:0]         q
);
    function automatic [LOG_DEPTH:0] bin2gray(input [LOG_DEPTH:0] b);
        bin2gray = b ^ (b >> 1);
    endfunction

    function automatic [LOG_DEPTH:0] gray2bin(input [LOG_DEPTH:0] g);
        integer i;
        begin
            gray2bin[LOG_DEPTH] = g[LOG_DEPTH];
            for (i = LOG_DEPTH - 1; i >= 0; i = i - 1)
                gray2bin[i] = gray2bin[i + 1] ^ g[i];
        end
    endfunction

    (* ram_style = "logic" *) logic [WIDTH-1:0] mem [0:DEPTH-1];

    // Write side
    logic [LOG_DEPTH:0] wr_bin, wr_gray;
    logic [LOG_DEPTH:0] rd_gray_meta, rd_gray_sync;
    wire  [LOG_DEPTH:0] rd_bin_w = gray2bin(rd_gray_sync);
    wire  push = wrreq & ((OVER_UNDER_FLOW_PROTECTION == 0) | ~full);

    always_ff @(posedge wrclk) begin
        if (push) mem[wr_bin[LOG_DEPTH-1:0]] <= data;
    end

    always_ff @(posedge wrclk) begin
        if (wrrst) begin
            wr_bin <= '0;
            wr_gray <= '0;
            rd_gray_meta <= '0;
            rd_gray_sync <= '0;
        end
        else begin
            if (push) begin
                wr_bin <= wr_bin + 1'b1;
                wr_gray <= bin2gray(wr_bin + 1'b1);
            end
            rd_gray_meta <= rd_gray;
            rd_gray_sync <= rd_gray_meta;
        end
    end

    // Full through reset and the cycle after, as the show-ahead FIFO is.
    logic wrrst_q;
    always_ff @(posedge wrclk) wrrst_q <= wrrst;

    assign usedw = wr_bin - rd_bin_w;
    assign full = (usedw >= DEPTH) | wrrst | wrrst_q;
    assign almost_full = (usedw >= (DEPTH - ALMOSTFULL_ENTRIES)) | wrrst | wrrst_q;

    // Read side
    logic [LOG_DEPTH:0] rd_bin, rd_gray;
    logic [LOG_DEPTH:0] wr_gray_meta, wr_gray_sync;
    wire  pop = rdreq & ((OVER_UNDER_FLOW_PROTECTION == 0) | ~empty);

    always_ff @(posedge rdclk) begin
        if (rdrst) begin
            rd_bin <= '0;
            rd_gray <= '0;
            wr_gray_meta <= '0;
            wr_gray_sync <= '0;
        end
        else begin
            if (pop) begin
                rd_bin <= rd_bin + 1'b1;
                rd_gray <= bin2gray(rd_bin + 1'b1);
            end
            wr_gray_meta <= wr_gray;
            wr_gray_sync <= wr_gray_meta;
        end
    end

    assign empty = (wr_gray_sync == rd_gray);
    assign q = mem[rd_bin[LOG_DEPTH-1:0]];
endmodule

`default_nettype wire
