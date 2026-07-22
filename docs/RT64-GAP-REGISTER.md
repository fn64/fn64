# RT64 Gap Register — for fn64's Rust port

Purpose: fn64 will port RT64's N64 RDP renderer to Rust (wgpu). This register catalogs where
RT64 is **known-incomplete or approximate**, so the port **closes** those gaps rather than
faithfully reproducing them. Read-only research; no code changed.

- **Checkout:** `/Users/jer/Code/no-mercy-recompiled/third_party/rt64` @ `f0728a2`.
- **Upstream drift:** `f0728a2` is the **tip of `origin/main`** (0 commits behind, `git rev-list --count f0728a2..origin/main == 0`). No missed merged accuracy fixes. **Pin recommendation: keep `f0728a2`.** All open fixes live in open PRs, not in unmerged main commits (see PR section).
- Sources: in-source markers (~92 in src/hle,render,shared,gbi), open issues (`gh`, authed jeremyw), open PRs.

---

## A. Accuracy gaps the port should fix

### A1. Microcode / GBI command coverage (hard stops today — `assert(false)`)
These are microcode features RT64 does **not** implement; it aborts. A faithful renderer that
targets games using these needs them. (For OoT specifically, most are irrelevant — see scope §D.)

| Site | Gap | Fix direction |
|---|---|---|
| `src/gbi/rt64_gbi_s2dex.cpp:576,595,614` | S2DEX `objLoadTxSprite/objLoadTxRect/objLoadTxRectR` draw side is `assert(false)` — texture load runs but `doObjSprite/doObjRectangle(R)` unimplemented | Implement the sprite/rect draw path (2D microcode) |
| `src/gbi/rt64_gbi_s2dex.cpp:403,415,450` | Pinned RT64 S2DEX BG (`objRenderMode`, `bgLoadTile`-only, texrect count) is approximate/partial: "Reimplement more accurately to match the microcode"; only `S2DEX_G_BGLT_LOADTILE` is handled upstream | **Closed and evidenced in fn64's Rust reference path:** the public Copy partition is shared by both S2DEX wire families and both `G_BGLT_LOADTILE`/`G_BGLT_LOADBLOCK`; a 2×2 real-load matrix proves a six-row TMEM strip plus exact two-row final remainder bounds and pixels. Filtered/fractional-`imageYorig` scaled partitions remain hardware-trace work rather than being inferred from this integer-Copy result. |
| `src/gbi/rt64_gbi_s2dex.cpp:19,30,37 & s2dex2.cpp:19,30` | Bare `assert(false)` stubs in S2DEX/S2DEX2 dispatch | Implement remaining S2DEX(2) opcodes |
| `src/gbi/rt64_gbi_f3dex2.cpp:58,91,130,134` | Unimplemented `moveMem`, `moveWord`, "combine matrices mode", `special1` | fn64 closes public LookAt/light/viewport/force-matrix DMA, segment/light/fog/clip/perspective/force-marker writes, and persistent debug `G_DMA_IO` in its Rust reference path. Non-public point/matrix subindices and all three reserved `G_SPECIAL_*` encodings trap rather than fabricate behavior. |
| `src/gbi/rt64_gbi_f3dex2.cpp:157` (`line3D`), `f3dex.cpp:42`, `f3d.cpp:90,107,114,136` | `line3D` and assorted F3D/F3DEX ops empty/`assert` | RT64 upstream still needs these paths. fn64's Rust reference lane has a typed public `G_LINE3D` path; exact microcode line-edge coefficients remain hardware-trace work. |
| `src/gbi/rt64_gbi_l3dex2.cpp:12` | `assert(false)` in L3DEX2 (line microcode) | RT64 upstream still needs line dispatch. fn64's Rust reference lane content-admits public L3DEX/L3DEX2 and routes their public line command through the typed line path. |
| `src/gbi/rt64_gbi_f3dwave.cpp:38` | `F3DWAVE_G_UNKNOWN` command nulled out "until it's figured out" | RE the unknown Wave Race command |
| `src/gbi/rt64_gbi_extended.cpp:19,349,384` | Extended-GBI unrecognized/invalid opcodes unhandled | (EX GBI is an RT64 enhancement — likely skip for faithful port, see §D) |
| `src/hle/rt64_rsp.cpp:832` | `assert(false && "Unsupported modify vertex")` — `G_MODIFYVTX` sub-modes unhandled | **Closed in fn64's Rust reference path (2026-07-18):** all four public final-cache destinations decode with their documented fixed-point formats; malformed slots/destinations trap by name. |
| **issue #195** | No **Rej** microcode support (F3DLX.Rej / clipping-reject variants) | RT64 upstream still needs Rej support. fn64's Rust reference lane digest-types the public F3DLX.Rej, F3DEX2.Rej, and F3DLX2.Rej identities; exact L-variant transformed-pixel precision remains a loud frontier. |
| **issue #217** | **F3DEX2 missing `resetFromLoad` for loaducode** — lights/lookat/fog/geom-mode/texture state not re-zeroed on ucode load (differs from F3D) | **Closed in fn64's Rust reference path (2026-07-18; retained here as RT64 upstream provenance, not live fn64 work):** the compound command now performs ordered persistent DMEM/IMEM loads and resets combined MP, vertices, lights/look-at, fog factors, geometry, texture selection, and clip ratio. It preserves the public F3DEX2 maintained list: DL/matrix stacks, modelview/projection, segments, viewport, scissor, other mode, and perspective normalization. RT64 upstream still requires its own fix. |
| **issue #243** | `setPerspNorm` not handled — `0` should mean nothing renders (degenerate persp) | **Closed at fn64's float-reference contract level (2026-07-18):** retain the public `.16` value across ucode reloads, make explicit zero non-renderable, and leave exact nonzero limited-divider precision to hardware trace. |

