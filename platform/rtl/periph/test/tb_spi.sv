// Simulation only: a self-checking testbench for spi_master.
//
// - Against spi_flash_model (mode 0), at CLK_DIV 2 and 1: the JEDEC ID as
//   four calls holding chip select over the first three, a status read,
//   and an 8-byte READ from 0x000010, queued back to back, under
//   backpressure.
// - In all four modes at CLK_DIV 1 and 3, MOSI looped back to MISO: random
//   bytes and holds, each result the byte sent.
//
// Every engine's pins are watched by spi_pin_monitor. Prints PASS, or
// ERROR lines and FAIL: N check(s).

`timescale 1ns / 1ps
`default_nettype none
// A testbench: several modules to a file; blocking assignments in clocked
// processes, which run in order like the procedural checks they are;
// variables given a starting value and then assigned; and counters read
// only through hierarchical references, which the lint does not see.
/* verilator lint_off DECLFILENAME */
/* verilator lint_off BLKSEQ */
/* verilator lint_off PROCASSINIT */
/* verilator lint_off UNUSEDSIGNAL */

// The pin-level contract, for any mode: SCLK idles at CPOL and has no edge
// while chip select is high; chip select falls at least a half-period
// before the first edge, rises at least a half-period after the last, with
// SCLK at CPOL, and stays high at least a half-period; edges are at least a
// half-period apart, sixteen to a byte; MOSI changes only where the mode
// lets the target expect it — while SCLK is at CPOL for CPHA = 0, with a
// leading edge for CPHA = 1. Counts whole transactions in `frames`, with
// the edges of the last in `last_edges`.
module spi_pin_monitor
#(
    parameter bit CPOL = 1'b0,
    parameter bit CPHA = 1'b0,
    parameter int CLK_DIV = 2,
    parameter string NAME = "spi"
)
(
    input wire clk,
    input wire rst,
    input wire sclk,
    input wire mosi,
    input wire cs_n
);
    int   errors = 0;
    int   frames = 0;
    int   last_edges = 0;
    int   edges = 0;
    int   cyc = 0, t_edge = 0, t_cs = 0;
    logic sclk_q = CPOL, mosi_q = 1'b0, cs_q = 1'b1;

    task automatic fail(input string what);
        $display("ERROR %s: %s at %0t", NAME, what, $time);
        errors++;
    endtask

    always @(posedge clk) begin
        cyc++;
        if (!rst) begin
            if (cs_n && sclk != CPOL) fail("SCLK not at CPOL while deselected");
            if (sclk != sclk_q) begin
                if (cs_n || cs_q) fail("SCLK edge while chip select high");
                if (edges == 0 && cyc - t_cs < CLK_DIV) fail("first SCLK edge too soon after chip select");
                if (edges != 0 && cyc - t_edge < CLK_DIV) fail("SCLK edges closer than a half-period");
                edges++;
                t_edge = cyc;
            end
            if (mosi != mosi_q && !cs_n) begin
                if (!CPHA && sclk != CPOL) fail("MOSI changed with SCLK away from CPOL (CPHA = 0)");
                if (CPHA && !(sclk != sclk_q && sclk != CPOL)) fail("MOSI changed off a leading edge (CPHA = 1)");
            end
            if (cs_n != cs_q) begin
                if (cyc - t_cs < CLK_DIV) fail("chip select changed again within a half-period");
                if (cs_n) begin
                    if (cyc - t_edge < CLK_DIV) fail("chip select rose too soon after the last SCLK edge");
                    if (edges % 16 != 0) fail($sformatf("transaction of %0d SCLK edges", edges));
                    frames++;
                    last_edges = edges;
                end
                edges = 0;
                t_cs = cyc;
            end
        end
        sclk_q = sclk;
        mosi_q = mosi;
        cs_q = cs_n;
    end
endmodule

