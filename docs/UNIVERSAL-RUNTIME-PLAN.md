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

That is the hardware-faithful prerequisite, not the whole product boundary.
Full stack parity also includes the behavior publicly advertised by RT64:
modern D3D12/Vulkan/Metal presentation, latency-reduction modes, resolution
scaling/downsampling, widescreen and ultrawide correction, high-frame-rate
interpolation, Extended GBI, DDS/Rice-name texture replacement with
asynchronous streaming, and deferred-frame/debugger integration. These remain
separate typed feature gates so a correct base pixel digest cannot be cited as
proof of an enhancement path that never ran. Features upstream still labels
in development are tracked as an extension watch list rather than silently
inflating or shrinking the current parity denominator.

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
| CPU decode | Documented MIPS III encodings decode in `fn64-recomp-rs`; ordinary integer, control, memory, delay-slot, and much COP1 behavior exists. The arbitrary-PC lane returns typed SYSCALL/BREAK/conditional-trap, signed-integer-overflow, instruction-fetch AdEL, aligned-memory AdEL/AdES, and COP1-unusable faults with exact bank, fault PC, EPC, BD, instruction code, BadVAddr, and Cause.CE where applicable; `BlockProgram::dispatch` applies CP0 Status/Cause/EPC/BadVAddr, resolves the BEV-dependent vector, enters a registered handler bank, and follows ERET through ErrorEPC/ERL or EPC/EXL while clearing LLbit. The live owner samples MI on CPU IP2 and Count/Compare on IP7 at block boundaries; Count advances once per two guest CPU cycles and a Compare write acknowledges the timer latch. | TLB/translation, remaining COP0/COP2 faults, most CP0/privilege behavior, instruction-interior timer visibility, the whole-function lane's exception boundary, and the exact FPU environment remain incomplete; several remaining faulting operations still panic as host failures. | Per-instruction differential plus architectural exception-state and live-checkpoint tests. |
| CPU dispatch | The function lane remains function-entry based. Official native-C builds inject a first-in-body observer into every generated `RECOMP_FUNC`; it maps the entered callable through generated section metadata and retains pointer-free `(section, offset, link VRAM, cycle)` history in exact order. The typed whole-function emitter likewise injects the first statement through its single `emit_function_resolved` body template and retains artifact-bound `(link VRAM, symbol, cycle)` entries; root, direct sibling/tail, and lookup-resolved guest calls enter that template, while host overrides and lookup probes/misses do not. Its ABI install fails closed unless given the observation-schema marker exported by the regenerated artifact, so identity-only or handwritten `RecompFunc` tables cannot claim a complete stream. The committed-VI boundary now freezes and mutation-checks that stream, and report schema v19 rejects empty, future-cycle, artifact-mismatched, or cross-lane entries before serializing exact ordered and canonical unique/count evidence as `typed_observed_function`. The working-tree bank lane emits every admitted aligned entry, preserves sparse code/data holes, and returns typed transfer/fault outcomes; a typed outer dispatcher follows direct/resolved transfers under one total instruction budget. `BlockProgram` atomically pairs disjoint bank-bound spans with the generated callable and checks sparse admission before invocation. That authoritative entry boundary retains an append-only, bank-qualified destination history with the immutable runner-artifact identity when supplied; resolution probes and failed sparse destinations are excluded, and the history is not future-affecting program state. `boot_thread0_block_program` owns it across thread 0/spawned OSThreads and charges checkpoint instructions through executor virtual time after coroutine suspension. Static known-host JALs emit typed HostCall/resume boundaries; dynamic JAL/JALR uses `ResolveCall` to distinguish the installed host table from guest banks, and generated returns require the thread sentinel. The OoT Rust host has an explicit, source-hash-bound pack-selection seam which rejects missing input and prevents whole-function guest fallback. It now hashes the generated source tree with a path-independent canonical wire and boots through `boot_thread0_with_execution_observation`; a stale cached generated crate without the marker fails compilation. | The private generated crate must be regenerated before that host can build and produce a v19 function-lane report. The existing OoT generator also does not emit the required block-pack source, so the alternate host path has no real OoT artifact to install. A third-party native archive that bypasses fn64's generated-source preparation has no universal observable function-entry boundary. | Regenerate the private module, then retain ten v19 `typed_observed_function` runs, or generate the complete OoT block-pack contract. Enter prepared functions through root, resolved, direct, tail, host-override, and failed-lookup paths without recording resolution attempts; retain exact entered-destination order across transfer and host resume. |
| Dynamic code | PI DMA activates pre-registered overlays. `ExecutableRegion` owns one active bank generation. Equal-length physical/virtual registrations observe typed CPU stores, generated-C RDRAM stores, and device DMA writes after commit; the next host boundary snapshots architectural byte order, atomically replaces code+runner, and re-resolves interrupt/checkpoint/host/spawned-thread entries at the same PC. | The current generated runner can execute later instructions after a store until it returns, so instruction-interior invalidation is not yet exact. Regions are not page-granular, no real pack supplies a translation builder, and dynamic targets are not promoted. | Execute generation A, rewrite through both CPU and PI DMA paths, prove A is unreachable before boundary resume/completion visibility, then execute generation B at the identical PC. |
| Clock/checkpoints | The coroutine executor owns ordered virtual time and explicit yield points. The block lane suspends on instruction budgets, commits due PI/MI state before any later resume, and samples interrupts before the next block. | The legacy function lane and host-atomic shims do not preempt internally; exact per-instruction device timing is not claimed. | Cycle-budgeted blocks stop at deterministic deadlines and service a higher-priority wakeup. |
| Devices/MMIO | One deterministic `DeviceFabric` owns typed PI, SI, AI, VI, MI, save, SP, and DP state. Managed calls, raw MMIO, generated-C proxies, and libultra shims converge on it. PI orders bytes/busy/MI/queue delivery; AI owns a timed two-slot FIFO; SI owns persistent PIF RAM and timed two-direction DMA. Raw and high-level EEPROM, Controller Pak, Rumble Pak, and Transfer Pak operations share their authoritative stores and latches. Runtime-configurable 1–62-bank Controller Paks use one physical image and retained bank latch as their sole authority: high-level operations decode and encode per-bank checksum-protected FAT/backup pages plus the sixteen-entry note directory seen by raw Joybus, global 16-bit chains cross bank boundaries, and ambiguous checksum/cycle/share/orphan/directory corruption returns `PFS_ERR_INCONSISTENT`. EEPROM writes defer backing-store mutation to one typed guest-cycle deadline, expose public `0x80` busy state through raw Info/Write, reject overlap, and make high-level polling plus LongWrite's per-block 15 ms timer use that same state. Transfer Pak support includes CRC-checked raw register windows, ROM/RAM persistence, ROM-only and MBC1/2/3/5 cartridge buses, sticky removal/reset state, all six public `osGbpak*` adapters, registration-header and connector validation, and documented deterministic initialization/power waits. Timer-bearing MBC3 cartridges advance on exact guest-second boundaries independently of Pak power, retain immutable latches, honor halt, and implement 9-bit day/sticky-carry rollover through both raw and high-level paths. Their live RTC/phase and exact ROM/type identity persist in a checksummed versioned sidecar; explicitly injected host timestamps materialize powered-off elapsed time once without entering runtime evidence. Voice has a typed initialized/READY/START/CANCEL/BUSY/END lifecycle shared by its nine shims, raw Info, captured result/status, five-write raw initialization, and initialization/clear/start/stop controls. VI owns its live register file, region timing, field/current/intr derivation, vblank-latched mode/scale/features/presentation, current mode/status queries, square-root gamma, coverage-gated median divot, RGBA16 neighborhood dither restoration, and retrace-seeded stochastic seven-bit gamma dither. Persistent RSP DMEM/IMEM, status/PC/semaphore, double-buffered DMA, raw DPC streams, and all six MI sources use the same event ordering. | PI/SI/SP-DMA and HLE SP/DP deadlines remain deterministic policies rather than measured hardware timing. EEPROM uses the public library's conservative 15 ms interval rather than a measured per-chip-revision timing model. Raw Voice still traps with typed evidence for `0x09` without an injected result, region-dependent `0x0A` staging, `0x0D` power/gain writes, and the unestablished `0x0C` dictionary-transfer mode. VI register timing lacks hardware-trace validation and exact random-stream identity remains unproven. A fourteen-phase pinned-Metal gate now supplies exact native post-VI pixels: gamma and nonidentity scale are causal and restorable, while gamma-dither, divot, dither-restoration, and AA-selector output remain named pixel-inert implementation residuals rather than silicon evidence. Function-lane generated code remains atomic between host boundaries. | Hardware-derived timing plus raw/shim/C-proxy byte-identical traces through the integrated executor. |
| RSP | The clean-room scalar/vector interpreter executes the persistent IMEM image from the fabric's PC, imports/commits architectural DMEM and SP status, resolves rectangular IMEM DMA generations, resumes after each overlay, and stops only at BREAK or a loud bounded failure. Unknown/custom task types take this LLE path. Known graphics/audio tasks execute admitted rspboot until control first reaches DMA-loaded ucode, committing RDRAM/DMEM/IMEM/status/entry-PC effects before the HLE backend; transactional LLE fallback also receives a typed snapshot of the complete non-memory machine state. Graphics tasks expose an explicit `HleOptimized`/`LleAccuracy` host policy; the release/parity harness selects accuracy and continues the loaded ucode through that same snapshot and interpreter. Synthetic normal, wrapped-overlay, invalid-boot, yielded, reload/resume, boot-register-continuity, and accuracy-policy raw-DPC gates prove the connection. The OS yield handshake uses the same live SP status: SIG0 requests yield, SIG1 prepares the task's yield buffer for restart, normal completion is read-only, and load clears stale handshake bits. | The selected HLE backend executes atomically, so SIG0 cannot yet preempt/resume an HLE task in flight, and instruction timing is a deterministic count rather than a pipeline model. | Run the synthetic admission/execution/overlay and yield-protocol gates 10 times, then exercise non-GBI full tasks with stable DMEM/RDRAM/DPC traces. |
| RDP/VI | RT64 is the faithful HLE lane; the pure-Rust F3DEX2 decoder emits ordered operations for 8-bit index/RGBA16/RGBA32 targets, fills, syncs, triangles, texture rectangles, and the independent depth-image register. One typed layout classifier is shared by validation, import, fill, copy, and commit. The reference backend imports, switches, commits, and same-address reinterprets all three public color-image layouts; the 8-bit layout stores one byte and ignores hidden coverage, while RGBA32 retains five-bit memory alpha and the three coverage bits in its alpha byte. It executes format-correct fill-cycle rectangles, normal/flipped RGBA16 copy-cycle rectangles, direct I8, packed IA8, and undereferenced CI8 copies into 8-bit targets, and one/two-cycle TEXRECT/TEXRECTFLIP through shared texture filtering, color combining, alpha compare, and framebuffer blending. Eight-bit copy preserves the original TMEM byte while alpha compare uses source-format intensity/alpha. YUV16 Y0/U/Y1/V tile loads, all six signed `G_SETCONVERT` fields, public `CONV`/`FILTCONV`/`FILT` selection, and K4/K5 combiner inputs run through the same triangle/rectangle sampler. `G_SETKEYR`/`G_SETKEYGB`, CENTER/SCALE combiner inputs, and `G_CK_KEY` alpha fixup implement the public soft-edge chroma equation and feed alpha compare. Programming Manual Chapter 13.7 mip/detail/sharpen selection uses immutable eight-tile primitive snapshots, adjacent perspective-corrected coordinate derivatives, modulo-eight tile selection, minimum/maximum LOD, and RGB/alpha LOD_FRACTION inputs across rectangles, high-level triangles, and raw coefficient triangles. A fill directed at the persistent depth image writes its raw halfwords and clears the covered software depth samples across later color-image switches. Bounded `osDpSetNextBuffer`, raw DPC START/END, and LLE RSP DPC submissions execute the proven subset. `CMD_END` captures the submitted words into an immutable command image staged outside guest RDRAM before backend dispatch. Both DRAM command DMA and XBUS DMEM command DMA reach the renderer. All eight raw RDP triangle layouts retain typed edge/shade/texture/Z planes through a coefficient-driven span walker with the public eight-sample checkerboard coverage mask; high-level F3DEX2 triangles now evaluate those same sample positions and use winding-independent top-left ownership for exact shared edges. Full masks retain pixel-center attributes while partial raw/high-level masks use one typed covered sample for shade, texture, and Z under a bounded nearest-to-center/stable-order policy. Set Scissor retains its public field-enable and odd/even selector, and every color/depth/rectangle/raw/high-level raster path rejects the opposite-parity scanlines. RGBA16 coverage and depth DeltaZ share a physical-address hidden-bit sidecar; all four coverage destinations, coverage/alpha selection, clear-on-wrap color writes, memory-coverage blending, and opaque coverage-wrap strict Z execute. `Z_CMP`/`Z_UPD` are independent; primitive depth works for triangles and rectangles; Chapter 15 relations drive opaque/interpenetrating/translucent/decal admission; and ordered RGB MagicSquare/Bayer plus alpha Pattern/InversePattern dither execute before target-format storage. One typed seedable deterministic per-fragment byte feeds combiner NOISE, RGB/alpha Noise, and `G_AC_DITHER`; the unpublished silicon stream remains unclaimed. Unsupported state still fails by name. | Exact LOD derivative norm/fixed-point boundaries, exact fixed-width/subpixel coefficient, conversion, key, and covered-sample selector arithmetic, interpenetration coverage adjustment, exact alpha-coverage rounding, same-visible-value CPU hidden-bit rewrites, filter arithmetic, exact hardware noise-generator identity/advancement, other unmodeled state, and precise timing behavior remain incomplete. | Raw fill/texture/depth-clear/triangle command streams produce deterministic image and coverage bytes through shim, MMIO, and LLE DPC entry paths; SP/DP/MI ordering and VI mode/field are captured separately. The bounded real C-lane path reached its first swap at step 445 after 28 graphics tasks in 20/20 clean runs. |
| Exploration | Discovery has typed trace/probe inputs and a bounded probe-plan foundation. | No real headless emulator producer, state mutation loop, coverage frontier, or forced-path admission rule is connected. | Digest-bound save state, forced branch, reproducible trace, and explicit natural-versus-forced reachability labels. |
| Validation | Unit/oracle tests, C ABI smoke, generated-body inventory/PC-set comparison, trace comparison, audio dumps, and live screenshots exist. Both boot harnesses use guest-quiescence timing. C/rs framebuffer observation matches through 60 swaps but is non-authoritative because the legacy C inventory has 116 callable empty bodies. Prepared native, schema-enabled typed-function, and typed-block lanes retain authoritative reached-destination order. Schema v19 binds those destinations, all five fixed-cycle channels, complete observation geometry, environment, closure, RT64's resolved graphics API and TV standard, nonzero completed workload/present identity, normalized ROM identity/class, decoded TV region, and the ordered ABI-owned `fn64.rsp-rdp-observations.v2` stream. The trusted private-series runner accepts only a policy-revalidated opaque v3 contract, owns exact-ten sequential fresh child launches, and independently recomputes each admitted ROM's normalized header evidence before accepting a report. Admission v6 binds retail/public-homebrew class-specific provenance, a typed build receipt, the exact child/source/recompiled input, and the runner-owned class environment. Matrix v5 derives report-visible coverage; retained v15 matrices credit fixed NTSC/PAL/MPAL only from header/device/renderer agreement, while report-only verification cannot credit ROM class. The private-series path jointly revalidates an opaque capability's v3 contract, exact-ten receipt, retained reports/journals, raw ROM, runner image, and bound inputs; it exact-matches semantic report and ordered run-event identities before retaining a separately hashed v1 class authority. Missing evidence returns an explicit v4 incomplete assessment rather than a smaller denominator. | No representative private lane has yet supplied ten semantically identical schema-v19 reports with ten distinct terminal v3 journals for every required scenario. Windows 10-versus-11 identity, platform case results, blocker closure, and allowed-source public-microcode identities remain absent. A class-specific local provenance string is an admitted attestation, not independent public-homebrew provenance. A self-hashed receipt is not transferable process attestation without an external trusted CI/code-signing root; native-archive identity does not repair the legacy C oracle's missing bodies; reached destinations do not prove unreachable code; native coroutine continuations remain excluded; focused oracles are not exhaustive; and measured parity only reaches swap 60 with a non-authoritative deeper C oracle. | Populate a successor allowed-source public-microcode catalog, add Windows-version/platform-case/blocker evidence, retain external process attestation where required, regenerate the private module or select a generated block pack, and retain ten trusted v19 report/journal pairs per profile scenario. See `RELEASE-GATE.md`. |

