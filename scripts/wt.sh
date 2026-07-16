#!/usr/bin/env bash
# fn64 worktree manager — keep the parallel-Codex fan-out tidy.
#   wt.sh            list every fn64 worktree: branch, commits-ahead-of-main,
#                    dirty-file count, live codex job?, and MERGED-CLEAN flag
#   wt.sh prune      remove worktrees whose branch is fully merged to main AND
#                    have no uncommitted changes AND no live codex job (safe)
#   wt.sh prune -f   also remove merged worktrees even if a stale branch lingers
# ponytail: a status view + a safe-prune. No add/create wrapper — `git worktree
# add -b fix/x /path main` is already one line; don't wrap what's already minimal.
set -euo pipefail
cd "$(git -C "${0%/*}" rev-parse --show-toplevel)"

live_job() { # $1=worktree path -> "codex" if a codex exec is -C'd there
  ps aux | grep "codex exec" | grep -v grep | grep -q -- "-C $1" && echo codex || true
}

case "${1:-list}" in
  list)
    printf '%-34s %-32s %5s %5s %-6s %s\n' WORKTREE BRANCH AHEAD DIRTY JOB STATE
    git worktree list --porcelain | awk '/^worktree /{print $2}' | while read -r wt; do
      [ "$wt" = "$PWD" ] && { printf '%-34s %-32s %5s %5s %-6s %s\n' "$(basename "$wt")" main - - - "(main)"; continue; }
      b=$(git -C "$wt" branch --show-current 2>/dev/null || echo "?")
      ahead=$(git -C "$wt" rev-list --count main..HEAD 2>/dev/null || echo "?")
      dirty=$(git -C "$wt" status --short 2>/dev/null | wc -l | tr -d ' ')
      job=$(live_job "$wt"); [ -z "$job" ] && job="-"
      # merged-clean = every commit on branch is in main (ahead=0) + clean + no job
      state="active"
      if [ "$ahead" = 0 ] && [ "$dirty" = 0 ] && [ "$job" = "-" ]; then state="MERGED-CLEAN (prunable)"; fi
      printf '%-34s %-32s %5s %5s %-6s %s\n' "$(basename "$wt")" "$b" "$ahead" "$dirty" "$job" "$state"
    done
    ;;
  prune)
    force="${2:-}"
    git worktree list --porcelain | awk '/^worktree /{print $2}' | while read -r wt; do
      [ "$wt" = "$PWD" ] && continue
      ahead=$(git -C "$wt" rev-list --count main..HEAD 2>/dev/null || echo 1)
      dirty=$(git -C "$wt" status --short 2>/dev/null | wc -l | tr -d ' ')
      [ -n "$(live_job "$wt")" ] && { echo "skip (live job): $(basename "$wt")"; continue; }
      if [ "$ahead" = 0 ] && [ "$dirty" = 0 ]; then
        git worktree remove --force "$wt" && echo "pruned: $(basename "$wt")"
      elif [ "$force" = "-f" ] && [ "$dirty" = 0 ]; then
        echo "skip (ahead=$ahead, unmerged commits — not pruning even with -f; merge or drop the branch first): $(basename "$wt")"
      fi
    done
    git worktree prune
    ;;
  *) echo "usage: wt.sh [list|prune [-f]]"; exit 2;;
esac
