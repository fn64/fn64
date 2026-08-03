# wm2000-boot

Headless boot harness for WM2000 (NWXE) on fn64. Contains **zero game
content** — see `build.rs`'s module doc for the exact env-var contract.
Standalone workspace (deliberately not a member of the main `fn64`
workspace), since it requires out-of-tree, game-derived build inputs the
main workspace must never depend on.

## Running

```
RECOMPILED_DIR=/path/to/aki-recomp/games/NWXE/RecompiledFuncs \
RECOMP_H_DIR=/path/to/aki-recomp/refs/WCWnWoRevengeRecomp/lib/N64ModernRuntime/N64Recomp/include \
ROM=/path/to/your/own/wm2000.z64 \
cargo run
```

- `RECOMPILED_DIR` — a directory containing N64Recomp-generated
  `RecompiledFuncs/*.c` + `recomp_overlays.inl` for WM2000 (NWXE).
- `RECOMP_H_DIR` — N64Recomp's own MIT-licensed `recomp.h` include directory.
- `ROM` — your own legally-obtained WM2000 (NWXE) ROM file. Never copied
  anywhere; read once at startup.

The default renderer is the deterministic software reference backend. To run
the same non-window-driving loop through the native RT64 adapter:

```sh
FN64_RT64_DIR=/path/to/pinned/rt64 \
FN64_RENDER=rt64 \
RECOMPILED_DIR=/path/to/RecompiledFuncs \
RECOMP_H_DIR=/path/to/N64Recomp/include \
ROM=/path/to/your/own/wm2000.z64 \
cargo run --release --features rt64
```

RT64 owns a hidden SDL/GPU surface; the harness does not create a window or
drive an event loop. On macOS the process must have WindowServer access and a
default Metal device, and `Rt64Backend::create` must run on the main thread.
This binary calls it directly from `main`. An explicit RT64 request fails
loudly if the feature, display, or GPU is unavailable; it never silently runs
the reference backend instead.

`WM2000_GRAPHICS_POLICY=hle` uses the content-addressed HLE path with
transactional LLE fallback. `WM2000_GRAPHICS_POLICY=lle` always executes the
loaded graphics microcode through fn64's RSP interpreter and sends only its
raw DPC stream to the selected renderer. The reference lane defaults to HLE
to preserve its existing diagnostic behavior; RT64 defaults to LLE so the
native run also exercises fn64's runtime/RSP path. Either default can be
overridden explicitly. No WM2000 microcode digest is currently admitted by
either backend's HLE catalog, so `hle` is presently an attempt followed by
the transactional LLE fallback, not a claim that WM2000 executed in HLE.

## What it does

1. Loads the ROM, registers every section from the real
   `recomp_overlays.inl` via `fn64-boot-harness`'s shared C-side walk into
   Rust, then marks the always-resident sections loaded.
2. Boots thread 0 running the real `recomp_entrypoint` symbol and drives
   the executor: `run_one_step` while runnable, `advance_virtual_time`
   to the exact earliest device-fabric deadline when idle (or catches the
   fabric up through current executor time when that deadline is already
   overdue), up to a bounded step budget. Earlier DMA/RCP completions are
   serviced without being mistaken for fields; RT64 capture is attempted only
   when the fabric reports one or more VI retraces actually committed. The
   harness logs periodically so a genuine stall/spin is visible rather than
   silently indistinguishable from real progress.
3. At each committed presentation after one or more `osViSwapBuffer` calls,
   the reference lane inspects the latest guest-RDRAM framebuffer if
   non-uniform (`/tmp/fn64-fb-<n>.png`). Multiple swap requests before one
   retrace correctly collapse to the buffer actually presented by that field.
   The RT64 lane instead reads the backend's fenced post-VI BGRA8 target,
   prints its exact SHA-256/workload/present identity, and writes
   `/tmp/fn64-rt64-post-vi-swap-<n>-present-<id>.png`. It never labels the
   guest framebuffer address as native RT64 output.
4. Writes the full `TraceEvent` stream to `/tmp/wm2000-boot-trace.jsonl`.
5. Seals the terminal executor lifecycle after every assertion and trace write.
   Started guest coroutines still blocked in generated C are detached without
   an invalid foreign-frame unwind; renderer/audio owners and ordinary Rust
   values then receive normal process teardown.

The harness registers its complete RDRAM allocation at the AI output boundary
and reports buffer/sample/nonzero/range counts at exit. It opens the default
cpal device when available; set `FN64_NO_AUDIO=1` for a headless run. Set
`FN64_DUMP_AUDIO_STREAM_PCM=/private/path/stream.pcm` to retain private
pre-resample stereo `s16le` for a bounded audio validation.
The boot loop normally runs faster than wall time and may deliberately skip
old samples at cpal's bounded latency cap. Set `FN64_AUDIO_PACE=1` for an
audible diagnostic: the harness holds the host-only queue near 125 ms and
drains it before exit without changing guest virtual time. Release evidence
rejects this wall-clock mode. For audio-only diagnosis, additionally set
`FN64_AUDIO_VALIDATION_SKIP_GRAPHICS=1`; this selects the ABI's explicit
`DiagnosticSkip` graphics policy while retaining live-IMEM LLE audio. It is
mutually exclusive with `WM2000_GRAPHICS_POLICY` and rejected in release mode.

`WM2000_NO_DUMP=1` disables PNG output and normally skips RT64 readback setup.
`WM2000_NO_TRACE=1` also disables PNGs for throughput measurements. Add
`WM2000_RT64_CAPTURE=1` to either configuration to retain the fenced post-VI
readback and exact digest without paying PNG-encoding or file-I/O cost.
When RT64 readback is enabled, `WM2000_STOP_AT_SWAP=<n>` waits for the first
committed post-VI capture at or after swap `<n>` before exiting; it does not
mistake the guest's swap request for a completed native presentation. A step
budget or steady-idle exit before that capture is a loud failure, not a
successful partial capture.

Ordinary runs remain diagnostics: they retain the explicitly documented
voice-map virgin-memory reproduction in `src/main.rs`. A diagnostic digest
proves which native pixels were produced and whether a bounded run repeats;
it does not by itself prove hardware pixel correctness or zero unsupported
behavior.