In the validation row, “binds” means exact identity co-binding. The typed
receipt does not prove that the child was compiled or linked from its lane
input; trusted build/link provenance remains a separate open evidence class.
The supported receipt materializer derives that co-binding from measured
files, and the OoT function-lane writer publishes the exact source wire
embedded by the child build. Production series additionally stage and
revalidate the admitted microcode text/data at every child boundary; the OoT
host registers those runner-owned bytes with the selected backend before boot.
Likewise, a release microcode family comes only from the selected backend's
exact text/data-pair catalog; text-only HLE recognition is diagnostic only.
Yield lineage is phase-typed: ordinary completion retires it, while a public
yield result authorizes one yielded Load and that Load consumes authorization.

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

Registered executable spans now use one post-commit observer for DMA and CPU
stores. At the next host boundary the live owner rebuilds from architectural
byte order and advances the generation before a suspended checkpoint or device
completion can expose stale ownership. A current generated runner is still an
immutable atomic body between its own boundaries, so a store may be followed
by later instructions in that runner before invalidation runs. Closing that
interior window is part of U3, not a claimed property. Guest cache operations
become validation/synchronization inputs, not no-ops that conceal stale
translations.

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

Known HLE selected by an exact microcode digest remains an optimization under
the explicit `HleOptimized` graphics execution policy. Release/parity evidence
selects `LleAccuracy`, which continues every loaded graphics ucode through the
same transactional rspboot snapshot and interpreter used by fallback; it does
not create a second RSP engine. Every unknown task, and every graphics task
under that accuracy policy, follows the general path:

