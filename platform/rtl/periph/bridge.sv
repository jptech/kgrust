// The host bridge's link: a UART carrying SLIP frames (RFC 1055), one frame
// held each way. The dispatcher a generated core puts around it decides what
// a frame means (crates/kg-board/src/wrapper.rs, `bridge`); the host side is
// platform/python/kgbridge.py.
//
// SLIP: 0xC0 ends a frame; 0xC0 and 0xDB inside one are sent as 0xDB 0xDC
// and 0xDB 0xDD. Every frame sent starts with 0xC0 too, so a receiver
// resynchronises on the next frame after noise. Bytes are held least
// significant first: byte i of a frame is data[8i +: 8].
//
// Receiving: a complete frame is offered with rx_valid_out until
// rx_take_in. Bytes arriving while one is offered cannot be kept (a UART
// cannot be told to wait), so that frame is lost, as is one longer than
// RX_BYTES; either pulses rx_lost_out once its end arrives. The host waits
// for each reply, so neither happens in use.
//
// Sending: tx_start_in while !tx_busy_out takes tx_len_in bytes of
// tx_data_in and sends them as one frame.

`default_nettype none

module kg_bridge_link
#(
    parameter int CLKS_PER_BIT = 208,
    parameter int RX_BYTES = 8,     // at most 255
    parameter int TX_BYTES = 8      // at most 255
)
(
    input  wire                     clk,
    input  wire                     rst,
    input  wire                     rxd,
    output wire                     txd,

    output logic                    rx_valid_out,
    output logic [7:0]              rx_len_out,
    output logic [8*RX_BYTES-1:0]   rx_data_out,
    input  wire                     rx_take_in,
    output logic                    rx_lost_out,

    input  wire                     tx_start_in,
    input  wire  [7:0]              tx_len_in,
    input  wire  [8*TX_BYTES-1:0]   tx_data_in,
    output wire                     tx_busy_out
);
    localparam logic [7:0] END = 8'hC0;
    localparam logic [7:0] ESC = 8'hDB;
    localparam logic [7:0] ESC_END = 8'hDC;
    localparam logic [7:0] ESC_ESC = 8'hDD;

    // ---- Receive -------------------------------------------------------

    wire       byte_valid;
    wire [7:0] byte_in;

    uart_rx #(.CLKS_PER_BIT(CLKS_PER_BIT)) receiver
    (
        .clk                (clk),
        .rst                (rst),
        .rxd                (rxd),
        .valid_out          (byte_valid),
        .data_out           (byte_in),
        .framing_error_out  ()
    );

    logic [7:0] count;      // bytes of the frame being received
    logic       escaped;    // the last byte was ESC
    logic       overflow;   // the frame being received is too long
    logic       dropping;   // bytes came while a frame was offered

    // A frame taken this cycle frees the buffer for this cycle's byte.
    wire offered = rx_valid_out && !rx_take_in;
    wire [7:0] value = !escaped ? byte_in : (byte_in == ESC_END) ? END : (byte_in == ESC_ESC) ? ESC : byte_in;

    always_ff @(posedge clk) begin
        rx_lost_out <= 1'b0;
        if (rst) begin
            rx_valid_out <= 1'b0;
            count <= '0;
            escaped <= 1'b0;
            overflow <= 1'b0;
            dropping <= 1'b0;
        end
        else begin
            if (rx_take_in) rx_valid_out <= 1'b0;
            if (byte_valid) begin
                if (byte_in == END) begin
                    // An empty frame is the separator every frame starts with.
                    if (dropping || overflow) rx_lost_out <= 1'b1;
                    else if (count != 0 && !offered) begin
                        rx_valid_out <= 1'b1;
                        rx_len_out <= count;
                    end
                    count <= '0;
                    escaped <= 1'b0;
                    overflow <= 1'b0;
                    dropping <= 1'b0;
                end
                else if (offered) dropping <= 1'b1;
                else if (byte_in == ESC) escaped <= 1'b1;
                else begin
                    escaped <= 1'b0;
                    // A byte to its place by comparison, not a variable
                    // index: an enable per byte rather than a barrel shifter.
                    for (int i = 0; i < RX_BYTES; i++) begin
                        if (count == 8'(i)) rx_data_out[8 * i +: 8] <= value;
                    end
                    if (count < 8'(RX_BYTES)) count <= count + 8'd1;
                    else overflow <= 1'b1;
                end
            end
        end
    end

    // ---- Send ----------------------------------------------------------

    typedef enum logic [2:0] { IDLE, LEAD, DATA, SECOND, TRAIL } tx_state_t;
    tx_state_t            state;
    logic [8*TX_BYTES-1:0] pending;     // the bytes still to send, next lowest
    logic [7:0]           left;
    logic [7:0]           second;       // an escape's second byte
    wire                  line_idle;
    wire [7:0]            next = pending[7:0];
    wire                  special = (next == END) || (next == ESC);

    logic       send;
    logic [7:0] send_byte;

    always_comb begin
        send = line_idle && state != IDLE;
        case (state)
            DATA:    send_byte = special ? ESC : next;
            SECOND:  send_byte = second;
            default: send_byte = END;
        endcase
    end

    assign tx_busy_out = state != IDLE;

    always_ff @(posedge clk) begin
        if (rst) state <= IDLE;
        else if (state == IDLE) begin
            if (tx_start_in) begin
                pending <= tx_data_in;
                left <= tx_len_in;
                state <= LEAD;
            end
        end
        else if (send) begin
            case (state)
                LEAD: state <= (left == 0) ? TRAIL : DATA;
                DATA: begin
                    if (special) begin
                        second <= (next == END) ? ESC_END : ESC_ESC;
                        state <= SECOND;
                    end
                    else state <= (left == 1) ? TRAIL : DATA;
                    if (!special) begin
                        pending <= pending >> 8;
                        left <= left - 8'd1;
                    end
                end
                SECOND: begin
                    pending <= pending >> 8;
                    left <= left - 8'd1;
                    state <= (left == 1) ? TRAIL : DATA;
                end
                default: state <= IDLE;     // TRAIL
            endcase
        end
    end

    uart_tx #(.CLKS_PER_BIT(CLKS_PER_BIT)) transmitter
    (
        .clk        (clk),
        .rst        (rst),
        .valid_in   (send),
        .data_in    (send_byte),
        .idle_out   (line_idle),
        .txd        (txd)
    );
endmodule

`default_nettype wire
