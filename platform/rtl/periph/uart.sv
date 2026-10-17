// A UART: 8 data bits, no parity, one stop bit, least significant bit
// first, at CLKS_PER_BIT clocks per bit (24 MHz / 115200 baud = 208, 0.16%
// fast). Vendor-neutral; a generated core instantiates it for a board's
// `uart` resource and the host bridge (bridge.sv) for its link, and a
// hand-written core may use it directly.

`default_nettype none

// The receiver: synchronises the line, finds a start bit's falling edge,
// samples each bit at its middle, and presents a byte for one cycle with
// `valid_out`. A byte whose stop bit is low is dropped, with
// `framing_error_out` for that cycle.
module uart_rx
#(
    parameter int CLKS_PER_BIT = 208
)
(
    input  wire        clk,
    input  wire        rst,
    input  wire        rxd,
    output logic       valid_out,
    output logic [7:0] data_out,
    output logic       framing_error_out
);
    localparam int CW = $clog2(CLKS_PER_BIT);

    (* ASYNC_REG = "TRUE" *) logic [1:0] sync = 2'b11;  // idle high
    wire line = sync[1];

    typedef enum logic [1:0] { IDLE, START, DATA, STOP } state_t;
    state_t        state;
    logic [CW-1:0] count;       // counts down from at most CLKS_PER_BIT - 1
    logic [2:0]    bit_index;
    logic [7:0]    shift;

    always_ff @(posedge clk) begin
        sync <= {sync[0], rxd};
        valid_out <= 1'b0;
        framing_error_out <= 1'b0;

        if (rst) begin
            state <= IDLE;
        end
        else begin
            case (state)
                IDLE: if (!line) begin
                    state <= START;
                    count <= CLKS_PER_BIT / 2 - 1;
                end
                // Confirm the start bit at its middle; a glitch returns to idle.
                START: if (count == 0) begin
                    state <= line ? IDLE : DATA;
                    count <= CLKS_PER_BIT - 1;
                    bit_index <= 0;
                end
                else count <= count - 1;
                DATA: if (count == 0) begin
                    shift <= {line, shift[7:1]};
                    count <= CLKS_PER_BIT - 1;
                    bit_index <= bit_index + 1;
                    if (bit_index == 7) state <= STOP;
                end
                else count <= count - 1;
                STOP: if (count == 0) begin
                    state <= IDLE;
                    valid_out <= line;
                    framing_error_out <= !line;
                    data_out <= shift;
                end
                else count <= count - 1;
            endcase
        end
    end
endmodule

// The transmitter: takes a byte with `valid_in` while `idle_out` is high,
// and sends the start bit, the data and the stop bit, each a full period;
// it is idle again once the stop bit's period has ended.
module uart_tx
#(
    parameter int CLKS_PER_BIT = 208
)
(
    input  wire        clk,
    input  wire        rst,
    input  wire        valid_in,
    input  wire  [7:0] data_in,
    output wire        idle_out,
    output logic       txd
);
    localparam int CW = $clog2(CLKS_PER_BIT);

    logic [8:0]    shift;       // the bits after the one on the line: data, then stop
    logic [3:0]    periods;     // bit periods left, the current one included
    logic [CW-1:0] count;

    assign idle_out = (periods == 0);

    always_ff @(posedge clk) begin
        if (rst) begin
            periods <= 0;
            txd <= 1'b1;
        end
        else if (periods == 0) begin
            if (valid_in) begin
                txd <= 1'b0;                    // start bit
                shift <= {1'b1, data_in};
                periods <= 10;
                count <= CLKS_PER_BIT - 1;
            end
        end
        else if (count == 0) begin
            periods <= periods - 1;
            if (periods > 1) begin
                txd <= shift[0];
                shift <= {1'b1, shift[8:1]};
                count <= CLKS_PER_BIT - 1;
            end
        end
        else count <= count - 1;
    end
endmodule

`default_nettype wire