## Voice-map-intervention-free schema-v29 release path

The generic runner-owned release tuple enables the fail-closed RT64/LLE path:

```text
FN64_RELEASE_GATE_CYCLE=C
FN64_RELEASE_REPORT=/private/path/report.json
FN64_RELEASE_ROM_CLASS=retail_cartridge
FN64_RELEASE_RUN_EVENT_SHA256=LOWERCASE_SHA256_FROM_THE_RUNNER_EVENT
FN64_RELEASE_MICROCODE_TEXT_PATH=/private/staged/imem.bin
FN64_RELEASE_MICROCODE_DATA_PATH=/private/staged/ucode-data.bin
FN64_RENDER=rt64
WM2000_GRAPHICS_POLICY=lle
```

The four `FN64_RELEASE_{GATE_CYCLE,REPORT,ROM_CLASS,RUN_EVENT_SHA256}`
variables are one indivisible tuple; partial tuples and the historical
`OOT_RELEASE_*` aliases fail. The private-series runner owns the event digest,
staged microcode pair, report path, and ROM-class authority. The target cycle
must be an exact scheduled VI edge with a completed RT64 post-VI presentation.
The host arms `LiveReleaseGate` and its crash-flushed unsupported-event journal
before thread 0 runs, freezes the native archive identity and all fixed-cycle
channels at that edge, writes the schema-v29 report, completes the unsupported
journal, and then requires closed-gate state. A failed closure may retain an
incomplete report for diagnosis, but the private-series runner rejects it.
Stopping early or skipping the requested cycle is a loud failure.

Release mode forces RT64 post-VI capture, requires a clean authoritative RT64
source identity, and requires LLE graphics execution. Most importantly, it
never writes the diagnostic voice-map zeros. If a fresh channel allocation
reaches the point where that reproduction would have fired, the process traps
before report creation and leaves the unsupported journal incomplete. An
accepted closed report therefore certifies that this voice-map intervention
did not occur in the measured execution. It still certifies deterministic executed
closure, not silicon pixel correctness; exact-ten fresh-process evidence and
independent hardware pixel validation remain separate requirements.

## Step budget (`WM2000_MAX_STEPS`)

Default 20,000,000 scheduler steps. Override with `WM2000_MAX_STEPS=<n>`
(positive integer). Measured economics (2026-07-21 trace,
`/tmp/wm2000-boot-trace.jsonl`): one full VI field of boot activity —
retrace fan-out, the AKI sound pump's ~6 raw `AI_STATUS`/`AI_LEN` polls
(each charged 32 guest cycles and one scheduler step), and one audio-task
submit — costs ~10–16 steps, so the default budget covers on the order of
a million VI fields (hours of virtual boot time). Audio tasks are paced
at exactly one per field by the retrace, the natural hardware cadence,
**not** a starved-feedback loop from the stand-in ucode completing
instantly — verified by the constant 1,563,558-cycle submit gaps in the
trace. Raise the budget for long captures; the budget is not what limits
graphics frames (see below).

## Boot stall after the first 3 gfx frames (2026-07-21)

With the IPL3 boot copy + RDRAM swizzle + RSP replay fixes in place, boot
reaches real rendered frames, then stalls:

1. sim 0 → ~16.03B cycles (~10,300 fields): audio-pump-only phase — one
   `M_AUDTASK` per field, no gfx.
2. sim ~16.03B–16.07B: the game submits 3 gfx frame pairs (F3DEX2 XBUS
   tasks); `osViSwapBuffer` fires and non-uniform framebuffers are dumped
   (near-white dithered fade frames).
3. sim >16.07B: **no further task submissions of any kind.** Persists at
   any budget (verified to 50M steps ≈ 38 virtual hours).

Deadlock shape (2026-07-21 trace diagnosis, using the new
`QueueOpKind::Drop` + `TraceKind::EventMesg` instrumentation): the gfx
manager (executor thread 17) and audio manager (thread 18) share **one
FIFO mesg queue** (rdram 0x52320) for `OS_EVENT_SP` task-done messages —
registered once at boot, never re-registered, zero messages dropped, all
completions delivered intra-field (~192 cycles after kick). The final
sequence (trace seq 312716–312800):

1. gfx task loaded+kicked by 17; 17 parks on 0x52320 — but 18 has been
   parked there since 23M cycles earlier, so the SP-done wakes **18**;
2. 18 treats it as its grant, kicks the last audio task (#9307), parks;
3. that audio task's SP-done wakes **17** (`Wake, thread: 17` — the
   crossover), which proceeds as if its gfx task completed: it loads the
   next gfx task, asks the service thread (3) for the go-ahead on queue
   0x522E8, and never kicks it;
4. terminal state: main thread (6) blocked forever on the sound-service
   response queue 0x559C0; service thread (3) wakes every field on its
   retrace queue 0x55948 but never replies; 18 parked forever on
   0x52320; the AI pump starves (its feeder queues 0xE0908/0xE0930 go
   quiet); no task of any kind is ever submitted again.

Eliminated by direct evidence: message drops (0 `Drop` events),
`OS_EVENT_SP` re-registration (a single boot-time `EventMesg`),
`osSpTaskYield` preemption (never called in the window), completion
latency past a field boundary (clamping latency to 500k cycles produces a
byte-identical trace), and the stand-in audio ucode (routing audio
through the real RSP LLE replay — `set_audio_task_lle` — changes
nothing).

RESOLVED (2026-07-21): decompiling the two manager loops answered the
open question and located the divergence in fn64, not the game.

