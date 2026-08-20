# RT64 parity: measuring how closely fn64's shipping renderer matches the oracle

**Status: in progress.** This file is committed early so the finding survives
the lane. Numbers below are marked CONFIRMED (measured this run) or
HYPOTHESIS (read, inferred, or quoted).

## The third arm builds and runs here

**CONFIRMED.** `fn64-render-conformance-rt64-deferred-history-runner` compiles
and executes a live render on this machine. The build needs `FN64_RT64_DIR`
pointed at an RT64 MIT source tree; the crate's default
(`../../../no-mercy-recompiled/third_party/rt64`, resolved relative to the
crate manifest) does not exist from a `/private/tmp` worktree, which is what
makes the build appear unavailable at first contact.

```sh
export FN64_RT64_DIR=/Users/jer/Code/no-mercy-recompiled/third_party/rt64
cargo build -p fn64-render-conformance --features rt64-deferred-history-runner \
  --bin fn64-render-conformance-rt64-deferred-history-runner
```

RT64's `run` subcommand reached a real Metal device — stderr reported
`Device Name: Apple M5 Pro` — and committed RED (`0xf801`) into the guest
framebuffer. RT64 is therefore a live third backend, not scaffolding.
