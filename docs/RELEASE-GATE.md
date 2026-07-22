# Fixed-cycle release evidence

Status: live minimum-scenario gate wired into `examples/oot-boot`; no
zero-unsupported full-ROM claim has been made.

`fn64-boot-harness` exposes two layers:

- `FixedCycleDigestGate` is the generic five-channel digest builder:
  framebuffer, pre-resample AI PCM, a declared memory image, the typed
  complete device-fabric, executor-control, ABI HostState, and typed-program
  evidence snapshot,
  and the typed timing trace.
- `LiveReleaseGate` arms trace and audio capture before guest cycle zero. Its
  committed-VI token copies physical RDRAM in logical order, pre-resample
  audio, traces, operation histories, typed owner state, and the executable
  entry stream at the configured cycle,
  derives a minimum closure ledger from typed observations, writes JSON even
  when the ledger is incomplete, and then fails the run unless every minimum
  path was exercised.

The digest rejects a wrong-cycle, duplicate, reordered, or omitted channel.
Each channel has a canonical lowercase SHA-256, and both live construction and
retained-report verification recompute the artifact root from the
`fn64.release-gate.v21` schema, cycle, exact ordered channel set, byte lengths,
and channel hashes. Schema v21 emits each closure path's typed observation
count and `report_sha256`, an explicit wire digest over the schema, scenario,
private input hash, complete fixed-cycle digest, and canonical counted closure
ledger.

For ROM input, v21 additionally binds the declared class, source z64/n64/v64
order, byte length, SHA-256 after canonical big-endian normalization, raw
destination code, decoded NTSC/PAL/M-PAL or region-free class, and the concrete
TV standard configured in the device and renderer. Construction compares the
supplied raw bytes and SHA-256 with the identity frozen by the ABI host's PI
owner. Fixed destination codes must agree with both the committed device
fabric and the renderer's create-time TV authority; unknown codes and
mismatches fail loudly. Region-free codes retain the concrete host choice but
satisfy no fixed TV-region requirement. ROM class is never inferred from
bytes. A generic declaration remains audit data, not certification authority;
only the private admission/contract path described below can authorize class
credit. On Windows, v21 also requires an exact native workstation identity:
kernel major/minor/build, update build revision (UBR), and the Windows 10 or 11
family derived from that build. Server products, detected Wine hosts, missing
UBR, and caller-relabeled families fail closed. Non-Windows reports cannot
carry Windows-version evidence.

This host classification uses Microsoft's public `RTL_OSVERSIONINFOEXW`
layout and `RtlGetVersion` contract, including `VER_NT_WORKSTATION` product
classification; Microsoft's release-health tables identify Windows 10 RTM as
build 10240 and Windows 11 21H2 as build 22000. Microsoft's OEM deployment
guide identifies
`HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\UBR` as the
Windows build revision, and `RegGetValueW` supplies the documented typed DWORD
query. These are host-identification sources, not N64 behavior sources. The
Wine rejection is deliberately bounded to the conventional
`wine_get_version` export documented by WineHQ; absence of that marker is not
a general compatibility-layer attestation.

- Microsoft Learn, [**OSVERSIONINFOEXW structure**](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-osversioninfoexw)
  and [**RtlGetVersion function**](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/wdm/nf-wdm-rtlgetversion).
