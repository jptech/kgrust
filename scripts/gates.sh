#!/bin/zsh
# The full gate suite — and a gate: it exits non-zero on any regression.
#
# docs/gates.md describes every stage, what makes it red and what each gate
# is blind to; this header is the short form.
#
# Red means one of: the build or a test failed; clippy warned (`-D warnings`);
# a generated file is out of date; `fmt-check`, `corpus`, `ir` or
# `edit-identity` failed; `conformance` fell below 100%; the ratchet ledger
# disagrees with the scoreboard's own logs; `mem` is over a budget; or any
# ratchet has an item that is newly failing against `baseline.json`
# (xtask/src/ratchet.rs) — `strict`, `errors`, `lint`, `resilience`, and with
# the backend `ptdiff`, `backend`, `output`, `repoint`, `cli`, `loop-left`
# and `shadow`. A total that rose while one item fell is red. Improvements
# are green and ask to be blessed:
#   cargo xtask ratchet --bless <gate>
#
# Shape, for speed:
#   * tests under `--profile quick` (opt-level 1, debug assertions on): 34 s
#     for the workspace where `dev` took 818 s;
#   * the gates that share nothing run together on the release binary, each
#     spread over threads of its own;
#   * then `strict-ledger` (which reads the conformance logs), `ir`,
#     `resilience`, `ptdiff`, `backend`, `repoint` and `cli` together;
#     `loop-left`, `edit-identity` and `output` one after another; `bench`
#     alone and last, on a quieter machine — a tripwire, reported and never
#     failed — then `mem`, each use case's peak memory against
#     memory-budgets.json, which fails the run over a budget (`--no-bench`
#     leaves out both).
# Timings: docs/gates.md, "How long" — about 6 minutes warm, 15–30 cold.
#
# The backend gates need the reference's build: `ptdiff` and `backend` read
# its recorded runs (`cargo xtask ptrace`, after `scripts/build-backend.sh
# --reference`) and load the backend library from it; `repoint` and `cli`
# need python3, and `cli` the build's configured tests and its compiler.
# Where the build is absent they are red, not quietly skipped — say
# `--no-backend` to leave them out on purpose (their ratchets are then not
# checked).
#
# Usage: scripts/gates.sh [--no-backend] [--no-bench] [--snapshot[=<commit>]] > gates.log 2>&1
#   The last screen of the log is the summary; target/gates/status.json holds
#   it machine-readably, target/gates/*.log each stage's full output.
#
#   --snapshot runs the gates on a committed tree (default HEAD) in a
#   persistent worktree, target/snapshot/tree, so this checkout stays free to
#   edit while they run — the rule against editing under a running gate is
#   about the tree being graded. The snapshot builds into its own target/
#   (incremental after the first run) and links the data stores
#   (scripts/lib.sh). Its results are target/snapshot/tree/target/gates/; a
#   ledger names the tree it was graded from by content, so while this
#   checkout still matches the commit, `cp target/snapshot/tree/target/gates/*
#   target/gates/` lets `cargo xtask ratchet` read or bless them here.
#   Uncommitted changes are not in the snapshot.
#
# One heavy run at a time, across every worktree: the gates take the lock in
# scripts/lib.sh and wait for it if another run holds it.
setopt pipe_fail no_unset
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
BACKEND=1; BENCH=1; SNAPSHOT=
PASS=()
for a in "$@"; do
  case $a in
    --no-backend) BACKEND=0; PASS+=($a) ;;
    --no-bench) BENCH=0; PASS+=($a) ;;
    --snapshot) SNAPSHOT=HEAD ;;
    --snapshot=*) SNAPSHOT=${a#--snapshot=} ;;
    *) print -u2 "unknown flag $a"; exit 2 ;;
  esac