The guest protocol (funcs_0.c): `func_80000B30` builds the AKI event
manager — per-runner command queues 0x522B0 (audio) / 0x522E8 (gfx), the
SHARED `OS_EVENT_SP` done queue 0x52320 (= 0x522E8+0x38), the DP queue
0x52358, and the handoff queue 0x52390 — then starts thread 18 (audio
runner, entry `func_80001024`, **priority 0x6E**) and thread 17 (gfx
runner, entry `func_80001180`, **priority 0x64**). The gfx runner kicks
a task and parks on 0x52320 for its done. The audio runner, on
receiving a command while a gfx task is in flight (`*0x52760 != 0`),
calls `osSpTaskYield`, parks on 0x52320 to consume the gfx task's
yield-or-done message, kicks its own task, parks on 0x52320 AGAIN for
the audio done, then either restarts the yielded gfx task or re-posts
the done to 0x52320 for a gfx task that completed instead of yielding.
So BOTH runners being parked on 0x52320 simultaneously is a designed,
legitimate state — the protocol is correct on hardware because libultra
wakes blocked receivers in THREAD-PRIORITY order (`__osEnqueueThread`
keeps `mq->mtqueue` priority-sorted; `osSendMesg` pops the head): every
SP-done that arrives while both are parked wakes the higher-priority
audio runner (0x6E > 0x64) first, and only the final resumed-gfx done
falls through to the gfx runner.

