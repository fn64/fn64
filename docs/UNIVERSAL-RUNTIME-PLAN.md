# Universal N64 execution plan

Status: active design, updated 2026-07-24. The current integration checkpoint
and exact resume sequence are recorded in `RUNTIME-RENDER-HANDOFF.md`. This is
the execution-closure companion to
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
| CPU decode | Documented MIPS III encodings decode in `fn64-recomp-rs`; ordinary integer, control, memory, delay-slot, and much COP1 behavior exists. The arbitrary-PC generated and interpreted lanes share modeled 32-bit COP0 moves plus indexed/random TLB write/read/probe state. The public manual supplies Random's inclusive 31-through-Wired range and Wired-write reset to 31; fn64's bounded, non-silicon clock policy advances once per charged block-runner instruction unit, including its existing second unit for an annulled likely slot, and TLBWR samples before its own unit advances. Canonical 32-bit KUSEG/KSSEG/KSEG3 data accesses use the recorded entries: legal PageMask sizes select EntryLo0/1, PFN plus page offset produces the physical address, and ASID/global/V/store-D checks return typed refill, invalid, or modified faults before memory changes. KSEG0/KSEG1 remain direct and multiple matches trap. Status.KSU plus UX/SX/KX now classify the documented 64-bit XUSEG/XKSSEG/XKUSEG, XSSEG, XKPHYS, and XKSEG data ranges. Mapped spaces compare EntryHi.Region plus VA[39:13], XKPHYS validates VA[58:32] before direct PA[31:0], and width/privilege violations return typed AdEL/AdES. Canonical 32-bit instruction fetch retains virtual `ExecutionKey.pc` for branch/link/EPC/BD while admitting the selected physical word as `InstructionWordIdentity { BankId, PA }`; AOT and interpreter execute the same one-word or independently fetched branch/delay unit, including nonadjacent cross-page PFNs. Fault entry retains exact bank/fault-PC/EPC/BD/BadVAddr, updates Context, XContext, and EntryHi while preserving their software-owned bases and ASID, and selects the first-level 32-bit refill, XTLB refill, or common/nested vector. The legacy whole-function lane keeps Random/TLBWR and mapped-memory/fetch exception return loud because its callable ABI has no instruction clock or typed transfer. The arbitrary-PC lane also returns typed SYSCALL/BREAK/conditional-trap, signed-integer-overflow, instruction-fetch AdEL, aligned-memory AdEL/AdES, and COP1-unusable faults; COP1 arithmetic, comparison, conversion, and CTC1 faults return precise FloatingPoint/ExcCode 15 before destination commit. `BlockProgram::dispatch` applies CP0 state, enters a registered handler bank, and follows ERET through ErrorEPC/ERL or EPC/EXL while clearing LLbit. The live owner samples MI on CPU IP2 and Count/Compare on IP7 at block boundaries; Count advances once per two guest CPU cycles, its retained phase makes interior MFC0 reads exact at charged-instruction granularity, and a Compare write acknowledges the timer latch. | Silicon-exact Random and FPU pipeline timing, undocumented FPU payload/exception-priority quirks, 64-bit instruction-PC/catalog identity, generated-lane translated physical device routing, remaining COP0/COP2 faults, instruction-interior interrupt/Compare sampling, and the whole-function lane's exception-return/instruction-clock boundary remain incomplete; several remaining faulting operations still panic as host failures. | Per-instruction AOT/interpreter differential plus architectural exception-state and live-checkpoint tests. |
| CPU dispatch | The function lane remains function-entry based. Official native-C builds inject a first-in-body observer into every generated `RECOMP_FUNC`; it maps the entered callable through generated section metadata and retains pointer-free `(section, offset, link VRAM, cycle)` history in exact order. The typed whole-function emitter likewise injects the first statement through its single `emit_function_resolved` body template and retains artifact-bound `(link VRAM, symbol, cycle)` entries; root, direct sibling/tail, and lookup-resolved guest calls enter that template, while host overrides and lookup probes/misses do not. Its ABI install fails closed unless given the observation-schema marker exported by the regenerated artifact, so identity-only or handwritten `RecompFunc` tables cannot claim a complete stream. The committed-VI boundary now freezes and mutation-checks that stream, and report schema v29 rejects empty, future-cycle, artifact-mismatched, or cross-lane entries before serializing exact ordered and canonical unique/count evidence as `typed_observed_function`. The working-tree bank lane emits every admitted aligned entry, preserves sparse code/data holes, and returns typed transfer/fault outcomes; a typed outer dispatcher follows direct/resolved transfers under one total instruction budget. `BlockProgram` atomically pairs disjoint bank-bound spans with the generated callable and checks sparse admission before invocation. That authoritative entry boundary retains an append-only, bank-qualified destination history with the immutable runner-artifact identity when supplied; resolution probes and failed sparse destinations are excluded, and the history is not future-affecting program state. `boot_thread0_block_program` owns it across thread 0/spawned OSThreads and charges checkpoint instructions through executor virtual time after coroutine suspension. Static known-host JALs emit typed HostCall/resume boundaries; dynamic JAL/JALR uses `ResolveCall` to distinguish the installed host table from guest banks, and generated returns require the thread sentinel. The OoT Rust host has an explicit, source-hash-bound pack-selection seam which rejects missing input and prevents whole-function guest fallback. It now hashes the generated source tree with a path-independent canonical wire and boots through `boot_thread0_with_execution_observation`; a stale cached generated crate without the marker fails compilation. | The private generated crate must be regenerated before that host can build and produce a v29 function-lane report. The existing OoT generator also does not emit the required block-pack source, so the alternate host path has no real OoT artifact to install. A third-party native archive that bypasses fn64's generated-source preparation has no universal observable function-entry boundary. | Regenerate the private module, then retain ten v29 `typed_observed_function` runs, or generate the complete OoT block-pack contract. Enter prepared functions through root, resolved, direct, tail, host-override, and failed-lookup paths without recording resolution attempts; retain exact entered-destination order across transfer and host resume. |
| Dynamic code | PI DMA activates pre-registered overlays. `ExecutableRegion` owns one active bank generation. Equal-length physical/virtual registrations in the arbitrary-PC block lane observe typed CPU stores and device DMA writes after commit. The ordinary notification observer remains unchanged; a second typed `GuestWriteBoundaryObserver` returns `ExecutableChanged` only for a proven overlap with an active executable region. Generated AOT and interpreted runners then stop after one straight instruction or one complete branch/delay pair. Typed executable-write exits preserve the source bank and defer call/fault continuation resolution until the host has snapshotted architectural byte order and atomically published the replacement code+runner generation. Non-overlapping stores continue normally, while a live stale-sentinel gate prevents generation A from executing after its overlapping store. The opt-in `dynamic-mapped-runtime` lane now installs an execution-local `DynamicMappedUnitCatalogV1` beside `CanonicalLiveBlockProgramV1`. One unified inner slice preserves a single total instruction budget across static misses, exact-unit execution, executable writes, and later static continuations; it samples interrupts only at the outer architectural checkpoint. Host bindings win first, digest-selected precompiled generations next, immutable static code next, and dynamic live fetch last. Every dynamic unit re-snapshots one straight word or a complete independently translated branch/delay pair, uses an implementation-issued source receipt in its full content identity, rejects static/precompiled bank collisions, and routes canonical MMIO through the installed `Rdram` hooks. JAL/JALR always crosses `ResolveCall`, including an in-bank target, so the unified owner applies host-first precedence; JR/JALR retains complete indirect-transfer observations and the thread-return sentinel. Bounded telemetry binds the resolver/program, dynamic source, optional ROM/bootstrap receipt, mutation journal, exact fetched identities, charged work, failures, and explicit saturation counters; the identity catalog also fails loudly at a configurable capacity rather than growing host memory without limit. The validated-owned dynamic boot retains exact ROM/bootstrap/mutation provenance while refusing every static writer/release authority. Focused and public-boot gates cover aliases, cross-page slots, A→B→A reuse, static→dynamic→static without replay, one-instruction remainder checkpointing before admission, host/precompiled precedence, prior-work fetch-fault accounting, delay-slot refetch, ambiguity, static-authority rejection, and static miss→dynamic call→suspended host→externally committed byte visibility→static resume without replay. A separate generation-backed gate drives a real raw-MMIO DeviceFabric PI DMA during that suspended dynamic host and proves exact `HostAbi → PiDma → HostAbi` mutation-journal ordering and digest chaining. Corrected in-bank call/observation regressions passed 10/10 guarded runs; the two public dynamic-boot/provenance tests passed 10/10; and the real-PI dynamic ordering gate passed 10/10 on 2026-07-31. The WM `dynamic-withheld` harness and paired runner bind independently-featured binaries to one unchanged ROM/BootContext/capture set while retaining the same complete static catalog plus canonical program and resolver-install identities. `FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY=1` selects only the installed entry `ExecutionKey`; the operational dispatcher redirects it once, clears the guard only after positive dynamic work, then restores normal static budgets and executable-mutation reconciliation. V2 telemetry proves that individual attempt with positive `charged_instructions` and zero `unsupported_exits`; aggregate totals cannot prove the entry. Both lanes stop at the same global charged-instruction horizon and compare full logical RDRAM plus canonical device/executor/ABI-host and per-thread CPU/continuation digests. The exact-entry pair/comparator contracts passed 10/10 on 2026-07-31. | One exact-entry real-ROM v3 diagnostic reached 100,001 instructions in both lanes and dynamically executed the selected entry once for one instruction with zero unsupported exits. RDRAM, CPU, device, executor, ABI-host, continuation, scheduler steps, simulation time, and the per-thread publication diagnostic matched. This one run is not ten-run parity evidence. The operational digest is deterministic comparison evidence, not an atomic savestate: opaque host/parked native continuations remain non-comparable, and canonical full-machine serialization, interpreter totality, and a held-out ROM remain open. Regions are not page-granular, automatic executable PI/decompression classification is absent, and function-lane/timer instruction-interior timing remain separate boundaries. | Repeat the frozen exact-entry comparison before promoting the one-run operational result to deterministic parity evidence. |
| Clock/checkpoints | The coroutine executor owns ordered virtual time and explicit yield points. The block lane suspends on instruction budgets, commits due PI/MI state before any later resume, and samples interrupts before the next block. `RecompContextEvidenceSnapshotV1` projects every future-affecting CPU field owned by one context: integer/FPU/LL state, pending Count/Compare writes, modeled COP0/TLB state, interrupt mask, and return sentinel. The canonical owner counts charged work globally across OSThreads and publishes each thread's latest exact pre-yield checkpoint, opaque host-in-flight marker, post-exception parked-fault marker, or returned CPU state. Exact publications bind their nonzero last-slice charge, cumulative global charge, pending exit, and any generation key prepared before suspension. The digest rejects impossible charge relations, missing/cross-variant prepared continuations, and a prepared target PC different from its transition PC while allowing generation activation to change the bank identity. Parked faults replace the earlier exact publication after exception entry and remain deliberately non-comparable because their native stopped continuation is not independently resumable. Operational-only canonical digests separate CPU bytes from continuation bytes and reject reordered or duplicate thread publications; device, executor, and ABI-host state use separate canonical component digests. The ABI publication and digest suites each passed 10/10 guarded runs on 2026-07-31. | A native host shim's stack and locals and a parked fault's native stop continuation remain deliberately opaque. Created-but-never-entered thread start contexts can be absent from the publication set. The latest per-thread publications are paired with owner snapshots taken after scheduler handling; this is deterministic comparison evidence rather than one atomic savestate. The legacy function lane and host-atomic shims do not preempt internally; exact per-instruction device timing is not claimed. | Require equal full RDRAM and owner-component digests at the exact global horizon, report CPU/continuation equality only when no executor thread is missing and no publication is opaque, then replace common native continuations with typed resumable continuations before claiming complete machine-state equality. |
| Devices/MMIO | One deterministic `DeviceFabric` owns typed PI, SI, AI, VI, MI, save, SP, and DP state. Managed calls, raw MMIO, generated-C proxies, and libultra shims converge on it. PI orders bytes/busy/MI/queue delivery; AI owns a timed two-slot FIFO; SI owns persistent PIF RAM and timed two-direction DMA. Raw and high-level EEPROM, Controller Pak, Rumble Pak, and Transfer Pak operations share their authoritative stores and latches. Runtime-configurable 1–62-bank Controller Paks use one physical image and retained bank latch as their sole authority: high-level operations decode and encode per-bank checksum-protected FAT/backup pages plus the sixteen-entry note directory seen by raw Joybus, global 16-bit chains cross bank boundaries, and ambiguous checksum/cycle/share/orphan/directory corruption returns `PFS_ERR_INCONSISTENT`. EEPROM writes defer backing-store mutation to one typed guest-cycle deadline, expose public `0x80` busy state through raw Info/Write, reject overlap, and make high-level polling plus LongWrite's per-block 15 ms timer use that same state. Transfer Pak support includes CRC-checked raw register windows, ROM/RAM persistence, ROM-only and MBC1/2/3/5 cartridge buses, sticky removal/reset state, all six public `osGbpak*` adapters, registration-header and connector validation, and documented deterministic initialization/power waits. Timer-bearing MBC3 cartridges advance on exact guest-second boundaries independently of Pak power, retain immutable latches, honor halt, and implement 9-bit day/sticky-carry rollover through both raw and high-level paths. Their live RTC/phase and exact ROM/type identity persist in a checksummed versioned sidecar; explicitly injected host timestamps materialize powered-off elapsed time once without entering runtime evidence. Voice has a typed initialized/READY/START/CANCEL/BUSY/END lifecycle shared by its nine shims, raw Info, captured result/status, five-write raw initialization, and initialization/clear/start/stop controls. VI owns its live register file, region timing, field/current/intr derivation, vblank-latched mode/scale/features/presentation, current mode/status queries, square-root gamma, coverage AA, coverage-gated median divot, RGBA16 neighborhood dither restoration, and retrace-seeded stochastic seven-bit gamma dither. Persistent RSP DMEM/IMEM, status/PC/semaphore, double-buffered DMA, raw DPC streams, and all six MI sources use the same event ordering. | PI/SI/SP-DMA and HLE SP/DP deadlines remain deterministic policies rather than measured hardware timing. EEPROM uses the public library's conservative 15 ms interval rather than a measured per-chip-revision timing model. Raw Voice still traps with typed evidence for `0x09` without an injected result, region-dependent `0x0A` staging, `0x0D` power/gain writes, and the unestablished `0x0C` dictionary-transfer mode. VI register timing lacks hardware-trace validation and exact random-stream identity remains unproven. A twenty-phase pinned-Metal gate supplies exact native post-VI gamma, bounded seeded gamma-dither, scale, divot, and RGBA16 restoration evidence. A separate adapter-capture test plus eleven-phase gate distinguish compatibility Unspecified from explicit mode 0 across the wire and native callback. For deliberately generated managed codes 1-6, modes 0/1 match an independent per-code Figure-11 CPU oracle, modes 2/3 restore exactly, and AA, source-before-projection divot, and AA-before-divot each change the six target pixels. This evidence is limited to pinned Metal, nearest/progressive synthetic RGBA16, opaque code-7 controls, and the original-aspect (4:3) presentation policy; pinned RT64's code-7 alias, code-0/save, natural/imported hidden coverage, insufficient neighborhoods, other filtering/scaling modes, MSAA/downsample, other graphics APIs, recognized-HLE/full-profile ROM breadth, silicon identity, and analog output remain uncertified. Function-lane generated code remains atomic between host boundaries. | Hardware-derived timing plus raw/shim/C-proxy byte-identical traces through the integrated executor. |
| RSP | The clean-room scalar/vector interpreter executes the persistent IMEM image from the fabric's PC, imports/commits architectural DMEM and SP status, resolves rectangular IMEM DMA generations, resumes after each overlay, and stops only at BREAK or a loud bounded failure. Unknown/custom task types take this LLE path. Known graphics/audio tasks execute admitted rspboot until control first reaches DMA-loaded ucode, committing RDRAM/DMEM/IMEM/status/entry-PC effects before the HLE backend; transactional LLE fallback also receives a typed snapshot of the complete non-memory machine state. Graphics tasks expose an explicit `HleOptimized`/`LleAccuracy` host policy; the release/parity harness selects accuracy and continues the loaded ucode through that same snapshot and interpreter. Synthetic normal, wrapped-overlay, invalid-boot, yielded, reload/resume, boot-register-continuity, and accuracy-policy raw-DPC gates prove the connection. The OS yield handshake uses the same live SP status: SIG0 requests yield, SIG1 prepares the task's yield buffer for restart, normal completion is read-only, and load clears stale handshake bits. | The selected HLE backend executes atomically, so SIG0 cannot yet preempt/resume an HLE task in flight, and instruction timing is a deterministic count rather than a pipeline model. | Run the synthetic admission/execution/overlay and yield-protocol gates 10 times, then exercise non-GBI full tasks with stable DMEM/RDRAM/DPC traces. |
| RDP/VI | RT64 is the faithful HLE lane; the pure-Rust F3DEX2 decoder emits ordered operations for 8-bit index/RGBA16/RGBA32 targets, fills, syncs, triangles, texture rectangles, and the independent depth-image register. One typed layout classifier is shared by validation, import, fill, copy, and commit. The reference backend imports, switches, commits, and same-address reinterprets all three public color-image layouts; the 8-bit layout stores one byte and ignores hidden coverage, while RGBA32 retains five-bit memory alpha and the three coverage bits in its alpha byte. It executes format-correct fill-cycle rectangles, normal/flipped RGBA16 copy-cycle rectangles, direct I8, packed IA8, and undereferenced CI8 copies into 8-bit targets, and one/two-cycle TEXRECT/TEXRECTFLIP through shared texture filtering, color combining, alpha compare, and framebuffer blending. Eight-bit copy preserves the original TMEM byte while alpha compare uses source-format intensity/alpha. YUV16 Y0/U/Y1/V tile loads, all six signed `G_SETCONVERT` fields, public `CONV`/`FILTCONV`/`FILT` selection, and K4/K5 combiner inputs run through the same triangle/rectangle sampler. `G_SETKEYR`/`G_SETKEYGB`, CENTER/SCALE combiner inputs, and `G_CK_KEY` alpha fixup implement the public soft-edge chroma equation and feed alpha compare. Programming Manual Chapter 13.7 mip/detail/sharpen selection uses immutable eight-tile primitive snapshots, adjacent perspective-corrected coordinate derivatives, modulo-eight tile selection, minimum/maximum LOD, and RGB/alpha LOD_FRACTION inputs across rectangles, high-level triangles, and raw coefficient triangles. A fill directed at the persistent depth image writes its raw halfwords and clears the covered software depth samples across later color-image switches. Bounded `osDpSetNextBuffer`, raw DPC START/END, and LLE RSP DPC submissions execute the proven subset. `CMD_END` captures the submitted words into an immutable command image staged outside guest RDRAM before backend dispatch. Both DRAM command DMA and XBUS DMEM command DMA reach the renderer. All eight raw RDP triangle layouts retain typed edge/shade/texture/Z planes through a coefficient-driven span walker with the public eight-sample checkerboard coverage mask; high-level F3DEX2 triangles now evaluate those same sample positions and use winding-independent top-left ownership for exact shared edges. Full masks retain pixel-center attributes while partial raw/high-level masks use one typed covered sample for shade, texture, and Z under a bounded nearest-to-center/stable-order policy. Set Scissor retains its public field-enable and odd/even selector, and every color/depth/rectangle/raw/high-level raster path rejects the opposite-parity scanlines. RGBA16 coverage and depth DeltaZ share a physical-address hidden-bit sidecar; all four coverage destinations, coverage/alpha selection, clear-on-wrap color writes, memory-coverage blending, and opaque coverage-wrap strict Z execute. `Z_CMP`/`Z_UPD` are independent; primitive depth works for triangles and rectangles; Chapter 15 relations drive opaque/interpenetrating/translucent/decal admission; and ordered RGB MagicSquare/Bayer plus alpha Pattern/InversePattern dither execute before target-format storage. One typed seedable deterministic per-fragment byte feeds combiner NOISE, RGB/alpha Noise, and `G_AC_DITHER`; the unpublished silicon stream remains unclaimed. Unsupported state still fails by name. | Exact LOD derivative norm/fixed-point boundaries, exact fixed-width/subpixel coefficient, conversion, key, and covered-sample selector arithmetic, interpenetration coverage adjustment, exact alpha-coverage rounding, same-visible-value CPU hidden-bit rewrites, filter arithmetic, exact hardware noise-generator identity/advancement, other unmodeled state, and precise timing behavior remain incomplete. | Raw fill/texture/depth-clear/triangle command streams produce deterministic image and coverage bytes through shim, MMIO, and LLE DPC entry paths; SP/DP/MI ordering and VI mode/field are captured separately. The bounded real C-lane path reached its first swap at step 445 after 28 graphics tasks in 20/20 clean runs. |
| Exploration | Discovery has typed trace/probe inputs and a bounded probe-plan foundation. | No real headless emulator producer, state mutation loop, coverage frontier, or forced-path admission rule is connected. | Digest-bound save state, forced branch, reproducible trace, and explicit natural-versus-forced reachability labels. |
| Validation | Unit/oracle tests, C ABI smoke, generated-body inventory/PC-set comparison, trace comparison, audio dumps, and live screenshots exist. Both boot harnesses use guest-quiescence timing. C/rs framebuffer observation matches through 60 swaps but is non-authoritative because the legacy C inventory has 116 callable empty bodies. Prepared native, schema-enabled typed-function, and typed-block lanes retain authoritative reached-destination order. Schema v29 binds those destinations, all five fixed-cycle channels, complete observation geometry, environment, closure, RT64's resolved graphics API and TV standard, nonzero completed workload/present identity, normalized ROM identity/class, decoded TV region, exact native Windows workstation build/UBR evidence, the ordered ABI-owned `fn64.rsp-rdp-observations.v2` stream, DeviceState v16's typed pending PI addresses, and live timing v2's matching PI DMA identity while retaining DeviceState v15's canonical 24-bit DPC counter projection, DeviceState v14's complete RSP interpreter continuation and admission generations, DeviceState v11's audio-task execution policy and translated artifact identity, DeviceState v10's AI/DPC latches and pending-transaction state, and v9's controller-manager, mapped-fetch/AOT expected-word, and typed pending/completed PFS transaction state. DPC counter increments and STATUS counter-clear/transaction interleavings remain open. Release admits only `AudioTaskExecutionPolicy::LleAccuracy`; translated identity cannot establish a live-IMEM match and diagnostic skip remains non-release. The trusted private-series runner accepts only a policy-revalidated opaque v3 contract, owns exact-ten sequential fresh child launches, and independently recomputes each admitted ROM's normalized header evidence before accepting a report. Admission v7 binds retail/public-homebrew class-specific provenance, a typed build receipt, the exact child/source/recompiled input, and the runner-owned class environment; retained v6 is read-only. Matrix v6 derives report-visible coverage; retained v19 matrices credit fixed NTSC/PAL/MPAL only from header/device/renderer agreement, while report-only verification cannot credit ROM class. The private-series path jointly revalidates an opaque capability's v3 contract, exact-ten receipt, retained reports/journals, raw ROM, runner image, and bound inputs; it exact-matches semantic report and ordered run-event identities before retaining a separately hashed v1 class authority. RT64 target-case credit requires a distinct opaque capability bound to the exact report and ordered matrix events. Its production constructor owns the repository-selected child, revalidated Cargo and fn64/adapter source identities, an isolated fresh build target, clean pinned RT64 source, 10/20-run watchdog-bounded process series, identical semantic output, and child-observed adapter/API identity. All 13 macOS/Metal examples emit the identity envelope; other target/API rows remain fail-closed. Missing evidence returns an explicit v7 incomplete assessment rather than a smaller denominator. | Historical private NTSC reference and RT64 LLE/post-VI lanes each supplied ten semantically identical schema-v22 reports with ten distinct terminal v3 journals and were independently reverified locally on 2026-07-22 at fixed cycle `722368695`, with zero unsupported events. A retained public synthetic identified-native XBUS series supplied ten more schema-v28 reports under the same unsupported-instrumentation denominator without receiving private-ROM authority; it too requires schema-v29 regeneration. The previous v6 incomplete assessment accepted all three scenarios and all 30 reports, satisfied 12 of 162 requirements, and retained 150 explicit gaps; the v7 assessment has not yet been regenerated. No schema-v29 representative series or assessment has yet been retained. No positive Windows report or production RT64 platform-case capability has yet been retained through the new constructor; blocker closure and allowed-source public-microcode identities also remain absent. A class-specific local provenance string is an admitted attestation, not independent public-homebrew provenance. A self-hashed receipt is not transferable process attestation without an external trusted CI/code-signing root; same-UID transient source mutation is outside the local runner's integrity threat model; native-archive identity does not repair the legacy C oracle's missing bodies; reached destinations do not prove unreachable code; native coroutine continuations remain excluded; focused oracles are not exhaustive; and measured parity only reaches swap 60 with a non-authoritative deeper C oracle. | Run and retain the macOS platform-case capabilities, populate a successor allowed-source public-microcode catalog, produce native Windows v29 evidence and production blocker authorities, retain external process attestation where required, and retain ten trusted v29 report/journal pairs for each remaining profile scenario. See `RELEASE-GATE.md`. |