```text
OSTask -> boot ucode -> IMEM generations -> RSP execution/DMA
       -> raw RDP command stream -> color/depth images -> VI
```

The device fabric owns persistent IMEM, DMEM, PC, status, semaphore, and IMEM
generations. Raw IMEM DMA installs bytes only at its guest-cycle deadline, and
`osSpTaskLoad` admits the task header/rspboot image at the synchronous shim
boundary and clears stale SIG0/SIG1 state. `osSpTaskYield` records the public
SIG0 request; `osSpTaskYielded` observes SIG1 and rewrites an acknowledged task
to restart from its yield buffer without redispatching work. A renderer
`Yielded` result sets that same SIG1 and schedules SP but not DP completion.
Reloading the rewritten task supplies the saved range to the next backend call,
so cooperative HLE resume works even though mid-call preemption does not. Unknown/custom
tasks now execute those images through the clean-room
scalar/vector interpreter. IMEM DMA pauses execution, replaces the persistent
generation, and resumes at the saved architectural PC; BREAK commits DMEM,
RDRAM DMA writes, status, and DPC ranges before guest execution resumes. Scalar
halfword/word DMEM accesses retain all twelve address bits and traverse
big-endian logical bytes even when unaligned or wrapping; the native-word
backing representation is not exposed as an unaligned host integer. DRAM and
XBUS/DMEM DPC ranges both reach the raw renderer seam. Empty `START == END`
initialization emits no range, later `END` writes submit only `[CURRENT, END)`,
and raw triangle dispatch decodes the command's six-bit opcode field rather
than rejecting its two high wire bits as a different opcode. Exact known HLE paths
classify the admitted image before task entry. Boot-overlay tasks execute
rspboot until the first instruction in a DMA-loaded IMEM span and commit its
observable memory/status/entry state; direct images whose aligned boot copy
already covers `ucode_boot == ucode` enter at live IMEM PC zero. Both then
optimize the ucode phase. A typed scheduler protocol now retains one opaque
backend continuation after a committed chunk, checks SIG0 before consuming it,
and validates task/yield-buffer ownership before exactly-once resume. Current
reference and RT64 adapters explicitly remain atomic because neither yet
exports checkpointable internal task state. A
family-changing `G_LOAD_UCODE` does not attempt an impossible mid-HLE
transplant: transactional preflight leaves task-entry state untouched, and the
runtime resumes the complete ucode phase through LLE with the rspboot machine
snapshot, or the untouched live PC-zero state for a direct image, including
scalar/VU and SP/DMA/DPC registers. HLE selection itself is also exact:
neither the Rust reference backend nor RT64 enters F3DEX2 at task entry unless
the complete live IMEM digest was explicitly admitted. RT64's raw-RDP bridge
consumes the LLE-produced DRAM/XBUS command ranges and waits for its exact
render-to-RAM workload. The raw seam carries the current VI output address per
submission, including CPU-only DPC work. Successful HLE and raw submissions
publish typed FullSync evidence; only `Reached` schedules DP, while unresolved
evidence traps. Raw command inspection follows exact triangle widths, and a
CPU/RSP-only FullSync schedules no SP event. SP latency remains an instruction-
count policy and the one-cycle DP offset remains an explicit compatibility
policy rather than measured hardware timing.

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
emitted helper exercised by the compile/run gate. The ABI's live block-program
boot lane now retains that owned program across thread 0 and spawned OSThreads.
`BlockProgram::run` records each successfully admitted guest entry with its
bank-generation identity and supplied runner-artifact identity before entering
the callable. Direct and resolved transfers, same-VA banks, and host resumes
retain stable order; catalog probes, holes, and unknown banks do not enter the
history. Copy and explicit-clear APIs give the release layer an observation
lifetime without adding historical events to the immutable program snapshot.
Each nonzero instruction checkpoint suspends the coroutine first, then advances
executor virtual time and services due devices before rescheduling; a live gate
proves exact 3+2-cycle charging and RDRAM mutation. Static known-host JAL,
dynamic JAL/JALR host-or-guest resolution, and explicit thread-return
boundaries are compiled/live gates. Real-pack boot wiring and runtime code
real-pack translation policy remain open. The live region mechanism observes
registered CPU/generated-C/DMA writes, snapshots their final architectural
bytes at a host boundary, atomically retires generation A, publishes B at the
same PC, and re-resolves interrupt/checkpoint/host/spawned-thread entries into B
without leaving A's runner reachable. It does not yet force a generated runner
to stop immediately after an executable store.