done
if [[ -n $SNAPSHOT ]]; then
  main=$(kg_main_root)
  snap=$main/target/snapshot/tree
  commit=$(git rev-parse --verify "$SNAPSHOT^{commit}") || exit 2
  [[ -z $(git status --porcelain --untracked-files=no) ]] || print -u2 "### snapshot: uncommitted changes here are not in it"
  kg_env
  kg_heavy_lock "gates.sh --snapshot"
  if [[ -d $snap/.git || -f $snap/.git ]]; then
    git -C $snap checkout --quiet --detach --force $commit && git -C $snap clean -fdq
  else
    mkdir -p ${snap:h} && git worktree add --quiet --detach $snap $commit
  fi
  (cd $snap && kg_link_stores)
  echo "### snapshot $(git rev-parse --short $commit) at $snap"
  exec zsh $snap/scripts/gates.sh $PASS
fi
kg_heavy_lock "gates.sh"
trap kg_heavy_unlock EXIT
# The stage timestamps are wall clock, and an idle Mac sleeps under a
# running gate: on 2026-09-23 a run read 13229 s — tests 3715, codegen 900 —
# where the same tree takes ~790 s awake. Hold idle sleep off for as long
# as this shell lives.
(( $+commands[caffeinate] )) && { caffeinate -i -w $$ & }
# The last line of defence against running the machine out of memory, which
# a session's gates and side runs did on 2026-09-23 (xtask/src/bounded.rs
# has the measurements and the per-process bounds). Below
# KG_GATES_MIN_FREE_PCT percent free system-wide (default 10; a green run
# leaves ~70 of 64 GB), name the largest processes of this run and kill every
# one of them — descendants by parent pid, not the process group, which from
# a non-interactive caller is the caller's own. The stages then read FAIL.
free_pct() {
  if [[ -r /proc/meminfo ]]; then
    awk '/^MemTotal/ {t=$2} /^MemAvailable/ {a=$2} END {print int(100*a/t)}' /proc/meminfo
  else
    sysctl -n kern.memorystatus_level 2>/dev/null || echo 100
  fi
}
descendants() {
  ps -Ao pid=,ppid= | awk -v root=$1 -v self=$2 '
    { parent[$1] = $2 }
    END { for (p in parent) { q = p; while (q in parent && q != root && q > 1) q = parent[q];
          if (q == root && p != root && p != self) print p } }'
}
memory_watchdog() {
  local floor=${KG_GATES_MIN_FREE_PCT:-10} me=$sysparams[pid] level victims
  while sleep 2 && kill -0 $1 2>/dev/null; do
    level=$(free_pct)
    (( level >= floor )) && continue
    victims=($(descendants $1 $me))
    print -u2 "### MEMORY: system free memory at ${level}% (< ${floor}%) — stopping this gate run"
    ps -o rss=,args= -p ${(j:,:)victims} 2>/dev/null | sort -rn | head -5 \
      | awk '{printf "    %6d MB  %s\n", $1/1024, substr($0, index($0,$2), 150)}' >&2
    kill -KILL $victims 2>/dev/null
    return
  done
}
zmodload zsh/system
{ memory_watchdog $$ 2>&1 | tee target/gates-memory.log >&2 } &!
kg_env
# The backend cache (docs/gates.md): re-run one hit in 20 against the real
# backend, so its premise is re-proven every milestone.
export KG_BACKEND_CACHE_VERIFY=${KG_BACKEND_CACHE_VERIFY:-20}
BUILD_DIR=$KANAGAWA_BUILD
R=(--kanagawa-root $KANAGAWA_ROOT)
T=target/gates; mkdir -p $T; rm -f $T/*.json(N) $T/*.log(N)
X=target/release/xtask
STAGES=()
t0=$(date +%s)
# note <stage> <exit code> [note]: record a stage's outcome for the summary.
note() {
  local s=ok; (( $2 == 0 )) || s=FAIL
  STAGES+=("--stage" "$1=$s:${3:-}")
  echo "### $1: $s ${3:-}  [+$(( $(date +%s) - t0 ))s]"
}
summary() {
  echo; echo "################ summary ################"
  local gates=(strict errors lint resilience); (( BACKEND )) && gates+=(ptdiff backend output repoint cli loop-left shadow)
  if [[ -x $X ]]; then $X ratchet $gates $STAGES; else echo "no xtask binary"; false; fi
  local rc=$?
  echo "[total $(( $(date +%s) - t0 ))s]"
  exit $rc
}

cargo build --release -p xtask -p kg-cli > $T/build.log 2>&1
rc=$?; note build $rc; (( rc == 0 )) || { tail -20 $T/build.log; summary; }

cargo test --profile quick --workspace > $T/test.log 2>&1
rc=$?
note tests $rc "$(awk '/^test result/ {p+=$4; f+=$6} END {print "passed",p,"failed",f}' $T/test.log)"
grep -E "^test .* FAILED|panicked" $T/test.log | head -10

cargo clippy --workspace --all-targets -- -D warnings > $T/clippy.log 2>&1
rc=$?; note clippy $rc "$(grep -cE '^(warning|error)' $T/clippy.log) diagnostics"
grep -E "^(warning|error)" $T/clippy.log | sort | uniq -c | head

# The syntax layer's files and the ABI's bindings, the latter regenerated
# from the pinned reference's headers (needs libclang; sprint W8.4).
$X codegen --check $R > $T/codegen.log 2>&1
rc=$?; note codegen $rc; (( rc == 0 )) || tail -5 $T/codegen.log

$X lint > $T/lint.log 2>&1
rc=$?; note lint $rc "$(grep -E '^  [a-z-]+ +[0-9]+' $T/lint.log | awk '{printf "%s %s, ", $1, $2}')"
grep -E "NEWLY FAILING" $T/lint.log | head -10

$X fmt-check $R > $T/fmt.log 2>&1 & p_fmt=$!
$X conformance $R > $T/conformance.log 2>&1 & p_conf=$!
$X conformance $R --strict > $T/strict.log 2>&1 & p_strict=$!
$X errors $R > $T/errors.log 2>&1 & p_err=$!
$X corpus $R > $T/corpus.log 2>&1 & p_corpus=$!
wait $p_fmt; note fmt-check $? "$(tail -1 $T/fmt.log)"
wait $p_conf; rc=$?; note conformance $rc "$(grep 'decidable assertions pass' $T/conformance.log)"
grep FAIL $T/conformance.log | head -5
# `--strict`'s verdict is the `strict` ratchet, read from its log below.
wait $p_strict
wait $p_err; rc=$?; note errors $rc "$(sed -nE 's/^  (agree|ours silent) +([0-9]+).*/\1 \2,/p' $T/errors.log | tr '\n' ' ')"
grep -E "NEWLY FAILING|GRADED FEWER|^Error" $T/errors.log | head -10
wait $p_corpus; note corpus $? "$(tail -1 $T/corpus.log | tr -s ' ')"

