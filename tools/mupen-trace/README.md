# mupen-trace

Runtime trace producers for fn64-discover: dlopen a `DEBUGGER=1`
mupen64plus-core build, drive it through its public frontend API, and emit
JSONL that fn64-discover ingests as evidence. fn64 itself never links this
core.

## Producers in this directory

- **`mupen_trace.c`** -- trace producer v1. Single-steps a bounded window
  from the ROM entrypoint, emitting fn64-discover's `executed_pc` /
  `watched_table_write` trace schema (`crates/fn64-discover/src/trace.rs`).
  The public debugger steps a MIPS control transfer and its delay slot as one
  unit, so the emitted pause-PC stream intentionally makes no executed-PC
  exhaustiveness claim. Pack admission adds the architecturally required
  delay-slot word for every observed control instruction.
  At the pause immediately before the normalized ROM header entry executes,
  it also creates a separate `fn64.boot-context.v1` file containing the exact
  ROM and IPL3 identities, header-derived TV standard, entry PC, all GPRs,
  HI/LO, and the public debugger's complete CP0 register image. The output is
  create-new: an existing context file is never replaced.
  The step handshake permits exactly one outstanding `DebugStep`; a missing
  callback traps after a bounded wait instead of queuing another step and
  silently changing the captured CPU state.
  Setting `FN64_CPU_SNAPSHOT_PC=<aligned-pc>` together with
  `FN64_CPU_SNAPSHOT=<create-new-json-path>` additionally captures GPRs,
  HI/LO, and all public-debugger CP0 slots at the first pause before that PC
  executes. The snapshot records the exact retired-instruction position in
  the bounded window and normalized-ROM identity; it does not interpret the
  debugger's opaque TLB pointer.
  The public debugger enum does name `M64P_CPU_TLB`, but
  `DebugGetCPUDataPtr` exposes it only as `void *`; the public API specifies no
  entry type, count, size, offsets, or versioned pointee layout. This producer
  therefore must not cast it or manufacture initial-TLB authority.
  Executable-image capture normally treats its capture PC as the first fetch.
  For a completed image observed at an earlier publication checkpoint, set
  `FN64_EXECUTABLE_IMAGE_FIRST_PC` to the independently observed first entry
  inside the captured range; the producer validates that it lies in-range.
  A capture may contain up to 262,144 words (one MiB), allowing one completed
  overlay or resident image to be hashed as a unit instead of stitching
  independently timed page captures.
  `FN64_CAPTURE_ONLY=1` suppresses per-PC JSONL records during long capture
  searches, and `FN64_STOP_AFTER_IMAGE=1` stops immediately after the target
  pause. These are capture-throughput controls; their abbreviated JSONL is
  not execution-coverage evidence.
  Repeated full traces are reproducible for static-pack admission when their
  parsed authority, completion shape, event counts, and exact bank-generation
  root sets match. Their raw PC order need not be byte-identical: public-core
  asynchronous interrupt delivery can move an exception entry by adjacent
  guest instructions without changing the observed scenario root set.
  Setting `FN64_WATCH_VI=1` additionally emits value transitions for the
  fourteen standard VI MMIO words. This is a diagnostic value observation,
  not an instruction-exact timing oracle: an MMIO transition can be observed
  on either side of an adjacent debugger pause even when the ordered values
  are stable.
  `FN64_WATCH_WORD=<aligned-address>` polls one additional word after every
  retired step and prints value transitions to stderr with the preceding
  pause PC and retired count. This is a diagnostic publication-boundary
  locator: the PC is not claimed as the writer because asynchronous device
  work can become visible between pauses. The diagnostic deliberately stays
  outside trace-schema JSONL and carries no static-discovery proof by itself.
- **`mupen_devtrace.c`** -- device-event producer for the timing oracle
  (`docs/superpowers/specs/2026-07-23-timing-oracle-design.md`). Drives the
  same debugger seam, but emits `crates/fn64-discover/src/timing_trace.rs`'s
  cycle-stamped device-event schema (PI/AI/SI DMA start/complete, MI
  interrupt raise/ack, VI retrace), read against mupen's own RCP register
  headers.
  Timing schema v2 records all three PI-only fields explicitly. A PI start
  carries `dma_direction` (`PI_WR_LEN` is `to_rdram`; `PI_RD_LEN` is
  `from_rdram`), `pi_device` (`rom` for physical Domain1 Address2; `sram` for
  physical Domain2 Address2), and `pi_offset` relative to that window. Its
  completion reuses that complete start observation, but BUSY falling alone is
  not accepted as completion because PI_STATUS reset also clears BUSY without
  committing bytes. The same poll must observe a newly raised PI MI bit; an
  already-pending PI bit or no new edge aborts the producer. A proven completion
  is emitted before the matching `mi_raise`, preserving the fn64 event order.
  Every non-PI event emits those three fields as JSON `null`; this is a
  producer-output requirement pinned by the compiled C fixture. Rust ingestion
  also rejects the producer's old schema-v1 header before interpreting any
  optional payload fields.
  The public debugger can expose only register readback, not the store that
  triggered DMA. On the pinned core both PI length registers commonly read as
  `0x7f` after the triggering store. If exactly one length register does not
  claim the start, both claim it, the encoded length cannot become a nonzero
  byte count, or the complete physical device range is outside one Address2
  window, the producer emits an aborted timing terminator and exits loudly
  before any PI start. A core-side emitter is required for runs where public
  readback cannot establish that identity.
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

