// The clock for a `serve` testbench kg board generates (crates/kg-board/src/
// bench.rs): `tb` takes its clock as a port, and this loop toggles it as
// fast as Verilator evaluates. Three times the speed of the same testbench
// clocking itself with `#` delays under --timing (pong on the virtual board:
// 11.7 MHz against 3.9, 2026-10-17), which is what makes its screen live.

#include <memory>

#include "Vtb.h"
#include "verilated.h"

int main(int argc, char** argv) {
    auto context = std::make_unique<VerilatedContext>();
    context->commandArgs(argc, argv);
    auto top = std::make_unique<Vtb>(context.get());
    while (!context->gotFinish()) {
        top->clk = 0;
        top->eval();
        top->clk = 1;
        top->eval();
    }
    top->final();
    return 0;
}
