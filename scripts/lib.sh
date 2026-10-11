# Shared by the gate scripts. Source it; it defines functions only.
#
# Worktrees. Several checkouts of this repository can work at once — a
# snapshot the milestone gates run in (`gates.sh --snapshot`), an agent's
# isolated worktree — each with its own build directory, but the gates' data
# stores are large, slow to rebuild and safe to share, so every worktree
# links them from the main checkout's `target/`:
#
#   target/ptrace         the reference's recorded runs (`xtask ptrace`)
#   target/output         `output`'s stored reference side and work dirs
#   target/backend-cache  the backend verdict cache (entries renamed into place)
#   target/oracle-cache   `kg-oracle` answers (entries renamed into place)
#   oracle/dist-newstyle  the built `kg-oracle` (`oracle/build.sh`), untracked
#
# And three things a worktree cannot find on its own, which `kg_env` exports
# (with `KG_REQUIRE_CORPUS=1`, so a test that finds no corpus is red):
# the reference checkout (`KANAGAWA_ROOT`: the main checkout's
# `third_party/kanagawa` submodule — a worktree's own is uninitialised),
# which `kg-ptrace`'s build script and every corpus test read; the
# reference's build tree (`KANAGAWA_BUILD`: `target/reference`, which
# `scripts/build-backend.sh --reference` makes — the reference's own
# compiler, which the gates record, and upstream's configured CLI tests);
# and the backend library `kg` loads (`KG_BACKEND_DIR`:
# `target/backend/dist/bin`) — the binaries look for it only there or beside
# themselves (`kg_backend_sys::locate`).
#
# The heavy lock. The walk tier and the full gates peak at ~17 GB and share
# the work directories under `target/ptrace/work` and `target/output/work`,
# so one runs at a time across every worktree: `kg_heavy_lock` waits for the
# lock directory in the main checkout's `target/` and holds it until the
# shell exits.

# The main checkout's root, from any worktree of it.
kg_main_root() {
  local common
  common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || return 1
  print -r -- "${common:h}"
}

# Link the shared stores from the main checkout into this worktree's
# `target/`. A no-op in the main checkout itself.
kg_link_stores() {
  local main here store
  main=$(kg_main_root) || return 1
  here=$(git rev-parse --show-toplevel) || return 1
  [[ $main == $here ]] && return 0
  mkdir -p $here/target $main/target
  for store in target/ptrace target/output target/backend-cache target/oracle-cache oracle/dist-newstyle; do
    [[ $store == target/* ]] && mkdir -p $main/$store
    [[ -e $main/$store ]] || continue
    if [[ -L $here/$store ]]; then
      continue
    elif [[ -e $here/$store ]]; then
      print -u2 "kg_link_stores: $here/$store exists and is not a link — left alone"
    else
      mkdir -p ${here}/${store:h}
      ln -s $main/$store $here/$store
    fi
  done
}

# The reference checkout and the reference build's backend library, from
# any worktree (see the header). Honours values already set.
kg_env() {
  local main
  main=$(kg_main_root)
  export KANAGAWA_ROOT=${KANAGAWA_ROOT:-$main/third_party/kanagawa}
  export KANAGAWA_BUILD=${KANAGAWA_BUILD:-$main/target/reference}
  # A corpus test that finds no corpus fails here rather than passing
  # having graded nothing (`kg_base::corpus_absent`; sprint W0.4).
  export KG_REQUIRE_CORPUS=1
  if [[ -z ${KG_BACKEND_DIR:-} && -d $main/target/backend/dist/bin ]]; then
    export KG_BACKEND_DIR=$main/target/backend/dist/bin
  fi
}

# Wait for the machine-wide lock on heavy runs, and take it. A lock whose
# holder has died is taken over. zsh runs an EXIT trap set inside a function
# when the *function* returns, so the caller releases it:
#
#   kg_heavy_lock "the walk tier"; trap kg_heavy_unlock EXIT
kg_heavy_lock() {
  local main lock holder waited=0
  main=$(kg_main_root) || return 1
  lock=$main/target/heavy.lock
  mkdir -p $main/target
  # Re-entrant: `gates.sh --snapshot` takes it, then runs the snapshot's own
  # `gates.sh` in the same process, which asks again.
  if [[ $(cat $lock/pid 2>/dev/null) == $$ ]]; then
    KG_HEAVY_LOCK=$lock
    return 0
  fi
  while ! mkdir $lock 2>/dev/null; do
    holder=$(cat $lock/pid 2>/dev/null)
    if [[ -n $holder ]] && ! kill -0 $holder 2>/dev/null; then
      print -u2 "### heavy lock: holder $holder is gone — taking it over"
      rm -rf $lock
      continue
    fi
    if (( waited % 60 == 0 )); then
      print -u2 "### heavy lock: waiting for $(cat $lock/what 2>/dev/null) (pid $holder)"
    fi
    sleep 5
    (( waited += 5 ))
  done
  print -r -- $$ > $lock/pid
  print -r -- "${1:-a heavy run} in $(git rev-parse --show-toplevel)" > $lock/what
  KG_HEAVY_LOCK=$lock
}

kg_heavy_unlock() {
  [[ -n ${KG_HEAVY_LOCK:-} && $(cat $KG_HEAVY_LOCK/pid 2>/dev/null) == $$ ]] && rm -rf $KG_HEAVY_LOCK
  KG_HEAVY_LOCK=
}
