#!/bin/zsh
# Plants one bug at a time in a copy of an engine, runs its testbench on the
# copy, and requires it to go red. The sources here are never touched: each
# mutant lives in a fresh directory under target/mutants.*, removed afterwards. A
# mutation whose sed changes nothing, or whose copy does not build, counts
# as a failure of this script, not as a catch. Mutants build with lint
# warnings non-fatal: a planted bug often leaves a signal unused.
#
# Usage: mutate.sh        exits 0 if every mutant was caught
set -u
here=${0:A:h}
engines=$here:h
mkdir -p $here/target
tmp=$(mktemp -d "$here/target/mutants.XXXXXX")
trap 'rm -rf $tmp' EXIT
escaped=0
n=0

mutant() {   # testbench (spi|i2c), file, description, sed expression
    local tb=$1 file=$2 what=$3 expr=$4
    local dir=$tmp/m$((++n))
    mkdir -p $dir
    cp $engines/*.sv $dir/
    sed -e "$expr" $engines/$file > $dir/$file
    if cmp -s $engines/$file $dir/$file; then
        echo "M$n not applied: $file: $what"
        escaped=1
        return
    fi
    OUT=$dir/obj VFLAGS=-Wno-fatal $here/run.sh $tb $dir > $dir/run.log 2>&1
    case $? in
        1) echo "M$n caught: $file: $what"
           { grep -m 1 ERROR $dir/run.log || grep -m 1 FAIL $dir/run.log } | sed 's/^/      /' ;;
        0) echo "M$n ESCAPED: $file: $what"; escaped=1 ;;
        *) echo "M$n did not build: $file: $what"; escaped=1 ;;
    esac
}

mutant spi spi_master.sv "first bit taken from the wrong end (LSB first out)" \
    's/mosi <= call_mosi\[7\]/mosi <= call_mosi[0]/'
mutant spi spi_master.sv "received bits assembled in the wrong order" \
    's/shift <= {shift\[6:0\], miso}/shift <= {miso, shift[7:1]}/'
mutant spi spi_master.sv "chip select released after every byte, hold ignored" \
    "s/step == 5'd16 \&\& !hold/step == 5'd16/"
mutant spi spi_master.sv "sampling and shifting on the wrong edges" \
    's/(step\[0\] == CPHA)/(step[0] != CPHA)/'
mutant spi spi_master.sv "SCLK reset to 0 whatever CPOL" \
    "s/sclk <= CPOL;/sclk <= 1'b0;/"
mutant spi spi_master.sv "no half-period between chip select and the first edge" \
    "/step <= 5'd0;/,/count <= RELOAD;/ s/count <= RELOAD;/count <= '0;/"
mutant spi spi_master.sv "chip select released with the last edge, no half-period after" \
    "s/sclk <= !sclk;/sclk <= !sclk; if (step == 5'd15 \&\& !hold) cs_n <= 1'b1;/"

mutant i2c i2c_master.sv "ACK polarity inverted in a WRITE's result" \
    's/{shift\[0\], 8.h00}/{!shift[0], 8'"'"'h00}/'
mutant i2c i2c_master.sv "no wait for a stretched SCL" \
    "s/(quarter == 3'd1) \&\& !scl/1'b0/"
mutant i2c i2c_master.sv "READ acknowledges the last byte and not the others" \
    "s/{8'hFF, !call_data\[0\]}/{8'hFF, call_data[0]}/"
mutant i2c i2c_master.sv "START and STOP move SDA the wrong way" \
    's/sda_oe <= (op == OP_START);/sda_oe <= (op == OP_STOP);/'
mutant i2c i2c_master.sv "data bits sent from the wrong end" \
    's/sda_oe <= !shift\[8\];/sda_oe <= !shift[0];/'

exit $escaped
