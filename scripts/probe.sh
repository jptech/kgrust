#!/bin/zsh
# One program, every verdict — the triage loop for "why does this program
# diverge?", in seconds rather than a gate run per question.
#
#   scripts/probe.sh <program>
#
# <program> is a gate's name for it — `syntax::basics::110` (an embedded
# assertion), `file::test/library/data/array.k` (a corpus file) — or a path
# to a corpus file (`test/library/data/array.k`, or absolute under the
# reference checkout).
#
# Prints, for that one program: the conformance verdict and --strict's; the
# `errors` comparison (our first error against `kg-oracle --errors`); the
# first divergence of our ParseTree walk from the reference's (`ptdiff`);
# both compilers' whole output through the real backend (`backend --show`,
# replayed from the verdict cache when it can be); and, for a corpus file,
# `output`'s comparison of the SystemVerilog. Every log is kept under
# target/probe/<program>/. Runs from any worktree (scripts/worktree.sh).
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
[[ -n ${1:-} ]] || { print -u2 "usage: scripts/probe.sh <syntax::file::n | file::path | path>"; exit 2 }
kg_env
R=(--kanagawa-root $KANAGAWA_ROOT)
# The backend build, named: from a worktree the default is not beside it.
B=(--build-dir $KANAGAWA_BUILD)
name=$1
if [[ $name != syntax::* && $name != file::* ]]; then
  name=${name#$KANAGAWA_ROOT/}
  name=file::${name#./}
fi
P=target/probe/${name//[:\/]/_}; mkdir -p $P
X=target/release/xtask
cargo build --release -p xtask -p kg-cli 2>&1 | grep -E "^(error|warning)"
# A name as a whole word: `syntax::basics::11` is not `::110`.
quoted=$(print -r -- $name | sed 's/[][\\.*^$()+?{}|]/\\&/g')
exact() { grep -E -- "${quoted}([^0-9]|\$)" }

if [[ $name == syntax::* ]]; then
  $X conformance $R --filter $name > $P/conformance.log 2>&1 &
  $X conformance $R --strict --filter $name > $P/strict.log 2>&1 &
  $X errors $R --filter $name --verbose > $P/errors.log 2>&1 &
fi
$X ptdiff $R --filter $name --verbose > $P/ptdiff.log 2>&1 &
[[ $name == file::* ]] && { $X output $R $B --filter $name --verbose > $P/output.log 2>&1 & }
wait

print "### $name"
if [[ $name == syntax::* ]]; then
  print "### conformance"; grep -E "decidable assertions" $P/conformance.log | tail -1; exact < $P/conformance.log | grep FAIL
  print "### conformance --strict"; if exact < $P/strict.log | grep -q FAIL; then exact < $P/strict.log | grep FAIL; else print "  passes"; fi
  print "### errors"
  if grep -qE "^[[:space:]]+(SILENT|DIFF)" $P/errors.log; then
    sed -nE "/^[[:space:]]+(SILENT|DIFF) ${quoted}([^0-9]|\$)/,/^[[:space:]]*\$/p" $P/errors.log
  else
    grep -E "^\s+(agree|parse agree|parse-time table)\s+[1-9]" $P/errors.log | sed 's/^/ /' || print "  not a must-reject snippet"
  fi
fi
print "### ptdiff"
if grep -q "nothing to compare" $P/ptdiff.log; then
  print "  not walked: the reference never reached its backend (or the program is unrecorded)"
else
  grep -E "^\s+exact [0-9]" $P/ptdiff.log
  grep -E "^\s+(DIVERGED|DECLINED|REJECTED|SHORT|FAILED) " $P/ptdiff.log | exact
  grep -E "^\s+(ours|theirs):" $P/ptdiff.log | head -2 | cut -c1-400
fi
if [[ $name == file::* ]]; then
  print "### output"
  if grep -qE "programs with RTL of 0 |nothing to compare" $P/output.log; then
    print "  the reference writes no RTL for it"
  else
    grep -E "^\s+identical modulo" $P/output.log; grep -E "^\s+(DIFFER|DECLINED|FAILED)" $P/output.log | exact | cut -c1-400
  fi
fi
print "### backend (both compilers' whole output)"
$X backend $R $B --show $name > $P/backend.log 2>&1
grep -v "^ptrace store:" $P/backend.log | sed '/^--- theirs/,$d' | grep -v "Finished \`release\`" | head -40
sed -n '/^--- theirs/,$p' $P/backend.log | head -20
print "\nlogs: $P/"
