# fn64 runtime completeness vs N64ModernRuntime

This is the mechanically maintained compatibility inventory for
N64ModernRuntime's public libultra `_recomp` shim surface. It measures ABI
surface coverage only. It does not claim that a present shim is cycle-exact,
hardware-exact, or behaviorally identical to the GPL reference runtime.

## Clean-room source and denominator

The canonical set is `crates/fn64-abi/nmr-surface.json`: 116 symbol names and
their subsystem grouping. It was transcribed from fn64's prior clean-room
inventory, whose provenance is N64Recomp's MIT `reimplemented_funcs` list,
N64Recomp-generated public ABI signatures, and public libultra names. It
contains no GPL implementation logic.

The denominator excludes N64Recomp's ignored functions, libc/libm renames, and
the codegen helpers `get_function`, `switch_error`, and `do_break`. Low-level
helpers that fn64 exports but N64Recomp does not include in the canonical 116
are reported separately rather than silently changing the denominator.

## Mechanical gates

Run the ordinary synchronization gate with:

```sh
scripts/check-nmr-surface.py --check-doc
```

It fails if the manifest is malformed, the live source changes without this
document being regenerated, a duplicate export appears, or the generated
matrix drifts. `cargo nextest run -p fn64-abi` runs the same check through
`tests/nmr_surface.rs`, and `scripts/lint-docs.py` runs it as part of the doc
gate.

Refresh this document after an intentional ABI change with:

```sh
scripts/check-nmr-surface.py --write-doc
```

The eventual full-surface release gate is:

```sh
scripts/check-nmr-surface.py --require-complete --check-doc
```

That strict command intentionally fails until all 116 canonical shims are
classified `implemented`, with zero partial, trap, or absent entries. Do not
weaken it to make a pre-parity tree green.

Once canonical parity is green, the broader runtime gate also checks every
adjacent `_recomp` export so title-specific helpers cannot hide outside the
116-name denominator:

```sh
scripts/check-nmr-surface.py --require-complete --require-all-exports --check-doc
```

## Behavioral parity frontier

