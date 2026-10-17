// KanagawaHALReadyValidFifo for iCE40: a ready/valid face on the HAL FIFOs,
// as upstream's mock HAL builds it (runtime/rtl/hal/mock/hal_ready_valid_fifo.sv).

`default_nettype none

module KanagawaHALReadyValidFifo
#(
    parameter DEPTH = 32,
    parameter LOG_DEPTH = $clog2(DEPTH),
    parameter WIDTH = 32,
    parameter USE_LUTRAM = 0,
    parameter ALMOSTFULL_ENTRIES = 0,   // must be 0 for dual-clock
    parameter DUAL_CLOCK = 0
)
(
    input wire                  input_clk,
    input wire                  input_rst,
    pd_fifo_intf.IN             input_intf,
    output wire [LOG_DEPTH:0]   input_usedw,
    output wire                 input_almost_full,

    input wire                  output_clk,
    pd_fifo_intf.OUT            output_intf
);
    wire input_full;
    wire output_empty;

    assign input_intf.ready = ~input_full;
    assign output_intf.valid = ~output_empty;

    generate
        if (DUAL_CLOCK != 0) begin : gen_dual_clock
            KanagawaHALDualClockFifo
            #(
                .DEPTH                      (DEPTH),
                .WIDTH                      (WIDTH),
                .ALMOSTFULL_ENTRIES         (ALMOSTFULL_ENTRIES),
                .USE_LUTRAM                 (USE_LUTRAM),
                .OVER_UNDER_FLOW_PROTECTION (1)
            ) fifo
            (
                .wrclk          (input_clk),
                .wrrst          (input_rst),
                .wrreq          (input_intf.valid),
                .data           (input_intf.data),
                .full           (input_full),
                .almost_full    (input_almost_full),
                .usedw          (input_usedw),
                .rdclk          (output_clk),
                .rdrst          (1'b0),
                .rdreq          (output_intf.ready),
                .empty          (output_empty),
                .q              (output_intf.data)
            );
        end
        else begin : gen_single_clock
            KanagawaHALShowAheadFifo
            #(
                .DEPTH                      (DEPTH),
                .WIDTH                      (WIDTH),
                .ALMOSTFULL_ENTRIES         (ALMOSTFULL_ENTRIES),
                .USE_LUTRAM                 (USE_LUTRAM),
                .OVER_UNDER_FLOW_PROTECTION (1)
            ) fifo
            (
                .clock          (input_clk),
                .rst            (input_rst),
                .wrreq          (input_intf.valid),
                .data           (input_intf.data),
                .full           (input_full),
                .almost_full    (input_almost_full),
                .usedw          (input_usedw),
                .rdreq          (output_intf.ready),
                .empty          (output_empty),
                .almost_empty   (),
                .q              (output_intf.data)
            );
        end
    endgenerate
endmodule

`default_nettype wire