`FN64_FAST_FORWARD_PC` may equal `FN64_CPU_SNAPSHOT_PC` or
`FN64_EXECUTABLE_IMAGE_PC`. The pause that starts the recorded window is also
the target-capture boundary, so CPU and memory state are sampled before that
instruction executes; the fast-forward transition does not consume the only
matching pause.

`FN64_CONTINUOUS_PRELUDE_MS` is rejected when a CPU or executable-image
target capture is armed. RUNNING mode does not publish every traversed PC, so
a wall-clock pause cannot prove that a transient target was not passed; use
bounded single-step fast-forward for target-state capture.

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

`FN64_BUILD_JOBS=<positive-integer>` bounds the core build's make parallelism.
Use `FN64_BUILD_JOBS=1` on a memory-constrained host; when unset, the helper
uses the host CPU count.

## Bounded producer-to-ingest pipeline

`scripts/run-black-box-trace.zsh` connects the supported public-debugger
producer to `fn64-discover --trace --summary` without putting a ROM, trace,
boot context, or diagnostic in the worktree or on stdout. Every path is
explicit and absolute. The output directory must not exist and must be outside
the fn64 worktree. Both subprocesses have a caller-selected wall-clock timeout,
the instruction window is capped at 100,000,000, and a failed or timed-out run
removes its newly created output directory. The script rehashes the producer,
core, RSP plugin, ROM, canonical trace, and discover executable across the
boundaries where each is consumed. `fn64-discover` itself rejects a trace whose
header does not name the normalized digest of the supplied ROM.

```sh
scripts/run-black-box-trace.zsh \
  --producer /absolute/path/to/mupen_trace \
  --discover /absolute/path/to/fn64-discover \
  --core /absolute/path/to/libmupen64plus.dylib \
  --rsp /absolute/path/to/mupen64plus-rsp-hle.dylib \
  --rom /absolute/private/path/game.z64 \
  --trace-id boot-window-1 \
  --steps 5000000 \
  --timeout-seconds 600 \
  --out-dir /absolute/private/path/new-capture-directory
```

This path produces executed-PC and watched-write observations only. It does
**not** claim PI-DMA coverage. The public debugger reports PI length registers
as their hardware readback values, not the completed transfer length, so both
`mupen_devtrace` timing v2 and its separate headless-bridge mode abort rather
than fabricate DMA geometry when the readback is ambiguous. Its separate
core-side emitter is not part of this public-interface pipeline. The resulting
`trace.jsonl` is already the canonical trace schema;
the discover ingestion pass is its strict normalization/ROM-binding gate, not
a lossy format adapter. The summary proves ingestion and fact folding ran but
is not a full discovery artifact or pack-admission authority.

Some ROMs remain in an IPL3 MMIO polling loop when driven exclusively through
the public single-step seam. For example, GoldenEye's `0x80000450` sequence
loads the PI/MI status word and branches until the asynchronous transfer clears;
the headless stepper can observe that loop without advancing the device event.
Such a trace is valid boot evidence but is explicitly not post-boot gameplay
coverage. A producer run that has only a small repeating PC set must therefore
be classified as a device-progress frontier and paired with a continuous-run
or breakpoint-based capture before its PCs can admit runtime overlays.
For that experiment, set `FN64_CONTINUOUS_PRELUDE_MS` (bounded to two minutes):
the producer runs normally, issues the public `M64CMD_PAUSE`, and only then
consumes that exact pause report before it can issue a debugger step, then
starts the deterministic single-step window. This is opt-in because the
continuous prelude is timing-sensitive and its resulting trace is diagnostic,
not an instruction-exact replay authority.

The producer is compiled separately against the public mupen64plus headers as
shown in `mupen_trace.c`'s header. The pipeline deliberately does not build or
inspect the GPL core. Run the synthetic command-contract gate without a ROM or
emulator:

```sh
scripts/test-run-black-box-trace.zsh
```

Before admitting a capture as runtime evidence, classify its pause-PC shape:

```sh
python3 tools/mupen-trace/classify-trace.py /absolute/path/trace.jsonl
```

The command exits `2` for a small repeating PC frontier (usually an
asynchronous device-progress wait), `0` for a diverse execution observation,
and `0` with `insufficient-observation` when too few executed records exist.

### Deterministic controller input

Build `fn64_input_plugin.c` as a separate input-plugin dylib and set
`FN64_INPUT_PLUGIN` to its absolute path. Set `FN64_INPUT_SCHEDULE` to a file
whose first non-comment line is `fn64.controller-input-schedule.v1`, followed
by rows of `port first_read end_read buttons_hex stick_x stick_y`. `GetKeys`
advances each port's read ordinal independently; a row applies only to its
declared port and uncovered reads are neutral. A macOS build using the public
Mupen headers is:

```sh
cc -fPIC -shared -O2 -Wall -Wextra -Werror \
  -I<path-to-mupen64plus-core>/src/api \
  -o <scratch>/fn64_input_plugin.dylib fn64_input_plugin.c
```

The plugin is original fn64 tooling and may be extracted into a separate
permissively licensed repository. Keep the GPL Mupen core, ROMs, schedules,
traces, and compiled dylibs outside that repository; this tree contains no
link-time dependency on the core.
The plugin is loaded only when `FN64_INPUT_PLUGIN` is set, preserving the
dummy-input default. Its source and build output remain outside the fn64 tree.

For a bounded startup bypass, `FN64_FAST_FORWARD_PC=0x800xxxxx` changes the
capture start from the default ROM entrypoint to an aligned resident address.
The producer still single-steps until that pause is observed, emits the same
boot-context contract, and keeps the pre-window step count diagnostic; invalid
or non-resident values fail closed.
