#!/bin/zsh

# Sample one isolated process group by aggregate resident memory and the host's
# system-wide available-memory signal. The group remains the ownership boundary
# after its original leader exits or descendants are reparented.

set -u
# The monitored command must keep the caller's scheduling priority. zsh
# otherwise automatically applies nice(5) to background jobs.
unsetopt BG_NICE

if (( $# == 0 )); then
    print -u2 -- "usage: scripts/memory-guard.zsh COMMAND [ARG ...]"
    exit 2
fi

typeset -ri max_tree_rss_mib=${FN64_GUARD_MAX_RSS_MIB:-2048}
typeset -ri min_free_percent=${FN64_GUARD_MIN_FREE_PERCENT:-40}
typeset -ri report_interval=${FN64_GUARD_REPORT_INTERVAL:-10}
typeset -ri max_seconds=${FN64_GUARD_MAX_SECONDS:-0}
typeset -r poll_seconds=${FN64_GUARD_POLL_SECONDS:-1}
typeset -r jsonl_path=${FN64_GUARD_JSONL:-}
typeset -ri max_tree_rss_kib=$((max_tree_rss_mib * 1024))
typeset -i guard_started_seconds=0
typeset -a guarded_pids
typeset -i peak_tree_rss_kib=0
typeset -i peak_process_pid=0
typeset -i peak_process_rss_kib=0
typeset peak_process_command=unknown
typeset -i poll_count=0
typeset -i last_tree_rss_kib=0
typeset -i last_largest_process_rss_kib=0
typeset -i last_free_percent=-1
typeset -i elapsed_seconds=0
typeset -i root_pid=0
typeset -i guard_pgid=0
typeset -r session_helper=/usr/bin/perl
typeset ready_dir=
typeset ready_path=

if (( max_tree_rss_mib <= 0 || min_free_percent < 0 || min_free_percent > 100 || report_interval <= 0 || max_seconds < 0 )); then
    print -u2 -- "memory guard: RSS/report limits must be positive, max seconds must be nonnegative, and free percent must be 0..100"
    exit 2
fi
case $poll_seconds in
    0.05|0.1|0.25|0.5|1|2) ;;
    *)
        print -u2 -- "memory guard: FN64_GUARD_POLL_SECONDS must be one of 0.05, 0.1, 0.25, 0.5, 1, or 2"
        exit 2
        ;;
esac

if [[ -n "$jsonl_path" ]]; then
    if [[ -d "$jsonl_path" ]]; then
        print -u2 -- "memory guard: FN64_GUARD_JSONL must name a file, not a directory"
        exit 2
    fi
    : >> "$jsonl_path" || {
        print -u2 -- "memory guard: cannot append JSONL samples to $jsonl_path"
        exit 2
    }
fi
if (( max_seconds > 0 )) || [[ -n "$jsonl_path" ]]; then
    guard_started_seconds=$(date +%s) || {
        print -u2 -- "memory guard: cannot read wall clock; refusing to launch"
        exit 98
    }
fi

record_json_sample() {
    local reason=$1
    [[ -z "$jsonl_path" ]] && return
    # Deliberately omit argv, process names, PIDs, and filesystem inputs. The
    # V1 field names remain stable even though the ownership boundary is now
    # an isolated process group rather than a discovered descendant tree.
    print -r -- "{\"schema\":\"fn64.memory-guard.sample.v1\",\"elapsed_seconds\":$elapsed_seconds,\"tree_rss_mib\":$((last_tree_rss_kib / 1024)),\"peak_tree_rss_mib\":$((peak_tree_rss_kib / 1024)),\"largest_process_rss_mib\":$((last_largest_process_rss_kib / 1024)),\"free_percent\":$last_free_percent,\"reason\":\"$reason\"}" >> "$jsonl_path" || {
        print -u2 -- "memory guard: cannot append JSONL sample; terminating process group $guard_pgid"
        terminate_group
        wait "$root_pid" 2>/dev/null || true
        exit 98
    }
}

read_macos_free_percent() {
    local pressure_output
    local line
    local candidate
    pressure_output=$(memory_pressure -Q 2>/dev/null) || return 1
    free_percent=
    for line in ${(f)pressure_output}; do
        if [[ "$line" == "System-wide memory free percentage: "*"%" ]]; then
            candidate=${line#System-wide memory free percentage: }
            candidate=${candidate%%%}
            [[ "$candidate" == <-> ]] || return 1
            free_percent=$candidate
            break
        fi
    done
    [[ -n "$free_percent" ]] || return 1
    (( free_percent >= 0 && free_percent <= 100 ))
}

read_linux_free_percent() {
    local meminfo_key
    local meminfo_value
    local meminfo_unit
    local ignored
    local total_kib=
    local available_kib=
    local total_unit=
    local available_unit=

    while read -r meminfo_key meminfo_value meminfo_unit ignored; do
        case ${meminfo_key%:} in
            MemTotal)
                total_kib=$meminfo_value
                total_unit=$meminfo_unit
                ;;
            MemAvailable)
                available_kib=$meminfo_value
                available_unit=$meminfo_unit
                ;;
        esac
    done < /proc/meminfo || return 1
    [[ "$total_kib" == <-> && "$available_kib" == <-> ]] || return 1
    [[ "$total_unit" == kB && "$available_unit" == kB ]] || return 1
    (( total_kib > 0 && available_kib >= 0 && available_kib <= total_kib )) || return 1
    free_percent=$((available_kib * 100 / total_kib))
}

