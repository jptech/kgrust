// KanagawaIce40Spram: the SB_SPRAM256KA blocks behind `lattice.spram`
// (platform/library/device/ice40-up5k/lattice/spram.k), Words deep and Width
// wide in blocks of 16K x 16. The port list is the one the backend generates
// for that external class: `access`, fixed latency 1.
//
// SB_SPRAM256KA reads synchronously: the word addressed in one cycle is on
// DATAOUT the next, and DATAOUT holds while the block is not selected, so a
// read result is the selected row's output, chosen by the bank registered
// with the request.

`default_nettype none

module KanagawaIce40Spram
#(
    parameter Words = 16384,
    parameter Width = 16
)
(
    input  wire                         clk,
    input  wire                         rst,

    input  wire                         access_valid_in,
    input  wire [$clog2(Words)-1:0]     access_addr_in,
    input  wire                         access_write_in,
    input  wire [Width-1:0]             access_data_in,
    input  wire [Width/4-1:0]           access_nibble_mask_in,
    output logic [Width-1:0]            access_result_out
);
    localparam int DEEP = Words / 16384;
    localparam int WIDE = Width / 16;

    logic [DEEP-1:0][Width-1:0] row_out;

    genvar d, w;
    generate
        for (d = 0; d < DEEP; d++) begin : gen_row
            wire selected;
            if (DEEP == 1) begin : gen_one
                assign selected = access_valid_in;
            end
            else begin : gen_bank
                assign selected = access_valid_in & (access_addr_in[$clog2(Words)-1:14] == d);
            end

            for (w = 0; w < WIDE; w++) begin : gen_block
                SB_SPRAM256KA spram
                (
                    .ADDRESS    (access_addr_in[13:0]),
                    .DATAIN     (access_data_in[w*16 +: 16]),
                    .MASKWREN   (access_nibble_mask_in[w*4 +: 4]),
                    .WREN       (access_write_in),
                    .CHIPSELECT (selected),
                    .CLOCK      (clk),
                    .STANDBY    (1'b0),
                    .SLEEP      (1'b0),
                    .POWEROFF   (1'b1),     // active low: powered
                    .DATAOUT    (row_out[d][w*16 +: 16])
                );
            end
        end

        if (DEEP == 1) begin : gen_one_row
            assign access_result_out = row_out[0];
        end
        else begin : gen_rows
            logic [$clog2(DEEP)-1:0] bank_ff;
            always_ff @(posedge clk) begin
                if (access_valid_in) bank_ff <= access_addr_in[$clog2(Words)-1:14];
            end
            assign access_result_out = row_out[bank_ff];
        end
    endgenerate
endmodule

`default_nettype wire