# The per-assertion ledger checks itself against both scoreboard logs above,
# so it starts after them, beside `ir`.
$X strict-ledger $R --against $T/conformance.log --against $T/strict.log > $T/strict-ledger.log 2>&1 & p_sl=$!
$X ir $R > $T/ir.log 2>&1 & p_ir=$!
# Hover and completion on broken buffers, as the editor answers them from
# the loop (~2 s, <1 GB; sprint W3.0, one engine since W3.7).
$X resilience $R > $T/resilience.log 2>&1 & p_res=$!
if (( BACKEND )); then
  $X ptdiff $R > $T/ptdiff.log 2>&1 & p_pt=$!
  $X backend $R --build-dir $BUILD_DIR > $T/backend.log 2>&1 & p_be=$!
  $X repoint $R --build-dir $BUILD_DIR > $T/repoint.log 2>&1 & p_rp=$!
  # Upstream's CLI tests and every mode's help (~2 s; sprint W8.6).
  $X cli $R --build-dir $BUILD_DIR > $T/cli.log 2>&1 & p_cli=$!
fi
wait $p_sl; rc=$?; note strict $rc "$(grep -E '^strict:' $T/strict-ledger.log | tr -s ' ')"
grep -E "NEWLY FAILING|GRADED FEWER|^Error|agrees with" $T/strict-ledger.log | head -10
wait $p_ir; rc=$?; note ir $rc
wait $p_res; rc=$?; note resilience $rc "$(grep -E '^  (hover|completion) ' $T/resilience.log | tr -s ' ' | tr '\n' ';')"
grep -E "NEWLY FAILING|GRADED FEWER|^Error" $T/resilience.log | head -10
grep -E "oracle cache|Error|DIFF |PROVENANCE" $T/ir.log | head -16
if (( BACKEND )); then
  wait $p_pt; rc=$?; note ptdiff $rc "$(grep -E '^  exact [0-9]' $T/ptdiff.log)"
  grep -E "NEWLY FAILING|GRADED FEWER|STALE|^Error" $T/ptdiff.log | head -10
  wait $p_be; rc=$?; note backend $rc "$(grep '^backend:' $T/backend.log | cut -d— -f2 | tr -s ' ')"
  grep -E "^binary:|NEWLY FAILING|GRADED FEWER|STALE|^Error" $T/backend.log | head -10
  wait $p_rp; rc=$?; note repoint $rc "$(grep 'upstream syntax tests pass' $T/repoint.log | cut -d' ' -f1)"
  grep -E "^binary:|NEWLY FAILING|GRADED FEWER|^Error" $T/repoint.log | head -10
  wait $p_cli; rc=$?; note cli $rc "$(grep '^cli:' $T/cli.log | cut -d' ' -f2)"
  grep -E "^fail |NEWLY FAILING|GRADED FEWER|^Error" $T/cli.log | head -10
  # After the batch: our loop over every root the reference compiles
  # (~4 GB, ~20 s) against `kg-oracle --loop`, replayed from its cache.
  $X loop-left $R > $T/loop-left.log 2>&1
  rc=$?; note loop-left $rc "$(grep -E '^  (left agree|rounds agree|near the guard)' $T/loop-left.log | tr -s ' ' | tr '\n' ';')"
  grep -E "NEWLY FAILING|GRADED FEWER|^Error" $T/loop-left.log | head -10
  # A warm `Db` after each edit against a fresh one, every root (~5 min,
  # 7 GB); any difference is red. The warm one is the editor's, and its row
  # says its last answer is `kg check`'s on every root (sprint W7.6, W7 item
  # 6's exit). Its snapshot's shadow (sprint W7.5d) writes the `shadow`
  # ledger: each kept round restored against the next run's.
  $X edit-identity $R > $T/edit-identity.log 2>&1
  rc=$?; note edit-identity $rc "$(grep -E '^edit-identity:' $T/edit-identity.log | cut -d— -f2);$(grep -E '^  the editor:' $T/edit-identity.log | sed -E 's/^  the editor: //; s/ \(the per-file.*\)//; s/;.*//')"
  grep -E "DIFFER|SHADOW|NEWLY FAILING" $T/edit-identity.log | head -10
  # Alone, after the batch: the reference compiler peaks at 7.5 GB on one
  # program and 13.7 GB across the gate's four workers when it records;
  # replaying its side, the gate stays under 4 GB (xtask/src/output.rs).
  $X output $R --build-dir $BUILD_DIR > $T/output.log 2>&1
  rc=$?; note output $rc "$(grep -E '^  identical modulo locations' $T/output.log | tr -s ' ')"
  grep -E "NEWLY FAILING|GRADED FEWER|^Error" $T/output.log | head -10
fi

if (( BENCH )); then
  echo "### bench (a tripwire: read it, it never fails the run)"
  out=$($X bench $R --filter keystroke 2>&1)
  line=$(echo "$out" | grep -E '^keystroke ')
  last=$(echo "$out" | grep -E '^fixpoint ')
  echo "$line"; echo "$last"
  # `keystroke <files> <median> <worst> <file>`: the worst to the editor's
  # first answer, and to its last (`fixpoint`, sprint W7.6), recorded for
  # `status-md`; a tripwire, so its outcome is always ok.
  note bench 0 "$(echo $line | awk '{print "worst", $4, "on", $5}'); the fixpoint $(echo $last | awk '{print "worst", $4, "on", $5}')"
  # Peak memory per use case — `kg check`, the editor opening a file and
  # after five edits — one process each, against memory-budgets.json
  # (xtask/src/mem.rs; the owner's condition on D1, 2026-10-10).
  $X mem $R > $T/mem.log 2>&1
  rc=$?; note mem $rc "$(grep -E '^mem:' $T/mem.log | sed 's/^mem: //')"
  grep -E "OVER BUDGET|^note:|^Error" $T/mem.log | head -10
fi
summary
