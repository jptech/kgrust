// Self-checking testbench for the UPduino blinky core (core.sv + Blinky),
// run by `kg board sim` in this directory against the iCE40 HAL, and
// by `netsim` against the synthesised netlist.
//
// The LED shows bits 25:23 of a counter that counts the calls of `step`, one
// per cycle from the first cycle after start-up: dark for the first 2^23
// calls, then blue, then green.

`timescale 1ns/1ps
`default_nettype none

module tb;
    // Set in the initial block, not by initialisers: Verilator 5.052 drops
    // the initialiser of a variable driving a netlist's input port, and the
    // design would never see reset (platform-layer.md §8.5).
    logic       clk;
    logic       rst;
    wire  [2:0] rgb;

    initial clk = 1'b0;
    always #20.833 clk = ~clk;  // 24 MHz

    upduino_core dut
    (
        .clk        (clk),
        .rst        (rst),
        .rgb        (rgb),
        .uart_txd   (),
        .uart_rxd   (1'b1),
        // The header buses, idle here: lines pulled up, nothing on them.
        .spi_sclk   (),
        .spi_mosi   (),
        .spi_miso   (1'b0),
        .spi_cs_n   (),
        .i2c_scl_oe (),
        .i2c_scl_in (1'b1),
        .i2c_sda_oe (),
        .i2c_sda_in (1'b1)
    );

    int errors = 0;

    task automatic expect_rgb(input logic [2:0] want, input string what);
        if (rgb !== want) begin
            $display("ERROR %s: rgb = %b, want %b", what, rgb, want);
            errors++;
        end
    endtask

    initial begin
        rst = 1'b1;
        repeat (16) @(negedge clk);
        rst = 1'b0;

        repeat (1000) @(negedge clk);
        expect_rgb(3'b000, "early");

        // Calls start a few cycles after reset: by 2^23 + 1000 cycles the
        // counter has passed 2^23, and is short of 2^24.
        repeat (1 << 23) @(negedge clk);
        expect_rgb(3'b001, "after 2^23 cycles");

        repeat (1 << 23) @(negedge clk);
        expect_rgb(3'b010, "after 2^24 cycles");

        if (errors == 0) $display("PASS: upduino blinky");
        else             $display("FAIL: %0d check(s)", errors);
        $finish;
    end
endmodule

`default_nettype wire