### U2 — deterministic PI/device vertical slice

- Introduce typed cycles, MMIO addresses, device events, and interrupt sources.
- Route raw PI registers and `osEPiStartDma` through one device state machine.
- Schedule completion, copy through typed RDRAM access, update PI/MI state,
  invalidate LL/SC and executable generations, then post the OS notification.

Gate: raw and shim starts have identical cycle-stamped traces in C and Rust
lanes; polling, interrupt, and blocking-queue paths all observe one state.

Working-tree frontier: managed EPI, raw PI APIs, and typed-Rust PI `lw/sw` now
enter the live fabric. Bytes remain invisible before the configurable deadline;
completion mutates the one process RDRAM allocation and proves the order
`bytes -> PI idle -> MI pending -> executor notification` before another
coroutine can resume. The ordering integration tests passed 20 consecutive
clean runs. Instruction checkpoints now commit that same fabric after the
coroutine suspends. Raw MI mask commands and `osSetIntMask` feed the common MI
gate; its level drives CPU IP2, and enabled pending PI enters the precise
BEV-selected handler before the next normal block. A live handler gate records
Cause/EPC/EXL, acknowledges PI, applies ERET, and proves IP2 lowers before the
resumed block; that exact interleaving passed 20 consecutive clean runs.
All six RCP sources now use that pending/mask gate. Typed raw SP status,
VI_CURRENT, AI_STATUS, SI_STATUS, and MI_MODE/DP acknowledgement writes mutate
the same latches used by shims and CPU IP2. Remaining U2 work is hardware-derived
PI timing, cross-context LL/SC invalidation, function-interior preemption, and
precise timing for the other devices; therefore this is not yet a
cycle-accurate live-ROM claim.

### U3 — runtime code admission

- Register ROM overlays, decompressed code, and generated code as bank
  generations at runtime.
- Translate on demand and invalidate stale blocks on CPU/DMA writes.
- Record dynamic targets for optional AOT promotion.

Gate: load two images at one VA, execute each, rewrite one executable page,
and prove no stale block can run.

Working-tree frontier: live registrations require matching physical/virtual
geometry and an already-installed generation, reject overlap, and share one
post-commit CPU/DMA observer. CPU and PI gates prove the old bank is retired
before checkpoint resolution or completion visibility and that the builder
receives canonical guest byte order; both passed 10 consecutive clean runs.
Exact store-interior checkpoints,
page-granular ownership, a real translator-backed builder, dynamic-target
recording, and boot-pack registration remain open.