read_free_percent() {
    if (( $+commands[memory_pressure] )); then
        read_macos_free_percent
    elif [[ -r /proc/meminfo ]]; then
        read_linux_free_percent
    else
        return 1
    fi
}

# Take one coherent process-table snapshot. Any ps failure or malformed row is
# a monitoring failure, not an empty group. The fourth field is diagnostic
# only; no correctness decision depends on parsing a path or argv.
collect_group() {
    local process_snapshot
    local line
    local -a fields
    local process_pid
    local process_pgid
    local process_rss_kib
    local process_command

    process_snapshot=$(ps -axo pid=,pgid=,rss=,comm= 2>/dev/null) || return 1
    guarded_pids=()
    tree_rss_kib=0
    largest_process_pid=0
    largest_process_rss_kib=0
    largest_process_command=unknown
    for line in ${(f)process_snapshot}; do
        fields=(${=line})
        (( ${#fields} >= 3 )) || return 1
        process_pid=$fields[1]
        process_pgid=$fields[2]
        process_rss_kib=$fields[3]
        [[ "$process_pid" == <-> && "$process_pgid" == <-> && "$process_rss_kib" == <-> ]] || return 1
        (( process_pgid == guard_pgid )) || continue
        process_command=${fields[4]:-unknown}
        guarded_pids+=("$process_pid")
        (( tree_rss_kib += process_rss_kib ))
        if (( process_rss_kib > largest_process_rss_kib )); then
            largest_process_pid=$process_pid
            largest_process_rss_kib=$process_rss_kib
            largest_process_command=$process_command
        fi
    done
}

signal_group() {
    local signal_name=$1
    # Never allow an unset, broad, or inherited group to become a kill target.
    (( guard_pgid > 1 && guard_pgid == root_pid && guard_pgid != $$ )) || {
        print -u2 -- "memory guard: invalid isolated process group; refusing to signal"
        return 1
    }
    kill -"$signal_name" -- -"$guard_pgid" 2>/dev/null || true
}

terminate_group() {
    local attempt
    signal_group TERM || return 1
    # Memory pressure is a safety boundary: do not wait indefinitely for a
    # wedged member to cooperate. Membership is re-sampled from the exact PGID.
    for attempt in {1..20}; do
        if ! collect_group; then
            print -u2 -- "memory guard: process-table failure during termination; escalating exact process group $guard_pgid"
            signal_group KILL
            return 1
        fi
        (( ${#guarded_pids} == 0 )) && return 0
        sleep 0.1
    done
    signal_group KILL
}

# Refuse to launch unless every observation primitive and the native setsid(2)
# bridge are present. Non-interactive zsh cannot enable job-control process
# groups reliably on macOS, so the system Perl POSIX binding is the small
# launch boundary.
ps -axo pid=,pgid=,rss=,comm= >/dev/null 2>&1 || {
    print -u2 -- "memory guard: process-table access is unavailable; refusing to launch"
    exit 98
}
read_free_percent || {
    print -u2 -- "memory guard: system free-memory sampling is unavailable; refusing to launch"
    exit 98
}
[[ -x "$session_helper" ]] && "$session_helper" -MPOSIX=setsid -e 'exit 0' >/dev/null 2>&1 || {
    print -u2 -- "memory guard: native setsid helper is unavailable; refusing to launch"
    exit 98
}

ready_dir=$(mktemp -d "${TMPDIR:-/tmp}/fn64-memory-guard.XXXXXX") || {
    print -u2 -- "memory guard: cannot create private launch handshake; refusing to launch"
    exit 98
}
ready_path=$ready_dir/ready
cleanup_ready() {
    [[ -n "$ready_path" ]] && rm -f -- "$ready_path"
    [[ -n "$ready_dir" ]] && rmdir -- "$ready_dir" 2>/dev/null || true
}
trap cleanup_ready EXIT

"$session_helper" -MPOSIX=setsid -e '
    my $ready = shift @ARGV;
    defined(setsid()) or die "setsid: $!\n";
    open my $handle, ">", $ready or die "ready: $!\n";
    print {$handle} "$$\n" or die "ready write: $!\n";
    close $handle or die "ready close: $!\n";
    exec { $ARGV[0] } @ARGV or die "exec: $!\n";
' -- "$ready_path" "$@" &
root_pid=$!
guard_pgid=$root_pid

# The helper publishes readiness only after setsid(2), and never starts the
# command before that write. A failed helper exits; a live helper is allowed to
# finish this bounded native operation rather than being killed through an
# unproven group identifier.
while [[ ! -f "$ready_path" ]]; do
    if ! kill -0 "$root_pid" 2>/dev/null; then
        wait "$root_pid" 2>/dev/null || true
        print -u2 -- "memory guard: failed to establish isolated process group; command not launched"
        exit 98
    fi
    sleep 0.01
done
typeset ready_pid
IFS= read -r ready_pid < "$ready_path" || ready_pid=
if [[ "$ready_pid" != "$root_pid" || "$ready_pid" != <-> ]]; then
    print -u2 -- "memory guard: invalid process-group handshake; terminating exact process group $guard_pgid"
    terminate_group
    wait "$root_pid" 2>/dev/null || true
    exit 98
fi

forward_signal() {
    terminate_group
}
trap forward_signal INT TERM HUP

while true; do
    if ! collect_group; then
        print -u2 -- "memory guard: cannot inspect process group; terminating exact group $guard_pgid"
        terminate_group
        wait "$root_pid" 2>/dev/null || true
        exit 98
    fi
    (( ${#guarded_pids} == 0 )) && break

    if (( tree_rss_kib > peak_tree_rss_kib )); then
        peak_tree_rss_kib=$tree_rss_kib
        peak_process_pid=$largest_process_pid
        peak_process_rss_kib=$largest_process_rss_kib
        peak_process_command=$largest_process_command
    fi

    if ! read_free_percent; then
        print -u2 -- "memory guard: cannot read system free percentage; terminating exact group $guard_pgid"
        terminate_group
        wait "$root_pid" 2>/dev/null || true
        exit 97
    fi

    if (( max_seconds > 0 )) || [[ -n "$jsonl_path" ]]; then
        current_seconds=$(date +%s) || {
            print -u2 -- "memory guard: cannot read wall clock; terminating exact group $guard_pgid"
            terminate_group
            wait "$root_pid" 2>/dev/null || true
            exit 98
        }
        elapsed_seconds=$((current_seconds - guard_started_seconds))
    fi
    last_tree_rss_kib=$tree_rss_kib
    last_largest_process_rss_kib=$largest_process_rss_kib
    last_free_percent=$free_percent
    record_json_sample sample

    if (( poll_count % report_interval == 0 )); then
        print -u2 -- "memory guard: group_rss_mib=$((tree_rss_kib / 1024)) peak_mib=$((peak_tree_rss_kib / 1024)) peak_process_mib=$((peak_process_rss_kib / 1024)) peak_pid=$peak_process_pid peak_command=$peak_process_command free_percent=${free_percent}"
    fi
    if (( max_seconds > 0 && elapsed_seconds >= max_seconds )); then
        print -u2 -- "memory guard: wall-time limit crossed elapsed_seconds=$elapsed_seconds limit_seconds=$max_seconds; terminating exact process group $guard_pgid"
        record_json_sample wall_time
        terminate_group
        wait "$root_pid" 2>/dev/null || true
        exit 124
    fi
    if (( tree_rss_kib > max_tree_rss_kib || free_percent < min_free_percent )); then
        threshold_reason=tree_rss
        (( free_percent < min_free_percent )) && threshold_reason=free_memory
        print -u2 -- "memory guard: sampled safety threshold crossed reason=$threshold_reason group_rss_mib=$((tree_rss_kib / 1024)) limit_mib=$max_tree_rss_mib largest_process_mib=$((largest_process_rss_kib / 1024)) largest_pid=$largest_process_pid largest_command=$largest_process_command free_percent=$free_percent min_free_percent=$min_free_percent; terminating exact process group $guard_pgid"
        record_json_sample "$threshold_reason"
        terminate_group
        wait "$root_pid" 2>/dev/null || true
        exit 97
    fi
    (( poll_count += 1 ))
    sleep "$poll_seconds"
done

wait "$root_pid"
exit_code=$?
if [[ -n "$jsonl_path" ]]; then
    current_seconds=$(date +%s) || {
        print -u2 -- "memory guard: cannot read final wall clock"
        exit 98
    }
    elapsed_seconds=$((current_seconds - guard_started_seconds))
fi
record_json_sample complete
print -u2 -- "memory guard: exit=$exit_code peak_group_rss_mib=$((peak_tree_rss_kib / 1024)) peak_process_rss_mib=$((peak_process_rss_kib / 1024)) peak_pid=$peak_process_pid peak_command=$peak_process_command"
exit "$exit_code"
