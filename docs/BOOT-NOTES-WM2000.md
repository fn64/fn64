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
crash, no log output" symptom exactly (a real unbounded recompiled loop, not a
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

## Session 2026-07-14: leads (a)/(b)/(c) closed, new hypothesis (d) -- DMA race

All three of the prior session's leads are now RULED OUT with direct evidence,
read-only, no fn64 code changes this session (`cargo build/test/clippy/fmt`
all still green, untouched from last session's report).

**Lead (b) IPL3/pre-main initializer -- ruled out.** Read `RecompiledFuncs`'
entry point directly: `func_80000450` (vram 0x80000450, matches
`aki-recomp/games/NWXE/profile.toml`'s own `main_vram`/`code_start`
donor-config numbers) IS the first N64Recomp-decompiled function; IPL3
(`aki-recomp/games/NWXE/disasm/assets/ipl3.bin`) is generic SDK bootcode
(copy-and-jump only) with no knowledge of a game-specific global like
`0x800481FC` -- there is no "second init table" to find. Confirmed
`func_80000460`->`func_80036498` (=`osInitialize`, aki-recomp's own rung-2
ID) runs right at the top, matching aki-recomp's independently-derived rung-2
trail byte-for-byte. This lead is closed for good, not just this-session.

**Lead (a) exception vectors -- ruled out by exhaustive raw-ROM word scan.**
`python3` byte-scan of the WHOLE `wm2000.z64` (not just `RecompiledFuncs/`)
for the big-endian 32-bit word `0x800236C0` (the writer function's own
address, as it would appear in ANY jump table, vtable, or lui/addiu-split
pointer-construction site): **zero hits, anywhere in the ROM.** This is
stronger than last session's per-file grep -- it covers overlay banks,
rodata, and any exception-vector region too, since it scans raw bytes, not
just the resident `RecompiledFuncs/*.c` corpus. `func_800236C0`'s address is
not stored as data anywhere in this cartridge image. Combined with last
session's LLDB watchpoint (fired zero times pre-crash) and the call-graph
grep (zero `jal`/tail-call sites), this closes the "hidden caller" hypothesis
completely -- not "we didn't find it," but "it structurally isn't there."

**Lead (c) dead/debug thread -- ruled out; this is the AUDIO thread, not a
debug path.** Cross-referenced `func_80026DE0` (thread-3's creator) against
aki-recomp's own donor-verified libultra names: `func_80026DE0` calls
`func_8002B8A0` at asm 0x80026E2C, and aki-recomp's profile.toml has
INDEPENDENTLY, previously named this exact address
`func_8002B8A0 = "osAiSetFrequency"` (donor: revenge, prefix+body-verified,
`games/NWXE/profile.toml:282`) -- confirmed by direct read, not
re-derived. A function that calls `osAiSetFrequency` while building a thread
is the game's **audio-manager bring-up**, not a stray debug thread; AKI
titles are not known to ship audio disabled at retail (audio is core to
every WWF/WCW AKI title). This node is real, load-bearing, and reached on
retail hardware every boot. Confirmed the whole call chain from `main()`
(`func_80000450`) through `func_800038B8`'s `jal func_800228C0` through
`func_800228C0`'s own `jal func_80026DE0` (asm 0x80022A44) through
`func_80026DE0`'s `osCreateThread`/`osStartThread` pair is **unconditional
straight-line code with zero intervening branches at any level** (read all
three function bodies directly in `aki-recomp/games/NWXE/disasm/asm/1050.s`)
-- there is no `MEM_BU`/flag gate anywhere in THIS specific chain (the
`MEM_BU(0x8003BAD4)`-gated create noted last session is a DIFFERENT
`osCreateThread` call, inside `func_800228C0`'s sibling `func_80001410`
subtree, not this one). Also confirmed via aki-recomp's reference boot log
(profile.toml's own rung history) that its ladder reached rung 18 (a
completely unrelated sin/cos-waveform-filler message-queue bug, now
SUSPENDED pending the fn64 runtime swap) WITHOUT ever naming/touching
`func_800236C0` or `0x800481FC` -- meaning the reference boot took a
DIFFERENT branch through the overlay-loader dispatch and never reached this
exact subtree either way; it neither confirms nor refutes a hardware crash
here, it's simply unexercised territory on both runtimes so far.

**New hypothesis (d), best-supported by evidence so far: this is a genuine
async-DMA race, not dead code and not a scheduler bug in the sense of
priority ordering.** `func_800236C0` (the sole writer, `MEM_W(-0x7E04,
$s3/ctx->r1) = ctx->r4`) sits in a tight cluster of 1-2-instruction
get/set accessor stubs (`func_8002368C`..`func_800236C0`, asm
0x8002368C-0x800236C8) for globals `D_80083A68`/`D_80083A6C` and a
struct-relative field at `+0x10` -- classic IDO-era `static inline` header
accessors that got compiled as real out-of-line functions but were meant to
be reached via **register-indirect calls from elsewhere in a struct-method
table**, not a literal `jal`. Given `func_80026F18` itself begins by
reading `0x800481FC` and immediately `jalr`-ing through `*(that_ptr)`
(asm 0x80026F5C-0x80026F64: `lw $v0,0($a0); jalr $v0`), the shape strongly
resembles an **audio-driver vtable/task-pointer that a DMA-completion
callback populates asynchronously** (the same "chunked PI-DMA + mesg-queue
completion" shape this project's OWN Task-1 finding this session's
predecessor already root-caused for `osEPiStartDma_recomp` on a DIFFERENT
subtree -- see this file's Task-1 section above). On real silicon, the PI
DMA that loads the audio driver/task table plausibly completes (and its
completion handler calls the accessor that writes `0x800481FC`) BEFORE the
audio-manager thread (pri 70, very high) gets its first scheduling slot,
because the DMA is already in flight from an earlier point in boot and
real hardware's PI is comparatively slow-but-already-started; if fn64's
executor creates+dispatches thread 3 synchronously right after
`osStartThread` returns (as the trace already showed: thread 3 scheduled
literally the NEXT `run_one_step` after thread 0 returns), it can easily
outrun an async completion that hasn't been modeled as taking "real" DMA
latency at all.

**Checked the sibling-caller asymmetry (done this session):** grepped all 5
neighbor accessors' `jal` callers. RESULT: partial asymmetry, but it
DISPROVES rather than supports hypothesis (d) as stated. `func_80023698`
and `func_800236B0` DO have real callers (2 each), but on inspection they
are the SAME early-boot function `func_80003720` (thread 0's synchronous
body -- the very function fn64's own notes already identified as
`func_800228C0`'s caller) calling them RIGHT AFTER `func_800228C0` already
returned, to bcopy-init unrelated per-struct fields at offset `+0x10` on
DIFFERENT struct pointers (`D_800571E8`/`D_800571EC`/`D_800571F0`-derived,
not `D_800481FC`) -- i.e. `func_800236B0` is a same-shaped-but-different
accessor (writes a passed-in struct's `+0x10` field, not the fixed
`0x800481FC` global `func_800236C0` writes), and it runs SYNCHRONOUSLY,
already-returned, no race, before `func_80003720` even returns to ITS OWN
caller. `func_8002368C`, `func_800236A4`, and `func_800236C0` (our actual
writer) remain the only 3 of the 6-accessor cluster with ZERO callers
anywhere. This is NOT the DMA-completion-callback evidence hypothesis (d)
predicted -- it just confirms the accessor-cluster pattern is real (siblings
ARE called directly elsewhere in ordinary boot code, ruling out "N64Recomp
never emits jal to this cluster" as a blanket codegen theory) while leaving
`func_800236C0` specifically unexplained. Retracting hypothesis (d)'s DMA-
race framing as unsupported by this check; downgrading back to: the writer
is real, reachable code with a real call site that exhaustive static+dynamic
methods (now 4 independent techniques across 2 sessions) cannot find in
this ROM's own text -- open question, not resolved.

**Bottom line, unchanged from last session's most defensible conclusion:**
the deref chain is unconditional, reachable on every boot, and is NOT dead/
debug code (confirmed: it's the audio-manager thread via the
`osAiSetFrequency` sibling-call cite). Its writer genuinely cannot be found
by any static or dynamic method tried across 2 sessions (grep, raw-word
scan, LLDB watchpoint, sibling-caller diff). Given aki-recomp's reference
boot has NEVER reached this exact call chain either (rung 18 suspended on
an unrelated subtree, confirmed this session by grep), there is no
reference-boot evidence either way that a real N64 would crash here or
sail through it. **Recommended next action, not done this session (out of
budget): stop trying to find a missing writer in the ROM's own code, and
instead check whether N64Recomp's OWN codegen for `func_80026F18` correctly
lowered its first ~3 instructions** -- i.e. re-verify byte-for-byte (as last
session already did once) but this time also diff against the ACTUAL
generated `RecompiledFuncs/funcs_10.c` C body character-by-character
(not just re-reading raw MIPS) for a subtly wrong `ctx->r4` vs `ctx->r5`
register mislabel N64Recomp might introduce silently, since a wrong-register
read (not a wrong branch) would look identical to "unconditional real deref"
in every check performed so far but would actually be a HOST-side
transcription bug, not a hardware-faithful one. No fix landed this session;
frontier unchanged; all evidence-gathering was read-only RE (lldb/grep/
python/disasm reads only), zero `fn64/` code touched, no framebuffer/PNG
milestone reached.

## Session 2026-07-14 (part 3): char-level codegen diff (Task 1) -- CLOSED, codegen is faithful

Did exactly the recommended next action: a char-level diff of
`func_80026F18`'s generated C (`aki-recomp/games/NWXE/RecompiledFuncs/funcs_10.c:3514-3562`)
against its raw MIPS, instruction by instruction, specifically hunting a
`ctx->r4`-vs-`ctx->r5` (or any other register) mislabel that would look
identical to "unconditional real deref" in every prior check.

**Result: no mislabel. Codegen is provably correct at every instruction in
this prologue.** Walked all 6 relevant instructions:
- `lui $v1,0x8008` -> `ctx->r3 = ...` ($v1=r3, correct)
- `lw $v1,0x3C98($v1)` -> `ctx->r3 = MEM_W(ctx->r3, 0x3C98)` (base=r3=$v1, correct)
- `lui $a0,0x8005` -> `ctx->r4 = ...` ($a0=r4, correct)
- `lw $a0,-0x7E04($a0)` -> `ctx->r4 = MEM_W(ctx->r4, -0x7E04)` (base=r4=$a0,
  reads `0x800481FC`, correct -- this IS the global read)
- `lw $v0,0x0($a0)` -> `ctx->r2 = MEM_W(ctx->r4, 0x0)` (base=r4=$a0 still
  holds the just-read global, dest=r2=$v0, correct -- this is the vtable
  dereference)
- `jalr $v0` -> `LOOKUP_FUNC(ctx->r2)(rdram, ctx)` (correct register)

Also independently re-verified the `MEM_W(offset, reg)` macro itself isn't
a red herring: despite the *parameter names* in `recomp.h` reading
`MEM_W(offset, reg)`, the call sites pass `MEM_W(base_expr, literal_offset)`
-- opposite of what the names suggest. This is NOT a bug: the macro body
is `rdram[(reg)+(offset)-0x80000000]`, i.e. plain addition, so argument
order is commutative and irrelevant. Confirmed this exact calling
convention (`MEM_W(ctx->rN, offset)`) is used identically hundreds of times
across the whole `RecompiledFuncs` corpus (grepped), and confirmed
`vendor/N64ModernRuntime/N64Recomp/include/recomp.h`'s macro body is
byte-identical to `refs/WCWvsNWOWorldTourRecomp`'s independent copy (only
the `WCW_WATCH_ENABLED` tracing wrapper differs, itself already understood
from rung-3's watchpoint work). Also confirmed `recomp_context`'s `rN`
fields are declared in flat MIPS register order (`r0..r31`), so
`ctx->r2`/`ctx->r4` really are `$v0`/`$a0` with no indirection to get
wrong.

**This definitively resolves the task's Task 1 branch: proceed to Task 2
("if codegen-correct: it IS a genuine null on the real path").** Combined
with the prior 2 sessions' 4 independent methods (call-graph grep, raw-ROM
32-bit-word scan, LLDB hardware watchpoint, sibling-accessor caller diff)
that already ruled out every writer hypothesis in the ROM's own reachable
text, the writer of `0x800481FC` is not findable by any static or dynamic
technique available on this ROM image. Do not re-attempt a 5th "find the
writer" pass without genuinely new evidence (e.g. a leaked/decompiled
NWXE source tree, which is not available -- unlike faki-tools' `some-mercy`
reference for NW4E, there is no equivalent decompiled-C reference for
WM2000/NWXE in this project).

### Task 3 scoping: why no code fix landed this session

Considered three seams for "fix at the true seam," per the task's own
framing:

1. **Upstream ROM/codegen patch** (aki-recomp's `[[patches.hook]]`
   mechanism, precedented at `games/NWXE/wm2000.toml:187-230` for exactly
   this kind of "splice a guard/fixup ahead of a vram address" need) --
   **blocked by this session's own read-only boundary on aki-recomp.** This
   is the seam the existing precedent (rung-15's `func_80015250` hook, and
   the rung-2 `func_800004D0` instruction patch) would normally use. Next
   session with write access to aki-recomp should add a
   `[[patches.hook]] before_vram = 0x80026F5C` in `wm2000.toml` that
   short-circuits the `jalr` (skip to `L_80026F6C`, mirroring how
   `func_80026F18`'s own `goto after_0` control flow already handles the
   "call returned" case) when the global reads exactly 0 -- narrowly scoped
   to this one call site, not a blanket null-tolerance change.
2. **`fn64-abi::get_function` null-tolerance** -- rejected. `get_function`
   is the single shared resolver for all 85+ `LOOKUP_FUNC` call sites in
   NWXE; making vram=0 silently no-op there would mask genuinely different
   bugs at every other unrelated call site (a real "resolver returned null
   because of a section-registration bug" would then also silently
   no-op instead of loudly failing, defeating its whole "resolve or panic"
   contract documented at `lib.rs:485`).
3. **`fn64-runtime` scheduler reordering** -- rejected, re-confirmed this
   session by reading `Executor::run_one_step`/`pick_next`
   (`crates/fn64-runtime/src/executor.rs:565-626`): the executor is
   strict-priority-preemptive at every scheduling point (`pick_next` always
   returns the highest-pri runnable thread), which IS the libultra-correct
   behavior real hardware also exhibits at its own scheduling points (a
   just-started pri-70 thread preempting a yielding pri-10 thread is
   correct, not a fn64 bug to "fix" by artificially delaying it). There is
   no ordering bug to correct here -- confirmed, not just asserted.

**No `fn64/` code changed this session.** Landing a speculative guard
inside `fn64/` itself (e.g. special-casing vram 0x80026F5C by address in
the executor or ABI layer) was considered and rejected as the wrong
architectural seam -- it would hide a real gap behind fn64-side plumbing
rather than fixing/documenting it at the actual point of uncertainty (the
game code's own missing-writer mystery), and no regression test could
meaningfully distinguish "correct faithful guard" from "papered-over
crash" without knowing the real writer's semantics. Recommending the
aki-recomp `[[patches.hook]]` route (option 1) as the concrete next step,
owned by whoever next has write access to that repo.

Gates this session: `cargo build --workspace`, `cargo test --workspace`,
`cargo clippy --workspace --all-targets`, `cargo fmt --all -- --check` all
still green (read-only RE session, zero `fn64/` code touched). Frontier
unchanged: SIGSEGV at `func_80026F18`'s `jalr` through
`MEM_W(0x800481FC)==0`, thread 3 (audio-manager), immediately after thread
0 yields. No new framebuffer/PNG milestone.

## Session 2026-07-14 (part 4): rung-3 fix landed in aki-recomp -- CLEARED, new frontier is AI-register MMIO

Landed the `[[patches.hook]]` fix in `aki-recomp/games/NWXE/wm2000.toml`
(write access to that repo this session), per the prior session's
recommended seam (option 1). **Two** hooks were needed, not the single
`before_vram=0x80026F5C` the earlier session's note anticipated -- direct
reading of `RecompiledFuncs/funcs_10.c:3562-3577` this session showed
`L_80026F6C` (reached either by falling through the first `jalr` or by
looping back to it) independently re-reads the SAME `D_800481FC` global 3
instructions later (`lui $v0,0x8005; lw $v0,-0x7E04($v0)` at
0x80026F6C-70) before dereferencing `+4` and `jalr`-ing again. A guard only
at the first site would have "fixed" the crash by moving it 3 instructions
into the same basic block, not clearing it. N64Recomp's `[[patches.hook]]`
can only `goto` an address with an N64Recomp-emitted `L_XXXXXXXX` label (a
real branch/jump target inside the function -- confirmed via
`N64Recomp/src/recompilation.cpp`'s label-emission pass), and `0x80026FA4`
(the first post-loop instruction that doesn't touch the global) has no
such label since nothing branches there, only falls through -- so "skip
the whole loop in one hook" wasn't expressible. Landed as two independent
hooks instead, one per dereference site, each jumping to the nearest
already-existing post-call label (`after_0`/`after_1`) and synthesizing
the state a real "call returned, nothing registered" would leave (notably
`ctx->r19`/$s3, read at 0x80026F90 right after hook B's landing point --
missed in the first draft of hook A, caught by tracing what `after_0`'s
bypassed instructions would otherwise have set, before ever running it).

Faithfulness argument (now in `wm2000.toml`'s own comment, not just here):
this models "the audio-dispatch table has zero entries" rather than
inventing a codegen fixup -- consistent with 2 sessions' worth of
exhaustive writer-search evidence that `D_800481FC` is never written
anywhere in this ROM's own static or dynamic closure. Real hardware would
also mis-execute a null indirect call at this exact site if its dispatch
table were genuinely empty; the same latent-bug class as NW4E's own rung-7
precedent (a discarded/never-populated pointer that happens not to matter
on the path actually exercised).

**Regenerated + rebuilt + ran. Rung 3 CLEARS.** `N64Recomp wm2000.toml`
exit 0 (no hook-target errors). Rebuilt `wm2000-boot` from a genuinely
clean `target/` (a stale build initially reused cached objects and still
showed the old crash -- `cargo clean -p wm2000-boot` alone did NOT clear
the build-script output dir on this machine; had to `rm -rf
examples/wm2000-boot/target` to force real recompilation of the 51
`RecompiledFuncs/*.c` files -- worth remembering next time a fix
"doesn't take effect"). Confirmed via LLDB (`bt` on the resulting SIGSEGV):

```
* thread #1 stop reason = EXC_BAD_ACCESS (code=1, address=0xaf450000c)
    frame #0: func_8002B890 + 8   (ldrsw x8, [x0, x8]  -- MEM_W read)
    frame #1: func_80026F18 + 640
```

`func_80026F18` is now 640 bytes further into its own body than the old
crash (which was at its first ~0x40 bytes) -- the null-jalr guard worked,
execution reached `func_8002B890` (one of the two neighbor calls after the
guarded dereferences: `lui $v0,0xA450; ori $v0,$v0,0xC; lw $v0,0($v0)`,
i.e. reading the real N64 AI hardware register `AI_STATUS` at
`0xA450000C`). The trace file (`ThreadSwitch`/`QueueOp` granularity only)
shows the same last event (`seq 316: to:3, Scheduled`) as before the fix
purely because the harness's trace only records scheduling events, not
individual instructions -- it does NOT mean the fix had no effect; LLDB is
the authority here, and it shows a different crash site 640 bytes deeper.

**New frontier: this harness has no MMIO shim for the `0xA45xxxxx` (AI,
Audio Interface) hardware register range.** `rdram` is allocated as a flat
8MB (`0x800000`) buffer (`examples/wm2000-boot/src/main.rs:193-194`);
`MEM_W`'s `rdram[(reg)+(offset)-0x80000000]` translation on
`reg=0xA450000C` computes offset `0x2450000C`, ~4x past the buffer's end
-- an out-of-bounds host read, not a null-pointer fault. This is
qualitatively different from the rung-3 bug: it's a missing piece of
hardware-register infrastructure (the AI register space needs its own
backing store/dispatch, the same way libultra-level `osXxx_recomp` shims
exist in `fn64-abi` for OS-level calls), not a game-logic null. Real
`0xA4500000-0xA450000F` on N64 hardware are `AI_DRAM_ADDR`/`AI_LEN`/
`AI_CONTROL`/`AI_STATUS` -- this thread (already identified as the
audio-manager, `osAiSetFrequency` sibling-call cite from an earlier
session) reading `AI_STATUS` directly is exactly the shape expected of
audio-manager bring-up code, so this is real, reachable, retail-boot code
hitting a genuine harness gap, not dead/debug code.

**Not fixed this session (scope: task was specifically the null-jalr
guard + climb as far as budget allows; an AI-register MMIO shim is a
distinct, larger infrastructure task, not a one-line guard).** Next
session's likely shape: give `fn64-abi` (or a new small MMIO module) a
backing map for the `0xA4xxxxxx`/`0xA8xxxxxx`-family hardware register
windows (AI/VI/PI/SI, same pattern N64ModernRuntime's own `librecomp` uses
for `osXxx_recomp` shims), starting narrowly with just `AI_STATUS`/
`AI_DRAM_ADDR`/`AI_LEN`/`AI_CONTROL` since that's what this exact call
site needs, rather than a blanket MMIO framework speculatively covering
registers nothing has touched yet.

**Did not reach VI bring-up, controller probe, or `osViSwapBuffer` this
session** -- the AI-register frontier is before all three in this thread's
own call chain (this is thread 3's very first few instructions past the
dispatch-loop prologue). No framebuffer/PNG produced. `first_frame` not
reached.

Gates this session (fn64 MAIN workspace only -- `examples/wm2000-boot` is
a deliberately separate standalone workspace per its own `Cargo.toml`
header comment, not part of `--workspace`): `cargo build --workspace`
clean, `cargo test --workspace` 27+1+34+10 passed/6 ignored/0 failed,
`cargo clippy --workspace --all-targets` clean, `cargo fmt --all --
--check` clean -- all unchanged from before this session since zero
`fn64/` source was touched (only `aki-recomp/games/NWXE/wm2000.toml`
changed). `wm2000-boot` itself isn't gated by these commands (by design);
its own gate is the N64Recomp regen (exit 0, confirmed) + the harness run
(now crashes at a materially later point, confirmed via LLDB backtrace,
not just trace-file inspection which is too coarse-grained to show the
difference on its own).

## Session 2026-07-19: C++ harness revived, audio manager PUMPS, discovery lane boots

Lane 1 (N64Recomp-C harness) had rotted against the working tree's C++
MMIO-proxy migration; four fixes revived it and moved the frontier a long
way past the old AI-register crash:

1. **build.rs jr_addend rewrite** -- aki-recomp's regenerated
   `RecompiledFuncs` now carry jump-table temporaries
   (`gpr jr_addend_X = <expr>;`) that other case arms `goto` past. Legal
   C11, hard error in C++. `examples/wm2000-boot/build.rs` now splits each
   into declaration + assignment (byte-identical semantics) into OUT_DIR
   copies before compiling.
2. **RECOMP_FUNC `inline` bug** -- `fn64_mmio_proxy.h`'s clang branch
   defined `RECOMP_FUNC` as `extern "C" inline ... weak`; a C++ `inline`
   function never called in its own TU emits NO symbol, so all 51 generated
   objects were EMPTY (link failed on `recomp_entrypoint`). `weak` alone
   provides the duplicate tolerance; `inline` removed.
3. **Cart handle** -- the new PI fabric requires
   `set_cart_rom_handle_vram`; NWXE's osCartRomInit (`func_80022540`)
   returns its OSPiHandle BSS at `D_800839A0` (disasm/asm/1050.s,
   `addiu $v0,$s0,%lo(D_800839A0)` at 0x80022578). Registered in main.rs.
4. **NDEBUG** -- generated code carries aki's `NAN_CHECK` debug asserts;
   0.0/0.0 in genuinely-uninitialized-BSS math aborted the boot. Real
   hardware propagates NaN silently; build.rs now defines NDEBUG.

**Hook C landed in aki-recomp `wm2000.toml`**: boot progressed past hooks
A/B into a THIRD dereference of the never-written `D_800481FC` dispatch
table -- `0x8002703C: lw $v0,0x8($v1)` (table+0x8) -- observed as SIGSEGV
at host `rdram+0x80000008` (guest null+8). Same "nothing registered"
guard, landing on `after_9`, synthesizing the bypassed delay slot
(`$a0=$sp+0x10`). Regenerated, rebuilt clean, cleared.

**C-lane raw MMIO now charges guest time**
(`fn64-abi/src/lib.rs::fn64_c_mmio_read_w/write_w`, 32 cycles/access, only
when a coroutine is active): the audio manager's raw
`while (AI_STATUS & FULL)` poll (via `func_8002B890/80`, AI_STATUS/AI_LEN
reads) used to spin forever inside ONE scheduling slice because AI drain
deadlines only fire as virtual time advances. With the charge, **the audio
manager works end-to-end**: it feeds 0x8A0-byte buffers at 28805 Hz every
~1.8M cycles (19 ms -- correct real-time cadence), the AI FIFO
fills/drains, and thread 6 wakes once per buffer.

**New Lane 1 frontier (20M-step run, sim_time 802M ~= 8.5 s virtual):**
threads: 0 dead (expected), 3 = audio pump (19,999,788 of 20M slices),
6 = per-buffer consumer (104 wakes, re-blocks on queue 0x838B8),
1 = ran exactly twice then blocked forever. vi_swaps=0, gfx_tasks=0.
The rest of boot waits on something never delivered -- plausibly a
notification one of the SKIPPED empty-dispatch-table handlers
(D_800481FC entries 0/1/2, hooks A/B/C) would have sent. The rung-3
mystery is now load-bearing: the table's writer is still unfound, and its
handlers appear to be what advances boot past audio bring-up. Exit-time
teardown also aborts (`panic_cannot_unwind` force-unwinding a coroutine
parked inside extern "C" `fn64_c_mmio_read_w`) -- cosmetic, post-summary,
worth an `extern "C-unwind"` follow-up.

## Lane 2 same session: wm2000-block-boot -- the DISCOVERY lane executes real ROM code

New standalone example `examples/wm2000-block-boot`: build.rs runs the
REAL fn64-discover pipeline on the ROM (`run_discovery` ->
`compose_materialized_bank_v1` -> `emit_block_pack_v1` ->
`materialize_block_pack` -> `emit_materialized_bank_runner`), emits the
197-block/1,039-word sparse runner + pack consts into OUT_DIR; main.rs
installs them via `ExecutableRegion`/`BlockProgram` and boots through
`fn64_abi::recompiled::boot_thread0_block_program`. Zero aki-recomp
metadata, zero N64Recomp C, zero game bytes in-repo.

Climb log (each a real frontier cleared):
- `CpuException` missing from gate_b2/gate_static_closure's emitted-runner
  import wrapper (345 rustc E0433s) -- fixed, gates green again.
- `mtc0` COP0 reg 18 (WatchLo): SDK boot disarms the watchpoint; emitter
  + `RecompContext` now round-trip WatchLo/WatchHi (18/19) as stored
  state, no watch-exception modeling.
- **Current frontier**: typed `Rdram::load_w` at guest `0xBFC007FC` -- the
  PIF terminate-boot status word osInitialize polls (rung-2's handshake).
  The typed lane's MMIO window models RCP `0xA4xxxxxx` only; the PIF RAM
  window (`0x1FC007C0-0x1FC007FF`) is not mapped. The discovered pack
  genuinely executes the bss-clear + osInitialize prologue to get there.

Gates: `cargo build/test/clippy/fmt --workspace` all green (51 suites, 0
failures). `gate_b1`/`gate_b2`/`gate_d1`/`gate_static_closure` all pass;
NWXE grade unchanged at 26 exact + 3 coarse + 0 wrong.

## Main-line verification run (2026-07-19, after session-patch port)

Ran the harness on the MAIN checkout (which now seeds IPL3 + resident
sections into rdram, this morning's parallel work). The session-patch
ports all engage correctly: cart handle consumed, NDEBUG active, and the
first VI presentation initially tripped `present_render_backend`'s "no
render backend registered" trap -- wm2000-boot now registers the software
`ReferenceBackend` (320x240, auto-dump `/tmp/fn64-wm2000-render-*`), same
pattern as oot-boot minus the RT64/env switch.

**New main-line frontier -- duplicate thread id 3, and it's REAL game
behavior:** with resident `.data` faithfully seeded, the gate byte at
`0x8003BAD4` reads its actual ROM initializer **0x01** (rom offset
0x3C6D4 -- verified against raw ROM bytes), so `func_80001410`'s
previously-skipped `osCreateThread(3, func_800033D4, pri 0x46)` now runs
(the earlier "reads 0 on a fresh boot" note was an artifact of unseeded
.data), and `func_80026DE0`'s audio-manager create of thread id 3 then
trips `Executor`'s duplicate-id trap (`executor.rs:283`). On real
hardware both creates are legal: libultra's thread id is an informational
tag (identity is the OSThread struct pointer -- the two creates use
different structs); fn64's executor keys threads by numeric id. Thread
identity needs to move to (or be disambiguated by) the OSThread vram
address before seeded-data boots can proceed past audio bring-up.

## Session 2026-07-19 (part 2): thread-identity gap FIXED; retrace loop alive; frame gated on dispatch registration

**Fix landed (main + worktree):** libultra OSIds carry no uniqueness
contract (identity = OSThread struct pointer), so `osCreateThread_recomp`
now detects an OSId collision via the new `Executor::thread_exists` and
keys the executor by a synthetic internal id (from 0xF000_0000 up);
`HostState::thread_guest_ids` preserves the guest-supplied OSId, which
`osGetThreadId_recomp` returns. All thread ops already resolved through
`thread_handles` (OSThread* -> id), so the remap is invisible to guest
code. Regression test:
`colliding_osids_create_two_distinct_threads_and_keep_the_guest_osid`.
NOTE: `docs/COMPLETENESS.md` records shim line numbers -- regenerate with
`scripts/check-nmr-surface.py --write-doc` after edits to fn64-abi or the
nmr_surface test fails on drift.

**Result on the seeded-data boot:** duplicate-id trap cleared; the
formerly-skipped id-3 thread (entry func_800033D4) runs as synthetic
0xF0000000. Boot now reaches a steady 60 Hz VI cadence: thread 6 wakes
exactly once per 1,562,500-cycle field and a 5-queue message cascade
(0x83828/0x838F0/0x838B8/0x838E8/0x835D0-area) cycles every retrace --
144 virtual seconds observed. Lifecycle probes confirmed threads 0 AND 1
both RETURN cleanly at boot (bootstrap threads, not stuck); threads 3
(queue 0x559A0), 6, and 0xF0000000 (queue 0xE0908) all park on
osRecvMesg.

**Frame gate, sharpened:** every live thread waits for messages that only
the D_800481FC-family dispatch handlers would send, and those tables are
still never populated even with fully-seeded .data (the ROM initializer
at D_800481FC's backing bytes is zero; the phantom-writer evidence from
the 2026-07-14 sessions still holds in the seeded world). vi_swaps=0,
gfx_tasks=0. The path to a first frame is now purely the registration
mystery: find what populates the audio/graphics dispatch tables (or what
message source drives the parked game loop) on real hardware. That is an
RE task, not a runtime gap -- the OS layer now idles indefinitely with
correct 60 Hz timing, which is exactly what an N64 with an empty dispatch
table would do.

## Session 2026-07-19 (part 3): the "phantom writer" is CLOSED -- it never existed

Three sessions of writer-hunting for `D_800481FC` rested on one wrong
observation. Direct ROM read this session (offset 0x48DFC = vram
0x800481FC via section 1's rom=0x1050/ram=0x80000450 mapping):

```
0x800481F0: 0x80026C3C   <- dispatch handler 0
0x800481F4: 0x80026CA0   <- dispatch handler 1
0x800481F8: 0x80026D00   <- dispatch handler 2
0x800481FC: 0x800481F0   <- D_800481FC's .data INITIALIZER: points at them
```

`D_800481FC` is initialized DATA, not BSS. The 2026-07-14 note "the ROM
bytes AT that exact rom offset are 00 00 00 00" was simply a misread, and
every downstream conclusion (phantom writer, 4-method search, hooks A/B/C's
"empty table" model) cascaded from it. Verified live: a seeded boot reads
`D_800481FC == 0x800481F0` the moment thread 0's first slice completes; a
raw-ROM scan for jal/j encodings of func_800236C0 (0x0C008DB0/0x08008DB0)
and ori-built forms of the address found zero hits -- consistent, because
no writer is NEEDED.

**Root cause, ROM-agnostic, already fixed:** the C-lane boot host was not
seeding resident `.data` into RDRAM (real hardware's boot copies
rom[0x1000..] wholesale). Main's `seed_resident_sections`/
`seed_ipl3_image` work fixes it generically for every ROM; the
thread-identity remap (part 2) handles the second consequence of faithful
seeding. Hooks A/B/C in aki-recomp's wm2000.toml are now inert (they only
fire on a zero table read) and can be deleted at the next regen.

**White-screen correction:** the shell's white window is the UNPRESENTED
surface (macOS default), not game output -- a 100-second probed shell run
never saw `current_vi_framebuffer()` become Some, printed no "presenting"
line, and observed zero raw SP-register writes. Earlier optimism in this
session's chat log was wrong; the log line to trust is
`[fn64-shell] presenting VI framebuffer (swap #N)`, which has not yet
appeared for NWXE.

**True current frontier (seeded, identity-fixed, dispatch table
populated):** boot reaches the steady 60 Hz retrace loop; threads 0/1
return cleanly; threads 3 (queue 0x559A0), 6 (5-queue retrace cascade),
and synthetic 0xF0000000 (queue 0xE0908) park on osRecvMesg; no VI mode/
origin programming, no SP task submission (NWXE hand-rolls raw-MMIO SP
task launch -- `funcs_15.c` 0xA404xxxx writers -- so when tasks DO start,
the fabric's raw-SP lane, not osSpTaskStart, is the seam to watch).
Next diagnostic: instrument which of the three dispatch handlers run per
retrace and where thread 6's handler chain decides "nothing to do" --
with aki-recomp's own rung ladder (which reached a waveform-filler stage
on the same ROM) as the reference map for what should happen next.

## Session 2026-07-19 (part 4): PIF RAM window + TLB COP0 modeling (ROM-agnostic)

Two hardware-model gaps closed, both found by chasing NWXE but generic:

1. **Direct CPU PIF RAM access** (`0x1FC007C0..0x1FC00800`, KSEG0/KSEG1):
   real hardware exposes PIF RAM to uncached CPU loads/stores, and AKI-era
   hand-rolled joybus code (NWXE links NO osCont*/osSi* shims -- its whole
   controller stack is raw) plus boot-handshake polls read it directly.
   The discovered-pack lane faulted at exactly 0xBFC007FC. Now:
   `DeviceFabric::pif_ram_cpu_read_w/write_w` back the window with the SAME
   64-byte PIF RAM SI DMA uses; routed through the one shared raw-MMIO seam
   (`pi.rs::pif_ram_window_offset` in read/write_raw_mmio_word) so the C
   lane (via a widened `fn64_is_rcp_mmio_word`) and the typed lane both get
   it. ponytail: CPU stores don't run the PIF command interpreter yet (the
   injected executor runs on DramToPif DMA, the path joybus commands use);
   round-trip test in pi.rs.
2. **TLB COP0 registers + `tlbwi` recording** (fn64-recomp-rs): Index/
   EntryLo0/EntryLo1/PageMask/Wired/EntryHi (0/2/3/5/6/10) are stored
   round-trip state, and `tlbwi` RECORDS the staged entry into
   `RecompContext::tlb_entries[32]` instead of trapping -- libultra's
   osInitialize installs a real valid mapping (observed live: EntryHi=
   0xC0000000, EntryLo0=0x02000017) and boot must not die on it. Address
   TRANSLATION through recorded entries stays unmodeled; a mapped-segment
   access faults at the memory path. WatchLo/WatchHi (18/19) same session,
   earlier.

**Discovered-lane climb log this session:** 0xBFC007FC PIF fault ->
mfc0 EntryHi -> tlbwi valid mapping -> NOW: load from KUSEG 0x60900184
(TLB-mapped space). The recorded `tlb_entries` are exactly what a
translation step needs to consult -- the next runtime work item is a
TLB-translate fallback in the typed memory path (fault only when no
recorded entry covers the address).

**C-lane frontier unchanged** (60 Hz retrace idle, no VI programming, no
SP tasks). NWXE's raw-SI controller probe is now unblocked
infrastructure-wise; next diagnostic remains per-retrace handler tracing
against aki-recomp's rung ladder.

Gates: fn64-abi/fn64-runtime/fn64-recomp-rs suites + fmt + lint-docs +
NMR surface doc all green after regen.

## Session 2026-07-19 (part 5): the frame gate is the CONTROLLER QUERY -- mapped to one function

Env-gated boot diagnostics landed (`FN64_BOOT_PROBE=1`, permanent):
osSetEventMesg registrations and raw SI/SP register traffic, in
`fn64-abi` (lib.rs helper, mesgqueue.rs, pi.rs).

**The boot state machine, decoded from the seeded trace + probes:**
- Thread 6, EVERY field: send+recv on queue `D_800B1368` (the SI access
  token -- aki's notes identify func_80032074/func_800320E0 as this
  create-once/acquire pair), then a NON-blocking recv poll on
  `D_80057228` (the OS_EVENT_SI queue, registered via
  `osSetEventMesg(5, 0x80057228)` -- observed live by the probe).
- The poll comes back empty for ~8,500 consecutive fields (~142 virtual
  seconds), then a fallback advances the state machine ONE stage (first
  send to the main thread's `D_800559A0` at sim 13.315B), and the cycle
  repeats on the next queue. The game IS running -- at roughly 1/1000th
  speed, each stage gated on an SI completion that never arrives.
- Root cause location: **zero raw SI register reads OR writes ever
  happen** (probe-confirmed over minutes of wall time). The hand-rolled
  `__osSiRawStartDma` (= `func_80031F70`, identified in aki-recomp's
  rung-18 notes) is never called. Its caller `func_800338B0`
  (acquire -> build 64-byte PIF command block -> jal 0x80031F70,
  STRAIGHT-LINE once entered) is never entered. `func_800338B0`'s call
  sites (0x800045BC/0x800047A0/0x80004804/0x80004850, funcs_1.c) all sit
  inside **`func_800044D0` -- the controller manager**, whose init
  provably RAN (it made the osSetEventMesg(5,...) registration the probe
  saw). Its controller-query calls are conditional and the condition
  never passes.

**NEXT SESSION'S ENTRY POINT (precise):** read `func_800044D0`'s body
(funcs_1.c, vram 0x800044D0 onward) and find the branch guarding the
0x800045BC call to func_800338B0 -- what state must hold for the first
controller query to fire, and which piece of it our runtime leaves wrong
(candidates: a message the manager waits for first, a PIF/SI init flag in
seeded .data, or a queue-capacity check). aki-recomp's own boot DID get
SI traffic (their rung 18 crashed on D_80057228 corruption -- i.e. their
runtime passed this gate), so their librecomp SI path is the behavioral
reference. All boot-probe infrastructure needed to verify a fix is in
place; the 142-second-per-stage fallback also means ANY fix here
collapses ~20 minutes of virtual idle into real boot progress at once.

Gates: fn64-abi/runtime/recomp-rs suites, fmt, NMR surface doc all green.

## Session 2026-07-19 (part 6): osGetCount stub fixed; SI/joybus chain VERIFIED end-to-end

**Root-cause fix landed (aki-recomp metadata + existing fn64 shim):**
`func_80037690` = `osGetCount` -- the innermost counter read of the
hand-rolled `osGetTime` (`func_80032570`) -- had been auto-stubbed by
gen-stubs' cop0 pre-scan (empty body, `$v0` left stale on every call).
Named in profile.toml `[syms.rename]` (evidence comment there), stub
entry removed from wm2000.toml, symbols + RecompiledFuncs regenerated;
`osGetTime` now reads the executor's real Count (probe-verified:
0x4E48769 ticks = 1.75 virtual seconds at the contInit call).

**The controller stack WORKS -- verified byte-level with the new probes**
(`FN64_BOOT_PROBE=1` now also logs `osSetTimer`, `osGetCount`,
`__osSiRawStartDma` calls with 64-byte block hexdumps, and the
`PifToDram` post-execute response in fn64-runtime):
- `func_8002F8E0` (hand-rolled osContInit) runs to completion: builds the
  standard `__osContRequesFormat` block (`ff 01 03 00 ...` x4, `fe`
  terminator, format byte), pumps it through `__osSiRawStartDma_recomp`
  (HOST-side -- note: raw SI register probes see nothing on this path by
  design), and our `execute_pif` answers correctly: port 0
  `05 00 00` (standard controller present), ports 1-3 rx-error 0x80
  (absent).
- The game then write-probes the controller pak (cmd 0x03, addr 0x8001,
  32x 0xFE payload) and receives the no-pak CRC answer; it resets and
  re-queries the channel -- 3 query pairs observed at successive boot
  stages, consistent with periodic pak re-probing, not an SI failure.

**Remaining frontier (unchanged symptom, upstream causes now all
eliminated):** boot stages still advance at the ~142-virtual-second
fallback cadence (~8,500 fields) rather than per-frame. Everything this
session checked -- time source, PIF window, SI DMA, joybus responses,
event registration (two `osSetEventMesg(5, ...)` calls: the contInit-era
stack queue 0x80083930, then D_80057228) -- now behaves correctly, so the
next suspect list narrows to: (a) OS_EVENT_SI delivery going to the OLD
queue after re-registration (check `Executor::set_event_mesg` replace
semantics vs which queue the per-field NOBLOCK poll actually reads);
(b) the game's own state machine intentionally pacing until some
still-unmet condition (audio? gfx microcode load via the raw SP lane?).
The per-field NOBLOCK poll queues in the trace (0x83828/0x838F0/0x838928
area) are func_800044D0-frame STACK queues -- map them to the sp values
of the live threads before assuming they are the SI event queue.

Gates: fn64-abi/fn64-runtime green, fmt, NMR doc regen, lint-docs clean.

## Session 2026-07-19 (part 7): the 142s mystery SOLVED; boot storms through streaming + save init; new frontier is a phantom mq=1

Suspect (a) from part 6 (event re-registration delivery) led somewhere
much bigger. Fixes landed, in causal order:

1. **Device-delivery ordering (the ACTUAL 142s cause).** Guest slices
   charge (nearly) zero virtual time, so every DMA completion's deadline
   (`sim_time + latency`, latency 1) missed the post-slice fabric commit
   (which only reaches `sim_time`) and waited for the NEXT FIELD's idle
   advance -- every completion, one field late. The boot's overlay
   streaming (megabytes in 4-byte and 0x200-byte chunked PI DMAs, via
   `func_800E1C40 -> func_800F4CA4/497C/4B60`) therefore ran at one chunk
   PER FIELD: the "142-second stages" were just multi-thousand-chunk
   copies. Fixes: `fn64_abi::next_device_deadline()` + deadline-aware
   pump in wm2000-boot (service due-before-next-field deadlines without
   letting a chattering device starve the VI tick), and a flat 250-cycle
   charge per C-lane OS-call yield (`suspend_active_coroutine`) so
   OS-call loops consume virtual time like real silicon.
2. **getenv on the PI hot path.** `start_timed_pi_dma` checked
   `FN64_TRACE_PI_DMA` via an UNCACHED `env::var_os` on every DMA
   (lldb-caught at ~26k DMAs/sec). Now OnceLock-cached.
3. **Save-device routing through the PI handle.** Real
   `osEPiStartDma(handle, mb, dir)` scopes `devAddr` by the handle's
   domain. Our shim ignored `$a0` entirely, so NWXE's SRAM init
   (`FromRdram devAddr=0x0` on its own save handle) routed to ROM offset
   0 and its write-verify loop retried forever (the post-streaming
   plateau at trace seq 34,739). Now: traffic on any non-cart handle
   (cart = `set_cart_rom_handle_vram`'s) is save traffic, rebased into
   `SRAM_DOMAIN2_BASE`. Handle-identity heuristic, ponytail-noted for a
   future modeled OSPiHandle. wm2000-boot also registers
   `InMemorySaveStorage(SramBanked)` (it never had save storage at all).
   RESULT: the plateau shattered -- 253k+ trace events, streaming AND
   save init complete, boot deep into new territory (sim 41M+).
4. **Executor queue-mirror bounds trap.** The next frontier SIGSEGV'd
   silently: `mirror_queue_to_rdram` writes guest OSMesgQueue structs
   unchecked. `set_rdram_base_with_len` + a loud assert now name the
   corrupt pointer instead of crashing.

**Current frontier, precisely diagnosed by the new trap:** guest code
calls osRecvMesg with `OSMesgQueue* == 0x1` -- the literal `msg=1` value
from the game's FIRST `osSetEventMesg(5, mq=0x80083930, msg=1)`
registration, consumed somewhere that treats received messages as queue
POINTERS (AKI's manager protocol passes response queues as messages, and
mixes flag-style event messages onto the same queues). Next session:
find which handler recvs on the 0x83930-era queue family and how real
hardware's delivery keeps flag messages and pointer messages apart --
suspect either our event delivery posting to a queue the game considers
retired (the 0x83930 stack frame dies with func_800044D0), or a message
our runtime injects that real hardware wouldn't. Diagnosis env:
`FN64_BOOT_PROBE=1` (all registrations + SI DMAs + handle identities),
`FN64_TRACE_PI_DMA=1` (per-DMA), and the mq-mirror trap message itself.
Repro: run wm2000-boot ~3-4 min wall; trap fires past step 100k at
sim ~41M.

Gates: fn64-abi/runtime/recomp-rs green, fmt, NMR doc regen clean.

## Session 2026-07-19 (part 8): the mq=1 forensics -- thread 6 is executing with $sp inside the SI event queue

Probe ladder (all `FN64_BOOT_PROBE=1`-gated; the NON-POINTER osRecvMesg
diagnostic is now permanent in `mesgqueue.rs`):

1. Guest `$ra` at the bogus recv reads 0 (generated code does not maintain
   r31 across direct C calls) -- use HOST backtraces instead; guest
   function names appear directly (`RECOMP_FUNC` symbols).
2. Host backtrace at `osRecvMesg(mq=0x1)`: `func_80004628` (the
   controller-manager reader from aki's rung-18 notes) <- overlay-bank
   chain `func_800E24D4 <- func_800E2704 <- func_800E1BAC/1B90` <-
   `func_80000870 <- func_800222D8` (thread 6). The OVERLAY GAME LOGIC is
   executing -- boot is in real menu/logo-era code now.
3. All three recv sites in func_80004628 pass the CONSTANT D_80057228;
   each rebuilds `$a0` from `$s0` after `jal func_8002F660`, which
   saves/restores s0 via its own frame. So the "corruption" is in what
   the restore loads.
4. **The punchline:** at the bogus recv, thread 6's `$sp` is
   `0x80057240` -- INSIDE the D_80057228 OSMesgQueue region. The
   "saved s0" slot at sp-8 = D_80057228+0x10 = the queue's msgCount
   field, which our queue mirror faithfully writes as... 1 (capacity).
   Nothing corrupted the stack; the STACK IS THE QUEUE. The msg=1
   coincidence was a red herring.

**Open question for next session (single, sharp):** how does thread 6
come to run `func_8002F660` with `$sp ~= 0x80057240`? Candidates:
(a) an AKI overlay-dispatch stack switch to a static system stack whose
address our boot computes differently (earlier divergence poisons a
stack-base global); (b) sp inherited from a struct field our runtime
seeds differently (osCreateThread sp arg, or a context-switch helper the
game hand-rolls); (c) genuine progressive stack overflow walking down
into .data (0x838xx frames observed earlier are ~0x2C6xx bytes above
0x57228 -- deep but finite; check thread 6's stack base/size in the
game's thread table). Instrument: log per-OS-call (thread, sp>>16)
transitions to find the exact call where sp jumps regions.

Fixes landed this part: executor `set_rdram_base_with_len` + loud
OSMesgQueue-mirror bounds trap (part 7 item 4 refined); permanent
NON-POINTER recv diagnostic. Gates green, NMR doc regenerated.

## Session 2026-07-19 (part 9): it's the MENU PUMP -- sp descent quantified, three ranked causes

Probe results (creation table + sp-region transitions, both now permanent
under FN64_BOOT_PROBE):

- Thread creation table: id1 boot sp=0x8004BE70; id6 overlay-loader
  sp=0x800839A0 (the stack TOP abuts the cart handle struct -- same
  constant, not a conflict); id3 main sp=0x80055940 pri70; id3->synthetic
  0xF0000000 audio sp=0x800E0900 pri80.
- Thread 6's $sp descends MONOTONICALLY (0x839A0 -> 0x80010 -> 0x7BFE8
  -> ... -> 0x57240 at the trap), ~0x550 bytes per iteration, plowing
  through its own OSThread struct at 0x800807F0 long before the trap.
  No guest code anywhere in the corpus loads $sp from memory or adjusts
  it dynamically -- this is pure nested-frame accumulation.
- The loop: `0x800E1BF4: jal func_800E2704; beq $v0, $zero, L_800E1BF4`
  (funcs_16.c, section-2 overlay bank -- N64Recomp DID suffix the
  same-VA twin as `_bank4_text`, so linking is unambiguous). This is the
  game's menu/mode-select pump: iterate until the handler returns a
  selection. On hardware it is sp-NEUTRAL per iteration.
- `execute_pif` now clears the PIF control byte (pif_ram[63]=0) after
  processing, like real hardware -- this alone took per-boot SI
  transactions from 8 to 83 (controller reads now flow per iteration),
  but the pump still never returns nonzero and still leaks stack.

**Ranked causes for next session:**
1. **The C-lane OS-call charge (OUR 2026-07-19 change) makes every OS
   call a TWO-step suspension** (checkpoint yield, then the real yield).
   Audit `Executor` resume delivery for wakes landing on the checkpoint
   suspension instead of the following real Block yield (pending_resume /
   wake_thread interleaving). A lost/misdelivered recv resume would make
   the pump's input read misbehave every iteration. Quick falsification:
   set the charge to 0 (or only charge AFTER the real yield) and see if
   the descent changes.
2. **Phantom menu descend**: the pump may be stacking menu screens on
   misread pad data (neutral pad misparsed as repeated input). Dump
   `read_data_response` bytes vs what func_80004628's parser expects.
3. Genuine generated-code sp-leak on some func_800E2704 exit path
   (least likely -- N64Recomp epilogues are per-exit-site faithful).

Also landed this part: permanent creation-table and sp-region probes,
PIF control-byte clear. Trap/back-stop diagnostics from parts 7-8 all
still in place. Gates green.
