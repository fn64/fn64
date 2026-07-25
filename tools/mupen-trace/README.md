# mupen-trace

Runtime trace producers for fn64-discover: dlopen a `DEBUGGER=1`
mupen64plus-core build, drive it through its public frontend API, and emit
JSONL that fn64-discover ingests as evidence. fn64 itself never links this
core.

## Producers in this directory

- **`mupen_trace.c`** -- trace producer v1. Single-steps a bounded window
  from the ROM entrypoint, emitting fn64-discover's `executed_pc` /
  `watched_table_write` trace schema (`crates/fn64-discover/src/trace.rs`).
- **`mupen_devtrace.c`** -- device-event producer for the timing oracle
  (`docs/superpowers/specs/2026-07-23-timing-oracle-design.md`). Drives the
  same debugger seam, but emits `crates/fn64-discover/src/timing_trace.rs`'s
  cycle-stamped device-event schema (PI/AI/SI DMA start/complete, MI
  interrupt raise/ack, VI retrace), read against mupen's own RCP register
  headers.
  - `FN64_CAPTURE_SECONDS=<n>` switches this producer to a full-speed mode:
    it drops `EnableDebugger` and runs `R4300Emulator=2` (dynarec) instead of
    the interpreter, driven by a wall-clock budget rather than a step count.
    This requires the core-side PI DMA emitter (`FN64_PI_DMA_TRACE`), since
    without single-stepping there is no debugger poll loop to observe from.
    It is not deterministic for every ROM (measured: Super Mario 64 was
    byte-identical across repeated runs, GoldenEye and Perfect Dark were
    not), so treat it as a throughput mode that complements the
    single-step path, not a replacement for it.
- **`ares_gdb_trace.py`** / **`test_ares_gdb_trace.py`** -- a GDB-remote-protocol
  driver against ares's debug stub, used for a narrower, human-supervised
  capture. See `tools/ares/README.md` for what ares can and can't do here.

## Why `DEBUGGER=1` is required

fn64-discover needs live execution evidence: which PCs actually ran, and
what specific memory cells held at specific points, to corroborate or refute
static discovery claims. mupen64plus-core only exposes that through its
debugger API (`m64p_debugger.h`): `DebugSetCallbacks`/`DebugStep` for
single-stepping with a pause callback, `DebugMemRead32`/`DebugMemRead8` for
live RDRAM reads, `DebugGetCPUDataPtr` for live CP0 register access (used by
`mupen_devtrace.c` for the CP0 Count guest-cycle stamp). None of this exists
on a normal (non-debugger) core build -- there is no other programmatic
memory-inspection or step-control surface. A core built *without*
`DEBUGGER=1` still dlopens and runs fine; it just silently lacks the exported
`Debug*` symbols, which turns into a confusing `dlsym` failure much later.
That's why `build-core.sh` verifies the three load-bearing symbols
(`DebugSetCallbacks`, `DebugStep`, `DebugMemRead32`) immediately after
building, rather than letting a bad build surface as a downstream mystery.

One real cost of the debugger API: `EnableDebugger` requires
`R4300Emulator=0` (the pure interpreter) on every host architecture, dynarec
or not. Single-stepped captures are therefore always interpreter-speed, which
is why `mupen_devtrace.c`'s `FN64_CAPTURE_SECONDS` mode exists -- it skips
the debugger and the interpreter constraint entirely to trade single-step
determinism for wall-clock throughput at full dynarec speed.

## Why mupen64plus and not ares

ares is ISC-licensed (no GPL boundary to manage) and has a real per-instruction
tracer and a GDB stub, but as shipped it has **no headless/offscreen mode** --
every launch opens a real AppKit window and blocks on the event loop, and its
instruction tracer is GUI-toggle-only with no CLI or settings.bml way to
enable it. That rules it out as an unattended, scriptable trace producer; it
is used here only as a human-supervised accuracy oracle via its GDB stub
(`ares_gdb_trace.py`). See `tools/ares/README.md` for the full investigation
(what was tried, what the ares source actually supports, and the turnkey
GDB-stub capture procedure). mupen64plus-core, once built with `DEBUGGER=1`,
is the tool that can actually run unattended and answer to a script.

## The GPL boundary

mupen64plus-core is GPLv2. fn64 must never link it, statically or
dynamically, and must never vendor its source. The producers in this
directory only ever:

- `dlopen()` a core dylib built out-of-tree (see below) and resolve symbols
  with `dlsym()` against mupen64plus's own **public, documented** frontend
  headers (`m64p_types.h`, `m64p_common.h`, `m64p_config.h`,
  `m64p_frontend.h`, `m64p_debugger.h`) -- no core implementation source is
  read or copied.
- Run the core as an in-process plugin loaded by a small fn64-owned driver,
  which itself links nothing GPL -- the driver and the core are separate
  compilation units bound only through `dlopen`/`dlsym` at runtime, never at
  link time.
- Write plain JSONL to a path the caller names. Traces are runtime artifacts
  derived from a user's own ROM; they stay out of git, same as the ROM
  itself.

The build output (the `.dylib`/`.so`) is produced by `build-core.sh` into an
out-of-tree scratch directory and must never be copied into fn64's git tree.

## The pin

Upstream mupen64plus-core does not build a working `DEBUGGER=1` core on
macOS/arm64. Two patches fix this, both in flight upstream as
[PR #1184](https://github.com/mupen64plus/mupen64plus-core/pull/1184):

- `3af70b7c` -- `new_dynarec`: native Apple Silicon (darwin-arm64) support
- `c6cf52d5` -- `debugger`: build `DEBUGGER=1` on macOS/arm64 without
  libbfd/libopcodes

`build-core.sh` pins `https://github.com/jeremyw/mupen64plus-core.git` at
commit `c6cf52d517e63fe4bed01554ddfbd9af5fb48d5a` (tip of branch
`darwin-arm64-dynarec`), which carries both patches on top of upstream base
`6dca4c15370ac3e2171ce7b31426695f8f39b460`. When PR #1184 merges, repoint the
script at upstream and delete the pin comment.

## Building the core

```sh
tools/mupen-trace/build-core.sh /path/to/scratch-dir
```

This clones the pin (or reuses/updates an existing clone), checks out the
pinned commit, builds with `make -C projects/unix all DEBUGGER=1 -jN`, and
verifies the three debugger symbols are actually exported before printing
the built dylib's path. On macOS the pinned patch defaults
`DEBUGGER_NO_DISASM=1`, so no libbfd/libopcodes dependency is needed. The
scratch directory must be outside fn64's git tree (e.g. under `/tmp`) --
never commit the clone or the build output.
