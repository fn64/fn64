# Universal N64 execution plan

Status: active design, 2026-07-17. This is the execution-closure companion to
`DISCOVER-PLAN.md`. Discovery explains what is known about a ROM; this document
defines how fn64 runs every reachable instruction even when historical
function boundaries, overlays, or generated code are not known ahead of time.

## 1. Outcome and boundary

The target is mechanical execution closure for a user-supplied N64 ROM:

- every CPU destination resolves to exact AOT code, bank/basic-block AOT code,
  or a semantics-preserving dynamic MIPS path;
- every RSP task resolves to a proven HLE implementation or general RSP/RDP
  execution;
- CPU, DMA, devices, interrupts, framebuffer, and audio advance on one
  deterministic guest clock; and
- a release report contains zero unsupported execution destinations.

Recovering the original source, names, types, and function partitions is a
separate decompilation-quality goal. It improves readable output and AOT
quality, but it is not allowed to block correct execution. A MIPS CPU does not
call source-language functions; it transfers to virtual addresses in the code
image currently mapped there.

This plan does not weaken fn64's clean-room or no-game-content boundaries.
ROMs, traces, captures, generated code, and derived metadata remain local and
gitignored. Runtime mechanisms come only from the allowed sources in
`AGENTS.md`: fn64's own behavioral evidence, public libultra and hardware
documentation, and the MIT N64Recomp source/output ABI. GPL runtime internals
are not an implementation source.

## 2. Current capability matrix

| Area | Present mechanism | Closure gap | Mechanical gate |
|---|---|---|---|
| CPU decode | Documented MIPS III encodings decode in `fn64-recomp-rs`; ordinary integer, control, memory, delay-slot, and much COP1 behavior exists. | Exceptions, CP0/TLB, interrupt entry, and exact FPU environment are incomplete. Several faulting operations still panic as host failures. | Per-instruction differential plus architectural exception-state tests. |
| CPU dispatch | The function lane remains function-entry based. The working-tree bank lane emits every admitted aligned entry, preserves sparse code/data holes, and returns typed transfer/fault outcomes; a typed outer dispatcher follows direct/resolved transfers under one total instruction budget. `BlockProgram` atomically pairs disjoint bank-bound spans with the generated callable and checks sparse admission before invocation. | The owned program is not connected to the live executor and global function lookup is still bare VRAM. | Enter any aligned interior PC; reject a bounding hole; distinguish two same-VA banks; typed cross-bank exits; integrate without changing ordinary-entry results. |
| Dynamic code | PI DMA activates pre-registered overlays. | No runtime code admission, generation identity, translation, or executable-write invalidation. | Upload/execute/rewrite the same virtual page and prove the new generation runs. |
| Clock/checkpoints | The coroutine executor owns ordered virtual time and explicit yield points. | Long functions and raw polling loops cannot observe device progress or interrupt preemption at instruction/block boundaries. | Cycle-budgeted blocks stop at deterministic deadlines and service a higher-priority wakeup. |
| Devices/MMIO | Useful PI, SI, AI, VI, controller, save, and queue shims exist. A working-tree PI fabric proves raw/shim deadline and completion-trace parity in isolation. | Existing ABI and flat raw-MMIO paths are not routed through that fabric; host audio progress and interrupt state still have different live authorities. | Raw MMIO and shim start the same PI DMA and produce byte-identical cycle-stamped state/event traces in the integrated runtime. |
| RSP | The scalar/vector RSP engine has broad instruction coverage. | Graphics bypasses it; IMEM is not a persistent generated-code image and unknown microcode has no LLE fallback. | A non-GBI synthetic `OSTask` loads boot/task ucode, changes IMEM, reaches BREAK/YIELD, and emits raw RDP commands. |
| RDP/VI | RT64 is the faithful HLE lane; a pure-Rust reference backend provides headless structural tests. | The reference path implements a subset and writes the current VI buffer instead of a general color-image target. Unknown/custom microcode cannot reach a general RDP path. | Raw fill command stream produces exact color-image bytes and SP/DP/MI ordering; VI captures the programmed mode/field. |
| Exploration | Discovery has typed trace/probe inputs and a bounded probe-plan foundation. | No real headless emulator producer, state mutation loop, coverage frontier, or forced-path admission rule is connected. | Digest-bound save state, forced branch, reproducible trace, and explicit natural-versus-forced reachability labels. |
| Validation | Unit/oracle tests, C ABI smoke, lane parity, trace comparator, audio dumps, and live screenshots exist. | No end-to-end deterministic device trace or zero-unsupported full-ROM report exists; lane parity shares renderer blind spots. | Stable framebuffer/audio/memory/timing digests at fixed guest cycles plus a zero-unsupported closure report. |

The evidence behind this matrix lives in `ISA-COVERAGE.md`,
`RSP-ISA-COVERAGE.md`, `DESIGN.md`, `R5-HANDOFF.md`,
`RT64-GAP-REGISTER.md`, and the open items in `ROADMAP.md`. The matrix is a
statement of current mechanisms, not a claim that general execution works.

