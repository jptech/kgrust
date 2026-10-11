#!/bin/zsh
# risc-v.k's backend for a bounded time: memory every 10 s, two samples,
# then stop it (it needs ~2600 s and 13.4 GB to finish).
set -u
# The directory that held kanagawa/, kanagawa-build/ and kgrust/ when these ran.
ROOT=${ROOT:-$HOME/projects/kg}
K=$ROOT/kanagawa
R=$ROOT/kgrust/target/release
export KG_BACKEND_DIR=$ROOT/kanagawa-build/dist/bin
B=${0:A:h}
d=$B/work/riscv && rm -rf $d && mkdir -p $d/out
( cd $d && exec $R/kg build -I $K/library -o out/riscv $K/test/library/processor/risc-v.k > $B/riscv.build.out 2> $B/riscv.build.err ) &
parent=$!
child=""
for i in $(seq 400); do child=$(pgrep -P $parent | head -1); [[ -n $child ]] && break; sleep 0.05; done
[[ -z $child ]] && { print "no worker"; exit 1 }
print "worker $child" > $B/riscv.mem.txt
for t in $(seq 0 10 240); do
  print "$t s rss $(ps -o rss= -p $child | awk '{printf "%.0f MiB", $1/1024}')" >> $B/riscv.mem.txt
  if (( t == 60 )); then sample $child 20 1 -mayDie -file $B/riscv.sample1.txt > /dev/null 2>&1; fi
  if (( t == 180 )); then sample $child 20 1 -mayDie -file $B/riscv.sample2.txt > /dev/null 2>&1; fi
  (( t == 60 || t == 180 )) || sleep 10
done
kill $child 2>/dev/null; sleep 1; kill $parent 2>/dev/null
wait $parent 2>/dev/null
print "stopped" >> $B/riscv.mem.txt
