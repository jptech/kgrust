#!/bin/zsh
# One program at a time, sequentially: our frontend profiled (kg check),
# our full build (kg build, KG_PHASES), our build through the compat binary
# with the backend's own stage timers (--verbose), and the reference's full
# compiler the same way. Every run in a fresh directory outside ../kanagawa.
set -u
# The directory that held kanagawa/, kanagawa-build/ and kgrust/ when these ran.
ROOT=${ROOT:-$HOME/projects/kg}
K=$ROOT/kanagawa
R=$ROOT/kgrust/target/release
H=$ROOT/kanagawa-build/dist/bin/kanagawa
export KG_BACKEND_DIR=$ROOT/kanagawa-build/dist/bin
B=${0:A:h}
L=$B/log
mkdir -p $L
print -r -- $'inline uint32 add_one(uint32 x)\n{\n    return x + 1;\n}' > $B/tiny.k

typeset -A prog
prog=(tiny $B/tiny.k cache $K/test/library/data/cache.k bits $K/test/library/data/bits.k
      array $K/test/library/data/array.k float $K/test/library/numeric/float.k
      riscv $K/test/library/processor/risc-v.k)

run() { # name label cmd…
  local n=$1 l=$2; shift 2
  local d=$B/work/$n-$l
  rm -rf $d && mkdir -p $d/out
  ( cd $d && /usr/bin/time -l "$@" > $L/$n-$l.out 2> $L/$n-$l.err ); local rc=$?
  print "$n $l exit $rc $(awk '/ real /{print $1"s"} /peak memory footprint/{printf " peak %.0fMiB", $1/1048576}' $L/$n-$l.err | tr -d '\n')"
}

print "# start $(date '+%H:%M:%S'), load $(uptime | sed 's/.*load averages*: //')"
(( $# )) || set -- tiny cache bits array float riscv
for n in "$@"; do
  p=$prog[$n]
  KG_PHASES=1 KG_LOOP_PROFILE=1 run $n check $R/kg check -I $K/library $p
  [[ $n == riscv ]] && continue   # the backend cannot build it inside 8 GB / 900 s (gates), nor can the reference
  KG_PHASES=1 run $n build $R/kg build -I $K/library -o out/$n $p
  run $n compat $R/kanagawa --verbose --import-dir $K/library --backend=sv -o out/$n $p
  run $n reference $H --verbose --import-dir $K/library --backend=sv -o out/$n $p
done
print "# end $(date '+%H:%M:%S'), load $(uptime | sed 's/.*load averages*: //')"
