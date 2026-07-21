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
blocks and high-level PFS notes share one
authoritative 32 KiB image for data pages, and raw Rumble writes share the
typed motor latch with `osMotor*`. The parser distinguishes 4K and 16K EEPROM
identities, reports the documented absent-device responses, and traps
malformed shapes or checksums with protocol context.

Voice now has one typed initialization and recognition-state authority across
the nine high-level shims and raw Joybus Info. Info returns the captured
`00 01` device identifier and changes its readiness byte only after
`osVoiceInit`; guest polling observes all five documented lifecycle states,
including `VOICE_STATUS_BUSY`, while host result injection outside an active
recognition request traps. The public
[Voice Recognition System manual](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro26/26-08.html)
defines the lifecycle, and the public
[hardware captures](https://sites.google.com/site/consoleprotocols/home/nintendo-joy-bus-documentation/n64-specific/voice-recognition-unit)
are the source for the wire identifier/readiness bytes.

This does not yet close SI accuracy: the DMA latency remains a deterministic
policy, the EEPROM's 15 ms compatibility deadline is not a measured
chip-revision timing model, Controller Pak management pages are not decoded
into the semantic directory/inode model, MBC3 host-off wall time and battery
metadata persistence are not connected, and raw Voice commands `0x09` through
`0x0D` still trap because the public captures do not yet establish their full
packet and error semantics.
Those are behavioral gaps even
though their public high-level shims are mechanically present below.

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
| core/OS | `osGetMemSize_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:82` |
| core/OS | `osSetIntMask_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:11` |
| core/OS | `__osDisableInt_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:94` |
| core/OS | `__osRestoreInt_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:108` |
| core/OS | `osVirtualToPhysical_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1247` |
| core/OS | `osGetCount_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:147` |
| core/OS | `osSetCount_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:158` |
| core/OS | `__osSetFpcCsr_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:173` |
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
| thread scheduler | `osStartThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:140` |
| thread scheduler | `osStopThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:272` |
| thread scheduler | `osDestroyThread_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:258` |
| thread scheduler | `osSetThreadPri_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:160` |
| thread scheduler | `osGetThreadPri_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:185` |
| thread scheduler | `osGetThreadId_recomp` | **implemented** | `crates/fn64-abi/src/thread.rs:237` |
| message queue | `osCreateMesgQueue_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:14` |
| message queue | `osRecvMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:78` |
| message queue | `osSendMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:33` |
| message queue | `osJamMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:169` |
| message queue | `osSetEventMesg_recomp` | **implemented** | `crates/fn64-abi/src/mesgqueue.rs:140` |
| timer | `osGetTime_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:126` |
| timer | `osSetTime_recomp` | **implemented** | `crates/fn64-abi/src/system.rs:189` |
| timer | `osSetTimer_recomp` | **implemented** | `crates/fn64-abi/src/timer.rs:30` |
| timer | `osStopTimer_recomp` | **implemented** | `crates/fn64-abi/src/timer.rs:63` |
| PI/ROM DMA | `osCartRomInit_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:997` |
| PI/ROM DMA | `osCreatePiManager_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1269` |
| PI/ROM DMA | `osPiReadIo_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1344` |
| PI/ROM DMA | `osPiStartDma_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1372` |
| PI/ROM DMA | `osEPiStartDma_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1109` |
| PI/ROM DMA | `osPiGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1414` |
| PI/ROM DMA | `osEPiRawStartDma_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1208` |
| PI/ROM DMA | `osEPiReadIo_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1317` |
| SI/controller | `osContInit_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:595` |
| SI/controller | `osContStartReadData_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:715` |
| SI/controller | `osContGetReadData_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:531` |
| SI/controller | `osContStartQuery_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:688` |
| SI/controller | `osContGetQuery_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:447` |
| SI/controller | `osContSetCh_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:667` |
| EEPROM | `osEepromProbe_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:105` |
| EEPROM | `osEepromWrite_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:243` |
| EEPROM | `osEepromLongWrite_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:274` |
| EEPROM | `osEepromRead_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:229` |
| EEPROM | `osEepromLongRead_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:259` |
| Flash | `osFlashInit_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:330` |
| Flash | `osFlashReadStatus_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:352` |
| Flash | `osFlashReadId_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:369` |
| Flash | `osFlashClearStatus_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:391` |
| Flash | `osFlashAllErase_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:423` |
| Flash | `osFlashAllEraseThrough_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:435` |
| Flash | `osFlashSectorErase_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:446` |
| Flash | `osFlashSectorEraseThrough_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:457` |
| Flash | `osFlashCheckEraseEnd_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:480` |
| Flash | `osFlashWriteBuffer_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:495` |
| Flash | `osFlashWriteArray_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:517` |
| Flash | `osFlashReadArray_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:544` |
| Flash | `osFlashChange_recomp` | **implemented** | `crates/fn64-abi/src/save.rs:580` |
| Controller Pak/PFS | `osPfsInitPak_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:120` |
| Controller Pak/PFS | `osPfsFreeBlocks_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:149` |
| Controller Pak/PFS | `osPfsAllocateFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:165` |
| Controller Pak/PFS | `osPfsDeleteFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:185` |
| Controller Pak/PFS | `osPfsFileState_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:219` |
| Controller Pak/PFS | `osPfsFindFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:200` |
| Controller Pak/PFS | `osPfsReadWriteFile_recomp` | **implemented** | `crates/fn64-abi/src/pfs.rs:249` |
| Rumble Pak | `__osMotorAccess_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:813` |
| Rumble Pak | `osMotorInit_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:753` |
| Rumble Pak | `osMotorStart_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:823` |
| Rumble Pak | `osMotorStop_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:834` |
| AI/audio | `osAiGetLength_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:144` |
| AI/audio | `osAiGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:114` |
| AI/audio | `osAiSetFrequency_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:28` |
| AI/audio | `osAiSetNextBuffer_recomp` | **implemented** | `crates/fn64-abi/src/ai.rs:183` |
| VI/DP | `osViSetXScale_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:143` |
| VI/DP | `osViSetYScale_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:127` |
| VI/DP | `osCreateViManager_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:33` |
| VI/DP | `osViBlack_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:176` |
| VI/DP | `osViSetSpecialFeatures_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:111` |
| VI/DP | `osViGetCurrentFramebuffer_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:358` |
| VI/DP | `osViGetNextFramebuffer_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:385` |
| VI/DP | `osViSwapBuffer_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:161` |
| VI/DP | `osViSetMode_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:66` |
| VI/DP | `osViSetEvent_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:48` |
| VI/DP | `osDpSetNextBuffer_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:58` |
| RSP/SP | `osSpTaskLoad_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch.rs:1172` |
| RSP/SP | `osSpTaskStartGo_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch.rs:1226` |
| RSP/SP | `osSpTaskYield_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch.rs:1306` |
| RSP/SP | `osSpTaskYielded_recomp` | **implemented** | `crates/fn64-abi/src/task_dispatch.rs:838` |
| RSP/SP | `__osSpSetPc_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:13` |
| Voice/ISV | `osVoiceSetWord_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:216` |
| Voice/ISV | `osVoiceCheckWord_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:187` |
| Voice/ISV | `osVoiceStopReadData_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:280` |
| Voice/ISV | `osVoiceInit_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:147` |
| Voice/ISV | `osVoiceMaskDictionary_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:231` |
| Voice/ISV | `osVoiceStartReadData_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:262` |
| Voice/ISV | `osVoiceControlGain_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:246` |
| Voice/ISV | `osVoiceGetReadData_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:299` |
| Voice/ISV | `osVoiceClearDictionary_recomp` | **implemented** | `crates/fn64-abi/src/voice.rs:198` |

### Adjacent exports outside the 116

These low-level or title-specific helpers are real ABI exports but are not part of N64Recomp's canonical 116-name `reimplemented_funcs` denominator:

| Shim | Status | Evidence |
|---|---|---|
| `__osPiGetAccess_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1291` |
| `__osPiRelAccess_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1301` |
| `__osSiRawStartDma_recomp` | **implemented** | `crates/fn64-abi/src/si.rs:343` |
| `__osSpGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:93` |
| `__osSpSetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:31` |
| `osDpGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:103` |
| `osDpSetStatus_recomp` | **implemented** | `crates/fn64-abi/src/sp_dp.rs:46` |
| `osEPiWriteIo_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1431` |
| `osGbpakCheckConnector_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:324` |
| `osGbpakGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:227` |
| `osGbpakInit_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:174` |
| `osGbpakPower_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:207` |
| `osGbpakReadId_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:287` |
| `osGbpakReadWrite_recomp` | **implemented** | `crates/fn64-abi/src/gbpak.rs:244` |
| `osLeoDiskInit_recomp` | **implemented** | `crates/fn64-abi/src/pi.rs:1445` |
| `osViFade_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:189` |
| `osViGetCurrentField_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:413` |
| `osViGetCurrentLine_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:399` |
| `osViGetCurrentMode_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:438` |
| `osViGetStatus_recomp` | **implemented** | `crates/fn64-abi/src/vi.rs:423` |
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
