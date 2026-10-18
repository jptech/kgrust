// VGA timing: the pixel position, blanking and sync pulses for one mode,
// 640x480 at 60 Hz by default (a 25.175 MHz pixel clock; 25 MHz is within
// what monitors accept). A generated core asks the design for each visible
// pixel's colour at (x_out, y_out), and delays the sync by the design's
// latency to match.
//
// Sync pulses are active low, as 640x480's are. frame_out pulses once a
// frame, in the first cycle after the visible area, when a design can
// update what the next frame shows.

`default_nettype none

module vga_timing
#(
    parameter int H_VISIBLE = 640,
    parameter int H_FRONT   = 16,
    parameter int H_SYNC    = 96,
    parameter int H_BACK    = 48,
    parameter int V_VISIBLE = 480,
    parameter int V_FRONT   = 10,
    parameter int V_SYNC    = 2,
    parameter int V_BACK    = 33
)
(
    input  wire       clk,
    input  wire       rst,
    output logic [9:0] x_out,
    output logic [9:0] y_out,
    output wire       active_out,
    output wire       hsync_out,
    output wire       vsync_out,
    output wire       frame_out
);
    localparam int H_TOTAL = H_VISIBLE + H_FRONT + H_SYNC + H_BACK;
    localparam int V_TOTAL = V_VISIBLE + V_FRONT + V_SYNC + V_BACK;

    always_ff @(posedge clk) begin
        if (rst) begin
            x_out <= '0;
            y_out <= '0;
        end
        else if (x_out == 10'(H_TOTAL - 1)) begin
            x_out <= '0;
            y_out <= (y_out == 10'(V_TOTAL - 1)) ? '0 : y_out + 10'd1;
        end
        else x_out <= x_out + 10'd1;
    end

    assign active_out = (x_out < 10'(H_VISIBLE)) && (y_out < 10'(V_VISIBLE));
    assign hsync_out = !((x_out >= 10'(H_VISIBLE + H_FRONT)) && (x_out < 10'(H_VISIBLE + H_FRONT + H_SYNC)));
    assign vsync_out = !((y_out >= 10'(V_VISIBLE + V_FRONT)) && (y_out < 10'(V_VISIBLE + V_FRONT + V_SYNC)));
    assign frame_out = (x_out == '0) && (y_out == 10'(V_VISIBLE));
endmodule

`default_nettype wire
