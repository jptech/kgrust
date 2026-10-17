// Self-checking testbench for the UPduino SPRAM core (core.sv + SpramTest +
// lattice.spram), run by `kg board sim` in this directory with
// Yosys's SB_SPRAM256KA model, and by `netsim` against the netlist.
//
// A pass is 65,536 calls, one per cycle: a write sweep and a read sweep of
// 32K words. The LED must turn green with the first result and stay so,
// never red; blue must come on at pass 128 (bit 7 of the pass count). Then
// every stored word is inverted behind the design's back, and the LED must
// turn red within two passes: the check itself is under test.

`timescale 1ns/1ps
`default_nettype none

// The SPRAM blocks' models: the RTL's hierarchy, or the netlist's flattened
// names (`netsim` defines NETSIM).
`ifdef NETSIM
`define SPRAM_ROW0 dut.\test._ram__banks__KanagawaIce40Spram_inst.gen_row[0].gen_block[0].spram
`define SPRAM_ROW1 dut.\test._ram__banks__KanagawaIce40Spram_inst.gen_row[1].gen_block[0].spram
`else
`define SPRAM_ROW0 dut.test._ram__banks__KanagawaIce40Spram_inst.gen_row[0].gen_block[0].spram
`define SPRAM_ROW1 dut.test._ram__banks__KanagawaIce40Spram_inst.gen_row[1].gen_block[0].spram
`endif

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

    localparam int PASS = 65536;

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

        repeat (100) @(negedge clk);
        expect_rgb(3'b010, "first results");

        // Three passes: every word written and read back three times, with
        // three different patterns. A mismatch latches red.
        repeat (3 * PASS) @(negedge clk);
        expect_rgb(3'b010, "after 3 passes");

        // Bit 7 of the pass count: blue from pass 128.
        repeat (125 * PASS + 1000) @(negedge clk);
        expect_rgb(3'b011, "after 128 passes");

        // Wherever the sweep is, some word it has written this pass and not
        // yet read back is now wrong (or, early in a write sweep, every word
        // of the last pass is, ahead of the read sweep).
        for (int i = 0; i < 16384; i++) begin
            `SPRAM_ROW0 .mem[i] = ~`SPRAM_ROW0 .mem[i];
            `SPRAM_ROW1 .mem[i] = ~`SPRAM_ROW1 .mem[i];
        end
        repeat (2 * PASS) @(negedge clk);
        if (rgb[2] !== 1'b1 || rgb[1] !== 1'b0) begin
            $display("ERROR after corrupting the SPRAM: rgb = %b, want red and not green", rgb);
            errors++;
        end

        if (errors == 0) $display("PASS: upduino spram");
        else             $display("FAIL: %0d check(s)", errors);
        $finish;
    end
endmodule

`default_nettype wire
