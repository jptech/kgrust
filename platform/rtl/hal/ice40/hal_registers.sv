// The HAL's register modules for iCE40. Interface: upstream's HAL
// (runtime/rtl/hal/mock/hal_non_mergeable_registers.sv). `keep` is the
// attribute lattice.ice40.config names as verilog_dont_merge_pragma_name.

`default_nettype none

module KanagawaHALNonMergeableRegisters
#(
    parameter WIDTH = 1,
    parameter DEPTH = 1,
    parameter logic [DEPTH-1:0][WIDTH-1:0] INIT_VAL = '0
)
(
    input  wire     clk,
    input  wire [DEPTH-1:0][WIDTH-1:0] data_in,
    output wire [DEPTH-1:0][WIDTH-1:0] data_out
);
    (* keep *) logic [DEPTH-1:0][WIDTH-1:0] data_ff = INIT_VAL;

    assign data_out = data_ff;

    always_ff @(posedge clk) data_ff <= data_in;
endmodule

module KanagawaHALNonMergeableRegistersWithClear
#(
    parameter WIDTH = 1,
    parameter DEPTH = 1,
    parameter logic [DEPTH-1:0][WIDTH-1:0] INIT_VAL = '0
)
(
    input  wire     clk,
    input  wire     clr,
    input  wire [DEPTH-1:0][WIDTH-1:0] data_in,
    output wire [DEPTH-1:0][WIDTH-1:0] data_out
);
    (* keep *) logic [DEPTH-1:0][WIDTH-1:0] data_ff = INIT_VAL;

    assign data_out = data_ff;

    always_ff @(posedge clk) data_ff <= clr ? '0 : data_in;
endmodule

module KanagawaHALRegisterWithInitialValue
#(
    parameter WIDTH = 1,
    parameter logic [WIDTH-1:0] INIT_VAL = '0
)
(
    input  wire     clk,
    input  wire [WIDTH-1:0] data_in,
    output wire [WIDTH-1:0] data_out
);
    logic [WIDTH-1:0] data_ff = INIT_VAL;

    assign data_out = data_ff;

    always_ff @(posedge clk) data_ff <= data_in;
endmodule

// Auto-pipelining is off on iCE40 (supports_auto_pipelining keeps its default,
// false), so the runtime instantiates these only through options that turn it
// on; each is then its fixed minimum, which is always a legal choice.
module KanagawaHALAutoPipelineChain
#(
    parameter integer WIDTH = 1,
    parameter integer MIN_DEPTH = 0,
    parameter integer MAX_DEPTH = 0,
    parameter AUTO_PIPELINE_GROUP = "UNUSED"
)
(
    input  wire                 clk,
    input  wire                 enable_in,
    input  wire  [WIDTH-1:0]    data_in,
    output logic                enable_out,
    output logic [WIDTH-1:0]    data_out
);
    generate
        if (MIN_DEPTH == 0) begin : gen_wire
            assign enable_out = enable_in;
            assign data_out = data_in;
        end
        else begin : gen_chain
            logic [MIN_DEPTH-1:0] enable_ff = '0;
            logic [MIN_DEPTH-1:0][WIDTH-1:0] data_ff;
            always_ff @(posedge clk) begin
                enable_ff <= (MIN_DEPTH == 1) ? enable_in : {enable_ff[MIN_DEPTH-2:0], enable_in};
                data_ff <= (MIN_DEPTH == 1) ? data_in : {data_ff[MIN_DEPTH-2:0], data_in};
            end
            assign enable_out = enable_ff[MIN_DEPTH-1];
            assign data_out = data_ff[MIN_DEPTH-1];
        end
    endgenerate
endmodule

`default_nettype wire