- Microsoft Learn, [**Windows 10 release information**](https://learn.microsoft.com/en-us/windows/release-health/release-information)
  and [**Windows 11 release information**](https://learn.microsoft.com/en-us/windows/release-health/windows11-release-information).
- Microsoft Learn, [**OEM deployment of Windows 11 desktop editions**](https://learn.microsoft.com/en-us/windows-hardware/manufacture/desktop/oem-deployment-of-windows-desktop-editions?view=windows-11)
  and [**RegGetValueW function**](https://learn.microsoft.com/en-us/windows/win32/api/winreg/nf-winreg-reggetvaluew).
- WineHQ, [**How can I detect WINE from my program?**](https://forum.winehq.org/viewtopic.php?start=25&t=4988)
  (`wine_get_version`).

The header offsets, destination-code table, and region-free values come from
the public N64brew **ROM Header** specification (sections “Standard header” and
“Game Code”). Byte-order normalization follows the same public header facts
and fn64's existing clean-room `fn64-discover::rom` normalization. The
retail/homebrew distinction has no header source and therefore remains a
separate admitted provenance claim.

V19 also binds the exact entered execution-destination sequence, its canonical
unique destination set and counts, and independent ordered/set SHA-256 values.
It retains v18's ABI-owned ordered RSP/RDP stream: exact 4 KiB IMEM digests,
diagnostic family recognition from the registered backend's runtime catalog,
the original task microcode-data address plus exact logical byte length/SHA-256,
committed IMEM replacements, and DRAM/XBUS DPC command digests. Those fields
share the `fn64.rsp-rdp-observations.v2` wire; a recognition event binds text,
data, and family together. Recognition is diagnostic/optimization evidence
only; release reports still require `LleAccuracy`, so a backend label never
substitutes HLE execution for the ROM's RSP instructions or independently
certifies its family. V21 validation requires nondecreasing event cycles and
global IMEM generations, strictly advancing replacement generations, and one
consistent text digest per generation. It also binds framebuffer source,
format, dimensions, tight row size, payload size, and either the physical
RDRAM address or RT64 post-VI backend/settings/workload/present identity, plus
the complete physical RDRAM observation range and boundary-frozen release
environment. V18 added the concrete active D3D12, Vulkan, or Metal API; the
requested `Automatic` setting is never an evidence identity. The exact API is
observed from the completed capture framebuffer and command-list types and
must agree with any explicit request, the host platform, and the authoritative
backend identity's post-VI capture API. V19 and earlier reports are rejected
rather than reinterpreted under the v21 host-identity wire.
The Memory
artifact byte count must be exactly eight
MiB. A reference Framebuffer artifact count must equal its RGBA16 payload; a
post-VI count must equal the exact canonical render envelope containing its
BGRA8 payload and metadata. V11 and earlier reports are rejected rather than
reinterpreted under the new wire shape: v8 lacks the
executor-control/ABI-host/program envelope, while v9 cannot distinguish a
truly program-free fixture from an unidentified linked native/C program. V10
lacks the boundary-frozen platform, four-port, cartridge-save, and renderer
environment, while v11 omits Controller Pak bank geometry and the active bank
latch. Cite the report SHA for cross-run evidence; the artifact root
alone does not bind the scenario's private-input or environment (although v21's
DeviceState component binds installed-ROM and executable-program identities). The
report never serializes ROM, framebuffer,
audio, trace source, or RDRAM bytes. Device and trace encodings have explicit
big-endian wire formats; Rust `Debug`, host wall time, and the diagnostic
sequence counter do not enter the root.

The artifact-root wire is itself domain-separated by the release-report
schema. Moving from v20 to v21 intentionally changes both the artifact root
and `report_sha256`. V21 makes memory/audio/trace observations boundary-owned,
cross-checks reference pixels against frozen RDRAM, and binds the compiled
`fn64.unsupported-instrumentation.v1` schema/SHA-256. V20 added exact native
Windows workstation build/UBR evidence. The earlier move from v18 to
v19 likewise changed both values for reports whose five captured
channels are otherwise unchanged. V19 adds normalized ROM identity, declared
class, decoded TV region, and renderer TV authority. V18 added the concrete
active graphics API to the canonical RT64 environment. The preceding v17 transition replaced the
v1 RSP/RDP observation wire with `fn64.rsp-rdp-observations.v2`, adding
task-start microcode-data address, exact length, and digest to each recognition
event. V16 had already added the ordered stream with text identity; no older
root can be relabeled as v21 evidence.

Consumers call `ReleaseGateReport::verify_integrity()` after deserializing a
retained JSON artifact. `require_closed()` performs that verification first,
so a mutated scenario, input hash, digest, observation descriptor, or ledger
cannot be accepted with a stale report SHA. Verification also rejects a stale
pre-v21 artifact root and contradictory closure states: unexercised means zero
observations and events, zero-unsupported requires a positive count and no
events, and unsupported requires a positive count covering a nonempty event
list.

Schema v21 retains `execution.unsupported-event-source` as a twelfth mandatory
path and the host-owned machine-readable observation geometry introduced by
v6. Its DeviceState channel retains schema v7's compact guest register projection,
PI timing-policy identity, pending PI/SI/AI/SP/RCP work and exact scheduled
event ordering, the complete VI register file and epoch, PIF RAM, RSP DMEM and
IMEM, SP DMA/register/semaphore state, and installed cartridge-save bytes plus
pending EEPROM programming. V8 adds all four PIF port identities, inputs, and
rumble latches; all four retained Controller Pak, Transfer Pak, and VRU slots;
each Controller Pak's authoritative raw image plus a strictly derived decoded
note projection; v12 additionally binds its physical bank count, 16-bit note
page chains, and active bank latch; inserted Game Boy
ROM/RAM, mapper, RTC, and guest-clock state; high-level VI manager and
compatibility-retrace state; and ABI-owned PI/SI completion and VI-latch
metadata. The DeviceState v7 wire additionally binds the future-affecting raw
VRU initialization-sequence position, the authoritative loaded-RSP-task token,
and sorted yielded-task lineage with original headers and exact microcode-data
identities plus its `Running`/`ResumeAuthorized`/`ResumeLoaded` lifecycle
phase. Host pointer values are excluded while
their validated one-RDRAM length and guest-visible delivery metadata are bound. A field-family
perturbation sweep and explicit collision regressions ensure those
future-affecting states cannot retain the same DeviceState digest. V9 adds
canonical executor thread/run/queue/timer/event/CP0 control state and the
complete modeled ABI HostState: Flash command state, overlay registry, retained
rspboot images, installed-ROM identity, guest handles/IDs, RDRAM registration
geometry, and debug-hardware selection. With the `recomp-rs` feature, it also
binds function/block program identity, every opaque runner/dispatch/builder
artifact identity, block code, budget, active generations, and pending
executable writes. V10 replaces v9's ambiguous no-typed-Rust-program tag with
four collision-tested classes: explicitly no executable program, unidentified
native program, identified native archive, and typed Rust. Live release capture
rejects the unidentified-native class. Native coroutine continuations and
callable pointers remain excluded. Append-only save and controller-operation
histories are also excluded from DeviceState v7 because they cannot affect a
future device result; their typed observations instead enter the canonical v21
closure ledger and therefore the report SHA. V18 likewise keeps historical
execution outside DeviceState. Identified native archives retain guest cycle,
section index, function offset, and link VRAM from the injected first-body-entry
hook. Typed whole-function execution retains guest cycle, link VRAM, and symbol
under the installed generated-artifact identity. Typed `BlockProgram` execution
retains bank, PC, and runner-artifact identity at the admitted runner-entry
boundary. All three retain exact order plus a
canonical unique set/count summary and independent digests. Resolution probes,
holes, failed destinations, and host calls do not count. A no-program fixture
must have an empty stream. Unidentified native programs, typed function lanes
without a regenerated entry-observation schema marker, cross-lane entries, missing block-runner identity,
future cycle-stamped entries, and any entry appended after the committed
boundary fail closed. The v21 RSP/RDP stream is likewise append-only release
observation rather than future-affecting DeviceState. Previously retained v7
through v20 reports remain historical evidence but are not valid inputs to the
v21 series verifier.
Regenerate each scenario's ten-report series; do not reinterpret or edit old
reports.

The typed whole-function emitter now places one observation call as the first
statement of every `emit_function_resolved` body. That shared template covers
root entry, direct sibling/tail calls, and lookup-resolved guest calls while
excluding host overrides and lookup misses. The ABI binds each ordered entry to
the producer-supplied artifact identity, stable `(link VRAM, symbol)` identity,
and current guest cycle. Artifact identity alone is insufficient: authoritative
installation must consume the regenerated artifact's exported
`FN64_FUNCTION_ENTRY_OBSERVATION_SCHEMA` marker. The committed-boundary freeze,
v21 report, private admission, and paired verifier consume that stream as the
distinct `typed_observed_function` lane. The stale `typed_function` label and
identity-only installations remain rejected.

Its report source is
`{"kind":"typed_observed_function_program","artifact_sha256":"…"}`.
Each ordered/unique destination is
`{"lane":"typed_function","vram":…,"symbol":"…"}`; ordered entries carry
their exact `guest_cycle`. Symbol length and bytes participate in the
domain-separated v2 destination wire, so equal VRAM with different symbols
remains collision-safe. Capture compares every observation's artifact identity
with the frozen function-program identity and rejects a mismatch.

For the OoT C host, `IdentifiedNativeArchive` is the domain-separated SHA-256
of the exact `oot_recompiled` and `oot_bridge` static-archive bytes in stable
logical-label order. That binds the generated machine-code members and the
compiled section-table bridge actually passed to Cargo's linker, including any
compiler, archiver, target-policy, member-order, or embedded-metadata change
that alters either archive. Filesystem paths, mtimes, and host pointers are not
separate digest fields; if a tool embeds them into an archive, the resulting
byte change intentionally moves the identity. The raw archive bytes and paths
never enter a report. A host-supplied digest is identity evidence, not proof
that the archive's callable bodies are complete or correct; callable-body
authority remains a separate parity precondition.

## What the minimum ledger proves

The live ledger contains these observation classes:

- CPU thread switches and OS message-queue operations;
- separate PI-byte-commit, SI-byte-commit, AI-DMA-completion, and synchronous
  SP-task-load-commit observations from the device fabric;
- graphics and audio RSP task submissions;
- nonempty current-VI framebuffer, AI PCM, and RDRAM observations;
- a typed unsupported-event source observed through the fixed-cycle gate.

The source inventory is checked by
`tools/check_unsupported_event_sites.py` against
`fn64.unsupported-event-sites.v2`. It scans the production Rust paths in the
runtime, ABI, audio/RSP, Rust CPU recompiler, shared renderer, and RT64 adapter.
Twenty exact record sites, the reference renderer's 43 literal
helper-routed operation identities, and the ABI's two command-indexed SI
operation families must remain registered. Balanced multiline macro scanning
and inline `cfg(test)` removal ensure test-oracle panics neither create false
coverage nor hide a production outcome. Audio/RSP unknown instructions, missing VU
bodies, invalid control flow/IMEM exits, the explicit audio-ucode stub,
translated CPU gaps and terminal unresolved execution, and reference-renderer
command/state/precision rejections all flush an event before preserving their
existing returned error or loud trap. The same sweep rejects typed
`Err(...::Unsupported*)` construction without a nearby recorder; unsupported
Transfer Pak metadata/cartridge attachment and a renderer backend's unlisted
microcode rejection therefore enter the source before preserving their returned
error or loud trap. The registry's sorted observable operation/subsystem/
disposition wire generates the checked-in `fn64.unsupported-instrumentation.v1`
identity; source paths and triggers remain checker linkage rather than changing
that behavioral identity when code is only relocated.

Zero unsupported events is an execution-coverage claim, not a silicon-exactness
claim. Deliberately bounded arithmetic/noise/VI policies remain visible in the
base-renderer matrix and do not masquerade as newly reached terminal
unsupported operations.

The gate can additionally derive these optional successful-operation paths:

- `save.eeprom-4k-operation` and `save.eeprom-16k-operation` from completed
  high-level or raw Joybus EEPROM reads and matured programming commits;
- `save.sram-operation` after a timed 32-KiB domain-2 PI DMA commits;
- `save.flashram-operation` after a FlashRAM read, page write, or erase
  changes or reads the authoritative backing store;
- `save.pfs-operation` after `osPfsReadWriteFile` or a raw lower-half Joybus
  block command successfully reads or writes the authoritative Controller Pak
  image;
- `controller.standard-input-read` after a present standard controller's
  high-level or raw Joybus input data reaches the guest;
- `controller.rumble-operation` after a high-level or raw motor command
  reaches the typed Rumble Pak latch;
- `controller.transfer-pak-operation` after a high-level or raw Transfer Pak
  read/write succeeds;
- `controller.voice-operation` after a supported high-level or raw Voice
  read/write/control operation succeeds.

Controller/accessory identity queries and probes, rejected requests, zero-byte
operations, Flash write-buffer staging, Controller Pak upper-half reads, and
bank-latch writes do not count. A single
`osEepromWrite` returns while programming is pending and emits no committed
event until device time reaches its exact deadline. PiDma records that matured
write even when a later raw Info query, high-level wait, or unrelated device
advance materializes the lazy commit. Raw and high-level EEPROM paths therefore
share one storage-boundary history instead of relying on shim-name inference.
That history is append-only for the lifetime of an installed ROM, including
an idle host-side save-store replacement. Installing a new ROM starts a new
release-observation lifetime and clears both save and controller/accessory
operation history.
Storage owners drain into that unified history at each host boundary, so
same-cycle PI commits, SI channel operations, and synchronous shim operations
retain execution order; an undrained storage-owner history is a loud invariant
failure rather than a best-effort merge with unknowable order.

The live gate derives those entries from captured bytes and typed trace events;
its host cannot mark them covered by declaration, and the schema-v21 report
factory is crate-private so external callers cannot bypass the typed live
capture methods accidentally. `unexercised` means no corresponding
observation reached the gate. `exercised_zero_unsupported` means the path ran
and no loud unsupported trap terminated that scenario before the report.
`exercised_unsupported` binds the reached subsystem, operation, context,
optional guest cycle, and disposition (`loud_trap`, `returned_error`, or
`needs_lle`). An unknown cycle remains explicit; ABI-owned sites supply the
live device cycle.

This is a **minimum scenario closure**, not full-runtime or subsystem semantic
closure. The legacy executor `TraceKind::Dma` has no device identity or commit
phase and therefore satisfies none of the device-qualified paths. The live
gate instead copies `DeviceFabric::trace()` through
`fn64_abi::copy_device_trace()`: `PiBytesCommitted`, `SiBytesCommitted`,
`AiDmaComplete`, and `SpTaskAdmitted` are the only events that count. The last
event is emitted after `osSpTaskLoad`'s documented task-header/rspboot
DMA-and-poll loops have committed DMEM/IMEM; it does not claim that the
separate raw timed SP-DMA queue ran. Raw SP starts/queues/commits remain hashed
when present. Other start/queued events do not claim completed work.
VI retrace/presentation is not mislabeled as DMA; the separate live
framebuffer observation covers the minimum VI path. The optional save and
controller paths prove successful device-qualified operations, not filesystem
content, durability across process restart, every raw PIF command, or complete
semantics for those device families.

Arming writes and flushes an unsupported journal header before boot. Every
typed event is appended and flushed immediately. Journal v3 binds a canonical,
caller-supplied run-event SHA-256 in that armed header, then writes its
completion record only after the fixed-cycle report itself is durable and
binds the exact guest cycle, the report's `report_sha256`, and the same run
identity. A closed v21 report is release evidence only when paired with that
terminal v3 journal. A
journal with events but no completion identifies a reached loud trap; an
armed-only journal identifies an early abort or otherwise unobserved path; and
a stale same-cycle journal fails its report-SHA binding. Journals v1 and v2
remain parseable as historical diagnostics, but neither binds the run identity
required by current release evidence. Recording never catches a panic, changes a
returned error, or suppresses a `NeedsLle` handoff. Avoided and unreachable
destinations are still not enumerated.

## OoT private-host path

`examples/oot-boot` accepts the generic runner-owned tuple:

```text
FN64_RELEASE_GATE_CYCLE=C
FN64_RELEASE_REPORT=/private/path/report.json
FN64_RELEASE_RUN_EVENT_SHA256=LOWERCASE_SHA256_FROM_THE_RUNNER_EVENT
```

For manual compatibility it also accepts the historical OoT aliases as one
complete, unmixed tuple:

```text
OOT_RELEASE_GATE_CYCLE=C
OOT_RELEASE_REPORT=/private/path/report.json
OOT_RELEASE_RUN_EVENT_SHA256=LOWERCASE_SHA256_FROM_THE_RUNNER_EVENT
```

The host also writes `report.unsupported.jsonl` beside `report.json`. Preserve
it with failed runs: its absent completion line is the evidence that must not
be mislabeled as a zero-event report.

When timing-policy changes make an old cycle non-quiescent, the same host can
discover the first real scheduler drain boundary at or after a floor:

```text
OOT_RELEASE_DISCOVER_QUIESCENT_AFTER=C
```

Discovery is diagnostics-only: it retains the executor/device traces, prints
the exact `GuestDrain::AdvanceField` cycle and their final eight events, and
exits successfully without writing a release report. It rejects either
`OOT_RELEASE_GATE_CYCLE` or `OOT_RELEASE_REPORT` in the same process. A bounded
run that stops before finding a boundary fails loudly. The printed cycle must
then be verified in a separate fixed-cycle release run; discovery output is
never release evidence by itself.

RT64 release cycles use the stricter presentation-aligned finder:

```text
FN64_RENDER=rt64
OOT_RELEASE_DISCOVER_PRESENTATION_AFTER=C
```

It enables the same clean-source post-VI capture seam as the release gate and
exits successfully only immediately after a host field advance when RT64 has a
completed presentation tagged with that exact guest cycle. A stale prior image,
an instruction checkpoint that happens to reach the floor, and a zero-delta
host call are not matches. Presentation discovery conflicts with quiescence
discovery and both release-report variables, and cannot itself write a report.

The gate must arm before boot with guest time, executor trace, device trace,
save-operation trace, and controller-operation trace all empty. The host drives
its normal guest-quiescence
loop through the device fabric's exact scheduled VI deadlines; it never
reconstructs one from an older host tick and never clamps to an arbitrary gate
cycle. `C` must equal an authoritative VI edge or the run fails. Evidence is
captured only on the explicit `advance_virtual_time(C)` edge, after due
device/VI events are committed and before any newly woken guest thread runs.
That edge returns an opaque, single-use `CommittedViBoundary`; both live
capture methods require the witness and reject it if executor, device, save,
controller-operation, RSP/RDP observation history, or guest time advances
before capture. An always-on
monotonic coroutine-resume
epoch makes that proof independent of optional diagnostic tracing, including a
same-cycle resume with tracing disabled. Cycle equality alone cannot call the
live gate API. The boundary also freezes the DeviceFabric, aggregate
executor-peripheral/ABI-manager snapshots, supported host target, exact PIF
port identities, cartridge-save configuration, renderer identity/settings,
and graphics policy at the committed edge. Host-side
controller input or VI configuration performed before capture therefore cannot
move the DeviceState artifact away from that edge. Physical RDRAM, audio, and
runtime/device/operation histories are copied into the opaque boundary.
Reference pixels must equal the named range of that frozen RDRAM image.
An instruction checkpoint reaching `C` never captures a report; a later step
past `C` fails loudly. A step limit, swap limit, or idle exit before `C` also
cannot return success without a report. This opaque-boundary rule is
independent of the schema-v21 wire shape and its geometry-bound encoding.

The cycle, report, and run-event variables are an inseparable triple. A
partial, mixed generic/OoT, non-Unicode, relative-report-path, or noncanonical
event identity fails before boot instead of silently selecting ordinary
reference execution. Unknown renderers while release or discovery mode is
active fail the same way.

The OoT host supplies presentation evidence from live state, not fixture buffers:

- the reference path copies framebuffer bytes in logical byte order from the
  current VI origin in the shared RDRAM allocation, using the harness's
  documented 320x240 RGBA16 observation geometry;
- the RT64 path requires clean-source backend identity, enables the opt-in
  post-VI capture before registration, and obtains the completed image through
  the backend-neutral release-capture seam. Its canonical framebuffer artifact
  binds the fn64 adapter-source SHA-256, clean upstream RT64 revision, overlay,
  platform-specific capture API, post-VI stage, exact VI-retrace guest cycle,
  dimensions, tight row bytes, BGRA8 format, nonzero completed workload ID,
  present ID, and pixels. The captured presentation cycle
  must equal the release cycle; a gate between VI presents cannot reuse the
  prior image;
- memory bytes come only from the boundary-owned complete physical eight-MiB
  RDRAM image in logical byte order.

Schema v21 binds both paths through one typed descriptor. Reference capture can
only construct physical-RDRAM RGBA16 evidence, RT64 capture can only construct
post-VI BGRA8 evidence, and both require a complete logical-byte observation of
physical eight-MiB RDRAM. The release-matrix verifier derives presentation
boundary from that source descriptor and rejects a renderer declaration that
cross-labels it; the `rt64_post_vi_capture` tag cannot promote a raw framebuffer.
RT64 certification additionally requires the exact canonical identity shape
with `adapter=fn64-render-rt64/rt64`, a lowercase `adapter_sha256`, a canonical
`source=git:<40 lowercase hex>`, `provenance=git-clean`, and the post-VI API
assigned to the report's host platform. Dirty, externally declared, synthetic,
malformed, wrong-platform API, unbound-adapter, or wrong-adapter identities
fail. The adapter digest covers fn64's Cargo/build/Rust/C++ adapter inputs plus
target and enabled features; it is provenance rather than compiler/binary
attestation.

The scenario name binds the recompiler lane, actual selected renderer (including
`reference-fallback`), and `LleAccuracy` graphics policy. Rust reports use
`oot-ntsc-1.0-headless-rs-{renderer}-lle-accuracy-minimum`. C reports use
`oot-ntsc-1.0-headless-c-legacy-{renderer}-lle-accuracy-observation` because
the known empty legacy C bodies make that lane non-authoritative for semantic
release claims.

The public repository contains no ROM content. A private run supplies the ROM,
generated functions, and (for the default feature set) private audio ucode at
build/run time. Reports and captures stay in `/tmp` or another private,
gitignored location. Under `AGENTS.md`, a deterministic closure claim requires
ten consecutive executions at the same cycle with the same semantic report SHA,
each carrying a distinct runner event identity, in addition to every declared
path being exercised.

Before a private Extended-GBI or full-ROM run, use the local-only contract in
`docs/PRIVATE-INPUT-ADMISSION.md`. It validates regular non-symlink files,
exact lengths and hashes, provenance labels, wire family, scenario policy, and
the release-matrix platform/controller/save/renderer vocabulary. Its emitted
readiness report is content-free and does not replace the private manifest,
the runtime microcode-recognition gate, or this release gate's ten reports.

During an observed invocation, the harness-owned
`run-private-release-series` process closes the fresh-process orchestration gap
for repository synthetic evidence. It accepts only an opaque verified
contract, creates one OS-random series nonce, launches exactly ten child
processes sequentially under `env_clear`, derives a distinct run-event identity
and report path for each child, executes a create-new exact copy of the verified
child image beside its original, seals the stage read-only, rehashes every bound
input and the stage before each spawn and after the series, and verifies each
report/journal pair before launching the next child. Production contracts also
produce separate create-new, read-only stages for the admitted microcode text
and data. The runner injects only those paths through reserved release
variables and revalidates both stages at every child boundary. The OoT host
opens and shape-checks them, while the ABI independently identifies F3DZEX2
from pinned RT64's larger raw text/data XXH3 prefixes at the live task
boundary. That software-parity identity is neither HLE nor public-family
credit. The runner then
writes one create-new, flushed and file-synced
`fn64.private-release-series-receipt.v1` binding the exact contract, runner and
child entry images, ten event/file/report identities, and the common semantic
report SHA-256. Retained evidence can be reverified from that receipt.

The receipt is a canonical self-hashed integrity record, not a signature or
operating-system process attestation. Later verification proves that the
retained files still match the recorded series and runner image; it cannot by
itself prove that the trusted runner originally performed the launches. A
release that needs transferable process-provenance evidence must retain an
external trusted CI/code-signing attestation over the receipt and runner image.
The local execution guarantee assumes no malicious same-UID writer can chmod
and replace the random staged contract, child, or microcode-pair paths between
validation and the operating-system open/spawn; the resolved system-Python
image is likewise trusted as OS-owned. This is the same explicit single-owner
boundary as private input admission, not a sandbox against the invoking user.

Raw JSON is not runner authority. Production loading first requires the
repository admission script to byte-match the copy embedded at runner build
time, then executes those embedded policy bytes directly through isolated
system Python while revalidating the v6 admission manifest, v5 readiness
report, typed build receipt, and v3 private contract.
A separate synthetic-only constructor accepts only fn64's fixed non-game
fixture, `NoProgram` source, and current test executable; arbitrary relabelled
input and all ROM purposes fail closed.

Production `full_rom` and `combined` contracts require a private
`fn64.release-program-build-receipt.v1`. It binds the exact child executable
and recomputes the report's expected execution source from one typed lane:
canonically labeled exact linked archives, the typed-observed-function
identity wire, or the typed-block pack plus expected live program SHA-256.
The receipt, admission verifier, and Rust loader require exactly one lane input
to equal the admitted `recompiled` descriptor and require the declared,
recomputed, contract, and report sources to agree. The receipt and every bound
file are reverified before the series, before each child, after the final
child, and during retained-series verification. This co-binds identities but
does not prove the child was compiled or linked from the lane input.

`materialize-release-program-build-receipt` is the supported create-new
producer for all three lanes. It measures the actual child and lane inputs,
derives rather than accepts the execution source, syncs the private JSON, and
reloads it through the production verifier. For the OoT function lane,
`write-function-identity-wire` publishes the exact source wire embedded by the
same private child build and first verifies that its SHA-256 equals the build's
typed-function artifact identity. The concrete commands live in
`PRIVATE-INPUT-ADMISSION.md`.

Build evidence does not stand in for runtime microcode kickoff identity. Each v21
production report must contain at least one individual recognized microcode event whose
text SHA-256, data length, and data SHA-256 equal the admitted
`microcode_text`/`microcode_data` pair and whose family is present. Split
matches across different events fail. Pinned raw-window classification supplies
the family when available and traps on a contradictory backend catalog label;
text-only HLE recognition cannot populate it. The ABI hashes logical RDRAM data bytes
at authoritative task start; replacement IMEM generations retain that original
identity, and a one-way typed lifecycle retires ordinary completion while
making each public yielded-resume authorization load-consumable exactly once.
Retained v21 validation requires every task address to name a complete 64-byte
header inside physical 8 MiB RDRAM, every nonempty microcode-data and DRAM-DPC
range to fit there, and every XBUS-DPC range to fit the 4 KiB DMEM bank.
These mechanisms make a valid contract launchable. Representative private NTSC
full-ROM exact-ten series for reference and RT64 LLE/post-VI completed and were
reverified locally on 2026-07-21; those results close this orchestration path
for two scenarios, not the remaining release-matrix denominator.

The current production loader specifically resolves and pins
`/usr/bin/python3`; Windows production admission therefore remains fail-closed
rather than certified.
Receipt re-verification also requires the current verifier executable to hash
to the exact runner image recorded by the receipt, so that binary is part of
the retained evidence set. `--print-contract-sha256` is only a canonical-wire
integrity diagnostic and cannot create the opaque runner authority.

Verify a retained run-ordered series with the harness-owned paired checker. It
revalidates each report's integrity and closed ledger, parses the corresponding
journal, requires a terminal v3 cycle/report-SHA/run-event binding, rejects a
run identity repeated anywhere in the series, and then compares the complete
semantic report digest. It also requires every live minimum path,
even though the generic report type can represent narrower scenario ledgers.
Every report argument is immediately followed by its journal:

```text
cargo run -p fn64-boot-harness --bin verify-release-evidence-series -- \
  --program-lane typed_observed_function \
  /private/path/report-01.json /private/path/report-01.unsupported.jsonl \
  ... \
  /private/path/report-10.json /private/path/report-10.unsupported.jsonl
```

The required `--program-lane` value must match the content-free readiness
report emitted before the run. The checker compares it with every v21
report's `execution_destinations.source`; stale `typed_function` and
`unidentified_native` are rejected with the exact remediation before any
report pair is accepted, and a source mismatch says to rerun rather than
relabel retained evidence. Public no-program fixtures select
`no_program_fixture`; native observation series select
`identified_native_archive`.

The checker rejects duplicate report paths, duplicate journal paths, and a
path used as both halves of a pair, so one retained file cannot be repeated to
satisfy the deterministic bar. It also rejects copied pairs by their duplicate
run-event identity. For manually assembled input this identity is provenance
supplied by the caller and does not prove that the operating system created a
physically distinct process. An observed trusted-runner invocation guarantees
ten direct sequential launches; its retained receipt proves only integrity and
semantic binding unless an external trusted execution attestation covers it.
Retain one distinct pair per invocation and pass pairs in execution order. The
`verify-release-series` compatibility command now takes the same report/journal
pairs; there is no report-only release verifier.

## Public synthetic end-to-end mechanism check

`synthetic_fixed_cycle_release` uses only repository-defined non-game bytes.
It runs a real executor coroutine and message queue, commits timed PI, SI, and
AI work, admits graphics and audio SP tasks, renders and presents a raw RDP
fixture through the reference backend, commits the exact scheduled VI edge,
and captures all five schema-v21 channels plus the ABI-owned RSP/RDP stream.
One invocation writes one report and
one bound v3 journal. The runner must generate a fresh canonical event identity
before launch and pass it as the third argument:

```text
cargo run -p fn64-render-rt64 --example synthetic_fixed_cycle_release -- \
  /tmp/fn64-synthetic-01.json /tmp/fn64-synthetic-01.unsupported.jsonl \
  RUN_EVENT_SHA256
```

Ten independent invocations can be passed to
`verify-release-evidence-series`. This proves the public mechanism and its
runtime/device/render boundaries are deterministic; it is not a full-ROM,
microcode-family, save-medium, controller/accessory, or platform certification.
The automated trusted-runner form is:

```text
cargo test -p fn64-render-rt64 --test private_release_runner
```

Its parent test constructs only a repository-owned `synthetic_mechanism`
contract, then verifies the receipt and all ten retained report/journal pairs.
On 2026-07-20 this integration test passed ten consecutive parent processes:
100 fresh children completed the live runtime/device/RSP/RDP/VI/reference-
render path, with every exact-ten receipt accepted. That retained run is v16
historical mechanism evidence; the v19-retained same-event microcode-data extension
requires a new exact-ten run. Both are synthetic and cannot satisfy a ROM row.

The schema-v7 complete-fabric change passed ten independent processes on
2026-07-20. All ten bound v2 journals verified at guest cycle `1562500`.
Their exact digests are historical manual evidence, not tested by the current
schema, and therefore are not release gates. This is mechanism evidence over
repository-defined bytes, not ROM evidence or a schema-v8 series.

The schema-v8 aggregate peripheral and committed-boundary change passed ten
fresh independent processes on 2026-07-20. All ten distinct report/journal
pairs verified at guest cycle `1562500`. Their exact digests are historical
manual evidence, not tested by the current schema, and therefore are not
release gates. This closes the historical schema-v8 public synthetic
end-to-end mechanism bar only. It is not private-ROM, representative-matrix,
hardware-timing, or whole-runtime state evidence.

The schema-v9 executor-control, complete modeled ABI HostState, and typed
program-envelope change passed ten fresh independent processes on 2026-07-20.
All ten distinct report/journal pairs verified at guest cycle `1562500`.
Their exact digests are historical manual evidence, not tested by the current
schema, and therefore are not release gates. This is public synthetic
mechanism evidence only. The run used the explicit no-typed-Rust-program tag,
so it does not certify a private function/block artifact, a C callable archive,
representative ROM coverage, hardware timing, or native coroutine continuation
equality.

The schema-v10 native-program classification and exact-archive identity change
passed ten fresh feature-enabled `fn64-boot-harness` library-test processes on
2026-07-20 (82/82 tests in each process). That validates canonical separation
of no-program, unidentified-native, identified-native, and typed-Rust states;
identity mutation/collision sensitivity; and the legacy API's fail-closed live
capture. Ten additional independent public synthetic processes produced ten
distinct report/journal pairs, all verified at guest cycle `1562500`. Their
exact digests are historical manual evidence, not tested by the current schema,
and therefore are not release gates. The synthetic series explicitly declared
`NoProgram`; it does not certify a private native archive, typed-Rust program,
representative ROM, hardware timing, native continuation, or full-ROM
unsupported-event denominator.

The repository also carries an opt-in native/C archive mechanism fixture:

```text
cargo run -p fn64-render-rt64 \
  --features synthetic-native-archive-evidence \
  --example synthetic_fixed_cycle_release -- \
  /tmp/fn64-native-synthetic-01.json \
  /tmp/fn64-native-synthetic-01.unsupported.jsonl \
  RUN_EVENT_SHA256
```

This feature does not enable or build RT64. Before the RT64 build branch, it
compiles two tiny repository-owned C translation units into separate static
archives. The build hashes the exact produced archive bytes under stable
`synthetic-generated-code` and `synthetic-section-bridge` labels. The example
embeds those same archive bytes, recomputes the identity, proves that flipping
one produced byte in either archive changes it, calls one linked symbol from
each archive, and commits `IdentifiedNativeArchive` at the VI edge. Filesystem
paths do not enter the archive identity or report.

Ten independent feature-enabled processes on 2026-07-20 produced ten distinct
report/journal pairs. The paired series verifier accepted all ten at guest
cycle `1562500` under scenario
`synthetic-native-archive-runtime-device-render-fixed-cycle-v1`. Their exact
digests are historical manual evidence, not tested by the current schema, and
therefore are not release gates. Together with the schema-v10 program-family
collision sweep, this proves the native archive build/link/identity/capture
mechanism over exact public fixture bytes. It does not certify a real generated-C
archive, callable-body completeness, a private ROM, representative execution,
or full-ROM unsupported-event closure.

Schema v11 added environment evidence derived exclusively from owners frozen
at the committed VI boundary: supported host target, exact four-port PIF
configuration, typed cartridge-save configuration, graphics execution policy,
and renderer self-report. The report wire has canonical collision sweeps for
every environment field, and unidentified save/backend registrations, HLE
policy, nonauthoritative RT64 identity, or observation mismatch fail closed.
On 2026-07-20, ten fresh default synthetic processes and their ten distinct v2
journals verified at guest cycle `1562500`. Ten fresh identified-native
synthetic processes independently verified at the same boundary. Their exact
digests are historical manual evidence, not tested by the current schema, and
therefore are not release gates. Both series used repository-defined bytes, a
reference LLE backend, one standard controller on port zero, no cartridge save,
and the build host's supported platform. They prove the v11 mechanism, not
private-ROM or RT64 GPU coverage.

Schema v12 added Controller Pak physical bank count, 16-bit cross-bank note
chains, and the future-affecting active bank latch to DeviceState. The focused
wire perturbation sweep covers each new field. On 2026-07-20, ten consecutive
fresh focused processes passed the bank geometry, raw SI selection, high-level
PFS capacity, and DeviceState collision tests. Ten additional fresh public
synthetic processes produced distinct report/journal pairs; the paired verifier
accepted all ten at guest cycle `1562500` under scenario
`synthetic-runtime-device-render-fixed-cycle-v1`. Their exact digests are
historical manual evidence, not tested by the current schema, and therefore are
not release gates. This closed the now-historical schema-v12 public mechanism
bar only; v13 required regenerated native/block execution destinations, v15
required regenerated observed typed-function evidence, v16 required the
ordered RSP/RDP stream, and v17 requires same-event microcode text/data
identity. Neither series certifies a private ROM or
representative banked-accessory execution.

## Representative release matrix

`fn64-boot-harness` accepts a bounded `fn64.release-matrix.v5` evidence
manifest. V5 removes the caller-authored `required` denominator and every
scenario `coverage` label. Instead, the manifest must name the exact
project-owned `fn64.certification-profile.full-parity.v1` schema and its golden
definition SHA-256. That profile currently contains 162 non-shrinking
requirements: two real full-ROM classes, NTSC/PAL/MPAL, all six
program/renderer lane pairs, five save classes, five controller classes,
twelve public microcode families, three RSP/RDP execution mechanisms, six
platform/API targets, all 13 RT64 cases on each target, and closure of all seven
blockers on each target. Synthetic fixtures are mechanism evidence and cannot
satisfy a real full-ROM class.

Each of at most 64 scenario declarations binds only a stable diagnostic ID,
one exact schema-v21 `report_scenario`, private-input SHA-256, report SHA-256,
and a canonical v5 `declaration_sha256` over those identities. The verifier
first validates every report, then routes it by the report's own `scenario`
value, which must match exactly one manifest declaration; command-line IDs
cannot assign evidence to a different declaration. Platform, all four PIF port
identities, cartridge-save hardware, renderer/presentation mode, and executable
program lane are derived as scenario coverage from the committed-boundary
report. The manifest cannot cross-label them. Committed DRAM-DPC, XBUS-DPC,
and IMEM-replacement mechanisms are likewise derived solely from the validated
v20 stream. A backend-recognized family remains diagnostic/optimization
evidence. Public-microcode credit instead requires the reported text digest to
match the immutable project-owned certified-public-microcode catalog v1; a
contradictory backend family is rejected. That catalog is currently empty
pending allowed-source digest provenance, so matrix v17 cannot yet satisfy any of the
twelve public-microcode requirements. The current FullParityV1 assignment pass
can satisfy program/renderer-lane, save, controller, and RSP/RDP mechanism
requirements. It also credits `macos-metal` or `linux-vulkan` only when a
validated RT64 report binds the matching concrete active API, authoritative
post-VI identity, and host platform. Scenario labels, reference rendering, and
coarse platform coverage cannot manufacture that credit. A Windows v21 report
derives exactly one of the four versioned Windows targets only when its native
build-derived family and observed D3D12/Vulkan API agree. No positive Windows
report is retained or claimed by this mechanism work.

Derived EEPROM, SRAM, and FlashRAM coverage still requires the corresponding
positive device-qualified save path. Derived controller coverage likewise
requires standard input, Rumble, Transfer Pak, Voice, or PFS operation as
applicable; a controller with an accessory projects both
`standard_controller` and that accessory. RT64 evidence requires the
authoritative clean fn64 adapter identity, matching post-VI settings identity,
LLE-accuracy policy, and exact post-VI capture. Every scenario still requires
exactly ten schema-v21 reports, each paired with its terminal v3 journal and a
globally unique run-event identity, while proving all five fixed-cycle
artifacts, every live-minimum path, and zero reached unsupported events.

Schema v21 exposes normalized ROM identity, a host-supplied typed ROM class,
and decoded TV region. A fixed NTSC/PAL/MPAL header earns TV-region coverage
only after the report has also proved agreement with the boundary-frozen device
and renderer TV standards; a region-free header earns no regional credit.
Generic reports retain ROM class as evidence but cannot award ROM-class
coverage, because only a verified private series can establish that class
authoritatively. That path accepts an opaque `VerifiedPrivateReleaseSeries`
created only after jointly revalidating the admitted contract, exact-ten
receipt, retained reports/journals, independently decoded raw ROM, exact runner
image, bound files, and program-build receipt. Its semantic report and ordered
runner-derived event identities must equal the matrix evidence. It retains
`fn64.verified-rom-class-authority.v1`; duplicate, unused, relabeled, or
receipt-detached authorities fail closed. The retained authority digest is
local integrity evidence, not a signature or transferable process attestation.
The Unix test `exported_private_series_matrix_path_admits_public_fixture_and_rejects_tamper`
exercises that exported path end to end with a generated, non-game public
homebrew-shaped fixture: the repository Python policy emits the production
contract, a typed-block build receipt binds the child, the trusted runner
retains ten fresh processes, and the opaque series earns only its exact
fixture ROM-class row. Reordered supplied run events and a changed retained
report both fail. The child's schema-v21 report template makes this authority-
plumbing evidence, not representative-ROM, runtime, renderer, or microcode
behavioral evidence; its `Other` microcode identity cannot enter the empty
certified-public-microcode catalog.

RT64 target-case credit has a separate opaque
`VerifiedRt64PlatformCaseSeries` boundary. Its retained projection binds the
exact v21 report scenario and semantic report SHA, exact ordered matrix run
events, native host identity, observed graphics/capture API, pinned RT64 and
adapter identities, child identity, case semantic digest, and the case's exact
10- or 20-run event set. Duplicate, unused, relabeled, report-detached, or
run-order-detached capabilities fail closed. Retained/self-hashed JSON cannot
enter matrix construction as authority, and phase one deliberately exposes no
production constructor for the opaque capability. Therefore no target-case
credit or positive Windows evidence is claimed yet.

Schema v21 still cannot expose blocker closure. Valid v21 evidence
therefore returns a typed `Incomplete` assessment listing the exact
unsatisfied project-owned requirements; it never emits a smaller passing
denominator. Allowed-source identities in a successor certified-public-
microcode catalog, Windows-version evidence, external platform/blocker results,
and representative verified-series ROM-class assignments are required before a
`Complete` v17 retained matrix is reachable.

A minimal two-scenario shape is:

```json
{
  "schema": "fn64.release-matrix.v5",
  "profile": {
    "schema": "fn64.certification-profile.full-parity.v1",
    "definition_sha256": "<FULL_PARITY_V1_DEFINITION_SHA256>"
  },
  "scenarios": [
    {
      "id": "game-a-reference",
      "report_scenario": "game-a-macos-reference-lle-accuracy",
      "input_sha256": "<64 lowercase hexadecimal characters>",
      "report_sha256": "<64 lowercase hexadecimal characters>",
      "declaration_sha256": "<canonical 64-character scenario declaration SHA>"
    },
    {
      "id": "game-b-rt64",
      "report_scenario": "game-b-macos-rt64-lle-accuracy",
      "input_sha256": "<64 lowercase hexadecimal characters>",
      "report_sha256": "<64 lowercase hexadecimal characters>",
      "declaration_sha256": "<canonical 64-character scenario declaration SHA>"
    }
  ]
}
```

Replace the profile placeholder with the exact
`FULL_PARITY_V1_DEFINITION_SHA256` exported by `fn64-boot-harness`; manifest
verification rejects every other value.

While preparing a manifest, compute each canonical declaration digest (the
stored value is excluded from its own digest) with:

```text
cargo run -p fn64-boot-harness --bin verify-release-matrix -- \
  --print-declaration-digests /private/path/matrix.json
```

Copy those printed values into the corresponding `declaration_sha256` fields;
the normal verifier rejects any subsequent declaration change until the
digest is intentionally regenerated and cited.

Keep the populated manifest and every report/journal pair outside the repository when
their hashes or names disclose private game identity. The manifest contains
only declarations and digests, never ROM bytes, framebuffer/audio/RDRAM
bytes, or recompiled output. Assign each private report and journal together at
verification time; supply each report and journal together, in execution order.
The report's validated `scenario` field performs the assignment:

```text
cargo run -p fn64-boot-harness --bin verify-release-matrix -- \
  --private-series /private/path/game-a-contract.json /private/path/game-a-series /private/path/game-a-series/receipt.json /private/path/run-private-release-series \
  --private-series /private/path/game-b-contract.json /private/path/game-b-series /private/path/game-b-series/receipt.json /private/path/run-private-release-series \
  /private/path/matrix.json \
  /private/path/game-a-01.json,/private/path/game-a-01.unsupported.jsonl \
  ... \
  /private/path/game-a-10.json,/private/path/game-a-10.unsupported.jsonl \
  /private/path/game-b-01.json,/private/path/game-b-01.unsupported.jsonl \
  ... \
  /private/path/game-b-10.json,/private/path/game-b-10.unsupported.jsonl
```

Use one repeatable `--private-series CONTRACT OUTPUT RECEIPT RUNNER` tuple for
every production scenario that should earn ROM-class credit. The exact runner
file is rehashed because a receipt digest alone cannot prove that identity.
Each tuple must authorize exactly one declared report scenario; duplicate or
unused series fail. Bare `--private-contract` is rejected. Omitting all series
tuples intentionally selects report-only verification and leaves the ROM-class
dimension empty.

Every report path and journal path must resolve to a distinct file, and the two
halves of a pair cannot name the same file. Private-series verification rechecks
the receipt's exact output-file hashes and contract/child/nonce-derived event
identities. It still cannot turn a self-hashed receipt into operating-system
process attestation; the observed runner invocation is the authority for the
ten fresh launches. The verifier rejects an event identity repeated within or
across matrix scenarios.

For a complete result, the default output is a human summary. Until all 162
profile requirements are proved, `--json` emits a tagged `incomplete` result
whose nested `fn64.release-matrix-incomplete.v6` assessment binds the manifest
and profile identities, verified counts, evidence-derived satisfied
assignments, the canonical missing requirement list, and its own SHA-256; the
command then exits nonzero. It is diagnostic evidence, not a verified release
artifact.

Only a complete profile emits a retained, machine-readable
`fn64.verified-release-matrix.v17` result. It contains the canonical manifest
and profile identities, every scenario's derived coverage, normalized ROM
evidence, optional verified-series ROM-class authority, and exact
input/report/declaration identity, the five fixed-cycle artifact
digests and byte lengths, the exact guest cycle, the canonical closure ledger
and its redundant path count, exact
boundary-frozen environment, the exact ordered and canonical unique/count
execution-destination evidence with both digests, the complete ordered RSP/RDP
stream and its digest, explicit zero unsupported-event count, v3
journal schema and binding count, the exact ordered run-event SHA-256 list, the typed
observation geometry (including RT64's nonzero completed workload and present
IDs), and whether its source proves a committed-VI physical framebuffer or
RT64's exact post-VI capture. A top-level verification SHA binds
that complete result; no ROM bytes, framebuffer, audio, RDRAM, or recompiled
bytes are serialized.

`--verify-json` accepts only that complete v17 artifact and does not treat its
self-digest as sufficient by itself. It
revalidates the retained semantic envelope: one to 64 scenarios and valid,
unique scenario/report identities,
lowercase SHA-256 fields, exactly ten reports, ten bound v3 journals, and ten
globally unique run-event identities per scenario, every fixed-cycle artifact and observation invariant, every positive
live-minimum closure path, and zero unsupported events. It also revalidates each
derived save/controller feature against its exact successful-operation path,
rejects operation paths outside the frozen environment (including PFS without
a Controller Pak), re-derives scenario coverage from the retained report,
enforces renderer combinations and exact program-lane agreement, proves that
every member of the immutable profile has a validated evidence assignment,
recomputes every declaration SHA, reconstructs each retained v21 report and
its report SHA, reconstructs the canonical manifest SHA, and re-derives any
ROM-class assignment only from its retained authority record. This standalone
check proves the artifact's canonical semantic integrity; without a signature
or external attestation root it does not prove who originally held the opaque
private capability.
A freshly re-digested empty, zero-path, relabeled, under-covered, cross-labeled,
or manifest-mismatched document is therefore rejected without the original
manifest. Historical retained JSON through v10 omits some part of the current
execution-destination, run-provenance, RT64 workload, or typed program-lane
envelope. V11 retains that envelope but lacks the fixed FullParityV1 profile
identity and requirement assignments; v12 lacks the retained ordered RSP/RDP
stream, same-event microcode data identity, and derived assignments. V13 lacks
the concrete active RT64 graphics API and its platform/API assignments. Every
historical version through v13 is
intentionally rejected; regenerate it from a v5 manifest and bound
report/journal pairs rather than relabeling it as v17. V14 lacks schema-v19 ROM
identity, decoded TV-region coverage, renderer TV-standard binding, and the
separately retained verified-series ROM-class authority, so it is intentionally
rejected. V15 lacks schema-v20 Windows host identity and retained opaque RT64
platform-case authorities. V16 lacks v21's boundary-owned observations and
unsupported-instrumentation identity. Both are intentionally rejected.

Keep populated results private when their scenario names or hashes disclose
game identity:

```text
cargo run -p fn64-boot-harness --bin verify-release-matrix -- \
  --json \
  --private-series /private/path/game-a-contract.json /private/path/game-a-series /private/path/game-a-series/receipt.json /private/path/run-private-release-series \
  --private-series /private/path/game-b-contract.json /private/path/game-b-series /private/path/game-b-series/receipt.json /private/path/run-private-release-series \
  /private/path/matrix.json \
  /private/path/game-a-01.json,/private/path/game-a-01.unsupported.jsonl \
  ... \
  > /private/tmp/verified-matrix.json

cargo run -p fn64-boot-harness --bin verify-release-matrix -- \
  --verify-json /private/tmp/verified-matrix.json
```

## Current certification census

As of this document revision, **no representative full-ROM class has a
certified release matrix**. The exact state is:

| ROM/report class | Mechanism available | Certified evidence retained |
| --- | --- | --- |
| Synthetic fixtures and end-to-end runner | Five-channel fixed-cycle reports, v3 report/journal/run-event binding, a trusted exact-ten fresh-process runner and receipt, real executor/device/RSP/RDP/VI/reference-render boundaries, derived matrix coverage, and the canonical incomplete assessment are available. | Mechanism evidence only; synthetic bytes are not a ROM certification, and an incomplete assessment is not a retained verified matrix. |
| OoT NTSC 1.0, Rust lane, reference LLE | Private host wiring, committed-VI capture, complete-RDRAM observation, an explicit source-hash-bound `BlockProgram` host-selection seam, an artifact/schema-bound whole-function entry stream, a create-new receipt/source-wire producer, runner-staged exact ROM/microcode-pair admission, and same-event kickoff check exist. The v21 report additionally owns memory/audio/trace bytes at the boundary and binds the unsupported-instrumentation denominator. | The 2026-07-21 exact-ten series is schema-v20 historical evidence. It must be rerun under v21 before current release credit is claimed. |
| OoT NTSC 1.0, Rust lane, RT64 LLE/post-VI | Exact-cycle presentation discovery, workload/present-bound v3 post-VI envelope, resolved graphics-API and TV-standard evidence, explicit program identity, and runner-staged exact ROM/microcode-pair admission exist. | The 2026-07-21 pinned-Metal exact-ten series is schema-v20 historical evidence. It must be rerun under v21 before current release credit is claimed. |
| OoT NTSC 1.0, legacy C lane | Observation tooling and exact linked-archive identity wiring exist. | Non-authoritative: measured framebuffer parity is only claimed through swap 60, and the C oracle's missing bodies prevent deeper arbitration beyond the known swap-231 frontier. |
| Other Fast3D/F3DEX-family, S2DEX, regional, save-medium, controller/accessory, and platform ROM classes | Matrix v5 derives the schema-v21-visible fixed TV region, save, PFS, controller input, Rumble, Transfer Pak, Voice, renderer, program-lane, committed RSP/RDP-mechanism, and authoritative platform/API assignments while retaining the remaining project-owned profile entries as missing. Backend microcode labels are diagnostic only; independent public-microcode adjudication uses the empty project-owned catalog v1. Generic report verification deliberately cannot turn the retained ROM-class label into profile credit; the separate private-series path revalidates its contract, receipt, exact output files, raw ROM, and runner before retaining `fn64.verified-rom-class-authority.v1`. RT64 target-case credit additionally requires the opaque platform-series capability, for which phase one has no production constructor. | No public-microcode requirement can be credited until allowed-source identities populate a successor catalog; regional and additional save/controller/render scenarios, positive native Windows evidence, and production platform-case authority remain unsupplied. |

Historical schema-v20 joint verification over the two representative NTSC
scenarios revalidated 20 reports and satisfied exactly 9 of 162 FullParityV1
requirements: `retail_cartridge`, `ntsc`,
`typed_observed_function/reference_lle_accuracy`,
`typed_observed_function/rt64_lle_accuracy`, `sram_32_kib`,
`standard_controller`, `dram-dpc`, `imem-replacement`, and `macos-metal`.
The other 153 remained explicit in that historical incomplete assessment;
schema-v21 has not yet been run on those private inputs.

Therefore the generic report mechanism can validate multi-ROM
zero-reached-unsupported evidence and retain its satisfied/missing profile
partition in an incomplete assessment, but a complete retained matrix remains
blocked on private ROM/recompiled inputs, ten independent fixed-cycle
executions per scenario, and the missing typed evidence classes above. The
matrix still proves only reached paths: it cannot enumerate avoided unsupported
destinations or convert representative scenarios into exhaustive reachable-ROM
closure.

The exact program-input identity-co-binding and runtime microcode-pair kickoff
identity blockers are closed by the typed receipt plus the same-event
microcode text/data check. Trusted evidence that the child was compiled or
linked from those inputs remains open; the receipt alone is not build
attestation. The remaining evidence frontier is to feed the retained
representative series through the verified-series matrix path, add the other
required private exact-ten scenarios, and satisfy the still-missing
public-microcode-catalog, positive Windows, platform-case, and blocker-result
classes.

## Remaining release frontier

- Schema v21 aggregates the modeled device fabric, executor control,
  ABI HostState, typed-Rust program identity, and exact native/C linked-archive
  identity plus actual platform, four-port controller/accessory placement,
  typed cartridge-save configuration, and renderer identity/active API/policy at the
  committed VI edge. It remains deliberately narrower than a
  whole-runtime savestate: native coroutine continuation stacks are not
  canonical or portable, and archive identity does not prove callable-body
  completeness. Exact ordered native/typed-function/block execution
  destinations and the ordered RSP/RDP observation stream are bound, but equal
  v21 digests do not claim excluded
  continuation state is equal.
- Extend the typed unsupported-site registry whenever a new runtime, ABI, or
  renderer boundary is added; preserve failed-run journals with reports.
- Populate and verify a representative private release matrix across the
  declared platform, controller/accessory, save, and renderer policy. Save,
  PFS, controller input, Rumble, Transfer Pak, and Voice rows require matching
  operations; every configuration row is bound to actual boundary-frozen
  owner state.
- Populate a successor immutable certified-public-microcode catalog only from
  reviewed allowed-source digest provenance; backend runtime catalogs and
  readiness declarations cannot supply certification identities.
- Extend scenario coverage to the full reachable-ROM frontier; even a complete
  representative matrix cannot establish paths its selected scenarios do not
  reach.

The deterministic device deadlines in the digest remain the policies
documented in `UNIVERSAL-RUNTIME-PLAN.md`. Reproducing those policies does not
make them hardware-cycle measurements. Provenance is fn64's typed
`DeviceFabric`, shared trace contract, and live host state; no external runtime
implementation was consulted.
