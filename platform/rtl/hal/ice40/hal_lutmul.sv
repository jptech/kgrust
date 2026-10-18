// KanagawaLutMul for iCE40: every multiplication a UP5K design makes.
// Interface: upstream's HAL (runtime/rtl/hal/xilinx/hal_dsp.sv).
//
// The device names no DSP intrinsic (lattice.ice40.config:
// integer_mul_src_width = 0), and with none the backend emits each `*` as
// this module, the `lutmul` op (compiler/cpp/ir.cpp:6551-6560), as it does
// the `lutmul` intrinsic itself. So it goes to the part's eight SB_MAC16
// blocks where it can; in logic, no UP5K design could reach them at all.
//
// - Both operands 16 bits or narrower: one SB_MAC16, instantiated, as a
//   combinational 16x16 multiplier (the backend registers the product
//   itself). Instantiated, and `kg board` synthesises without -dsp: Yosys
//   0.69's ice40_dsp crashes on a 16x16 product, inferred or instantiated,
//   followed by two register stages, which is the backend's own pipeline
//   (board-tooling.md §5).
// - Wider: `*`, in logic.
//
// A signed operand stays signed; with one signed and one not, the unsigned
// one gains a zero bit so the product is signed throughout. The product is
// taken modulo 2^DW_Z, as the backend's result width asks.

`default_nettype none

module KanagawaLutMul
#(
    parameter int unsigned  DW_X = 8,
    parameter int unsigned  DW_Y = 8,
    parameter bit           X_SIGNED = 1'b0,
    parameter bit           Y_SIGNED = 1'b0,
    parameter int unsigned  DW_Z = 16
)
(
    input wire  [DW_X-1:0]  x_in,
    input wire  [DW_Y-1:0]  y_in,
    output logic [DW_Z-1:0] z_out
);
    // Each operand as the block sees it: signed if either is, so an
    // unsigned one needs a spare bit then.
    localparam bit SIGNED = X_SIGNED | Y_SIGNED;
    localparam int unsigned WX = DW_X + ((SIGNED && !X_SIGNED) ? 1 : 0);
    localparam int unsigned WY = DW_Y + ((SIGNED && !Y_SIGNED) ? 1 : 0);
    localparam int unsigned P = DW_X + DW_Y + 2;

    generate
        if (WX <= 16 && WY <= 16) begin : gen_dsp
            wire [15:0] a = 16'($signed({X_SIGNED & x_in[DW_X-1], x_in}));
            wire [15:0] b = 16'($signed({Y_SIGNED & y_in[DW_Y-1], y_in}));
            wire [31:0] o;

            // A 16x16 multiplier with no registers: both halves of the
            // output select the 16x16 product (SB_MAC16's OUTPUT_SELECT 3).
            SB_MAC16
            #(
                .NEG_TRIGGER                (1'b0),
                .C_REG                      (1'b0),
                .A_REG                      (1'b0),
                .B_REG                      (1'b0),
                .D_REG                      (1'b0),
                .TOP_8x8_MULT_REG           (1'b0),
                .BOT_8x8_MULT_REG           (1'b0),
                .PIPELINE_16x16_MULT_REG1   (1'b0),
                .PIPELINE_16x16_MULT_REG2   (1'b0),
                .TOPOUTPUT_SELECT           (2'b11),
                .TOPADDSUB_LOWERINPUT       (2'b00),
                .TOPADDSUB_UPPERINPUT       (1'b0),
                .TOPADDSUB_CARRYSELECT      (2'b00),
                .BOTOUTPUT_SELECT           (2'b11),
                .BOTADDSUB_LOWERINPUT       (2'b00),
                .BOTADDSUB_UPPERINPUT       (1'b0),
                .BOTADDSUB_CARRYSELECT      (2'b00),
                .MODE_8x8                   (1'b0),
                .A_SIGNED                   (SIGNED),
                .B_SIGNED                   (SIGNED)
            ) mac
            (
                .CLK(1'b0), .CE(1'b1), .C(16'd0), .A(a), .B(b), .D(16'd0),
                .AHOLD(1'b0), .BHOLD(1'b0), .CHOLD(1'b0), .DHOLD(1'b0),
                .IRSTTOP(1'b0), .IRSTBOT(1'b0), .ORSTTOP(1'b0), .ORSTBOT(1'b0),
                .OLOADTOP(1'b0), .OLOADBOT(1'b0), .ADDSUBTOP(1'b0), .ADDSUBBOT(1'b0),
                .OHOLDTOP(1'b0), .OHOLDBOT(1'b0), .CI(1'b0), .ACCUMCI(1'b0), .SIGNEXTIN(1'b0),
                .O(o), .CO(), .ACCUMCO(), .SIGNEXTOUT()
            );

            // Beyond 32 bits a signed product only repeats its sign.
            assign z_out = DW_Z'($signed({SIGNED & o[31], o}));
        end
        else if (!SIGNED) begin : gen_unsigned
            wire [DW_X+DW_Y-1:0] product = x_in * y_in;
            assign z_out = DW_Z'(product);
        end
        else begin : gen_signed
            wire signed [DW_X:0] x = $signed({X_SIGNED & x_in[DW_X-1], x_in});
            wire signed [DW_Y:0] y = $signed({Y_SIGNED & y_in[DW_Y-1], y_in});
            wire signed [P-1:0]  product = x * y;
            assign z_out = DW_Z'(product);
        end
    endgenerate
endmodule

`default_nettype wire