fn64 Rust-reference progress (2026-07-18): transformed F3DEX2 vertices retain
all six homogeneous clip-plane codes. `G_CULLDL` applies the public inclusive
`v*2` cache range and ends only the current display list when every selected
vertex shares an outside plane. The public `G_RDPHALF_1`/`G_BRANCH_Z` compound
form compares retained unsigned-16.16 screen depth and performs a tail branch,
and `G_POPMTX` consumes its full `w1 / 64` count. Both public light-color-copy
destinations update directional or ambient RGB without changing direction.
Signed fog factors generate clamped vertex shade alpha from projected depth;
exact microcode fixed-point rounding remains a hardware-trace item. A typed
`G_LINE3D` path now handles public variable width, six-plane clipping,
flat/smooth shade, perspective texture attributes, scissor/eight-sample
coverage, the blender, and read-only Z. Exact microcode line-edge coefficients
remain a hardware-trace item. The public LookAt X/Y DMAs and regular/linear
texture-generation mappings are typed and unit-verified; exact microcode
trigonometric lookup/rounding remains open. The public two-command force-matrix
path now stages and activates a concatenated transform without changing either
stack, and an ordinary matrix operation supersedes it. Unsupported move
subindices and unknown opcodes remain loud frontiers. Perspective normalization
also retains its public scale, rejects zero-scale geometry, and records exact
nonzero divider precision as a hardware-trace frontier.
All four public clip-ratio writes are also typed and command-persistent; they
reset on self-load, expand line clipping, and do not change ordinary
`G_CULLDL` frustum codes.

### A2. Rasterization / RDP pixel-pipeline accuracy (the CORE — port these correctly)
These are where RT64 is **approximate** in the base rasterizer. Highest fidelity value.

| Site / Issue | What's wrong | Fix if known |
|---|---|---|
| `src/hle/rt64_rdp.cpp:115` | `assert(false && "Crash screen not implemented.")` — RDP crash/error path missing | Low priority for faithful render; port graceful handling |
| `src/hle/rt64_framebuffer_manager.cpp:345` | `assert("Unimplemented reinterpretation logic.")` — some framebuffer format reinterpretations unhandled | fn64 classifies the three legal public RDP color layouts once and reimports exact RDRAM bytes on same-address layout switches; retain this RT64 item only for non-public/native format paths |
| `src/hle/rt64_state.cpp:556` | Direct FB reinterpretation "not implemented yet" → forces a sync fallback | Implement direct reinterpret to avoid stalls + correctness edge |
| `src/render/rt64_native_target.cpp:167-176, 254-262` | Native FB **readback/writeback** unimplemented for 4-bit, RGBA8, IA8, CI16/32, IA16/32, I16/32 (long assert list) | Implement the missing color-image read/writeback format combos |
| `src/render/rt64_render_target.cpp:180,268` | `assert(false)` unsupported render-target copy / buffer format | Handle remaining copy/format combos |
| `src/render/rt64_texture_cache.cpp:730` | Unsupported DDS format (replacement-texture path) | Enhancement-adjacent (§D); low priority |
| **issue #200** | **RDP register timing off by one cycle** in 2-cycle mode: Texel0/Texel1 fetch, shade color, alpha-compare, memory color all use next/prev pixel's value. "Can't be emulated exactly; approximate with derivatives." | Port the texel1-derivative trick + extend to shade/alpha/mem; add debugger warnings |
| **issue #189** | **TLUT accuracy**: in copy mode / bilerp, the 4 samples of a palette pixel should each use their own palette entry | Per-sample TLUT lookup; fn64 closes this by looking up each physical-TMEM sample before filtering |
| **issue #116** | **Tile index wrap**: tile0=7 ⇒ tile1 should wrap mod 8 (not saturate at 7). Also a **buffer overflow**: `RDPTiles[8]` OOB when `rdpTileIndex==7` w/ LOD off (`RasterPS.hlsl:139`) | mod-8 tile selection + bounds-fix; handle LOD clamp special case |
| **issue #201** | **Light color copy** not applied to odd-indexed vertices (SM64 Koopa) | Fix per-vertex light-color copy indexing |
| **issue #203** | **F3D near-clip** doesn't trigger at exactly z=0 (tri at z=0 with near=0 wrongly shows) | Correct near-clip boundary (≤ vs <) |
| **issue #106** | **FRUST_RATIO clipping** imposed by RSP viewport not implemented | **Closed at fn64's high-level output contract (2026-07-18):** retain all four public per-side writes, expand line clip planes, keep `G_CULLDL` independent, and rely on equivalent triangle scissoring inside the visible rectangle. Exact microcode subdivision/rounding remains hardware-trace work. |
| **issue #103** | **Prim Z (`gsDPSetPrimDepth`) inaccurate** — Prim Z −1 renders behind on HW but is *missing* in RT64 (needs 0x7FFE hack) | Fix prim-depth mapping to HW range |
| **issue #198** | **Depth precision** light flicker (Perfect Dark) | Match N64 depth quantization |
| **issue #235** | **Vertex coloring wrong** (Mystical Ninja Goemon frog white not green) — shading/combiner bug | Root-cause combiner/shade path |
| **issue #194** | Goldeneye sky needs **multiple triangles from LLE triangles** (rasterizer-shape emulation) | Hard; only if targeting affected games |
| **issue #183** | **Flat shading** currently via shader flat-output (driver-fragile); should duplicate vertices with copied normals | Duplicate-vertex flat shading (also simpler shaders) |
| **issue #150** | **F3D dz decal tolerance**: F3D/F3DEX tweak `dzdy` to raise computed dz → higher decal tolerance; replicate in pixel shader | Tweak computed dz to match |
| **issue #210 / #202 / #197 / #193 / #199** | Unverified RDP edge behaviors: copy-mode w/ mismatched formats; ZBUFFER-off + depth mode; fill-rect vs scissor in copy mode; fillrect `lrx+1` inclusivity; erroneous loadtile assert (`rt64_rdp.cpp:579`) | Verify against HW and encode correct behavior; remove bogus assert |
| **issue #82** | Extended-scissor/rect misalignment should round up (not down) when origin right | Rounding fix (widescreen-adjacent, §D) |