// The engine against the flash model.
module spi_flash_test
#(
    parameter int CLK_DIV = 2,
    parameter int SEED = 1
)
(
    input  wire clk,
    input  wire rst,
    output int  errors,
    output bit  done
);
    localparam string NAME = $sformatf("flash CLK_DIV=%0d", CLK_DIV);

    logic       call_empty, call_rden, result_valid, result_rdy;
    logic [8:0] call_args;
    logic [7:0] result;
    logic       sclk, mosi, miso, cs_n;
    int         model_errors;
    int         checks = 0;

    tb_call_driver #(.AW(9), .RW(8), .SEED(SEED), .NAME(NAME)) drv (
        .clk, .call_empty, .call_rden, .call_args, .result_valid, .result, .result_rdy);

    spi_master #(.CLK_DIV(CLK_DIV)) dut (
        .clk, .rst, .call_empty, .call_rden,
        .call_mosi(call_args[8:1]), .call_hold(call_args[0]),
        .result_valid, .result, .result_rdy, .sclk, .mosi, .miso, .cs_n);

    spi_flash_model #(.DEPTH(256)) flash (.sclk, .mosi, .cs_n, .miso, .errors(model_errors));

    spi_pin_monitor #(.CPOL(1'b0), .CPHA(1'b0), .CLK_DIV(CLK_DIV), .NAME(NAME)) mon (
        .clk, .rst, .sclk, .mosi, .cs_n);

    assign errors = drv.errors + drv.timeouts + model_errors + mon.errors + checks;

    task automatic expect_byte(input int i, input logic [7:0] want, input string what);
        if (drv.got[i] !== want) begin
            $display("ERROR %s: %s: got %02h, want %02h", NAME, what, drv.got[i], want);
            checks++;
        end
    endtask

    task automatic call(input logic [7:0] b, input bit hold);
        drv.push({b, hold});
    endtask

    initial begin
        done = 0;
        @(negedge rst);

        // The JEDEC ID, one call at a time, each result taken at once.
        call(8'h9F, 1);
        drv.wait_results(1, 1000);
        call(8'h00, 1);
        drv.wait_results(2, 1000);
        call(8'h00, 1);
        drv.wait_results(3, 1000);
        if (cs_n) begin
            $display("ERROR %s: chip select released while held", NAME);
            checks++;
        end
        call(8'h00, 0);
        drv.wait_results(4, 1000);
        expect_byte(1, 8'hEF, "JEDEC manufacturer");
        expect_byte(2, 8'h40, "JEDEC type");
        expect_byte(3, 8'h16, "JEDEC capacity");
        if (!cs_n) begin
            $display("ERROR %s: chip select still low after the last byte", NAME);
            checks++;
        end
        if (mon.frames != 1 || mon.last_edges != 4 * 16) begin
            $display("ERROR %s: JEDEC ID as %0d transaction(s), the last %0d edges", NAME,
                     mon.frames, mon.last_edges);
            checks++;
        end

        // A status read and the 8-byte READ, all queued at once, results
        // under heavy backpressure, with a spell of none taken at all.
        drv.stall_pct = 60;
        call(8'h05, 1);
        call(8'h00, 0);
        call(8'h03, 1);
        call(8'h00, 1);
        call(8'h00, 1);
        call(8'h10, 1);
        for (int i = 0; i < 8; i++) call(8'h00, i != 7);
        drv.stall_pct = 100;
        repeat (40 * CLK_DIV) @(posedge clk);
        drv.stall_pct = 60;
        drv.wait_results(4 + 2 + 12, 10000);
        expect_byte(5, 8'h00, "status");
        for (int i = 0; i < 8; i++)
            expect_byte(10 + i, 8'(8'h10 + i) ^ 8'hA5, $sformatf("READ byte %0d", i));
        if (mon.frames != 3 || mon.last_edges != 12 * 16) begin
            $display("ERROR %s: %0d transaction(s), the last %0d edges", NAME, mon.frames, mon.last_edges);
            checks++;
        end
        if (drv.ngot != 18) begin
            $display("ERROR %s: %0d results, want 18", NAME, drv.ngot);
            checks++;
        end
        done = 1;
    end
endmodule

// The engine in one mode, MOSI looped back to MISO.
module spi_loop_test
#(
    parameter bit CPOL = 1'b0,
    parameter bit CPHA = 1'b0,
    parameter int CLK_DIV = 1,
    parameter int SEED = 1
)
(
    input  wire clk,
    input  wire rst,
    output int  errors,
    output bit  done
);
    localparam string NAME = $sformatf("loopback mode %0d CLK_DIV=%0d", {CPOL, CPHA}, CLK_DIV);
    localparam int N = 40;

    logic       call_empty, call_rden, result_valid, result_rdy;
    logic [8:0] call_args;
    logic [7:0] result;
    logic       sclk, mosi, cs_n;
    logic [7:0] sent [N];
    int         checks = 0;
    int         releases = 0;
    int unsigned rng = SEED;

    tb_call_driver #(.AW(9), .RW(8), .SEED(SEED), .NAME(NAME)) drv (
        .clk, .call_empty, .call_rden, .call_args, .result_valid, .result, .result_rdy);

    spi_master #(.CLK_DIV(CLK_DIV), .CPOL(CPOL), .CPHA(CPHA)) dut (
        .clk, .rst, .call_empty, .call_rden,
        .call_mosi(call_args[8:1]), .call_hold(call_args[0]),
        .result_valid, .result, .result_rdy, .sclk, .mosi, .miso(mosi), .cs_n);

    spi_pin_monitor #(.CPOL(CPOL), .CPHA(CPHA), .CLK_DIV(CLK_DIV), .NAME(NAME)) mon (
        .clk, .rst, .sclk, .mosi, .cs_n);

    assign errors = drv.errors + drv.timeouts + mon.errors + checks;

    function automatic int unsigned next();
        rng = rng * 1103515245 + 12345;
        return rng >> 8;
    endfunction

    initial begin
        bit hold;
        done = 0;
        @(negedge rst);
        for (int i = 0; i < N; i++) begin
            // The first half one call at a time with gaps, the second queued
            // in bursts; the stalls change as it goes.
            if (i == N / 2) drv.stall_pct = 50;
            sent[i] = 8'(next());
            hold = (i != N - 1) && (next() % 3 != 0);
            if (!hold) releases++;
            drv.push({sent[i], hold});
            if (i < N / 2) begin
                drv.wait_results(i + 1, 1000);
                repeat (next() % 4) @(posedge clk);
            end
        end
        drv.wait_results(N, 100 * N * CLK_DIV);
        for (int i = 0; i < N; i++)
            if (drv.got[i] !== sent[i]) begin
                $display("ERROR %s: call %0d: got %02h, want %02h", NAME, i, drv.got[i], sent[i]);
                checks++;
            end
        if (mon.frames != releases) begin
            $display("ERROR %s: %0d transactions, want %0d", NAME, mon.frames, releases);
            checks++;
        end
        done = 1;
    end
endmodule

module tb;
    logic clk = 1'b0;
    logic rst = 1'b1;
    always #5 clk = !clk;

    int  err [10];
    bit  fin [10];

    spi_flash_test #(.CLK_DIV(2), .SEED(11)) flash2 (.clk, .rst, .errors(err[0]), .done(fin[0]));
    spi_flash_test #(.CLK_DIV(1), .SEED(12)) flash1 (.clk, .rst, .errors(err[1]), .done(fin[1]));

    for (genvar m = 0; m < 4; m++) begin : g_mode
        for (genvar d = 0; d < 2; d++) begin : g_div
            spi_loop_test #(.CPOL(m[1]), .CPHA(m[0]), .CLK_DIV(d == 0 ? 1 : 3), .SEED(100 + 2 * m + d))
                loop (.clk, .rst, .errors(err[2 + 2 * m + d]), .done(fin[2 + 2 * m + d]));
        end
    end

    initial begin
        int total;
        bit all;
        repeat (4) @(posedge clk);
        rst = 1'b0;
        for (int t = 0; t < 200000; t++) begin
            @(posedge clk);
            all = 1;
            foreach (fin[i]) all &= fin[i];
            if (all) break;
        end
        repeat (20) @(posedge clk);         // let the monitors see the last edges
        total = 0;
        foreach (err[i]) total += err[i];
        foreach (fin[i])
            if (!fin[i]) begin
                $display("ERROR test %0d did not finish", i);
                total++;
            end
        if (total == 0) $display("PASS tb_spi: flash at CLK_DIV 2 and 1; loopback in modes 0-3 at CLK_DIV 1 and 3");
        else $display("FAIL: %0d check(s)", total);
        $finish;
    end
endmodule

`default_nettype wire
