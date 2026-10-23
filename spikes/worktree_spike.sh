#!/usr/bin/env bash
# M0 worktree + ACP spike.
#
# 1. Make a scratch repo whose main checkout is "dirty": a staged change, an
#    unstaged change and a stash entry.
# 2. Create a task worktree OUTSIDE the repo on branch qur/<agent>/<slug>.
# 3. Run a real agent over ACP (acp_spike) in that worktree.
# 4. Commit the agent's work on the task branch, merge it into main.
# 5. Verify the main checkout's index, unstaged change and stash are untouched,
#    and that no agent process is left behind.
#
# usage: spikes/worktree_spike.sh <agent-id> <adapter-cmd> [extra acp_spike flags]
set -euo pipefail

AGENT=${1:?agent id, e.g. claude}
ADAPTER=${2:?adapter command, e.g. claude-agent-acp}
shift 2
HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/out/$AGENT-$(date +%H%M%S)"
QUR_HOME="$HERE/out/qur-home"   # stands in for ~/Library/Application Support/Qur
REPO="$OUT/repo"
SLUG="add-greeting"
BRANCH="qur/$AGENT/$SLUG"
mkdir -p "$OUT" "$QUR_HOME"

say() { printf '\n== %s\n' "$*"; }
fail() { printf 'FAIL: %s\n' "$*"; exit 1; }

say "scratch repo"
git init -q -b main "$REPO"
cd "$REPO"
git config user.email spike@qur.local
git config user.name "qur spike"
mkdir src
printf 'export function add(a: number, b: number) {\n  return a + b;\n}\n' > src/math.ts
printf '# scratch\n' > README.md
printf 'notes v1\n' > notes.txt
git add -A && git commit -qm "initial"

# Dirty state the merge must not disturb.
printf 'notes v2 (stashed)\n' > notes.txt && git stash push -q -m "user stash"
printf '# scratch\n\nstaged line\n' > README.md && git add README.md
printf 'notes v3 (unstaged)\n' > notes.txt

snapshot() {
  git -C "$REPO" diff --cached --binary > "$OUT/$1.index.diff"
  git -C "$REPO" diff --binary > "$OUT/$1.worktree.diff"
  git -C "$REPO" stash list --format='%H %gs' > "$OUT/$1.stash"
}
snapshot before

say "worktree outside the repo"
REPO_ID=$(git rev-list --max-parents=0 HEAD | head -1 | cut -c1-12)
WT="$QUR_HOME/worktrees/$REPO_ID/$SLUG-$(basename "$OUT")"
BASE=$(git rev-parse HEAD)
t=$(perl -MTime::HiRes=time -e 'print time')
git worktree add -q -b "$BRANCH" "$WT" "$BASE"
echo "worktree add: $(perl -MTime::HiRes=time -e "printf '%.0f', (time-$t)*1000") ms -> $WT"
case "$WT" in "$REPO"/*) fail "worktree is inside the repo";; esac

say "agent over ACP ($ADAPTER)"
PROMPT='Create a new file src/greet.ts exporting a function greet(name: string): string that returns "hello, " + name. Then edit src/math.ts to add an exported function sub(a: number, b: number). Do not run git commands. Keep it short.'
set +e
"$HERE/acp_spike/target/release/acp_spike" "$ADAPTER" "$WT" "$OUT/acp.jsonl" "$PROMPT" "$@"
ACP_RC=$?
set -e
echo "acp_spike exit: $ACP_RC"
[ $ACP_RC -eq 3 ] && fail "orphaned processes"

say "agent's changes in worktree"
git -C "$WT" status --short
[ -n "$(git -C "$WT" status --porcelain)" ] || fail "agent made no changes"

say "commit on task branch (Qur does this on Merge)"
git -C "$WT" add -A
git -C "$WT" -c user.email=qur@local -c user.name=Qur commit -qm "qur: $SLUG ($AGENT)"
git -C "$WT" log --oneline -1

say "merge into main checkout (dirty, user would be asked first)"
# Plain `git merge` refuses when the index has staged changes. Instead:
# compute the merge in memory, then move HEAD with a 2-way read-tree that
# carries the user's staged/unstaged changes along (and refuses, touching
# nothing, if a dirty file would be overwritten).
OLD_HEAD=$(git -C "$REPO" rev-parse HEAD)
if ! TREE=$(git -C "$REPO" merge-tree --write-tree "$OLD_HEAD" "$BRANCH"); then
  fail "conflict: Qur falls back to a real merge on a clean checkout and opens conflicts in the editor"
fi
TREE=$(printf '%s\n' "$TREE" | head -1)
MERGE=$(git -C "$REPO" commit-tree "$TREE" -p "$OLD_HEAD" -p "$BRANCH" -m "Merge $BRANCH")
git -C "$REPO" read-tree -m -u "$OLD_HEAD" "$MERGE"
git -C "$REPO" update-ref -m "qur: merge $BRANCH" HEAD "$MERGE" "$OLD_HEAD"
git -C "$REPO" log --oneline --graph -4

say "verify untouched"
snapshot after
for k in index.diff worktree.diff stash; do
  if cmp -s "$OUT/before.$k" "$OUT/after.$k"; then echo "ok   $k unchanged"; else echo "DIFF $k"; diff "$OUT/before.$k" "$OUT/after.$k" | head; fail "$k changed"; fi
done
git -C "$REPO" cat-file -e HEAD:src/greet.ts && echo "ok   src/greet.ts merged"

say "cleanup worktree + branch (Discard/after-merge path)"
git -C "$REPO" worktree remove "$WT"
git -C "$REPO" branch -d "$BRANCH"
git -C "$REPO" worktree list
echo
echo "PASS  logs: $OUT"