Checkpoint update after the matrix snapshot: the canonical catalog owner now
counts exact charged `BlockProgram` work across all of its OSThreads while
excluding synthetic host and legacy-C charges. The WM bounded loop can stop at
the first scheduler checkpoint at or beyond a requested minimum and can
require the exact achieved baseline count on a comparison run. The receipt
records minimum, expected, and achieved counts; a minimum may overshoot by one
outer dispatch slice, while an exact expectation fails instead of rounding or
splitting a branch/delay pair. Static-through-dynamic boot accounting and the
generation-backed real-PI dynamic path each passed 10/10 guarded counter
assertions on 2026-07-31. This closes measurement of a shared guest-work
horizon, not complete state equality: typed native-host continuation and
canonical atomic full-machine serialization remain open.

`scripts/wm2000-withheld-rdram-diff.zsh` is the first executable A/B wrapper.
It runs separately retained AOT and `dynamic-withheld` binaries sequentially
under the memory guard, derives the baseline's achieved checkpoint at or above
the requested guest-instruction minimum, requires the dynamic lane to hit that
exact count, and compares a transient hash of all logical RDRAM bytes plus canonical
device, executor, and ABI-host component digests. Canonical thread CPU and
continuation digests are retained independently; they are comparable only
when both lanes publish the same complete non-opaque thread set. An exact CPU
publication is sampled before checkpoint suspension, a parked-fault marker is
sampled after exception entry, and returned state is terminal; owner components
are sampled after scheduler handling. The bundle therefore identifies its
relation as latest per-thread publications paired with post-scheduler owner
snapshots and is not labeled an atomic savestate. The wrapper writes only to a fresh private
out-of-tree directory and labels the strict result
`operational_rdram_and_owner_components_with_published_cpu_diagnostics`.
Its mock-binary contract test exercises baseline extraction, exact horizon
propagation, stale-ROM rejection, owner-component mismatch rejection, and the
positive match receipt.