fn64's divergence: `fn64-runtime`'s `BlockedList` popped waiters in
ARRIVAL order. After the audio runner consumed the gfx yield-done and
re-parked, the gfx runner was the longer-parked waiter — so the AUDIO
task's done woke the GFX runner (the trace's crossover, seq
312716-312800). Fixed in `crates/fn64-runtime/src/mesgqueue.rs` by
making both blocked lists priority-sorted at insertion (descending,
FIFO among equals), with the parking thread's priority captured at
block time. With the fix the boot runs straight through the former
3-frame wall: 5 gfx tasks / 6 `osViSwapBuffer` swaps, and the fade
ramp COMPLETES (near-white dither at swap #0 through near-black at
swap #4/#5) before boot moves on to the next phase.

## Second fix same day: PI-manager command-queue model

Immediately past the fade, boot livelocked at 100% host CPU inside the
chunked asset loader (`funcs_5.c` `func_80012064`, asm 0x8001214C:
`while (osEPiStartDma(..) != 0);`). fn64 returned -1 whenever ONE PI
transfer was in flight, but real `osEPiStartDma` enqueues the OSIoMesg
on the PI manager's command queue and returns 0 — -1 means the command
queue itself is full. The guest's retry loop never yields, so under
the cooperative executor the pending completion could never commit.
Fixed in `crates/fn64-abi` (`pi.rs`/`lib.rs`): managed requests
accepted while busy are queued (capacity = the game's own
`osCreatePiManager(cmdMsgCnt)`; NWXE passes 0x40, funcs_0.c asm
0x800004F8) and started FIFO as each transfer completes.

## RESOLVED (2026-07-21): music-sequencer stub livelock

Boot progressed past the fade + asset loading into the AKI sound
driver's sequencer tick and spun forever inside ONE scheduler step:
`funcs_9.c` `func_80023930` (asm 0x80023A4C) loops
`while (nextEventTime - clock < 0) func_80023C20(track);` — and
`func_80023C20` (the 0x54C-byte music-sequence command interpreter,
`disasm/asm/1050.s` line 40933, marked `nonmatching`) was emitted as an
EMPTY stub in RecompiledFuncs.

Root cause was a CORPUS defect, repaired in aki-recomp (commits
`add8b28` + `9f06f81` on the corpus side): gen_stubs.py's opcode scan
had auto-stubbed the interpreter on two guarded IDO div-by-zero
`break` asserts (the same false-positive class as `func_8011DFE4` /
`func_800E1FB8`). Its dispatch is a `jalr` through `D_80047F24` — a
function-POINTER table in .data (rom 0x48B24; 45 entries, command
bytes 0x80–0xAC, each a handler function in 0x80022xxx/0x80025xxx),
which N64Recomp translates fine as `LOOKUP_FUNC` — no jump-table
declaration was needed, just un-stubbing via profile.toml
`[syms].force_recompile`. Second rung the same day: with the
interpreter live, the sequence's first tick issued command 0x90 into
the still-stubbed handler `func_800226A0`; the stub's stale `$v0` made
the dispatch loop re-read code bytes as sequence data and spin (sim
frozen at ~16.153B). Its three div-guard siblings (`func_800226A0`,
`func_800256AC`/`func_800257D4` = cmds 0x84/0x85, and `func_800241E8`,
jal'd directly by the tick epilogue) were force-recompiled the same
way.

With all five repaired, boot clears the sequencer rung: gfx task
submissions resume past the former 16.07B-cycle wall (10 tasks by
16.12B), audio stays exactly 1/field, and the sequencer emits its
first real voiced audio command list at sim ~16.175B.

## RESOLVED (2026-07-21, later): first voiced audio task blew the LLE admission bound

That first real audio command list was the next wall — fn64-side, in the
RSP LLE core's DMEM model. Diagnosed with the new env-gated forensic
capture (`FN64_RSP_LLE_DEBUG_DIR=<dir>` dumps the admitted DMEM/IMEM,
machine state, OSTask header, a single-stepped PC ring, and the raw
rdram window on an admission-bound hit) plus the offline replayer
(`cargo run -p fn64-audio --release --example rsp_replay`), which
re-executes the captured task deterministically and reported the first
bad control transfer:

The wm2000 audio ucode (in-ROM at 0x39510, IMEM 0x080) dispatches its
8-byte commands through a 16-entry halfword jump table at DMEM 0x0,
with two unused slots at 0xE/0x10. Command 0x0F stores a DRAM segment
pointer across those slots with `sw r1, 0xe(r0)` — an UNALIGNED word
store, legal on the byte-addressed RSP scalar unit. fn64's
`Dmem::write_w` modeled only aligned stores (native word, no byte-lane
XOR), so the store scattered the pointer bytes onto the wrong `^3`
lanes and rewrote table entry 6 (DMEM 0xC) from 0x1208 to 0x5FC0. The
next command-6 dispatch jumped to IMEM 0x1FC0, ran off the top of IMEM
into the rspboot remnant at 0x000, re-entered the ucode at 0x1080,
re-read the OSTask header — which the ucode itself legitimately
overwrites as mixer scratch (negative-offset SQV to 0xfb0/0xfc0, `sh`
to 0xff0) — and looped forever on a garbage fetch pointer, tripping the
2^26-instruction bound with the PC sampled mid-fetch at 0x1128.

Fixed in `crates/fn64-audio/src/rsp/dmem.rs`: `read_w`/`write_w`/
`read_h`/`write_h`/`read_hu` now assemble/scatter logical big-endian
bytes on their `^3` lanes at ANY byte address (aligned fast paths
bit-identical), with regression tests for the exact jump-table
scenario. The captured failing task replays to BREAK in 106,411
instructions, and in-harness the voiced task completes.

## Second wall the same rung: PI command-queue-full retry livelock

Immediately past the voiced task, boot livelocked at 100% host CPU in
the AKI chunked loader (`func_80000660`, jal'd from the audio driver's
sample-load chain): `while (osEPiStartDma(..) != 0);` with the
0x40-deep PI command queue genuinely full. Device completions only
commit as virtual time advances, and a shim-level retry — unlike a raw
MMIO poll — carried no instruction checkpoint, so the retrying thread
spun forever inside one scheduling slice. `start_timed_pi_dma` now
charges the same 32-cycle stall raw MMIO polls pay when it reports
PiBusy, so the executor commits the in-flight completion and the next
retry observes a freed slot.

With both fixes, boot runs ~3.05B cycles past the former wall (sim
16.18B → 19.23B): gfx task submissions grow 10 → 14, VI swaps 5 → 7
with NEW frame content (white fade-in frames after the fade-to-black),
and audio fields tick 9,385 → 11,533 through the voiced era.

## RESOLVED (2026-07-21, later): the -4 OSIoMesg size was a pause_self fall-through

The frontier panic (`osEPiStartDma_recomp: OSIoMesg at 0x0005b268 carries
dramAddr offset 0x002c92a0 + size 0xfffffffc (devAddr 0x00144aa4)`) was
root-caused end-to-end with lldb (deterministic run, breakpoints on the
recompiled symbols + hardware watchpoints on rdram):

WHO WROTE THE -4: the streaming-loader itself — `func_80011E20` (funcs_5.c,
the per-stream start: full chain `func_800E1FB8` → `func_800F53C8` →
`func_80011488` → `func_80011E20`) computes `remaining = fileSize − 4` and
`chunk = min(remaining, 0x62)` from the AKI file table
(`func_80003DD4(out, fileId)`: offset table [0x80057200], size table
[0x80057204], base 0x144AA0) and stores the chunk into the stream OSIoMesg
size word at 0x8005B278. The tables were verified CORRECT at the moment of
the failing lookup (size-table[0] = 0x1C5F, matching the ROM). The killer
input was the fileId: **0xC5BE** (50622, way past the 0x2848-entry table).

THE MECHANISM (fn64 bug): `func_80003DD4` validates `1 <= fileId < 0x2848`
and on failure enters the guest's intentional hang-assert — `j self; nop`
at 0x80003DF0. N64Recomp's C codegen emits that self-loop as a bare
`pause_self()` call with NO loop back (the reference runtime never returns
from it without an explicit `osStartThread`). fn64's `pause_self` used
`Yield::PauseSelf` (rung 14's auto-requeue), so the thread RESUMED and
FELL THROUGH the unpassable assert into the lookup, indexed 0xC5BD*4 past
both tables (reading zeros), and fabricated romStart = 0x144AA0+0 (hence
the "live-looking" devAddr 0x144AA4) and fileSize = 0 (hence size −4).

FIX (fn64): the C-ABI `pause_self` now parks the thread —
`Yield::StopSelf` in `fn64-runtime` (state `Stopped`, off the run queue,
resumable only via `osStartThread`, matching N64ModernRuntime). The
translated-code path (fn64-recomp-rs) keeps auto-requeue `PauseSelf`
because its codegen preserves the loop (`pause_self(); pc = self;
continue`). Regression tests: `rung_regressions.rs`
(`stop_self_parks_the_thread_until_an_explicit_restart`,
`stop_self_parked_thread_never_starves_other_work`) and
`fn64-abi::thread::tests::pause_self_parks_via_real_executor_until_explicit_restart`.

With the fix, boot no longer aborts: the game thread parks honestly at the
guest assert at sim ~19.23B and the rest of the machine keeps running
(verified to a 2M-step budget = sim 181.9B: audio fields tick past
106,000 tasks, VI keeps swapping the same last frame; 7 VI swaps / 7 gfx
tasks, unchanged — all still untextured fade/fill frames, 0 triangles).

## RESOLVED (2026-07-21, latest): announcer voice request with an uninstalled voice map

Why fileId was 0xC5BE at all (the next rung, fully forensically mapped):

1. The demo/attract engine queues a REAL pointer-shaped voice request for
   sound channel 1: `func_8011E4BC` (funcs_22.c, request installer; chain
   `func_8011C91C` → `func_8011F078` → `func_8011EC20` → `func_8011E6A4`)
   sets chan1 state ([chan+0x2FC] at rdram 0x2C8998) to 0 = "start voice"
   with sound code [chan+0x2E6] left at its init value 0.
2. The per-frame tick `func_800F53C8` sees state != 0x80(idle) and resolves
   code 0 via `func_800F5190(chan, 0)`, whose code-0 rule reads the
   per-channel voice-map halfword table at chan+4 — designed to map
   code -> streamed-voice fileId (e.g. the wrestler's announcer lines).
3. chan1+4 (rdram 0x2C86A0) was NEVER written by any CPU code (hardware
   watchpoint over the whole run: only the boot-arena bzero, then a PI DMA,
   then whole-rdram render round-trips). The chan array (0xC30 bytes at
   0x802C8390, alloc'd in `func_800E1DF0` and stored to [0x8011B5D8]) is
   allocated UNCLEARED from the guest heap, directly over the freed
   decompression TEMP buffer of file 0x435 (the sound-map file: compressed
   0x87F2 bytes DMA'd to temp 0x802C0C40.., decompressed 0x290DC bytes to
   dest 0x802C9450, temp freed — `func_800110C0` → `func_80003E98`). The
   stale compressed bytes at temp+0x7A60 (ROM 0x48CDE4: C5 BE B5 5E) are
   exactly what chan1+4 later reads as 0xC5BE.
4. `func_800E1DF0`'s own init loop only writes chan+0x25C..0x2FC (state
   0x80, codes 0/-1); nothing in the reachable, non-stubbed code installs
   the chan+4 voice-map table before the request fires (stub-closure scan
   over the whole dispatch subtree found only the already-annotated
   `func_80120D60` fragment, which is camera math, not the installer).

RESOLUTION (same day, lldb function-tracing + state probes): the installer
EXISTS, WORKS, and RAN — the frontier was a heap-content divergence, not a
missing installer or a spurious request.

The installer, end to end (all recompiled, none stubbed): the per-frame
sound tick `func_800E1FB8` (called once per game frame from the scene
loop `func_800E2704`, AFTER the 4-channel request resolver `func_800F53C8`
pass) calls `func_800F5BB4`, which dispatches on the sound mode
`D_8011B0F4` (1 = demo, set by the demo-engine init `func_8011EE44`) to
`func_80122558`, which steps `func_800F5704` for each channel.
`func_800F5704` runs a wrestler-assignment countdown at chan+0x2F0:
assignment tick copies chan+0x268→0x264, next tick sees 0x266 != 0x264,
sets the countdown to 3, and on the decrement-to-2 tick calls
**`func_800F6ED8` — the voice-map builder**: bzero(chan+4, 0x248), then
per announcer-code entries assembled from the decompressed sound-map
file's tables (`D_80107EFC`/`D_80107EF8`/`D_80107198`, code list
`D_8010718C`) into the 0x124-halfword map at chan+4 (the `sh v1, 0x4(v0)`
loop at asm 0x800F723C). Probes watched it install chan0's real map at
demo frame 6 (chan0's packed theme voice, id 2, code -5).

Why the assert fired anyway: the demo pointer-voice protocol
(`func_8011E4BC`) is DESIGNED to fire one frame after assignment — frame
N assigns chan+0x268 and idles the channel (state |= 0x80), frame N+1
sees the id already current and fires (`func_80122428` + state 0), and
the tick's resolver pass reads the map (code 0 = "announce wrestler
name" = map slot 0) BEFORE that same tick's 5BB4 phase can run the
install countdown. The guest tolerates this BY DESIGN: `func_800F5190`'s
code-0 rule (asm 0x800F5310) treats a ZERO map slot as "not installed
yet" and falls back to the built-in announce tables (`D_801065A0`..) via
a code -1 re-entry. Fresh chan arrays read zero on hardware because the
AKI heap's arena is bzero'd at first alloc (`func_80000898`, 0x21D800
bytes at 0x80172000) and the next-fit roving pointer's position at
scene-init is the product of thousands of timing-dependent alloc/free
pairs (probe: 3,310 heap events before the demo, dominated by repeating
per-poll loader pairs). Under fn64's current pacing (the game frame loop
completes one frame per ~1,400 VI fields against a hardware-cadence
audio pump — thread 6 wakes on its vsync queue 0x838A8 every field but
only finishes a demo frame rarely) that history collapses, and the chan
array lands exactly on the freed decompression temp of sound-map file
0x435 — so chan1's map slot 0 read the stale compressed pair 0xC5BE
instead of virgin zeros, and the streamer asserted on the wild fileId.

FIX (harness-level, documented in `src/main.rs`): reproduce the
hardware-visible outcome — whenever a fresh chan-array pointer appears at
`D_8011B5D8`, zero the four 0x248-byte voice maps at chan+4, exactly the
virgin bytes the guest's own zero-tolerant fallback expects; real
installs land later and overwrite freely. The honest root fix is fn64's
game-frame pacing (the ~1,400-fields-per-frame stall against the
audio-clock-synced demo timeline), which is its own rung.

## RESOLVED (2026-07-21, latest): first real RDP triangles rasterize

With the voice maps virgin, the fileId assert never fires, the game
thread survives the demo's announcer requests, and boot advances past
the former park: gfx task #7 (first after the 7 fade/fill tasks)
submits a raw RDP XBUS stream containing opcode 0xCE — an RDP
triangle-family command (shade+texture triangle) — i.e. the intro scene
is finally drawing REAL GEOMETRY. The reference backend panicked
(`raw RDP opcode G_<unrecognized> (0xce) ... is unsupported`).

Resolved with THREE reference-backend rungs (fn64-render-rt64), each
hit in sequence by the same task:

1. **Triangle wire encoding.** The raw-RDP lane already implemented all
   eight edge/coefficient triangle layouts (decode + edge-walker +
   shade/texture/Z attribute planes in `raster.rs`) — but only under the
   canonical 0x08..=0x0F spelling its fixtures used. The RDP decodes
   just the low 6 bits of the command byte and RSP microcode sets the
   top two, so real streams carry 0xC8..=0xCF. Both spellings now name
   the identical decode; other alias bases (0x48/0x88) stay loud.
2. **RGB noise dither.** The triangles select `G_CD_NOISE`, which
   trapped. Implemented under the existing alpha-noise precedent: one
   deterministic per-pixel 3-bit xorshift32 draw applied to the blender
   output on all three channels before the pixel write. The ordered
   MagicSquare/Bayer matrices still trap (their tables are unpublished).
3. **Pattern alpha dither.** The same triangles select `G_AD_PATTERN`,
   which the public gbi.h couples to the RGB dither stage's per-pixel
   value (`G_AD_NOTPATTERN` = its 3-bit complement) — exactly
   representable under RGB noise, zero guessed tables. The dither pair
   is now drawn once per fragment (`Framebuffer::fragment_dither`) and
   threaded explicitly through the fragment structs. Pattern alpha
   under an ordered/disabled RGB selector still traps by name.

With all three, gfx task #7 renders **133 raw shade+texture triangles**
(`[fn64-render-rt64] gfx task #7: NON-CLEAR (133 tris)`), swap #8
presents, and the frame stream shows the demo scene's first geometry.
Stream #7's shape (decoded from the `FN64_XBUS_STREAM_DUMP_DIR`
capture): full-screen fill to near-black, 133 green/blue Gouraud
triangles (the arena scene) around y=155..270, then 160 full-screen
32x24 texrect tiles — the white fade overlay still at (or near) full
cover, which is why the dumped frames remain near-white: the geometry
is drawn UNDER the fade. Visible AKI/THQ/title content requires
surviving more frames of the fade ramp — blocked by the next frontier
below. New offline tool: `cargo run -p fn64-render-rt64 --example
xbus_replay -- <xbus-NNNN.bin> <out-dir>` replays a captured stream
through the reference backend without booting the game.

## RESOLVED-AS-DIAGNOSED (2026-07-21, latest): the wild-pointer SIGSEGV was a
## deterministic guest cascade, now trapped loudly at its first symptom

Full forensics on the demo-scene wild-pointer crash below (watchpoint
sessions on the corrupted `s5` save slot, whole-RDRAM byte-diffs between
runs, and macOS crash-report comparison):

1. **The "run-variant host bytes" hypothesis is REFUTED for a fixed
   binary.** Two bare (ASLR-on) runs of the same binary produce
   byte-identical 656 MiB RDRAM images at scheduler steps 200k, 240k,
   244k, 248k and 250k (`WM2000_SNAPSHOT_STEP`/`WM2000_SNAPSHOT_PATH`,
   the new determinism-forensics knob in `src/main.rs`), and crash at the
   IDENTICAL guest pointer (`0x27814010`) and host site
   (`func_80030EC0`'s matrix store) both bare and under lldb. The
   run-to-run variance observed last cycle was cross-BUILD variance:
   different fn64 builds legitimately produce different guest-visible
   bytes (renderer dither changes feed back through the framebuffer, the
   KSEG1 fix below changes task-yield bytes), shifting where the same
   cascade first faults.
2. **The cascade (established by value-change watchpoints on the
   `func_80121764` `s5` save slot at guest 0x800837FC):** in one demo
   frame (~step 250k), `func_80122204`'s object loop calls
   `func_80121764`, whose body stores G_ENDDL/zero words through a
   DL-cursor that points INTO the game thread's own stack (stack top
   0x800839A0, per `osCreateThread` at 0x80022280), zeroing its own
   saved `s5`; the restored garbage then walks a phantom 0xEC-stride
   object chain for tens of thousands of iterations (observed marching
   monotonically for hundreds of MB through fn64's oversized mapping,
   where real hardware faults at 8 MiB), until a store through the
   chimera pointer `0x27813FE8` exceeds the 656 MiB mapping -> host
   SIGSEGV.
3. **Two real fn64 soundness bugs found and fixed on the way:**
   - **KSEG1 uncached-mirror aliasing** (`fn64_mmio_proxy.h`,
     `fn64_fold_kseg1_mirror`): generated-C accesses through `or
     $v0, 0xA0000000`-built pointers (WM2000's own task loader does this
     at 0x80031E28 to read `ucode_data+0xBFC` on OS_TASK_YIELDED reload;
     its raw-read helper at 0x800373B8 does the same) previously landed
     at rdram offset `0x20000000 + phys` -- a disjoint, permanently-zero
     region -- so uncached reads returned deterministic ZEROS and
     uncached writes vanished from the cached view. Hardware maps KSEG0
     and KSEG1 to the same DRAM; the proxy now folds non-RCP KSEG1
     (0xA0000000..0xA4000000) onto the physical bytes. (Verified by
     rdram-snapshot scans: nothing had ever written 0x20000000..0x24000000,
     i.e. every uncached read had been consuming zeros.)
   - **Silent unaligned lw/sw/lh/sh** (`fn64_mmio_proxy.h` +
     `fn64_c_mem_unaligned` in `fn64-abi`): MIPS requires natural
     alignment (hardware raises AdEL/AdES); recomp.h's raw host-pointer
     cast instead read/wrote a byte-lane CHIMERA of two adjacent native
     words -- exactly the kind of value (`0x27813FE8`) the crash rode.
     The C lane now traps loudly, naming the access.
4. **Current state:** the boot no longer dies with a wild host SIGSEGV
   inside recompiled code. It stops with a NAMED trap: `generated-C
   4-byte load at unaligned guest address 0x1DB` inside
   `func_80121764` (i.e. `lw 0xDC($s1)` with the object pointer
   `s1 = 0xFF` -- the first observable symptom of the same demo-frame
   cascade). With the KSEG1 fix the yielded-task reload now consumes
   real yield bytes instead of zeros, and the fatal demo frame arrives
   after gfx task #4 instead of #7 -- the cascade's entry is now the
   frontier itself.

## RESOLVED (2026-07-21, this cycle): the "DL cursor into the stack" was a
## fall-through-truncated recompiled function skipping its own epilogue

The stack-aimed DL cursor hypothesis above was WRONG in an instructive
way. Full watchpoint chain (all evidence deterministic and reproducible
with the scripts noted below):

1. **The cursor was never bad at any hand-off.** The per-frame DL arena
   is a 64 KiB double buffer: base allocated once (`func_8011EE44` ->
   `func_80000898(0x10000)`, lands at 0x80315DE0), master cursor global
   at 0x80129F44 reseeded every frame by `func_8011F078` as
   `base + ((parity^1)<<15) + 8`. A harness word-probe
   (`WM2000_PROBE_WORDS`, new env knob in `src/main.rs`) showed
   base/parity/master stayed sane through the whole run, and an lldb
   entry-check on the full chain (`func_8011F078` -> `func_80121FA8` ->
   `func_80122204` -> `func_80121764`) showed every `*cursor-cell` value
   inside the arena right up to the fatal call.
2. **The corruption was register/stack shredding INSIDE the emitters.**
   A hardware watchpoint on the RecompContext's `r17` (guest `s1`)
   during the fatal `func_80121764` call caught the sequence: s1 valid
   (0x8012A2B8, the object) through `func_80030EC0`'s save/restore,
   then `func_801200DC`'s EPILOGUE restored s1 = 0xFF (the trap value:
   `lw 0xDC($s1)` at guest addr 0x1DB). A second watchpoint on the
   guest stack slot where `func_801200DC` saved s1 (0x8008381C) proved
   NOTHING ever overwrote the slot -- the epilogue simply read the
   wrong address, because guest `sp` was still 0x88 LOW when
   `func_801200DC`'s epilogue ran.
3. **Root cause: answer-key function splits with no fall-through.** The
   corpus's partition split real functions at internal labels, and
   N64Recomp emitted each fragment as a separate C function with no
   fall-through tail call. `func_8011F67C` (the announcer/sprite DL
   emitter `func_801200DC` dispatches to for element kinds 4-7) ends at
   asm 0x8011FE74 and, on hardware, execution falls THROUGH into
   0x8011FE78 -- the epilogue fragment holding the DL-cursor write-back,
   all 10 s-register restores, and `sp += 0x88`. The generated
   `func_8011F67C` C body just fell off its closing brace, so every
   call skipped its own epilogue: caller registers stayed clobbered with
   DL words (s1 = 0xDE000000/0xDF000000/0xFF -- G_DL, G_ENDDL, alpha)
   and sp stayed 0x88 low, so every subsequent restore in the caller
   chain read the wrong frame. THAT is what shredded `func_80122204`'s
   saved registers -- not stores through a stack-aimed cursor. The
   phantom 0xEC walk and the 0x27813FE8 chimera all follow from
   restoring DL words as pointers. The corpus has ~80 such truncated
   fragments (5 more in this very DL path:
   `func_80120B28/B84/BA0/D20/D98`, consecutive fragments of one
   emitter).
4. **The fix (faithful, systematic): build-time fall-through mend.**
   The harness opts into `fn64-boot-harness/build_support.rs`'s shared
   generated-C preparation, which parses `recomp_overlays.inl`'s
   per-section `FuncEntry` tables into an address-contiguity successor
   map and appends `func_<successor>(rdram, ctx);` to every generated body
   that can fall off its end -- exactly the tail call N64Recomp itself emits
   when its function list is correct, and exactly what hardware does
   (execution continues at the next address). 1996 bodies get the
   append (most are dead code after an unreachable trailing delay-slot
   dup -- harmless by construction; the reachable ones are the real
   fixes). Zero game bytes involved: pure source-to-source mend of
   out-of-tree generated C, in OUT_DIR only. The same pass retains the
   native function-entry observer required by release evidence. The repair is
   explicitly WM2000-only; the shared default used by other generated-C
   consumers performs normalization and entry instrumentation without adding
   successor calls.

**Result:** the demo frame survives. Boot now runs to gfx task #26+
(previous frontier: crash after #7), with real scene geometry growing
across tasks -- 133 -> 682 -> 723 -> 777 -> 1013 raw triangles per
task -- and swaps through #15. The fade layer has progressed from
full-white texrect cover to near-black frames (`/tmp/fn64-fb-15.png`,
`/tmp/wm2000-gfx-dumps/wm2000-0007.png` via the new
`WM2000_GFX_DUMP_SKIP`/`WM2000_GFX_DUMP_LIMIT` window knobs).

Forensics tooling from this cycle, all reusable: `WM2000_PROBE_WORDS`
(comma-separated hex guest addrs; logs step-bracketed value changes of
each word -- the harness-level analogue of the lldb value-change
watchpoint), plus the lldb script patterns in `/tmp/wm2000_dlwatch.py`
(cursor-chain entry checks + ring buffer), `/tmp/wm2000_r17watch.py`
(RecompContext register watch armed at the Nth call), and
`/tmp/wm2000_slotwatch.py` (guest stack-slot watch).

### Follow-up (2026-07-23): fn64-shell was not using the mend

The later interactive-shell failure after about 540 VI frames had the same
observable shape (`func_80121764` entered with `s1 = 0xF0`, then `lw
0xDC($s1)` trapped at `0x1CC`), but it did not establish a second bank-copy
parser bug. The WM2000-specific harness above used the forced repair entry
point; `fn64-shell` still used the default generated-C preparer and therefore
compiled **zero** successor calls. Its prepared `func_8011F67C` again fell off
after the call at guest `0x8011FE70`, skipping `func_8011FE78`'s cursor
write-back, saved-register restores, and `sp += 0x88`. The display-list byte
left in `s1` differs from the earlier `0xFF`, but the missing epilogue is the
same.

The shared preparer now exposes a generic structurally admitted path for the
shell. It enables the existing section-local repair only when the generated
corpus itself contains a reachable predecessor fall-off whose
address-contiguous successor restores `ra` plus saved registers, returns
through `ra`, and advances `sp` without allocating a frame. The NWXE corpus
passes that gate and receives the same 1,996 repairs as this harness. A normal
adjacent-function corpus does not. Bank-local names such as `_bank4_text` are
parsed by the same section-reset successor map and have a dedicated regression;
the suspected `func_8011FBD0 -> func_8011FE78_bank4_text` pair is not the leak
because `func_8011FBD0` already executes its own complete restore/return.

## RESOLVED (2026-07-21, same cycle): rasterizer W-reciprocal trap

With the fall-through mend in place the run next ended at a NAMED
renderer trap: `raster.rs` asserted `non-positive W reciprocal
-4570513467 at (30, 206)` around gfx task #27 -- a perspective triangle
crossing the near plane, which legitimately presents w <= 0 at edge
pixels of the interpolated plane. Real RDP hardware's `tcdiv` derives
1/w from the operand's top bits with no sign trap: garbage texels, no
fault. The raw-RDP lane now divides by the magnitude (min one ULP) --
defined-garbage tolerance, hardware's actual behavior. The F3DEX2 HLE
lane keeps its asserts (there the ucode's near-plane cull enforces the
invariant before rasterization).

**Combined result of the two fixes this cycle:** boot runs from
crash-after-gfx-task-#7 to **gfx task #781 / VI swap #272** (a 2M-step
run, ~100x deeper than the previous frontier). The demo scene stays
alive throughout -- per-task triangle counts cycle 50..1013 as the
camera moves -- and the fade layer executes full visible ramps:
near-black (`/tmp/fn64-fb-15.png`) -> mid-gray (`/tmp/fn64-fb-186.png`)
-> near-white (`/tmp/fn64-fb-272.png`, the deepest presented frame).
By task ~#780 the task shape changes to small 11-25-triangle DLs --
logo/menu-scale content beginning, still under the fade cover.

## RESOLVED (2026-07-21): SRAM read one page past the 32 KiB device

The 2M-step run ended at a NAMED save-storage trap, not corruption:
`InMemorySaveStorage::read_into: range 0x20..0x8020 exceeds device
length 0x8000` (save.rs:192), immediately after swap #272 / gfx task
#781. Root cause: the boot save-load path (`func_800F497C`) validates
the 0x20-byte 0x19990901 signature header at device offset 0, then
reads the payload as a round **0x8000 bytes from device offset 0x20**
(OSIoMesg `devAddr=0x20, size=0x8000`, OS_READ) -- 0x20 past the end
of a 256 Kbit chip.

**The evidenced geometry is 256 Kbit (0x8000 bytes), not 768 Kbit.**
Two independent signals agree:

1. **The game's own save-region table** (13 entries of
   `{u16 offset, u16 stride, u16 size, u16 pad}` at `0x8010625C`,
   bank-1 data, ROM `0x7082C`): regions span device offsets
   `0x20..0x66E4` including each region's trailing 4-byte checksum
   (`func_800F03C0`'s per-region sum written at `buf+offset+size`).
   Entry 13 (`0x801062C4`) is the whole-payload pseudo-region the
   format path (`func_800F4B60`) writes: offset 0, length 0x66E0 --
   device end 0x6700. No code path addresses past 0x8020, and no
   write EVER crosses 0x8000; only the boot read's round 0x8000 does.
2. **mupen64plus.ini**: all WM2000 regions (U/E/J) are
   `SaveType=SRAM` -- the flat 32 KiB device.
**Fix (hardware address decode, not a bigger device):** a discrete
256 Kbit SRAM part only decodes A0..A14, so the PI byte address
aliases modulo the power-of-two device size -- the read's last 0x20
bytes wrap to `0x0..0x20` and return the signature header again, into
a buffer tail the game never consumes (payload use caps at 0x66E0).
`PiDma::sram_decode` (fn64-runtime `rom.rs`) now models exactly that
for both directions: offset masks down, a transfer crossing the end
splits at the boundary and wraps to 0. A transfer longer than the
whole device (aliases every byte more than once -- no shipped
pattern) and non-power-of-two devices (no defined undriven-line
model) keep the loud trap; EEPROM/Flash/PFS bounds traps untouched.
4 new wrap tests in `rom.rs`; workspace 1361 passed / 0 failed.

**Result:** the save read completes and boot runs past the old
frontier to **VI swap #732 / gfx task #1241** (manually bounded, no
crash -- 2.7x deeper). Immediately after the save read the task shape
changes for good: tasks #783+ carry a CONSTANT 100/104-triangle DL
(vs the demo's cycling 50..1013) -- a new static scene (title/menu
scale) never reached before -- while the fade layer keeps executing
full ramps (white `/tmp/fn64-fb-272.png` -> black `fn64-fb-318.png`
-> mid-gray `fn64-fb-360.png`/`fn64-fb-500.png` -> black
`fn64-fb-414.png`, deepest `fn64-fb-732.png`).

