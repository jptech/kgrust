// An I2C master behind a Kanagawa callback `(uint2 op, uint8 data) ->
// uint9 _i2c`, one bus operation per call:
//
//   op 0  START: a start condition, or a repeated start if the bus is
//         already ours. Returns 0.
//   op 1  STOP: a stop condition, releasing the bus. Returns 0; a no-op
//         when the bus is not ours, which a stop would otherwise turn into
//         a start.
//   op 2  WRITE `data`, most significant bit first, then read the target's
//         acknowledge: result[8] = 1 if it did not acknowledge (NACK);
//         result[7:0] = 0.
//   op 3  READ a byte and acknowledge it if data[0] = 1 (more to come), or
//         not if 0 (the last). result[7:0] = the byte; result[8] = 0.
//
// Both lines are open drain: `*_oe` = 1 pulls the line low, 0 releases it
// to the pull-up. The lines are read back through two flops each.
//
// Timing is in quarters of the SCL period, CLK_DIV clocks each (24 MHz /
// (4 * 60) = 100 kHz); CLK_DIV >= 2, so that SCL has been low longer than
// the synchroniser's delay before we look for it high. A bit is four
// quarters: SDA changes a quarter after SCL falls, SCL is released a
// quarter later, held high for two, then pulled low. START and STOP are
// six and five quarters: SDA released (START) or pulled low (STOP) with SCL
// low, SCL released and high for two quarters, SDA flips — the condition —
// and two quarters pass; a START then pulls SCL low for a quarter. Two
// quarters cover the standard-mode set-up and hold times of START and STOP
// at 100 kHz (4.7 us, 4.0 us), and a STOP's last two the bus-free time.
// Fast mode's 1.3 us low phase needs 2 * CLK_DIV clocks to span it: at
// 24 MHz, CLK_DIV = 16 (375 kHz), not 15.
// Whenever SCL is released, the quarter's count starts only once the line
// reads high, so a target stretching the clock holds us there.
//
// A WRITE or READ shifts nine bits through one register: the eight data
// bits and the acknowledge bit go out of the top while what the line
// carried is sampled into the bottom, at the middle of each high phase. So
// after a WRITE, bit 0 is the target's acknowledge; after a READ, bits 8..1
// are its byte. A call is popped (`call_rden`) in the cycle it starts, and
// its result is offered, gated by `result_rdy`, until taken; between calls
// the bus is left as the last one left it, SCL held low while the bus is
// ours. A single master: there is no arbitration, and a WRITE or READ with
// no START before it goes out regardless.

`default_nettype none

module i2c_master
#(
    parameter int CLK_DIV = 60
)
(
    input  wire        clk,
    input  wire        rst,

    input  wire        call_empty,
    output logic       call_rden,
    input  wire  [1:0] call_op,
    input  wire  [7:0] call_data,
    output wire        result_valid,
    output logic [8:0] result,
    input  wire        result_rdy,

    output logic       scl_oe,
    input  wire        scl_in,
    output logic       sda_oe,
    input  wire        sda_in
);
    localparam logic [1:0] OP_START = 2'd0, OP_STOP = 2'd1, OP_WRITE = 2'd2, OP_READ = 2'd3;
    localparam int DW = CLK_DIV > 1 ? $clog2(CLK_DIV) : 1;
    localparam logic [DW-1:0] RELOAD = DW'(CLK_DIV - 1);

    // No starting value: whatever they hold at power-up is flushed by a
    // reset of two clocks or more.
    (* ASYNC_REG = "TRUE" *) logic [1:0] scl_sync;
    (* ASYNC_REG = "TRUE" *) logic [1:0] sda_sync;
    wire scl = scl_sync[1];
    wire sda = sda_sync[1];

    typedef enum logic [1:0] { IDLE, BUSY, DONE } state_t;
    state_t        state;
    logic [1:0]    op;
    logic          owned;   // a START has gone out and no STOP since
    logic [2:0]    quarter;
    logic [3:0]    nbit;    // the bit of a WRITE or READ on the line, 0..8
    logic [8:0]    shift;
    logic [DW-1:0] count;   // clocks left in this quarter, less one

    wire byte_op = op[1];
    wire start = !call_empty && (state == IDLE || (state == DONE && result_rdy));
    // Every sequence releases SCL at the start of quarter 1.
    wire stretched = (quarter == 3'd1) && !scl;

    assign call_rden = start;
    assign result_valid = (state == DONE) && result_rdy;
    assign result = op == OP_WRITE ? {shift[0], 8'h00}
                  : op == OP_READ  ? {1'b0, shift[8:1]}
                  : 9'h000;

    always_ff @(posedge clk) begin
        scl_sync <= {scl_sync[0], scl_in};
        sda_sync <= {sda_sync[0], sda_in};

        if (rst) begin
            state <= IDLE;
            owned <= 1'b0;
            scl_oe <= 1'b0;
            sda_oe <= 1'b0;
        end
        else if (start) begin
            op <= call_op;
            quarter <= 3'd0;
            nbit <= 4'd0;
            count <= RELOAD;
            case (call_op)
                OP_START: begin
                    state <= BUSY;
                    sda_oe <= 1'b0;
                end
                OP_STOP: begin
                    state <= owned ? BUSY : DONE;
                    if (owned) sda_oe <= 1'b1;
                end
                OP_WRITE: begin
                    state <= BUSY;
                    shift <= {call_data, 1'b1};     // release SDA for the acknowledge
                    sda_oe <= !call_data[7];
                end
                default: begin                      // OP_READ
                    state <= BUSY;
                    shift <= {8'hFF, !call_data[0]};
                    sda_oe <= 1'b0;
                end
            endcase
        end
        else if (state == DONE) begin
            if (result_rdy) state <= IDLE;
        end
        else if (state == BUSY) begin
            if (stretched) count <= RELOAD;
            else if (count != 0) count <= count - 1'b1;
            else begin
                count <= RELOAD;
                quarter <= quarter + 3'd1;
                case (quarter)
                    3'd0: scl_oe <= 1'b0;
                    3'd1: if (byte_op) shift <= {shift[7:0], sda};
                    3'd2: if (byte_op) scl_oe <= 1'b1;
                          else sda_oe <= (op == OP_START);  // the condition itself
                    3'd3: if (byte_op) begin
                              if (nbit == 4'd8) state <= DONE;
                              else begin
                                  nbit <= nbit + 4'd1;
                                  quarter <= 3'd0;
                                  sda_oe <= !shift[8];
                              end
                          end
                    3'd4: if (op == OP_STOP) begin
                              state <= DONE;
                              owned <= 1'b0;
                          end
                          else scl_oe <= 1'b1;
                    default: begin                  // the end of a START
                        state <= DONE;
                        owned <= 1'b1;
                    end
                endcase
            end
        end
    end
endmodule

`default_nettype wire
