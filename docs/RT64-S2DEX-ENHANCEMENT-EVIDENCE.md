# RT64 S2DEX enhancement evidence

Status: bounded causal behavior for both `s2dex.framebufferFastPath` and
`s2dex.fixBilerpMismatch` is proven on the certified Metal host.
Recognized-game coverage, measured performance, and the wider platform matrix
remain open.

## Public mechanism

Pinned MIT RT64 exposes the two live S2DEX booleans in
[`EnhancementConfiguration::S2DEX`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/common/rt64_enhancement_configuration.h#L34-L37).
The public S2DEX manual's scaled-background command divides an image into TMEM
slices. RT64's framebuffer fast path instead attempts one whole-image tile
copy before that ordinary slice loop
([`rt64_gbi_s2dex.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/gbi/rt64_gbi_s2dex.cpp#L323-L352)).
That is a mechanism optimization: correct pixels should remain identical.

The mismatch control is in this same scaled-background handler. Pinned RT64
derives `usesBilerp` from `G_OBJ_RENDERMODE`, compares it with the RDP texture
filter, and removes the extra bilerp load footprint only when the two disagree
and `fixBilerpMismatch` is enabled
([`rt64_gbi_s2dex.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/gbi/rt64_gbi_s2dex.cpp#L202-L218)).
S2DEX2 maps `G_BG_1CYC` directly to that handler
([`rt64_gbi_s2dex2.cpp`](https://github.com/rt64/rt64/blob/f0728a2520d5aa735886240de3fee75cc805f6d6/src/gbi/rt64_gbi_s2dex2.cpp#L66-L69)).
The separately unimplemented compound object-rectangle handlers are therefore
an upstream microcode-coverage gap, but not the execution path controlled by
this setting.

The evidence fixture uses only already-admitted public S2DEX2 operations: a
64×64 RGBA16 framebuffer is written in one earlier workload, then sampled by
`G_BG_1CYC` into a distinct 64×64 target. The image exceeds one TMEM slice.
The non-default `synthetic-s2dex-evidence` transport installs RT64's real
`GBI_RDP` plus `GBI_S2DEX2` tables and calls the normal interpreter/workload
path. It neither adds a production microcode digest nor replaces a command
handler.

## Causal result

[`rt64_s2dex_enhancement_behavior.rs`](../crates/fn64-render-rt64/examples/rt64_s2dex_enhancement_behavior.rs)
warms the prior framebuffer relation, then switches the setting off, on, and
off through the live enhancement-policy seam. A read-only completed-workload
observer binds the complete ordered load program: texture address/format/size/
width, tile format/size/line/TMEM/palette/address modes/masks/shifts/bounds,
operation kind, tile index, load bounds, and block DXT where applicable. It
also binds exact load-address minimum/maximum and distinct count, tile-copy
operation vectors, source framebuffer base/width/height/size and process-local
identity, and the strict source-before-target write order. It observes
downstream workload artifacts, not a counter inside either enhancement branch.

| Phase | Policy SHA-256 | Load digest | Exact route |
|---|---|---:|---|
| fast path off | `0ae411439f1b742ee2017a8f537212767925b71810b4813461becefdee40f3e9` | `11081332784341843569` | three distinct in-range load starts, three `CreateTileCopy` operations, three tile-copy dispatches |
| fast path on | `7b73fb8c7d547e59aecb67b2d0a001ce21d0bad6f3bf2730d7dffdfa6237ca54` | `13074734122227382117` | one base-address load, one `CreateTileCopy`, one tile-copy dispatch |
| fast path off restored | exact first measured off identity | exact first measured off digest | exact three-load route restored |
| fast path on, ordinary RDRAM source | enabled identity | `17405972784478231233` | zero GPU copy operations, three ordinary RDRAM/TMEM uploads |

The mismatch matrix uses a 60×60 frame inset into the same 64×64 source so
the required neighbour footprint remains inside the managed framebuffer:

| Object mode / RDP filter | Fix policy | Load-program digest | Exact route |
|---|---|---:|---|
| point / point | off, then on | `404009634700744000` in both phases | two managed tile copies in both phases |
| bilerp / point mismatch | off | `11746348200741240963` | three managed tile copies including the extra bilerp footprint |
| bilerp / point mismatch | on | `404009634700744000` | exact point/point two-load program |
| bilerp / point mismatch | off restored | `11746348200741240963` | exact first mismatched three-load program restored |
| bilerp / bilerp | off, then on | `11746348200741240963` in both phases | three managed tile copies in both phases |

The fix-only active-policy SHA-256 is
`a3b4dafb32caa764476b2bc5138c7df4ab57aba0ffd863d639dd90cf793dc917`.
Point/point and matched-bilerp invariance reject a broad policy effect; the
mismatch off/on/off sequence proves the exact predicate and restoration.

Every measured draw retains exact post-VI SHA-256
`1c6bb9863b124ebc394d9ce73d1287d9ad651814e655c949a70118f44c334df2`
and exact target-RDRAM SHA-256
`d41df4d6472ee3f7a8440e3f92b1f9bc96d6aaee4fb6a23e813cdf2208118e81`.
Source, target, and ordinary-input guards remain unchanged. Workload IDs
strictly advance, the same nonzero source identity survives off/on/off, and
ordinary unknown-microcode tasks return `NeedsLle` before and after the
synthetic interval. Because the deferred snapshot holds RT64's worker mutex
across enqueue and snapshot, this gate uses the concurrency bar: twenty fresh
processes, not ten.

This proves causal coalescing and the exact bilerp-mismatch load-footprint
correction for these synthetic `G_BG_1CYC` cases. It does not measure a
speedup, generalize to every S2DEX background/format/scale/edge condition,
implement the separately missing object draw handlers, recognize a game
microcode, or certify another host API/platform.
