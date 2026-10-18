// KanagawaHALSimpleDualPortRamTile for iCE40: one write port, one read port,
// the shape of SB_RAM40_4K. Interface: upstream's HAL
// (runtime/rtl/hal/mock/hal_simple_dual_port_ram_tile.sv).
//
// The tile carries no USE_LUTRAM from the backend; like upstream's mock it
// chooses by size. A tile of at most 64 entries goes to flip-flops, as the
// mock puts it in LUT-RAM; a larger one to block RAM.

`default_nettype none

module KanagawaHALSimpleDualPortRamTile
#(
    parameter integer WIDTH = 32,
    parameter integer DEPTH = 32,
    parameter integer MUX = 4,
    parameter integer USE_OUTPUT_REG = 1,
    parameter integer SUPPORTS_RW_COLLISIONS = 1,
    parameter integer RW_COLLISIONS_IMPOSSIBLE = 0,
    parameter DEVICE_FAMILY = "ice40",
    parameter integer ADDR_WIDTH = $clog2(DEPTH)
)
(
    input  wire                     clk,
    input  wire                     wr_en_in,
    input  wire [ADDR_WIDTH-1:0]    wr_addr_in,
    input  wire [WIDTH-1:0]         wr_data_in,
    input  wire                     rd_en_in,
    input  wire [ADDR_WIDTH-1:0]    rd_addr_in,
    output wire [WIDTH-1:0]         rd_data_out
);
    wire [1:0][WIDTH-1:0] data_out;
    assign rd_data_out = data_out[1];

    KanagawaHALDualPortRAM
    #(
        .DATA_WIDTH             (WIDTH),
        .ADDR_WIDTH             (ADDR_WIDTH),
        .DEPTH                  (DEPTH),
        .USE_OUTPUT_REG         (USE_OUTPUT_REG),
        .TRUE_DUAL_PORT         (0),
        .USE_LUTRAM             (DEPTH <= 64),
        .USE_BRAM               (DEPTH > 64),
        .SUPPORTS_RW_COLLISIONS (SUPPORTS_RW_COLLISIONS),
        .RW_COLLISIONS_IMPOSSIBLE (RW_COLLISIONS_IMPOSSIBLE)
    ) ram
    (
        .clk                            (clk),
        .addr_in                        ({rd_addr_in, wr_addr_in}),
        .wren_in                        ({1'b0, wr_en_in}),
        .data_in                        ({{WIDTH{1'b0}}, wr_data_in}),
        .inject_correctable_error_in    (1'b0),
        .inject_uncorrectable_error_in  (1'b0),
        .rden_in                        ({rd_en_in, 1'b0}),
        .data_out                       (data_out),
        .error_detected_out             (),
        .data_valid_out                 ()
    );
endmodule

`default_nettype wire