### U4 — CPU architectural closure

- Replace host panics with precise VR4300 faults and exception vectoring.
- Complete CP0, Count/Compare, TLB/address translation, privilege, delay-slot
  BD/EPC state, and interrupt arbitration.
- Make COP1 rounding, flags, enabled exceptions, NaN behavior, and FR modes
  instruction-faithful.

Gate: zero unsupported documented CPU encodings/effects in the executable
corpus plus focused hardware/manual-derived architectural vectors.

The release journal now observes function-lane CPU traps through one host
callback before panic, including COP0/COP1/TLB/COP2 and mapped-address gaps;
terminal arbitrary-PC dispatch/transfer/fault failures use the same typed
recompiler event. The v2 static registry sweeps `fn64-recomp-rs` production
sources as well as runtime/ABI/audio/render paths. These mechanics make a
reached gap impossible to certify as zero, but they do not close the
architectural behaviors listed below.

Working-tree frontier: explicit `SYSCALL`, `BREAK`, all integer
conditional-trap instructions, and trapping `ADD`/`ADDI`/`SUB` plus their
64-bit `DADD`/`DADDI`/`DSUB` counterparts in emitted bank runners now return
`CpuFaultKind::Exception` rather than panicking. Faults preserve the active
bank, faulting PC, architectural EPC, branch-delay flag, and instruction code;
normal, taken-delay-slot, conditional-taken/not-taken, and overflow paths
passed 10 consecutive compile-and-execute runs. Applying a synchronous fault
now sets Status.EXL, Cause.ExcCode/BD, and EPC with correct nested-EXL
preservation and returns the BEV-selected general exception vector; those
focused transitions also passed 10 consecutive runs. `BlockProgram::dispatch`
now resolves that vector through the active executable mapping, enters an
emitted handler bank, and accounts the faulting instruction plus handler work
under one budget; its compile-and-run gate passed 10 consecutive runs. The
same lane now executes ERET with ErrorEPC/ERL precedence, EPC/EXL fallback,
LLbit clearing, and active-mapping resolution of the return bank. Typed MFC0
reads cover BadVAddr, Count, Compare, Status, Cause, EPC, and ErrorEPC; MTC0
writes cover Count, Compare, Status, Cause's software-pending bits, EPC, and
ErrorEPC, so the compile/run gate now
executes a real read/advance/write EPC handler before ERET. The combined library
and compile/run gate passed 10 consecutive runs. The coroutine runtime now owns
this block program and samples precise MI/IP2 interrupts at instruction-budget
boundaries. The executor now owns Count's half-rate phase, wrap-safe Compare
matching, and the latched IP7 line; live MTC0 Count/Compare writes cross back
to that authority, including same-value Compare acknowledgement before ERET.
The exact checkpoint-match/handler/acknowledge/ERET interleaving passed 20
consecutive clean runs.
Every naturally aligned integer, LL/SC, and COP1 memory operation in the bank
lane now checks its effective address before any register, memory, or
reservation mutation. Misaligned loads return AdEL/ExcCode 4; stores return
AdES/ExcCode 5; both carry BadVAddr, and delay-slot faults preserve branch EPC
plus Cause.BD. The compile-and-run gate covers a normal load fault, a store
fault in a taken delay slot, side-effect suppression, installed-handler entry,
and ERET resumption. Misaligned initial PCs and computed targets now use the
same AdEL path with EPC and BadVAddr equal to the requested fetch address and
BD clear. A computed target checkpoints before its fault when the branch/delay
pair exactly exhausts the budget; the next dispatch counts the fetch attempt,
enters the installed handler, and can ERET to an aligned handler-selected EPC.
Every decoded COP1 family now shares one Status.CU1 guard in the bank emitter.
Disabled COP1 raises ExcCode 11 with Cause.CE=1 before a register, FPU state,
memory access, branch delay instruction, or address-alignment check can occur.
The compile/run gate covers straight execution, a COP1 branch, COP1 in an
integer delay slot, CU1-enabled execution, CU1-versus-AdEL priority, installed
handler entry, and ERET. Remaining COP0/COP2, TLB/translation,
instruction-interior timing, whole-function-lane, and floating-point
exceptions remain open.

### U5 — device closure

- Bring VI, AI, SI/PIF/controllers/accessories, saves, and remaining PI/MI
  behavior into the same fabric.
- Model FIFO depth, status, latency, masks, regions, and event ordering.

Gate: no shim/raw-MMIO split authority; deterministic framebuffer, audio,
memory, interrupt, and timing traces at fixed guest cycles.

