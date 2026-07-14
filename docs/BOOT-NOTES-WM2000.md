# WM2000 boot session notes (2026-07-14)

Starting state: HEAD=a7f94c33. Frontier per commit msg: func_800004D0 unbounded
poll ~iter 400. Trace file /tmp/wm2000-boot-trace.jsonl only has 1 event (stale
from prior run, not yet re-run this session).

Reference (read-only): /Users/jer/Code/aki-recomp games/NWXE/profile.toml
rung-2 trail = PIF terminate-boot handshake archaeology:
func_80036498/osInitialize loops __osSiRawReadIo on 0x1FC007FC until a status
bit (rung-1: __osSiDeviceBusy semantics).

## Task 1 finding: root cause identified, NOT the PIF-poll bit

The "func_800004D0" frontier named in the task is misleading -- that function
is thread 0's `game_main`, whose OWN tail is the (already correctly patched,
per aki-recomp wm2000.toml patches.instruction on func_800004D0 vram
0x800005AC) cooperative idle self-loop -- not the stall.

Per fn64/docs/DESIGN.md's M1 writeup (2026-07-14), the REAL stall is on
thread 6 (spawned by game_main), 3 call-levels deep:
`func_800222D8` -> `func_80003720` -> `func_80000660`.

`func_80000660` (aki-recomp games/NWXE/RecompiledFuncs/funcs_0.c:430-560ish)
is a chunked-PI-DMA-read helper: for `size` > 0x200 it loops
(`L_800006E4`, asm 0x800006E4-0x800006FC) calling `osEPiStartDma_recomp`
each iteration, testing `$v0` (ctx->r2) for nonzero to decide whether to loop
again (`bne $v0, $zero, L_800006E4`), and only falls through to a blocking
`osRecvMesg_recomp` once $v0 reads 0.

ROOT CAUSE (fn64-abi/src/lib.rs): `osEPiStartDma_recomp` (~line 848) NEVER
WRITES `ctx.r2`. It also takes `ctx: *mut RecompContext` but immediately
reborrows it as `&*ctx` (shared, read-only) -- can't write r2 without
changing that to `&mut`. Real `osEPiStartDma` (WCW Revenge's byte-identical
func_800219B0, refs/WCWnWoRevengeRecomp/disasm/libultra.md line ~213: `if
(!__osPiDevMgr.active) return -1; ... osSendMesg(...); ` -- i.e. returns 0 on
successful enqueue, -1 if the PI dev-manager thread isn't active yet.
fn64-abi's shim performs the DMA synchronously and unconditionally succeeds,
so it should always set `ctx.r2 = 0` (success) after completing.

Because r2 is left stale (whatever value it held from an EARLIER computation
in the caller, non-zero per the observed hang), `func_80000660`'s loop
condition `v0 != 0` is always true -> infinite reissue of the same DMA chunk,
never reaching `osRecvMesg`. This matches DESIGN.md's "tens of seconds, no
crash, no log output" symptom exactly (a real unbounded native loop, not a
missing model).

FIX: change `osEPiStartDma_recomp`'s ctx binding to `&mut`, set `ctx.r2 = 0`
unconditionally after a successful synchronous DMA (matches "all DMAs
succeed once a ROM is installed" -- this shim has no failure path since
`with_pi_dma` panics rather than returning -1 today; a real -1 "devmgr not
active" path only matters if/when the shim becomes async, out of scope
here). Add a regression test asserting r2==0 post-call. Then re-run the
harness and see how far it climbs.

## Fix applied (crates/fn64-abi/src/lib.rs)

- `osEPiStartDma_recomp`: `ctx` binding changed `&*ctx` -> `&mut *ctx`;
  added `ctx.r2 = 0` at the end of every synchronous-completion path (the
  function has no failure path today, so unconditional is correct per its
  new doc-comment "Correction (2026-07-14)" block).
