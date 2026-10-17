#!/bin/zsh
# Every example through every flow it has, with `kg board`: what to run
# after a change to the platform layer, `kg board` or the backend's options.
#
#   platform/check.zsh [<example dir>…]
#
# The examples are each a project: the boards' own (platform/boards/*/
# examples), the platform's (platform/examples, for any board) and the
# tooling's own tests (platform/test). In each, as it has them:
# - `kg board sim`, with a testbench;
# - `netsim`, `bitstream` and `area`, for a board with an open flow;
# - `kg board test`, with test_*.py, when $KG_COCOTB_PYTHON (default
#   python3) has cocotb 2;
# - each test_*.py over the bridge under `kg board serve`, and a host.py's
#   own test;
# - a check_panel.py through the front panel: buttons, LEDs, console,
#   screen.
# Then the platform's examples on every other board (`kg board new`), the
# peripheral engines' tests and the iCE40 HAL's check. Prints a line per
# step and exits non-zero if any failed. Logs land in each project's
# target/board/.
emulate -L zsh
cd "$(dirname "$0")/.." || exit 1
source scripts/lib.sh
kg_env

cargo build --profile quick --bin kg 2>&1 | grep -E "^(error|warning)"
kg=${PWD}/target/quick/kg
python=${KG_COCOTB_PYTHON:-python3}
# A worktree's own submodule is empty: the examples' path to the standard
# library reaches nothing there, and this one serves.
export KG_IMPORT_DIRS=$KANAGAWA_ROOT/library

examples=("$@")
(( ${#examples} )) || examples=(platform/boards/*/examples/*(/) platform/examples/*(/) platform/test/*(/))
failed=0
step() {
  local dir=$1 name=$2; shift 2
  local log=$dir/target/board/check-$name.log
  mkdir -p $dir/target/board
  if (cd $dir && "$@") > $log 2>&1; then
    print -r -- "pass  $dir $name  $(grep -E '^(PASS|  ICESTORM_LC|  fmax|[A-Za-z_]+: LUT4|  TESTS=)' $log | tr -s ' ' | paste -sd ';' -)"
  else
    print -r -- "FAIL  $dir $name  ($log)"
    failed=1
  fi
}

cocotb=0
$python -c 'import cocotb, cocotb_tools.runner' 2>/dev/null && cocotb=1
(( cocotb )) || print -r -- "skip  kg board test: $python has no cocotb 2 (set KG_COCOTB_PYTHON)"

run() {   # the flows of the project in $1
  local dir=$1
  [[ -f $dir/kanagawa.toml ]] || return
  local info tool tests
  info=$(cd $dir && $kg board info)
  tool=$(print -r -- $info | awk '/^tools/ { gsub(/[():]/, "", $2); print $2 }')
  tests=($dir/test_*.py(N:t))
  local tb=0
  grep -q '^testbench' $dir/kanagawa.toml && tb=1
  (( tb )) && step $dir sim $kg board sim
  if [[ $tool == icestorm ]]; then
    (( tb )) && step $dir netsim $kg board netsim
    step $dir bitstream $kg board bitstream
    step $dir area $kg board area
  fi
  (( ${#tests} && cocotb )) && step $dir test $kg board test --python $python
  for t in $tests; do
    step $dir serve-$t:r $kg board serve -- python3 -I $t
  done
  [[ -f $dir/host.py ]] && step $dir serve-host $kg board serve -- python3 -I host.py test --count 200 --stream
  [[ -f $dir/check_panel.py ]] && step $dir panel $kg board serve -- python3 -I check_panel.py
}

for dir in $examples; do
  run $dir
done

# The platform's examples, copied for every board but their own, as a user
# would start one; those that use what a board lacks are refused by name.
if (( ! $# )); then
  for ex in platform/examples/*(/:t); do
    own=$(awk -F'"' '/^name = / && board { print $2 } /^\[board\]/ { board = 1 }' platform/examples/$ex/kanagawa.toml)
    for board in platform/boards/*(/:t); do
      [[ $board == $own ]] && continue
      # A board with no resources takes a project's own core, which the
      # platform's examples do not have.
      grep -q '^\[resources\.' platform/boards/$board/board.toml || continue
      dest=target/portability/$board-$ex
      rm -rf $dest
      if $kg board new $dest --board $board --example $ex --platform platform > /dev/null 2>&1; then
        mkdir -p $dest/target/board
        if (cd $dest && $kg board interface) > $dest/target/board/check-bind.log 2>&1; then
          run $dest
        elif grep -q 'no resource `' $dest/target/board/check-bind.log; then
          print -r -- "skip  $dest  $(grep -o 'no resource `[a-z0-9_]*`' $dest/target/board/check-bind.log | head -1) on $board"
        else
          print -r -- "FAIL  $dest bind  ($dest/target/board/check-bind.log)"
          failed=1
        fi
      else
        print -r -- "FAIL  $dest new"
        failed=1
      fi
    done
  done
fi

if platform/rtl/periph/test/run.sh > target/periph.log 2>&1; then
  print -r -- "pass  platform/rtl/periph test  $(grep -c 'PASS' target/periph.log) PASS lines"
else
  print -r -- "FAIL  platform/rtl/periph test  (target/periph.log)"
  failed=1
fi
if platform/rtl/hal/ice40/test/check.zsh > target/hal-ice40.log 2>&1; then
  print -r -- "pass  platform/rtl/hal/ice40 check  $(grep -c '^PASS' target/hal-ice40.log) PASS lines"
else
  print -r -- "FAIL  platform/rtl/hal/ice40 check  (target/hal-ice40.log)"
  failed=1
fi
exit $failed
