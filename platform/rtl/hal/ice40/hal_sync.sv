// The HAL's clock-domain primitives for iCE40. Interfaces: upstream's HAL
// (runtime/rtl/hal/mock/hal_reset_synchronizer.sv, hal_sync_regs.sv).

`default_nettype none

// An asynchronous reset, asserted at once and released on the clock.
module KanagawaHALResetSynchronizer
(
    input  wire     clk,
    input  wire     arst,
    output wire     rst
);
    logic [1:0] sreg = 2'b11;

    always_ff @(posedge clk or posedge arst) begin
        if (arst) sreg <= 2'b11;
        else      sreg <= {sreg[0], 1'b0};
    end

    assign rst = sreg[1];
endmodule

// A synchroniser chain of DEPTH flip-flops per bit (at least 2).
module KanagawaHALSyncRegs
#(
    parameter WIDTH = 32,
    parameter DEPTH = 2
)
(
    input  wire             clk,
    input  wire [WIDTH-1:0] din,
    output wire [WIDTH-1:0] dout
);
    logic [DEPTH-1:0][WIDTH-1:0] chain = '0;

    always_ff @(posedge clk) chain <= {chain[DEPTH-2:0], din};

    assign dout = chain[DEPTH-1];
endmodule

`default_nettype wire
