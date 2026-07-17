#!/usr/bin/env bash
# dispatch.sh -- one delegated wave task: worktree + AGENTS.md-prefixed card
# + background `codex exec`. See docs/DELEGATION.md for the full loop
# (supervise/verify/merge stay with the dispatcher; this only launches).
#
# Usage: scripts/dispatch.sh <name> <task-card.md> [extra codex args...]
#   -> worktree ../fn64-<name>-wt, branch wave/<name>,
#      log <worktree>/.dispatch.log, pid <worktree>/.dispatch.pid
set -euo pipefail

FN64=$(cd "$(dirname "$0")/.." && pwd)
name="${1:?usage: dispatch.sh <name> <task-card.md> [codex args...]}"
card="${2:?task card file required}"
shift 2

wt="$FN64/../fn64-$name-wt"
branch="wave/$name"
[ -e "$wt" ] && { echo "dispatch: $wt already exists" >&2; exit 2; }
git -C "$FN64" worktree add "$wt" -b "$branch" main

prompt="$wt/.dispatch-prompt.md"
{
  echo "Read AGENTS.md at the repo root FIRST and obey it for everything"
  echo "below (clean-room GPL ban, validation bars, loud traps, evidence-"
  echo "cited commits, docs change in the same commit, no game content in"
  echo "git). Work ONLY in this worktree, commit on the current branch"
  echo "($branch), do not push, do not merge. One focused task; if blocked,"
  echo "write the precise frontier to BLOCKED.md and stop."
  echo
  cat "$card"
} > "$prompt"

log="$wt/.dispatch.log"
nohup codex exec -C "$wt" --sandbox workspace-write "$@" - < "$prompt" \
  > "$log" 2>&1 &
echo $! > "$wt/.dispatch.pid"
echo "dispatched $name: wt=$wt branch=$branch pid=$(cat "$wt/.dispatch.pid") log=$log"
