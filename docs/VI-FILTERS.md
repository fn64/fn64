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
| X/Y resampling and field provenance | A latched mode supplies typed 12-bit U2.10 step and subpixel-offset fields for each axis plus progressive/even/odd field identity. A checked host position type computes exact `offset + coordinate * step`, then separates the integer source index from all ten register fraction bits. Interior samples retain adjacent indices and the exact fraction. Positions at or beyond the last source index become a typed `HeldLast` boundary with one canonical last/last sample and zero fraction. Vertical interpolation between successive post-divot rows runs first; horizontal interpolation between neighboring vertical results runs second; gamma follows both. All four stored host framebuffer channels are interpolated so identity scanout preserves nonopaque alpha; this is a host-representation contract, not a VI silicon-alpha claim. Backend-only callers without live VI registers retain identity scanout. | The topology, linear equation, register fields, order, and interlaced AA row spacing follow the public patents. The accumulated host integer width is deliberately not named as a hardware format. Ten-bit positive weighted sums round to nearest, `HeldLast` is fn64's explicit high-border policy, and the configured framebuffer is both source and output extent; accumulator precision, tie behavior, silicon border fetch, and active-window extent mapping are unpublished. Field-specific raw Y offsets are honored, but no additional half-line phase is invented from field parity. |
| Gamma | `floor(sqrt(channel * 255))` maps eight-bit linear RGB to eight-bit output and preserves 0 and 255 exactly. | Deterministic integer realization of the documented square-root transfer. The exact silicon transfer ROM is not established by the public prose. |
| Gamma dither | One random bit stochastically rounds each RGB component to seven bits; the reference framebuffer expands that value back to eight-bit storage by replicating its high bit into output bit zero. | The stochastic seven-bit reduction follows the documented mechanism. Both eight-bit host representation and SplitMix64-derived coordinate/channel noise keyed by retrace guest cycle are explicit policies; the silicon generator, seed, and advancement are unpublished. |

The implemented composition order is STATUS-selected per-sample coverage AA or
full-coverage RGBA16 restoration, divot, STATUS-selected vertical resampling,
horizontal resampling, gamma, then gamma dither.
`crates/fn64-render-rt64/src/vi.rs`
contains exact vectors for the signed 3x3 and border cases, the preferred AA
footprint and interlaced row spacing, partial-neighbor rejection, the
insufficient-neighborhood state, exhaustive stored-color background equations
and coverage blends, all four AA/resample selector values, DITHER_FILTER
independence and full-coverage admission, identity/offset/half-step/clamped-edge resampling,
nonopaque host-alpha identity and fractional-position vectors, exhaustive
individual 12-bit step and offset positions, typed `HeldLast` boundary vectors,
exhaustive interpolation fractions and color endpoints, all three divot
coverage positions, gamma landmarks, both stochastic-rounding outcomes, the
seeded host-noise policy, and the composed pipeline. Shared renderer and ABI
tests exhaust the raw scale/offset field decode and prove live retrace wiring.

Run the focused gate with:

```sh
cargo test -p fn64-render-rt64 vi::tests --lib
cargo test -p fn64-render-rt64 --features rt64 \
  ffi::tests::vi_status_wire_preserves_every_typed_antialias_mode --lib
cargo test -p fn64-render vi_ --lib
cargo test -p fn64-abi vi --lib
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
- Exact resampling accumulator precision/rounding, border fetch, active-window
  output extent, any half-line phase beyond the field-specific raw Y offset,
  and field/AA-mode fetch timing.
- Coverage-centroid/subpixel attribute selection before framebuffer storage.
- The silicon gamma transfer table and random-stream identity.
- Exact interaction timing between filter blocks and field/retrace state.
- Mode-0 unconditional versus mode-1 conditional extra-line fetch timing and
  memory-bus behavior; their public pixel-stage selection is implemented.
- Video DAC conversion, composite/S-Video encoding, bandwidth limits, and
  analog NTSC/PAL/MPAL output characteristics.
- Pixel-level hardware traces spanning the framebuffer through physical video
  output.

These gaps prevent closing the broader "analog VI" frontier. The deterministic
digital mechanisms above are suitable for repeatable reference-renderer
digests, but those digests are not evidence of physical-console video parity.
