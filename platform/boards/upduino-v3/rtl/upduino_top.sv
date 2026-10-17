// Board shell for the tinyVision UPduino v3 (iCE40UP5K-SG48): the parts
// every design on the board shares, around an example's `upduino_core`.
//
// - Clock: the internal 48 MHz oscillator (SB_HFOSC) divided to CLK_MHZ
//   (48, 24, 12 or 6): no jumper or PLL needed. 24 MHz by default.
// - Reset: held for the first 16 cycles after configuration. The FPGA leaves
//   configuration with every flip-flop at its INIT value, so the counter needs
//   no reset of its own; the oscillator's start-up delay passes before its
//   first edge.
// - RGB LED: through SB_RGBA_DRV at its lowest current; rgb[2:0] is
//   {red, green, blue}, 1 lighting the colour (the pins themselves sink
//   current, active low).
// - An SPI bus and an I2C bus on header pins (constraints/upduino-v3.pcf),
//   for a design's `spi` and `i2c` resources; idle when no design uses
//   them. I2C is open drain through SB_IO, with the iCE40's weak pull-ups
//   on; a bus longer than a breadboard wants 4.7k resistors to 3.3 V.
// - The SPI flash's chip select (pin 16) left to the FT232H: an input with
//   the pull-up, never driven. In UART mode the FT232H drives that net as
//   DTR# (ADBUS4, the pin iceprog uses as chip select), so an FPGA driving
//   it would fight a terminal that asserts DTR. Keeping the flash
//   deselected is the host's part: hold DTR and RTS deasserted (host.py
//   does; constraints/upduino-v3.pcf has the trace).
//
// Port names match constraints/upduino-v3.pcf. A board with other pins but
// the same parts reuses this shell with its own constraints.

`default_nettype none

module upduino_top
#(
    parameter int CLK_MHZ = 24
)
(
    output wire led_red,
    output wire led_green,
    output wire led_blue,
    output wire serial_txd,
    input  wire serial_rxd,
    input  wire spi_cs,     // unused: see above
    output wire spi_sclk,
    output wire spi_mosi,
    input  wire spi_miso,
    output wire spi_cs_n,
    inout  wire i2c_scl,
    inout  wire i2c_sda
);
    localparam string HFOSC_DIV =
        (CLK_MHZ == 48) ? "0b00" :
        (CLK_MHZ == 24) ? "0b01" :
        (CLK_MHZ == 12) ? "0b10" : "0b11";

    wire clk;

    SB_HFOSC #(.CLKHF_DIV(HFOSC_DIV)) osc
    (
        .CLKHFPU    (1'b1),
        .CLKHFEN    (1'b1),
        .CLKHF      (clk)
    );

    logic [3:0] por_count = '0;
    wire        rst = ~&por_count;

    always_ff @(posedge clk) begin
        if (rst) por_count <= por_count + 4'd1;
    end

    wire [2:0] rgb;
    wire       scl_oe, scl_in, sda_oe, sda_in;

    upduino_core core
    (
        .clk        (clk),
        .rst        (rst),
        .rgb        (rgb),
        .uart_txd   (serial_txd),
        .uart_rxd   (serial_rxd),
        .spi_sclk   (spi_sclk),
        .spi_mosi   (spi_mosi),
        .spi_miso   (spi_miso),
        .spi_cs_n   (spi_cs_n),
        .i2c_scl_oe (scl_oe),
        .i2c_scl_in (scl_in),
        .i2c_sda_oe (sda_oe),
        .i2c_sda_in (sda_in)
    );

    // Open drain: driven low while `oe`, else released to the pull-up.
    // PIN_TYPE 1010_01: output enabled by OUTPUT_ENABLE, input unregistered.
    SB_IO #(.PIN_TYPE(6'b1010_01), .PULLUP(1'b1)) scl_io
    (
        .PACKAGE_PIN    (i2c_scl),
        .OUTPUT_ENABLE  (scl_oe),
        .D_OUT_0        (1'b0),
        .D_IN_0         (scl_in)
    );

    SB_IO #(.PIN_TYPE(6'b1010_01), .PULLUP(1'b1)) sda_io
    (
        .PACKAGE_PIN    (i2c_sda),
        .OUTPUT_ENABLE  (sda_oe),
        .D_OUT_0        (1'b0),
        .D_IN_0         (sda_in)
    );

    // RGB0 is green, RGB1 blue, RGB2 red on this board (tinyVision's
    // RTL/blink_led/rgb_blink.v).
    SB_RGBA_DRV
    #(
        .CURRENT_MODE   ("0b1"),        // half current
        .RGB0_CURRENT   ("0b000001"),
        .RGB1_CURRENT   ("0b000001"),
        .RGB2_CURRENT   ("0b000001")
    ) rgb_drv
    (
        .CURREN     (1'b1),
        .RGBLEDEN   (1'b1),
        .RGB0PWM    (rgb[1]),
        .RGB1PWM    (rgb[0]),
        .RGB2PWM    (rgb[2]),
        .RGB0       (led_green),
        .RGB1       (led_blue),
        .RGB2       (led_red)
    );
endmodule

`default_nettype wire
