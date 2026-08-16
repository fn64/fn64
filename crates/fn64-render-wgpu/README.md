# fn64-render-wgpu

`fn64-render-wgpu` is the pure-Rust GPU backend being built to replace the
quarantined RT64 C++ adapter. The current M3.1 surface is intentionally small:
it requests one headless native wgpu 30 device, creates one compute pipeline
before admission, and executes one exact 2x2 fill/FullSync mechanism fixture
from a move-only `fn64-render-ir::SubmittedTicket`.

The fixture is a lifecycle proof, not an RDP implementation. It accepts only a
single reviewed DRAM stream containing Set Color Image, Set Fill Color, Fill
Rectangle, and FullSync in that exact order, with one exact declared RDRAM
effect. The journal must contain only ordered operation 0 `Read/CommandDecode`
for the exact command range and operation 1 `Write/RenderTarget` for the exact
16-byte framebuffer range. Its observation timeline is exactly `CMD_END ->
FullSync -> DP interrupt`, and the fixed host evidence vector preserves RGBA
byte order as `21 3c 4d 59` for each of four pixels. Effect bytes use
render-IR's canonical digest, shared with the M1.2 guest-staging adapter. It
does not claim general raw-DPC decoding, fill-cycle arithmetic,
TMEM, framebuffer persistence, VI, surface presentation, RT64 parity, or
performance. Those remain M3.2 and later work in
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

Provenance: command fields use the public SGI *RDP Command Summary* and public
libultra `gDPSetColorImage`, `gDPSetFillColor`, and `gDPFillRectangle`
descriptions already cited by `fn64-render-ir` and the reference renderer.
The shader is a repository-owned mechanism fixture. No RT64 shader, C++, CMake,
DXC artifact, GPL runtime implementation, or game content is used here.
