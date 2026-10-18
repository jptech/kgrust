// Self-checking testbench for the UPduino monitor (core.sv + Monitor +
// lattice.spram + rtl/uart.sv), run by `kg board sim` in this directory
// and by `kg board netsim` against the netlist.
//
// It speaks to the core's serial pins as the host would, 8N1 at the core's
// 208 clocks per bit, and checks every reply byte, in order:
// - writes and reads back, in both SPRAM blocks, in both cases, with CR, LF
//   and CR LF endings; the address's top bit ignored;
// - malformed lines answered "?", blank lines not answered;
// - a glitch on the idle line, shorter than half a bit, ignored;
// - 32 commands sent back to back, with no wait for replies: all answered;
// - a burst of short malformed lines, faster than their replies can go out:
//   the replies that do go out are whole, and the LED shows the drop;
// - the LED green after a good command, red after a bad one.

`timescale 1ns/1ps
`default_nettype none

module tb;
    // Set in initial blocks: Verilator 5.052 drops the initialiser of a
    // variable driving a netlist's input port (platform-layer.md §8.5).
    logic       clk;
    logic       rst;
    logic       rxd;
    wire        txd;
    wire  [2:0] rgb;

    initial clk = 1'b0;
    always #20.833 clk = ~clk;  // 24 MHz

    upduino_core dut
    (
        .clk        (clk),
        .rst        (rst),
        .rgb        (rgb),
        .uart_txd   (txd),
        .uart_rxd   (rxd),
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

    localparam int BIT = 208;   // clocks per bit at 24 MHz, 115200 baud

    int errors = 0;
    byte expected[$];
    int received = 0;
    bit collect = 0;        // keep bytes in `collected` rather than check them
    byte collected[$];

    task automatic fail(input string what);
        if (errors < 20) $display("ERROR %s", what);
        errors++;
    endtask

    // Host to board: one byte, 8N1.
    task automatic send_byte(input byte b);
        rxd = 1'b0;
        repeat (BIT) @(posedge clk);
        for (int i = 0; i < 8; i++) begin
            rxd = b[i];
            repeat (BIT) @(posedge clk);
        end
        rxd = 1'b1;
        repeat (BIT) @(posedge clk);
    endtask

    task automatic send(input string s);
        for (int i = 0; i < s.len(); i++) send_byte(s[i]);
    endtask

    // Queued before the command is sent: a reply can begin while the last
    // bytes of its line (an LF after the CR) are still going out.
    task automatic expect_reply(input string s);
        for (int i = 0; i < s.len(); i++) expected.push_back(s[i]);
    endtask

    // Board to host: every byte checked against the expected stream.
    initial begin
        @(negedge rst);
        forever begin
            byte b;
            @(negedge txd);
            repeat (BIT / 2) @(posedge clk);
            if (txd !== 1'b0) fail("start bit not low at its middle");
            for (int i = 0; i < 8; i++) begin
                repeat (BIT) @(posedge clk);
                b[i] = txd;
            end
            repeat (BIT) @(posedge clk);
            if (txd !== 1'b1) fail($sformatf("stop bit low after byte %0d", received));
            if (collect) collected.push_back(b);
            else if (expected.size() == 0) fail($sformatf("unexpected byte %02h", b));
            else begin
                // Assigned, not initialised: this block is static, and an
                // initialiser would run once, at time zero.
                byte want;
                want = expected.pop_front();
                if (b !== want) fail($sformatf("reply byte %0d: %02h, want %02h", received, b, want));
            end
            received++;
        end
    end

    // Wait until every expected byte has arrived, and a little more.
    task automatic drain(input string what);
        int waited = 0;
        while (expected.size() != 0 && waited < 200 * 10 * BIT) begin
            @(posedge clk);
            waited++;
        end
        if (expected.size() != 0) fail($sformatf("%s: %0d reply bytes never came", what, expected.size()));
        repeat (20 * BIT) @(posedge clk);
    endtask

    task automatic expect_led(input logic [1:0] red_green, input string what);
        if (rgb[2:1] !== red_green) fail($sformatf("%s: LED %b, want red/green %b", what, rgb, red_green));
    endtask

    function automatic string hex4(input int v);
        return $sformatf("%04x", v[15:0]);
    endfunction

    initial begin
        rxd = 1'b1;
        rst = 1'b1;
        repeat (16) @(negedge clk);
        rst = 1'b0;
        repeat (100) @(negedge clk);

        // Write and read back, in each block, lower and upper case.
        expect_reply("beef\r\n");  send("w 0010 beef\r");
        expect_reply("beef\r\n");  send("r 0010\r");
        expect_reply("c0de\r\n");  send("W 4010 C0DE\n");
        expect_reply("c0de\r\n");  send("R 4010\r\n");      // the LF is a blank line
        expect_reply("beef\r\n");  send("r 0010\n");        // the other block kept its word
        expect_reply("c0de\r\n");  send("r c010\r");        // bit 15 ignored
        drain("read and write");
        expect_led(2'b01, "after good commands");

        // Malformed lines, and a blank one, which gets nothing.
        expect_reply("?\r\n");     send("x\r");
        expect_reply("?\r\n");     send("r 12\r");
        expect_reply("?\r\n");     send("r 00100\r");
        expect_reply("?\r\n");     send("w 0010\r");
        expect_reply("?\r\n");     send("r 0010 0000\r");
        send("\r");
        drain("malformed lines");
        expect_led(2'b10, "after a bad command");
        expect_reply("beef\r\n");  send("r 0010\r");        // nothing was written
        drain("after malformed lines");
        expect_led(2'b01, "after a good command again");

        // A blank line with nothing else going out: still no reply (drain
        // fails on any byte not expected).
        send("\r");
        drain("blank line");

        // A glitch on the idle line, a quarter of a bit: not a start bit.
        rxd = 1'b0;
        repeat (BIT / 4) @(posedge clk);
        rxd = 1'b1;
        repeat (2 * 10 * BIT) @(posedge clk);
        expect_reply("beef\r\n");  send("r 0010\r");
        drain("after a glitch");

        // Back to back: 16 writes and 16 reads, sent with no waiting.
        for (int i = 0; i < 16; i++) begin
            expect_reply({hex4(16'h1111 * i + 7), "\r\n"});
            send({"w ", hex4(i * 2049), " ", hex4(16'h1111 * i + 7), "\r"});
        end
        for (int i = 0; i < 16; i++) begin
            expect_reply({hex4(16'h1111 * i + 7), "\r\n"});
            send({"r ", hex4(i * 2049), "\r"});
        end
        drain("back to back");
        expect_led(2'b01, "after the stream");

        // Four 2-byte malformed lines back to back: each wants a 3-byte
        // reply, so some are dropped. Whatever goes out must still be whole
        // "?" CR LF lines, never a reply cut short by the next.
        collect = 1;
        send("x\rx\rx\rx\r");
        repeat (20 * 10 * BIT) @(posedge clk);
        collect = 0;
        if (collected.size() == 0 || collected.size() % 3 != 0 || collected.size() > 12)
            fail($sformatf("burst: %0d reply bytes, want whole 3-byte replies", collected.size()));
        for (int i = 0; i < collected.size(); i++) begin
            byte want;
            want = (i % 3 == 0) ? "?" : ((i % 3 == 1) ? 8'h0d : 8'h0a);
            if (collected[i] !== want) fail($sformatf("burst byte %0d: %02h, want %02h", i, collected[i], want));
        end
        if (collected.size() == 12) fail("burst: no reply was dropped");
        expect_led(2'b10, "after the burst");

        if (errors == 0) $display("PASS: upduino monitor (%0d reply bytes)", received);
        else             $display("FAIL: %0d check(s)", errors);
        $finish;
    end
endmodule

`default_nettype wire
