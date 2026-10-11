#!/bin/zsh
# Prove a test (or gate) load-bearing: plant one mutation, run the check,
# and require it to go red — then restore the file, whatever happens.
#
#   scripts/mutant.sh <file> <old> <new> -- <command …>
#
# <old> must occur in <file> exactly once and is replaced, literally, by
# <new>. The command is run with the mutation in place; exit 0 means it
# failed (the mutant was **caught**), exit 1 that it passed (the mutant
# **survived** — the check is not load-bearing for that line), exit 2 a
# usage error. The file is restored byte for byte on every exit, interrupt
# included, from a copy under target/mutant/.
#
#   scripts/mutant.sh crates/kg-sem/src/infer.rs \
#       'self.types.before(id)' 'self.types.get(id)' \
#       -- cargo test --profile quick -p kg-sem --lib incremental_inference
#
# Do not run it while a gate is running on this tree: the mutation is in the
# tree for as long as the command runs.
cd "$(dirname "$0")/.." || exit 2
(( $# >= 5 )) && [[ $4 == -- ]] || { print -u2 "usage: scripts/mutant.sh <file> <old> <new> -- <command …>"; exit 2 }
file=$1 old=$2 new=$3
shift 4
[[ -f $file ]] || { print -u2 "mutant: no file $file"; exit 2 }
mkdir -p target/mutant
backup=target/mutant/${file//\//_}.$$
cp -p $file $backup
# Restored with a fresh mtime: the content is the original's, but the last
# build was of the mutant, and a file older than that build is one cargo
# would not rebuild — the next gate would grade the mutant (2026-09-27).
restore() { [[ -f $backup ]] && { cp -p $backup $file; touch $file; rm -f $backup } }
trap restore EXIT INT TERM
python3 - "$file" "$old" "$new" <<'EOF' || exit 2
import sys
path, old, new = sys.argv[1:4]
text = open(path, encoding="utf-8").read()
n = text.count(old)
if n != 1:
    sys.exit(f"mutant: `{old}` occurs {n} times in {path}; it must occur exactly once")
open(path, "w", encoding="utf-8").write(text.replace(old, new, 1))
EOF
print "### mutant: $file: \`$old\` → \`$new\`"
"$@" > target/mutant/last.log 2>&1
rc=$?
tail -5 target/mutant/last.log
if (( rc != 0 )); then
  print "### caught (the check failed with the mutation in place; log: target/mutant/last.log)"
  exit 0
fi
print "### SURVIVED — the check passed with the mutation in place (log: target/mutant/last.log)"
exit 1
