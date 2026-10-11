#!/bin/zsh
# The inner loop: one test filter under the quick profile — over the crates
# the frontend's work is in: kg-db, kg-ty, kg-hir, kg-sem, kg-ptbridge — then
# conformance and errors on the rebuilt release binary, together. About half
# a minute. Runs from any worktree (scripts/worktree.sh).
# Usage: scripts/quick.sh <test-name-filter>
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
kg_env
R=(--kanagawa-root $KANAGAWA_ROOT)
T=target/gates; mkdir -p $T
t0=$(date +%s); stage() { echo "  [+$(( $(date +%s) - t0 ))s]"; }
echo "### tests ($1)"; cargo test --profile quick -p kg-db -p kg-ty -p kg-hir -p kg-sem -p kg-ptbridge -- "$1" 2>&1 | grep -E "^test .*FAILED|test result: FAILED|panicked|left:|right:|^error"; stage
echo "### build release"; cargo build --release -p xtask -p kg-cli 2>&1 | grep -E "^(error|warning)"; stage
X=target/release/xtask
$X conformance $R > $T/conformance 2>&1 &
$X errors $R > $T/errors 2>&1 &
wait
echo "### conformance"; tail -1 $T/conformance; grep FAIL $T/conformance | head -5
echo "### errors"; grep -E "^\s+(agree|ours silent)\s|disagreements|oracle cache" $T/errors; stage
