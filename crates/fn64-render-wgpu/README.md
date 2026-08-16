# fn64-render-wgpu

`fn64-render-wgpu` is the pure-Rust GPU backend being built to replace the
quarantined RT64 C++ adapter. The current M3.2 surface is intentionally small:
it decodes a bounded raw-DPC subset into ordered typed commands, a
transaction-local RDP state delta, and an exact resource plan. M3.1's headless
wgpu 30 fixture remains byte-for-byte frozen as a separate lifecycle bridge.

The decoder admits only the eight low no-op variants, fill-cycle Set Other
Modes, Set Color Image, Set Fill Color, whole-pixel Fill Rectangle, and
FullSync. The two non-command bits in the wire opcode are ignored for command
selection, and command stepping uses `fn64-render::raw_rdp_command_width`.
FullSync preserves its unassigned payload bits; the command identity and the
IR-owned completion observation are the admitted semantics.
Unsupported, unknown-width, truncated, and state-invalid commands fail with
workload, stream, chunk, source-byte-offset, and wire-opcode identity. A fill
requires transaction-local fill cycle, color image, and fill color state;
RGBA16/32 row ranges are planned exactly. The input journal must equal the
entire ordered plan, including operation identities. No durable state is
mutated: decode consumes the submitted ticket, and staged state is move-only,
queue-bound, submission-ordinal-bound, and exact-successor-sequence-bound.

M3.3a freezes the contract immediately after that decoder. Its only admitted
candidate is an exact synthetic 4x2 RGBA16 red fill: 8 MiB installed RDRAM,
commands at `0x100..0x128`, color writeback at `0x400..0x410`, transaction
sequence 7, and the exact ten command words exported by
`NATIVE_FILL_COMMAND_WORDS`. The logical/device RGBA16 result is `[f8 01] x8`.
The ABI transaction does not accept those logical bytes: its existing commit
loop flat-copies staged effects into N64Recomp's native-word RDRAM allocation,
so `N64RecompRdramStorageBytes` instead carries `[01 f8] x8`. `RdramView`
mechanically proves that backing storage reads as eight logical `0xf801`
pixels. The frozen backing-storage, native RGBA8, and post-VI BGRA8 SHA-256
values are respectively `007d65aa7365956d4ae38da6ee8849b14b7a5d88658adfb49df757255249f248`,
`5ed2cb747cf2014feda8638a6894704f15eb46c867ce7bae38d0447556f80549`,
and `f9d2bc2ea8345a97d8a514eae7f50c165175355a80ca805309429d83748f7ee2`.
The render-IR workload, raw-stream, and journal identities are also frozen as
`3d079907c20080a277ccee1344e6af9332828b3c520dd12d31f502bbf8d63c2c`,
`057b789d4989fe90faf753f8f6802db8aa64b94249dadffdda8e3a70ff4753d1`,
and `1206767d7c857d57832d88bb557a450d0e8f3fb331669e827316b676db83bc50`.

The linear ownership path is `DecodedRawDpc -> PreparedNativeFill ->
InFlightNativeFill -> PendingNativeCommit -> guest-owned commit ->
CommittedNativeFrame`. Preparation compares the decoder's retained predecessor
with an exclusively borrowed `NativeDurableState`. Backend modules alone may
advance prepared work or report GPU output. `DeviceRgba16Bytes` can never be
mistaken for `N64RecompRdramStorageBytes` at the guest boundary. Pending work
is assembled only from the separately named crate-private `from_device_bytes`
and `from_n64recomp_storage_bytes` constructors; the native-output constructor
accepts those types rather than adjacent raw vectors. It transfers its
`GpuCompleteTicket` and immutable backing-storage bytes through a guest-owner callback;
only the exact returned `GuestCommittedTicket` publishes the RDP delta, target
identity, generation, and last-commit lineage. Decode rejection, target/raster
failure, incorrect output, callback failure, a hostile receipt, or dropping any
pre-commit state leaves the prior renderer state unchanged. The renderer never
receives live guest-memory authority, interrupt/scheduler authority, or a guest
receipt-issuing capability.

