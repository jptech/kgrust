#!/bin/zsh
# The cross-build identity check as one command (CLAUDE.md, "How a piece of
# work goes", step 5; sprint W6): every corpus root's elaborated fingerprint
# — types, trajectories, every fixpoint's unresolved counts
# (`kg_db::looped::fingerprint`) — as this checkout builds it and as
# <commit> builds it, compared file by file.
#
#   scripts/identity.sh [<commit>] [--filter <substring>]
#
# <commit> defaults to HEAD, so an uncommitted optimisation is graded
# against the tree it was made on. The base builds in a persistent worktree,
# `target/identity-base`, moved to <commit> each run and linked to the
# shared stores (`scripts/worktree.sh`), so after the first build it is
# incremental. Exits 1 naming each root whose fingerprint differs; an
# exact optimisation lands with this at zero (and `cargo xtask
# infer-identity`, both inference modes in one build, green). Takes the
# machine-wide heavy lock (`scripts/lib.sh`).
setopt err_exit no_unset pipe_fail
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
kg_env

BASE=HEAD
FILTER=()
while (( $# )); do
  case $1 in
    --filter) FILTER=(--filter "$2"); shift 2 ;;
    -*) print -u2 "usage: scripts/identity.sh [<commit>] [--filter <substring>]"; exit 2 ;;
    *) BASE=$1; shift ;;
  esac
done

ROOT=$PWD
WT=$ROOT/target/identity-base
OUT=$ROOT/target/identity
COMMIT=$(git rev-parse --verify "$BASE^{commit}")

kg_heavy_lock "identity.sh against ${COMMIT[1,8]}"; trap kg_heavy_unlock EXIT

if [[ -d $WT ]]; then
  git -C $WT checkout --quiet --detach $COMMIT
else
  scripts/worktree.sh add $WT $COMMIT > /dev/null
fi
rm -rf $OUT; mkdir -p $OUT/base $OUT/here

print "### base ${COMMIT[1,8]}: building and dumping"
(cd $WT && cargo -q xtask infer-identity --kanagawa-root $KANAGAWA_ROOT --dump $OUT/base $FILTER) | tail -1
print "### this checkout: building and dumping"
cargo -q xtask infer-identity --kanagawa-root $KANAGAWA_ROOT --dump $OUT/here $FILTER | tail -1

differ=()
for f in $OUT/base/*(N:t) $OUT/here/*(N:t); do
  [[ ${differ[(Ie)$f]} -eq 0 ]] || continue
  if ! cmp -s $OUT/base/$f $OUT/here/$f; then
    differ+=($f)
  fi
done
total=$(ls $OUT/here | wc -l | tr -d ' ')
if (( ${#differ} )); then
  print "identity: ${#differ} of $total roots differ from ${COMMIT[1,8]} (dumps under target/identity/{base,here}):"
  for f in $differ; do print "  DIFFER ${f//__//}"; done
  exit 1
fi
print "identity: $total roots identical to ${COMMIT[1,8]}"
