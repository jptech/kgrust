#!/bin/zsh
# Synthesises each engine alone, with its default parameters, for the
# iCE40 UP5K (sv2v, then Yosys synth_ice40, as `kg board` runs it), and prints its cells.
# Outputs go to $OUT, by default target/synth/ beside this script.
set -eu
here=${0:A:h}
out=${OUT:-$here/target/synth}
mkdir -p $out
for top in spi_master i2c_master; do
    sv2v $here:h/$top.sv > $out/$top.v
    yosys -q -p "read_verilog $out/$top.v; synth_ice40 -top $top; tee -q -o $out/$top.stat stat"
    echo "$top:"
    grep -E '^ +[0-9]+ +(SB_|cells)' $out/$top.stat
done
