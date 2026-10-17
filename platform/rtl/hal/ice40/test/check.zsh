#!/bin/zsh
# Check the iCE40 HAL (platform/rtl/hal/ice40): its behaviour against
# reference models (tb_hal.sv, Verilator, with Yosys's iCE40 cell models), and
# its storage decisions through synthesis (map_check.sv, Yosys through sv2v,
# as `kg board`'s icestorm flow runs them, crates/kg-board/src/icestorm.rs).
#
#   platform/rtl/hal/ice40/test/check.zsh
#
# Work lands in target/hal-ice40/.
emulate -L zsh
here=${0:A:h}
cd $here/../../../../..
source scripts/lib.sh
kg_env
out=target/hal-ice40
rm -rf $out; mkdir -p $out/strip

rtl=$KANAGAWA_ROOT/runtime/rtl
hal=${here:h}
src=()
for f in ${(f)"$(grep -v '^#' $rtl/files.f)"}; do src+=($rtl/$f); done
for f in ${(f)"$(grep -v '^#' $hal/files.f)"}; do src+=($hal/$f); done
cells=$(yosys-config --datdir)/ice40/cells_sim.v
result=0

# Behaviour.
if verilator --binary --timing -j 0 -DSIMULATION -Wno-fatal -Wno-lint -Wno-style --top-module tb \
    -Mdir $out/obj -o tb $cells $src $(for f in ${(f)"$(grep -v '^#' $rtl/sim/files.f)"}; print -r -- $rtl/sim/$f) \
    $here/tb_hal.sv > $out/build.log 2>&1; then
  $out/obj/tb | tee $out/run.log | grep -E "^(PASS|FAIL|ERROR)"
  grep -q "^PASS" $out/run.log || result=1
else
  grep %Error $out/build.log | head; print -u2 "check: verilator failed ($out/build.log)"; result=1
fi

# Storage decisions.
python3 -I $here/strip_translate_off.py $out/strip $src $here/map_check.sv
for top in ${(f)"$(grep -o '^module map_[a-z_]*' $here/map_check.sv | awk '{print $2}')"}; do
  want=(${=$(grep -B1 "^module $top " $here/map_check.sv | sed -n 's|^// expect \(.*\)|\1|p')})
  cell=${want[1]%=*} count=${want[1]#*=}
  policy=
  [[ ${want[2]:-} == policy ]] && policy='setattr -set ram_style "logic" m:* a:ram_style %d;'
  sv2v -DSYNTHESIS --top=$top $out/strip/*.sv -w $out/$top.v 2> $out/$top.sv2v.log || { print "FAIL $top: sv2v"; result=1; continue }
  yosys -q -l $out/$top.log -p "read_verilog -DSYNTHESIS $out/$top.v; hierarchy -top $top; $policy synth_ice40 -top $top; tee -q -o $out/$top.stat stat" > /dev/null 2>&1 \
    || { print "FAIL $top: yosys"; result=1; continue }
  got=$(awk -v c=$cell '$2 == c {n = $1} END {print n + 0}' $out/$top.stat)
  if [[ $got == $count ]]; then print "PASS $top: $cell=$got"; else print "FAIL $top: $cell=$got, want $count"; result=1; fi
done
exit $result
