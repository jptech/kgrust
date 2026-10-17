// Board shell for the Mimas A7 pilot: binds the Blinky design's exported
// interface (blinky.k) to the board's clock, reset, DIP switches and LEDs.
// Port names match ../../constraints/mimas-a7.xdc.
//
// The handshakes follow upstream's own testbench for an exported class
// (test/interface/simple_exported_class.sv): a call is offered only while
// `<fn>_rdy_out` is high, and the result port is read show-ahead, its data
// valid while `<fn>_empty_out` is low and consumed by `<fn>_rden_in`.

`default_nettype none

module mimas_a7_top
(
    input  wire        CLK1,      // 100 MHz oscillator
    input  wire        RESET,     // push button S3, active high
    input  wire [7:0]  dip_sw,
    output logic [7:0] LED
);
    // Reset: the button or power-on, asserted asynchronously and released in
    // step with the clock. The FPGA leaves configuration with every register
    // at its INIT value, so `por_count` needs no reset of its own.
    logic [3:0] por_count = '0;
    logic [1:0] rst_sync = 2'b11;
    wire        por_done = &por_count;

    always_ff @(posedge CLK1) begin
        if (!por_done) por_count <= por_count + 4'd1;
    end

    always_ff @(posedge CLK1 or posedge RESET) begin
        if (RESET) rst_sync <= 2'b11;
        else       rst_sync <= {rst_sync[0], ~por_done};
    end

    wire rst = rst_sync[1];

    // The switches are asynchronous to CLK1.
    (* ASYNC_REG = "TRUE" *) logic [7:0] sw_meta, sw_sync;

    always_ff @(posedge CLK1) begin
        sw_meta <= dip_sw;
        sw_sync <= sw_meta;
    end

    logic       startup_done;
    logic       step_rdy;
    logic       step_empty;
    logic [7:0] step_result;

    wire step_valid = ~rst & startup_done & step_rdy;
    wire step_rden  = ~rst & ~step_empty;

    Blinky blinky
    (
        .clk                      (CLK1),
        .rst                      (rst),
        .rst_and_startup_done_out (startup_done),

        .step_valid_in            (step_valid),
        .step_switches_in         (sw_sync),
        .step_rdy_out             (step_rdy),

        .step_rden_in             (step_rden),
        .step_empty_out           (step_empty),
        .step_result_out          (step_result),

        .stall_rate_supported_out (),
        .stall_rate_valid_in      (1'b0),
        .stall_rate_in            (3'd0)
    );

    always_ff @(posedge CLK1) begin
        if (rst)            LED <= '0;
        else if (step_rden) LED <= step_result;
    end
endmodule

`default_nettype wire
