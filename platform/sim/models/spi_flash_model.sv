// A simulation model of a serial NOR flash, SPI mode 0 only (SCLK idles
// low, MOSI sampled on the rising edge, MISO changed on the falling edge),
// enough to exercise an SPI master:
//
//   0x9F  JEDEC ID: 0xEF 0x40 0x16, a Winbond W25Q32's.
//   0x05  read status register 1: 0x00, never busy.
//   0x03  READ: a 24-bit address, most significant byte first, then bytes
//         from that address on for as long as chip select stays low, the
//         address wrapping modulo DEPTH.
//
// Other commands are ignored until chip select rises. The memory starts as
// `addr ^ 8'hA5` per byte unless INIT names a file for $readmemh. MISO is
// driven 0 rather than left floating while deselected or idle, which keeps
// the model two-state; nothing in a test may depend on it there.
//
// The model also checks the mode-0 contract it relies on, printing an
// ERROR line and counting in `errors`: MOSI changes only while SCLK is low,
// and SCLK is low whenever chip select changes.

`default_nettype none
// A behavioural model: its edge-triggered processes assign with `=` so that
// each reads what it has just computed.
/* verilator lint_off BLKSEQ */
// Chip select resets the bit count asynchronously, as in the device.
/* verilator lint_off SYNCASYNCNET */

module spi_flash_model
#(
    parameter int    DEPTH = 256,
    parameter string INIT = ""
)
(
    input  wire  sclk,
    input  wire  mosi,
    input  wire  cs_n,
    output logic miso,
    output int   errors
);
    localparam int AW = DEPTH > 1 ? $clog2(DEPTH) : 1;

    logic [7:0]  mem [DEPTH];
    logic [7:0]  rx;        // the byte coming in
    int          nbit;      // bits of it received so far
    int          nbyte;     // whole bytes received in this transaction
    logic [7:0]  cmd;
    logic [23:0] addr;
    logic [7:0]  tx;        // the byte going out
    logic        tx_ready;  // a byte has just ended: load `tx` at the next falling edge
    logic [7:0]  next_tx;

    initial begin
        errors = 0;
        miso = 1'b0;
        if (INIT != "") $readmemh(INIT, mem);
        else for (int i = 0; i < DEPTH; i++) mem[i] = 8'(i) ^ 8'hA5;
    end

    // Rising edges shift MOSI in; a whole byte decides what goes out next.
    always @(posedge sclk or posedge cs_n) begin
        if (cs_n) begin
            nbit = 0;
            nbyte = 0;
            tx_ready = 1'b0;
        end
        else begin
            rx = {rx[6:0], mosi};
            nbit = nbit + 1;
            if (nbit == 8) begin
                nbit = 0;
                next_tx = 8'h00;
                if (nbyte == 0) begin
                    cmd = rx;
                    if (cmd == 8'h9F) next_tx = 8'hEF;
                end
                else case (cmd)
                    8'h9F: next_tx = nbyte == 1 ? 8'h40 : nbyte == 2 ? 8'h16 : 8'h00;
                    8'h03: begin
                        if (nbyte <= 3) addr = {addr[15:0], rx};
                        else addr = addr + 24'd1;
                        if (nbyte >= 3) next_tx = mem[AW'(addr % DEPTH)];
                    end
                    default: ;
                endcase
                nbyte = nbyte + 1;
                tx_ready = 1'b1;
            end
        end
    end

    // Falling edges put the next bit out; the first bit of a byte goes out
    // at the falling edge that ends the byte before.
    always @(negedge sclk) begin
        if (!cs_n) begin
            if (tx_ready) begin
                tx = next_tx;
                tx_ready = 1'b0;
            end
            else tx = {tx[6:0], 1'b0};
            miso = tx[7];
        end
    end

    // The command byte has nothing to answer with.
    always @(cs_n) begin
        tx = 8'h00;
        miso = 1'b0;
        if (sclk) begin
            $display("ERROR spi_flash_model: chip select changed with SCLK high at %0t", $time);
            errors = errors + 1;
        end
    end

    always @(mosi) begin
        if (!cs_n && sclk) begin
            $display("ERROR spi_flash_model: MOSI changed with SCLK high at %0t", $time);
            errors = errors + 1;
        end
    end
endmodule

`default_nettype wire
