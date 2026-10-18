// A simulation model of an I2C target with a register file, shaped like a
// TMP102: sixteen byte registers, reset to 8'h10 + i, behind a pointer.
// After its address with the write bit, the first byte sets the pointer
// (its low four bits) and every further byte is stored at the pointer;
// after its address with the read bit, it sends bytes from the pointer for
// as long as the master acknowledges them. The pointer increments after
// every byte stored or sent, wrapping at 16. It acknowledges ADDRESS and
// every byte written to it, and stays off the bus for any other address.
//
// `scl` and `sda` are the resolved bus levels; `scl_oe` and `sda_oe` pull
// the lines low. The model is clocked, like a target built in an FPGA: it
// sees the bus at each rising edge of `clk`, so it reacts to an SCL edge a
// clock later, which gives the bus a data hold time after SCL falls. With
// STRETCH > 0 it holds SCL low for STRETCH clocks after the falling edge
// that ends each byte it takes part in, a clock stretch; with 0 `scl_oe`
// stays low.

`default_nettype none

module i2c_target_model
#(
    parameter logic [6:0] ADDRESS = 7'h48,
    parameter int         STRETCH = 0
)
(
    input  wire  clk,
    input  wire  scl,
    input  wire  sda,
    output logic scl_oe,
    output logic sda_oe
);
    typedef enum { IDLE, ADDR, POINTER, WRITE, READ, IGNORE } mode_t;

    logic [7:0] regs [16];
    logic [3:0] ptr;
    mode_t      mode;
    logic       scl_q, sda_q;
    int         nbit;       // the bit on the bus within its byte, 0..8; -1 just after START
    logic [7:0] rx;
    logic [7:0] tx;
    logic       reading;    // the address byte asked for a read
    logic       acked;      // the master acknowledged the byte we sent
    int         stretch;

    initial begin
        for (int i = 0; i < 16; i++) regs[i] = 8'h10 + 8'(i);
        ptr = 4'd0;
        mode = IDLE;
        scl_q = 1'b1;
        sda_q = 1'b1;
        scl_oe = 1'b0;
        sda_oe = 1'b0;
        stretch = 0;
        nbit = 0;
    end

    always @(posedge clk) begin
        if (stretch > 0) begin
            stretch <= stretch - 1;
            if (stretch == 1) scl_oe <= 1'b0;
        end

        if (scl_q && scl && sda_q && !sda) begin            // START
            mode <= ADDR;
            nbit <= -1;
            sda_oe <= 1'b0;
        end
        else if (scl_q && scl && !sda_q && sda) begin       // STOP
            mode <= IDLE;
            sda_oe <= 1'b0;
        end
        else if (mode != IDLE && !scl_q && scl) begin       // SCL rises: a bit is valid
            if (nbit < 8) rx <= {rx[6:0], sda};
            else acked <= !sda;
        end
        else if (mode != IDLE && scl_q && !scl) begin       // SCL falls: the next bit
            if (nbit == -1) nbit <= 0;
            else if (nbit < 7) begin
                nbit <= nbit + 1;
                if (mode == READ) sda_oe <= !tx[6 - nbit];
            end
            else if (nbit == 7) begin                       // the acknowledge slot
                nbit <= 8;
                sda_oe <= 1'b0;
                case (mode)
                    ADDR: begin
                        reading <= rx[0];
                        if (rx[7:1] == ADDRESS) sda_oe <= 1'b1;
                        else mode <= IGNORE;
                    end
                    POINTER: begin
                        ptr <= rx[3:0];
                        sda_oe <= 1'b1;
                    end
                    WRITE: begin
                        regs[ptr] <= rx;
                        ptr <= ptr + 4'd1;
                        sda_oe <= 1'b1;
                    end
                    default: ;                              // READ: the master acknowledges
                endcase
            end
            else begin                                      // the byte is over
                nbit <= 0;
                sda_oe <= 1'b0;
                if (STRETCH > 0 && mode != IGNORE) begin
                    scl_oe <= 1'b1;
                    stretch <= STRETCH;
                end
                case (mode)
                    ADDR: begin
                        mode <= reading ? READ : POINTER;
                        if (reading) begin
                            tx <= regs[ptr];
                            sda_oe <= !regs[ptr][7];
                            ptr <= ptr + 4'd1;
                        end
                    end
                    POINTER: mode <= WRITE;
                    READ: if (acked) begin
                        tx <= regs[ptr];
                        sda_oe <= !regs[ptr][7];
                        ptr <= ptr + 4'd1;
                    end
                    else mode <= IGNORE;
                    default: ;
                endcase
            end
        end

        scl_q <= scl;
        sda_q <= sda;
    end
endmodule

`default_nettype wire
