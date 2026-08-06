# WM2000 playability blocker — hypothesis ledger

Goal: WM2000 recompile 100% playable through the fn64 pipeline (discovery →
runtime → render). Current state: `gfx_submits=0`; no display list is ever
submitted. This file is the working record for the blocker in front of that.

Rule for this file: a hypothesis leaves the OPEN table only with a measurement
attached. Five hypotheses died twice across sessions because the evidence lived
in transcripts instead of here.

## The blocker

```
unjournaled executable mutation changed physical RDRAM [0x0009b0b3, 0x0009b0b4)
expected=Some(0) live=Some(1)
journal_entries=104420        # identical every run — fully deterministic
```

Reached at step ~1,183,304 after entering a second overlay at
[0x8011c900,0x801226f0). Prior to the baseline fix the route died at 421,717.

## Writes to 0x0009b0b3 — measured, both seams instrumented

| # | Writer | Value | Attributed? |
|---|--------|-------|-------------|
| 1 | `write_logical_bytes [0x400,+0x100000)` (boot publication) | ROM byte `0x10` | yes |
| 2 | `write_u8 [0x9b0b3,+0x1)` (boot publication) | — | yes |
| 3 | `store_backed_word [0x9b0b0,+0x4)` (guest CPU store) | `0x0` | yes — `seq=81661 CpuInstructionStore` |
| 4 | **`mirror_queue_to_rdram [0x9b0b0,+0x4)` ×2** — FOUND | `0x1` | **no — the blocker** |

Writer 3 legitimately explains `expected=0`: the guest zeroed the word, the
store was declared, the baseline advanced.

## Root cause (CONFIRMED by measurement)

`Executor::mirror_queue_to_rdram` (`crates/fn64-runtime/src/executor/mod.rs:666`)
mirrors guest `OSMesgQueue` fields into RDRAM with raw
`std::ptr::copy_nonoverlapping`, bypassing every view type and every
`notify_*_write`. WM2000 has a queue at guest `0x8009b0b0`; a `validCount` of 1
writes native `01 00 00 00` at storage offset `0x9b0b0`, and since storage
offset `o` is logical byte `o^3`, the `01` lands at logical `0x0009b0b3` --
inside a watched executable range.

Proven with a temporary probe: `FN64_WATCH_WRITE=0x9b0b3` printed
`mirror_queue_to_rdram [0x0009b0b0,+0x4) covers 0x0009b0b3` twice.

Note this is a swizzle effect, but in the *writer*, not in the snapshot/baseline
comparison -- the latter remains dead as a hypothesis.

The repair is attribution, not suppression: the mirror is a legitimate host
write that must declare itself on the `HostAbi` channel, exactly as fn64-abi's
sibling scheduler running-thread mirror already does
(`recompiled/execution.rs:698-710`). `fn64-runtime` cannot call the recompiler
crate in production (dev-only, deliberately one-way, `Cargo.toml:18-23`), so the
host installs a callback.

In-tree corroboration written before the cause was known
(`live_program.rs:2049-2052`): *"at least one path reaches RDRAM without passing
through `record_executable_and_renderer_write`."* This is that path.

### Instrumentation coverage (why #4 is invisible)

Two unrelated types are both named `Rdram`, and until this session only one was
watched:

- `fn64_runtime::Rdram` + its views — watched by `watch_raw_write`
  (`crates/fn64-runtime/src/rdram.rs:464`).
- `fn64_recomp_rs::runtime::host::Rdram` (`runtime/host.rs:20`) — what
  recompiled guest code stores through; writes `self.mem[..]` directly. Now
  watched by `watch_guest_store` at `store_backed_word`/`store_h`/`store_b`/
  `store_d`.

Known remaining gaps: `Rdram::as_mut_slice()` (`host.rs:574`, documented
in-tree as "an UNATTRIBUTED write path"; sole non-test caller
`recompiled/runners.rs:1331` hands a raw pointer to a generated C shim),
`RdramViewMut::write_u16` (`rdram.rs:593`), `RdramPtr::write_u32`
(`rdram.rs:512`), `RdramPtr::write_u16` (`rdram.rs:553`),
`fn64_runtime::Rdram::write_bytes` (`rdram.rs:757`), and any DMA path writing
through `ProcessDmaMemory` or a raw pointer.

## Dead hypotheses — do not re-propose without new evidence

| Hypothesis | How it died |
|---|---|
| Byte-lane swizzle mismatch between snapshot and baseline | ROM word at 0x9b0b0 is `a4 45 00 10`; lane-XOR-3 gives `10 00 45 a4`. Neither order yields 0x00 or 0x01. Also killed earlier in `a2d1982`/`ba0af45`. |
| `FN64_FAST_MUTATION_JOURNAL` gate skipping a baseline-advancing read | Flag is unset in these runs. |
| Device-advance empty `RdramViewMut` (site 1) | Fixed in `121a8cf`/`8aaf654`. Note the fix commit's stated mechanism is itself wrong: an empty view cannot silently zero — `RdramView::range` (`rdram.rs:294-299`) asserts. It changed behavior via device-advance timing. |
| Device-advance empty `RdramViewMut` (site 2, `pi/timing.rs:390`) | Patched it to describe the real allocation: broke 13 tests AND left the panic byte-identical. Reverted. That empty view is legitimate. |
| `expected` seeded before publication ("sealed too early") | `FN64_BASELINE_PROBE` in `121a8cf` measured `expected[0x9b0b3]=Some(16)` = 0x10, correct at boot. Superseded. |
| A second RDRAM allocation (`expected` and `live` reading different memory) | `boot_thread0_validated_catalog_generation_program_v1` (`execution.rs:1323-1325`) *moves* `validated.storage` into the process RDRAM. Same buffer. |
| Overlapping watched ranges consulting different range objects | `CanonicalExecutableMutationStateV1::new` (`live_program.rs:44-49`) asserts `physical_start > previous_end`. |
| `covering_declarations=2` in the panic proves the delta was declared | That filter (`live_program.rs:539-550`) scans the ENTIRE journal history; `seq=81661` of 104420 is an old acceptance. Misleading as written. |

## Open

- **H1 — writer #4 is a DMA/device writeback.** A value of exactly `0x01`
  reads like a status/flag byte, not data. PI/SI/SP completion and RSP/HLE
  writeback all reach RDRAM outside both watches.
- **H2 — writer #4 comes through `as_mut_slice()` into a C shim.**
  `FN64_RECOMP_RS_SHIM_TRACE=1` shows only 8 shim calls in the whole run
  (osCreateMesgQueue ×3, __osSiDeviceBusy ×2, osStartThread, osSendMesg,
  osCreateThread), none of which obviously targets 0x8009b0b3 — so this is
  ranked below H1 but not eliminated.

## Reproduce

```
cd examples/wm2000-block-boot
export ROM="$FN64_DISCOVER_NWXE_ROM"
C=~/Code/aki-recomp/captures; G="$C/wm-general-exception-images"
export FN64_EXECUTABLE_IMAGES="$G/run-1/image.json:$G/run-2/image.json:$G/run-3/image.json"
export FN64_BOOT_CONTEXT="$C/wm2000-boot-context.json"
export FN64_ABSENT_N64DD=1 FN64_BLOCK_MAX_STEPS=1300000
export FN64_WATCH_WRITE=0x9b0b3          # add FN64_WATCH_WRITE_BACKTRACE=1 for stacks
./target/release/wm2000-block-boot
```

Deterministic — one pass suffices.
