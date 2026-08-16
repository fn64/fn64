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
