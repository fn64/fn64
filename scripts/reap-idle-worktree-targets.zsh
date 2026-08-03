#!/bin/zsh

# Report, and on request delete, Cargo target directories in worktrees whose
# tracked source has not changed recently.
#
# Build artifacts are the dominant disk cost of this repository and they are
# never reclaimed on their own: a measured sweep found 164 GB across worktrees
# nobody had touched in weeks, including a single 103 GB example target. All of
# it is regenerable, so the only real risk is deleting the artifacts of a
# worktree still in use -- which is why idleness is judged from tracked source
# mtime rather than from the target directory itself, whose mtime a stray
# `cargo metadata` would refresh.
#
# Dry run by default. Nothing is deleted without --delete.
#
#   scripts/reap-idle-worktree-targets.zsh                # report only
#   scripts/reap-idle-worktree-targets.zsh --days 30      # stricter threshold
#   scripts/reap-idle-worktree-targets.zsh --delete       # actually reclaim

set -eu

typeset -r script_path=$0
# The main checkout, not this script's own worktree. Worktrees live under the
# main checkout's .claude/, so running this from inside one -- which is the
# normal case -- must still resolve to the shared root.
typeset -r script_root=${0:A:h:h}
typeset repo_root=$script_root
typeset -r common_dir=$(git -C "$script_root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
if [[ -n $common_dir && -d ${common_dir:h} ]]; then
    repo_root=${common_dir:h}
fi
typeset -i idle_days=14
typeset -i delete=0

while (( $# > 0 )); do
    case $1 in
        --delete) delete=1; shift ;;
        --days)
            (( $# >= 2 )) || { print -u2 -- "$script_path: --days needs a value"; exit 2 }
            [[ $2 == <-> ]] || { print -u2 -- "$script_path: --days expects an integer, got $2"; exit 2 }
            idle_days=$2; shift 2 ;;
        -h|--help)
            print -- "usage: $script_path [--days N] [--delete]"
            print -- "  Reports Cargo target dirs under worktrees idle for N+ days (default 14)."
            print -- "  Deletes them only with --delete."
            exit 0 ;;
        *) print -u2 -- "$script_path: unexpected argument $1"; exit 2 ;;
    esac
done

typeset -r worktree_root=$repo_root/.claude/worktrees
if [[ ! -d $worktree_root ]]; then
    print -- "no worktree root at $worktree_root; nothing to reap"
    exit 0
fi

# The worktree this script is running from is never a candidate: its own build
# is what an operator is most likely mid-way through.
typeset -r self_worktree=${repo_root:A}

# Newest mtime among tracked source files, excluding target/ itself. `git
# ls-files` keeps generated and ignored output from making an idle worktree
# look busy.
newest_tracked_source_epoch() {
    local worktree=$1
    local -i newest=0
    local file epoch
    while IFS= read -r file; do
        [[ -f $worktree/$file ]] || continue
        epoch=$(stat -f %m "$worktree/$file" 2>/dev/null) || continue
        (( epoch > newest )) && newest=$epoch
    done < <(git -C "$worktree" ls-files -- '*.rs' '*.toml' '*.zsh' '*.py' '*.md' 2>/dev/null)
    print -- $newest
}

typeset -i now=$(date +%s)
typeset -i threshold=$(( idle_days * 86400 ))
typeset -i total_kib=0
typeset -i reaped=0

print -- "reap-idle-worktree-targets: threshold ${idle_days}d, mode $( (( delete )) && print -- delete || print -- report )"

for worktree in $worktree_root/*(N/); do
    typeset name=${worktree:t}
    typeset target=$worktree/target
    [[ -d $target ]] || continue

    typeset -i size_kib=$(du -sk "$target" | awk '{print $1}')

    if [[ ${worktree:A} == $self_worktree ]]; then
        printf '  %-34s %8s MiB  SKIP (running from here)\n' "$name" $(( size_kib / 1024 ))
        continue
    fi

    typeset -i newest=$(newest_tracked_source_epoch "$worktree")
    if (( newest == 0 )); then
        printf '  %-34s %8s MiB  SKIP (no tracked source read)\n' "$name" $(( size_kib / 1024 ))
        continue
    fi

    typeset -i age_days=$(( (now - newest) / 86400 ))
    if (( now - newest < threshold )); then
        printf '  %-34s %8s MiB  keep (source %sd old)\n' "$name" $(( size_kib / 1024 )) "$age_days"
        continue
    fi

    printf '  %-34s %8s MiB  IDLE %sd\n' "$name" $(( size_kib / 1024 )) "$age_days"
    total_kib=$(( total_kib + size_kib ))
    reaped=$(( reaped + 1 ))
    if (( delete )); then
        rm -rf "$target"
    fi
done

print -- ""
if (( delete )); then
    print -- "reclaimed ${total_kib}KiB ($(( total_kib / 1048576 ))GiB) from $reaped target director$( (( reaped == 1 )) && print -- y || print -- ies)"
else
    print -- "would reclaim ${total_kib}KiB ($(( total_kib / 1048576 ))GiB) from $reaped target director$( (( reaped == 1 )) && print -- y || print -- ies); re-run with --delete"
fi