The two lanes install the same complete static catalog and retain the same
canonical program and resolver-install identities. Only the dynamic lane
receives
`FN64_DYNAMIC_WITHHOLD_CANONICAL_ENTRY=1`. Its owner validates that the
selected `ExecutionKey` equals the installed entry, then redirects that entry
from static to dynamic execution once at the operational unified-dispatch seam.
The guard clears only after positive dynamic work; normal static budgets and
executable-mutation reconciliation then resume. Telemetry schema
`fn64.wm2000.dynamic-withheld-telemetry.v2` binds a per-attempt entry bank/PC,
positive `charged_instructions`, and zero `unsupported_exits`. Aggregate dynamic
totals do not prove that attempt. The identity line and comparator also bind
`resolver_install_sha256`. Whole-shard removal is obsolete because a bounded
route can avoid the selected shard entirely.

This checkpoint supersedes the dynamic-code matrix cell's earlier request to
add a horizon and operational digest: both mechanisms now exist in source. The
exact-entry pair and comparator contracts passed the deterministic bar after
the schema migration. Earlier real runs reached
100,001 instructions with whole shards 1 and 2 withheld and matched full RDRAM,
device, executor, and ABI-host digests, but neither withheld shard was entered;
those runs are partial owner-state evidence, not dynamic-execution evidence.
One exact-entry real-ROM v3 diagnostic reached 100,001 instructions in both lanes.
The withheld `(bank, PC)` `81bf2e27273b27db:80000400` ran dynamically once for
one instruction with zero unsupported exits. RDRAM, CPU, device, executor,
ABI-host, continuation, scheduler steps, and simulation time matched. The two
publication diagnostics also matched on pending `ExecutableWrite`, last charge
five, cumulative charge 100,001, and absent prepared continuation. This is one
operational diagnostic only; no ten-run parity result is claimed.

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

