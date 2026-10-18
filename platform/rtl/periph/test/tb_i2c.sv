// Simulation only: a self-checking testbench for i2c_master, against
// i2c_target_model at 0x48 on a bus with pull-ups. CLK_DIV and the target's
// STRETCH are parameters of `tb` (verilator -G), so one build runs one bus
// speed with one amount of clock stretching.
//
// The calls, queued back to back under backpressure: write the pointer
// 0x02 and the bytes 0xAB 0xCD; by repeated starts, set the pointer back to
// 0x02 and read two bytes, acknowledging the first and not the second;
// stop; read register 0x0B, never written, for its reset value; address
// the missing target 0x49 and see the NACK; stop; stop again, a no-op.
// Prints PASS, or ERROR lines and FAIL: N check(s).

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

// An independent watch on the bus, sampled every clock: SDA may change
// while SCL is high only as a START (falling) or a STOP (rising), and those
// only between bytes — after a multiple of nine clock pulses since the last
// START; SDA never changes in the same clock as SCL, which would leave the
// bit ambiguous; and SCL's high and low phases each last at least half the
// master's period, 2 * CLK_DIV, however a target stretches it. Counts the
// conditions in `starts` and `stops`, and keeps SCL's longest low phase
// within a transfer in `max_low`.
module i2c_bus_monitor
#(
    parameter int CLK_DIV = 60
)
(
    input wire clk,
    input wire rst,
    input wire scl,
    input wire sda
);
    int   errors = 0;
    int   starts = 0;
    int   stops = 0;
    int   pulses = 0;       // SCL falls since the last START, its own not counted
    int   cyc = 0, t_scl = 0;
    int   max_low = 0;
    logic scl_q = 1'b1, sda_q = 1'b1;

    task automatic fail(input string what);
        $display("ERROR i2c bus: %s at %0t", what, $time);
        errors++;
    endtask

    always @(posedge clk) begin
        cyc++;
        if (!rst) begin
            if (sda != sda_q && scl != scl_q) fail("SDA and SCL changed in the same clock");
            else if (sda != sda_q && scl) begin
                automatic string kind = sda ? "STOP" : "START";
                if (pulses % 9 != 0) fail($sformatf("%s after %0d clock pulses, not between bytes",
                                                    kind, pulses));
                if (sda) stops++;
                else starts++;
                pulses = sda ? 0 : -1;
            end
            if (scl != scl_q) begin
                if (cyc - t_scl < 2 * CLK_DIV)
                    fail($sformatf("SCL %s for %0d clocks", scl_q ? "high" : "low", cyc - t_scl));
                if (!scl) pulses++;
                else if (cyc - t_scl > max_low) max_low = cyc - t_scl;
                t_scl = cyc;
            end
        end
        scl_q = scl;
        sda_q = sda;
    end
endmodule