`DecodedRawDpc` now has one crate-private consuming decomposition for this
path. It retains the immutable predecessor state as part of its proof, while
the public `into_staged_state` path remains a mutually exclusive speculative
decode choice. Native preparation rejects a decode produced from a consumed
speculative predecessor because that route cannot return the prior staged
token on later failure. It consumes a durable-origin decoded owner and checks
the predecessor, delta-derived staged state, queue, submission ordinal, and
transaction sequence against the exclusive durable owner. Consequently the
same submission cannot be sent both to native execution and staged chaining,
and a staged result cannot become durable before guest commit.

M3.3b adds CPU-only native color-target ownership beneath that contract.
Typed keys bind the installed-memory layout, physical range, extent, and
RGBA16/32 format. Exact row plans are distinct from move-only completed-write
capabilities; the latter bind target key, generation, full range, device-byte
domain, and exact byte count before a resident generation can be published.
The completed-write type intentionally has no production constructor yet, so
planning cannot masquerade as raster completion. The RGBA5551/RGBA32 CPU
pack/unpack oracles and the M3.3a `DeviceRgba16Bytes` narrowing seam are
executable mechanism evidence only.

This contract is deliberately not an implementation of GPU target allocation,
raster execution, guest writes, VI filtering, headless capture, or
presentation. It admits no depth image or depth write, no TMEM,
textures, blending, coverage, multisampling, ray tracing, surface path, or
performance/parity claim. Its byte-exact synthetic fixture is mechanism
evidence, not the required real captured workload. It consumes no M2.5 shader
artifact: a separately reviewed repository WGSL implementation may satisfy this
one mechanism later, while any RT64 HLSL corpus claim must wait for M2.5's
complete 56-artifact receipts.

The retained GPU fixture is a lifecycle proof, not a broad RDP implementation.
It continues to require the exact M3.1 eight-word DRAM stream: Set Color Image,
Set Fill Color, Fill Rectangle, and FullSync, with exact wire words apart from
the fixture's selected fill-color value and one exact 16-byte RDRAM effect. Its
FullSync remains at byte 24 and its observation timeline is exactly `CMD_END ->
FullSync -> DP interrupt`. The fixed host evidence vector preserves RGBA byte
order as `21 3c 4d 59` for each of four pixels. Effect bytes use render-IR's
canonical digest, shared with the M1.2 guest-staging adapter. This slice does not claim
TMEM, persistent framebuffer ownership, broad raster, VI, surface presentation,
RT64 parity, or performance. Those remain later work in
[`../../docs/RENDER-WGPU-PORT-PLAN.md`](../../docs/RENDER-WGPU-PORT-PLAN.md).

The lifecycle keeps the renderer's paired backend-completion authority private.
One in-flight type owns the semantic ticket and wgpu `SubmissionIndex`; it can
yield a completion only after an exact indexed wait, completion-callback
observation, bounded readback, and byte validation. Every output byte must be
covered by the packet's resource journal before the backend effect receipt is
issued. Dropping any earlier state cancels the synthetic operation without a
guest commit.

Ordinary tests are GPU-independent. The host test is explicit:

```sh
scripts/guarded-cargo-test.zsh -p fn64-render-wgpu --features host-gpu-tests
```

A machine with no selected native adapter returns the typed `NoAdapter`
outcome; that is unsupported host evidence, not a skipped or passing GPU
claim.

Provenance: command fields and fill-cycle rules use the public SGI *RDP Command
Summary* and the public libultra `gDPSetCycleType`, `gDPSetColorImage`,
`gDPSetFillColor`, `gDPFillRectangle`, and `gDPFullSync` descriptions. State
field interpretation follows the permitted MIT RT64 semantic source pinned by
the port plan; no RT64 code is copied. The shader is a repository-owned
mechanism fixture. No RT64 shader, C++, CMake, DXC artifact, GPL runtime
implementation, texture hasher, game content, or excluded tool is used here.
