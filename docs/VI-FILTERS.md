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
| Active window, RDRAM source, X/Y resampling, and field provenance | One typed snapshot retains all fourteen live VI words at each exact retrace deadline. Every field crosses the renderer boundary even when a progressive register image repeats, because field cadence and the retrace-cycle noise seed remain distinct scanout inputs. A move-only request co-binds that image with a read-only capability over the current physical 8 MiB allocation; integrated execution creates it only while the guest is suspended and a safe backend cannot retain it. `H_START` supplies horizontal start/end in output pixels, `V_START` supplies vertical start/end in half-lines, and the post-VI image is allocated at `h_end - h_start` by `(v_end - v_start) / 2`; reversed windows, an active image with zero effective 12-bit source stride, and odd half-line extents trap. A jointly zero H/V window remains a typed inactive live image and never reads source bytes or falls back to host geometry. The reference path rereads the exact live 24-bit origin and effective 12-bit stride on every field, decodes RGBA16 with its correlated hidden-coverage sidecar or byte-gathered RGBA32, and stages all inferred hidden-bit changes until the complete scanout succeeds. A checked envelope covers the last vertical coordinate, a resampling lower sample, and the largest restoration/coverage-AA row halo; invalid bounds and odd RGBA16 origins are named errors, while blank images do not fetch. The same register snapshot retains the 12-bit U2.10 step and subpixel offset for each axis plus progressive/even/odd field identity. A checked host position computes exact `offset + coordinate * step`, retaining all ten fraction bits. Positions at or beyond the last loaded source index become typed `HeldLast`. Vertical interpolation between post-divot rows precedes horizontal interpolation; mode 3 uses the same coordinate generators with lower-sample replication. Gamma follows. Blank, fade, and repeat-line retain the programmed active extent. Backend-only callers without live registers retain explicit identity compatibility state. RT64 consumes the same current allocation and live origin/effective stride, applies its one-row or odd-serrated two-row origin bias, waits both native queues idle, and restores placeholder memory aliases before returning. | Active-window field units, the linear topology, register fields, ordering, and interlaced AA row spacing follow public interfaces and patents. The accumulated host integer width is deliberately unnamed. Ten-bit positive sums round to nearest, `HeldLast` is fn64's explicit high-border policy, and the checked bottom fetch envelope is a deterministic safety policy rather than a silicon bus claim. Exact silicon edge inclusion, accumulator/tie behavior, border fetch, origin alignment/masking beyond the named RGBA16 rejection, odd-half-line behavior, and field phase remain open; fn64 rejects rather than invents the odd-half-line case. Field-specific raw Y offsets are honored, but no additional phase is inferred from parity. |
| Gamma | `floor(sqrt(channel * 255))` maps eight-bit linear RGB to eight-bit output and preserves 0 and 255 exactly. | Deterministic integer realization of the documented square-root transfer. The exact silicon transfer ROM is not established by the public prose. |
| Gamma dither | One random bit stochastically rounds each RGB component to seven bits; the reference framebuffer expands that value back to eight-bit storage by replicating its high bit into output bit zero. | The stochastic seven-bit reduction follows the documented mechanism. Both eight-bit host representation and SplitMix64-derived coordinate/channel noise keyed by retrace guest cycle are explicit policies; the silicon generator, seed, and advancement are unpublished. |

The implemented composition order is STATUS-selected per-sample coverage AA or
full-coverage RGBA16 restoration, divot, STATUS-selected vertical resampling,
horizontal resampling, gamma, then gamma dither.
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
without treating RT64 as a silicon oracle. One asymmetric public raw-RDP
RGBA16 fixture is submitted once, then twenty complete live VI register
images cross the same context. Every phase retains the same nonzero workload
identity, a strictly increasing present identity, and exact 8x6 BGRA8
geometry. Five baseline observations return byte-for-byte to the first
baseline, and disabling gamma dither restores the exact gamma-only image.

The `vi-gamma-dither:v1` source overlay replaces only pinned RT64's final VI
fullscreen shaders. It applies the shared
`fn64.vi-public-filters.bounded-v1` seven-bit quantizer after RT64's gamma
stage. The shader mirrors the SplitMix64-derived coordinate/channel stream
with paired 32-bit arithmetic because the supported Metal shader target lacks
native 64-bit integers. The complete retrace seed crosses the Rust/C/C++ wire;
one ordinary fn64 VI event enqueues one ordinary RT64 presentation even when
the source/register image is unchanged. It does not relabel early presents or
alter RT64 workload history.

Twenty fresh Metal processes retained byte-identical complete logs and every
exact SHA-256 identity enforced by the standalone gate. The run closes the
present-queue interleaving named at the native wait site: a preceding
process-time early present cannot retain the prior VI policy while the next
retrace replaces its seed. The expanded full `rt64_metal_backend_behavior`
process invokes this gate in a fresh context and separately completed the
official watchdog-bounded 20-process backend-lifecycle bar with the required
macOS surface-teardown interval:

| Native phase | Nonblack pixels | Unique colors |
| --- | ---: | ---: |
| Baseline, gamma, and their restorations | 48 | 48 |
| Dither-only and gamma-plus-dither, two seeds plus exact repeats | 48 | 48 |
| Nonidentity 1.5x X/Y scale under all four AA selectors | 40 | 41 |

Gamma, gamma dither, and nonidentity scale each causally change the exact
captured pixels. Gamma dither is independently causal with gamma disabled or
enabled; two distinct retrace seeds produce distinct exact images, while an
identical repeated seed reproduces the exact image across a distinct present.
Divot and `DITHER_FILTER` remain identical to baseline, and AA selector values
0-3 remain identical at the nonidentity scale. Those equalities are named
native implementation residuals rather than positive filter results. The
fixture fails if a source update changes either side of that boundary without
an explicit review. This proves the bounded deterministic native mechanism;
it does not establish the unpublished silicon random stream, physical-console
filter arithmetic, or analog-video parity.

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
- Native RT64 divot, RGBA16 dither-restoration, and AA-selector pixel behavior.
  The live Metal gate currently preserves each as an exact pixel-inert
  implementation residual at the pinned source revision. Their exact public
  mechanisms require source-neighborhood or coverage state that is not yet
  available at the final sampled-color VI shader stage.
- Video DAC conversion, composite/S-Video encoding, bandwidth limits, and
  analog NTSC/PAL/MPAL output characteristics.
- Pixel-level hardware traces spanning the framebuffer through physical video
  output.

These gaps prevent closing the broader "analog VI" frontier. The deterministic
digital mechanisms above are suitable for repeatable reference-renderer
digests, but those digests are not evidence of physical-console video parity.
