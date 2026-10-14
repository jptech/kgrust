#!/usr/bin/env bash
# CI: what .github/workflows/ci.yml runs, as one script, so a pull request
# and a developer's machine run the same commands.
#
#   scripts/ci.sh          # both jobs: `test`, then `lint`
#   scripts/ci.sh test     # the tests, then the corpus gates (corpus,
#                          #   conformance, fmt-check, lint), codegen --check,
#                          #   and `kg check` and `kg fmt --check` of examples/
#   scripts/ci.sh lint     # clippy, and the documentation's links
#
# This repository's Rust code only — the whole workspace but kg-ptrace,
# whose build script compiles against the reference's headers — sized for
# GitHub's free runners (Linux: 4 cores, 16 GB, 14 GB of disk). It builds
# neither the C++ backend nor the reference compiler: the tests that need
# them skip, and `cargo build` links neither — `kg` loads the backend at run
# time. The corpus gates read the `third_party/kanagawa` submodule's test
# files, library and ABI headers; nothing of it is compiled. `codegen
# --check` needs libclang (Xcode's on macOS; `libclang-dev` on Ubuntu, whose
# bindings come out byte-identical). The parity gates, which need the
# backend and the reference built, stay local (`scripts/gates.sh`,
# AGENTS.md).
#
# Under CI (`CI` set) the build is lean: one `quick` profile without debug
# info or incremental state serves the tests and the xtask gates alike.
# Measured 2026-10-11, cold, 4 build jobs and 4 test threads on Apple
# silicon: 152 s, the largest process 2.1 GB, `target/` 586 MB. Run
# locally, it keeps your `quick` settings and shares their build.
#
# Bash, not zsh like the other scripts: GitHub's Linux runners have no zsh.
# Written for the bash macOS ships (3.2).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

job=all
case ${1:-} in
  ''|all) ;;
  test|lint) job=$1 ;;
  -h|--help) sed -n '2,/^set -uo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
  *) echo "ci: unknown job ${1} (try --help)" >&2; exit 2 ;;
esac

if [[ -n ${CI:-} ]]; then
  export CARGO_INCREMENTAL=0 CARGO_PROFILE_QUICK_DEBUG=0 CARGO_PROFILE_QUICK_INCREMENTAL=false CARGO_PROFILE_DEV_DEBUG=0
fi

summary="" failures=0
step() { # name command…
  local name=$1 t0=$SECONDS rc
  shift
  if [[ -n ${GITHUB_ACTIONS:-} ]]; then echo "::group::$name"; else echo "### $name"; fi
  "$@"; rc=$?
  [[ -n ${GITHUB_ACTIONS:-} ]] && echo "::endgroup::"
  if (( rc == 0 )); then
    summary+="  ok    $name ($((SECONDS - t0)) s)"$'\n'
  else
    summary+="  FAIL  $name ($((SECONDS - t0)) s, exit $rc)"$'\n'
    failures=$((failures + 1))
    [[ -n ${GITHUB_ACTIONS:-} ]] && echo "::error::$name failed (exit $rc)"
  fi
}

# kg-ptrace is the backend interposer; its build script runs bindgen over
# the reference's headers. Everything else in the workspace, xtask's own
# tests included — they caught a Linux bug the day they first ran here.
workspace=(--workspace --exclude kg-ptrace)
run_test() {
  step "tests" cargo test --locked --profile quick "${workspace[@]}"
  local gate
  for gate in corpus conformance fmt-check lint "codegen --check"; do
    # shellcheck disable=SC2086 # `codegen --check` is a command and its flag
    step "xtask $gate" cargo run --locked --quiet --profile quick -p xtask -- $gate
  done
  step "examples" check_examples
}
# The examples, checked against every device they target and for format.
# Building and simulating them needs the backend: examples/gemm/run.sh.
check_examples() {
  local kg=(cargo run --locked --quiet --profile quick -p kg-cli --bin kg --) device
  for device in mock agilex-7-f-023 ultrascaleplus-xcu250; do
    KG_TARGET_DEVICE=$device "${kg[@]}" check -C examples/gemm || return
  done
  "${kg[@]}" fmt --check examples/gemm/src examples/gemm/config
}
run_lint() {
  step "clippy" cargo clippy --locked "${workspace[@]}" --all-targets -- -D warnings
  step "links" python3 scripts/check-links.py
}

case $job in
  all) run_test; run_lint ;;
  test) run_test ;;
  lint) run_lint ;;
esac

echo
echo "ci ($job):"
printf '%s' "$summary"
if (( failures == 0 )); then echo "all green"; else echo "$failures failed"; fi
exit $(( failures > 0 ))
