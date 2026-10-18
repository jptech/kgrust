// KanagawaHALDualPortRAM for iCE40: the interface of upstream's HAL
// (runtime/rtl/hal/mock/hal_dual_port_ram.sv), written for Yosys inference.
//
// Storage follows the backend's decision, never Yosys's: USE_LUTRAM is set
// for a memory the backend left out of block RAM (on iCE40 that "LUT" class
// is flip-flops; platform/library/lattice/ice40/config.k), and Yosys must
// not move it into an EBR the backend has budgeted elsewhere.
//
// No ECC (block_ram_supports_ecc = false): the status outputs report clean.

`default_nettype none

module KanagawaHALDualPortRAM
#(
    parameter DATA_WIDTH = 32,
    parameter ADDR_WIDTH = 6,
    parameter DEPTH = 2**ADDR_WIDTH,
    parameter USE_OUTPUT_REG = 0,
    parameter INITIAL_DATA_FILE = "UNUSED",
    parameter TRUE_DUAL_PORT = 0,       // if 0, port 0 only writes and port 1 only reads
    parameter USE_HARDENED_BYPASS = 0,
    parameter MAX_DEPTH = 0,
    parameter DEVICE_FAMILY = "ice40",
    parameter USE_LUTRAM = 0,
    parameter USE_BRAM = 0,
    parameter SUPPORTS_RW_COLLISIONS = 0,
    parameter RW_COLLISIONS_IMPOSSIBLE = 0,
    parameter ECC = 0
)
(
    input wire clk,

    input wire [1:0][ADDR_WIDTH-1:0] addr_in,

    input wire [1:0] wren_in,
    input wire [1:0][DATA_WIDTH-1:0] data_in,
    input wire inject_correctable_error_in,
    input wire inject_uncorrectable_error_in,

    input wire [1:0] rden_in,
    output logic [1:0][DATA_WIDTH-1:0] data_out,
    output logic error_detected_out,
    output logic data_valid_out
);
    logic [1:0][DATA_WIDTH-1:0] rddata;

    assign error_detected_out = 1'b0;
    assign data_valid_out = 1'b1;

// One body for both storage kinds; only the attribute differs.
`define KANAGAWA_ICE40_DPRAM_BODY \
        always_ff @(posedge clk) begin \
            if (wren_in[0]) \
                mem[addr_in[0]] <= data_in[0]; \
            else if ((TRUE_DUAL_PORT != 0) && wren_in[1]) \
                mem[addr_in[1]] <= data_in[1]; \
        end \
        always_ff @(posedge clk) begin \
            if (rden_in[1]) \
                rddata[1] <= mem[addr_in[1]]; \
        end \
        if (TRUE_DUAL_PORT != 0) begin : gen_port0_read \
            always_ff @(posedge clk) begin \
                if (rden_in[0]) \
                    rddata[0] <= mem[addr_in[0]]; \
            end \
        end \
        else begin : gen_port0_write_only \
            assign rddata[0] = '0; \
        end \
        if (INITIAL_DATA_FILE != "UNUSED" && INITIAL_DATA_FILE != "") begin : gen_init \
            initial $readmemh(INITIAL_DATA_FILE, mem); \
        end

    generate
        if (USE_LUTRAM != 0) begin : gen_logic
            (* ram_style = "logic" *) logic [DATA_WIDTH-1:0] mem [0:DEPTH-1];
            `KANAGAWA_ICE40_DPRAM_BODY
        end
        else begin : gen_block
            (* ram_style = "block" *) logic [DATA_WIDTH-1:0] mem [0:DEPTH-1];
            `KANAGAWA_ICE40_DPRAM_BODY
        end

        if (USE_OUTPUT_REG != 0) begin : gen_output_reg
            always_ff @(posedge clk) data_out <= rddata;
        end
        else begin : gen_no_output_reg
            assign data_out = rddata;
        end
    endgenerate

`undef KANAGAWA_ICE40_DPRAM_BODY

endmodule

`default_nettype wire