## Next frontier: the fade cover never lifts (no scene bleed-through)

No presented frame ever contains textured/scene content: every
`/tmp/fn64-fb-*.png` through #732 is a UNIFORM fill at the fade's
current level (a full-frame variance scan found structure only from
the 1-px dotted VI artifact column). That is evidence of a compositing
bug, not game logic: at mid-ramp, an alpha-blended fade rect over the
live scene (549..723-tri bursts pre-save at tasks #778/#25/#28;
constant 100/104 tris post-save) MUST show the scene tinted, never a
flat field -- so the full-screen cover is being rendered opaque (color
ramping, alpha ignored), or the scene geometry never survives into
the color image. Next rung: capture a post-save-read display list
(`FN64_XBUS_STREAM_DUMP_DIR` + `WM2000_GFX_DUMP_SKIP=783`) and decode
the cover texrect's combine/blend state vs the scene draws -- decide
whether the raw-RDP lane's blender, the HLE texrect path, or the
scene's TMEM loads are at fault.

## Superseded framing (2026-07-21, earlier): wild-pointer crash in the demo scene, one task after first geometry

Immediately after gfx task #7 and swap #8, the boot dies with a real
host SIGSEGV inside recompiled guest code — not a renderer trap. Crash
chain (lldb, `--batch -o run -k bt`): scene loop `func_800E2704` →
demo engine `func_8011C91C` → `func_8011F078` → `func_80122204` →
`func_80121764` (funcs_22.c, demo scene object setup, nonmatching) →
`func_800EEA14` → `func_80030EC0` (funcs_1, an FP multiply chain that
looks like a matrix builder), which stores through a garbage guest
pointer (observed `str wzr, [x8, x9]` with x9=0x80000000 — KSEG0 NULL —
and variants). The faulting SITE varies run to run between
`func_80030EC0` stores and `func_80121764` loads, and the wild
addresses vary beyond ASLR page slide — so the guest is consuming
run-variant bytes. Guest time is deterministic (`osGetCount` is
virtual-time), which points at host-derived garbage reaching guest
state (uninitialized RecompContext/host-stack reads in a nonmatching
recompiled function, or a host value stored into rdram) rather than a
timing divergence. Next rung: watchpoint forensics on the pointer slot
`func_80030EC0` dereferences, same method as the voice-map rung.

## Known frontier (2026-07-14)

See `docs/DESIGN.md`'s "M1 boot-host attempt" section for the full,
byte-cited writeup: this run gets substantially deeper than any prior
milestone (real thread creation, a real stack pointer, a real PI DMA three
call-levels deep on a second thread) before stalling inside a long-or-
unbounded recompiled loop in `func_800004D0` — not yet root-caused. Four real
bugs (executor reentrancy, thread-identity-by-handle, an unseeded stack
pointer, and a pervasive native-vs-big-endian `MEM_W` mistranscription)
were found and fixed along the way, each with a regression test in
`fn64-abi`/`fn64-runtime`.

That historical run used a non-executing audio stand-in. The harness now
selects `LleAccuracy`, so admitted live audio-task IMEM executes through
fn64's clean-room RSP interpreter without linking external runtime code.
