#!/bin/zsh
# A second working copy that can build and run the gates beside the main one.
#
#   scripts/worktree.sh add <path> [<commit>]   a detached worktree at <commit>
#                                               (default HEAD), stores linked
#   scripts/worktree.sh link                    link the stores into the worktree
#                                               this is run from — an agent's
#                                               isolated worktree, say
#
# Each worktree builds into its own `target/` (the first build is a few
# minutes, later ones incremental); the gates' data stores — the reference's
# recorded runs, `output`'s stored side, the backend and oracle caches — are
# links to the main checkout's (`scripts/lib.sh`). Heavy runs take a
# machine-wide lock, so worktrees queue rather than run the machine out of
# memory. Remove one with `git worktree remove <path>`.
setopt err_exit no_unset
source "${0:a:h}/lib.sh"
case ${1:-} in
  add)
    [[ -n ${2:-} ]] || { print -u2 "usage: scripts/worktree.sh add <path> [<commit>]"; exit 2 }
    git worktree add --detach "$2" "${3:-HEAD}"
    (cd "$2" && kg_link_stores)
    print "worktree at $2 ($(git -C "$2" rev-parse --short HEAD)), stores linked from $(kg_main_root)/target"
    ;;
  link)
    kg_link_stores
    print "stores linked into $(git rev-parse --show-toplevel)/target"
    ;;
  *)
    print -u2 "usage: scripts/worktree.sh add <path> [<commit>] | link"
    exit 2
    ;;
esac