- New regression test
  `os_epi_start_dma_writes_zero_return_value_even_with_stale_nonzero_r2`:
  seeds `ctx.r2` with a realistic stale non-zero value (0x1234) before the
  call (mirroring the real caller's register state), asserts it reads back
  0 -- deliberately NOT zero-initializing ctx.r2 first, since a
  zero-initialized ctx would hide a regression that stops writing r2.

Gates run (all green): `cargo build --workspace`, `cargo test --workspace`
(fn64-abi lib: 27 passed/6 ignored, no regressions), `cargo clippy
--workspace --all-targets` (clean), `cargo fmt --all -- --check` (clean).

## Harness re-run infra notes (for next session)

- `cargo` isn't on PATH by default in this shell -- prepend
  `export PATH="$HOME/.cargo/bin:$PATH"`.
- `RECOMP_H_DIR` must point at an UNMODIFIED N64Recomp `recomp.h`.
  `aki-recomp/vendor/N64ModernRuntime/N64Recomp/include` is NOT clean on
  this machine -- it's been hand-patched with `wcw_atomic_i64`/
  `wcw_atomic_u64` wrapper types that fail to compile as C++ `std::atomic`
  on this toolchain (deleted copy-ctor on `static wcw_atomic_i64
  wcw_watch_addr_cache = -1;` at recomp.h:138, clang errors). Use
  `aki-recomp/refs/WCWvsNWOWorldTourRecomp/lib/N64ModernRuntime/N64Recomp/include`
  instead -- that one compiles clean against
  `games/NWXE/RecompiledFuncs` (verified this session).
- `RECOMPILED_DIR=/Users/jer/Code/aki-recomp/games/NWXE/RecompiledFuncs`,
  `ROM="/Users/jer/Downloads/WWF WrestleMania 2000 (USA).z64"` both
  confirmed present and working on this machine.
- env vars must be set on the SAME invocation as `cargo build`/`cargo run`
  (build.rs reads them) -- `export`-ing then a bare `cargo build` in a later
  Bash call does NOT see them (each Bash call may reset shell state); either
  chain the export and the cargo command in one call, or prefix the command
  directly like `FOO=bar cargo build`.
- No `timeout` binary on this macOS host either (same gotcha as
  faki-tools' CLAUDE.md) -- run the binary as a backgrounded process
  (`&` + `disown`) and use the Monitor tool's poll-loop pattern to bound
  wall time and kill on budget exceeded, not a bare `timeout N cmd`.

## Result after the fix: DMA-poll frontier CLEARED, new frontier found

Re-ran the harness (release build) after the `osEPiStartDma_recomp` fix.
Exit code 139 (SIGSEGV) in well under a second (was: tens of seconds hung
inside one `run_one_step` before). Confirmed via `lldb -o run -o bt`:

```
* thread #1 stop reason = EXC_BAD_ACCESS (code=1, address=0x5da000000)
    frame #0: func_80026F18(rdram, ctx) at funcs_10.c:3552
        ctx->r2 = MEM_W(ctx->r4, 0X0);      // lw $v0, 0x0($a0) -- crashes, ctx->r4 == 0
    frame #1: fn64_abi::osCreateThread_recomp::{{closure}}...  lib.rs:587
    frame #2: fn64_abi::with_active_yielder                    lib.rs:331
    frame #3: fn64_abi::osCreateThread_recomp::{{closure}}     lib.rs:565
    frame #4: fn64_runtime::thread::GameThread::new::{{closure}} thread.rs:175
```

So `func_80026F18` IS a thread entry point (called from inside
`osCreateThread_recomp`'s coroutine bootstrap, per the frame chain) --
NWXE spawned a new thread and its first executed instructions read a
static global `MEM_W(0x800481FC)` (`lui $a0,0x8005; lw $a0,-0x7E04($a0)`,
funcs_10.c:3520-3524) and immediately dereference it as a vtable-style
function pointer (`lw $v0,0($a0); jalr $v0`). `ctx->r4` reads back exactly
0 -- confirmed via `lldb -o "p ctx->r4"` -- i.e. that global was never
populated before this thread ran.

Found the writer: `func_800236C0` (funcs_9.c:36-44) is a trivial one-line
setter -- `sw $a0, -0x7E04($at=0x80050000)` (stores its OWN `a0` argument
into that exact global, no other logic). There is a matching getter,
`func_800236A4` (funcs_9.c:4-14), which reads a DIFFERENT global
(0x8008_3A68) -- looks like a getter/setter accessor PAIR for some
lazily-initialized singleton (game-state pointer, active-actor pointer, or
similar), not part of any static `jal` call graph in
`games/NWXE/RecompiledFuncs/*.c` (grepped for both `0x80026F18` and
`func_800236C0` callers -- zero static call sites for either; both are
only reachable via a computed/jump-table address, consistent with
`func_800236C0` being invoked by name through some registration table and
`func_80026F18` being passed as `entry` to a real `osCreateThread` call
this harness hasn't found the call site for yet).

**Confirmed this session:** `func_80026F18` IS a real `osCreateThread`
entry point -- found the exact call site in its ENCLOSING function
`func_80026DE0` (funcs_10.c:3303-3500ish): at 0x80026EE0
(`ctx->r6 = ADD32(0x8002_0000, 0x6F18)` = 0x80026F18 = entry) ->
`osCreateThread_recomp` at 0x80026EF0 -> `osStartThread_recomp` at
0x80026EF8 immediately after, `a3`(arg)=0 (0x80026EE4). So this genuinely
is the game creating and starting a new thread whose entry point
immediately reads an uninitialized global.

Checked whether `func_80026DE0` itself calls the setter `func_800236C0`
first (the obvious "maybe it's just later in the same function") -- it
does NOT; `func_800236C0` is not called anywhere in `func_80026DE0`'s body
at all. Grepped ALL of `RecompiledFuncs/*.c` for both `func_80026F18` and
`func_800236C0` as call targets -- ZERO static `jal`/`j` call sites for
either. Both are reached only via a computed address (consistent with
`func_80026F18` being passed as a thread-entry function pointer here, and
`func_800236C0` presumably being reached the same way -- a registration
callback of some kind -- from code this session didn't locate).

**Working hypothesis for next session:** this is a genuine scheduling-
order bug in OUR executor (or in what we feed osCreateThread), not evidence
of a second missing shim -- the new thread's entry point assumes some
earlier thread/init step has already called `func_800236C0` to populate
that global, and our cooperative scheduler let this thread run before
that write happened. Next step: find `func_800236C0`'s real caller --
since it's not a static `jal` target anywhere, it must be reached via a
computed address too (a jump table, or passed as a callback pointer the
same way `func_80026F18` was passed to `osCreateThread` -- grep for its
address `0x800236C0`/`800236c0` as a DATA WORD, not an instruction,
across `games/NWXE/RecompiledFuncs/*.c`'s `S32(...)`/`ADD32(...)` literal
constructions building that exact address, the same pattern
`func_80026DE0` used to build `0x80026F18`). Also worth checking: does
`func_80026DE0` itself run on a DIFFERENT thread than the one that's
supposed to call `func_800236C0` first, and is OUR scheduler running
`func_80026DE0` (and thus spawning+starting this new thread) before that
other thread has had a chance to run at all -- i.e. a real priority/
ordering bug in `fn64_runtime`'s scheduler, not a missing shim.

**Harness limitation surfaced this session:** `write_trace_file` (called
at the very end of `examples/wm2000-boot/src/main.rs`'s `main()`, after the
step loop) never runs on a SIGSEGV -- the trace file is NOT written on a
crash, only on a clean step-budget-exhausted/idle-steady-state exit. This
means `/tmp/wm2000-boot-trace.jsonl` was NOT updated by this session's run
(still absent after the crash -- confirmed, `ls` reports "No such file").
Worth a future fix (flush trace events incrementally, or register a panic/
signal hook) so a crash mid-boot doesn't lose the whole trace, but out of
scope for this session's task (root-cause the DMA-poll frontier).

## Gates status (per task's own bar)

- `cargo build --workspace`: green.
- `cargo test --workspace`: green (all pass, no regressions; new r2 test
  added and passing).
- `cargo clippy --workspace --all-targets`: green.
- `cargo fmt --all -- --check`: green.
- Trace file growth: NOT satisfied this session -- see harness limitation
  above. The task's own gate ("trace file grows past 3 events") could not
  be met because the new frontier is a crash, not a clean exit, and the
  harness only serializes the trace on clean exit. `/tmp/wm2000-boot-
  trace.jsonl` remains absent (pre-fix it had exactly 1 stale event from an
  even earlier run; that stale file was deleted this session and never
  regenerated because every subsequent run either got externally killed or
  crashed before reaching the write call).
- Framebuffer PNGs: none produced (boot didn't reach VI bring-up this
  session either, consistent with the new frontier being pre-VI).


## Session 2026-07-14 (part 2): incremental trace flush + rung-3 investigation

### Task 1: crash-safe trace flushing (DONE)

Root cause: `TraceLog` only ever buffered events in an in-memory `Vec`;
`write_trace_file` was called once, at the very end of `main()`, after the
step loop returns normally. A SIGSEGV mid-boot (as happened at the
`func_80026F18` frontier last session) skips that call entirely -- zero
trace on disk despite dozens of real events having happened.

Fix: `TraceLog` (`crates/fn64-runtime/src/trace.rs`) gained an optional
`sink: Option<File>` field + `set_sink_file(path)`. `record()` now writes
+ flushes the new event's `Debug` line to the sink (if armed) BEFORE
pushing to the in-memory Vec -- every single `record()` call is durable on
disk immediately, no buffering, no `Drop`/destructor dependency (a SIGSEGV
runs no destructors, so anything relying on `Drop` would still lose data).
Plumbed through `Executor::set_trace_sink_file` and
`fn64_abi::set_trace_sink_file`. `examples/wm2000-boot/src/main.rs` now
calls `fn64_abi::set_trace_sink_file("/tmp/wm2000-boot-trace.jsonl")`
BEFORE `boot_thread0`, so trace events survive any crash from that point
on. The old end-of-run `write_trace_file(&trace, ...)` call is kept too
(harmless double-write on a clean exit, since the incremental sink already
covers it).

Regression tests added in `crates/fn64-runtime/src/trace.rs`'s new `tests`
module: `record_flushes_each_event_to_the_sink_file_immediately` (reads
the file back after 1, then after 3, `record()` calls -- never calls any
flush/close/shutdown method, simulating "the process is killed right
here") and `record_without_a_sink_does_not_touch_the_filesystem` (no-sink
case is unchanged).

Gates: `cargo build --workspace`, `cargo test --workspace` (34+10 passed,
no regressions), `cargo clippy --workspace --all-targets` (clean),
`cargo fmt --all -- --check` (clean, incl. the example crate).

### Task 2/3: rung-3 investigation (func_800236C0 writer / scheduling order)

See below for findings on why `0x800481FC` is unwritten when
`func_80026F18` runs.

**Confirmed call chain (fully traced, `games/NWXE/RecompiledFuncs/*.c`):**
thread 0 (`recomp_entrypoint`, boot) reaches the overlay-selection loader
(`func_800222D8`, reached only via computed address -- `overlays.json`'s
own documented `loader_fn`, part of the real `LoadOverlay`
dispatch) -> `func_80003720` -> `func_800228C0` (calls `func_80001410`
first, which creates a DIFFERENT thread id 3/entry `func_800033D4`/pri
0x46, but that whole block is gated behind a `MEM_BU` flag byte at
`0x8003BAD4` that reads 0 on a fresh boot -- confirmed via the branch at
`funcs_0.c:2502`, so this create is SKIPPED, not a duplicate-id panic) ->
`func_80026DE0` (called exactly once, `funcs_8.c:7405`, with `r4=$s3,
r5=0x80083A44, r6=2`) -> creates thread id **3**, entry `func_80026F18`,
pri read from a field of the `$s2` arg struct, `t=0x80083AE0`, `arg=0` ->
`osStartThread`. Thread 0 then returns (its own call chain unwinds, "step
1", matches the harness's own doc comment about this being expected).
Next `run_one_step` schedules thread 3 (confirmed via the incremental
trace: `seq 316: ThreadSwitch { to: 3, reason: Scheduled }`, last event
before the crash) -- thread 3's FIRST TWO INSTRUCTIONS read the global at
`0x800481FC` and `jalr` through it as a function pointer -> SIGSEGV.

**Writer search (exhaustive, three independent methods, all agree):**
1. Static grep for the only valid `lui`/16-bit-offset encoding of
   `0x800481FC` (`0x8005_0000 - 0x7E04`, verified this is the ONLY
   `lui`+`addiu` pair that produces this exact address -- checked all
   `hi in 0x8000..0x8010`) across all 51 `RecompiledFuncs/*.c` files:
   exactly ONE writer, `func_800236C0` (`funcs_9.c:36-46`, a one-line
   `MEM_W(-0x7E04, ctx->r1) = ctx->r4` setter) and zero call sites for it
   anywhere in the corpus (grepped both as a call target and as a raw
   32-bit data word across the WHOLE ROM file -- zero occurrences either
   way, so it's not reached via a jump table in `.rodata` either).
2. Cross-checked `func_800236C0`'s only static caller candidates
   (everything near it in the file, `func_800228C0`/`func_80026DE0`/
   `func_80022540`) -- none of them call it or write that offset;
   `func_80022540` is a confirmed, documented, deliberate no-op stub
   (`profile.toml`'s rung-10 "osDriveRomInit... meaningless on cart
   hardware" writeup), unrelated.
3. **Empirical confirmation**: set an LLDB hardware watchpoint on the real
   rdram-backed byte address of `0x800481FC` right before `boot_thread0`,
   ran to the crash -- the watchpoint fired exactly ONCE, at the moment
   LLDB armed it (reading the pre-existing 0 value), and never fired again
   before the SIGSEGV. Nothing writes this global at any point during this
   boot run, confirmed by instrumentation, not just static analysis.

**Conclusion:** this is NOT a scheduling-order bug in `fn64_runtime`'s
executor (thread 3 is pri 0x46=70, far higher than thread 0's pri 10, so
running it as soon as it's created+started and thread 0 yields is the
libultra-correct choice -- real hardware can't preempt mid-instruction
either, only at the next scheduling point, which is exactly where thread 0
returning gives it up). It is also not a missing shim -- there is no
`osXxx_recomp` in play here, just recompiled game code reading its own
global. The genuine finding: `func_800236C0`'s ONLY possible caller, per
three independent checks, does not exist in the reachable call graph of
this ROM at all -- either (a) N64Recomp's function-boundary detection
missed a real call site that uses an addressing mode this project's static
grep methodology doesn't cover (e.g. a `$gp`-relative small-data access --
checked, zero `ctx->r28` usage anywhere in this corpus, so not this), or
(b) the real N64 hardware ALSO never calls it before this point and
relies on something in `func_80026F18`'s own prologue (not yet reached at
the crash address, offset +0x0-+0xC) to short-circuit before the
dereference -- worth re-examining the FULL disassembly of
`func_80026F18` for a skipped conditional branch our `RECOMP_FUNC`
transcription may have mis-ordered, since this project's own scar-tissue
list already has one prior "misread jal delay slot" bug in this exact
boot path (profile.toml's rung-10 correction). This needs either (1) a
byte-level re-disassembly pass on `func_80026F18`'s raw `.text` bytes
(not the already-transcribed `RecompiledFuncs/*.c`, in case N64Recomp's
own codegen dropped/misordered a guard), or (2) accepting this as the
current frontier and reporting it as a real, unresolved gap rather than
inventing a shim. Recommending (1) as next session's first move --
NOT climbing past this point with a guess.

Gates this session: `cargo build --workspace`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets`, `cargo fmt --all -- --check`
all green (unchanged from Task 1's report -- rung-3 investigation made no
further code changes, read-only RE + one empirical lldb watchpoint check).

**Byte-level re-disassembly check (ruling out a transcription bug):**
Read the raw ROM bytes for `func_80026F18`'s first 16 instructions
directly (rom offset computed from section 1's registered
`rom_addr=0x1050`/`ram_addr=0x80000450`, vram 0x80026F18 -> rom 0x49f18):
`3c038008 8c633c98 3c048005 8c8481fc ...` decodes to exactly
`lui $v1,0x8008 / lw $v1,0x3c98($v1) / lui $a0,0x8005 / lw $a0,-0x7E04($a0)`
-- byte-for-byte identical to `RecompiledFuncs/funcs_10.c`'s transcription,
no missing branch/guard, no misordered delay slot. N64Recomp's codegen is
faithful here; the unconditional dereference is really in the ROM. Also
confirmed the harness's section-copy is not a boundary bug: `0x800481FC`
falls inside section 1's copied byte range (rom_addr 0x1050 + size
0x4b070), and the ROM bytes AT that exact rom offset (0x48dfc) are `00 00
00 00` -- whatever the linker called this (`.data` vs `.bss`), the bytes
physically in the ROM there are zero, so a verbatim copy is correctly
zero. This is not a segment-boundary miscopy.

**Bottom line for next session:** the writer is real, reachable ONLY on
actual N64 hardware through a path this project's static analysis (call-
graph grep + raw ROM data-word scan + LLDB watchpoint) has now
exhaustively failed to find in the boot-thread's (thread 0's) own
synchronous call chain. Do not re-run the same three checks again without
new evidence. Next productive avenues, in priority order: (a) check
whether `func_800236C0`'s caller is in the MIPS **exception/interrupt
vector** code (`0x80000000`-`0x80000180`ish, cop0 exception handlers) --
none of this session's grepping covered that address range specifically;
(b) check IPL3/CIC boot code executed before `recomp_entrypoint` (outside
`RecompiledFuncs` entirely, since N64Recomp only decompiles the cartridge
image from its own entry point onward) for a pre-`main` initializer that
writes `.data` globals from a second, separate table (some SDKs have an
`_init_data`/`__init_registers` sweep alongside the bss-clear loop already
seen at the top of `recomp_entrypoint`); (c) consider that this specific
thread (id 3, pri 70, entry `func_80026F18`) may be a DEBUG/DEVELOPMENT
path never actually exercised in the shipped game's retail boot -- i.e.
the crash is real but this thread was never meant to run at all on a
retail cart, and the actual bug is further up the chain (something that
should have prevented `func_80026DE0`'s `osCreateThread`/`osStartThread`
call from executing in the first place, matching the profile.toml
precedent of "a discarded return value was actually load-bearing," rung
10's cart-vs-drive-init correction).