Working-tree frontier: AI shim and typed-Rust raw submissions now share a
two-entry current/next FIFO. The fabric computes completion deadlines from
buffered stereo frames, the 93.75 MHz CPU clock, and the quantized DAC rate;
`AI_LEN` decreases with guest time, FIFO-full submissions fail, completion
raises MI AI, and OS_EVENT_AI is posted only after that state is visible. The
public `rcp.h` command semantics also converge SP/SI/VI/AI/DP acknowledgement
on the common MI source. That RCP/MI authority exists from host-state creation,
independent of cartridge ROM installation; PI separately retains a loud
missing-ROM gate. The typed IPL standard selects NTSC/PAL/MPAL VI and AI
clocks and now crosses `RenderConfig` into native RT64 workload-rate inference,
so stable VI factors derive from 60/50/60 Hz without changing an Extended-GBI
refresh override. Ten fresh Metal processes observe exact PAL/MPAL completed-
workload sequences `[0,0,0,50]` and `[0,0,0,60]`; release evidence still must
co-bind decoded TV authority to the renderer configuration. Raw generated-C
writes remain open. VI now stores the complete raw register block, samples `VI_CURRENT` from
the programmed `VI_V_SYNC`, schedules MI at `VI_INTR`, decodes the public
`OSViMode` layout, and latches pending mode/framebuffer state before either
VI-manager message path and renderer presentation. Progressive and serrated
interlaced output expose the documented even/odd `VI_CURRENT` sequences and
the adjacent current-line/field/status/mode shims. Both public `OSViMode`
field-register images now alternate with live field parity, including
framebuffer-relative origin offsets. The four public
special-feature ON/OFF pairs now mutate the queued VI control image, including
the dither-filter bit 16. That latched status crosses the typed presentation
boundary: the Rust lane implements square-root gamma, partial-coverage
horizontal-median divot correction, and RGBA16 3x3-neighbor dither restoration,
while the RT64 lane receives the same status bits. A fourteen-phase pinned-Metal
pixel gate now observes nondefault 8x6 active geometry over one workload: gamma
and 1.5x scale change exact pixels, six disabled phases restore the baseline,
and every present identity advances. Gamma dither, divot, RGBA16 dither
restoration, and all four AA selectors remain exact pixel-inert native
residuals, matching the pinned shader's sampling/border/gamma-only
implementation boundary rather than a silicon claim. Gamma dither uses a
deterministic, retrace-seeded stochastic seven-bit quantizer; the unpublished
silicon random stream is not claimed exact. Black, public 10-bit fade, and first-line
repeat transitions now independently trigger V-blank presentation through
typed VI state; the Rust reference applies them without erasing its RDP source
and restores that source when disabled. Fade/repeat are exported beyond the
canonical NMR inventory. The RT64 path maps the same controls to VI pixel type,
vertical scale, and vertical subpixel offset through its quarantined C boundary;
physical-console filter traces and native implementation of the named residual
stages remain open.
SI schedules separate 64-byte DRAM-to-PIF and
PIF-to-DRAM transfers against
persistent PIF RAM; typed raw registers, the raw SI shim, and controller starts
share BUSY/error/status, MI, and post-commit OS event ordering. The current
one-cycle SI policy still needs hardware-derived timing. External channel 4
now executes EEPROM probe/read/write packets against the same 512-byte or
2048-byte save store and typed programming deadline as `osEeprom*`; tests
prove raw-to-shim and shim-to-raw byte convergence, device identity/no-response
behavior, before/at-deadline visibility, public Info/Write `0x80` busy
responses, overlap rejection, 4-Kbit address wrapping, and loud malformed-
packet rejection. Single high-level writes return while the device remains
busy; reads and later writes poll the same state, and LongWrite advances one
documented 15 ms CPU-timer interval per block. Controller Pak/Rumble Pak
commands validate the public
five-bit address CRC, return the 32-byte data CRC, expose the 0x8000 probe,
and mutate the same typed motor/data-page state as `osMotor*` and `osPfs*`.
Transfer Pak commands now expose typed power/probe, status/mode, four-bank
Game Boy bus, cartridge RAM, and ROM-only/MBC1/MBC2/MBC3/MBC5 mapper state
through that same CRC-checked raw path. The adjacent public `osGbpak*` family
shares it for aligned I/O, sticky removal status, registration-header and
connector checks, and deterministic documented initialization/power waits.
Timer-bearing MBC3 cartridges advance from the executor's guest cycles even
while Pak power is off; exact second boundaries, immutable relatching, halt
and fractional resume, 9-bit day rollover, sticky carry, timerless MBC3, and
raw/high-level convergence are covered. Pan Docs' public
[MBC3 hardware description](https://gbdev.io/pandocs/MBC3.html) is the source
for those register and counter semantics and establishes that an external
oscillator plus battery can keep the RTC running while the Game Boy is off.
fn64's host policy persists the live RTC, guest-cycle subsecond phase, exact
ROM SHA-256/type identity, and a caller-supplied Unix-nanosecond checkpoint in
a fixed-length, checksummed v1 sidecar. Import takes a second explicit host
sample, rejects rollback, identity mismatch, corruption, invalid values, and
unknown versions, then materializes elapsed time exactly once. Fractional host
time is rounded down by less than one guest cycle per restore. The timestamp
is discarded after import and excluded from fixed-cycle evidence; the
resulting RTC/phase remains evidence because it is future guest-visible state.
Export first advances to the caller's exact guest cycle and never samples host
time itself. Mapper banks, enable/latch state, the latched copy, Pak state, and
guest `now` are not battery payload. A missing sidecar alone means fresh RTC;
invalid sidecars never silently migrate to fresh state.
Cartridge removal changes status and powered data access traps rather than
fabricating a cartridge. Controller Paks are runtime-configurable from 1–62
32-KiB banks. Their retained bank latch maps Joybus's lower address half to
one physical bank, upper-half writes select a bank, and upper-half reads
return zero. The published low-bit mirror is modeled for power-of-two bank
counts; nonuniform selects and nonexistent-bank selects on odd capacities are
an explicit public-evidence frontier. Each bank contributes one primary and backup FAT page in bank
zero; the directory follows those tables, the checksum slot reserves the
first page of every later bank, and global 16-bit chains cross bank boundaries
without a shadow filesystem. `osPfsInitPak` repairs each checksum-invalid FAT
page only from its unambiguously valid corresponding copy; valid-but-different
copies and malformed, cyclic, shared, orphaned, or reserved-boundary chains
return `PFS_ERR_INCONSISTENT`. This layout comes from the public libultra
Programming Manual Chapter 26.3, the public
[N64brew Controller Pak hardware map](https://n64brew.dev/wiki/Controller_Pak),
and the public
[N64brew Controller Pak filesystem description](https://n64brew.dev/wiki/Controller_Pak/Filesystem?oldid=5639).
The EEPROM deadline is a conservative public-library compatibility policy
rather than measured chip-revision timing. Raw Voice Info, result/status,
captured five-write initialization, and initialization/clear/start/stop forms
share high-level state. No-result `0x09`, region-dependent `0x0A` staging,
`0x0D` power/gain writes, and the remaining `0x0C` dictionary-transfer mode
trap with typed evidence at the command-specific public capture frontier. HLE ucode work
is still host-atomic, but
SP and graphics DP completion occur as separate fabric events after measured
rspboot work and on successive deadlines, preserving SP-before-DP MI and message order. Persistent RSP
DMEM/IMEM, PC/status/semaphore, double-buffered aligned DMA, and task/rspboot
admission and the SIG0/SIG1 yield/header handshake now share this fabric; the
SP DMA setup/beat latency is an explicit deterministic policy. Unknown/custom tasks execute from that state through
BREAK, including IMEM overlay replacement and DRAM/XBUS DPC forwarding. Known
HLE tasks execute rspboot through the first DMA-loaded ucode entry and commit
its observable state before backend dispatch. All renderer entry paths use one
loud missing/error gate. Task-entry LLE fallback preserves the rspboot machine
snapshot. Exact DP timing, production-backend chunk checkpoints, hardware
validation of VI timing, and save-device timing remain open.

EEPROM timing and high-level waits come from the public libultra Programming
Manual's [Chapter 26.6, EEPROM](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro26/26-06.html)
and the [`osEepromWrite`](https://ultra64.ca/files/documentation/online-manuals/man/n64man/os/osEepromWrite.html)
and [`osEepromLongWrite`](https://ultra64.ca/files/documentation/online-manuals/man/n64man/os/osEepromLongWrite.html)
function pages. Raw identifiers, upper-bit address behavior, deferred commit,
and `0x80` busy replies come from the public hardware
[Joybus Protocol](https://n64brew.dev/wiki/Joybus_Protocol). No runtime
implementation was used as a source.

### U6 — RSP/RDP closure

- Run every non-exact/custom boot/task microcode through persistent IMEM
  generations. (Synthetic overlay/resume and BREAK gates are present; full-ROM
  corpus evidence remains.)
- Connect DP writes to a general command sink; complete image, TMEM/TLUT,
  combiner, blender, depth, coverage, and synchronization behavior needed by
  the corpus.

Gate: unknown/custom microcode takes LLE rather than skip/fake-complete; exact
SP/DP/MI ordering and framebuffer bytes are reproducible.

The HLE graphics seam now receives the device fabric's typed persistent
`RspMemory` directly. F3DEX2 `G_DMA_IO` executes its public debug READ
(DRAM -> I/DMEM) and WRITE (I/DMEM -> DRAM) directions in command order with
the documented 64-bit alignment and bank bounds. The same-task rewrite test
proves a WRITE to the next display-list word changes what the decoder executes;
the ABI seam test proves backend mutations remain visible through CPU/LLE RSP
memory access after task return. F3DEX2 `G_LOAD_UCODE` now consumes the public
compound wire form, copies the declared data prefix and fixed 4 KiB text image
into those live banks in command order, advances the IMEM generation, and
retains exactly the state enumerated by the public F3DEX2 release notes. A
same-stream DMA readback proves ordering and a nested-list test proves the
maintained display-list stack still returns to its caller. The reference
backend content-addresses every changed IMEM image: only the current admitted
task image or an explicitly registered F3DEX2-compatible SHA-256 continues in
HLE. Preflight executes against cloned RDRAM/RSP state. An unknown digest
discards that clone and returns `NeedsLle`; the runtime replays the complete
ucode phase from untouched post-rspboot state through the general interpreter.
Focused gates prove DMEM/PC/BREAK commit and LLE-produced DPC forwarding.
Public no-ops are intentional handlers;
reserved and unknown commands no longer enter a rate-limited silent-skip path.
The same fail-closed rule now covers display-list truncation/cycles/call-stack
overflow, malformed other-mode fields and vertex-cache ranges, and short
vertex, matrix, viewport, or light DMA; none returns a partial render or keeps
stale state. Transformed `G_VTX` also requires an explicitly loaded viewport;
the former host-sized 320×240 mapping is gone.
The pure-Rust backend now retains one device-owned RDP decode state across
both admitted HLE tasks and raw DPC submissions: other mode, combiner,
key/convert/constants, fill/scissor, texture-image/tile/TLUT registers, and
the physical TMEM image persist while RSP-owned `G_TEXTURE` selection resets
per task. A cross-task fill gate proves register persistence, a cross-task
texture gate proves TMEM reuse, and missing live TMEM traps rather than
becoming an implicit white texel. Production F3DEX2/raw color operations also
require that persistent `G_SETCIMG` state; VI/`output_addr` state is accepted
only by the fixture-only simple decoder and cannot silently become an RDP
target. The selected image is re-imported from RDRAM at production task entry,
so persistence does not hide CPU/device writes behind a stale host surface.

Working-tree frontier: F3DEX2 decode now preserves triangles, color-image
changes, fill rectangles, and full-sync boundaries in one ordered operation
stream. `G_MODIFYVTX` now updates all four public post-transform cache fields
(RGBA, ST, screen XY, and screen Z) with their documented fixed-point
formats, closing a known RT64 hard stop without importing its implementation.
Transformed vertices retain all six homogeneous clip-plane codes;
`G_CULLDL` ends only the current display list when its inclusive cache range
shares an outside plane. The public two-command `G_RDPHALF_1`/`G_BRANCH_Z`
form performs an exact unsigned-16.16 screen-depth comparison and tail branch,
and `G_POPMTX` now consumes its full `w1 / 64` count. Both primary and copied
`G_MW_LIGHTCOL` destinations update directional or ambient RGB without
disturbing light direction. Signed `G_MW_FOG` factors generate clamped vertex
shade alpha from projected depth when `G_FOG` is active; the existing blender
then consumes the interpolated alpha. Exact microcode fixed-point rounding is
a hardware-trace frontier. `G_LINE3D` now emits a typed line with the public
1.5-plus-half-pixel width, six-plane homogeneous clipping, flat/smooth shade,
perspective texture attributes, scissor/coverage/blender execution, and
read-only depth. Exact microcode line-edge coefficients remain a hardware-trace
item. The two public `gSPLookAt` DMAs now retain signed screen-space X/Y
directions, and `G_TEXTURE_GEN`/`G_TEXTURE_GEN_LINEAR` replace explicit vertex
coordinates with the manual's signed-projection or inverse-cosine mapping and
`gSPTexture` scale. Missing lighting/look-at prerequisites trap by name; exact
microcode trigonometric lookup and fixed-point rounding remain hardware-trace
items. Public F3DEX2 `gSPForceMatrix` now stages and activates one complete
concatenated transform without mutating the matrix stacks; the next ordinary
matrix operation supersedes it, and modelview-only loads use identity
projection. `gSPPerspNormalize` retains its public `.16` value across ucode
reloads; nonzero scales cancel in the float divide and explicit zero rejects
geometry, while exact limited-divider precision remains hardware-trace work.
All four public `gSPClipRatio` writes are typed, reset on ucode reload, expand
per-side line clip planes, and remain independent from `G_CULLDL`.
Exact clipped-triangle subdivision/rounding, non-public move subindices, and
unknown opcodes remain loud or hardware-trace frontiers.
The reference executor uses one typed classifier to load, switch, commit, and
same-address reinterpret the public 8-bit index/intensity, RGBA16, and RGBA32
targets at FullSync/final completion. The 8-bit layout uses one logical byte
per pixel, ignores hidden coverage, consumes all four fill-register bytes, and
accepts direct I8, packed IA8, and undereferenced CI8 copy-cycle sources. Copy
retains the original TMEM byte while threshold comparison consumes I8
intensity, IA8's expanded low alpha nibble, or the CI8 index. It applies the public
fill-cycle inclusive lower-right rule with exclusive scissor clipping,
alternates both RGBA16 fill-color halfwords, and consumes a whole fill word
per RGBA32 pixel while preserving memory alpha and coverage packing. Set
Scissor's field-enable and odd/even selector
are retained and applied uniformly to color fills, depth fills, copy/combined
rectangles, raw triangles, and high-level triangles. An end-to-end task gate verifies two ordered
fills in the commanded RDRAM image. The same stream now preserves the complete
16-byte texture-rectangle state and executes the public non-flipped RGBA16
copy path: inclusive bounds, S10.5 origins, S5.10 gradients, `4<<10` horizontal
copy stepping, alpha compare, and per-tile texture identity. One/two-cycle
TEXRECT and TEXRECTFLIP use exclusive bounds, S5.10 per-pixel stepping, the
shared point/four-sample-average/documented three-nearest texture filter,
sequential color-combiner cycles, alpha compare, framebuffer blending, and a
distinct TEXEL1 sample. Chapter 13.7 LOD/detail/sharpen tile selection and
LOD_FRACTION input share the triangle sampler. Unsupported target formats,
invalid copy gradients/scissors, shade-dependent rectangle programs, and
pixel-Z rectangle depth requests return named errors. Threshold alpha compare
is exact at the documented blend-color boundary; `G_AC_DITHER` now compares
against the same typed per-fragment noise byte used by the combiner and dither
selectors, not an ordered Bayer approximation. One/two-cycle ordered RGB
MagicSquare/Bayer and alpha Pattern/InversePattern dither now execute before
target-format storage. RGB Noise, alpha Noise, and `G_AC_DITHER` use a seedable
deterministic SplitMix64 reference policy; the unpublished silicon sequence
remains a hardware-trace frontier. `G_SETPRIMDEPTH` plus
`G_ZS_PRIM` now drive raw triangles and combined texture rectangles through
the shared persistent compressed-Z path. Copy-cycle TEXRECTFLIP now
swaps the S/T screen axes with copy-mode gradient normalization.
The shared triangle/rectangle sampler now retains every public `G_SETTILE`
clamp, mirror, mask, shift, TMEM-base, and line field and applies the documented
right/left shift, implicit clamp, wrap, mirror, and mirror-then-clamp coordinate
sequences. A physical 4 KiB TMEM snapshot backs cross-load/render-tile and
masked addressing, odd-row exchange, RGBA32 and YUV split-half storage, and
quadricated RGBA16/IA16 TLUT lookup. Uninitialized bits trap by physical
address. Equal low/high fractional load bounds retain subtexel origins, and
source-sized transfers accept distinct load-descriptor sizes, including the
public RGBA32-through-16-bit form. Unequal low/high fractional edge selection
remains open. Bounded raw
RDP state/fill/texture ranges execute through both `osDpSetNextBuffer` and raw
DPC START/END. Writing `CMD_END` captures the exact submitted words; renderer
dispatch uses an immutable staged copy at the physical 8 MiB boundary outside
guest RDRAM. Task and raw-DPC renderer entries expose exactly that complete
physical device while excluding the generated-code allocation's appended
MMIO/non-RDRAM backing, so later guest stores or RSP DMA cannot mutate queued
commands and transactional decode never clones host-only windows.
The real C-lane boot reached its first swap at executor step 445 after 28
graphics tasks in 20/20 independent clean runs. All eight raw triangle layouts
from SGI *RDP Command Summary* Tables 11-15 are width-checked and decode signed edge/shade/texture/Z
coefficients. Edge-only, shade, Z, loaded-texture, and combined `0x0f` streams
rasterize; the combined gate covers the complete 176-byte layout, and XBUS
DMEM submission covers a variable-width Z triangle. The focused raw-render/
runtime set passed 10 consecutive clean runs. Unmodeled-state opcodes return
named errors. Raw triangles now remain coefficient-bearing render operations:
the pixel-center walker obeys right/left-major span selection and evaluates
shade/texture/Z with `d/de + d/dx`. `Z_CMP` and `Z_UPD` are modeled
independently, and the Programming Manual Chapter 16 compressed-Z/DeltaZ codec
has exhaustive boundary and canonicalization tests. `Z_UPD` now stores the
visible compressed halfword and physical-address-owned hidden DeltaZ bits;
image selection reloads both across task and target switches. Exact
Farther/Nearer/In-Front relations now select opaque, interpenetrating,
translucent, and decal writes, and the stored DeltaZ exponent is the documented
most-significant-bit index. Raw triangles, high-level triangles, and lines now
retain the public eight-sample checkerboard identity in one typed mask until
the fragment boundary, with exhaustive edge/scissor sweeps; raw edge and
attribute planes are evaluated as signed fixed-point values from Table 12's
documented reference points. RGBA16 coverage persists across its visible LSB
and the same physical hidden-bit sidecar; all four coverage destinations,
coverage/alpha selection, clear-on-wrap color writes, memory-coverage
blending, and opaque-wrap strict Z execute. High-level F3DEX2 triangles use
the same eight sample positions. Full masks use pixel-center attributes;
partial raw/high-level masks share one typed covered sample for shade, texture,
and Z under a bounded nearest-to-center/stable-order policy. That policy has an
explicit total preference order and exhaustive nonzero-mask/equal-distance-tie
tests; raw edge comparisons cover one Q16 LSB below/on/above every checkerboard
X boundary. Silicon-internal
accumulator width/truncation and exact representative/correction behavior,
interpenetration coverage adjustment, exact alpha-coverage tie rounding,
same-visible-value CPU hidden-bit rewrites, arbitrary depth-fill hidden bits,
and SP/DP/MI timing remain open. All three legal public color-image memory
layouts are modeled; RT64-native assertions for other format/size combinations
do not describe additional legal RDP render-target layouts.

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
