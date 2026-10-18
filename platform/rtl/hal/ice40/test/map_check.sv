// Synthesis tops for check.zsh: each instantiates one HAL storage decision,
// and check.zsh requires Yosys to map it to the primitive the decision names
// (the `expect` comment above each module, read by check.zsh). Synthesis runs
// without the icestorm flow's rule that unmarked memories are logic, so the
// HAL's own attributes are what is checked; only a top marked `policy` gets
// the rule, for the runtime's register FIFO, which carries no attribute.

`default_nettype none

// expect SB_RAM40_4K=0 policy
module map_fifo_registers (input wire clk, rst, push, pop, input wire [15:0] d, output wire [15:0] q, output wire empty, full);
    KanagawaHALShowAheadFifo #(.DEPTH(32), .WIDTH(16), .ALMOSTFULL_ENTRIES(4), .USE_LUTRAM(1)) f
        (.clock(clk), .rst(rst), .wrreq(push), .data(d), .full(full), .almost_full(), .usedw(),
         .rdreq(pop), .empty(empty), .almost_empty(), .q(q));
endmodule

// expect SB_RAM40_4K=1
module map_fifo_block (input wire clk, rst, push, pop, input wire [15:0] d, output wire [15:0] q, output wire empty, full);
    KanagawaHALShowAheadFifo #(.DEPTH(256), .WIDTH(16), .ALMOSTFULL_ENTRIES(4), .USE_LUTRAM(0)) f
        (.clock(clk), .rst(rst), .wrreq(push), .data(d), .full(full), .almost_full(), .usedw(),
         .rdreq(pop), .empty(empty), .almost_empty(), .q(q));
endmodule

// expect SB_RAM40_4K=0
module map_ram_logic (input wire clk, we, re, input wire [7:0] wa, ra, input wire [15:0] d, output wire [15:0] q);
    wire [1:0][15:0] o;
    KanagawaHALDualPortRAM #(.DATA_WIDTH(16), .ADDR_WIDTH(8), .USE_OUTPUT_REG(1), .USE_LUTRAM(1)) r
        (.clk(clk), .addr_in({ra, wa}), .wren_in({1'b0, we}), .data_in({16'd0, d}),
         .inject_correctable_error_in(1'b0), .inject_uncorrectable_error_in(1'b0),
         .rden_in({re, 1'b0}), .data_out(o), .error_detected_out(), .data_valid_out());
    assign q = o[1];
endmodule

// expect SB_RAM40_4K=1
module map_ram_block (input wire clk, we, re, input wire [7:0] wa, ra, input wire [15:0] d, output wire [15:0] q);
    wire [1:0][15:0] o;
    KanagawaHALDualPortRAM #(.DATA_WIDTH(16), .ADDR_WIDTH(8), .USE_OUTPUT_REG(1), .USE_BRAM(1)) r
        (.clk(clk), .addr_in({ra, wa}), .wren_in({1'b0, we}), .data_in({16'd0, d}),
         .inject_correctable_error_in(1'b0), .inject_uncorrectable_error_in(1'b0),
         .rden_in({re, 1'b0}), .data_out(o), .error_detected_out(), .data_valid_out());
    assign q = o[1];
endmodule

// expect SB_SPRAM256KA=4
module map_spram (input wire clk, v, w, input wire [15:0] a, input wire [15:0] d, output wire [15:0] q);
    KanagawaIce40Spram #(.Words(65536), .Width(16)) s
        (.clk(clk), .rst(1'b0), .access_valid_in(v), .access_addr_in(a), .access_write_in(w),
         .access_data_in(d), .access_nibble_mask_in(4'hF), .access_result_out(q));
endmodule

// Every UP5K multiplication is KanagawaLutMul (hal_lutmul.sv): a product
// that fits a DSP block must reach one.
// expect SB_MAC16=1
module map_lutmul (input wire [15:0] x, y, output wire [31:0] z);
    KanagawaLutMul #(.DW_X(16), .DW_Y(16), .DW_Z(32)) m (.x_in(x), .y_in(y), .z_out(z));
endmodule

// expect SB_MAC16=1
module map_lutmul_signed (input wire [7:0] x, y, output wire [15:0] z);
    KanagawaLutMul #(.DW_X(8), .DW_Y(8), .X_SIGNED(1), .Y_SIGNED(1), .DW_Z(16)) m (.x_in(x), .y_in(y), .z_out(z));
endmodule

// The backend's pipeline after a product: two registers, which crash
// Yosys 0.69's ice40_dsp when the product is inferred (hal_lutmul.sv).
// expect SB_MAC16=1
module map_lutmul_pipelined (input wire clk, input wire [15:0] x, y, output logic [31:0] z);
    wire [31:0] p;
    logic [31:0] r;
    KanagawaLutMul #(.DW_X(16), .DW_Y(16), .DW_Z(32)) m (.x_in(x), .y_in(y), .z_out(p));
    always_ff @(posedge clk) begin
        r <= p;
        z <= r;
    end
endmodule

// `kg board` synthesises without -dsp (crates/kg-board/src/icestorm.rs):
// a `*` written by hand is logic.
// expect SB_MAC16=0
module map_mul (input wire [15:0] x, y, output wire [31:0] z);
    assign z = x * y;
endmodule

`default_nettype wire
