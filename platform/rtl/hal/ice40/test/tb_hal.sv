// Self-checking testbench for the iCE40 HAL (platform/rtl/hal/ice40), run by
// platform/rtl/hal/ice40/test/check.zsh. Random traffic against reference
// models, cycle by cycle:
//
// - the show-ahead FIFO in both storage kinds (the runtime's register FIFO,
//   and KanagawaIce40BlockRamFifo): order, data, usedw, empty, full and
//   almost_full after every cycle, under random pushes and pops;
// - the dual-clock FIFO across two unrelated clocks: order and data;
// - KanagawaHALDualPortRAM, flip-flops and block RAM, with and without the
//   output register: every read against the model at its latency;
// - KanagawaLutMul, signed and unsigned, against `*`;
// - KanagawaIce40Spram, 64K x 16 (four blocks deep) and 16K x 64 (four
//   wide), with nibble masks, against a model, through Yosys's
//   SB_SPRAM256KA.

`timescale 1ns/1ps
`default_nettype none

module tb;
    logic clk;
    logic rst;
    int errors;

    initial clk = 1'b0;
    always #5 clk = ~clk;

    task automatic fail(input string what);
        if (errors < 20) $display("ERROR %s", what);
        errors++;
    endtask

    //
    // Show-ahead FIFOs, one per storage kind
    //
    localparam int FD = 16, FW = 8, AF = 4;

    logic [1:0]          f_push, f_pop;
    logic [FW-1:0]       f_data;
    wire  [1:0]          f_full, f_afull, f_empty;
    wire  [1:0][4:0]     f_usedw;
    wire  [1:0][FW-1:0]  f_q;

    for (genvar k = 0; k < 2; k++) begin : gen_fifo
        KanagawaHALShowAheadFifo
        #(
            .DEPTH              (FD),
            .WIDTH              (FW),
            .ALMOSTFULL_ENTRIES (AF),
            .ALMOSTEMPTY_VAL    (0),
            .USE_LUTRAM         (k == 0)
        ) fifo
        (
            .clock          (clk),
            .rst            (rst),
            .wrreq          (f_push[k]),
            .data           (f_data),
            .full           (f_full[k]),
            .almost_full    (f_afull[k]),
            .usedw          (f_usedw[k]),
            .rdreq          (f_pop[k]),
            .empty          (f_empty[k]),
            .almost_empty   (),
            .q              (f_q[k])
        );
    end

    // The runtime asserts almost_full the cycle after rst
    // (write_delay_fifo.sv:74): both FIFOs, and the dual-clock one's write
    // side after wrrst.
    logic rst_seen = 1'b0, wrst_seen = 1'b0;
    wire  d_afull;
    always @(posedge clk) begin
        if (rst_seen && f_afull !== 2'b11) fail($sformatf("almost_full %b the cycle after reset", f_afull));
        rst_seen <= rst;
    end
    always @(posedge wclk) begin
        if (wrst_seen && d_afull !== 1'b1) fail("dual-clock almost_full low the cycle after wrrst");
        wrst_seen <= d_wrst;
    end

    // The register FIFO shows a word the cycle after its push. The block-RAM
    // FIFO reads synchronously, so a word pushed into an empty FIFO reaches
    // the head a cycle later, as an XPM first-word-fall-through FIFO's does;
    // occupancy and almost_full count it at once all the same. The model
    // allows that one cycle and no more.
    task automatic test_fifo(input int k, input int cycles);
        logic [FW-1:0] model[$];
        int pushed_at[$];
        string name = (k == 0) ? "register FIFO" : "block-RAM FIFO";
        int pushes = 0;
        for (int c = 0; c < cycles; c++) begin
            bit lagging = (k == 1) && model.size() != 0 && pushed_at[0] == c - 1;
            // Flags and head against the model, then this cycle's traffic.
            if (f_usedw[k] != model.size()) fail($sformatf("%s cycle %0d: usedw %0d, model %0d", name, c, f_usedw[k], model.size()));
            if (f_empty[k] != (model.size() == 0) && !(lagging && f_empty[k])) fail($sformatf("%s cycle %0d: empty %b with %0d entries", name, c, f_empty[k], model.size()));
            if (f_full[k] != (model.size() >= FD)) fail($sformatf("%s cycle %0d: full %b with %0d entries", name, c, f_full[k], model.size()));
            if (f_afull[k] != (model.size() >= FD - AF)) fail($sformatf("%s cycle %0d: almost_full %b with %0d entries", name, c, f_afull[k], model.size()));
            if (!f_empty[k] && model.size() != 0 && f_q[k] !== model[0]) fail($sformatf("%s cycle %0d: q %h, model %h", name, c, f_q[k], model[0]));
            // Bursts of filling and draining, so both full and empty are reached.
            f_push[k] = !f_full[k] && ($urandom_range(0, 99) < (((c / 200) % 2) ? 80 : 30));
            f_pop[k] = !f_empty[k] && ($urandom_range(0, 99) < (((c / 200) % 2) ? 30 : 80));
            f_data = $urandom;
            @(posedge clk);
            if (f_pop[k]) begin void'(model.pop_front()); void'(pushed_at.pop_front()); end
            if (f_push[k]) begin model.push_back(f_data); pushed_at.push_back(c); pushes++; end
            #1;
        end
        f_push[k] = 0; f_pop[k] = 0;
        if (pushes < cycles / 4) fail($sformatf("%s: only %0d pushes", name, pushes));
    endtask

    //
    // Dual-clock FIFO
    //
    logic wclk, rclk;
    initial begin wclk = 1'b0; rclk = 1'b0; end
    always #3.7 wclk = ~wclk;
    always #6.1 rclk = ~rclk;

    logic          d_push, d_pop, d_wrst, d_rrst;
    logic [FW-1:0] d_data;
    wire           d_full, d_empty;
    wire [FW-1:0]  d_q;

    KanagawaHALDualClockFifo #(.DEPTH(16), .WIDTH(FW), .ALMOSTFULL_ENTRIES(0)) dcfifo
    (
        .wrclk(wclk), .wrrst(d_wrst), .wrreq(d_push), .data(d_data), .full(d_full), .almost_full(d_afull), .usedw(),
        .rdclk(rclk), .rdrst(d_rrst), .rdreq(d_pop), .empty(d_empty), .q(d_q)
    );

    logic [FW-1:0] dc_model[$];
    int dc_sent, dc_got;

    task automatic test_dual_clock(input int words);
        dc_sent = 0; dc_got = 0;
        fork
            begin
                while (dc_sent < words) begin
                    @(negedge wclk);
                    d_push = !d_full && $urandom_range(0, 1);
                    d_data = $urandom;
                    if (d_push) begin dc_model.push_back(d_data); dc_sent++; end
                end
                @(negedge wclk); d_push = 0;
            end
            begin
                while (dc_got < words) begin
                    @(negedge rclk);
                    d_pop = !d_empty && $urandom_range(0, 1);
                    if (d_pop) begin
                        if (d_q !== dc_model[0]) fail($sformatf("dual-clock FIFO word %0d: q %h, model %h", dc_got, d_q, dc_model[0]));
                        void'(dc_model.pop_front());
                        dc_got++;
                    end
                end
                @(negedge rclk); d_pop = 0;
            end
        join
    endtask

    //
    // Dual-port RAMs: {flip-flops, block} x {no output register, output register}
    //
    localparam int RW = 12, RA = 6;

    logic [RA-1:0]        r_waddr, r_raddr;
    logic                 r_wren, r_rden;
    logic [RW-1:0]        r_wdata;
    wire  [3:0][RW-1:0]   r_q;

    for (genvar k = 0; k < 4; k++) begin : gen_ram
        wire [1:0][RW-1:0] data_out;
        KanagawaHALDualPortRAM
        #(
            .DATA_WIDTH     (RW),
            .ADDR_WIDTH     (RA),
            .USE_OUTPUT_REG (k / 2),
            .USE_LUTRAM     (k % 2 == 0),
            .USE_BRAM       (k % 2 == 1)
        ) ram
        (
            .clk                            (clk),
            .addr_in                        ({r_raddr, r_waddr}),
            .wren_in                        ({1'b0, r_wren}),
            .data_in                        ({{RW{1'b0}}, r_wdata}),
            .inject_correctable_error_in    (1'b0),
            .inject_uncorrectable_error_in  (1'b0),
            .rden_in                        ({r_rden, 1'b0}),
            .data_out                       (data_out),
            .error_detected_out             (),
            .data_valid_out                 ()
        );
        assign r_q[k] = data_out[1];
    end

    task automatic test_rams(input int cycles);
        logic [RW-1:0] model [0:(1<<RA)-1];
        logic [RW-1:0] expect1 = 'x, expect2 = 'x;
        bit valid1 = 0, valid2 = 0;
        for (int a = 0; a < (1 << RA); a++) model[a] = 'x;
        for (int c = 0; c < cycles; c++) begin
            // Latency 1 without the output register, 2 with it.
            if (valid1 && r_q[0] !== expect1) fail($sformatf("flip-flop RAM cycle %0d: %h, model %h", c, r_q[0], expect1));
            if (valid1 && r_q[1] !== expect1) fail($sformatf("block RAM cycle %0d: %h, model %h", c, r_q[1], expect1));
            if (valid2 && r_q[2] !== expect2) fail($sformatf("flip-flop RAM + register cycle %0d: %h, model %h", c, r_q[2], expect2));
            if (valid2 && r_q[3] !== expect2) fail($sformatf("block RAM + register cycle %0d: %h, model %h", c, r_q[3], expect2));
            r_wren = $urandom_range(0, 1);
            r_waddr = $urandom;
            r_wdata = $urandom;
            // Reads only of written words, never the word being written
            // (SUPPORTS_RW_COLLISIONS = 0: either value would be legal).
            r_raddr = $urandom;
            r_rden = (model[r_raddr] !== 'x) && !(r_wren && r_waddr == r_raddr);
            expect2 = expect1; valid2 = valid1;
            expect1 = model[r_raddr]; valid1 = r_rden;
            @(posedge clk);
            if (r_wren) model[r_waddr] = r_wdata;
            #1;
        end
        r_wren = 0; r_rden = 0;
    endtask

    //
    // LutMul
    //
    logic [5:0] m_x, m_y;
    wire  [11:0] m_uu, m_ss;
    wire  [7:0]  m_trunc;

    KanagawaLutMul #(.DW_X(6), .DW_Y(6), .DW_Z(12)) mul_uu (.x_in(m_x), .y_in(m_y), .z_out(m_uu));
    KanagawaLutMul #(.DW_X(6), .DW_Y(6), .X_SIGNED(1), .Y_SIGNED(1), .DW_Z(12)) mul_ss (.x_in(m_x), .y_in(m_y), .z_out(m_ss));
    KanagawaLutMul #(.DW_X(6), .DW_Y(6), .X_SIGNED(1), .DW_Z(8)) mul_su (.x_in(m_x), .y_in(m_y), .z_out(m_trunc));

    // At the DSP block's full width (SB_MAC16, from Yosys's cell models),
    // a signed product wider than its 32 bits, and one too wide for it.
    logic [15:0] w_x, w_y;
    logic [19:0] v_x, v_y;
    wire  [31:0] w_uu;
    wire  [39:0] w_ss;
    wire  [39:0] v_uu;
    KanagawaLutMul #(.DW_X(16), .DW_Y(16), .DW_Z(32)) mul_w_uu (.x_in(w_x), .y_in(w_y), .z_out(w_uu));
    KanagawaLutMul #(.DW_X(16), .DW_Y(16), .X_SIGNED(1), .Y_SIGNED(1), .DW_Z(40)) mul_w_ss (.x_in(w_x), .y_in(w_y), .z_out(w_ss));
    KanagawaLutMul #(.DW_X(20), .DW_Y(20), .DW_Z(40)) mul_v_uu (.x_in(v_x), .y_in(v_y), .z_out(v_uu));

    task automatic test_lutmul();
        for (int i = 0; i < 2000; i++) begin
            w_x = $urandom; w_y = $urandom; v_x = $urandom; v_y = $urandom;
            if (i == 0) begin w_x = 16'hFFFF; w_y = 16'hFFFF; end
            if (i == 1) begin w_x = 16'h8000; w_y = 16'h8000; end
            #1;
            if (w_uu !== 32'(w_x * w_y)) fail($sformatf("lutmul u16 %h*%h = %h", w_x, w_y, w_uu));
            if (w_ss !== 40'($signed(w_x) * $signed(w_y))) fail($sformatf("lutmul s16 %h*%h = %h", w_x, w_y, w_ss));
            if (v_uu !== 40'(v_x * v_y)) fail($sformatf("lutmul u20 %h*%h = %h", v_x, v_y, v_uu));
        end
        for (int x = 0; x < 64; x++) begin
            for (int y = 0; y < 64; y++) begin
                m_x = x; m_y = y; #1;
                if (m_uu !== 12'(x * y)) fail($sformatf("lutmul u%0d*u%0d = %0d", x, y, m_uu));
                if (m_ss !== 12'($signed(6'(x)) * $signed(6'(y)))) fail($sformatf("lutmul s%0d*s%0d = %h", x, y, m_ss));
                if (m_trunc !== 8'($signed(6'(x)) * $signed({1'b0, 6'(y)}))) fail($sformatf("lutmul s%0d*u%0d = %h", x, y, m_trunc));
            end
        end
    endtask

    //
    // SPRAM: four blocks deep, then four wide
    //
    logic         s_valid, s_write;
    logic [15:0]  s_addr;
    logic [63:0]  s_data;
    logic [15:0]  s_mask;
    wire  [15:0]  s_deep_q;
    wire  [63:0]  s_wide_q;

    KanagawaIce40Spram #(.Words(65536), .Width(16)) spram_deep
    (
        .clk(clk), .rst(rst), .access_valid_in(s_valid), .access_addr_in(s_addr),
        .access_write_in(s_write), .access_data_in(s_data[15:0]), .access_nibble_mask_in(s_mask[3:0]),
        .access_result_out(s_deep_q)
    );

    KanagawaIce40Spram #(.Words(16384), .Width(64)) spram_wide
    (
        .clk(clk), .rst(rst), .access_valid_in(s_valid), .access_addr_in(s_addr[13:0]),
        .access_write_in(s_write), .access_data_in(s_data), .access_nibble_mask_in(s_mask),
        .access_result_out(s_wide_q)
    );

    task automatic test_spram(input int cycles);
        logic [15:0] deep_model[int];
        logic [63:0] wide_model[int];
        logic [15:0] deep_expect;
        logic [63:0] wide_expect;
        bit check = 0;
        // A few addresses in each of the deep instance's four blocks, so
        // reads hit written words often and cross blocks every cycle.
        int addrs[16];
        for (int i = 0; i < 16; i++) addrs[i] = (i % 4) * 16384 + $urandom_range(0, 16383);
        for (int c = 0; c < cycles; c++) begin
            if (check && s_deep_q !== deep_expect) fail($sformatf("SPRAM 64Kx16 cycle %0d: %h, model %h", c, s_deep_q, deep_expect));
            if (check && s_wide_q !== wide_expect) fail($sformatf("SPRAM 16Kx64 cycle %0d: %h, model %h", c, s_wide_q, wide_expect));
            s_valid = 1;
            s_addr = addrs[$urandom_range(0, 15)];
            s_data = {$urandom, $urandom};
            s_mask = (c < 64) ? 16'hFFFF : 16'($urandom);
            // Write until every address holds a word, then mix.
            s_write = (c < 64) || $urandom_range(0, 1);
            check = !s_write && deep_model.exists(s_addr) && wide_model.exists(s_addr[13:0]);
            if (check) begin
                deep_expect = deep_model[s_addr];
                wide_expect = wide_model[s_addr[13:0]];
            end
            @(posedge clk);
            if (s_write) begin
                logic [15:0] d = deep_model.exists(s_addr) ? deep_model[s_addr] : 'x;
                logic [63:0] w = wide_model.exists(s_addr[13:0]) ? wide_model[s_addr[13:0]] : 'x;
                for (int n = 0; n < 16; n++) begin
                    if (n < 4 && s_mask[n]) d[n*4 +: 4] = s_data[n*4 +: 4];
                    if (s_mask[n]) w[n*4 +: 4] = s_data[n*4 +: 4];
                end
                deep_model[s_addr] = d;
                wide_model[s_addr[13:0]] = w;
            end
            #1;
        end
        s_valid = 0;
    endtask

    initial begin
        errors = 0;
        rst = 1;
        f_push = 0; f_pop = 0; f_data = 0;
        d_push = 0; d_pop = 0; d_data = 0; d_wrst = 1; d_rrst = 1;
        r_wren = 0; r_rden = 0; r_waddr = 0; r_raddr = 0; r_wdata = 0;
        s_valid = 0; s_write = 0; s_addr = 0; s_data = 0; s_mask = 0;
        repeat (4) @(negedge clk);
        rst = 0;
        repeat (4) @(negedge wclk); d_wrst = 0;
        repeat (4) @(negedge rclk); d_rrst = 0;
        @(negedge clk); #1;

        test_fifo(0, 4000);
        test_fifo(1, 4000);
        test_dual_clock(2000);
        @(negedge clk); #1;
        test_rams(4000);
        test_lutmul();
        @(negedge clk); #1;
        test_spram(4000);

        if (errors == 0) $display("PASS: iCE40 HAL");
        else             $display("FAIL: %0d check(s)", errors);
        $finish;
    end
endmodule

`default_nettype wire
