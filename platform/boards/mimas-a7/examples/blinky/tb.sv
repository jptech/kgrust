// Self-checking testbench for the Mimas A7 pilot (top.sv + Blinky),
// runnable without Vivado: `kg board sim` in this directory simulates it
// with upstream's portable runtime RTL and its mock HAL.
//
// Checks that the LEDs mirror the DIP switches while the counter's top byte
// is zero, follow a change of the switches, that LED0 toggles once the
// counter passes 2^24 calls (one per cycle, ~0.17 s on the board), and that
// the reset button restarts the counter.

`timescale 1ns/1ps
`default_nettype none

module tb;
    // Set in the initial blocks, not by initialisers: Verilator 5.052 drops
    // the initialiser of a variable driving a netlist's input port
    // (platform-layer.md §8.5).
    logic       clk;
    logic       reset;
    logic [7:0] dip_sw;
    wire  [7:0] led;

    initial clk = 1'b0;
    always #5 clk = ~clk;   // 100 MHz

    mimas_a7_top dut
    (
        .CLK1   (clk),
        .RESET  (reset),
        .dip_sw (dip_sw),
        .LED    (led)
    );

    int errors = 0;

    task automatic expect_led(input logic [7:0] want, input string what);
        if (led !== want) begin
            $error("%s: LED = %h, want %h", what, led, want);
            errors++;
        end
    endtask

    initial begin
        reset = 1'b0;
        dip_sw = 8'h5a;

        // Power-on reset alone must bring the design up.
        repeat (200) @(posedge clk);
        expect_led(8'h5a, "after power-on");

        dip_sw = 8'hc3;
        repeat (20) @(posedge clk);
        expect_led(8'hc3, "after switch change");

        // Past 2^24 calls the counter's bit 24 is set: LED0 inverts.
        repeat (1 << 24) @(posedge clk);
        expect_led(8'hc2, "after 2^24 cycles");

        // The button resets the design: the counter restarts from zero, so
        // LED0 comes back.
        reset = 1'b1;
        repeat (5) @(posedge clk);
        reset = 1'b0;
        repeat (200) @(posedge clk);
        expect_led(8'hc3, "after button reset");

        if (errors == 0) $display("PASS: mimas_a7_top blinky");
        else             $display("FAIL: %0d check(s)", errors);
        $finish;
    end
endmodule

`default_nettype wire
