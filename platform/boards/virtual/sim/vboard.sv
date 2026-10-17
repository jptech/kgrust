// The virtual board: `virtual_core` with every resource on the host. Run by
// `kg board serve` for a project on this board, until killed.
//
// - The serial port on a pseudo-terminal (platform/sim/uart_pty.cpp, the
//   path exported as BOARD_PTY), and mirrored to the front panel's console,
//   whose typing reaches the design too.
// - The LEDs, buttons, switches and screen on the front panel, a page this
//   simulation serves (platform/sim/vboard.cpp; `kg board serve` prints its
//   address).
// - A SPI flash and an I2C sensor at 0x48 on the buses
//   (platform/sim/models).
//
// The screen is read off the VGA pins as a monitor would: the sync pulses
// find each line and frame, and the pixels between are 640x480's visible
// area, at 25 MHz, a clock a pixel.

`timescale 1ns/1ps
`default_nettype none

module tb;
    import "DPI-C" function int uart_pty_open(input string path_file);
    import "DPI-C" function int uart_pty_getc();
    import "DPI-C" function void uart_pty_putc(input int c);
    import "DPI-C" function int vb_open(input string page, input string port_file);
    import "DPI-C" function void vb_leds(input int which, input int value);
    import "DPI-C" function int vb_inputs(input int which);
    import "DPI-C" function void vb_uart_out(input int c);
    import "DPI-C" function int vb_uart_in();
    import "DPI-C" function void vb_pixel(input int x, input int y, input int rgb);
    import "DPI-C" function void vb_frame();
    import "DPI-C" function void vb_cycles(input int n);

    // 25 MHz, 115200 baud; `kg board serve` sets it from the board's `uart`.
    parameter int CLKS_PER_BIT = 217;

    logic        clk;
    logic        rst;
    wire  [2:0]  rgb;
    wire  [7:0]  leds;
    logic [3:0]  buttons;
    logic [7:0]  switches;
    wire         core_txd, core_rxd;
    wire         spi_sclk, spi_mosi, spi_miso, spi_cs_n;
    wire         core_scl_oe, core_sda_oe, sensor_scl_oe, sensor_sda_oe;
    wire         scl = !(core_scl_oe | sensor_scl_oe);
    wire         sda = !(core_sda_oe | sensor_sda_oe);
    wire         hsync, vsync;
    wire  [11:0] vga_rgb;

    initial clk = 1'b0;
    always #20 clk = ~clk;  // 25 MHz

    virtual_core dut
    (
        .clk        (clk),
        .rst        (rst),
        .rgb        (rgb),
        .leds       (leds),
        .buttons    (buttons),
        .switches   (switches),
        .uart_txd   (core_txd),
        .uart_rxd   (core_rxd),
        .spi_sclk   (spi_sclk),
        .spi_mosi   (spi_mosi),
        .spi_miso   (spi_miso),
        .spi_cs_n   (spi_cs_n),
        .i2c_scl_oe (core_scl_oe),
        .i2c_scl_in (scl),
        .i2c_sda_oe (core_sda_oe),
        .i2c_sda_in (sda),
        .vga_hsync  (hsync),
        .vga_vsync  (vsync),
        .vga_rgb    (vga_rgb)
    );

    spi_flash_model flash
    (
        .sclk(spi_sclk), .mosi(spi_mosi), .cs_n(spi_cs_n), .miso(spi_miso), .errors()
    );

    i2c_target_model #(.ADDRESS(7'h48)) sensor
    (
        .clk(clk), .scl(scl), .sda(sda), .scl_oe(sensor_scl_oe), .sda_oe(sensor_sda_oe)
    );

    // ---- Serial: the pseudo-terminal and the panel's console ----------

    wire       from_core_valid;
    wire [7:0] from_core;
    logic      to_core_valid;
    logic [7:0] to_core;
    wire       to_core_idle;

    uart_rx #(.CLKS_PER_BIT(CLKS_PER_BIT)) host_rx
    (
        .clk(clk), .rst(rst), .rxd(core_txd),
        .valid_out(from_core_valid), .data_out(from_core), .framing_error_out()
    );

    uart_tx #(.CLKS_PER_BIT(CLKS_PER_BIT)) host_tx
    (
        .clk(clk), .rst(rst), .valid_in(to_core_valid), .data_in(to_core),
        .idle_out(to_core_idle), .txd(core_rxd)
    );

    string path_file, page, port_file;

    initial begin
        rst = 1'b1;
        to_core_valid = 1'b0;
        buttons = '0;
        switches = '0;
        if (!$value$plusargs("pty_path_file=%s", path_file)) path_file = "uart_pty.path";
        if (!$value$plusargs("vboard_page=%s", page)) page = "vboard.html";
        if (!$value$plusargs("vboard_port_file=%s", port_file)) port_file = "vboard.port";
        if (uart_pty_open(path_file) != 0) $fatal(1, "no pseudo-terminal");
        if (vb_open(page, port_file) != 0) $fatal(1, "no front panel");
        repeat (16) @(negedge clk);
        rst = 1'b0;
    end

    // Polled every 64 cycles, a thirtieth of a byte at this rate; the
    // pseudo-terminal first, then the console.
    logic [5:0] poll;
    logic [9:0] count;
    logic [2:0] last_rgb;
    logic [7:0] last_leds;

    always @(posedge clk) begin
        if (from_core_valid) begin
            uart_pty_putc({24'd0, from_core});
            vb_uart_out({24'd0, from_core});
        end

        poll <= poll + 1;
        to_core_valid <= 1'b0;
        if (!rst && to_core_idle && !to_core_valid && poll == 0) begin
            int c;
            c = uart_pty_getc();
            if (c < 0) c = vb_uart_in();
            if (c >= 0) begin
                to_core <= c[7:0];
                to_core_valid <= 1'b1;
            end
        end

        // Inputs and the speed readout, every 1024 cycles (41 us).
        count <= count + 1;
        if (count == 0) begin
            int b, s;
            b = vb_inputs(0);
            s = vb_inputs(1);
            buttons <= b[3:0];
            switches <= s[7:0];
            vb_cycles(1024);
        end

        if (rgb !== last_rgb) begin
            vb_leds(0, {29'd0, rgb});
            last_rgb <= rgb;
        end
        if (leds !== last_leds) begin
            vb_leds(1, {24'd0, leds});
            last_leds <= leds;
        end
    end

    // ---- The screen, read off the VGA pins -----------------------------
    //
    // 640x480: a line is 96 clocks of sync, 48 of back porch, then 640
    // visible; a frame is 2 lines of sync, 33 of back porch, then 480.

    localparam int H_START = 96 + 48;
    localparam int V_START = 2 + 33;

    int  hx, vy;            // clocks since the line's sync began; lines since the frame's
    logic hsync_q = 1'b1, vsync_q = 1'b1;
    bit   frame_seen = 1'b0;

    always @(posedge clk) begin
        hsync_q <= hsync;
        vsync_q <= vsync;
        if (hsync_q && !hsync) begin
            hx <= 0;
            vy <= vy + 1;
        end
        else hx <= hx + 1;
        if (vsync_q && !vsync) begin
            vy <= 0;
            if (frame_seen) vb_frame();
            frame_seen <= 1'b1;
        end
        if (frame_seen && hx >= H_START && hx < H_START + 640 && vy >= V_START && vy < V_START + 480)
            vb_pixel(hx - H_START, vy - V_START, {20'd0, vga_rgb});
    end
endmodule

`default_nettype wire
