#!/bin/zsh
# Builds and runs the testbenches, printing each one's PASS, FAIL and first
# ERROR lines. Exits 0 if every one passed, 1 if any failed, 2 if any did
# not build (a build failure is not a test failure: mutate.sh relies on
# telling them apart).
#
# Usage: run.sh [spi|i2c|all [engine directory]]
# The engines are read from the directory given (default: platform/rtl/periph,
# above this one), the testbenches from here, the models of the devices on
# the far end from platform/sim/models.
# Builds go to $OUT, by default target/obj/ beside this script; $VFLAGS adds to
# verilator's arguments.
set -u
here=${0:A:h}
which=${1:-all}
src=${2:-$here:h}
models=$here:h:h:h/sim/models
out=${OUT:-$here/target/obj}
mkdir -p $out
failed=0
broken=0

run() {   # name, then verilator arguments
    local name=$1; shift
    if ! verilator --binary --timing -j 0 -Wall ${=VFLAGS:-} --top-module tb -Mdir $out/$name "$@" \
            > $out/$name.build.log 2>&1; then
        echo "BUILD FAILED $name (see $out/$name.build.log)"
        broken=1
        return
    fi
    $out/$name/Vtb > $out/$name.log 2>&1
    { grep -E '^(PASS|FAIL)' $out/$name.log; grep -m 3 '^ERROR' $out/$name.log } | sed "s/^/[$name] /"
    grep -q '^PASS' $out/$name.log || failed=1
}

if [[ $which == spi || $which == all ]]; then
    run spi $here/tb_spi.sv $here/tb_call_driver.sv $src/spi_master.sv $models/spi_flash_model.sv
fi
if [[ $which == i2c || $which == all ]]; then
    # bus speed (CLK_DIV) and clock stretch: stretches longer than the
    # master's low phase (2 * CLK_DIV) are the ones that make it wait
    for div stretch in 60 0  8 0  8 37  60 300  2 37; do
        run i2c_${div}_${stretch} -GCLK_DIV=$div -GSTRETCH=$stretch \
            $here/tb_i2c.sv $here/tb_call_driver.sv $src/i2c_master.sv $models/i2c_target_model.sv
    done
fi
(( broken )) && exit 2
exit $failed
