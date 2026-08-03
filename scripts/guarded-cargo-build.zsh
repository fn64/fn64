#!/bin/zsh

# Safe local Cargo build/check feedback loop. Every invocation names its
# manifest and scope explicitly; generated-code builds remain serial and under
# the same aggregate-memory/free-memory envelope as guarded tests.

set -eu

typeset -r repo_root=${0:A:h:h}
typeset -r script_path=$0
typeset -r mode=${1:-}
typeset -r manifest_argument=${2:-}
typeset -r package=${3:-}

usage() {
    print -u2 -- "usage: $script_path check MANIFEST [PACKAGE]"
    print -u2 -- "       $script_path build MANIFEST PACKAGE"
    print -u2 -- "       $script_path full MANIFEST"
}

if [[ -z "$mode" || -z "$manifest_argument" || $# -gt 3 ]]; then
    usage
    exit 2
fi

typeset -r manifest=${manifest_argument:A}
if [[ ! -f "$manifest" || ${manifest:t} != Cargo.toml ]]; then
    print -u2 -- "guarded cargo build: MANIFEST must name an existing Cargo.toml"
    exit 2
fi
if [[ "$manifest" != "$repo_root"/Cargo.toml && "$manifest" != "$repo_root"/* ]]; then
    print -u2 -- "guarded cargo build: manifest must be inside $repo_root"
    exit 2
fi

typeset cargo_operation
typeset -a scope
case "$mode" in
    check)
        cargo_operation=check
        if [[ -n "$package" ]]; then
            scope=(-p "$package")
        else
            scope=()
        fi
        ;;
    build)
        if [[ -z "$package" ]]; then
            print -u2 -- "guarded cargo build: build mode requires one explicit PACKAGE"
            usage
            exit 2
        fi
        cargo_operation=build
        scope=(-p "$package")
        ;;
    full)
        if [[ -n "$package" ]]; then
            print -u2 -- "guarded cargo build: full mode does not accept a PACKAGE"
            usage
            exit 2
        fi
        cargo_operation=build
        scope=()
        ;;
    *)
        print -u2 -- "guarded cargo build: mode must be check, build, or full"
        usage
        exit 2
        ;;
esac

export FN64_GUARD_MAX_RSS_MIB=${FN64_GUARD_MAX_RSS_MIB:-2048}
export FN64_GUARD_MIN_FREE_PERCENT=${FN64_GUARD_MIN_FREE_PERCENT:-40}
export CARGO_BUILD_JOBS=1

exec "$repo_root/scripts/memory-guard.zsh" \
    cargo "$cargo_operation" -j1 --manifest-path "$manifest" "${scope[@]}"
