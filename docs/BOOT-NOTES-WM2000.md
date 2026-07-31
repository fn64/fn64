# WM2000 boot session notes (2026-07-14)

2026-07-25 arbitrary-PC update: checked AOT `lw`/`sw` now reaches the installed
word-MMIO hook before RDRAM backing rejection. The private NWXE pack passes the
former `__osSiDeviceBusy` SI-status fault at `0x80038268`. Its host override is
not address-guessed: the build requires one exact six-word implementation,
emits the matched address into the generated pack, and binds only that fact to
the typed adapter. Three independent public-debugger target snapshots showed
the apparent `0x80036f10` TLB refill was a harness bug: the reference `$t8` was
`0xffffffff80048860`, while fn64's `0x60880480` was its exact byte reversal.
The block example now materializes the IPL3 ROM copy through logical RDRAM byte
lanes. Ten corrected runs first stopped identically at the honest sparse-pack
miss for spawned thread entry `0x800004d0`. The pack builder now admits exact
bank-generation PCs from three byte-identical, ROM-bound black-box traces
without treating scenario observations as function-owner proof or exhaustive
support. Because the debugger advances branch+delay atomically, its producer
no longer claims executed-PC exhaustiveness and pack admission adds each
required delay-slot word from verified ROM bytes. The bounded pack contains
1,929 observed PCs plus 289 required delay slots in 90 spans / 2,517 total
words. It passes `0x800004d0`; the first ten consecutive runs then stopped on
a separate runtime memory-model fault at `0x8002a8d8` for cartridge address
`0xffffffffb0000000`. Canonical KSEG0/KSEG1 PI-domain-1 word reads now use the
same installed, read-only ROM source as PI DMA. Ten corrected runs pass that
load and expose a raw guest VI initialization image with V timing and scales
programmed but `H_START` still zero when status is enabled. The independent
public-debugger observation exposes the same values and no H_START transition;
its status transition varies by one adjacent pause, so it is value evidence,
not an instruction-exact timing trace. A zero H or V interval now remains an
inactive retained image while nonzero malformed intervals still trap. Ten
consecutive corrected runs pass that assertion and stop identically at the
separate missing-render-backend frontier, with no prior `AotMiss`.
Gap diagnostics retain current
CP0 context; a non-architectural miss does not commit an exception.

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
handlers appear to be what advances boot past audio bring-up. The former
post-summary teardown abort was not guest execution: TLS tried to force-unwind
a suspended coroutine through extern-C `fn64_c_mmio_read_w`. The shared
terminal `fn64_abi::prepare_process_exit` boundary now detaches only
started/unfinished foreign stacks, discards a retained renderer continuation
at its committed boundary without resuming it, clears saved pointers, and
allows ordinary Rust process teardown; changing individual shims to
`extern "C-unwind"` would not cover the generated-C frames above them.

## Lane 2 same session: wm2000-block-boot -- the DISCOVERY lane executes real ROM code

