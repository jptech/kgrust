#!/bin/zsh
# The standing against the reference (`docs/plans/performance.md` §11):
# the reference's frontend against our `kg check`, per program, best of N.
#
#   scripts/vs-haskell.sh            # §11's seven programs, three runs each
#   N=1 scripts/vs-haskell.sh cache.k array.k   # a subset, by file name
#
# Three columns: the reference on one core, the reference on every core
# (as the shipped compiler runs), and ours (`kg check`, single threaded).
#
# The reference side is `kg-oracle --errors` — the reference's own Haskell
# library through every frontend pass — built *as the shipped executable is
# built* (`-threaded -feager-blackholing -with-rtsopts=-A128M`,
# `compiler/kanagawa.cabal:24-29`), which the gates' oracle is not: it is a
# copy of `oracle/` with that one `ghc-options` line, in
# `target/oracle-bench`, built once (about two minutes) and reused while the
# reference's sources are unchanged. The gates' oracle and its cache are
# never touched. Our side is the release `kg`, built first.
#
# Cores: the shipped compiler sets one capability per processor
# (`app/Main.hs:62-68`, `--jobs` unset) and its frontend runs passes per
# module in parallel (`Template.hs:57-78`, `Symbols.hs:41`); `oracle/driver`
# sets none. So the timing copy is built with `-rtsopts` and run twice:
# `+RTS -N1` (one core, as `kg` runs) and `+RTS -N` (every core, as
# shipped). Until 2026-10-11 it ran on one core only, unlabelled.
#
# Runs interleave per program (one core, every core, ours, one core, …), each
# in a fresh directory outside the reference tree, timed by `/usr/bin/time`
# (`-l` on macOS, `-v` on Linux, where it is `apt install time`). Read only
# a quiet machine's numbers: this is the comparison `AGENTS.md` asks to be
# interleaved and quiet. Takes about twelve minutes for the full set at
# N=3, nearly all of it the reference's.
# Prints one row per run, then each program's best (MiB as `time` reports
# bytes / 2^20); the raw log is target/vs-haskell/runs.txt.
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
kg_env
set -u
K=$KANAGAWA_ROOT
N=${N:-3}
W=$PWD/target/vs-haskell
B=$PWD/target/oracle-bench
export PATH="$HOME/.ghcup/bin:$PATH"

# The timing oracle: rebuilt when the reference's frontend sources differ
# from the copy it was built from.
FLAGS='-w -threaded -feager-blackholing -rtsopts -with-rtsopts=-A128M'
if ! diff -rq $K/compiler/hs/lib $B/lib >/dev/null 2>&1 || [[ -z $(find $B/dist-newstyle -name kg-oracle -type f -perm -u=x 2>/dev/null) ]] \
   || ! grep -qF -- "$FLAGS" $B/oracle.cabal; then
  print -u2 "building the timing oracle in $B (about two minutes)"
  rm -rf $B && mkdir -p $B
  cp -R oracle/driver oracle/oracle.cabal $B/ && cp -R $K/compiler/hs/lib $B/lib
  grep -q '^  ghc-options:         -w$' $B/oracle.cabal || { print -u2 "oracle.cabal's executable ghc-options moved; update this script"; exit 1 }
  sed -i.orig "s/^  ghc-options:         -w\$/  ghc-options:         $FLAGS/" $B/oracle.cabal && rm $B/oracle.cabal.orig
  ( cd $B && cabal build -v0 kg-oracle ) || exit 1
fi
H=$(find $B/dist-newstyle -name kg-oracle -type f -perm -u=x | head -1)
cargo build -q --release -p kg-cli --bin kg || exit 1
R=$PWD/target/release/kg

rm -rf $W && mkdir -p $W/run
print -r -- $'inline uint32 add_one(uint32 x)\n{\n    return x + 1;\n}' > $W/tiny.k
all=($W/tiny.k $K/test/library/data/{cache,bits,vector,array}.k $K/test/library/numeric/float.k $K/test/library/processor/risc-v.k)
progs=()
for p in $all; do
  (( $# == 0 )) || (( ${@[(Ie)${p:t}]} )) && progs+=$p
done

# BSD time (macOS) reports with -l, in bytes; GNU time (Linux, `apt install
# time`) with -v, in KiB, and has no footprint.
[[ $OSTYPE == darwin* ]] && TIME_FLAG=-l || TIME_FLAG=-v
measure() { # side program cmd… → "program side real user rss footprint exit"
  local side=$1 p=$2; shift 2
  ( cd $W/run && /usr/bin/time $TIME_FLAG "$@" > $W/run/out.txt 2> $W/run/time.txt ); local rc=$?
  awk -v p=${p:t} -v s=$side -v rc=$rc '
    / real /{real=$1; user=$3}
    /maximum resident set size/{rss=$1/1048576}
    /peak memory footprint/{fp=$1/1048576}
    /Elapsed \(wall clock\)/{n=split($NF, t, ":"); real=(n==3 ? t[1]*3600+t[2]*60+t[3] : t[1]*60+t[2])}
    /User time \(seconds\)/{user=$NF}
    /Maximum resident set size \(kbytes\)/{rss=$NF/1024}
    END{printf "%s %s %.2f %.2f %.0f %.0f %d\n", p, s, real, user, rss, fp, rc}' $W/run/time.txt
}

{
  print "# $(git rev-parse --short HEAD), $( (sysctl -n hw.ncpu || nproc) 2>/dev/null) cores, load $(uptime | sed 's/.*load averages*: //')"
  print "# program side real user rss_mib footprint_mib exit"
  for p in $progs; do
    for i in $(seq $N); do
      measure haskell-1 $p $H +RTS -N1 -RTS -I $K/library --base $K/library/base.k --errors $p
      measure haskell-N $p $H +RTS -N -RTS -I $K/library --base $K/library/base.k --errors $p
      measure rust $p $R check -I $K/library $p
    done
  done
  print "# load after: $(uptime | sed 's/.*load averages*: //')"
} | tee $W/runs.txt

print "\nbest of $N, seconds (× = the reference's time over ours), peak RSS MiB:"
printf "  %-12s %9s %9s %7s %5s %5s   %s\n" program "hs 1 core" "hs all" rust "×1" "×all" "rss hs 1 / hs all / rust"
awk '!/^#/ {
  k=$1; s=$2; if ($7 != 0) bad[k]=bad[k] " " s " exit " $7
  key=k SUBSEP s
  if (!(key in t) || $3<t[key]) t[key]=$3
  if (!(key in m) || $5<m[key]) m[key]=$5
  if (!(k in seen)) { seen[k]=1; order[n++]=k }
} END { for (i=0;i<n;i++) { k=order[i]; a=t[k,"haskell-1"]; b=t[k,"haskell-N"]; r=t[k,"rust"]
  printf "  %-12s %8.2fs %8.2fs %6.2fs %4.0fx %4.0fx   %d / %d / %d%s\n", k, a, b, r, a/r, b/r, m[k,"haskell-1"], m[k,"haskell-N"], m[k,"rust"], (k in bad ? "  (non-zero:" bad[k] ")" : "") } }' $W/runs.txt
