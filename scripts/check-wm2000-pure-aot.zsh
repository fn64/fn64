#!/bin/zsh

# Fail if Cargo feature unification puts the development interpreter into the
# standalone WM2000 dense-AOT artifact.

set -eu

typeset -r repo_root=${0:A:h:h}
typeset -r manifest=$repo_root/examples/wm2000-block-boot/Cargo.toml
typeset feature_tree

feature_tree=$(cargo tree --manifest-path "$manifest" -e normal,features -p wm2000-block-boot)

if print -r -- "$feature_tree" | grep -Fq 'fn64-recomp-rs feature "dev-interpreter"'; then
    print -u2 -- "wm2000 pure-AOT gate: dev-interpreter is enabled in the production feature graph"
    exit 1
fi
if ! print -r -- "$feature_tree" | grep -Fq 'fn64-recomp-rs feature "aot-runtime"'; then
    print -u2 -- "wm2000 pure-AOT gate: aot-runtime is absent from the production feature graph"
    exit 1
fi
if ! print -r -- "$feature_tree" | grep -Fq 'fn64-recomp-rs feature "production-aot"'; then
    print -u2 -- "wm2000 pure-AOT gate: production-aot is absent from the production feature graph"
    exit 1
fi

print -- "wm2000 pure-AOT gate: production-aot/aot-runtime present; dev-interpreter absent"