New standalone example `examples/wm2000-block-boot`: build.rs runs the
REAL fn64-discover pipeline on the ROM (`run_discovery` ->
`compose_materialized_bank_validated_v2` -> `emit_validated_block_pack_v2` ->
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
2. **TLB COP0 registers + management operations** (fn64-recomp-rs): Index/
   Random/EntryLo0/EntryLo1/PageMask/Wired/EntryHi (0/1/2/3/5/6/10) are typed
   state. TLBWI/TLBWR record staged entries into
   `RecompContext::tlb_entries[32]`, TLBR reads them, and TLBP applies
   PageMask plus Global/ASID matching. In the arbitrary-PC lanes fn64 applies
   the public inclusive Random/Wired range through a bounded once-per-charged-
   instruction-unit policy, not a silicon cycle-timing claim;
   legacy whole-function Random/TLBWR stays loud without that clock. Libultra's
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

## Session 2026-07-26: dense resident AOT, compile-budget sharding, and the first separated runtime divergence

`FN64_BLOCK_PROGRESS_ONLY=1` makes the dense block harness stop after its
bounded milestone counters and process-exit cleanup, before enforcing the
overlay-entry release gate. This is an exploratory profiling mode only; its
successful exit is not overlay evidence.

`FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1` keeps the bounded executor running after
the first digest-selected overlay entry instead of stopping at that release
milestone. It exists to locate the next post-entry execution boundary; the
ordinary gate remains stop-on-first-entry so its result has one narrow meaning.
`FN64_BLOCK_PROGRESS_ONLY=1` still writes an explicitly requested
`FN64_BLOCK_PC_TRACE` or `FN64_BLOCK_HOST_TRACE` before preparing process exit.
Progress-only runs also disable retention of the executor and device diagnostic
event vectors by default. The device fabric continues to maintain constant-space
typed transition counters used by the progress report. Set
`FN64_BLOCK_EXECUTOR_TRACE=1` or `FN64_BLOCK_DEVICE_TRACE=1` to retain the
corresponding full history when a diagnosis actually needs it. Release evidence
keeps both histories enabled by default.

`FN64_PROFILE_AOT_PCS` accepts a comma-separated list of hexadecimal guest
PCs and reports exact dense-runner entry counts plus the first and last
observed `v0/v1/a0/a1/a2/a3/sp/ra`, six incoming stack words, COP0
`Status/Cause/EPC/BadVAddr`, FCSR, and selected raw FPR values for each at the
bounded exit. The snapshot is constant-space and sampled only at named PCs.
It is a diagnostic observation list, never an address signature or discovery
input.

`FN64_PROFILE_STOP_AT_PC` accepts one hexadecimal guest PC and ends the
exploratory loop after the scheduler step that first enters that AOT
destination. Pair it with the same PC in `FN64_PROFILE_AOT_PCS` to capture the
transition's first/last GPR and COP0 state without overshooting a late
milestone. It is profiling control only and is never release-gate evidence.

`FN64_PROFILE_RDRAM_RANGES` accepts comma-separated `HEX_VRAM:BYTE_LEN`
ranges, each capped at 256 bytes, and reports architectural bytes at the same
bounded exit. It is observational only and never seeds generated content.

`FN64_PROFILE_AOT_RECENT` accepts a positive destination count, enables the
existing bounded destination ring at exactly that limit, and reports the 20
most frequent bank-qualified PCs in the retained tail. It is intended for
late-loop localization without restoring the default unbounded certification
history to exploratory million-step runs.

`FN64_PROFILE_HOST_RECENT` accepts a positive host-boundary count, bounds the
existing host-boundary ring at exactly that limit, and reports both the 20 most
frequent `(thread, target, phase, resume)` tuples and the final 12 boundaries.
It is the bounded diagnostic alternative to `FN64_BLOCK_HOST_TRACE`; absent
both variables, exploratory runs retain no host-boundary history.

`FN64_PROFILE_CONTROL=1` reports the typed executor control snapshot at the
bounded exit so a localized PC or host-boundary loop can be attributed to its
guest thread and scheduler state. It does not enable any execution history.

The block lane now emits the complete one-MiB IPL3 resident image: 262,144
aligned entries, including data words and precise RI behavior. A monolithic
generated unit was roughly 267 MiB and crossed the two-minute compile gate.
Sixteen content-addressed 64 KiB crate artifacts retain the prescribed shard
identity; each is statically divided into sixteen 4 KiB callable subrunners.
This is still AOT at every entry—cross-subrunner transfers use the normal typed
resolver, with no runtime decoder or interpreter fallback. Full `cargo check
-j4` measured 62.67 seconds, native debug build 107.56 seconds, and an
unchanged rebuild 0.06 seconds. The resulting debug binary is 295 MiB.

Three 400,000-step black-box runs produced byte-identical traces, BootContexts,
and completed-image observations for the four-word general exception preamble
at `0x80000180` (SHA-256
`92d005d9f1c311068500142b0129d6160dd193f92baa1e0f84061a169b48b982`).
That CPU-produced image is a separate generation and its four live words are
compared directly at every matching runner entry; a mismatch computes the full
digest for evidence. Dense execution passes the prior sparse-PC, transitional
VI, and missing-renderer frontiers with no `AotMiss`.

Those private inputs now have a bounded acquisition wrapper rather than a
manual producer recipe: `scripts/capture-wm-executable-image-group.zsh` runs
the public-debugger producer at least three times in isolated private
directories, requires both the image and ROM-entry BootContext even when the
producer exits zero, and validates the group with the canonical discovery
parser under the common 2048 MiB/40%-free guard and Cargo `-j1`. For this known
generation its typed inputs are group
`FN64_EXECUTABLE_IMAGE_GENERAL_EXCEPTION`, image ID
`general-exception-preamble`, capture/first/start PC `0x80000180`, four words,
and 400,000 steps. The wrapper prints only a path-free receipt and keeps all
ROM-derived bytes outside the repository. Its ROM-free fake-producer contract
gate is `scripts/capture-wm-executable-image-group.zsh --selftest`.

Two newly exposed faults were runtime wiring, not AOT closure. The harness now
registers NWXE's typed in-memory 32 KiB SRAM device. RSP RDRAM publication no
longer tries to reborrow the live `BlockProgram` while an AOT `SP_STATUS` store
still owns it: the RSP records the write, the generated instruction exits via
its executable-write boundary, and the outer owner publishes before the next
guest instruction. Ten consecutive dense end-to-end runs cleared that nested
borrow and reached the same later guest fault at EPC `0x800e1ba4`; their panic
text is not byte-identical because Rust includes the process/thread ID. The
focused `fn64-abi` suite passes 318 tests.

Follow-up diagnosis disproved the initial indirect-transfer interpretation. A bounded
typed `jr`/`jalr` provenance ring showed that no register-targeted transfer
selected `0x800e1b90`; the actual source is the direct `jal` at `0x80000884`.
The resident bytes make `0x800e1b90` the delay-slot NOP of a preceding
`jr $v0`, which exposed stale-generation execution. Dense subrunners now bind
each 4 KiB page digest and validate it on first fetch and after an intersecting
guest write. Session-qualified per-page write tokens avoid hashing unchanged
pages at every checkpoint without allowing a fresh boot to inherit an old
verification result.

The corrected run stops before stale execution with a typed `AotMiss` for
`0x800e1400..0x800e2400`: the resident and live digests differ.
At the last common checkpoint before the `0x80003720` initialization call,
the public debugger still observes the resident digest. Three byte-identical
captures immediately after that call observe the same live digest as fn64,
with first
entry `0x800e1b90`; no exact copy of the 4 KiB image occurs in the normalized
ROM, so this evidence classifies it as CPU-produced rather than direct ROM
DMA.

The pack now emits both the resident page and that completed image as distinct
immutable banks. Writes only dirty the registered physical range. At the next
attempted fetch, a typed `ImageChanged` boundary returns without entering the
stale runner; the outer owner matches the completed bytes against the closed
pack, atomically replaces code and runner, preserves the instructions retired
before the boundary, and retries the same PC under the new bank identity. An
unknown digest or later generation remains a loud `AotMiss`; there is no
translator or interpreter fallback. One early end-to-end measurement passed
the old page miss and entered `0x800e1b90` from the captured bank.

The newly exposed frontier is the separate runtime-behavior path already seen
before stale-generation diagnosis: the guest reaches the null-pointer `LHU`
at EPC `0x800e1ba4` and requests the unadmitted `0x80000000` refill vector.
The three one-million-step black-box traces reach neither PC, so fn64-only
vector bytes must not be admitted as closure evidence. This fetch-activation
mechanism is unit-tested, but it has not yet met the ten-run deterministic-fix
bar; the next investigation is the first runtime/input-state divergence that
causes the guest fault.

### Bounded profiling and resident host-boundary closure

The July 26 dense-lane investigation uses `scripts/memory-guard.zsh` for every
substantial build and run. The guard measures only the launched process tree,
defaults to a 10 GiB aggregate-RSS limit and a 25% free-memory floor, and now
refuses to launch when `ps` or `pgrep` is unavailable. Cargo builds are debug
only and limited to `-j3`; the complete host-binding rebuild peaked at 4.57 GiB.
This replaces the unsafe parallel release-build shape that launched fourteen
`rustc` processes before the machine exhausted memory.

Mechanical overlay discovery was not the remaining runtime bottleneck. Its
enumeration phase fell from 1569 ms to 14--15 ms and the complete receipt from
1780 ms to 201--209 ms, with identical receipts in ten consecutive runs.
Content-stable generated writes and removal of profiling environment variables
from semantic build inputs also reduce unchanged rebuilds to 0.12 seconds.
For runtime diagnosis, `FN64_BLOCK_PROGRESS_ONLY`, `FN64_PROFILE_AOT_PCS`, and
`FN64_PROFILE_RDRAM_RANGES` provide bounded milestone, exact-entry, and small
architectural-memory observations. They are exploratory evidence only. A
four-entry bank-to-artifact cache made the warm 200,000-step run slightly slower
(8.99--9.01 seconds versus 8.85--8.86 seconds) and was removed.

The dense resident pack now discovers public libultra host boundaries by
semantics rather than addresses. The RSP recognizers follow the public
`osSpTaskLoad`, `osSpTaskStartGo`, `osSpTaskYield`, and `osSpTaskYielded` manual
operations and require helper-target relationships as well as task/status/DMA
field behavior. The timer recognizer follows the public `osSetTimer` manual's
o32 argument layout, `OSTimer` fields, and list insertion behavior. Each role
must have exactly one match; relocated synthetic fixtures, invariant mutations,
duplicate candidates, and an environment-gated NWXE corpus test cover the
mechanism. Discovered calls map to the existing typed ABI adapters. Audio tasks
use `LleAccuracy`, not the diagnostic-skip policy.

This changes the observed frontier. Before task binding, 200,000 steps reported
zero admitted SP tasks. After binding, 20,000 steps reported four audio submits,
four SP tasks, four RCP completions, and four RSP/RDP executions. The next zero
counter was SI/controller progress: the hand-written controller initialization
called guest `osSetTimer`, then blocked in `osRecvMesg`; the raw guest timer list
could not arm the typed executor's `TimerWheel`. After semantic `osSetTimer`
binding, a bounded 50,000-step run reached the post-receive continuation once,
performed eight raw SI DMAs, and completed one controller operation. Process-tree
RSS peaked at 126 MiB. Ten subsequent sequential clean 50,000-step runs produced
the same `sim_time=13087281`, eight raw SI DMAs, one controller operation, eight
SP tasks, and identical watched-PC counts/register snapshots. Their guarded peak
aggregate RSS was 129 MiB. This meets the deterministic-fix run-count bar for
the timer-to-controller-init stall; it is not an overlay-entry claim.

The next post-entry profile exposed a VI scheduling defect rather than a need
for a longer horizon. NWXE rewrites `VI_H_SYNC` and `VI_V_SYNC` from its VI
manager on every interrupt; applying those timing values restarted the modeled
beam epoch, so `VI_INTR=2` scheduled another interrupt after only a few
scanlines. Timing writes now change the field cadence without resetting the
running field origin. A focused epoch-preservation regression passes 10/10.
After that correction, the ordinary non-exploratory gate reaches recovered
generation `0x5DEA0D1723E94993` at step 19,523 and
`sim_time=13990253`. Ten consecutive clean runs produced that exact milestone
and process-exit summary; guarded aggregate RSS peaked at 134 MiB. This meets
the deterministic bar for resident-to-recovered-overlay entry. It does not by
itself prove gameplay, rendering parity, or that every recovered overlay
generation is reachable.

A bounded 100,000-step continuation reaches `sim_time=223614515` with 143 VI
interrupts, 130 completed audio tasks, 8,781 PI starts, one controller
operation, four save operations, and no graphics submit. It completes overlay0
and returns to predominantly resident execution; no other recovered generation
is entered. A proposed dense-runner local-transfer fast path reduced retired
host instructions by only 2.8% and did not reduce the 100,000-step CPU time
(`92.74` versus `93.25` seconds), so it and two speculative extra
`opt-level=1` shard overrides were removed. This rules out mechanical overlay
discovery and those codegen changes as useful next actions; the zero graphics
submit remains the current behavior frontier.

Native stack sampling then attributed 33% of all samples, and most of the
late-run cost, to SHA-256 setup for the 16-byte general-exception preamble.
The vector gate had re-hashed those four words on every exception entry. It
now compares the admitted words directly on the matching hot path and retains
the admitted expected digest plus a freshly hashed live digest on mismatch.
This preserves the same loud `AotMiss` evidence while moving hashing off the
ordinary exception path. The same untraced 100,000-step workload fell from
93.25 to 67.09 user CPU seconds (28.1%) with identical `sim_time=223614515`
and identical device/task counters. Ten consecutive clean 50,000-step runs
then reproduced `sim_time=37453026`, the step-19,523 overlay milestone, and
every progress counter exactly; aggregate guarded RSS peaked at 139 MiB.

One adjacent generic codegen defect was corrected independently: an arbitrary-PC
runner's interior `MFC0 Count` now includes instructions retired before the
read, matching the interpreter; whole-function entry reads retain the entry
value. Its focused regression passes, but NWXE's `osGetCount` begins at a runner
boundary, so this change did not cause the controller advance and is not cited
as such.

### Typed exception ownership and bounded profiling (2026-07-27)

A 200,000-step continuation was not waiting on graphics or another mechanical
overlay scan. At step 50,568, resident `DIV.D` at `0x800303e4` raised an enabled
invalid-operation exception (`FCSR 0x01800804 -> 0x01810804`). The four-word
vector reached the retail handler, but its load of the guest running-thread
global returned null and recursively faulted while saving context. That global
was not already modeled. Discovery now derives it without an address signature:
the independently unique public `osGetThreadPri(NULL)` and
`osSetThreadPri(NULL, ...)` bodies must load the same global before accessing
`OSThread.priority`. The scheduler mirrors the selected registered
`OSThread*` at its single resume seam.

The mirror exposed a deeper ownership conflict: the raw handler then attempted
to dispatch from guest thread queues and saved contexts that the host-bound
thread API intentionally does not own, selecting unmapped PC `0x00000400`.
The live block lane now asks `BlockProgram` to return architectural faults to
the live owner. Registered guest threads commit precise CP0 state, optionally
post the public BREAK/FAULT event through the typed executor, and stop; the raw
guest dispatcher is retained for program owners that do not replace libultra's
scheduler. One bounded 60,000-step probe cleared the handler recursion and
completed at `sim_time=387768784`, with 226 audio tasks and zero graphics tasks;
peak process-tree RSS was 139 MiB. This is first-run frontier evidence, not the
required 10-run deterministic validation.

### Canonical scheduler-mirror writer boundary (2026-07-31)

The first one-million-instruction withheld-shard AOT run stopped before its
dynamic comparison at physical RDRAM `[0x00048870, 0x00048874)`. The loud
mutation guard was correct: the dense boot pack makes every aligned resident
word an arbitrary-PC fallback candidate, so the zero-initialized
`__osRunningThread` data word is inside the watched union. The write itself was
not self-modifying code. `run_one_step` mirrors the selected `OSThread *` there
through a raw host pointer immediately before resuming the coroutine, bypassing
the canonical writer journal.

The canonical catalog lane now reconciles the watched image before that write,
skips an unchanged pointer, and otherwise commits the exact four-byte HostAbi
declaration through a move-only child writer transaction before dispatch. It
does not invent a catalog host-call target/resume frame for scheduler state, so
the existing host-call-only completion receipt remains honestly open. A
synthetic validated-bootstrap regression places the global in a watched
non-entry bank and proves the exact declaration, changed-byte runs, quiescent
pending state, and successful dispatch. That deterministic regression passed
10 consecutive clean runs. The package-wide `fn64-abi` nextest run passed 399
tests before the generated completeness-document check detected transient
surface-line drift; the exact document gate then passed after regeneration.
The rebuilt 100,000-instruction whole-shard-withholding comparison cleared that
writer escape.
The AOT lane reached its natural architectural boundary at 100,001 charged
instructions. The dynamic lane then stopped with exactly one instruction left:
the old `InstructionBudget` type rejected every one-instruction slice even when
the next unit was a straight instruction. The core budget now admits one
straight instruction while preserving branch/delay atomicity and returns a
typed error for a genuinely indivisible final pair. Generated AOT, semantic,
and static-micro-op regressions passed 10 consecutive clean runs, and the full
`fn64-recomp-rs` suite passed 370 tests with one skipped. Operational thread
publication v2 also removes only the last-slice charge from equality while
retaining and validating it as diagnostic evidence; cumulative charge and
continuation state remain bound. The rebuilt real pair then reached exactly
100,001 in both lanes. Full logical RDRAM plus device, executor, and ABI-host
digests matched. Boot shards 1 and 2 were not entered within that horizon, so
neither short run exercised dynamic translation and both correctly failed the
withheld-entry requirement. Those results remain useful partial evidence for
RDRAM and owner-state agreement, but are not dynamic-execution parity evidence.
CPU and continuation publication digests also
differed despite the matching owner state. `RecompContext` keeps Count at
dispatch-entry time while the executor owns and charges elapsed time after the
coroutine publishes its checkpoint, making slice-dependent timing fields one
suspected observation artifact; the exact differing fields remain to be
diagnosed.

Whole-shard withholding is now replaced by one operational exact-key redirect.
Both lanes retain the complete static catalog and identical canonical program
and resolver-install identities. With
`FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY=1`, the dynamic lane requires the
selected `ExecutionKey` to equal the installed entry and redirects it once at
the unified dispatcher; it does not remove a shard or alter static authority.
The guard clears only after positive dynamic work, after which normal static
budgets and executable-mutation reconciliation resume. Telemetry schema
`fn64.wm2000.dynamic-withheld-telemetry.v2` proves the individual attempt with
its bank/PC, positive `charged_instructions`, and zero `unsupported_exits`;
aggregate dynamic charge is not proof of that attempt. The program-identity
line and comparator additionally bind `resolver_install_sha256`. Publication
digest v2 removes dispatch-slice partition noise without dropping hardware
authority: it omits the context's stale Count/Compare and host-driven Cause
mirrors only because the separately required executor/device/ABI digests own
those values, and it rejects any pending Count/Compare write.

The attempt-17 v4 pair was seeded from the completed attempt-16 Cargo cache.
Its AOT build completed in 33 seconds at 755 MiB peak tree RSS, and its dynamic
build completed in 32 seconds at 751 MiB. The receipt is retained privately at
`/private/tmp/fn64-wm-exact-entry-pair-20260731-17/receipt.json`.

One real-ROM 100,000-instruction diagnostic then reached 100,001 charged
instructions in both lanes. The exact withheld key
`81bf2e27273b27db:80000400` ran dynamically once, charged one instruction, and
reported zero unsupported exits. Full logical RDRAM, CPU, device, executor,
ABI-host, and simulation time matched. Continuation digests differed, and the
AOT lane took 33,333 scheduler steps while the dynamic lane took 25, so the v2
comparison failed. Its private evidence is
`/private/tmp/fn64-wm-exact-entry-diff-20260731-17/comparison.json`,
`dynamic-telemetry.json`, `aot.log`, and `dynamic.log`. This is one diagnostic,
not a fixed or parity claim. The exact next step is to expose and diff the
pending exit and prepared continuation at publication.

Profiling gained `FN64_PROFILE_STOP_AT_PC` self-enablement (it no longer needs a
second AOT profiling flag), FCSR in watched-PC snapshots, and
`FN64_PROFILE_EXCEPTIONS=1` for exception type/current-thread ownership. Watched
PC snapshots also include raw `d0`/`d18`; the faulting `DIV.D` sees both as
zero, proving the immediate operation is `0.0 / 0.0` rather than an FPU flag
classification error. The upstream zero-vector provenance remains open.
`FN64_PHASE_TIMING=1` now prints executor/graphics/audio wall-time totals in
this harness. Progress-only probes disable the unbounded executor trace unless
`FN64_BLOCK_EXECUTOR_TRACE=1` is requested; task counts remain available from
always-on counters, so the progress summary no longer clones that full trace.

The zero-vector provenance is now closed. The only call before the fault came
from `0x8000d6f0` with a non-degenerate look-at tuple: eye `(0,400,500)`, target
`(0,150,0)`, and up `(0,1,0)`. Its reciprocal constant was present in RDRAM as
the double `1.0`, but the created thread had inherited reset-thread
`Status.FR=1`; the function's FR=0 `MTC1` even/odd construction therefore left
the double operand in `f18` equal to zero. N64Recomp's generated NWXE
`osCreateThread` body initializes the saved SR to `0x0000ff03` (FR clear), so
the typed created-thread context now clears FR while retaining its other
modeled Status fields. A focused regression passes, and one bounded 60,000-step
probe reached the old `0x800303e4` site twice with divisor `1.0`, no exception,
and peak RSS 149 MiB. A 65,000-step continuation then produced 14 graphics
submits, 56 audio submits, and no render error at peak RSS 211 MiB. Its 49.584 s
executor time initially reported only 1.865 ms of graphics dispatch because the
phase counter covered HLE preflight but not the subsequent recognized LLE/raw-DPC
path. Native sampling, rather than that incomplete counter, located the actual
renderer cost described below.
The former fault boundary then passed ten consecutive clean guarded runs. Every
run stopped deterministically at step 50,568 / simulation time 39,272,554,
entered `0x800303e4` twice with Status `0x30000000`, divisor `1.0`, unchanged
FCSR `0x01800804`, and no render error; the ten-run process-tree peak was
137 MiB. This satisfies the deterministic-fix run-count bar for the FR
transition. The separate 65,000-step graphics continuation remains a one-run
frontier observation at this point; the later renderer section records its
ten-run validation.

### Post-submit renderer profile and bounded optimization (2026-07-27)

An eight-second native sample inside the post-overlay window attributed 5,058
of 5,904 main-thread samples to the recognized graphics-LLE/raw-DPC path. Of
those, 3,681 were in the full framebuffer commit and 1,170 in RGBA16 hidden-bit
writes. The hidden coverage sidecar used a hash table keyed by physical
halfword even though the domain is exactly the fixed 8 MiB RDRAM allocation.
It now uses lazily allocated dense packed storage: one `u32` per physical
halfword, with `u32::MAX` as the untouched sentinel. First use costs a bounded
16 MiB; lookup and update are direct bit-test/shift indexing, and an untouched
backend still allocates nothing.

The same 65,000-step workload fell from 49.584 s to 29.000 s after that storage
change, with identical step count, simulation time, device/task counters, and
render-error state. The standalone harness now compiles only the handwritten
`fn64-render-reference` crate at `opt-level=2`; generated AOT shards keep their
existing low-memory profiles. That targeted build rebuilt the renderer and
harness in 3.97 s, peaked at 387 MiB, and did not recompile any shard. The final
workload measured 16.592 s executor time, including 3.607 s across fourteen
graphics-LLE tasks, with the same observable counters and a 216 MiB guarded
runtime peak. This is a 66.5% executor-time reduction from the measured
baseline. `FN64_PHASE_TIMING=1` now reports aggregate graphics phases separately
from recognized LLE task time, so an HLE preflight followed by LLE is no longer
mislabelled as two graphics tasks.

Mechanical discovery was rechecked independently after these runtime changes:
ten consecutive `profile_overlay_regions` runs produced one identical stable
receipt. Enumeration took 13--14 ms and the complete
normalize/enumerate/admit/recipe pipeline took 199--225 ms (200 ms median).
Mechanical ROM discovery is therefore not the remaining long-run bottleneck.

Ten consecutive final guarded 65,000-step runs then reproduced the exact
overlay milestone at step 19,523 / simulation time 13,990,253 and completed at
`sim_time=98511883`. Every run reported 14 graphics submits, 56 audio submits,
the same device/RSP/RDP/controller/save counters, the same two entered recovered
generations, and `render_error=None`. Peak aggregate RSS was 219 MiB. This meets
the deterministic run-count bar for the dense-sidecar/runtime-profile candidate;
it does not establish framebuffer parity against an external runtime.

The faster lane extends cleanly to 100,000 steps / simulation time 598,985,169:
175 graphics tasks, 351 audio tasks, 177 controller operations, four save
operations, the same two recovered generations, and no render error. Executor
time was 69.474 s, of which recognized graphics LLE consumed 43.677 s; guarded
RSS peaked at 226 MiB. A post-optimization native sample attributed 2,482 of
3,374 renderer-bound samples to RGBA16 framebuffer commit, primarily the
canonical checked halfword writer rather than hidden-sidecar hashing. A typed
bulk-halfword experiment matched individual writes in a focused test and kept
all 461 renderer tests/snapshots green, but its allocation and second pass
regressed the identical 100,000-step workload to 77.371 s and graphics to
51.137 s. It was removed completely. The next performance frontier is reducing
full-frame commit work without adding a second pass or weakening the canonical
RDRAM lane mapping; this is not a reason to reopen mechanical ROM discovery.

The allocation-free continuation of that profile is now implemented. RGBA16
commit packs two adjacent logical pixels into one canonical native-word write
and updates the dense hidden-bit sidecar as the same pair; it creates no
temporary buffer and performs no second framebuffer pass. On the identical
controller-scheduled 100,000-step route, raw-RDP time fell from 11.065 s to
6.791 s (38.6%) and total executor time from 30.385 s to 26.144 s (14.0%),
with identical virtual time, device/task counters, render-error state, and
entered generation list. A ten-second native sample had identified the old
per-halfword RGBA16 commit as the largest avoidable renderer cost. The split
phase timer now reports RSP execution and raw-RDP processing separately; on
this route they consumed 0.148 s and 6.791 s respectively, ruling out the RSP
interpreter as the next performance target.

The next profile found that the reference backend's atomic `process_task`
implemented atomicity by repeatedly calling its public resumable one-operation
path. After the first draw made an image dirty, every following operation
therefore rewrote the complete active color/depth image even though no guest
could interleave. Atomic dispatch now executes the same ordered operations in
one call and commits only at existing semantic barriers (target changes and
FullSync) plus task completion; the public chunk/token path and its per-op
commit contract are unchanged. The existing atomic-vs-chunked test proved
identical final RDRAM, framebuffer pixels, and FullSync evidence, and the full
renderer suite passed 455 unit plus six replay/snapshot tests.

On the identical 100,000-step scheduled route, raw-RDP time fell from 6.791 s
to 2.453 s (63.9%) and total executor time from 26.144 s to 22.027 s (15.7%).
Step/time, all device/task counters, render-error state, and the three entered
generations were identical. `scripts/wm2000-route-series.zsh` then completed
10/10 guarded runs with byte-identical extracted evidence; peak process-tree
RSS was 222--223 MiB. Full logs are retained out of tree at
`/private/tmp/fn64-wm-route-series.dXIqO2`.

The standalone profile also compiles the handwritten `fn64-abi` crate at
`opt-level=2`. That package-scoped change reduced the same route from 37.717 s
to 30.560 s before the packed commit landed, with unchanged guest evidence;
generated shard codegen remains narrowly scoped as above. Progress-only runs
now discard both executor and device diagnostic vectors by default while
constant-space device counters remain active. The full reference-renderer
suite passed 455 unit plus six replay/snapshot integration tests. Ten
consecutive final guarded 65,000-step runs reproduced the exact step 19,523
overlay milestone, completion virtual time, all counters, and two-generation
list; peak process-tree RSS was 214--217 MiB. The scheduled 100,000-step route
also reached the mechanically recovered third generation, whose image covers
`0x8011c900..0x8016e650`. A scheduled 200,000-step route remained pure static
AOT and clean but did not enter the fourth catalog generation. Runtime entry
coverage is not a static-closure requirement: the generated pack already
contains every aligned word of all four mechanically recovered images, and a
generation is recorded as entered only when one of its runners executes. The
route result therefore establishes three-generation execution coverage, not a
missing-code frontier.
Three bounded vertical-menu probes then moved down one, three, and six rows
after START before confirming. Each produced distinct timing/controller/task
counters, proving the scripted inputs reached different live paths, but all
three remained within the first two recovered generations at 100,000 steps.
The earlier alternating START/A route remains the only tested path that enters
the third generation. Simple main-menu vertical coverage therefore does not
reach the fourth; the next sweep must vary a later-stage choice (for example
left/right or character selection) rather than extend these same routes.

For that sweep, `FN64_RENDER_DUMP_DIR` enables out-of-tree reference-renderer
PNG output, `FN64_RENDER_DUMP_FIRST_TASK` skips earlier graphics tasks, and
`FN64_RENDER_DUMP_LIMIT` bounds the number of non-clear frames written. These
are opt-in diagnostics only; ordinary progress and release runs perform no
image I/O. The renderer reports the first omitted frame after that bound and
then suppresses identical limit notices.

The 200,000-step endpoint was one input edge too early to characterize the
next screen: it stopped at controller read 469. A guarded 240,000-step run of
the same alternating schedule crossed A presses at reads 510 and 570, reached
638 graphics tasks and 591 controller operations, and remained pure static
AOT in the same three generations. Peak process-tree RSS was 342 MiB with 85%
system memory free. Bounded frames proved that the late route had entered
Exhibition / Single Match setup; the changing triangle workload after read
510 showed a further screen transition, but not the fourth generation.

`scripts/wm2000-route-probe.zsh` is the short feedback-loop entry point. It
requires the user's `ROM` and a `FN64_BOOT_CONTEXT`, reuses the existing
harness binary, enables constant-space progress mode, continues after early
overlays, and always runs under `scripts/memory-guard.zsh`. Its optional third
argument becomes `FN64_PROFILE_STOP_AT_GENERATION`, which stops immediately
after the requested recovered generation is first entered. The wrapper clears
all four opt-in unbounded trace-history variables and defaults its process-tree
RSS ceiling to 2 GiB; callers can deliberately set a different
`FN64_GUARD_MAX_RSS_MIB`. The guard escalates from TERM to KILL after a bounded
two-second grace period for the exact captured process tree. A configured stop
generation that is absent at the bounded exit is a loud failure rather than a
successful identical miss. Controller input
edges now include scheduler step, simulation time, graphics/audio task counts,
and the entered-generation set, so a schedule can be correlated without a
second trace. Example (the generation ID is decimal or `0x`-prefixed):

```sh
ROM=/path/to/wm2000.z64 \
FN64_BOOT_CONTEXT=/path/to/boot-context.json \
scripts/wm2000-route-probe.zsh /tmp/route.schedule 240000 3068194456377681093
```

`scripts/wm2000-route-series.zsh SCHEDULE MAX_STEPS RUNS [STOP_GENERATION]`
runs that same guarded probe strictly sequentially, retains full stdout/stderr
logs under a fresh `/private/tmp` directory, and requires byte-identical schedule,
input-edge, generation, completion, counter, and entered-generation evidence.
It is the default helper for the ten-run deterministic validation bar; it does
not compare host wall time or memory-pressure telemetry.

`scripts/wm2000-scenario-gate.zsh SCHEDULE MAX_STEPS [RUNS]
[REQUIRED_GENERATIONS]` is the authoritative bounded-scenario wrapper. It
first rejects a feature graph containing the development interpreter, then
runs the existing dense-AOT binary sequentially under the memory guard. Every
run must report the linked library's production-AOT feature receipt, the same
program and controller-schedule identities, at least one consumed standard-
controller read, input edge, graphics submission, completed RCP task,
recognized microcode, committed DRAM/XBUS DPC stream, executor timing
checkpoint, no render error, and every declared overlay generation. It
rejects typed static-dispatch and unsupported outcomes, then requires
byte-identical semantic evidence. The policy receipt binds the actual binary,
ROM, BootContext, schedule content, run count, step bound, required generation
list, and milestone thresholds by digest or value, without recording private
paths. Authoritative use requires 10--100 runs; ten is the default. Minimum
counters can be raised with the `FN64_SCENARIO_MIN_*` variables; audio defaults
to a recorded-but-zero-allowed checkpoint until a route declares
`FN64_SCENARIO_MIN_AUDIO=1` or higher. This proves the declared exercised
scenario only; it does not replace the separate build-owned executable-image
catalog exhaustiveness receipt required for a 100% static-recompilation claim.
`scripts/test-wm2000-scenario-gate.zsh` is its ROM-free, sub-second parser
feedback loop.

The first fresh production proof with this gate used the digest-bound NWXE
ROM, retained BootContext and controller schedule, and the newly linked
111 MiB binary. Its exact binary digest is retained with the out-of-tree run
evidence rather than asserted as a live repository invariant.
One 100,000-step smoke run entered required generations
`6767235783115491731`, `3179581202434458265`, and
`5416062183125883563`; completed 138 standard controller reads, 119 graphics
submissions, 262 audio submissions, and 500 RCP tasks; recognized 357
microcodes; and committed 119 XBUS DPC streams with no render error. The
authoritative wrapper then passed 10/10 runs with byte-identical evidence.
Logs are retained out of tree at `/private/tmp/fn64-wm-scenario-gate.EQslgV`.
This closes the declared scenario proof, not the separate exhaustive source-
catalog frontier.

The hook was validated against already-known generation
`6767235783115491731`: a nominal 100,000-step probe stopped at its exact first
entry, step 19,523 / simulation time 13,990,253, in 2.96 s with 91 MiB peak
RSS. After hardening the wrapper, ten consecutive probes reproduced identical
extracted evidence and the same 91 MiB peak; logs are retained at
`/private/tmp/fn64-wm-route-series.ZDwPcz`. A one-step probe requesting the
unreached fourth generation exited nonzero with its target and bounded process
exit in the panic, proving a miss cannot be reported as a successful series.
A follow-up 320,000-step route replaced the post-read-510 START/A
alternation with spaced A confirmations through read 782. It reached 1,161
graphics tasks and 800 controller operations, remained pure static in the
same three generations, and peaked at 955 MiB under a 2 GiB guard. That rules
out simple repeated confirmation as an observed fourth-generation route. A
different menu branch is relevant only to runtime reachability coverage, not
to whether the already linked fourth generation is statically recompiled.

The later-stage branch was then resolved visually rather than guessed. A
downward mode choice produced `STONE COLD VS STONE COLD`, proving that the CPU
opponent path was active. On the rules page, A opened the highlighted Time
Limit chooser and START closed it. The observed navigation graph then gave the
confirm path without importing an external implementation claim: D-UP moved
from Decision to the bottom Options control, A opened the global
settings page, START returned, D-DOWN wrapped back to Decision, and A produced
the `Single Match / STEVE AUSTIN VS STEVE AUSTIN` presentation. This removes
the earlier ambiguity between a two-player roster screen and match setup.

It does not enter the fourth catalog generation. A 420,000-step guarded run
pressed START twice during the versus/entrance presentation, completed 1,707
graphics submits and 1,162 controller operations with `render_error=None`, and
remained in the same three recovered generations. The selector gate word at
`0x8003dd0c` remained `00000000`; peak process-tree RSS was 1,250 MiB under the
2 GiB route cap. Therefore the catalogued generation
`3068194456377681093` is not yet proven to be the ordinary single-match bank,
and no retained route recipe reaches it.

A follow-up run used the existing constant-space AOT-PC profiler on that exact
420,000-step schedule under a tighter 1 GiB process-tree cap. It watched the
fourth descriptor's selector continuation (`0x80022484`), selected path
(`0x80022498`), loader return (`0x800224d4`), and image return (`0x80022510`).
All four counts were zero. The bounded exit still contained gate word zero and
the complete descriptor bytes at `0x80047eec..0x80047f10`, including ROM
`0x000d2720..0x00144aa0`, load address `0x800e1b90`, data end
`0x80153f10`, and invalidation/BSS end `0x8016f170`. The run peaked at 698 MiB.
Thus the single-match route never executed this loader-loop path; gate value
zero at the endpoint was not evidence that it selected or rejected the fourth
image. Extending the same route cannot answer fourth-generation reachability.

The closure terms are now kept separate. The fourth generation is
**catalogued** by mechanical ROM discovery and **compiled** into eight linked
dense-AOT shards. It was not **selected/materialized** by this observed loader
path and was not **entered/executed** by any retained route. Lack of runtime
entry does not subtract from the pack's byte/entry ownership, just as executing
three generations does not prove unreachable code. The remaining 100% claim
is a static evidence audit: prove that the recovered resident-plus-four-image
catalog is exhaustive for required CPU code and that the production artifact
has no interpreter or missing-AOT path. It is not an input-route sweep.

That audit found one production feature leak: the standalone harness selected
`aot-runtime` without default features, but each generated shard's normal and
build dependency accepted `fn64-recomp-rs` defaults. Cargo feature unification
therefore enabled `dev-interpreter` in the final graph even though every linked
shard had a generated runner. All 34 shard manifests in that measured build selected only
`aot-runtime`. The final host selects the stronger `production-aot` feature,
which is compile-time incompatible with `dev-interpreter`; its explicit
resolver-2 workspace keeps host/build-tool features out of the linked target
graph. `scripts/check-wm2000-pure-aot.zsh` is the permanent fast gate: it
resolves normal linked feature edges, requires both `production-aot` and
`aot-runtime`, and fails if the development interpreter reappears.

The corrected graph rebuilt successfully with `cargo build -j1` under the
process-tree memory guard. The build receipt reports 16 resident and 18 overlay
dense-AOT shards; peak aggregate RSS was 3,194 MiB with at least 74% system
memory free. The resolved feature gate passes, the linked binary contains no
`fn64-recomp-rs:dev-interpreter:artifact` marker, and the no-default-features
`production_aot` test proves that admitted code without a generated entry fails
closed as `MissingAotEntry`. A short guarded smoke then reached generation
`6767235783115491731` at the unchanged step 19,523 / simulation time
13,990,253 with 116 MiB peak RSS. This verifies pure-AOT artifact composition
and the known entry path; it does not prove catalog exhaustiveness.

The honest remaining static-closure frontier is catalog exhaustiveness. The
pack proves complete aligned-entry ownership for the mechanically recovered
one-MiB resident image and all four records in the uniquely admitted overlay
descriptor table. It does not yet prove that no other mechanism can produce or
load required CPU code outside those five images. Runtime route coverage cannot
prove that negative, and the generic discovery scoreboard still contains
unsupported destinations. A 100% recompilation claim therefore remains not
verified until a build gate binds exhaustive executable-image ownership to the
pure-AOT feature/entry receipt.

The linked-catalog seam now fails earlier: installation checks every generation
shard against the live `BlockProgram`, requiring the bank to exist and its one
contiguous span to equal the catalog range. The WM host also exact-matches each
flattened overlay artifact's bank and range against the generated generation
table before registration. This closes catalog/program drift, but it is not an
exhaustiveness proof. Exception ownership is a concrete remaining gap. The
current CPU model has an exact six-address denominator: `0x80000000`,
`0x80000080`, and `0x80000180` with BEV clear, plus `0xbfc00200`, `0xbfc00280`,
and `0xbfc00380` with BEV set. `0x80000100` is not a modeled exception
destination. Only the independently captured `0x80000180` general vector is
admitted. An fn64-only fault route has already requested `0x80000000`; the
reference traces did not, so its live bytes cannot be promoted from that route.
The frontier receipt now requires every modeled address to carry either one
exact image owner, a validated machine-checkable unreachability receipt, or an
explicit open disposition. The unreachability form fails closed until its
state-proof validator exists. A bounded CFG data-flow pass can recover exact
ROM-word stores into fixed vector addresses, but labels them conditional on the
source word remaining unchanged until its load. These gaps therefore still
require allowed black-box captures or a mechanical writer/state proof.
Generated disassembly is not an authority for filling them.

The manifest-only frontier now applies that bounded store pass to the resident
generation and each of the four recovered overlay generations. It requires an
exact one-to-one match between every dense generation and its proven physical
ROM mapping, seeds only proven function entries (plus the hardware entrypoint
in the resident range), and bank-qualifies every result with the dense bank ID.
Per-generation scan summaries expose root/block/finding counts. A recovered
store remains candidate provenance only: it neither proves that its source was
stable nor that runtime control executed the store, and it does not promote an
exception vector to an exact code owner.

Each dense generation now also records every aligned word that decodes as a
direct COP0 Status write (`MTC0` or `DMTC0` to register 12), partitioned by CFG
classification into proven code, proven data, and unclassified words. Open
indirect sites remain attached to the same scan. This closes the cheap static
instruction inventory but does not yet prove `Status.BEV == 0`: the captured
BootContext, `__osSetSR`/legacy host bridges, and saved thread contexts can all
replace Status outside that instruction subset. The receipt therefore keeps
all three `0xbfc0...` destinations open until a ROM-bound runtime-state/effect
receipt validates those authorities in process.

The source-frontier receipt now carries that initial runtime-state authority.
If `FN64_BOOT_CONTEXT` is absent it records an explicit missing authority; if
present, the gate parses the canonical context and checks its normalized-ROM
digest, header entry, NTSC selection, destination code, and IPL3 digest against
the normalized discovery image before retaining the exact CP0 Status value.
Malformed or mismatched contexts fail loudly. This makes the initial BEV bit
auditable without treating it as proof for later `MTC0`, host, or child-thread
effects.

Captured external executable generations are no longer outside this Status
denominator. Each reproducible capture is scanned from its retained
architectural words with the same raw decoder and bounded CFG machinery, and
the receipt requires a scan matching its exact image identity, generation,
range, digest, and first fetch. Any unclassified Status-shaped word remains an
open frontier rather than being hidden by the capture's external provenance.
Exception-vector ownership is now entry-specific: only the capture's
reproducible first fetch may receive `ExactCodeOwner`; other vector addresses
that merely fall inside the same byte range remain open.

Each proven-code `MTC0 Status` now also carries a one-to-one value proof from
the existing whole-CFG abstract interpreter. Constants and finite joins can
prove BEV clear, while known-zero/known-one propagation retains bit invariants
even when the complete value is unknown. `MFC0 Status` seeds BEV as known zero,
which closes the resident read/modify/write site at `0x8002a26c`: its mask
preserves BEV clear without claiming a complete Status value. Loads from
mutable image memory reset both masks, so their initial ROM bytes cannot become
runtime facts; subsequent bit operations must establish any retained mask.
Unknown/widened values and mutable-memory provenance remain explicit blockers,
while `DMTC0 Status` stays unsupported rather than being truncated to the
interpreter's 32-bit domain. The current canonical receipt therefore has four,
not five, open proven-code Status value proofs.

The canonical validator can now mark the three bootstrap vectors unreachable
through an inductive BEV-clear disposition, but only in process: initial
BootContext Status must be clear; every dense and external Status scan/value
proof must close; the exact 15 installed host symbols and effects must match;
all normal-vector handlers must have scanned owners; and executable writer,
DMA, and transfer closure must be complete. Forged/incomplete host catalogs,
open normal handlers, and opaque proof digests are rejected. The present WM
inputs do not meet that bar, so this mechanism does not yet make the real
`0xbfc0...` frontier closed.

The WM production boot build, every generated shard build, and the manifest
gate now obtain their host targets from the same exact 15-binding catalog.
`__osSiDeviceBusy` is included through its public SI status-register behavior;
its signature is no longer privately duplicated in the shard emitter or
omitted from the source receipt. The common C adapter now compares
Status.BEV before and after every admitted shim and traps before copy-back on a
transition, so the receipt marks every installed adapter's current-context
effect as runtime-enforced BEV preservation. `osCreateThread` also records its
child-Status transition as caller inheritance with FR cleared. That remaining
inheritance edge turns a previously implicit runtime authority into a
canonical, typed blocker.

The independently compiled dense shards export full source and emitted-
runner SHA-256 values. The boot build independently derives each expected
source identity from normalized-ROM identity, generation/ROM/load geometry,
and exact bytes, then rejects any linked mismatch before registration. It also
hashes the actual linked `CodeBank` words and compares the full digest. Runner
identity composes the installed dispatch source, complete generated pack,
generated runner, bank, and typed adapter role; captured external-image runners
use the same rule. The resulting live program must produce a canonical
`BlockProgram` evidence snapshot before thread-zero boot, so a truncated bank
ID is no longer mistaken for native artifact proof.

The canonical generation migration makes the resident/overlay ownership
boundary explicit. The permanently resident IPL prefix is now 15 static shards
covering `[0x80000400,0x800e1b90)`. The overlapping IPL tail is a two-shard
precompiled generation with image `[0x800e1b90,0x80100400)`, invalidation union
`[0x800e1b90,0x80171a60)`, an exact ROM-byte digest, and a domain-separated ID
binding the normalized ROM plus that geometry. Together with the 18 overlay
artifacts this makes 35 generated shard packages. Treating the tail as static
would overlap overlay ownership; dropping it would lose executable resident
code before the first overlay load.

The WM host now consumes that catalog through `CatalogGenerationInstallV1`:
an exact 15-target host-function catalog, direct-KSEG physical backing for the
resident tail and every overlay generation, and no ambient entry, transfer, or
host callback. A typed IPL3 publication seeds an owned 8 MiB allocation; commit
binds ROM, resolver, generation definition, entry image, and watched bytes,
then boot moves the allocation into `HostState` and records the publication as
mutation-journal sequence zero. The generated-C 0x29000000-byte sparse MMIO
mirror is not allocated in this typed block lane. A no-ROM guarded Cargo check
compiled the complete 35-package build-script graph and stopped at the named
`ROM` input requirement; real pack generation, root compilation, and route
execution remain unverified until the private ROM, BootContext, and executable-
image capture group are supplied.

The fast feedback command for this frontier is now
`scripts/wm2000-static-frontier.zsh`. It composes the production feature gate
with the ROM-only dense-manifest/source-receipt path under a 2 GiB guard. The
receipt is canonical and digest-bound, retains exact raw-PI register-site PCs
rather than an aggregate count, and lists every presently open writer class.
It deliberately avoids runner emission, shard compilation, linking, and route
execution; a production rebuild is reserved for changes that survive this
cheap structural loop.

The source-receipt path now reuses its dense and external CFG closures to scan
transfers in the same process. Direct guest/host edges and every indirect
exhaustive/bounded/open state replace the former all-zero placeholder summary;
the scanner also retains ambiguous/outside-owner, `$ra` return, decoder/trap,
malformed delay-slot, run-off-end, and reached-data-fence blockers. It checks
the CFG's direct, tail, unresolved-indirect, and resolved-indirect inventories
against the actual block terminators, including call continuations and exact
resolved target sets. This adds no second CFG pass. Current WM evidence remains
explicitly open because proven fact roots do not enumerate every callable
entry and return provenance has not been closed; a caller assertion cannot
promote that bounded scan to exhaustive evidence. The receipt consumes the
opaque analyzer result and retains the full scan rather than only its summary,
so another producer cannot manufacture a complete inventory through public
Rust fields or direct receipt deserialization.

The 30-read neutral gaps are also deliberately conservative: retained edges
show roughly 30 graphics submissions between adjacent inputs. A safe speed
experiment preserves the proven prefix through read 510, halves only the late
suffix to 15-read gaps while retaining two-read pulses, and checks the same UI
states and generation prefix. No compressed schedule is authoritative until a
bounded visual run establishes semantic equivalence and the required series
establishes deterministic replay.
