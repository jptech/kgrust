#!/bin/zsh
# The per-change tier between `quick.sh` and `gates.sh`: only the gates a
# change can move, graded by their ratchets. `gates.sh` stays the milestone
# run — it is what `output`, `repoint`, `ir` and the bench need; this
# takes a minute or two.
#
#   scripts/fast.sh [--lint]          diagnostics: conformance, errors, strict, lint, cli, editions (~35 s)
#   scripts/fast.sh [--lint] walk     … and the walk: ptdiff, then backend (~80 s)
#   scripts/fast.sh [--lint] loop     … the walk, `infer-identity`, `loop-left`, `resilience`
#                                     and a quarter of `edit-identity`'s roots, its editor row
#                                     and the snapshot's shadow included (the shadow held to
#                                     its baseline's items for those roots) — for a change to
#                                     inference, the program-wide loop, a memo or the editor's
#                                     layered answers (~4 min)
#
#   --lint adds clippy over the workspace (`-D warnings`), as the gates run it.
#
# The diagnostics gates are light and run together; `ptdiff`, `backend` and
# `infer-identity` carry the memory budget and run one after another, never
# beside anything heavy — and across worktrees, the walk and loop tiers take
# the machine-wide heavy lock (`scripts/lib.sh`). Workspace tests are not run
# here: run them before committing, as ever. Runs from any worktree of this
# repository (`scripts/worktree.sh`).
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
LINT=0; TIER=diag
for a in "$@"; do
  case $a in
    --lint) LINT=1 ;;
    walk|loop) TIER=$a ;;
    *) print -u2 "usage: scripts/fast.sh [--lint] [walk|loop]"; exit 2 ;;
  esac
done
kg_env
R=(--kanagawa-root $KANAGAWA_ROOT)
T=target/gates; mkdir -p $T
t0=$(date +%s); stage() { echo "  [+$(( $(date +%s) - t0 ))s]"; }
echo "### build release"; cargo build --release -p xtask -p kg-cli 2>&1 | grep -E "^(error|warning)"; stage
X=target/release/xtask
$X conformance $R > $T/conformance.log 2>&1 & p_conf=$!
$X errors $R > $T/errors.log 2>&1 & p_err=$!
$X strict-ledger $R > $T/strict-ledger.log 2>&1 & p_sl=$!
$X lint > $T/lint.log 2>&1 & p_lint=$!
# Upstream's CLI tests and every mode's help against the reference binary
# (~2 s); it needs the reference's build, absent is red.
$X cli $R --build-dir $KANAGAWA_BUILD > $T/cli.log 2>&1 & p_cli=$!
# The language beyond the reference (~6 s): pass/fail, no ledger.
$X editions $R > $T/editions.log 2>&1 & p_ed=$!
CLIPPY=0
if (( LINT )); then
  cargo clippy --workspace --all-targets -- -D warnings > $T/clippy.log 2>&1 & p_clippy=$!
fi
wait $p_conf $p_err $p_sl $p_lint $p_cli
wait $p_ed; EDITIONS=$?
if (( LINT )); then wait $p_clippy; CLIPPY=$?; fi
echo "### conformance"; grep -E "decidable assertions" $T/conformance.log | tail -1; grep FAIL $T/conformance.log | head -5
echo "### errors"; grep -E "^\s+(agree|ours silent)\s" $T/errors.log
echo "### strict"; grep -E "^strict:" $T/strict-ledger.log
echo "### lint"; grep -E "^  [a-z-]+ +[0-9]+" $T/lint.log | awk '{printf "  %s %s", $1, $2} END {print ""}'
echo "### cli"; grep -E "^cli:|^fail |^Error" $T/cli.log | head -6
echo "### editions"; grep -E "^(editions|FAIL|Error)" $T/editions.log | head -6
if (( LINT )); then
  echo "### clippy: $(grep -cE '^(warning|error)' $T/clippy.log) diagnostics"; grep -E "^(warning|error)" $T/clippy.log | sort | uniq -c | head
fi
stage
gates=(strict errors lint cli)
if [[ $TIER != diag ]]; then
  kg_heavy_lock "fast.sh $TIER"; trap kg_heavy_unlock EXIT
  echo "### ptdiff"; $X ptdiff $R > $T/ptdiff.log 2>&1; grep -E "^\s+exact " $T/ptdiff.log; stage
  echo "### backend"; $X backend $R --build-dir $KANAGAWA_BUILD > $T/backend.log 2>&1; grep -E "^backend:" $T/backend.log; stage
  gates+=(ptdiff backend)
fi
if [[ $TIER == loop ]]; then
  echo "### infer-identity"; $X infer-identity $R > $T/infer-identity.log 2>&1
  IDENTITY=$?
  grep -E "^infer-identity:|DIFFER" $T/infer-identity.log | head -6; stage
  echo "### loop-left"; $X loop-left $R > $T/loop-left.log 2>&1
  grep -E "^  (left agree|left  |rounds agree|near the guard)" $T/loop-left.log; stage
  gates+=(loop-left)
  # The editor's answers read both engines' types (~2 s).
  echo "### resilience"; $X resilience $R > $T/resilience.log 2>&1
  grep -E "^  (hover|completion) " $T/resilience.log; stage
  gates+=(resilience)
  # Every fourth root, largest first; the milestone run takes them all.
  echo "### edit-identity"; $X edit-identity $R --every 4 > $T/edit-identity.log 2>&1
  EDITS=$?
  grep -E "^edit-identity:|edits made|the editor:|snapshot's shadow|DIFFER|SHADOW|NEWLY FAILING|^Error" $T/edit-identity.log | head -12; stage
fi
echo "### ratchet"; $X ratchet $gates 2>&1 | sed -n '/ratchets/,$p'
RC=${pipestatus[1]}
# infer-identity has no ledger: its verdict is its exit code.
if [[ $TIER == loop ]] && (( IDENTITY != 0 )); then echo "### infer-identity: FAIL"; exit 1; fi
if [[ $TIER == loop ]] && (( EDITS != 0 )); then echo "### edit-identity: FAIL"; exit 1; fi
if (( EDITIONS != 0 )); then echo "### editions: FAIL"; exit 1; fi
if (( CLIPPY != 0 )); then echo "### clippy: FAIL"; exit 1; fi
exit $RC
