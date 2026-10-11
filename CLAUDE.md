@AGENTS.md

## Claude Code specifics

The rules above bind every agent. These add what is particular to Claude
Code.

- **Subagents work in isolated worktrees** (the Agent tool's `isolation:
  "worktree"`). The first command in each is `scripts/worktree.sh link`,
  which links the gates' data stores from the primary checkout; the
  worktree's own `third_party/kanagawa` stays uninitialised, and the
  scripts find the primary checkout's. Work there, never `cd` into the
  primary checkout, and commit on the worktree's branch.
- **Those worktrees sit under `.claude/worktrees/`, inside the primary
  checkout**, so the nested-worktree rule above applies: `UPDATE_EXPECT=1`
  needs `CARGO_WORKSPACE_DIR=$PWD`.
- **Two or three agents building at once, at most.** Each worktree builds
  its own `target/` (a few minutes the first time), and builds do not take
  the heavy lock; the walk and loop tiers and the full gates do, and queue.
- **Work comes home as commits** on the agent's `worktree-agent-*` branch,
  merged into the topic branch once its fast tier is green in the primary
  checkout; blessing happens there, never in the agent's worktree. Those
  branches are never pushed.
- **Drafts go in the scratchpad while a gate grades the tree.** Run gates
  with `run_in_background` and wait on them with Monitor, not by polling.
  Before editing or building in a checkout that has background jobs, check
  `pgrep -fl '[x]task|[f]ast\.sh|[g]ates\.sh|[m]utant\.sh'`: a long single
  gate counts. The brackets matter: the shell running the command holds the
  pattern in its own command line, and a bare `gates.sh` matches that shell —
  a wait loop on it never ends.
- **Keep gate output out of the context**: send it to a log (`scripts/gates.sh
  --snapshot > target/snapshot-gates.log 2>&1`) and read the last screen and
  `target/gates/status.json`, not the whole log.
