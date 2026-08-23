# VI filter boundary

The reference renderer's VI stage is a deterministic digital pre-DAC model.
It is not an analog NTSC/PAL encoder and does not claim byte-for-byte identity
with an unpublished VI gamma ROM or random generator. This boundary keeps the
parts established by public evidence separate from reproducibility policies
and still-open silicon behavior.

## Allowed public evidence

- The Nintendo 64 Programming Manual and `osViSetSpecialFeatures` manual define
  the `VI_STATUS` controls, including gamma, gamma dither, divot, and the
  16-bit dither filter. The public VI control definition assigns STATUS bits
  8-9 to four modes: anti-alias plus resample with unconditional extra fetch,
  anti-alias plus resample with conditional extra fetch, resample only, and
  neither operation. The mode manual also states that partial-coverage samples
  use the anti-alias backend rather than the dither backend.
- [US 5,699,079, Restoration filter for truncated pixels](https://patents.google.com/patent/US5699079A/en)
  specifies the full-coverage 5-to-8-bit restoration mechanism: shift the
  center component left three, then add one for every greater neighbor and
  subtract one for every lesser neighbor in the available 3x3 neighborhood.
- [US 5,742,277, Antialiasing of silhouette edges](https://patents.google.com/patent/US5742277A/en),
  which the public Video Interface patent incorporates directly, specifies
  the partial-coverage path: a six-sample checkerboard neighborhood, rejection
  of partial neighbors and componentwise extrema, background estimation from
  the remaining penultimate interval, and foreground/background interpolation
  by coverage.
- [US 6,166,748, Video Interface](https://patents.google.com/patent/US6166748A/en)
  specifies horizontal three-sample median divot correction on or adjacent to
  silhouette edges; vertical linear interpolation between successive filtered
  lines followed by horizontal linear interpolation between neighboring
  pixels; X/Y scale and subpixel-offset register fields; square-root gamma
  correction; and fresh random low-bit noise before final seven-bit video
  quantization.

No GPL runtime implementation was consulted.

## Implemented contract

| Slice | Deterministic reference behavior | Exactness boundary |
| --- | --- | --- |
| VI STATUS AA/resample selector | A typed four-value control preserves bits 8-9 at the V-blank latch and native RT64 wire. Modes 0 and 1 enable partial-coverage silhouette AA and resampling, mode 2 enables only resampling, and mode 3 bypasses both. `DITHER_FILTER` remains an independent bit and selects restoration only for full-coverage RGBA16 samples. Backend-only callers without a live STATUS value retain an explicit `Unspecified` policy rather than impersonating one register code. | Exact public control topology and output-stage admission. Modes 0 and 1 intentionally have the same pixel result; their always-fetch versus fetch-when-needed bus behavior remains outside the deterministic framebuffer model. |
| RGBA16 dither restoration | Each full-coverage RGB component uses the signed comparisons against every available 3x3 neighbor. Corners use three neighbors and edges use five. Alpha is unchanged. | Exact implementation of the cited public mechanism. This is digital pre-DAC output, not an analog capture claim. |
| Partial-coverage silhouette AA | The preferred Figure 11 footprint is `(-1,-1)`, `(1,-1)`, `(-2,0)`, `(2,0)`, `(-1,1)`, `(1,1)`. Partial and out-of-frame neighbors are rejected. In interlaced fields the upper/lower offsets become two framebuffer lines, as the patent specifies. For each component, one minimum and maximum are rejected; the remaining penultimate interval is extended to contain the foreground and reflected across its midpoint to estimate the hidden background. Equation 4 then blends that estimate with the foreground using the resident coverage count. | Exact public topology and real-valued equation, with an explicit bounded integer realization: five-to-eight-bit expansion, saturating background, and round-to-nearest division by eight. The patent does not publish VI fixed-point rounding or overflow. If fewer than three full neighbors remain, the penultimate interval is undefined and a typed `InsufficientFullCoverage` state selects the named five-bit expansion policy. |
| Divot | If the left, center, or right source sample has partial coverage, each RGB component of the center output becomes the median of those three post-filter samples. Borders are unchanged. | The public horizontal-median mechanism is implemented. Complete silhouette output remains bounded by the preceding AA arithmetic and insufficient-neighborhood policy. |
| Active window, RDRAM source, X/Y resampling, and field provenance | One typed snapshot retains all fourteen live VI words at each exact retrace deadline. Every field crosses the renderer boundary even when a progressive register image repeats, because field cadence and the retrace-cycle noise seed remain distinct scanout inputs. A move-only request co-binds that image with a read-only capability over the current physical 8 MiB allocation; integrated execution creates it only while the guest is suspended and a safe backend cannot retain it. `H_START` supplies horizontal start/end in output pixels, `V_START` supplies vertical start/end in half-lines, and the post-VI image is allocated at `h_end - h_start` by `(v_end - v_start) / 2`; reversed windows, an active image with zero effective 12-bit source stride, and odd half-line extents trap. A jointly zero H/V window remains a typed inactive live image and never reads source bytes or falls back to host geometry. The reference path rereads the exact live 24-bit origin and effective 12-bit stride on every field, decodes RGBA16 with its correlated hidden-coverage sidecar or byte-gathered RGBA32, and stages all inferred hidden-bit changes until the complete scanout succeeds. A checked envelope covers the last vertical coordinate, a resampling lower sample, and the largest restoration/coverage-AA row halo; invalid bounds and odd RGBA16 origins are named errors, while blank images do not fetch. The same register snapshot retains the 12-bit U2.10 step and subpixel offset for each axis plus progressive/even/odd field identity. A checked host position computes exact `offset + coordinate * step`, retaining all ten fraction bits. Positions at or beyond the last loaded source index become typed `HeldLast`. Vertical interpolation between post-divot rows precedes horizontal interpolation; mode 3 uses the same coordinate generators with lower-sample replication. Gamma follows. Blank, fade, and repeat-line retain the programmed active extent. Backend-only callers without live registers retain explicit identity compatibility state. RT64 consumes the same current allocation and unmodified live origin/effective stride; pinned RT64 applies its one-row or odd-serrated two-row lookup bias exactly once, waits both native queues idle, and restores placeholder memory aliases before returning. | Active-window field units, the linear topology, register fields, ordering, and interlaced AA row spacing follow public interfaces and patents. The accumulated host integer width is deliberately unnamed. Ten-bit positive sums round to nearest, `HeldLast` is fn64's explicit high-border policy, and the checked bottom fetch envelope is a deterministic safety policy rather than a silicon bus claim. Exact silicon edge inclusion, accumulator/tie behavior, border fetch, origin alignment/masking beyond the named RGBA16 rejection, odd-half-line behavior, and field phase remain open; fn64 rejects rather than invents the odd-half-line case. Field-specific raw Y offsets are honored, but no additional phase is inferred from parity. |
| Gamma | `floor(sqrt(channel * 255))` maps eight-bit linear RGB to eight-bit output and preserves 0 and 255 exactly. | Deterministic integer realization of the documented square-root transfer. The exact silicon transfer ROM is not established by the public prose. |
| Gamma dither | One random bit stochastically rounds each RGB component to seven bits; the reference framebuffer expands that value back to eight-bit storage by replicating its high bit into output bit zero. | The stochastic seven-bit reduction follows the documented mechanism. Both eight-bit host representation and SplitMix64-derived coordinate/channel noise keyed by retrace guest cycle are explicit policies; the silicon generator, seed, and advancement are unpublished. |

The implemented composition order is STATUS-selected per-sample coverage AA or
full-coverage RGBA16 restoration, divot, STATUS-selected vertical resampling,
horizontal resampling, gamma, then gamma dither.

The wgpu scanout takes one immutable pre-restoration source snapshot and assigns
distinct output rows to Rayon workers. Rows therefore share no mutable bytes;
every neighbor read still comes from the same immutable snapshot, so scheduling
cannot feed a restored output back into another pixel. Setting
`FN64_PARALLEL_VI_DITHER=0` restores the scalar row walk as a measurement
control; absent or `1` selects the byte-identical parallel path, and every other
value traps rather than silently selecting a policy. A direct scalar/parallel
test covers borders, interior pixels, and mixed restoration eligibility.
`crates/fn64-render-reference/src/vi.rs`
contains exact vectors for the signed 3x3 and border cases, the preferred AA
footprint and interlaced row spacing, partial-neighbor rejection, the
insufficient-neighborhood state, exhaustive stored-color background equations
and coverage blends, all four AA/resample selector values, DITHER_FILTER
independence and full-coverage admission, identity/offset/half-step/clamped-edge resampling,
nonopaque host-alpha identity and fractional-position vectors, exhaustive
individual 12-bit step and offset positions, typed `HeldLast` boundary vectors,
exhaustive interpolation fractions and color endpoints, all three divot
coverage positions, gamma landmarks, both stochastic-rounding outcomes, the
seeded host-noise policy, and the composed pipeline. Active-window tests bind
an unequal source/output crop, the normal 640-dot window with `0x200` X scale,
and blank/fade/repeat geometry. Source-authority vectors additionally bind
nonzero RGBA16 origin with padded effective stride, unaligned RGBA32 origin
with odd stride, exact field origins, exact-edge and transactional
out-of-bounds behavior, blank/inactive no-read behavior, and rereading bytes
changed between retraces without a graphics task. Shared renderer and ABI
tests retain the exact fourteen-word image, raw-MMIO authority, and multi-field
deadline stepping; the RT64 adapter tests cross the Rust/C/C++ boundary and
preserve the current call's physical-memory authority.

Run the focused gate with:

```sh
cargo test -p fn64-render-reference vi::tests --lib
cargo test -p fn64-render-rt64 --features rt64 \
  ffi::tests::vi_status_wire_preserves_every_typed_antialias_mode --lib
cargo test -p fn64-render vi_ --lib
cargo test -p fn64-abi vi --lib
```

## Native RT64 pixel boundary

`rt64_vi_filter_behavior` observes pinned RT64's native Metal post-VI output
without treating RT64 as a silicon oracle. One coverage-isolated public
raw-RDP RGBA16 fixture is submitted once, then twenty complete live VI register
images cross the same context. Every phase retains the same nonzero workload
identity, a strictly increasing present identity, and exact 8x6 BGRA8
geometry. Six baseline observations return byte-for-byte to the first
baseline, and disabling gamma dither restores the exact gamma-only image.

The `vi-gamma-dither:v1`, `vi-dither-filter:v1`, `vi-divot:v1`, and
`vi-silhouette-aa:v1` source
overlays replace only pinned RT64's final VI fullscreen shaders. Gamma dither
applies the shared `fn64.vi-public-filters.bounded-v1` seven-bit quantizer
after RT64's gamma stage. The shader mirrors the SplitMix64-derived
coordinate/channel stream with paired 32-bit arithmetic because the supported
Metal shader target lacks native 64-bit integers. The complete retrace seed
crosses the Rust/C/C++ wire;
one ordinary fn64 VI event enqueues one ordinary RT64 presentation even when
the source/register image is unchanged. It does not relabel early presents or
alter RT64 workload history. Divot consumes RT64's framebuffer-alpha coverage
estimate, treats modulo-eight code seven as full coverage, and applies the
public componentwise horizontal median when a left/center/right triplet
contains a non-full sample. Dither restoration recovers each five-bit RGB
component from the synthetic RGBA16 source and applies the exact signed
comparison against every available neighbor in its 3x3 footprint only when
the center carries RT64's full-coverage code. It preserves alpha, feeds divot
before presentation sampling, and never substitutes clamped duplicate texels
for unavailable border neighbors. The positive restoration evidence below is
limited to nearest host filtering at native scale in one progressive synthetic
RGBA16 image; the shader's linear and anti-aliased-pixel-scaling paths are not
promoted by that result.

The device-free adapter-capture test preserves whether the typed AA selector
was actually supplied across Rust/C/C++, and `rt64_vi_aa_selector_behavior`
proves the same distinction at the native callback. Compatibility-only
`Unspecified` therefore cannot collapse onto hardware mode 0. Modes 0
and 1 apply the public Figure-11 six-neighbor filter to qualified RT64
managed-coverage codes 1 through 6; modes 2 and 3 leave those pixels unchanged.
Code 0 remains ambiguous across modulo-wrap and zero-destination
save/unqualified paths. Code 7 is treated as full, but pinned RT64 aliases
managed 7/8 and clamped 8/8 at that code; the fixture's opaque code-7 controls
are known full by construction, not by the encoding alone. An independent CPU
oracle expands each declared RGB5 source, asserts all six admitted neighbors
around each target are the fixture's full controls, discards one
componentwise minimum and maximum, and applies the penultimate interval and
Equation-4 blend independently for managed coverage codes 1 through 6. The
resulting exact RGB8 vectors, in coverage-code order, are `[50, 45, 35]`,
`[76, 60, 53]`, `[102, 70, 75]`, `[128, 87, 95]`, `[158, 95, 109]`, and
`[185, 113, 128]`. The divot oracle independently reconstructs each
horizontal triplet from the declared RDP source before projection rather than
borrowing already projected neighboring pixels. The eleven-phase Metal
fixture proves modes 0/1 are identical, modes 2/3 restore the pinned baseline,
compatibility `Unspecified` matches compatibility replicate while explicit
compatibility mode 0 matches the AA oracle, and AA, divot alone, and
AA-before-divot each change exactly the six projected target pixels. The four
exact BGRA8 SHA-256 digests are baseline
`6639c251163aa9dc6d660abf9da11a20bf29222b5d6d16ba0743f599e0666730`, AA
`83cf93557a7ad54d2a3d6badee86664b07f3df46383b28e16393a032ca9895f9`, divot
`8220a101f0de0ffdcefef798c2cec0fd46d3ff653de584a062c8fa86785e1801`, and
combined
`af2739c8bb26869cafbf62f62f52e343b61a38addb0df31aba3e09b5f4bda17b`; they
remain stable while one workload is retained and presentation identity
advances. This proves the bounded public arithmetic and stage order over
deliberately generated RT64-managed codes 1-6 with opaque code-7 controls in
pinned Metal's nearest, progressive, synthetic RGBA16 path under the
original-aspect (4:3) presentation policy. It does not promote that managed
alpha to authoritative N64 hidden coverage.

The eleven-phase selector fixture is embedded in every
`rt64_metal_backend_behavior` process. The recreated context remains live
across the compatibility present, live nearest-policy application, the
twenty-phase filter gate, a resize, and this selector gate. Exact identity is
`workload/present 1/1` for the recreated release present, `1/2` for
compatibility, workload 2 with presents 3 through 22 for filters, and workload
3 with presents 23 through 33 for the selector. On 2026-07-22 the expanded
codes-1-6 source passed the official watchdog-bounded backend-lifecycle runner
in 20/20 fresh processes on Darwin 25.5.0 arm64. The runner also enforces the
macOS surface-teardown interval, while each successful child drains the
present queue before destroying the final context.

| Native phase | Nonblack pixels | Unique colors |
| --- | ---: | ---: |
| Baseline, gamma, and their restorations | 48 | 6 |
| Dither-only and gamma-plus-dither, two seeds plus exact repeats | 48 | 15-17 |
| Coverage-gated divot median | 48 | 8 |
| Full-coverage RGBA16 dither restoration | 48 | 18 |
| Nonidentity 1.5x X/Y scale under all four AA selectors | 40 | 7 |

Gamma, gamma dither, divot, dither restoration, and nonidentity scale each
causally change the exact captured pixels. Three full-coverage divot-control
rows stay byte-identical; exactly twelve eligible pixels in the otherwise
identical non-full rows change, and every eligible BGRA component equals the
baseline horizontal median. Dither restoration changes exactly eighteen
eligible full-coverage pixels by the shared available-neighbor signed 3x3
formula, leaves all twenty-four non-full pixels and all six flat
full-coverage controls byte-identical, and preserves alpha everywhere.
Borders follow their exact available-neighbor footprints, and the adjacent
off phases restore the exact baseline.
Gamma dither is independently causal with gamma disabled or
enabled; two distinct retrace seeds produce distinct exact images, while an
identical repeated seed reproduces the exact image across a distinct present.
AA selector values 0-3 remain identical at the nonidentity scale because that
older fixture deliberately supplies only full code-7 and ambiguous
code-0/save rows. It is a negative control, not an AA implementation residual
or positive selector evidence. The
restoration closure is deliberately no broader than clean pinned Metal,
nearest filtering, native scale, progressive scanout, and this synthetic
RGBA16 source. A managed RT64 target does not retain authoritative per-pixel
N64 dither history or complete physical hidden coverage; linear and
anti-aliased-pixel-scaling filtering, enhancement resolution, MSAA,
downsampling, D3D12, Vulkan, and representative full-ROM presentation
remain uncertified. The
fixture fails if a source update changes either side of that boundary without
an explicit review. This proves the bounded deterministic native mechanism;
it does not establish full-ROM behavior, the unpublished silicon random
stream, physical-console filter arithmetic, or analog-video parity.

Run the native gate directly with:

```sh
cargo run -p fn64-certification --features rt64 --example rt64_vi_filter_behavior
```

## Physical-video capture admission

`VI-ANALOG-CAPTURE-PROGRAM.md` defines the strict external-evidence boundary
for the still-open analog frontier. `tools/vi-analog-captures` separately
hashes a public synthetic digital vector and its lossless composite/S-Video
artifact, binds the complete register/filter/region/field/reset/repeat and
capture-chain identities, and requires ten distinct power-cycle hardware runs
for a controlled cohort. Synthetic manifests validate only as explicitly
non-certifying fixtures. The repository contains no physical captures, and the
tool's consensus result cannot close the base-renderer row by itself.

## Still open

- Exact VI coverage-AA fixed-point rounding/overflow and behavior with fewer
  than three full neighbors.
- Exact resampling accumulator precision/rounding, border fetch and bottom-row
  bus envelope, active-window edge inclusion, origin alignment/masking,
  odd-half-line behavior, any phase beyond the field-specific raw Y offset,
  and field/AA-mode fetch timing.
- Coverage-centroid/subpixel attribute selection before framebuffer storage.
- The silicon gamma transfer table and random-stream identity.
- Exact interaction timing between filter blocks and field/retrace state.
- Mode-0 unconditional versus mode-1 conditional extra-line fetch timing and
  memory-bus behavior; their public pixel-stage selection is implemented.
- Native RT64 AA-selector arithmetic is certified for deliberately generated
  managed codes 1-6 with opaque code-7 controls in the pinned-Metal, nearest,
  progressive synthetic RGBA16 projection under the original-aspect (4:3)
  presentation policy. Code 7 aliases RT64's managed 7/8 and clamped 8/8
  estimates. Code 0/save semantics, natural triangle coverage, imported
  framebuffer hidden coverage, insufficient-neighborhood output,
  interlaced/scaled and linear/anti-aliased-pixel-scaling paths, enhancement
  resolution,
  MSAA/downsample resolve semantics, D3D12, Vulkan, and representative
  full-ROM presentation remain open rather than inheriting that narrow
  result. RGBA16 dither restoration has the same bounded managed-source
  provenance.
- Video DAC conversion, composite/S-Video encoding, bandwidth limits, and
  analog NTSC/PAL/MPAL output characteristics.
- Pixel-level hardware traces spanning the framebuffer through physical video
  output.

These gaps prevent closing the broader "analog VI" frontier. The deterministic
digital mechanisms above are suitable for repeatable reference-renderer
digests, but those digests are not evidence of physical-console video parity.
