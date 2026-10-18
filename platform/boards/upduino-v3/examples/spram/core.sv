// UPduino v3 core for the SPRAM example, written by hand: the board can
// generate one from its [resources], and this one shows what a project
// supplies to replace it (it is named in kanagawa.toml's [board] rtl).
//
// SpramTest's `step` every cycle once the design is out of reset. `step` is
// fixed-latency 2 (spram.k): the LED takes each result as it arrives, two
// cycles after its call, and holds it until the next.

`default_nettype none

module upduino_core
(
    input  wire        clk,
    input  wire        rst,
    output logic [2:0] rgb,
    output wire        uart_txd,
    input  wire        uart_rxd,
    output wire        spi_sclk,
    output wire        spi_mosi,
    input  wire        spi_miso,
    output wire        spi_cs_n,
    output wire        i2c_scl_oe,
    input  wire        i2c_scl_in,
    output wire        i2c_sda_oe,
    input  wire        i2c_sda_in
);
    wire       startup_done;
    wire [2:0] step_result;
    logic [1:0] in_flight = '0;     // calls issued 1 and 2 cycles ago

    SpramTest test
    (
        .clk                        (clk),
        .rst                        (rst),
        .rst_and_startup_done_out   (startup_done),
        .step_valid_in              (startup_done),
        .step_result_out            (step_result),
        .stall_rate_supported_out   (),
        .stall_rate_valid_in        (1'b0),
        .stall_rate_in              (3'd0)
    );

    always_ff @(posedge clk) begin
        in_flight <= {in_flight[0], startup_done};
        if (rst)               rgb <= '0;
        else if (in_flight[1]) rgb <= step_result;
    end

    // Every resource this design leaves alone, idle.
    assign uart_txd = 1'b1;
    assign spi_sclk = 1'b0;
    assign spi_mosi = 1'b0;
    assign spi_cs_n = 1'b1;
    assign i2c_scl_oe = 1'b0;
    assign i2c_sda_oe = 1'b0;
endmodule

`default_nettype wire