## 3. Core architecture

### 3.1 Bank-qualified block execution

The universal CPU destination is:

```rust
struct ExecutionKey {
    bank: BankId,
    pc: GuestPc,
}
```

`BankId` identifies immutable executable bytes plus their load lineage or
generation. It is never inferred from virtual address alone. A block ends with
a typed outcome:

```rust
enum BlockExit {
    Transfer(ExecutionKey),
    ResolveTransfer { source_bank: BankId, target_pc: GuestPc },
    HostCall { vram: GuestPc, resume: ExecutionKey },
    Checkpoint(ExecutionKey),
    Yield(ExecutionKey),
    Fault(CpuFault),
}
```

`ResolveTransfer` is the honest intermediate for a computed jump that supplies
only a virtual PC. The active mapping layer must resolve it to one exact bank
before execution continues; generated code may not guess the source bank or a
registration-order winner.

Each turn will return a `BlockRun { exit, instructions }`. The instruction
count is deterministic work accounting, not yet a claim that every VR4300
instruction costs one cycle. U2 maps instruction/device effects onto typed
guest cycles; host wall time never fills that gap.

The existing whole-function lane remains an optimization and oracle. The new
lane admits an executable interval, emits or translates every aligned entry,
starts at the supplied PC, and reports transfers to a dispatcher. Function
metadata may coalesce blocks into faster direct calls only after doing so
cannot change behavior.

### 3.2 Executable generations and invalidation

Every executable bank records:

- byte digest and source lineage (ROM interval, DMA/decompression event, or
  generated-memory generation);
- virtual and physical mapping;
- admitted instruction interval;
- translation generation and dependent blocks; and
- evidence class: exact AOT, block AOT, dynamic MIPS, or unsupported.

Writes to an executable physical page advance its generation before another
block can dispatch through stale code. DMA and CPU stores use the same write
observer. Guest cache operations become validation/synchronization inputs,
not no-ops that conceal stale translations.

### 3.3 Deterministic device fabric

One `DeviceFabric` owns PI, SI, AI, VI, MI, save-device, and relevant SP/DP
state plus a stable `(deadline, sequence)` event heap. Both libultra shims and
raw KSEG1 loads/stores call the same typed device methods. Host audio and pixels
are sinks; callback timing never becomes guest hardware state.

Translated blocks consume a deterministic cycle budget. At block boundaries,
MMIO, or an earlier device deadline, control returns to the dispatcher. It
advances due events atomically: device status, MI pending bits, CPU interrupt
eligibility, and registered OS notification become observable together before
guest execution resumes.

PI read DMA is the first vertical proof because it exercises ROM bytes, RDRAM
layout, busy state, timing, MI, OS queues, overlay/code admission, and LL/SC
invalidation in one bounded path.

### 3.4 General RSP/RDP path

Known HLE selected by an exact microcode digest remains an optimization. Every
other task follows the general path:

```text
OSTask -> boot ucode -> IMEM generations -> RSP execution/DMA
       -> raw RDP command stream -> color/depth images -> VI
```

The RSP engine owns persistent IMEM, DMEM, PC, status, and translation
generation. IMEM DMA installs bytes, invalidates dependent translations, and
resumes at the requested PC. SP completion occurs only at a real BREAK/YIELD;
DP completion occurs only after the command stream reaches its modeled sync.

### 3.5 Mechanical exploration

A headless emulator is an evidence producer, not a hidden runtime dependency.
For a ROM-digest-bound save state, the explorer may mutate registers or memory,
invert bounded branches, and run forward while recording bank/PC, indirect
targets, DMA loads, executable writes, MMIO, interrupts, and task boundaries.

Forced execution proves that a path is executable under the recorded synthetic
state. It does not prove natural reachability. The graph stores those labels
separately and never promotes either into a silent function-boundary claim.

## 4. Milestones and acceptance gates

### U0 — execution identity

- Introduce `BankId`, `GuestPc`, `ExecutionKey`, `CpuFault`, and `BlockExit`.
- Admit immutable code banks independently of historical function extents.
- Resolve overlapping same-VA banks only by explicit bank identity.

Gate: interior resolution, same-VA isolation, cross-bank destination
preservation, and faults naming the exact `(BankId, PC)`.

### U1 — one-bank arbitrary-PC runner

- Emit a dispatch arm for every aligned instruction in one admitted bank.
- Begin at a supplied `ExecutionKey`.
- Convert J/JAL/JR/JALR and fallthrough into local transitions or typed exits.
- Add deterministic instruction-budget checkpoints without changing ordinary
  opcode semantics or splitting a branch/delay pair. Guest-cycle charging is
  U2's device-fabric boundary.

Gate: interior entry; computed transfer within and outside the bank; ordinary
function entry matches the current function lane instruction-for-instruction;
invalid entry is a typed fault, never `unreachable!`.

