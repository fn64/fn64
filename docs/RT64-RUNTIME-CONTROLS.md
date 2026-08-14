# RT64 runtime-control boundary

fn64 treats renderer availability and renderer policy as different things. A
compiled host may contain the RT64 adapter while a user changes presentation
policy at runtime; generated game code does not own those preferences. This
document fixes the complete public control denominator at the pinned MIT RT64
commit `f0728a2520d5aa735886240de3fee75cc805f6d6` so implementing one
configuration structure cannot be reported as implementing every runtime
control.

## Control families

| Family | Public surface | Application boundary | fn64 status |
|---|---|---|---|
| User rendering | The 19 fields in [`UserConfiguration`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_user_configuration.h#L86): API, resolution, buffering, MSAA, downsampling, filters, aspect, 2D scaling, refresh, color format, resolve, and developer/performance flags | [`updateUserConfig`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_application.cpp#L741); MSAA has a separate resource-rebuild path | Complete typed Rust/C/C++ seam and active-policy digest present; feature-specific behavioral certification remains open. |
| Enhancement policy | Eight fields in [`EnhancementConfiguration`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_enhancement_configuration.h#L10), including `Console`, `SkipBuffering`, and `PresentEarly` latency modes | [`updateEnhancementConfig`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_application.cpp#L749) | Complete typed Rust/C/C++ live seam and active-policy digest present. Twenty consecutive clean live Metal runs close PresentEarly process-time presentation and synchronization; the other advertised enhancement behaviors retain their own inventory rows. |
| Emulator/device policy | Four fields in [`EmulatorConfiguration`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_emulator_configuration.h#L10): post-blend noise choices and render-to-RAM/copy-with-GPU policy | [`updateEmulatorConfig`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_application.cpp#L745) | Complete typed Rust/C/C++ live seam and active-policy digest present. Ten clean Metal runs retain exact positive/negative noise, render-to-RAM, and exclusive GPU-tile-copy versus ordinary RDRAM/TMEM-upload mechanism evidence. |
| Texture replacements | Ordered directory/`.rtz` inputs, enable state, and [`ReplacementConfiguration`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_replacement_database.h#L58), including RT64/Rice name selection and preload/stream/stall policy | [`loadReplacementDirectories`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/render/rt64_texture_cache.cpp#L1462), reload, and enable/disable operations | Complete typed Rust/C/C++ seam and active-policy digest. Ten consecutive clean live Metal runs close DDS/mips, Rice naming, and held-queue fallback-to-final Stream behavior using a stable cleared baseline and identical evidence digests. |
| Host startup | [`ApplicationConfiguration`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/hle/rt64_application.h#L37): application/data paths and configuration-file policy | Consumed during application construction/setup | Host configuration, not generated game code and not a live setting. |
| Game cooperation | Extended-GBI activation and commands from [`rt64_extended_gbi.h`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/include/rt64_extended_gbi.h#L16) | Commands arrive in the game's display-list stream | Optional game-side cooperation; runtime settings cannot synthesize missing camera, interpolation, UI, or depth-test intent. |
| Build capability | Platform graphics APIs plus optional compiled subsystems such as `RT_ENABLED` and `SCRIPT_ENABLED` | Selected by the RT64/fn64 build | Cannot be enabled at runtime if absent from the binary. Metal backend initialization, raw-RDP submission, post-VI capture, synchronized resize, and recreation are closed by 20 consecutive clean live runs; macOS platform-wide support and the other graphics APIs retain their separate inventory rows. |

The counts intentionally describe fields, not advertised behavior claims. The
behavior denominator remains `RT64-PUBLIC-FEATURE-INVENTORY.md`; several
claims share a control and every claim still needs its own evidence.

## Mutation classes

Most `UserConfiguration` values propagate live. Resolution mode/manual scale
and aspect mode/manual target also discard cached framebuffers. MSAA is live
but synchronously rebuilds shader and framebuffer resources. Graphics API,
display buffering, and internal color format are setup-owned and require
backend recreation; fn64 reports that result explicitly rather than retaining
an apparently applied value.

The WM2000 interactive shell has a bounded experiment surface for these live
controls. F7 cycles native 1x, high-resolution 2x, high-resolution 2x with an
explicit 2x box downsample, and native 1x with MSAA4x. F6 reloads the complete
strict TOML image named by `FN64_RT64_SETTINGS_FILE`; the file is also applied
once at startup. The schema requires `resolution`, `resolution_multiplier`,
`downsample_multiplier`, and `antialiasing`, rejects unknown fields, and is
illustrated by `examples/wm2000-block-boot/rt64-aa.example.toml`. Each
successful mutation prints its complete settings digest and whether RT64
discarded framebuffer resources. These shortcuts are an experiment harness,
not the settings UI policy; they intentionally cross the same typed
registered-renderer seam that a later frontend uses.

An explicit headless diagnostic can additionally report the positive finite
workload scale and the concrete managed target's positive finite scale,
nonzero raster extent, and downsample multiplier after both RT64 workers are
idle. This evidence caught a former double origin compensation that sent
WM2000 presentation through RT64's native scratch upload and made every F7
resolution mode inert; the ordinary frame path does not pay for the diagnostic
wait.

The Metal user-control gate also isolates the four live fields that do not
intentionally discard framebuffer resources: Manual 72 Hz refresh targeting,
disabled hardware resolve, enabled idle work, and developer mode. Each phase
must retain its exact complete active-policy digest, advance presentation,
preserve the same nonzero source-resource identity, and reproduce the exact
MSAA4x post-VI pixels; restoring the prior policy after every phase must do the
same. This closes the local raw-DPC mutation/continuity mechanism only. It does
not establish physical refresh cadence, prove which resolve implementation a
driver selected, or replace recognized-HLE and cross-platform combination
coverage.

fn64 exposes complete enhancement and emulator structures through explicit
live update methods matching the pinned application. The default enhancement
profile is faithful/off and the distinct pinned upstream profile is named.
The `f3dex.forceBranch` control additionally has a ten-fresh-process causal
Metal differential: one false public F3DEX2 depth branch renders red with the
control off, green with it on, and exact red again after live restoration.
`RT64-FORCE-BRANCH-EVIDENCE.md` records its exact policy/pixel identities and
the production-recognition negative control. `textureLOD.scale` now has an
independent ten-fresh-process F3DEX2 differential at 2x resolution: off selects
the green lower mip, on selects the red base mip, and restoring off reproduces
the exact first-phase identity. `RT64-TEXTURE-LOD-EVIDENCE.md` records that
bounded synthetic result. The S2DEX framebuffer fast path has an independent
synthetic `G_BG_1CYC` mechanism differential: the live control coalesces three
ordered managed-framebuffer tile-copy loads to one while exact pixels and
target RDRAM remain unchanged, and restoration returns to the exact three-load
digest. An ordinary-RDRAM negative arm remains three CPU uploads with the fast
path enabled. `RT64-S2DEX-ENHANCEMENT-EVIDENCE.md` records the bounded result.
The same public `G_BG_1CYC` handler owns `s2dex.fixBilerpMismatch`. An inset
point/point, bilerp/point, and bilerp/bilerp matrix proves the exact mismatch
predicate: live enable changes only the mismatched three-load program to the
point two-load program, restoration returns to three, and the two matched
controls are invariant. The missing object-rectangle handlers are a separate
upstream command-coverage gap rather than this control's execution path.
Texture packs use load/reload/enable operations rather than
`updateUserConfig`. They are still runtime host controls, but their filesystem
identity and replacement database are part of the active policy and must be
represented in evidence.

`Rt64Backend::load_replacement_packs` accepts an ordered list of typed host
inputs. Before create it stages them; create re-inspects and loads that exact
order. After create it follows pinned RT64's full clear-and-load path.
`reload_replacement_packs` deliberately performs that same full clear and
load, matching the pinned inspector's reload action, while
`set_replacements_enabled` changes only `replacementMapEnabled`. Directory and
`.rtz` roots must exist, be unique after canonicalization, and use UTF-8 host
paths. Root or nested directory symlinks, special files, missing/empty
`rt64.json`, malformed databases, unsupported future hash versions, and
ambiguous `auto` defaults fail loudly. Pinned RT64's effective database image
supplies the RT64/Rice, preload/stream/stall, shift, configuration-version, and
hash-version fields; ordered per-texture/filter behavior remains bound by the
raw database SHA-256.

## Evidence rule

The fixed-cycle render artifact must hash the settings actually active for the
captured presentation. Pending recreate settings must not replace that digest.
The current typed digest composes `UserConfiguration`,
`EnhancementConfiguration`, `EmulatorConfiguration`, and texture-replacement
policy. Each replacement entry binds its position, canonical content SHA-256,
raw `rt64.json` SHA-256, and effective database configuration. Directory
content identity is a sorted relative-path/file-byte encoding; `.rtz` identity
is the exact archive bytes. Machine-local absolute paths are operational input
only and are deliberately excluded from evidence. Activation copies the
inspected bytes into a process-owned temporary snapshot, re-inspects that copy,
and passes only its private paths to C++. A post-activation inspection must
still match the original identity. The snapshot then stays alive with the
native context, so Stream policy cannot observe later edits to the caller's
mutable pack and release capture does no per-frame filesystem walk or hashing.
Configured-but-not-active inputs never enter capture, and any failed or raced
load destroys native state before its snapshot is removed. An explicit reload
or recreation is required to adopt later source-pack changes. This proves
which policy and pack bytes were active but does not by itself close behavior
claims. The separate synthetic Metal fixture covers DDS/mipmap, Rice selection,
and a deterministic held-queue Stream fallback-to-final transition without
wall-clock timing. Ten consecutive clean live Metal runs closed all three
texture-replacement behavior rows.

High-resolution rendering is closed by ten consecutive clean live Metal runs.
The paired fixture first reproduces a certified 4x4 control capture, then draws
a fractional, high-frequency diagonal texture rectangle against the same
target under exact Manual 1x and 2x policies. Every run retained the expected
layout and anchors while exactly three post-VI pixels changed: native bounds
and red count were `[4,4,14,8]` and 32, while 2x produced `[4,4,14,9]` and 35
with distinct stable image digests.

Downsampling is independently closed by a second ten-run live Metal
differential. With a 16x16 swapchain, both the 32x32 high-resolution target and
its explicit 2x box-filtered 16x16 target collapse to identical post-VI pixels.
The fixture therefore resizes through a certified 32x32 handoff, where the high
path exposes the larger target while the downsample path box-filters to 16 and
nearest-expands. Both captures retain row-bytes 128, bounds `[5,7,14,12]`, and
33 red pixels, but the filter changes exactly seven pixels and moves the stable
image digest from `f06f2043...` to `867d0aab...`. Exact policy identity and
present/capture ordering are checked separately from that pixel differential.

Aspect selection is also proven to be a live host control, but its public
behavior rows remain open. Ten fresh watchdog-bounded Metal processes switched
one backend through original 4:3 and manual 16:9, 2:1, and 21:9. Each mutation
took the framebuffer-discard path, retained a distinct complete active policy,
and produced two stable exact raw-DPC post-VI captures. Those bytes reject a
no-op control path; they cannot prove recognized-HLE projection correction or
correct viewport/scissor/2D intent because raw RDP begins after projection.
`RT64-ASPECT-EVIDENCE.md` records the exact hashes and remaining gate.

Manual HFR selection and the cooperating-game path now have bounded causal
Metal evidence, closing the renderer-API behavior row. Twenty fresh
watchdog-bounded processes first negotiated Extended v1 through a required
GetVersion task, then emitted Enable, SetRefreshRate 60, and decomposed
Auto-order matrix group 7 with position/rotation interpolation in every
moving-triangle workload. The opt-in admission substitutes only the fixture's
public F3DEX2 dialect; the Extended commands, workload matching,
interpolation, rendering, and presentation are pinned RT64 mechanisms.

Each process live-switched from Original to Manual 120 Hz, warmed up the
explicitly grouped transform under the enabled policy, and captured the exact
120/60 two-image burst. Typed evidence binds source workload 3, current
workload 4, present 3, source rate 60, target rate 120, and generated fractions
1/2 then 2/2 to the ordered post-VI images. The
161-pixel red shape moved in ordered half steps: x bounds/sums were
`19..37/4508`, `21..39/4830`, and `23..41/5152`, while y bounds/sum remained
`15..29/3902`. The intermediate SHA-256 is `af5e25c1...`; the endpoint is
`b7116e22...`, distinct from both preceding images and exactly equal to the
Original control. Switching back restores one exact Original presentation.
The evidence-only F3DEX2 admission is compiled only by the non-default
`synthetic-f3dex2-evidence` feature, while the HFR-only present observer uses
the feature-qualified `hfr-post-present-call:v1` exact-source overlay;
ordinary unknown-microcode tasks returned `NeedsLle` both before and after the
synthetic call.

The HFR observer separately brackets the actual swapchain `present` call with
a monotonic host clock. Its start is after pinned RT64's `preciseSleepUntil`
and optional present wait; its return is immediately after the API call. Each
fixture run requires eight exact 120/60 two-call bursts. The predeclared host
scheduler acceptance bounds require every within-burst call-start interval to
remain within 0.70–2.50 target periods, at least seven of eight within
0.85–1.35 periods, and the median within 0.90–1.20 periods. These are fixture
bounds that reject missing pacing and large stalls, not RT64 guarantees.
A second 20-fresh-process, per-child-watchdog Metal bar retained all 160 exact
call pairs inside the tight bounds. Observed within-burst intervals ranged
from 8,331,458 to 8,780,417 ns, per-run medians ranged from 8,333,521 to
8,333,979 ns, and the longest swapchain `present` call was 28,166 ns.

This proves runtime selection, supported HLE-dialect/Extended cooperation,
matching/interpolation/render ordering, two post-VI images, and post-sleep
swapchain API-call cadence without weakening production microcode admission.
The row closes at RT64's renderer/present API boundary. It does not expose the
compositor's physical scanout timestamp; that remains an explicit macOS and
cross-platform certification residual rather than being inferred from API
return time.

An upstream-default profile and a hardware-faithful/off profile are distinct.
Pinned RT64 defaults to window-integer scaling at 2x with scaled-only 2D
upscaling, while fn64's faithful profile selects original resolution and
original 2D treatment. Both profiles must be named and tested; an unqualified
`default` must not be cited as proof that their policies match.
