#!/bin/zsh
# Build the GEMM engines with `kg build`, verilate them, and co-simulate each
# against the bit-exact model in sim/.
#
#   ./run.sh [-d DEVICE]... [-f FORMAT]... [-- SIM-ARGS...]
#
# DEVICE: mock (default), ultrascaleplus-xcu250, agilex-7-f-023.
# FORMAT: fp32, bf16, int8, mxint8 (default: all four).
# SIM-ARGS go to the simulator: --seed N, --stall P, --quick.
#
# The generated RTL instantiates the device's HAL modules (RAMs, FIFOs,
# DSPs). Simulation stands the reference runtime's mock HAL in for the
# vendor's; on the U250 the HAL's DSP multiplier, which is plain behavioural
# Verilog, is the real one.
#
# Environment: KG (the `kg` binary), KG_BACKEND_DIR, KANAGAWA_ROOT (the
# reference checkout, for the SV runtime), VERILATOR.
set -euo pipefail

here=${0:A:h}
repo=${here:h:h}
: ${KANAGAWA_ROOT:=$repo/third_party/kanagawa}
: ${VERILATOR:=verilator}
if [[ -z ${KG:-} ]]; then
    if [[ -x $repo/target/release/kg ]]; then KG=$repo/target/release/kg; else KG=kg; fi
fi
if [[ -z ${KG_BACKEND_DIR:-} && -d $repo/target/backend/dist/bin ]]; then
    export KG_BACKEND_DIR=$repo/target/backend/dist/bin
fi

devices=()
formats=()
while (( $# )); do
    case $1 in
        -d) devices+=$2; shift 2 ;;
        -f) formats+=$2; shift 2 ;;
        --) shift; break ;;
        *) print -u2 "usage: $0 [-d DEVICE]... [-f FORMAT]... [-- SIM-ARGS...]"; exit 64 ;;
    esac
done
(( ${#devices} )) || devices=(mock)
(( ${#formats} )) || formats=(fp32 bf16 int8 mxint8)
sim_args=("$@")

rtl=$KANAGAWA_ROOT/runtime/rtl
typeset -A tops=(fp32 GemmFp32 bf16 GemmBf16 int8 GemmInt8 mxint8 GemmMxInt8)

# Every warning Verilator raises on the reference runtime, as upstream's own
# Verilator tests waive them (build/cmake/test_helper.cmake).
waivers=(TIMESCALEMOD WIDTHTRUNC SELRANGE WIDTHEXPAND CONSTRAINTIGN UNSIGNED INITIALDLY UNOPTFLAT
         ASCRANGE BLKANDNBLK CASEINCOMPLETE MULTIDRIVEN SHORTREAL MISINDENT WIDTHCONCAT PINCONNECTEMPTY)

failed=()
for device in $devices; do
    # The HAL: the mock's, but for the U250's DSP multipliers.
    hal=()
    for f in ${(f)"$(<$rtl/hal/mock/files.f)"}; do
        if [[ $device == ultrascaleplus-* && $f == hal_dsp.sv ]]; then
            hal+=$rtl/hal/xilinx/hal_dsp.sv
        else
            hal+=$rtl/hal/mock/$f
        fi
    done
    runtime=(${(f)"$(grep -v '^#' $rtl/files.f | sed '/^$/d' | sed "s|^|$rtl/|")"} $rtl/sim/string_table.sv)

    for format in $formats; do
        top=${tops[$format]}
        out=$here/build/$device/$format
        mkdir -p $out
        print "== $format on $device"

        print "   kg build --target-device $device"
        start=$SECONDS
        $KG build --target-device $device -o $out/ $here/src/gemm_$format.k
        print "   RTL: $(wc -l < $out/$top.sv | tr -d ' ') lines in $(( SECONDS - start ))s"

        start=$SECONDS
        $VERILATOR --cc --exe --build -j 0 --quiet \
            --top-module $top --assert -DSIMULATION ${waivers/#/-Wno-} \
            -Mdir $out/obj -o gemm_sim \
            -CFLAGS "-std=c++17 -O1 -ffp-contract=off -DFORMAT_${(U)format} -I$here/sim" \
            $runtime $hal $out/${top}_types.sv $out/$top.sv $here/sim/main.cpp \
            > $out/verilator.log 2>&1 || { tail -20 $out/verilator.log; failed+="$format/$device"; continue }
        print "   verilated in $(( SECONDS - start ))s"

        $out/obj/gemm_sim $sim_args || failed+="$format/$device"
    done
done

if (( ${#failed} )); then
    print -u2 "FAILED: $failed"
    exit 1
fi
print "all passed"