Mapped instruction admission adds a second, deliberately non-interchangeable
identity:

```rust
struct InstructionWordIdentity {
    bank: BankId,
    physical_address: u32,
}
```

`ExecutionKey.pc` remains the architectural VA used for branch/J/JAL link,
EPC, BadVAddr, and Cause.BD. Canonical 32-bit fetch translates that VA through
KSEG0/KSEG1 or the recorded TLB, then looks up only the physical identity
above. An AOT unit binds one straight word or a branch plus its independently
translated delay slot; the interpreter fetches the same unit dynamically.
Thus two VAs may share one word identity without sharing control-flow state,
and a remap cannot reuse a stale translated word. `BlockProgram::run` selects
this path whenever the destination `BankId` owns a physical generation: a
registered mapped AOT entry runs after exact preflight, and every other entry
uses the mapped interpreter. `dispatch` therefore follows both through its
existing transfer/exception loop rather than requiring a parallel executor.
Canonical `BlockProgram` evidence sorts and binds all physical spans/words and
every mapped unit's entry, exact `BankId`/PA sequence, preflight-expected words,
and generated artifact identity without retaining native pointers.
Mapped-interpreter destination
observations have no generated-runner artifact and therefore remain
operational/differential-only, not fixed-cycle release evidence under schema
v29; artifact-identified mapped AOT observations retain their real artifact
and are eligible, while compatibility AOT without one is not. The 64-bit
instruction-PC/catalog identity and the legacy whole-function boundary remain
loud rather than borrowing this 32-bit block-lane contract. Data addresses do
use the wider VR4300 contract: Status.KSU plus UX/SX/KX classify the extended
mapped ranges and XKPHYS, and extended refill entry updates full BadVAddr,
Context, XContext, and EntryHi before selecting the XTLB vector.

The opt-in live miss path uses the same exact-unit shape without first
mutating `BlockProgram`: `DynamicMappedUnitCatalogV1` translates and
snapshots the primary physical word, snapshots an independently translated
delay word when required, then executes that immutable local unit through the
shared interpreter. Its full SHA-256 identity binds the implementation-issued
dynamic source receipt and ordered physical address/word pairs. That receipt
conservatively binds the crate manifest and every library `src/**/*.rs` file;
an inventory test rejects an added Rust source until it is included. The
architectural VA remains only in `ExecutionKey`. A projected `BankId` is never
accepted without retaining and comparing the full digest, and any collision
with immutable static, mapped-AOT, or inactive precompiled-generation banks is
loud. Re-snapshotting every subsequent unit makes A→B→A content reuse exact
and prevents a committed code write from leaving a later unit cached. The ABI
owner places it after host, active/activatable precompiled, and static
resolution, reconciles attributed executable writes between units, and charges
the combined slice once. This proves the mechanism and ordering only; focused
tests are not a whole-ROM closure claim, and enabling the lane cannot mint
static writer or release authority.

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
completion can expose stale ownership. That notification observer remains
notification-only. A live owner additionally installs the typed
`GuestWriteBoundaryObserver`, which returns `ExecutableChanged` only after it
proves the committed range overlaps an active executable region. Generated AOT
and interpreted runners consume the mark after one straight instruction or
after the complete indivisible branch/delay pair. `BlockExit::ExecutableWrite`
preserves the source bank and resume PC; the typed resolve-call and fault forms
likewise carry their unresolved continuation out without entering its target.
The owner retires generation A, publishes B, and only then resolves the resume,
call target, or exception vector. Non-overlapping writes remain ordinary runner
work. The project-owned contiguous/sparse, emitted/interpreted delay-slot,
deferred-continuation, and live stale-sentinel tests define this runtime
contract. It is not a silicon cache-coherency claim: guest cache operations
remain validation/synchronization inputs, and cache tags and exact hardware
self-modifying-code rules are still unmodeled.

