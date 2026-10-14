// Drives an exported Kanagawa class through its ports, one clock at a time.
//
// Every exported method has the same handshake. Arguments go in on
// `<m>_valid_in` with `<m>_<arg>_in`, taken on a rising edge where
// `<m>_rdy_out` is high. Results, or for a `void` method a completion, come
// out of a show-ahead FIFO: `<m>_result_out` is valid while `<m>_empty_out`
// is low, and is popped by `<m>_rden_in` on the edge.
//
// Every engine exports the same five methods, so one template drives any of
// them.
#pragma once

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <deque>
#include <functional>
#include <random>
#include <type_traits>
#include <utility>

#include "formats.h"
#include "verilated.h"

template <typename P>
void drive(P& port, const Bits& b) {
    if constexpr (std::is_integral_v<P>) {
        port = P(b.get(0, 8 * sizeof(P)));
    } else {
        for (size_t i = 0; i < sizeof(P) / sizeof(port[0]); ++i) port[i] = i < b.w.size() ? b.w[i] : 0;
    }
}

template <typename P>
Bits sample(const P& port) {
    Bits b;
    if constexpr (std::is_integral_v<P>) {
        b.put(0, 8 * sizeof(P), uint64_t(port));
    } else {
        b.w.assign(sizeof(P) / sizeof(port[0]), 0);
        for (size_t i = 0; i < b.w.size(); ++i) b.w[i] = port[i];
    }
    return b;
}

struct Geometry {
    unsigned kv, lanes, max_m, max_n, max_k;
};

template <typename VTop>
class Dut {
public:
    using Setter = std::function<void(VTop*)>;

    Dut(VerilatedContext* ctx, double stall, uint32_t seed) : top_(new VTop(ctx)), stall_(stall), rng_(seed) {}
    ~Dut() {
        top_->final();
        delete top_;
    }

    uint64_t cycle() const { return cycle_; }

    void reset() {
        top_->rst = 1;
        for (int i = 0; i < 10; ++i) tick();
        top_->rst = 0;
        run_until([&] { return top_->rst_and_startup_done_out != 0; }, "reset");
    }

    Geometry geometry() {
        geometry_.push_back([](VTop*) {});
        run_until([&] { return geometry_results_.size() == 1; }, "geometry");
        Bits r = geometry_results_.front();
        geometry_results_.clear();
        return {unsigned(r.get(0, 16)), unsigned(r.get(16, 16)), unsigned(r.get(32, 16)), unsigned(r.get(48, 16)),
                unsigned(r.get(64, 16))};
    }

    // Calls are queued; `wait_*` runs the clock until every queued call of
    // that method has returned.
    void load_a(unsigned row, unsigned kc, Bits data) {
        load_a_.push_back([=](VTop* t) {
            t->load_a_row_in = row;
            t->load_a_kc_in = kc;
            drive(t->load_a_data_in, data);
        });
        ++load_a_issued_;
    }
    void load_b(unsigned row, unsigned kc, Bits data) {
        load_b_.push_back([=](VTop* t) {
            t->load_b_row_in = row;
            t->load_b_kc_in = kc;
            drive(t->load_b_data_in, data);
        });
        ++load_b_issued_;
    }
    void wait_loads() {
        run_until([&] { return load_a_done_ == load_a_issued_ && load_b_done_ == load_b_issued_; }, "load");
    }

    // Returns the cycles from the call's acceptance to its return.
    uint64_t compute(unsigned m, unsigned strips, unsigned chunks, bool accumulate) {
        compute_.push_back([=](VTop* t) {
            t->compute_m_in = m;
            t->compute_strips_in = strips;
            t->compute_chunks_in = chunks;
            t->compute_accumulate_in = accumulate;
        });
        run_until([&] { return compute_.empty(); }, "compute accepted");
        uint64_t start = cycle_;
        size_t want = compute_done_ + 1;
        run_until([&] { return compute_done_ == want; }, "compute");
        return cycle_ - start;
    }

    void read_c(unsigned row, unsigned strip) {
        read_c_.push_back([=](VTop* t) {
            t->read_c_row_in = row;
            t->read_c_strip_in = strip;
        });
        ++read_c_issued_;
    }
    std::deque<Bits> wait_reads() {
        run_until([&] { return read_c_results_.size() == read_c_issued_; }, "read_c");
        read_c_issued_ = 0;
        return std::exchange(read_c_results_, {});
    }

private:
    VTop* top_;
    double stall_;
    std::mt19937 rng_;
    uint64_t cycle_ = 0;

    std::deque<Setter> geometry_, load_a_, load_b_, compute_, read_c_;
    std::deque<Bits> geometry_results_, read_c_results_;
    size_t load_a_issued_ = 0, load_b_issued_ = 0, read_c_issued_ = 0;
    size_t load_a_done_ = 0, load_b_done_ = 0, compute_done_ = 0;

    bool stalled() { return stall_ > 0 && std::uniform_real_distribution<double>(0, 1)(rng_) < stall_; }

    void tick() {
        top_->clk = 1;
        top_->eval();
        top_->clk = 0;
        top_->eval();
        ++cycle_;
    }

    template <typename Pred>
    void run_until(Pred done, const char* what) {
        const uint64_t limit = cycle_ + 2000000;
        while (!done()) {
            if (cycle_ > limit) {
                std::fprintf(stderr, "timeout waiting for %s at cycle %llu\n", what, (unsigned long long)cycle_);
                std::exit(2);
            }
            step();
        }
    }

    // One clock: offer the head of each call queue (unless a random stall
    // holds it back), drain each result FIFO (likewise), and see on the edge
    // what was taken.
    void step() {
        VTop* t = top_;
        auto offer = [&](std::deque<Setter>& q) {
            bool v = !q.empty() && !stalled();
            if (v) q.front()(t);
            return v;
        };
        t->geometry_valid_in = offer(geometry_);
        t->load_a_valid_in = offer(load_a_);
        t->load_b_valid_in = offer(load_b_);
        t->compute_valid_in = offer(compute_);
        t->read_c_valid_in = offer(read_c_);
        t->stall_rate_valid_in = 0;
        t->stall_rate_in = 0;
        t->eval();

        auto drain = [&](auto empty) { return !empty && !stalled(); };
        t->geometry_rden_in = drain(t->geometry_empty_out);
        t->load_a_rden_in = drain(t->load_a_empty_out);
        t->load_b_rden_in = drain(t->load_b_empty_out);
        t->compute_rden_in = drain(t->compute_empty_out);
        t->read_c_rden_in = drain(t->read_c_empty_out);
        t->eval();

        // Sample what the edge will take.
        bool took_geometry = t->geometry_valid_in && t->geometry_rdy_out;
        bool took_load_a = t->load_a_valid_in && t->load_a_rdy_out;
        bool took_load_b = t->load_b_valid_in && t->load_b_rdy_out;
        bool took_compute = t->compute_valid_in && t->compute_rdy_out;
        bool took_read_c = t->read_c_valid_in && t->read_c_rdy_out;
        if (t->geometry_rden_in) geometry_results_.push_back(sample(t->geometry_result_out));
        if (t->read_c_rden_in) read_c_results_.push_back(sample(t->read_c_result_out));
        load_a_done_ += t->load_a_rden_in;
        load_b_done_ += t->load_b_rden_in;
        compute_done_ += t->compute_rden_in;

        tick();

        if (took_geometry) geometry_.pop_front();
        if (took_load_a) load_a_.pop_front();
        if (took_load_b) load_b_.pop_front();
        if (took_compute) compute_.pop_front();
        if (took_read_c) read_c_.pop_front();
    }
};