fn64 Rust-reference progress (2026-07-18): F3DEX2 decode now retains
SetColorImage, fill rectangles, FullSync, and triangles in one ordered
operation stream. The reference executor supports all three public RDP color-
image layouts: 8-bit intensity plus RGBA16 and RGBA32 target load/switch/write-
back. I8 imports/commits one logical byte per pixel, ignores hidden coverage,
and consumes all four fill-register bytes in order. RGBA16 retains hidden-bit coverage and
alternating fill-color halfwords; RGBA32 retains five-bit memory alpha plus
the three coverage bits in the alpha byte and consumes the whole fill word per
pixel. Both apply the public fill-cycle inclusive-lower-right rule clipped by
the exclusive scissor. One/two-cycle `G_FILLRECT` uses exclusive lower-right
bounds and the supported combiner, alpha-compare, primitive-depth, blender,
ordered-dither, coverage, and color-write path; unsupported texture, shade,
LOD, noise, or unavailable prior-COMBINED sources remain loud. Fill-cycle
`G_FILLRECT` rejects every retained `Z_CMP`, `Z_UPD`, or `IM_RD` combination
through one shared bypass-hazard validator before color/depth mutation, as
required by the public `gDPFillRectangle`/`gDPSetCycleType` safe-mode notes.
Copy-cycle fill remains a loud rejection because the public result is
explicitly unguaranteed, not a locally implementable contract. It now
also preserves both words of TextureRectangle and executes the public
non-flipped RGBA16 copy-cycle rule, including inclusive bounds, fixed-point
origins/gradients, `dsdx=4<<10`, per-tile source identity, and the public
RGBA16 threshold rule where the one-bit alpha is a write enable rather than an
eight-bit blend-alpha comparison.
One/two-cycle TEXRECT and TEXRECTFLIP now execute their exclusive bounds and
fixed-point gradients through a shared point/average/three-nearest texture
filter, color combiner, alpha compare, framebuffer blender, and distinct
TEXEL1 sampling. Copy-cycle TEXRECTFLIP now swaps the S/T screen
axes with the public copy-gradient normalization. The copy sampler also owns a
distinct typed address mode: Programming Manual
[Chapter 13.11, "Restrictions"](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro13/13-11.html)
disables programmed clamping in copy mode while retaining mask wrap/mirror.
An exhaustive axis sweep covers both address modes, every four-bit mask,
clamp/mirror selection, and the complete public signed integer coordinate
range; a rectangle gate proves an enabled clamp bit cannot suppress copy wrap.
The reference backend also
accepts bounded raw DPC state/fill/texture ranges from both the shim and MMIO
entry paths. The RT64 C ABI exposes the matching bounded LLE entry through its
public `Application::processDisplayLists(..., false)` path, including
render-to-RAM workload synchronization; task-entry GBI HLE remains gated by an
explicit exact IMEM digest. Raw Load/Pipe/Tile/Full Sync accepts its unassigned
second word per SGI *RDP Command Summary*, while atomic F3DEX2 macro decode
keeps the stricter reserved-payload check. All eight raw RDP triangle record layouts (`0x08..0x0f`) now have
bounded widths and typed signed edge/shade/texture/Z coefficient ingestion
from SGI *RDP Command Summary* Tables 11-15. Edge-only, shade, texture, Z, and
the maximum-width combined form execute through the software rasterizer.
Raw work remains a distinct coefficient-bearing operation through execution:
the pixel-center span walker selects left/right sides from the major-edge bit
and steps shade/texture/Z from `d/de + d/dx` instead of reconstructing three
vertices. Texture forms bind the command's loaded tile, S/T/W retain
perspective division, and Z follows the documented near-zero 15.3 ordering.
XBUS DMEM submission also
executes a variable-width Z triangle. Unmodeled state opcodes still fail by
name. `G_SETZIMG`
now persists independently of color targets, and a fill directed at that image
writes the documented raw fill halfwords while clearing the same covered
software depth samples; this survives a later color-image switch. Programming
Manual Chapter 13.7 mip/detail/sharpen selection now runs across rectangles,
F3DEX2 triangles, and raw RDP triangles with immutable eight-tile snapshots,
modulo-eight indexing, and loud missing-tile traps. Exact derivative norm and
fixed-point boundary precision remain open. Shade-dependent rectangle
programs and pixel-Z rectangle requests still fail by name.
`G_SETCONVERT` retains all six signed nine-bit fields; YUV16 tile loads decode
the public Y0/U/Y1/V shared-chroma layout; and `G_TC_CONV`, `G_TC_FILTCONV`,
and `G_TC_FILT` select point-convert, filter-convert, and filter-only sampling
for raw and F3DEX2 rectangles and triangles. K4/K5 are also live combiner
sources. The command layout and equations come from the public SGI
[*RDP Command Summary*, Table 28 and texture-filter section](https://ultra64.ca/files/documentation/silicon-graphics/SGI_RDP_Command_Summary.pdf)
and the mode behavior from Programming Manual
[Chapter 12.5](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro12/12-05.html).
Exact silicon accumulator widths and negative-product rounding remain a
hardware-trace item. `G_SETKEYR`/`G_SETKEYGB` also retain their public center,
scale, and 4.8-width fields. `G_CK_KEY` evaluates the documented two-stage
chroma equation through typed CENTER/SCALE combiner inputs and alpha fixup;
the raw-command gate proves that result reaches alpha compare. This follows
Programming Manual
[Chapter 12.6](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro12/12-06.html)
and SGI *RDP Command Summary* Tables 29-30; internal precision remains a
hardware-trace frontier. `G_SETPRIMDEPTH` and `G_ZS_PRIM` now supply persistent uniform
Z/DeltaZ for raw triangles and combined texture rectangles, using the public
libultra register format. This does not close the hardware-verification items
in #193/#199:
the Chapter 16 18-bit-Z to 3-bit-exponent/11-bit-mantissa codec and split
visible/hidden DeltaZ packing are implemented with exhaustive
canonicalization/quantization checks. Passing raw `Z_UPD` fragments persist
both pieces, image selection reloads them, and a physical-address hidden-bit
store preserves aliasing and switch-away/back behavior. Fill-mode depth writes
now use one typed expansion of each 16-bit fill-register half. Programming
Manual
[Chapter 12.8, Figure 12.8.2](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro12/12-08.html)
specifies that the halfword LSB is replicated into both physical hidden bits;
an exhaustive halfword sweep and a raw alternating-lane fill prove nonzero
stored DeltaZ reaches visible RDRAM, hidden storage, and depth-image reload.
The Chapter 15 Farther/Nearer/In-Front equations now drive all four Z modes,
and stored DeltaZ uses the documented most-significant-bit index rather than
the former off-by-one bit width. Raw edges now evaluate the public eight-sample
checkerboard mask, and edge/attribute planes remain signed fixed point through
evaluation using Table 12's documented X reference points. Coverage persists through RGBA16's visible LSB plus shared
physical hidden bits; all four `CVG_DST` rules, coverage-alpha selection,
`CLR_ON_CVG`, memory-coverage blending, and the documented opaque-wrap strict
Z override execute. Chapter 15's public `IM_RD` contract now gates both old
color and old coverage through one typed optional framebuffer sample. With the
bit clear, Clamp/Wrap writes cannot merge or wrap prior coverage, while a
framebuffer color or coverage-alpha mux traps instead of silently reading
memory. Exhaustive mode/coverage sweeps and paired raw/high-level partial
triangles cover the mechanism. High-level F3DEX2 triangles now use the same eight sample
positions for edge coverage. Set Scissor field/odd controls survive decode and
gate scanlines across fill, depth-fill, copy/combined rectangle, raw-triangle,
and high-level-triangle paths. The three public 8-bit/RGBA16/RGBA32 color
layouts share typed validation/import/fill/commit and exact-byte same-address
reinterpretation. An exhaustive typed matrix covers all nine target switches,
all three fill layouts, direct I8, packed IA8, undereferenced CI8, and RGBA16
copy sources, and every admitted source/target size pairing; unsupported
copy-source formats and cross-size targets fail loudly. Eight-bit copy retains
the original TMEM byte, including odd-row bank layout, while alpha comparison
uses I8 intensity, IA8's expanded low nibble, or the undereferenced CI8 index.
With RGB/alpha dither disabled, RGBA16 RGB and RGBA32 memory
alpha use the manual's three-bit truncation instead of round-to-nearest.
One/two-cycle RGB MagicSquare/Bayer and alpha Pattern/InversePattern selectors
execute before all three public I8, RGBA16, and RGBA32 color-image writes. The
pre-write ordering is structural rather than a per-layout exception: the
destination layout is not available to the RGB dither transform. This follows
Programming Manual
[Chapter 15.5](https://ultra64.ca/files/documentation/online-manuals/man/pro-man/pro15/15-05.html),
which says RGB dither still perturbs the low three bits for RGBA32 even though
that layout does not truncate them and lists the 8-bit layout alongside the
two RGBA layouts. One typed per-fragment noise byte now
feeds combiner NOISE, RGB/alpha Noise, and `G_AC_DITHER`; the default seedable
SplitMix64 policy is deterministic and long-period. Nintendo's unpublished
silicon generator and seed remain a hardware-trace frontier. Separately, the
clean pinned native RT64 path has an exact-source overlay for the combiner-alpha
`G_AD` selectors and their random-source topology. One typed fragment-noise
sample derives from one `nextRandUint` result; combiner NOISE and
`G_AC_DITHER` consume its low-24-bit unit float, and `G_AD_NOISE` consumes its
low three bits. The overlaid `G_AD` quantizer still runs after alpha
compare/coverage rejection and before the blender, so `G_AC_DITHER` remains an
independent earlier decision over unmodified alpha even though the Noise
selector shares the sample. The
existing synthetic raw-DPC Metal gate binds exact 16x16 RGBA16 Pattern,
InversePattern, Noise, and Disabled digests plus exact ordered 4x4 tiles and
live Noise. Its paired combiner-NOISE/`G_AC_DITHER` phase accepts exactly 146
pixels, all grayscale and no brighter than the primitive half-alpha cutoff,
which binds the shared route on pinned Metal. The complete twelve-phase,
seven-repeat transcript was identical in 10/10 fresh native processes. Its
ordinary G_AC, shared control, and shared G_AC digests are respectively
`1493e7af74f80caff7a0c645b0f522ec347ce38a198237ab3cbd802394e0c793`,
`0268d9c2410c25067f144983829a5a091525f357e2981fc53f25e3d2c054da7f`, and
`70289db3267cb703e806ee9ba86635ec651aab0ec56f434db1cf7988cbb34251`.
This bounded slice does not alter shade, fog, or coverage alpha; it does not
repair RT64's framebuffer-wide/deferred RGB-dither choice. The reference path's full
eight-bit `G_AC_DITHER` sample and RT64's unit-float threshold are not claimed
to quantize identically. Silicon-internal
accumulator width/truncation and subpixel attribute/Z correction remain open. Public
coverage-wrap routing for depth-compared fragments is now typed for every Z
mode: a wrapping opaque fragment selects strict `In Front`, while wrapping
`ZMODE_INTER` traps by name instead of silently borrowing opaque correlation.
The interpenetration coverage-adjustment arithmetic and exact alpha-coverage
tie rounding remain open. The manual-defined non-RDP 16-bit write rule now has
one post-commit typed event
from both generated-C `MEM_H` and typed-Rust `store_h`. Cached and uncached
aliases canonicalize to one physical halfword, and the reference backend
replicates the visible LSB into an already-owned hidden-bit sidecar even when
the visible value is unchanged. Every backend explicitly reports whether it
applied a Rust sidecar; native RT64 reports `NoRustHiddenSidecar`, so event
delivery does not itself certify RT64 hidden-bit parity. Exact filter
precision and the high-level covered-subpixel
attribute selector/arithmetic remain open. Public documentation proves the
alpha/coverage mode topology and normalized product but not the multiplier
width, `/255` versus `/256` normalization, quantization/tie rule, or the
one-through-eight coverage-to-alpha code. The current nearest-`/255` and
normalized-u8 policy is therefore explicitly a reference approximation;
synthetic boundary fixtures plus strict capture analyzers now require both
directions of the complete 8-coverage by 256-alpha domain in one-cycle and
two-cycle modes. They preserve every observed product count and threshold pass
bit without claiming silicon accuracy; closure still requires the out-of-tree
hardware producer and ten controlled captures. Raw triangles,
high-level triangles, and lines now return one shared typed eight-sample
identity mask rather than collapsing immediately to a count; exhaustive
axis-aligned edge and scissor sweeps catch identity-preserving errors that a
population-only assertion cannot. The bounded nearest-to-center selector is
now a typed policy with one explicit total preference order. Its tests exhaust
all 255 nonzero masks, preserve pixel-center evaluation only for the full mask,
and separately sweep every equal-distance tie subset. Raw left-inclusive and
right-exclusive comparisons are also swept one Q16 LSB below, exactly on, and
one Q16 LSB above every checkerboard X position. These characterize fn64's
policy and commanded fixed-point boundary without changing the public
checkerboard output or claiming an internal accumulator width. Allowed public sources still do not
establish whether silicon uses a representative lookup or any arithmetic
centroid. Full-coverage triangles and lines retain pixel-center attributes.
Partial raw and high-level triangles plus normalized lines share one typed
covered attribute sample for shade, texture, and Z, matching Programming
Manual 15.4's requirement that corrected Z intersect the primitive.
Nearest-covered selection and stable tie order are a declared bounded host
policy. The raw executor no longer converts
coefficient stepping to host floats, but gate-level truncation remains a
black-box differential frontier.

The shared texture sampler now retains all public `G_SETTILE` clamp, mirror,
mask, shift, TMEM-base, and line fields. It reproduces the Programming Manual's
wrap, mirror, and mirror-then-clamp coordinate sequences for triangles and
both texture-rectangle directions. Physical 4 KiB TMEM storage now supplies
cross-load/render-tile and masked addressing, odd-row exchange, split-half
RGBA32/YUV storage, and quadricated per-sample RGBA16/IA16 TLUT lookup.
Uninitialized bits fail by physical address. Each low/high fractional bound is
retained independently while its integer part selects the inclusive source
span, and source-sized loads support distinct descriptor sizes including
RGBA32-through-16-bit. Wrapped unclamped axes use the nonzero mask domain
rather than an unused clamp bound. Post-perspective coordinates now enter one
typed signed S10.5 boundary that loudly enforces the public -1024..+1023.99
range. All 16 tile shifts then produce a distinct wide host accumulator for
signed texel/fraction decomposition, origin subtraction, address transforms,
and three-nearest weights; that host width is not a silicon-register claim.
The host floor into S10.5 plus average/filter output rounding are still bounded
reference choices, not claims about unpublished silicon reciprocal or
accumulator precision.

### A3. RSP transform / lighting / texgen approximations
| Site | Gap |
|---|---|
| `src/hle/rt64_rsp.cpp:245` | Assumes RDRAM loads never unaligned/overlapping — unverified assumption in vertex load |
| `src/hle/rt64_rsp.cpp:1123` | TEXGEN texcoord tracking unhandled (`// TODO: Figure out how to handle texcoord tracking on TEXGEN cases`). **Closed for fn64's base Rust reference renderer (2026-07-18):** generated coordinates are materialized in typed vertices and follow the normal perspective interpolation path. |
| `src/hle/rt64_rsp.cpp:966` | bare `// TODO` in RSP command handling |
| `src/hle/rt64_game_frame.cpp:566-567` | look-at matching stubbed (`(true && true)`); assumes one look-at per draw call, only checks first vertex — texgen/env-map interpolation approximate. fn64's base reference renderer now consumes exact per-command LookAt state; this RT64 temporal-interpolation heuristic is not a dependency. |

### A4. Temporal / interpolation heuristics (RT64's frame-interpolation layer)
RT64 does motion interpolation via workload/projection matching and a rigid-body decomposition.
This is **RT64-specific machinery**, not N64 semantics. It is sequenced after
the hardware-faithful base renderer, but it is part of the full RT64 feature-
parity denominator in §D because fn64 targets the combined runtime/renderer
stack rather than only stock-console output:
- `rt64_game_frame.cpp:128,139,268,512,929,948,1012,1028` — linear-lookup workload/projection matching, "must be per projection", "assume same order between frames", "support more than perspective projections", velocity-buffer TODOs.
- `rt64_rigid_body.cpp:22,23,42,55,74` — velocity tolerances hardcoded, scale/skew assumed to match rotation, "defaults to always interpolate", perspective-interp TODO.

---

## B. Open PRs — ready-made fixes to fold into the port

| PR | Fixes | Port relevance |
|---|---|---|
| **#254** | Improve synchronization detection for tiles being sampled (don't require sync for tagged FB regions when the fbPair is synced) | **Correctness+perf** — fold the sync logic into the port's framebuffer/tile sync model |
| **#246** | Removes `VK_EXT_scalar_block_layout` requirement — fixed serious graphical corruption on devices lacking it (Switch/Nvidia). Closes #23 | Reveals a **buffer-layout assumption**; the wgpu port must not depend on scalar block layout for its uniform/storage buffers |
| **#248** | `texture-cache`: parse replacement-DB bytes portably (libc++ `char_traits<unsigned char>`) | Build-portability only; irrelevant to a Rust port |
| **#251** | Bump hlslpp to v3.9 (macOS 26 build) | Irrelevant (Rust/wgpu, no hlslpp) |
| **#249** | Fix device-vendor console log formatting | Irrelevant |
| **#252** | Librashader support | Enhancement, §D — skip |

Only **#254** (tile-sampling sync) and **#246** (no scalar-block-layout assumption) carry lessons for the port. Neither main nor these PRs contain an un-ported base-rasterization accuracy fix beyond what's already in `f0728a2`.

---

## C. What RT64 itself does NOT do well / at all (from code + domain knowledge)
- **2-cycle RDP register timing** (#200) — fundamentally approximate; can't be bit-exact.
- **Coverage / AA / dither** — RT64 renders on a modern rasterizer; N64 3-point
  coverage-based AA and RDP dither remain approximate (issue #38, #73/#107
  MSAA sample positions). fn64's reference lane models the public eight-sample
  coverage mask, exact disabled-dither truncation, ordered matrices, and one
  typed deterministic per-fragment sample for combiner/RGB/alpha noise plus
  `G_AC_DITHER`. The clean pinned native path overlays only combiner-alpha
  `G_AD` immediately before blending and creates one typed sample from one
  `nextRandUint` result for combiner NOISE, `G_AC_DITHER`, and `G_AD_NOISE`.
  Exact synthetic Metal evidence covers distinct
  Pattern/InversePattern/Disabled output, live Noise, and the paired shared
  combiner-NOISE/`G_AC_DITHER` relation: exactly 146 accepted pixels are all
  grayscale at or below the primitive half-alpha cutoff, with an identical
  twelve-phase, seven-repeat transcript in 10/10 fresh native processes.
  Shade/fog/coverage alpha, framebuffer-wide/deferred RGB dither,
  native/reference threshold quantization and random-stream parity,
  generator seed/advancement, ordered matrices/ties, internal precision,
  non-Metal/MSAA paths, full-ROM reach, and silicon remain open. The reference
  sample stream and native outputs are deliberately not claimed as the
  unpublished hardware generator.
- **TMEM edge cases** (#196) and **TLUT copy/bilerp** (#189) — partial in RT64;
  fn64 has physical TMEM, per-sample TLUT lookup, independent fractional load
  edges whose integer parts select the inclusive span, source-sized mismatched-
  descriptor transfers, and mask-sized domains for wrapped unclamped axes.
- **S2DEX 2D sprite/rect microcode** — upstream RT64's draw side remains
  unimplemented (§A1). fn64's Rust reference lane now content-admits
  F3DEX_GBI_2 object rectangles, matrix-relative rectangles, and rotating
  sprites; block/tile/TLUT object loads and their compound draw forms; full and
  sub object matrices; S/T flips and matched bilerp mode; Copy/OneCycle
  backgrounds with both load selectors and bounded remainder strips; and the
  public conditional-list status equation. Segmented object pointers are
  resolved through typed RDRAM addressing. The current tranche also types
  clamp/filter/perimeter/ignored legacy-edge render-mode state, Average
  filtering including exact inward SHRINK1/2 on non-rotating rectangles, and
  exact-quarter 3/8-texel WIDEN. `NOTXCLAMP` is admitted only when typed
  Point or four-neighbour Average proofs keep every sample inside the source;
  current public-header XLU/AA bits are
  retained as ignored legacy state. Loud frontiers remain for `NOTXCLAMP`
  draws that can leave the source domain, WIDEN combined with
  filtering/flip/shrink/Copy or sub-quarter rounding, Average combined with
  WIDEN/Copy/rotating polygons or an out-of-domain NOTXCLAMP footprint,
  historical XLU/AA semantics, and filtered/fractional-`imageYorig`
  backgrounds. These are evidence-blocked neighbour, edge-ownership, and
  rounding frontiers; Copy subpixel processing and vertical background
  subpixel motion are explicitly unsupported by the public manual rather than
  actionable local omissions.
- **Rej / line (L3DEX2) / Wave microcodes** — RT64 upstream remains
  missing/partial. fn64's Rust reference lane digest-types the public Rej
  variants and content-admits L3DEX/L3DEX2 through its typed line path. Wave,
  exact L-variant transformed-pixel rounding, and exact line-edge coefficients
  remain loud trace frontiers; this is no longer a blanket Rej/line omission.
- **Prim-depth, near-clip-at-0, frustum-ratio clip, flat shading** — each individually wrong (§A2).
- **Framebuffer format reinterpretation & native read/writeback** — several format combos assert-out.
- **PAL/50Hz workload rate — closed at the adapter seam.** Pinned
  `rt64_vi.cpp` hardcodes 60 Hz `FullRate` while deriving a stable workload
  rate. The exact-source `vi-region-rate:v1` overlay obtains that base from the
  active fn64 context instead: typed `RenderConfig` carries the IPL-selected
  NTSC/PAL/MPAL standard, each `VIHistory` has an independently registered
  60/50/60 Hz base, and the device-free C++ probe calls the patched RT64 method
  for factor-one and factor-two vectors (60/30, 50/25, 60/30). Ten fresh live
  Metal processes additionally carry PAL and MPAL through production context
  creation and ordinary VI events, then observe exact completed-workload rate
  sequences `[0,0,0,50]` and `[0,0,0,60]` without an Extended override. The
  overlay does not alter the later Extended-GBI refresh-rate override. A
  schema-v22 full-ROM path now co-binds normalized destination-code TV region,
  committed device TV state, and renderer create-time configuration. No
  representative private exact-ten PAL/MPAL series has yet been retained;
  physical compositor cadence, field timing, and analog PAL output remain
  outside this closure.
- **Native VI pixel observation, bounded gamma dither, divot, RGBA16
  restoration, and qualified AA-selector output — closed at pinned fixtures.**
  A twenty-phase live Metal gate keeps one
  completed workload, strictly advances presents, captures exact nondefault
  8x6 BGRA8 output, and restores baseline/gamma phases byte-for-byte. Gamma,
  seeded gamma dither, and nonidentity X/Y scale change exact pixels. Repeating
  an identical dither seed reproduces exact pixels across a distinct present;
  changing only the seed changes them. The exact-source overlay mirrors fn64's
  named bounded-v1 stream in pinned RT64's final VI shader. Coverage-gated
  divot now makes twelve exact componentwise-median changes and restores
  exactly. RGBA16 `DITHER_FILTER` performs the signed comparison against every
  available 3x3 neighbor and preserves alpha: eighteen eligible full-coverage
  pixels change exactly, twenty-four non-full pixels remain unchanged, and six
  flat full-coverage controls remain unchanged. A separate eleven-phase
  qualified-coverage fixture, together with the adapter-capture integration
  test, preserves the hardware-mode-0 versus compatibility-`Unspecified`
  wire and native-callback distinction. For deliberately generated managed
  codes 1-6, modes 0/1 match an independent per-code Figure-11 CPU oracle at
  exact RGB8 vectors `[50, 45, 35]`, `[76, 60, 53]`, `[102, 70, 75]`,
  `[128, 87, 95]`, `[158, 95, 109]`, and `[185, 113, 128]`; modes 2/3 restore
  the exact baseline, and compatibility `Unspecified` matches replicate while
  explicit compatibility mode 0 matches AA. The divot oracle reconstructs
  the declared RDP source before projection, and AA, divot, and
  AA-before-divot each change exactly the six projected targets. This closure
  is bounded to pinned Metal, nearest filtering, progressive synthetic RGBA16
  input, opaque code-7 controls, and the original-aspect (4:3) presentation
  policy. Pinned RT64 aliases managed 7/8 and clamped 8/8 at code 7. The
  context-reuse gate binds exact workload/present continuity through both
  fixtures. The expanded codes-1-6 source passed the official
  watchdog-bounded lifecycle runner in 20/20 fresh Darwin 25.5.0 arm64
  processes on 2026-07-22.
  RT64's managed target does not retain authoritative RGBA16 storage or RDP
  dither history, and its alpha supplies only the native coverage estimate;
  code-0/save, natural triangle coverage, imported hidden coverage, other
  filtering/scaling modes, MSAA/downsample behavior, other graphics APIs,
  full-ROM coverage, silicon behavior and random-stream identity, DAC output,
  and analog video remain unproven.

---

## D. Sequencing boundary — hardware-faithful core, then full RT64 feature parity

**PHASE 1 (accurate base renderer — prerequisite for every later feature):**
- `src/gbi/*` GBI/microcode decode (F3D, F3DEX, F3DEX2, F3DZEX2 — OoT uses F3DZEX2/F3DEX2), `src/hle/rt64_rsp.cpp` (transform/lighting/clip), `rt64_rdp.cpp` + `rt64_rdp_tmem.cpp` (RDP state, tiles, TMEM), `rt64_state.cpp` core draw path, `src/shared/rt64_blender.h` + `rt64_color_combiner.h` + `rt64_other_mode.h` (blender/CC/othermode semantics), the raster pixel-shader logic (`src/shaders/RasterPS.hlsl`, `TextureSampler.hlsli`), framebuffer manager + native target read/writeback, `rt64_vi.cpp` (VI scan-out).
- **These carry the §A accuracy gaps — port the corrected versions.**

**PHASE 2 (available RT64 features — required for full stack parity):**
- higher-resolution rendering and downsampling;
- arbitrary widescreen/ultrawide presentation, including the extended-scissor
  and 2D correction contracts needed by patched titles;
- 60-FPS-and-above temporal interpolation, draw-call/projection matching, and
  the public latency-reduction modes;
- Extended GBI activation and its public viewport, matrix, branch, scissor,
  interpolation, and depth-test commands;
- DDS/Rice-name texture packs, replacement scaling, and asynchronous
  streaming;
- the public deferred-frame/debugger behavior and native-resolution
  render-to-RAM guarantees; and
- the D3D12, Vulkan, and Metal host capability exposed by the stack.

The implementation may use Rust/wgpu or the quarantined MIT RT64 bridge; parity
is judged by the public behavior and evidence, not by reproducing RT64's
internal architecture. Platform-specific workarounds that have no observable
contract need not be copied, but the corresponding supported-platform behavior
must still pass. Performance work is required where an advertised contract
depends on it—for example, asynchronous texture streaming and no runtime
pipeline-compilation stutter—not merely because an internal TODO exists.

RT64's public README currently labels path tracing, model replacement, the game
script interpreter, and emulator integration as **in development**. They are an
upstream-watch list, not part of the available-feature parity denominator until
upstream exposes them as available behavior. fn64 may ship such capabilities as
explicit extensions earlier, but doing so cannot substitute for an open parity
item above.

---

## E. Top 5 gaps to close in the port (prioritized for faithful OoT output)

OoT uses **F3DZEX2** microcode; it is a 3D game (no S2DEX 2D-sprite draws), so 2D-microcode and
Rej/Wave/line gaps are low priority *for OoT specifically*. Prioritization below is OoT-first.

Historical issue #217 remains in §A1 because RT64 upstream still has the gap,
but fn64's Rust reference path has closed it; it is not live fn64 work and is no
longer ranked here.

1. **RDP 2-cycle register timing (#200)** — OoT leans on 2-cycle CC/blender heavily; texel1, shade, alpha-compare, and memory-color timing drive many surface effects. Preserve the register-lane distinction and verify the remaining timing approximation. The strict 75-case raw-DPC analyzer now retains final RGBA/depth/coverage divergence and command-bound adjacent-pixel order, but no repeated hardware bundle exists and final planes do not directly prove cycle-one register timing.
2. **Combiner/shade and blender correctness (#235, `rt64_blender.h:224`)** — audit shade-color sourcing and the framebuffer-color-dependent blender modes used by fog, fades, and transparency instead of treating them as part of one generic 2-cycle item. The analyzer supplies a producer-reviewable five-bit-alpha/denominator boundary matrix; it does not infer division or rounding.
3. **Tile-index mod-8 wrap + `RDPTiles[8]` OOB fix (#116)** — an actual out-of-bounds read plus wrong multi-tile/LOD texturing. Correctness + memory-safety; trivially wrong today.
4. **Decal / Prim-Z / near-clip depth accuracy (#150, #103, #203, `state.cpp:1655`)** — OoT uses decals (shadows, floor marks) and depth-sorted effects; RT64's decal path is admittedly re-approximated ("Reimplement proper decals in RT") and prim-depth/near-clip are off. Fixes z-fighting and missing/mis-depthed sprites.
5. **Framebuffer reinterpretation and native RDRAM synchronization (`framebuffer_manager.cpp:345`, `state.cpp:556`, `rt64_native_target.cpp:167-176,254-262`)** — close the remaining RT64-native format transitions and read/writeback cases without reopening the three public layouts already modeled by fn64's Rust reference lane.

(For RT64 bridge certification, retain its upstream S2DEX/Rej gaps. For fn64's
Rust reference lane, promote the remaining combination-specific S2DEX cases,
Wave, and exact transformed-pixel/line precision rather than the closed blanket
S2DEX/Rej/line labels.)
