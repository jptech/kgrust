// Simulation only: the call side of a Kanagawa design, as the backend
// builds it for a callback, driving an engine.
//
// Calls queue in a show-ahead FIFO (`push`): `call_empty` is low while one
// is pending, `call_args` holds the oldest, and `call_rden` pops it at the
// clock edge. Results are taken on any cycle where `result_valid` is high;
// `result_rdy` is low on a random `stall_pct` percent of cycles, or always
// while `stall_pct` is 100, to exercise backpressure. Every result lands in
// `got[0 .. ngot-1]`, in order.
//
// It checks the protocol from the design's side: a pop of an empty FIFO,
// and a result offered while `result_rdy` is low, are ERRORs, counted in
// `errors`; `wait_results` counts its timeouts in `timeouts`.

`default_nettype none
// Simulation state given a starting value and then assigned.
/* verilator lint_off PROCASSINIT */

module tb_call_driver
#(
    parameter int AW = 9,           // the arguments, concatenated
    parameter int RW = 8,
    parameter int SEED = 1,
    parameter string NAME = "driver"
)
(
    input  wire           clk,
    output logic          call_empty,
    input  wire           call_rden,
    output logic [AW-1:0] call_args,
    input  wire           result_valid,
    input  wire  [RW-1:0] result,
    output logic          result_rdy
);
    logic [AW-1:0] fifo [256];
    logic [RW-1:0] got [256];
    int unsigned   head = 0;        // advanced by the engine's pops
    int unsigned   tail = 0;        // advanced by `push`
    int            ngot = 0;
    int            errors = 0;      // protocol violations
    int            timeouts = 0;    // results that never came
    int            stall_pct = 0;
    int unsigned   seed = SEED;

    assign call_empty = (head == tail);
    assign call_args = fifo[head[7:0]];

    initial result_rdy = 1'b1;

    always @(posedge clk) begin
        if (call_rden) begin
            if (head == tail) begin
                $display("ERROR %s: call_rden with no call pending at %0t", NAME, $time);
                errors <= errors + 1;
            end
            else head <= head + 1;
        end
        if (result_valid) begin
            if (!result_rdy) begin
                $display("ERROR %s: result_valid while result_rdy is low at %0t", NAME, $time);
                errors <= errors + 1;
            end
            got[ngot[7:0]] <= result;
            ngot <= ngot + 1;
        end
        // A linear congruential generator, so each driver's stalls are its own
        // and repeat from run to run.
        result_rdy <= int'((seed >> 16) % 100) >= stall_pct;
        seed <= seed * 1103515245 + 12345;
    end

    // Called away from the rising edge, so the FIFO's contents and `tail`
    // never change under the engine's sampling.
    task automatic push(input logic [AW-1:0] args);
        @(negedge clk);
        fifo[tail[7:0]] = args;
        tail = tail + 1;
    endtask

    task automatic wait_results(input int n, input int max_cycles);
        int waited = 0;
        while (ngot < n && waited < max_cycles) begin
            @(posedge clk);
            waited++;
        end
        if (ngot < n) begin
            $display("ERROR %s: %0d of %0d results after %0d cycles", NAME, ngot, n, max_cycles);
            timeouts = timeouts + 1;
        end
    endtask
endmodule

`default_nettype wire