Working-tree frontier: `dispatch_until_boundary` follows direct and
mapping-resolved transfers without losing bank identity, preserves cumulative
work on resolver faults, checkpoints before a next indivisible unit, and
rejects zero-progress or over-budget runner defects. `BlockPackV1` feeds a
sparse emitter and `CodeCatalog`; the real NWXE gate compiles 197 blocks and
re-resolves all 1,039 admitted words while rejecting a real gap. Generated
registration is now bank-checked and atomic inside `BlockProgram`, with an
emitted helper exercised by the compile/run gate. Live dispatcher ownership
and guest-cycle charging remain open.

### U2 — deterministic PI/device vertical slice

- Introduce typed cycles, MMIO addresses, device events, and interrupt sources.
- Route raw PI registers and `osEPiStartDma` through one device state machine.
- Schedule completion, copy through typed RDRAM access, update PI/MI state,
  invalidate LL/SC and executable generations, then post the OS notification.

Gate: raw and shim starts have identical cycle-stamped traces in C and Rust
lanes; polling, interrupt, and blocking-queue paths all observe one state.

Working-tree frontier: the isolated fabric already proves identical raw/shim
start state, bytes remaining invisible before the deadline, and the exact
completion order `bytes -> PI idle -> MI pending -> notification`. Routing the
existing ABI/raw access paths and executor queue delivery through it remains
open; this isolated result is not yet a live-ROM timing claim.

### U3 — runtime code admission

- Register ROM overlays, decompressed code, and generated code as bank
  generations at runtime.
- Translate on demand and invalidate stale blocks on CPU/DMA writes.
- Record dynamic targets for optional AOT promotion.

Gate: load two images at one VA, execute each, rewrite one executable page,
and prove no stale block can run.

### U4 — CPU architectural closure

- Replace host panics with precise VR4300 faults and exception vectoring.
- Complete CP0, Count/Compare, TLB/address translation, privilege, delay-slot
  BD/EPC state, and interrupt arbitration.
- Make COP1 rounding, flags, enabled exceptions, NaN behavior, and FR modes
  instruction-faithful.

Gate: zero unsupported documented CPU encodings/effects in the executable
corpus plus focused hardware/manual-derived architectural vectors.

### U5 — device closure

- Bring VI, AI, SI/PIF/controllers/accessories, saves, and remaining PI/MI
  behavior into the same fabric.
- Model FIFO depth, status, latency, masks, regions, and event ordering.

Gate: no shim/raw-MMIO split authority; deterministic framebuffer, audio,
memory, interrupt, and timing traces at fixed guest cycles.

### U6 — RSP/RDP closure

- Run boot/task microcode through persistent IMEM generations.
- Connect DP writes to a general command sink; complete image, TMEM/TLUT,
  combiner, blender, depth, coverage, and synchronization behavior needed by
  the corpus.

Gate: unknown/custom microcode takes LLE rather than skip/fake-complete; exact
SP/DP/MI ordering and framebuffer bytes are reproducible.

### U7 — exploration and release closure

- Connect a black-box headless emulator producer to digest-bound trace and
  forced-state probes.
- Iterate unexplored edges and unsupported destinations to a fixed point.
- Emit Decomp Pack and Recompiler Pack separately.

Gate: the Recompiler Pack has zero unsupported CPU/RSP/device destinations for
the tested ROM/input corpus. Exact-source/decomp coverage is reported
separately and may be lower without weakening execution closure.

## 5. Short feedback loops

Each layer gets the cheapest oracle that can falsify it:

1. Decoder/type/unit tests: sub-second target during editing.
2. One-bank generated-code compile/run differential: seconds.
3. Synthetic raw-MMIO/device trace: seconds and headless.
4. Fixed-cycle headless ROM checkpoints: framebuffer, audio, memory, timing.
5. Live shell screenshot/audio inspection: required for claims about the live
   window; nonblank and heartbeat logs are not image evidence.
6. Deterministic changes run clean ten consecutive times before “fixed”;
   concurrency/interleaving changes run at least twenty, per `AGENTS.md`.

Every failure reports the first divergent guest cycle, execution key, device
event, or output sample. Aggregate “coverage improved” numbers are diagnostic;
they are not release gates.

## 6. Corpus strategy

OoT remains the large graded answer key. The AKI family, including locally
generated evidence from released WCW recompilation projects and the existing
WWF work, is valuable because related games vary overlays, indirect targets,
microcode, and engine revisions while retaining comparable structure.

The corpus is used in three roles:

- training titles may supply candidate patterns and known mappings;
- holdout titles measure whether the mechanism generalizes without a
  game-specific descriptor; and
- black-box releases supply behavioral checkpoints and task/frame/audio traces.

No corpus descriptor becomes a runtime requirement. Any title-specific fact
must either be mechanically derived and hash-bound as pack data or remain an
external grading annotation. GPL runtime implementation code remains outside
the clean room.
