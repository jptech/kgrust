#!/bin/zsh
# Sample the backend worker of `kg build` for one program: start the build,
# find the worker (the child process), `sample` it until it exits.
# usage: prof.sh <name> <file> <seconds>
set -u
# The directory that held kanagawa/, kanagawa-build/ and kgrust/ when these ran.
ROOT=${ROOT:-$HOME/projects/kg}
K=$ROOT/kanagawa
R=$ROOT/kgrust/target/release
export KG_BACKEND_DIR=$ROOT/kanagawa-build/dist/bin
B=${0:A:h}
n=$1 f=$2 secs=$3
d=$B/work/$n && rm -rf $d && mkdir -p $d/out
( cd $d && exec $R/kg build -I $K/library -o out/$n $f > $B/$n.build.out 2> $B/$n.build.err ) &
parent=$!
child=""
for i in $(seq 200); do
  child=$(pgrep -P $parent | head -1)
  [[ -n $child ]] && break
  sleep 0.05
done
[[ -z $child ]] && { print "no worker found"; wait $parent; exit 1 }
sample $child $secs 1 -mayDie -file $B/$n.sample.txt > /dev/null 2>&1
wait $parent
print "$n: build exit $? (worker $child), sample in $n.sample.txt"