The export gate is green, but behavioral parity is tracked independently.
Raw SI now executes controller query/read and external-channel EEPROM
probe/read/write packets. It also executes Controller Pak/Rumble Pak
accessory reads and writes with the public address and data CRCs. Transfer Pak
power/probe, mode/status, bank, and 16 KiB Game Boy bus windows run through
the same raw accessory protocol. Inserted cartridge images use typed ROM/RAM
state with ROM-only, MBC1, MBC2, MBC3, and MBC5 mappers, so mapper-register
writes and persistent cartridge RAM are not flattened into a guessed byte
array. Timer-bearing MBC3 cartridges advance seconds on exact guest-cycle
boundaries even while Transfer Pak power is off, retain immutable latch
snapshots, honor halt/resume, and implement the 9-bit day counter plus sticky
overflow carry. Plain MBC3 cartridge types do not fabricate timer registers.
Timer+battery types also support an exact-ROM-bound, checksummed v1 RTC
sidecar. The host explicitly injects checkpoint/resume Unix nanoseconds;
restore rejects clock rollback and materializes the elapsed interval once,
after which guest cycles are again the sole clock. Host timestamps and powered
mapper/latch/Pak state are not retained in runtime evidence, while the
materialized live RTC and subsecond phase remain covered.
The six public `osGbpak*` adapters use
that same bus for aligned reads/writes, sticky cartridge-removal status,
registration-header validation, and address-line connector checks. Their
documented 0.2-second initialization and 0.12-second power-on waits advance
the deterministic guest clock. EEPROM packets and `osEeprom*` share the
fabric's single save store and typed write-busy deadline;
`raw_eeprom_and_high_level_shims_share_one_backing_store` proves both
directions. A write latches eight bytes, leaves the backing store unchanged
until the exact guest-cycle deadline, exposes status bit `0x80` through Info
and overlapping raw Write responses, then commits without requiring another
SI command. Single high-level writes return after command acceptance;
high-level reads and consecutive writes poll the same state, while LongWrite
advances the public EEPROM Manager's conservative 15 ms timer once per block.
Raw 4-Kbit block addresses ignore their upper two bits as documented, while
the stricter high-level API rejects out-of-range addresses. Raw Controller Pak
blocks and high-level PFS notes share one authoritative physical image. The
host can select 1–62 32-KiB banks at runtime without recompiling the game.
Joybus writes in the upper address half update the retained bank latch, reads
there return zero, and lower-half transfers address the selected bank. Uniform
out-of-range selects on power-of-two capacities use the documented low-bit
mirror (including the published four-bank 5→1 vector); nonuniform select
payloads and out-of-range selects on odd capacities are rejected because their
latch/mirror behavior is not publicly established. Each bank contributes a primary
and backup FAT page in bank zero, the directory follows those tables, and the
checksum slot reserves the first physical page of every later bank. High-level
allocation/deletion encode global 16-bit page chains across bank boundaries;
raw metadata writes determine later discovery, chains, and free space.
Ambiguous FAT copies, cycles, shared pages, orphan pages, reserved boundary
pages, and invalid directory records report `PFS_ERR_INCONSISTENT`. These
rules follow the public
[Controller Pak hardware map](https://n64brew.dev/wiki/Controller_Pak) and
[filesystem geometry](https://n64brew.dev/wiki/Controller_Pak/Filesystem?oldid=5639).
Raw Rumble writes share the
typed motor latch with `osMotor*`. The parser distinguishes 4K and 16K EEPROM
identities, reports the documented absent-device responses, and traps
malformed shapes or checksums with protocol context.

Release evidence separately records typed successful save operations at the
authoritative storage boundary: raw and high-level EEPROM reads plus matured
programming commits, completed 32-KiB domain-2 SRAM DMA, FlashRAM read/write/
erase, and high-level or raw Controller Pak data read/write. It does not count
probes, rejected requests, Flash staging, a pending EEPROM write, or Controller
Pak upper-half reads and bank-latch writes. EEPROM events are owned by PiDma,
where both Joybus paths converge; raw Controller Pak events are emitted only
after a lower-half block operation succeeds against the typed Pak image.
Timed SRAM events are likewise recorded by PiDma at the exact PI commit cycle,
so a multi-device advance cannot reorder them around an SI callback.

Release evidence also records successful controller/accessory operations at
the authoritative high-level or raw Joybus boundary. Standard input reads,
Rumble motor controls, Transfer Pak reads/writes, and Voice reads/writes/
controls map to distinct closure paths; Controller Pak behavior remains bound
to the save/PFS operation path. Port configuration, identity queries, probes,
failed operations, and Rumble probe reads do not count. Raw PIF observations
retain wire order and are appended by the same device-fabric callback that
commits the command, while high-level adapters record only after their typed
operation succeeds. Installing a ROM clears this historical evidence so a
later release scenario cannot inherit an earlier device operation.
Representative-matrix verification requires the matching positive path for
every declared controller/accessory, rejects paths for undeclared devices, and
retains the exact canonical closure ledger in the verified-matrix v7 wire so a
saved result can revalidate those feature-specific facts without the source
reports.

Release schema v13 also retains every reached executable destination at the
authoritative entry boundary. Prepared native bodies record cycle plus stable
section/offset/link-VRAM identity; the Rust block lane records bank-qualified
PC plus the immutable runner-artifact identity. The report binds exact order,
a canonical unique set with counts, and separate digests for both views. The
verified-matrix v7 wire carries and independently revalidates that evidence.
Lookup probes and failed destinations are excluded, while unidentified or
cross-lane program evidence fails closed.

Voice now has one typed initialization and recognition-state authority across
the nine high-level shims, raw Joybus Info, raw `0x09` result and `0x0B`
status, the captured five-write `0x0D` initialization sequence, and captured
`0x0C` initialization/clear/start/stop controls. Info returns the captured
`00 01` device identifier and changes its readiness byte only after
`osVoiceInit`; `0x0B 00 00` reports the captured pre-initialization `01 00`
status pair and then the same READY/START/CANCEL/BUSY/END status as the
high-level handle, with the public Joybus CRC. The exact `0x0C` payloads
`00 00 01 00`, `02 00 nn 00`, `00 00 06 00`, and `05 00 00 00` finalize
initialization, clear the dictionary, start, and stop, respectively, and return
the captured payload CRC. A result-ready `0x09` read serializes host-injected
`VoiceData` into the captured 36-byte little-endian envelope and consumes the
same pending result as `osVoiceGetReadData`. Guest polling observes all five
documented lifecycle states, including `VOICE_STATUS_BUSY`, while host result
injection outside an active recognition request traps. The public
[Voice Recognition System manual](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro26/26-08.html)
defines the lifecycle, and the public
[hardware summary](https://sites.google.com/site/consoleprotocols/home/nintendo-joy-bus-documentation/n64-specific/voice-recognition-unit),
[command notes](https://pastebin.com/raw/TcsfwpSM),
[communication trace](https://pastebin.com/raw/6UiErk5h), and public
[US](https://pastebin.com/raw/5Fr9G36N) and
[Japanese](https://pastebin.com/raw/rNe7CQNf) captures are the source for the
wire identifier, readiness/status bytes, byte order, control forms, ordered
initialization writes, result envelope, and CRC vectors.

This does not yet close SI accuracy: the DMA latency remains a deterministic
policy, the EEPROM's 15 ms compatibility deadline is not a measured
chip-revision timing model, and raw Voice is still partially closed. A `0x09`
read without a host-injected result has no established error response; `0x0A`
dictionary transfer lacks a proven region-independent staging/error contract;
`0x0D` power/gain writes remain unidentified beyond the exact initialization
sequence; and the captured `0x0C` dictionary-transfer mode lacks established
state/error semantics. Each of those paths records command-specific typed
unsupported evidence and traps rather than accepting a fabricated no-op.
Those are behavioral gaps even
though their public high-level shims are mechanically present below.

The ABI also exports public adapters outside the fixed N64ModernRuntime
inventory below. `osPfsIsPlug_recomp` now probes the shared typed PIF port
identities, honors `osContSetCh`'s active controller prefix, and synchronously
crosses the timed SI/event-queue boundary before writing its result. Its typed
in-flight transaction binds the private queue/message, caller thread,
registered-RDRAM destination, and latched bitmap in both pending and
posted-before-resume release evidence. A pre-existing blocked receiver is
rejected before SI starts rather than being allowed to steal the completion.
Its C ABI shape is link-checked without invoking a blocking shim outside a guest
coroutine, while spawned-thread tests cover completion order, shared-queue
rejection, evidence phase transitions, bitmap digest separation, and
busy/no-write behavior. This additional export does not change the canonical 116-symbol
denominator. Channel-count-dependent hardware timing and the documented
libultra-version conflict over failure value `1` versus `-1` remain explicit
residuals.

## Live surface

<!-- BEGIN GENERATED NMR SURFACE -->
_Generated by `scripts/check-nmr-surface.py` from `crates/fn64-abi/nmr-surface.json` and the live ABI source. Do not edit this block by hand._

Status meanings: **implemented** has no `unimplemented!` path in the shim; **partial** has a real path plus a loud unimplemented branch; **trap** immediately traps; **absent** has no exported `_recomp` definition. These are source-shape classifications, not claims of hardware-exact behavior.

Live headline: **116/116 canonical shims are exported** — 116 implemented, 0 partial, 0 immediate traps, and 0 absent.

| Subsystem | Total | Implemented | Partial | Trap | Absent |
|---|---:|---:|---:|---:|---:|
| core/OS | 28 | 28 | 0 | 0 | 0 |
| thread scheduler | 7 | 7 | 0 | 0 | 0 |
| message queue | 5 | 5 | 0 | 0 | 0 |
| timer | 4 | 4 | 0 | 0 | 0 |
| PI/ROM DMA | 8 | 8 | 0 | 0 | 0 |
| SI/controller | 6 | 6 | 0 | 0 | 0 |
| EEPROM | 5 | 5 | 0 | 0 | 0 |
| Flash | 13 | 13 | 0 | 0 | 0 |
| Controller Pak/PFS | 7 | 7 | 0 | 0 | 0 |
| Rumble Pak | 4 | 4 | 0 | 0 | 0 |
| AI/audio | 4 | 4 | 0 | 0 | 0 |
| VI/DP | 11 | 11 | 0 | 0 | 0 |
| RSP/SP | 5 | 5 | 0 | 0 | 0 |
| Voice/ISV | 9 | 9 | 0 | 0 | 0 |
| **Total** | **116** | **116** | **0** | **0** | **0** |

### Per-shim matrix

| Subsystem | Shim | Status | Evidence |
|---|---|---|---|
| core/OS | `__osInitialize_common_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:46` |
| core/OS | `osInitialize_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:57` |
| core/OS | `osGetMemSize_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:83` |
| core/OS | `osSetIntMask_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:11` |
| core/OS | `__osDisableInt_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:95` |
| core/OS | `__osRestoreInt_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:109` |
| core/OS | `osVirtualToPhysical_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:965` |
| core/OS | `osGetCount_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:148` |
| core/OS | `osSetCount_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:167` |
| core/OS | `__osSetFpcCsr_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:182` |
| core/OS | `osInvalDCache_recomp` | **implemented** | `crates/fn64-abi/src/cache.rs:16` |
| core/OS | `osInvalICache_recomp` | **implemented** | `crates/fn64-abi/src/cache.rs:27` |
| core/OS | `osWritebackDCache_recomp` | **implemented** | `crates/fn64-abi/src/cache.rs:36` |
| core/OS | `osWritebackDCacheAll_recomp` | **implemented** | `crates/fn64-abi/src/cache.rs:47` |
| core/OS | `is_proutSyncPrintf_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:65` |
| core/OS | `__checkHardware_msp_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:95` |
| core/OS | `__checkHardware_kmc_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:104` |
| core/OS | `__checkHardware_isv_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:113` |
| core/OS | `__osInitialize_msp_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:127` |
| core/OS | `__osInitialize_kmc_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:136` |
| core/OS | `__osInitialize_isv_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:145` |
| core/OS | `__osRdbSend_recomp` | **implemented** | `crates/fn64-abi/src/debug.rs:77` |
| core/OS | `__ull_div_recomp` | **implemented** | `crates/fn64-abi/src/softmath.rs:85` |
| core/OS | `__ll_div_recomp` | **implemented** | `crates/fn64-abi/src/softmath.rs:40` |
| core/OS | `__ll_mul_recomp` | **implemented** | `crates/fn64-abi/src/softmath.rs:68` |
| core/OS | `__ull_rem_recomp` | **implemented** | `crates/fn64-abi/src/softmath.rs:110` |
| core/OS | `__ull_to_d_recomp` | **implemented** | `crates/fn64-abi/src/softmath.rs:129` |
| core/OS | `__ull_to_f_recomp` | **implemented** | `crates/fn64-abi/src/softmath.rs:143` |
| thread scheduler | `osCreateThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:48` |
| thread scheduler | `osStartThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:149` |
| thread scheduler | `osStopThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:290` |
| thread scheduler | `osDestroyThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:276` |
| thread scheduler | `osSetThreadPri_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:182` |
| thread scheduler | `osGetThreadPri_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:203` |
| thread scheduler | `osGetThreadId_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:255` |
| message queue | `osCreateMesgQueue_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:14` |
| message queue | `osRecvMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:100` |
| message queue | `osSendMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:33` |
| message queue | `osJamMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:206` |
| message queue | `osSetEventMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:171` |
| timer | `osGetTime_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:127` |
| timer | `osSetTime_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:198` |
| timer | `osSetTimer_recomp` | **implemented** | `crates/fn64-abi/src/timer.rs:30` |
| timer | `osStopTimer_recomp` | **implemented** | `crates/fn64-abi/src/timer.rs:69` |
| PI/ROM DMA | `osCartRomInit_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:732` |
| PI/ROM DMA | `osCreatePiManager_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:990` |
| PI/ROM DMA | `osPiReadIo_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1091` |
| PI/ROM DMA | `osPiStartDma_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1119` |
| PI/ROM DMA | `osEPiStartDma_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:806` |
| PI/ROM DMA | `osPiGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1157` |
| PI/ROM DMA | `osEPiRawStartDma_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:926` |
| PI/ROM DMA | `osEPiReadIo_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1038` |
| SI/controller | `osContInit_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1182` |
| SI/controller | `osContStartReadData_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1338` |
| SI/controller | `osContGetReadData_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1114` |
| SI/controller | `osContStartQuery_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1310` |
| SI/controller | `osContGetQuery_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1032` |
| SI/controller | `osContSetCh_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1292` |
| EEPROM | `osEepromProbe_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:188` |
| EEPROM | `osEepromWrite_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:334` |
| EEPROM | `osEepromLongWrite_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:365` |
| EEPROM | `osEepromRead_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:320` |
| EEPROM | `osEepromLongRead_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:350` |
| Flash | `osFlashInit_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:426` |
| Flash | `osFlashReadStatus_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:465` |
| Flash | `osFlashReadId_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:482` |
| Flash | `osFlashClearStatus_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:503` |
| Flash | `osFlashAllErase_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:556` |
| Flash | `osFlashAllEraseThrough_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:568` |
| Flash | `osFlashSectorErase_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:579` |
| Flash | `osFlashSectorEraseThrough_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:590` |
| Flash | `osFlashCheckEraseEnd_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:615` |
| Flash | `osFlashWriteBuffer_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:633` |
| Flash | `osFlashWriteArray_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:655` |
| Flash | `osFlashReadArray_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:692` |
| Flash | `osFlashChange_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:736` |
| Controller Pak/PFS | `osPfsInitPak_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:284` |
| Controller Pak/PFS | `osPfsFreeBlocks_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:313` |
| Controller Pak/PFS | `osPfsAllocateFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:329` |
| Controller Pak/PFS | `osPfsDeleteFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:349` |
| Controller Pak/PFS | `osPfsFileState_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:383` |
| Controller Pak/PFS | `osPfsFindFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:364` |
| Controller Pak/PFS | `osPfsReadWriteFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:413` |
| Rumble Pak | `__osMotorAccess_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1441` |
| Rumble Pak | `osMotorInit_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1374` |
| Rumble Pak | `osMotorStart_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1451` |
| Rumble Pak | `osMotorStop_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:1462` |
| AI/audio | `osAiGetLength_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:131` |
| AI/audio | `osAiGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:105` |
| AI/audio | `osAiSetFrequency_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:47` |
| AI/audio | `osAiSetNextBuffer_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:173` |
| VI/DP | `osViSetXScale_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:143` |
| VI/DP | `osViSetYScale_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:127` |
| VI/DP | `osCreateViManager_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:33` |
| VI/DP | `osViBlack_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:176` |
| VI/DP | `osViSetSpecialFeatures_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:111` |
| VI/DP | `osViGetCurrentFramebuffer_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:380` |
| VI/DP | `osViGetNextFramebuffer_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:407` |
| VI/DP | `osViSwapBuffer_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:161` |
| VI/DP | `osViSetMode_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:66` |
| VI/DP | `osViSetEvent_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:48` |
| VI/DP | `osDpSetNextBuffer_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:61` |
| RSP/SP | `osSpTaskLoad_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch/lifecycle.rs:905` |
| RSP/SP | `osSpTaskStartGo_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch/lifecycle.rs:1015` |
| RSP/SP | `osSpTaskYield_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch/lifecycle.rs:1393` |
| RSP/SP | `osSpTaskYielded_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch/rsp_commit.rs:1401` |
| RSP/SP | `__osSpSetPc_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:12` |
| Voice/ISV | `osVoiceSetWord_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:233` |
| Voice/ISV | `osVoiceCheckWord_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:200` |
| Voice/ISV | `osVoiceStopReadData_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:311` |
| Voice/ISV | `osVoiceInit_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:156` |
| Voice/ISV | `osVoiceMaskDictionary_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:252` |
| Voice/ISV | `osVoiceStartReadData_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:289` |
| Voice/ISV | `osVoiceControlGain_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:271` |
| Voice/ISV | `osVoiceGetReadData_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:334` |
| Voice/ISV | `osVoiceClearDictionary_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:211` |

### Adjacent exports outside the 116

These low-level or title-specific helpers are real ABI exports but are not part of N64Recomp's canonical 116-name `reimplemented_funcs` denominator:

| Shim | Status | Evidence |
|---|---|---|
| `__osPiGetAccess_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1012` |
| `__osPiRelAccess_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1022` |
| `__osSiDeviceBusy_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:633` |
| `__osSiRawStartDma_recomp` | **implemented** | `crates/fn64-abi/src/si/mod.rs:705` |
| `__osSpGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:108` |
| `__osSpSetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:30` |
| `osDpGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:118` |
| `osDpSetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:46` |
| `osEPiWriteIo_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1172` |
| `osGbpakCheckConnector_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:336` |
| `osGbpakGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:227` |
| `osGbpakInit_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:174` |
| `osGbpakPower_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:207` |
| `osGbpakReadId_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:299` |
| `osGbpakReadWrite_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:244` |
| `osLeoDiskInit_recomp` | **implemented** | `crates/fn64-abi/src/pi/timing.rs:1207` |
| `osPfsIsPlug_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:182` |
| `osViFade_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:189` |
| `osViGetCurrentField_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:435` |
| `osViGetCurrentLine_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:421` |
| `osViGetCurrentMode_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:460` |
| `osViGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:445` |
| `osViRepeatLine_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:202` |
<!-- END GENERATED NMR SURFACE -->

## How to read the result

`implemented` is deliberately narrow: the exported function exists and its
body has no `unimplemented!` branch. It may still be a justified no-op or a
target-game approximation. Examples include cache operations over fn64's flat
coherent RDRAM model, synchronous peripheral completion, and VI state that does
not yet model every hardware distinction. The universal behavioral gaps and
their acceptance gates live in `docs/UNIVERSAL-RUNTIME-PLAN.md`.

`partial` means at least one real path exists but another path still traps.
`trap` means the shim immediately refuses to fabricate behavior. `absent`
means no matching exported definition exists anywhere under
`crates/fn64-abi/src/`.

Surface parity is therefore necessary but not sufficient for feature parity.
Behavior known only from N64ModernRuntime must be established by a black-box
differential experiment, never by reading GPL implementation bodies. This repo
does not currently have an end-to-end reference-runtime differential; see
`AGENTS.md` and `crates/fn64-diff/src/lib.rs` before making a behavioral parity
claim.

## Product gates are separate

Three completion questions must remain distinct:

1. **Target-game replacement:** every shim and device path reached by the
   supported game/input corpus works, with framebuffer, audio, input, save, and
   timing evidence.
2. **NMR surface parity:** the strict 116/116 gate above passes.
3. **Universal runtime closure:** CPU, device, dynamic-code, RSP, and RDP
   behavior reaches the zero-unsupported gates in
   `docs/UNIVERSAL-RUNTIME-PLAN.md`.

A game can boot before NMR surface parity, and 116/116 surface parity can pass
before universal behavioral closure. None of those facts may be reported as
one of the others.