### 3.3 Deterministic device fabric

One `DeviceFabric` owns PI, SI, AI, VI, MI, save-device, and relevant SP/DP
state plus a stable `(deadline, sequence)` event heap. Both libultra shims and
raw KSEG1 loads/stores call the same typed device methods. Host audio and pixels
are sinks; callback timing never becomes guest hardware state.

That single authority now includes every AI DRAM/CONTROL/DACRATE/BITRATE latch,
the AI current/next FIFO, every DPC START/END/CURRENT/STATUS register, and a
tokenized pending DPC submission with an explicit RDRAM or DMEM source. A
renderer completion commits CURRENT and consumes the token; cancellation
consumes it without fabricating a commit. DeviceState v10 binds all of this
future-affecting state, preventing equal status projections from hiding
different DACRATE, START, source, or range futures. DeviceState v11 binds the
install-once audio-task execution policy and translated artifact identity;
DeviceState v12 additionally binds DPC CLOCK, BUFBUSY, PIPEBUSY, and TMEM;
DeviceState v13 adds the complete ABI-owned RSP interpreter continuation, and
DeviceState v14 binds the process-monotonic generation of loaded, lineage,
unavailable, and in-flight task owners plus the next admission generation.
DeviceState v15 canonicalizes the four public DPC performance counters to 24
bits at import and fails closed if release encoding sees a noncanonical value;
counter increments and STATUS counter-clear/transaction interleavings remain
open. DeviceState v16 types pending PI requests as ROM or SRAM offsets, and
live timing v2 carries the same typed identity for PI DMA rows. The
release environment admits only live-image `LleAccuracy`. The public
[N64brew Audio Interface register map](https://n64brew.dev/wiki/Audio_Interface?oldid=5924)
defines `AI_CONTROL.DMA_ENABLE` and the reflected `AI_STATUS.ENABLED` bit.
`AI_LEN` fills the two-slot FIFO while disabled but cannot drain until CONTROL
is enabled; `osInitialize` establishes the initial enable used by raw libultra
DRAM/LEN callers, and the managed submission repeats that idempotent write.
The public source does not define a mid-transfer disable/pause transition, so
changing CONTROL while either FIFO slot is active remains a named unsupported fault.
DACRATE and BITRATE changes are rejected under the same live-FIFO rule;
idempotent rewrites are accepted without mutating the active transfer. Typed
requests must match the public integer rate derived from the admitted divisor.
Exact silicon AI/DPC timing and counters, the precise AI-interrupt phase, the
hardware semantics of mid-transfer AI control/rate writes, FREEZE/FLUSH,
subword raw access, native-C mid-task visibility,
and silicon bus behavior remain outside the current model.

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
XBUS/DMEM DPC ranges both reach the raw renderer seam through that same
tokenized device transaction. Empty `START == END`
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

### 4.0 Sequencing and blockers

The U0-U7 milestones below specify *what* closure means for each area. They do
not say what order to work in, and two of the largest remaining gaps are not
engineering problems at all. This subsection is the critical path; it changes
what the next session picks up, and nothing below it repeats.

**Not every open gap is code.** Three distinct blocker classes exist, and only
one of them closes by writing Rust:

- **Sourcing** — the work is blocked on obtaining an allowed-source artifact.
  No amount of implementation moves it.
- **Capture** — blocked on physical hardware observation that has not been
  performed.
- **Implementation** — ordinary engineering, the only class an agent wave can
  close unattended.

**The long pole is capture, not code.** `BASE-RENDERER-BEHAVIOR-MATRIX.md`
records 4 of 24 base-render rows as exact public contract, 18 as bounded
reference, and 2 as missing; **19 rows are blocked on hardware trace** and the
repository contains zero physical captures. `RDP-SILICON-VECTORS.md` states
the position plainly: no hardware capture has been performed and no
silicon-accuracy claim is made. `VI-ANALOG-CAPTURE-PROGRAM.md` defines the
ten-run power-cycle cohort methodology, and it is unpopulated. Until captures
exist, those 19 rows cannot close **by any amount of implementation work**,
and every bounded-reference policy beneath them stays bounded. Treat the
capture program as a parallel long-lead track started early, not as a
follow-up to U6. Its absence is the single largest determinant of when base
accuracy can be called closed.

Two different bars are in play and must not be conflated. Render/VI silicon
accuracy needs **physical capture** and is genuinely blocked. Device *timing*
needs only **reference-emulator parity** through the differential oracle in
step 1 below — no hardware, no capture cohort. Do not let the blocked capture
program stall the timing work; they are independent.

**The certified-public-microcode catalog gates U6 and therefore U7.** Catalog
v1 is empty pending allowed-source digest provenance (`MICROCODE-DENOMINATOR.md`),
so no matrix can satisfy any of the twelve public-microcode requirements
today. F3DZEX2 — the family OoT and MM actually run — is named but unadmitted.
That chain is `catalog provenance -> F3DZEX2 admission -> U6 -> U7
full-rom-zero-unsupported`, currently 12 of 162 requirements satisfied with
150 explicit gaps retained. The head of that chain is a **sourcing** task with
no code in it, and it silently blocks the release gate. Start it before, not
alongside, the U6 implementation waves that depend on it.

**Recommended order.** U4 and U5 are implementation-class and mutually
independent — they touch disjoint crates and can run as parallel waves. U6
depends on the catalog head above. U7 is terminal by construction: it consumes
every other milestone's evidence, so it cannot start early and must not be
partially credited.

1. **Blocked on a review decision, and blocking the most work:** two
   foundation sub-specs are written and awaiting approval before
   implementation. Both gate large downstream sets, so approving them is the
   highest-leverage action available:
   - `superpowers/specs/2026-07-23-u4-fpu-environment-design.md` — the FPU
     environment, U4's critical path and the single largest behavior item.
     fn64's FPU is currently a round-to-nearest fast path: FCSR.RM is ignored
     on ADD/SUB/MUL/DIV/SQRT, arithmetic raises no IEEE flags, there is no
     ExcCode-15 path (an enabled exception panics), host NaNs are emitted
     rather than the MIPS canonical forms, FR=0 is hardcoded, FP conditional
     moves are absent, and the interpreter lane implements no COP1 arithmetic
     at all. Open decision: approving `rustc_apfloat`, the first soft-float
     dependency — host `fesetround` is rejected because it breaks `wrong == 0`
     determinism.
   - `superpowers/specs/2026-07-23-timing-oracle-design.md` — the differential
     timing harness. Every timing item in U2/U5/U6 is currently deterministic
     *policy*, and none can be called done without a reference oracle to
     measure against. Open decision: tolerance philosophy. Note the bar here
     is **reference-emulator parity, not cycle-exact silicon** — a
     deliberately different and cheaper bar than the render capture program,
     and not blocked on hardware.
2. **Immediately, in parallel and unattended:** the rest of U4 CPU
   architectural closure and U5 device closure. These touch disjoint crates.
3. **Immediately, as long-lead non-code tracks:** microcode catalog
   provenance; the hardware capture program.
4. **On catalog admission:** U6 RSP/RDP closure.
5. **Terminal:** U7, once the above retain evidence.

**Unowned gap: RDRAM sizing.** `rdram.rs`'s `DEFAULT_RDRAM_SIZE` is a
hard-asserted 8 MiB constant, and `osGetMemSize_recomp` returns it
unconditionally. No 4 MiB stock-console configuration exists, so a ROM whose
behavior differs without an Expansion Pak cannot be modeled. This is
implementation-class, cheap, and belongs to U5. It appeared in no milestone
before this revision.

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
boundaries are compiled/live gates. Real-pack boot wiring now requires a
canonical ROM/IPL3/TV/entry-bound `BootContext`, restores the captured
GPR/HI/LO/modeled-CP0 image, and seeds Count/Compare/IP7 before thread 0 is
created. The public-debugger black-box producer emits the complete raw CP0 slot
image at the header-entry pause. The first private capture series exposed a
queued-step race in the producer: a timeout re-kick could be consumed after the
next pause but before callback publication, losing a pre-window observation
and changing CP0 Count. The fix permits one outstanding step and traps on
callback stall. Twenty consecutive NWXE captures then reached the same
5,079,153-instruction pre-window horizon and produced one boot-context digest
and one bounded-trace digest. A real fn64 first-entry comparison and runtime
code real-pack translation policy were the next boundary. The generated-runner
gate now compares the live state before instruction one; NWXE matched all GPRs,
HI/LO, and modeled CP0 fields exactly. Checked aligned word accesses now offer
the translated address to the live MMIO hook before rejecting absent RDRAM
backing, so the SI-status read at `0x80038268` / `0xffffffffa4800018` executes
in arbitrary-PC AOT. The generated NWXE pack additionally recognizes the
unique exact six-word `__osSiDeviceBusy` body, emits its address as a pack fact,
and gives only that fact typed host-call precedence; absence or ambiguity is a
build failure. Three independent public-debugger target snapshots then
classified the apparent TLB frontier as a harness materialization bug: the
reference reached `0x80036f10` after 261,748 retired window instructions with
`$t8 = 0xffffffff80048860`; fn64's `0x60880480` was the exact byte reversal
caused by copying flat big-endian ROM bytes into native-word RDRAM storage.
The block example now uses the canonical logical IPL3 DMA materializer. Ten
corrected runs first stopped identically at the honest sparse-pack miss for
spawned thread entry `0x800004d0`. The fixed-point pack builder now requires at
least three ROM-bound black-box traces with identical parsed authority,
completion shape, event counts, and exact activation-0 boot-root sets, and
augments the static bank with those observed bank-generation words. Raw PC
order is not an authority: regenerated two-million-step captures first
diverged when asynchronous interrupt delivery moved a general-exception entry
by adjacent guest instructions, while all three retained the same 5,183-PC
total set and exact activation-0 boot-root digest. Observations stay scenario
coverage rather than function-owner proof or an exhaustive support claim. The
public debugger does not expose a separate pause for architectural delay slots,
so the producer removed its former executed-PC exhaustiveness claim and the
pack adds required delay-slot words from the verified ROM mapping. Three
regenerated traces reproduced the exact admitted root set. The resulting bounded
NWXE pack contains 1,929 observed PCs plus 289 required delay slots in 90
spans / 2,517 total words and admits `0x800004d0`. Ten consecutive runs reach
the next frontier with no earlier `AotMiss`: a runtime memory-model fault at
`0x8002a8d8` for guest address `0xffffffffb0000000`. The checked raw-word seam
now maps canonical cached/uncached PI-domain-1 cartridge reads to the installed
read-only ROM source shared with PI DMA. Ten consecutive corrected runs pass
that access and expose a raw guest VI initialization image with V timing and
scales programmed, H_START zero, and status enabled. The independent public
debugger observes the same values and no H_START transition; one adjacent-pause
variation in the status observation prevents treating that diagnostic as an
instruction-exact timing oracle. A zero H or V interval now remains an
inactive retained image, while nonzero malformed intervals still trap. Ten
consecutive corrected runs pass the old VI assertion and stop identically at
the separate missing-render-backend frontier, with no earlier `AotMiss`.
Static closure and runtime behavior therefore remain separate reports. The loud gap retains current CP0
context without claiming a non-architectural miss committed an exception.
The next full-image gate emits all 262,144 aligned words of the one-MiB NWXE
resident image. A single generated control-flow body exceeded the two-minute
budget and approached the 4 GiB per-process ceiling. Sixteen fixed,
content-addressed 64 KiB crate artifacts now retain the planned bank boundary;
each uses static 4 KiB subrunners internally so rustc never analyzes 16,384
entries as one function. No runtime decoder was introduced. Measured on the
same input, full `cargo check -j4` completed in 62.67 seconds, native debug
build in 107.56 seconds, and unchanged rebuild in 0.06 seconds; the debug
binary is 295 MiB. Three independent 400,000-step traces, BootContexts, and
completed `0x80000180` image captures are byte-identical. The four-word
CPU-produced general-exception preamble remains a separate digest-gated bank.
Its four admitted words are compared directly on the matching hot path, while
a mismatch still hashes the complete live image and reports the admitted
expected digest in `AotMiss`; hashing is evidence construction, not routine
exception-entry work.
The standalone artifact represents captured exception images as a validated
catalog rather than one scalar bank. Multiple independently reproducible
groups can therefore install disjoint, digest-gated static runners without
changing runtime translation policy. Catalog construction rejects unmodeled
first-fetch entries, overlapping ranges, immutable-shard overlap, duplicate
capture identities, and truncated bank-ID collisions. Only the existing
`0x80000180` capture is presently owned; the other modeled vector entries stay
open until equivalent evidence or a machine-checked unreachability proof
exists.
With an explicit reference renderer and typed in-memory SRAM device, dense AOT
passes every former sparse-pack miss. RSP-produced RDRAM writes now defer
generation publication until the enclosing generated instruction returns,
closing a nested live-program borrow while preserving publication before the
next guest instruction. A typed indirect-transfer trace proved that the
apparent `0x800e1b90` indirect frontier is instead the target of a direct
`jal`. The first fetch-time implementation hashed the containing 4 KiB page
and incorrectly classified it as a replacement generation. The capture's
`first_executed_pc` was caller-supplied rather than observed, its first
`0x790` bytes are mutable non-code state, and its executable suffix
`0x800e1b90..0x800e2400` is byte-identical to the ordinary resident mapping
from ROM `0xe2790..0xe3000`. A public-debugger word probe found the entry word
present at the header handoff and unchanged over two million steps; a
byte-for-byte comparison establishes the complete suffix identity. The
trace-derived duplicate generation is therefore rejected. Dense AOT runners
now verify the exact instruction word immediately before executing it and
verify a delay word only on a path that executes the delay slot. Neighboring
data writes no longer retire valid code; a changed fetched word returns typed
`ImageChanged` with zero retired instructions, and the closed catalog hashes
only complete mechanically recovered overlay candidates before retrying the
same instruction. Unknown content traps with `AotMiss`; there is no runtime
translator or interpreter fallback in the `aot-runtime` production feature.
Focused gates cover changed instructions, mutable neighbors, annulled/taken
branch-likely delays, shard-end delay lookahead, and cross-shard fallthrough.
The corrected debug executable completes both two-million- and ten-million-step
idle-boot runs without an earlier `AotMiss` or false `ImageChanged`; the longer
run executes 9,305,999 AOT entries and peaks below 1 GiB. Neither run enters a
ROM-recovered overlay generation. Extending the horizon fivefold therefore
does not supply overlay evidence: the preserved public-debugger traces contain
no controller-input authority, and this idle scenario does not request the
gameplay overlays. Closure now needs a deterministic controller scenario (and
independent black-box trace of that same scenario), not another passive boot
horizon. A serial release build is not safe on this 24 GiB host: one
`wm2000-block-overlay-2-shard-04` rustc
process crossed the explicit 10 GiB guard and was terminated while system free
memory remained above 80%. The guarded serial debug build completed in 25
minutes 33 seconds and stayed near 1.1 GiB during code generation after the
CPU-only per-shard discovery phase. Its exact minimum-budget PC history matches
the public-debugger guest prefix after accounting for architectural delay
slots, annulled branch-likely charged units, and the typed
`__osSiDeviceBusy` substitution. Flat lockstep ceases at the expected
`osCreateThread` host boundary. Public-debugger target snapshots are not a
state arbiter there: the reported 24-byte stack difference contradicts the
reference trace's own intervening stack-adjusting instructions, and three
common-resume attempts expose a non-boolean `$v0` after the independently
decoded boolean-return body. No runtime behavior change is justified by those
snapshots. ROM-recovered overlay entry and ten-run deterministic validation
remain open runtime-timeline gates.
Build-phase profiling subsequently showed that the standalone shard workspace
compiled mechanical discovery at `opt-level = 0`: one representative overlay
shard spent 69.185 of 70.731 build-script seconds in overlay recovery. A
scoped build-profile override plus a sparse valid-record index reduced the
same complete build script to 249 ms (86 ms recovery) without optimizing the
generated guest crate. The recovery receipt and `2 / 1 / 4`
candidate/admission/recipe counts are unchanged across 10/10 real-ROM runs;
the full guarded debug build now completes in 17 minutes 17 seconds, down from
25 minutes 33 seconds, while paying the one-time optimized host-dependency
rebuild. It peaks at 3.27 GiB during final linking with 88% system memory free;
serial generated-crate compilation, not discovery, is now the dominant cost.
The next invalidation reduction is implemented as an inactive foundation: the
legacy build and one-shot ROM-wide producer share one generator, the producer
atomically publishes a digest-indexed private prepared tree outside the repo,
and each shard's std-only materializer can copy only its verified pair of
generated sources. The v2 root cross-binds stable package sidecars: global
source-claim changes leave them byte-identical, and one changed artifact
changes only its owning sidecar at the byte-contract layer. The
producer retains the same root for claim-only retries by atomically replacing
only the authority manifest. Actual zero/one-shard Cargo invalidation remains
an activation benchmark, not a current result. The canonical format and
cold-authority activation gates are in
[`WM-PREPARED-SHARDS.md`](WM-PREPARED-SHARDS.md). The 35 manifests deliberately
remain on the current build script. Generated-build v3 now owns and repeatedly
measures the producer and private prepared candidate, but records the exact
mode as `legacy_with_prepared_candidate`; the selected binary's legacy source
attestation therefore remains the compilation authority. Real-ROM byte parity,
the all-manifest `prepared_consumed` switch, and guarded cold/warm invalidation
measurements remain open, so this is not yet a speedup result.
The shared shard build script now treats `FN64_PROFILE_BUILD` as observational
rather than a Cargo invalidator and writes generated outputs only when content
changes. A full hot graph with the flag toggled completes in 0.12 seconds. The
checked-in macOS guard launches a dedicated session/process group, samples
that exact group until it is empty even when descendants are reparented, and
terminates only that group. RSS and system-free thresholds are sampled safety
signals, not kernel hard limits, so short overshoot between samples remains
possible. The
current local build wrapper fixes Cargo at one job and defaults to a 4 GiB
aggregate ceiling plus a 40%-free system floor; historical debug `-j3`
measurements reached 5.28 GiB, and broad release parallelism remains outside
the safe envelope. Opt-in wall-time and path-free JSONL sampling make a profile
bounded without making the profiling flag a Cargo input. A per-bank AOT
counter attributes a 200,000-step resident probe to shard 00 (56.5%), shard 03
(34.1%), and shard 02 (9.4%). A historical experiment optimizing the first two
hot crates reduced that exact probe from 24.75 to 18.41 seconds; its guarded
`-j2` rebuild took 1 minute 41 seconds and peaked at 5.45 GiB. The current
standalone full-graph profile instead applies opt-level 1 / 16 codegen units to
all dependency packages so the final link can load the shard catalog within
the measured envelope, with opt-level 2 restricted to the handwritten
reference-renderer, runtime, and ABI hot paths. Those first measurements also
exposed zsh's automatic `nice(5)` for monitored background jobs. Disabling
that priority change in the guard reduced the same final probe again to 9.07
seconds, for a compound 63% reduction from the original guarded configuration.
Unrequested
block-destination and host-boundary
histories are suppressed only by this exploratory harness; the library default
remains complete history, and setting either trace-output environment variable
retains the complete corresponding stream.

The controller schedule is now a digest-bound, per-port read-ordinal format,
so translated host substitutions cannot skew its replay clock. Its first
resident probe reports final ordinals `[0, 0, 0, 0]` after 200,000 steps: the
current timeline performs no standard-controller read at all. Input search
therefore cannot request an overlay yet, and building the independent Mupen
input plugin would not close the present frontier. Device/task progression to
the first controller poll or natural overlay request must be diagnosed first.

Subsequent semantic host discovery bound the public `osSetTimer` behavior into
the typed timer wheel, allowing controller initialization to complete one SI
operation without an input schedule. The same work added uniqueness-gated
bindings for the public `osSpTaskLoad`, `osSpTaskStartGo`, `osSpTaskYield`, and
`osSpTaskYielded` operations and retained `LleAccuracy` for audio tasks. A
separate VI cadence correction stopped repeated H/V timing writes from
restarting the beam epoch. With those corrections, 10/10 current
non-exploratory runs enter mechanically recovered overlay generation
`0x5DEA0D1723E94993` at step 19,523 and `sim_time=13990253`; guarded peak RSS is
134 MiB. ROM-recovered overlay entry is therefore closed for this scenario,
superseding the passive-horizon status above. Full static execution is still
open: a bounded 100,000-step continuation completes overlay0 but reports zero
graphics submits and enters no later overlay generation.

Reserved
encodings in the dense AOT emitter and development interpreter produce precise architectural RI exceptions
(ExcCode 10) instead of code-generation or unsupported-lane failures. The
block lane's live region
mechanism observes registered typed CPU and device DMA writes, snapshots their
final architectural
bytes at a host boundary, atomically retires generation A, publishes B at the
same PC, and re-resolves interrupt/checkpoint/host/spawned-thread entries into B
without leaving A's runner reachable. Its separate typed post-commit overlap
boundary now stops generated AOT and interpreted runners after one straight
instruction or one complete branch/delay pair. Deferred exits retain the
source bank and prevent call/fault target resolution until B is published;
non-overlap continues, and the live gate keeps A's post-store sentinel
unreachable. Page-granular ownership, automatic executable detection, a real
translator/pack, dynamic-target recording, and real private-pack execution
evidence remain open.

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
receives canonical guest byte order. The project-owned
`GuestWriteBoundaryObserver` contract now classifies proven active-region
overlap without changing ordinary write notification. Generated AOT and the
interpreter stop at the first architectural boundary after the committed
store, deferred exits preserve source-bank lineage and postpone call/fault
resolution until replacement publication, and the live stale sentinel cannot
run from A. Non-overlap remains in the current turn.
Page-granular ownership, a real translator-backed builder/pack, automatic
executable PI/decompression detection, dynamic-target recording, and boot-pack
registration remain open.

### U4 — CPU architectural closure

- Replace host panics with precise VR4300 faults and exception vectoring.
- Complete remaining CP0, Count/Compare timing, 64-bit instruction-PC/catalog
  identity, delay-slot BD/EPC state, and interrupt arbitration.
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

The executable-source receipt now treats initial Status as a distinct U4
authority. An absent `FN64_BOOT_CONTEXT` is recorded as an open `missing`
state; a supplied context is accepted only after its canonical ROM, IPL3,
region, TV, and entry identities match the normalized discovery image, and
then retains the exact CP0 Status value. This proves neither subsequent
`MTC0` values nor child-thread inheritance, which remain separate frontiers.
The same receipt now requires a digest/range/generation-bound Status scan for
every reproducible external executable image, so dynamically captured vector
or generated code cannot sit outside the U4 instruction denominator.
Proven-code Status writes are paired one-to-one with the existing CFG
value-set analysis. Only exhaustive finite `MTC0` values that all clear BEV
close that write; mutable image loads, widening, unknowns, and every `DMTC0
Status` remain typed U4 blockers.
Only after those facts, the exact typed installed-host denominator, normal
handler ownership, executable-writer closure, and transfer closure all agree
may the receipt's in-process BEV-clear induction mark the three bootstrap
vectors unreachable. Exception/interrupt entry, ERET, scheduler restore, and
`osCreateThread` preserve the invariant; no caller-supplied opaque proof digest
is accepted.

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
boundaries. The executor now owns Count's half-rate phase, passes that exact
phase into both arbitrary-PC lanes for interior MFC0 Count reads, and retains
wrap-safe Compare matching plus the latched IP7 line; live MTC0 Count/Compare
writes cross back to that authority, including same-value Compare
acknowledgement before ERET. Interior reads do not mutate the boundary-owned
clock, so its post-block advance remains single-counted.
The exact checkpoint-match/handler/acknowledge/ERET interleaving passed 20
consecutive clean runs.
Status.FR is now a lossless view switch over all 32 physical FGRs:
FR=0 exposes their low words and joins adjacent even/odd low words only for
even-indexed doubleword operands, while FR=1 exposes each full 64-bit register.
FR=0 odd doubleword access is loud rather than silently aliased. This
implements the documented register organization without claiming silicon
parity.

S/D ADD, SUB, MUL, DIV, SQRT, ABS, and NEG use the host-independent soft-float
path in both arbitrary-PC lanes. They honor FCSR.RM, report modeled IEEE
conditions, reject denormal operands/results with unmaskable Cause.E, and
return FloatingPoint/ExcCode 15 before destination commit when required.
MOVF/MOVT and MOVZ/MOVN preserve raw S/D bits and predicate destination
mutation. Float-to-float and fixed-to-float conversion boundaries, complete
privileged/RI behavior, cache effects, cycle timing, and 64-bit instruction
identity remain open.

The generated-C ABI bridge preserves the public `recomp_context` layout: FR=1
maps all 32 C FPR slots directly, while FR=0 even slots carry active pairs and
otherwise-invalid odd doubleword slots carry latent upper halves. The bridge
requires `status_reg.FR == mips3_float_mode`, arms `f_odd` for the entry view,
and admits only the exact registry of FR-stable host shims before exposing the
raw context pointer. This rejects even a transition which accesses the other
view and restores the entry mode before return; the exit check remains a
second invariant before decoding entry-view bytes
under the wrong layout. The same common boundary snapshots Status.BEV and
rejects any exit transition before copying `status_reg` back. Whole-register
Status replacement remains available only through typed guest/COP0 authority,
never as an incidental side effect of an admitted legacy-C adapter.
Every naturally aligned integer, LL/SC, and COP1 memory operation in the bank
lane now checks its effective address before any register, memory, or
reservation mutation. Generated arms construct these failures through the
typed, shared cold boundary in `fn64_recomp_rs::generated_support`; the normal
memory path remains inline. Misaligned loads return AdEL/ExcCode 4; stores return
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
handler entry, and ERET. Remaining COP0/COP2, 64-bit instruction admission,
instruction-interior interrupt/Compare sampling, whole-function-lane, and
floating-point exceptions remain open.

### U5 — device closure

- Bring VI, AI, SI/PIF/controllers/accessories, saves, and remaining PI/MI
  behavior into the same fabric.
- Model FIFO depth, status, latency, masks, regions, and event ordering.

Gate: no shim/raw-MMIO split authority; deterministic framebuffer, audio,
memory, interrupt, and timing traces at fixed guest cycles.

Working-tree frontier: AI shim and typed-Rust raw submissions now share a
single complete AI register file and two-entry current/next FIFO. Raw and shim
starts consume the same DRAM/CONTROL/DACRATE/BITRATE state. DPC START/END/
CURRENT/STATUS and each pending RDRAM-or-DMEM submission likewise live in the
fabric until explicit commit or cancellation; no shim, LLE, or raw path retains
a second range authority. The fabric computes completion deadlines from
buffered stereo frames, the 93.75 MHz CPU clock, and the exact public
`VI_CLOCK / (DACRATE + 1)` rational with one final ceiling. The truncated
integer ABI rate remains backend metadata. The ABI computes its divisor with
exact integer round-to-nearest, uses fn64's bounded 132..=16384 admission, and
publishes metadata only after successful register admission. `AI_LEN`
decreases in eight-byte units; typed starts reject unrepresentable
address/length/range inputs, while raw writes mask them first. FIFO-full
submissions fail. FIFO FULL 1-to-0 raises MI AI and posts OS_EVENT_AI only
after queue promotion is visible; lone/final BUSY retirement does not
fabricate that edge. CONTROL-disabled starts remain accepted into the dormant
two-slot FIFO and the ENABLED bit reflects the same latch; 0-to-1 begins drain.
Mid-transfer CONTROL/DACRATE/BITRATE writes fail with named unsupported faults.
The hardware interrupt phase, other assertion causes, DAC clock-domain phase,
and per-edge `AI_LEN` timing remain explicitly unclaimed. Exact silicon AI/DPC
timing and counters, FREEZE/FLUSH, subword raw access, native-C mid-task
visibility, and silicon behavior remain open. The
public `rcp.h` command semantics also converge SP/SI/VI/AI/DP acknowledgement
on the common MI source. That RCP/MI authority exists from host-state creation,
independent of cartridge ROM installation; PI separately retains a loud
missing-ROM gate. Public `OSPiHandle` decoding is now the common managed/raw
EPI and programmed-I/O authority: every EPI entry validates Chapter 27's
uncached KSEG1 handle base, applies the handle's domain timing to the same raw
PI registers, and forms the public KSEG1 `baseAddress | devAddr` before
converting at the physical PI boundary and entering the fabric. Game Pak ROM
and SRAM normalize into the existing one
ROM/save engine; malformed handles, domain/base mismatches, and documented
64DD/bulk spaces without an attached backing device record one typed
unsupported event and trap instead of falling through to ROM bytes. The old
`osPi*` family retains its documented Game Pak-relative address convention
while converging below handle decode on the same fabric. These semantics come
from the public Programming Manual Chapter 27, “EPI Manager” and “SRAM,” and the public
`osEPiStartDma`/`osEPiRawStartDma` function pages; no runtime implementation
was used as a source. The typed IPL standard selects NTSC/PAL/MPAL VI and AI
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
while the RT64 lane receives the same status bits. A twenty-phase pinned-Metal
pixel gate now observes nondefault 8x6 active geometry over one workload:
gamma, bounded seeded gamma dither, and 1.5x scale change exact pixels;
baseline/gamma phases restore exactly; repeated equal seeds reproduce exact
pixels across distinct presents; and different seeds change them. Native
coverage-gated horizontal divot now makes twelve exact componentwise-median
changes and restores exactly. Native RGBA16 `DITHER_FILTER` applies the signed
available-neighbor 3x3 restoration while preserving alpha: eighteen eligible
full-coverage pixels change exactly, twenty-four non-full pixels remain
unchanged, and six flat full-coverage controls remain unchanged. A separate
adapter-capture test and eleven-phase gate preserve hardware mode 0 versus
compatibility `Unspecified` across the wire and native callback. For
deliberately generated managed codes 1-6, modes 0/1 match an independent
per-code Figure-11 CPU oracle, modes 2/3 restore the exact baseline, and AA,
source-before-projection divot, and AA-before-divot each change the six target
pixels. Gamma dither
uses a deterministic, retrace-seeded stochastic seven-bit quantizer; the
unpublished silicon random stream is not claimed exact. The native restoration
evidence is bounded to pinned Metal with nearest filtering, progressive
synthetic RGBA16 input, deliberately generated managed codes 1-6 with opaque
code-7 controls, and the original-aspect (4:3) presentation policy. RT64's
managed target is not authoritative
RGBA16 storage or RDP dither history, and its retained alpha is only the native
coverage estimate, and code 7 aliases managed 7/8 with clamped 8/8;
code-0/save and natural/imported hidden coverage, insufficient neighborhoods,
other filtering/scaling modes, MSAA/downsample behavior, other graphics APIs,
full-ROM coverage, silicon identity, and analog output remain open. Black,
public 10-bit fade, and first-line
repeat transitions now independently trigger V-blank presentation through
typed VI state; the Rust reference applies them without erasing its RDP source
and restores that source when disabled. Fade/repeat are exported beyond the
canonical NMR inventory. The RT64 path maps the same controls to VI pixel type,
vertical scale, and vertical subpixel offset through its quarantined C boundary;
physical-console filter traces, natural/imported native coverage, RT64's
code-7 7/8-versus-8/8 alias, code-0/save, insufficient neighborhoods, and
broader filter-lattice certification remain open; the bounded managed
codes-1-6 AA-selector stage itself is implemented.
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
