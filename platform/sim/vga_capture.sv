// A screen for a simulated board: reads a VGA output's pins as a monitor
// would and hands each visible pixel to the front panel
// (platform/sim/vboard.cpp), which shows each whole frame. Used by the
// `serve` testbench kg board generates for any board with a `vga` resource.
//
// The sync pulses find each line and frame: a line starts at hsync's
// falling edge, its visible part H_SYNC + H_BACK clocks later; a frame at
// vsync's, its visible part V_SYNC + V_BACK lines later. The defaults are
// 640x480 at 60 Hz, which crates/kg-board/src/wrapper.rs's VGA_H and VGA_V
// pass in, as they pass vga_timing.sv's. One pixel a clock.
//
// The capture costs nothing measurable: pong simulates at 3.89 MHz with it
// and 3.85 without (2026-10-17); the design is what takes the time.

`default_nettype none

module vga_capture
#(
    parameter int H_VISIBLE = 640,
    parameter int H_SYNC    = 96,
    parameter int H_BACK    = 48,
    parameter int V_VISIBLE = 480,
    parameter int V_SYNC    = 2,
    parameter int V_BACK    = 33,
    parameter int RGB_WIDTH = 12
)
(
    input wire                  clk,
    input wire                  hsync,
    input wire                  vsync,
    input wire [RGB_WIDTH-1:0]  rgb
);
    import "DPI-C" function void vb_pixel(input int x, input int y, input int rgb);
    import "DPI-C" function void vb_frame();

    localparam int H_START = H_SYNC + H_BACK;
    localparam int V_START = V_SYNC + V_BACK;

    int   hx = 0, vy = 0;   // clocks since the line's sync began; lines since the frame's
    logic hsync_q = 1'b1, vsync_q = 1'b1;
    bit   locked = 1'b0;    // a frame's start has been seen

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
            if (locked) vb_frame();
            locked <= 1'b1;
        end
        if (locked && hx >= H_START && hx < H_START + H_VISIBLE && vy >= V_START && vy < V_START + V_VISIBLE)
            vb_pixel(hx - H_START, vy - V_START, int'(rgb));
    end
endmodule

`default_nettype wire
