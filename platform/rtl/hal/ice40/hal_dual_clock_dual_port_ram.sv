// KanagawaHALDualClockDualPortRAM for iCE40: read latency 2, as upstream's
// Xilinx (READ_LATENCY_B(2)) and mock implementations have it. SB_RAM40_4K
// has independent read and write clocks.

`default_nettype none

module KanagawaHALDualClockDualPortRAM
#(
    parameter DATA_WIDTH = 32,
    parameter ADDR_WIDTH = 9,
    parameter DEVICE_FAMILY = "ice40"
)
(
    input wire rdclk,
    input wire [ADDR_WIDTH-1:0] readaddr_in,
    output logic [DATA_WIDTH-1:0] data_out,

    input wire wrclk,
    input wire wren_in,
    input wire [ADDR_WIDTH-1:0] writeaddr_in,
    input wire [DATA_WIDTH-1:0] data_in
);
    logic [DATA_WIDTH-1:0] mem [0:(2**ADDR_WIDTH)-1];
    logic [DATA_WIDTH-1:0] rddata;

    always_ff @(posedge wrclk) begin
        if (wren_in) mem[writeaddr_in] <= data_in;
    end

    always_ff @(posedge rdclk) begin
        rddata <= mem[readaddr_in];
        data_out <= rddata;
    end
endmodule

`default_nettype wire
