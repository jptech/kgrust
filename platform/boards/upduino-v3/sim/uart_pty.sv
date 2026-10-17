// Bridge testbench: the example's `upduino_core` with its serial pins on a
// pseudo-terminal (platform/sim/uart_pty.cpp), so a host program talks to
// the simulated design as to the board. Run by `kg board serve` in a project
// on this board; it runs until killed. The platform's UART modules
// (rtl/periph/uart.sv) frame the bytes; the examples' testbenches check that
// framing independently.
//
// The header buses have devices on them, so a design using the board's
// `spi` and `i2c` resources has something to talk to: a SPI flash and an
// I2C sensor at 0x48 (platform/sim/models, which kgbridge.SpiFlash and
// kgbridge.I2cTarget mirror for cocotb).

`timescale 1ns/1ps
`default_nettype none

module tb;
    import "DPI-C" function int uart_pty_open(input string path_file);
    import "DPI-C" function int uart_pty_getc();
    import "DPI-C" function void uart_pty_putc(input int c);

    // 24 MHz, 115200 baud; `kg board serve` sets it from the board's `uart`.
    parameter int CLKS_PER_BIT = 208;

    logic       clk;
    logic       rst;
    wire        core_txd;
    wire        core_rxd;
    wire  [2:0] rgb;

    initial clk = 1'b0;
    always #20.833 clk = ~clk;

    wire        spi_sclk, spi_mosi, spi_miso, spi_cs_n;
    // Open drain with pull-ups: a line is low while anyone pulls it.
    wire        core_scl_oe, core_sda_oe, sensor_scl_oe, sensor_sda_oe;
    wire        scl = !(core_scl_oe | sensor_scl_oe);
    wire        sda = !(core_sda_oe | sensor_sda_oe);

    upduino_core dut
    (
        .clk        (clk),
        .rst        (rst),
        .rgb        (rgb),
        .uart_txd   (core_txd),
        .uart_rxd   (core_rxd),
        .spi_sclk   (spi_sclk),
        .spi_mosi   (spi_mosi),
        .spi_miso   (spi_miso),
        .spi_cs_n   (spi_cs_n),
        .i2c_scl_oe (core_scl_oe),
        .i2c_scl_in (scl),
        .i2c_sda_oe (core_sda_oe),
        .i2c_sda_in (sda)
    );

    spi_flash_model flash
    (
        .sclk(spi_sclk), .mosi(spi_mosi), .cs_n(spi_cs_n), .miso(spi_miso), .errors()
    );

    i2c_target_model #(.ADDRESS(7'h48)) sensor
    (
        .clk(clk), .scl(scl), .sda(sda), .scl_oe(sensor_scl_oe), .sda_oe(sensor_sda_oe)
    );

    // Core to host.
    wire       from_core_valid;
    wire [7:0] from_core;

    uart_rx #(.CLKS_PER_BIT(CLKS_PER_BIT)) host_rx
    (
        .clk(clk), .rst(rst), .rxd(core_txd),
        .valid_out(from_core_valid), .data_out(from_core), .framing_error_out()
    );

    // Host to core: a byte from the pseudo-terminal whenever the line is idle.
    logic       to_core_valid;
    logic [7:0] to_core;
    wire        to_core_idle;

    uart_tx #(.CLKS_PER_BIT(CLKS_PER_BIT)) host_tx
    (
        .clk(clk), .rst(rst), .valid_in(to_core_valid), .data_in(to_core),
        .idle_out(to_core_idle), .txd(core_rxd)
    );

    string path_file;
    logic [2:0] last_rgb;

    initial begin
        rst = 1'b1;
        to_core_valid = 1'b0;
        if (!$value$plusargs("pty_path_file=%s", path_file)) path_file = "uart_pty.path";
        if (uart_pty_open(path_file) != 0) $fatal(1, "no pseudo-terminal");
        repeat (16) @(negedge clk);
        rst = 1'b0;
    end

    // Polled every 64 cycles, a fortieth of a byte at this baud rate.
    logic [5:0] poll;

    always @(posedge clk) begin
        if (from_core_valid) uart_pty_putc({24'd0, from_core});

        poll <= poll + 1;
        to_core_valid <= 1'b0;
        if (!rst && to_core_idle && !to_core_valid && poll == 0) begin
            int c;
            c = uart_pty_getc();
            if (c >= 0) begin
                to_core <= c[7:0];
                to_core_valid <= 1'b1;
            end
        end

        if (rgb !== last_rgb) begin
            $display("uart_pty: LED red=%b green=%b blue=%b", rgb[2], rgb[1], rgb[0]);
            last_rgb <= rgb;
        end
    end
endmodule

`default_nettype wire