module tb
#(
    parameter int CLK_DIV = 60,
    parameter int STRETCH = 0
);
    localparam logic [1:0] START = 2'd0, STOP = 2'd1, WRITE = 2'd2, READ = 2'd3;
    localparam int N = 21;

    logic clk = 1'b0;
    logic rst = 1'b1;
    always #5 clk = !clk;

    logic       call_empty, call_rden, result_valid, result_rdy;
    logic [9:0] call_args;
    logic [8:0] result;
    logic       m_scl_oe, m_sda_oe, t_scl_oe, t_sda_oe;
    wire        scl = !(m_scl_oe | t_scl_oe);     // the pull-ups
    wire        sda = !(m_sda_oe | t_sda_oe);

    tb_call_driver #(.AW(10), .RW(9), .SEED(7), .NAME("i2c")) drv (
        .clk, .call_empty, .call_rden, .call_args, .result_valid, .result, .result_rdy);

    i2c_master #(.CLK_DIV(CLK_DIV)) dut (
        .clk, .rst, .call_empty, .call_rden,
        .call_op(call_args[9:8]), .call_data(call_args[7:0]),
        .result_valid, .result, .result_rdy,
        .scl_oe(m_scl_oe), .scl_in(scl), .sda_oe(m_sda_oe), .sda_in(sda));

    i2c_target_model #(.ADDRESS(7'h48), .STRETCH(STRETCH)) target (
        .clk, .scl, .sda, .scl_oe(t_scl_oe), .sda_oe(t_sda_oe));

    i2c_bus_monitor #(.CLK_DIV(CLK_DIV)) mon (.clk, .rst, .scl, .sda);

    logic [1:0] ops [N];
    logic [7:0] data [N];
    logic [8:0] want [N];
    int         n = 0;
    int         checks = 0;

    task automatic call(input logic [1:0] op, input logic [7:0] d, input logic [8:0] w);
        ops[n] = op;
        data[n] = d;
        want[n] = w;
        n++;
    endtask

    initial begin
        int total;
        // address 0x48 is 0x90 to write, 0x91 to read
        call(START, 8'h00, 9'h000);
        call(WRITE, 8'h90, 9'h000);
        call(WRITE, 8'h02, 9'h000);     // the pointer
        call(WRITE, 8'hAB, 9'h000);
        call(WRITE, 8'hCD, 9'h000);
        call(START, 8'h00, 9'h000);     // repeated
        call(WRITE, 8'h90, 9'h000);
        call(WRITE, 8'h02, 9'h000);
        call(START, 8'h00, 9'h000);     // repeated
        call(WRITE, 8'h91, 9'h000);
        call(READ,  8'h01, 9'h0AB);     // acknowledged: more to come
        call(READ,  8'h00, 9'h0CD);     // not: the last
        call(STOP,  8'h00, 9'h000);
        call(START, 8'h00, 9'h000);
        call(WRITE, 8'h90, 9'h000);
        call(WRITE, 8'h0B, 9'h000);
        call(START, 8'h00, 9'h000);     // repeated
        call(WRITE, 8'h91, 9'h000);
        call(READ,  8'h00, 9'h01B);     // the reset value, 8'h10 + 11
        call(START, 8'h00, 9'h000);     // repeated, to no one
        call(WRITE, 8'h92, 9'h100);     // 0x49: nobody acknowledges
        if (n != N) $fatal(1, "N is %0d, the calls %0d", N, n);

        repeat (4) @(posedge clk);
        rst = 1'b0;
        repeat (3) @(posedge clk);

        // The first five one at a time, the rest queued at once with the
        // results under backpressure and a spell of none taken at all.
        for (int i = 0; i < 5; i++) begin
            drv.push({ops[i], data[i]});
            drv.wait_results(i + 1, 1000 * CLK_DIV + 20 * STRETCH);
        end
        drv.stall_pct = 40;
        for (int i = 5; i < N; i++) drv.push({ops[i], data[i]});
        drv.push({STOP, 8'h00});
        drv.push({STOP, 8'h00});        // not ours any more: a no-op
        drv.stall_pct = 100;
        repeat (100 * CLK_DIV) @(posedge clk);
        drv.stall_pct = 40;
        drv.wait_results(N + 2, 1000 * CLK_DIV * N + 50 * STRETCH * N);
        repeat (10 * CLK_DIV) @(posedge clk);

        for (int i = 0; i < N + 2; i++) begin
            automatic logic [8:0] w = i < N ? want[i] : 9'h000;
            if (drv.got[i] !== w) begin
                $display("ERROR call %0d (op %0d, data %02h): got %03h, want %03h", i,
                         i < N ? ops[i] : STOP, i < N ? data[i] : 8'h00, drv.got[i], w);
                checks++;
            end
        end
        if (mon.starts != 6 || mon.stops != 2) begin
            $display("ERROR %0d START(s) and %0d STOP(s) on the bus, want 6 and 2", mon.starts, mon.stops);
            checks++;
        end
        // A stretch longer than the master's own low phase shows on the bus
        // only if the master waited for it.
        if (STRETCH > 2 * CLK_DIV && mon.max_low < STRETCH) begin
            $display("ERROR the longest SCL low phase is %0d clocks; the target stretches %0d",
                     mon.max_low, STRETCH);
            checks++;
        end
        if (!scl || !sda) begin
            $display("ERROR the bus is not released at the end");
            checks++;
        end
        if (target.regs[2] != 8'hAB || target.regs[3] != 8'hCD || target.regs[4] != 8'h14) begin
            $display("ERROR target registers 2..4: %02h %02h %02h", target.regs[2], target.regs[3],
                     target.regs[4]);
            checks++;
        end

        total = drv.errors + drv.timeouts + mon.errors + checks;
        if (total == 0) $display("PASS tb_i2c: CLK_DIV=%0d STRETCH=%0d", CLK_DIV, STRETCH);
        else $display("FAIL: %0d check(s)", total);
        $finish;
    end
endmodule

`default_nettype wire
