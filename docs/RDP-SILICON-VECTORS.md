# Raw-DPC silicon-vector evidence

Status: strict capture-independent schema and validator implemented. No
hardware capture has been performed and no silicon-accuracy claim is made.

`tools/rdp-silicon-vectors` accepts only synthetic raw-DPC evidence bundles.
Each case binds all of the following in one strictly parsed JSON document:

- exact command bytes, byte length, and SHA-256;
- required pre-submit DPC START, END, and STATUS observations, plus optional
  MI/VI setup registers;
- non-overlapping, content-digested initial physical-RDRAM regions;
- exact framebuffer and depth bytes with address, format, dimensions, stride,
  and digest;
- a mandatory coverage plane tied to the framebuffer address and geometry;
- producer, platform, adapter, executable, settings, capture method, and UTC
  provenance.
- optional typed capture intent for cross-case experiments whose completeness
  is checked separately from the generic evidence envelope.

Unknown fields, missing channels or registers, duplicate case/register/region
identities, overlapping input regions, malformed lowercase hexadecimal,
digest mismatches, invalid physical ranges, geometry/length mismatches, and
out-of-range normalized coverage values fail loudly. The validator returns a
SHA-256 over its deterministic serialization of the fully validated bundle.
That digest proves the same envelope was compared; it does not prove the
producer is hardware.

Physical VI output is intentionally outside the raw-DPC bundle. A framebuffer,
depth plane, or hidden-coverage observation is digital device state, not an
analog composite/S-Video capture. `VI-ANALOG-CAPTURE-PROGRAM.md` and
`tools/vi-analog-captures` provide the separate schema that binds one public
digital VI vector to lossless physical-video artifacts and repeated hardware
provenance without promoting either schema fixture into hardware evidence.

## Repeated-capture consensus

The companion consensus validator treats every JSON bundle as one independent
capture run. Its CLI defaults `--min-runs` to 10, matching the deterministic
validation bar in `AGENTS.md`:

```sh
cargo run --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- consensus \
  capture-01.json capture-02.json capture-03.json capture-04.json \
  capture-05.json capture-06.json capture-07.json capture-08.json \
  capture-09.json capture-10.json
```

The `consensus` word is optional; consensus remains the default mode. Consensus
fails unless every producer is explicitly `hardware`, the requested
minimum is nonzero and satisfied by distinct bundle digests, and every run has
the same suite/case identity, command input, setup-register sequence,
initial-memory sequence, and output geometry. It then requires byte-identical
framebuffer, depth, and coverage channels. A failure names the first run,
field, and—when bytes differ—the first byte offset and values.

Controlled runs must also use identical producer name/version, platform,
adapter name/version, producer-binary digest, settings digest, and capture
method. Their `recorded_at_utc` values must be distinct. This prevents changes
to the capture stack from being counted as repeated trials of one setup.

The result retains each run's complete `Producer` record and content-bound
bundle digest. Runs are sorted by bundle digest before the consensus digest is
calculated, so caller file order cannot change the result and per-run UTC,
binary, and settings provenance is never collapsed into a shared summary.
Passing a lower explicit `--min-runs` can support exploratory capture work but
does not satisfy fn64's ten-run deterministic claim bar.

The three coverage encodings are deliberately distinct. `rgba16_hidden_bits_u2`
is one normalized byte per pixel containing the two physical hidden bits only;
`stored_coverage_u3` contains the reconstructed stored value 0 through 7; and
`coverage_count_u4` contains the reconstructed sample count 0 through 8. A
producer must name which observation it supplied rather than silently treating
these representations as interchangeable.

The schema requires `content_class: "synthetic_raw_dpc"`. ROM bytes, game
content, and recompiled-game output are not admitted. The synthetic unit
fixture constructs bytes directly and carries `producer.kind` equal to
`synthetic_fixture`; it exercises the interchange contract but is not renderer
or hardware evidence.

## RGB dither channel sweep

`capture_intent.kind: "rgb_dither_sweep"` isolates one selected RGB input
channel over the complete 0 through 255 domain while holding the other two
channels fixed. Each input point is replayed from reset and produces one exact
4x4 RGBA16 tile at a declared screen origin. A sweep selects exactly one public
Other Modes RGB-dither value (`magic_square`, `bayer`, `noise`, or `disabled`),
one channel, and one fragment-noise sample index. Analyze it with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-rgb-dither magic-red-v1 /path/to/capture.json
```

The analyzer requires all 256 channel values in both one-cycle and two-cycle
modes. Selector, non-swept input channels, origin, sample index, framebuffer,
depth, and coverage geometry must remain identical throughout. Missing or
duplicate values, reset omissions, selected-channel mismatches, geometry
drift, or a framebuffer other than tightly packed 4x4 RGBA16 fail loudly.

The result preserves two lowercase hexadecimal digits for every observed
five-bit channel code in input-then-row-then-column order. It also reports the
number of distinct output codes, per-pixel monotonicity, and exact cross-cycle
equality. Non-monotonic or cycle-dependent output is retained rather than
rejected. One sweep intentionally covers only one selector/channel pair: run
separate named sweeps for all selector and channel combinations so changing a
noise sequence cannot be hidden inside an aggregate comparison.

As with every typed intent here, the producer asserts that the opaque command
bytes implement the declared combiner color and selector. A hardware claim
still requires review of that command generator and repeated physical-console
provenance. The in-tree synthetic tile validates only completeness and
reduction.

## Alpha-compare dither threshold sweep

The public SGI *RDP Command Summary* Other Modes command names
`G_AC_DITHER`, but it does not publish the random generator, its reset seed or
advancement, or the exact comparison at threshold ties. A bundle can attach
`capture_intent.kind: "alpha_compare_dither_sweep"` to controlled probe cases
and run:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-alpha-dither sample-zero /path/to/capture.json
```

The analyzer requires all 256 combined-alpha values for both `one_cycle` and
`two_cycle`, exactly one point per value. Every point must declare replay from
reset, the same fragment-noise sample index, and the same distinct pass/reject
RGBA16 markers. The observed target must be a one-pixel RGBA16 plane equal to
one of those markers. Missing/duplicate values, changed controls, unknown
outputs, or ambiguous markers fail loudly. The JSON result preserves each
cycle's exact 256-bit pass bitmap and reports pass/transition counts, the first
pass, any later reject, and monotonicity. `null` preserves an all-rejected
endpoint. Non-monotonic results and a cross-cycle mismatch are evidence to
retain, not validation failures. `cycle_transitions_match` is true only when
the one-cycle and two-cycle 256-bit pass bitmaps are bit-for-bit identical; an
equal first-passing alpha alone is not a match.

Capture intent is an auditable producer assertion. This ingest tool cannot
decode the opaque command stream to prove that its combiner and Other Modes
bits match the declaration. A hardware claim therefore still requires review
of the synthetic command generator plus real producer provenance. If reset
does not reproduce one noise sample, the pass bitmap exposes that failure of
the controlled-sample premise instead of hiding the observation. Ten
byte-identical bundles may use `consensus`; varying silicon sequences must not
be forced through that deterministic consensus mode.

## Alpha/coverage product sweep

`capture_intent.kind: "alpha_coverage_product_sweep"` turns the complete
public `CVG_X_ALPHA` input domain into one mechanically checked experiment:
input coverage 1 through 8, combined alpha 0 through 255, and both one-cycle
and two-cycle modes. Every point must replay from reset and expose an exact
one-pixel `coverage_count_u4` plane. Analyze it with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-alpha-coverage product-v1 /path/to/capture.json
```

The analyzer rejects any missing or duplicate point and preserves all 256
observed output counts for each of the sixteen curves as hexadecimal digits.
It reports the raw alpha-zero observation separately because a zero-product
rejection can leave the cleared full coverage untouched; first nonzero/full
alpha and monotonicity therefore start at alpha one. Transition count still
covers the raw 256-point observation. Non-monotonic and cycle-dependent output
is retained as evidence. As with alpha-dither intent, the producer asserts that the opaque
commands clear memory coverage to full and select `CVG_DST_WRAP`; review of
the out-of-tree command generator and real hardware provenance remains
mandatory.

`capture_intent.kind: "coverage_to_alpha_sweep"` complements that product
experiment. It disables `CVG_X_ALPHA`, enables `ALPHA_CVG_SEL` and
`G_AC_THRESHOLD`, and sweeps threshold alpha 0 through 255 for input coverage
1 through 8 in both cycle modes. Every point replays from reset and uses fixed,
distinct one-pixel RGBA16 pass/reject markers. Analyze it with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-coverage-alpha coverage-alpha-v1 /path/to/capture.json
```

The result preserves every 256-bit pass bitmap, the greatest passing
threshold, transition count, monotonicity, and exact cross-cycle equality.
This directly exposes the eight selected pixel-alpha codes and their tie
behavior without inferring them from blender RGB.

## Blender precision and adjacent-pixel memory feedback

Programming Manual Chapter 15.5 establishes the framebuffer blender's
five-bit alpha inputs, selector topology, force-blend control, memory color,
and two-cycle combined-color handoff. It does not publish the divider width,
denominator boundary behavior, rounding, special bypass precision, or the
relative memory-color/coverage timing of adjacent pixels.
`capture_intent.kind: "blender_precision_boundary_sweep"` and
`"blender_memory_feedback_pair"` define a finite final-output experiment for
those unknowns without supplying a host formula. Analyze one matrix with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-blender-precision blender-precision-v1 /path/to/capture.json
```

The precision denominator is exactly 72 reset-isolated points: ordinary,
force-blend, and fog-pass modes; one-cycle and two-cycle operation; isolated
five-bit alpha codes `0`, `1`, `30`, and `31`; and producer-declared
denominators exactly one below, on, and one above one shared unsigned six-bit
boundary. Every point holds pixel, memory, and fog colors, setup, sample
coordinate, RGBA32/depth/stored-coverage geometry, depth, and coverage controls
fixed. Expected RGBA32 values remain producer declarations and must agree
across cycle modes for the same mode/alpha/boundary point. The analyzer never
derives the denominator from the alpha code and never infers division or
rounding from the expected marker.

Three additional cases—one per mode—each start from reset and execute their
two horizontally adjacent pixels inside one two-cycle command stream. The
declared `ordered_pair_command_sha256` must equal the exact command blob
digest, binding the pair's order without pretending the validator can decode
opaque commands. Output geometry is tightly packed 2x1 RGBA32, depth, and
normalized `stored_coverage_u3`. Distinct producer-declared cycle-one-handoff
and prior-memory color/coverage markers classify the second pixel while its
first and second raw framebuffer, depth, and coverage values remain intact.
The setup schema cannot directly initialize RGBA16 hidden coverage; a real
producer must establish any memory-coverage prestate in its reviewed command
prelude. Final planes expose consequences only and are not a direct probe of
an internal cycle-one register.

Missing or duplicate cells, a non-extreme alpha code, false or non-adjacent
denominator labels, reset omissions, mode/cycle/control/setup/geometry drift,
a one-cycle feedback label, non-adjacent pair coordinates, command-digest
drift, and ambiguous candidate markers fail loudly. Results have fixed
mode/cycle/alpha/below-on-above order, preserve exact raw observations and
one/two-cycle divergence counts, and classify every ordered pair without
rejecting an unknown output. `analysis_sha256` domain-separates and binds the
validated bundle digest, producer kind, all controls and geometry, raw planes,
expectation results, pair classifications, and divergence counters.

The result always emits `base_matrix_row_closed: false`. The included
synthetic fixture executes no RDP work, and even one hardware bundle is not
consensus. Renderer arithmetic or timing changes require a reviewed producer
and ten byte-identical physical-console runs through the existing consensus
gate; surprising or divergent output remains evidence rather than being
normalized away.

## Three-nearest texture-filter tie boundary

`capture_intent.kind: "texture_filter_tie_sweep"` defines one deliberately
small fixed-point experiment around the three-nearest fractional diagonal.
Each point declares the exact signed ten-bit integer S/T texel coordinates,
unsigned five-bit S/T fractions, unsigned six-bit diagonal boundary, 2x2
RGBA16 texels and their physical address, and the output sample coordinate.
The `below`, `on`, and `above` labels are not trusted as prose: their fraction
sums must equal boundary minus one, boundary, and boundary plus one exactly.

Analyze one complete matrix with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-texture-filter-tie three-nearest-diagonal-v1 /path/to/capture.json
```

The analyzer requires all three boundary positions in both one-cycle and
two-cycle modes. Every point must replay from reset and retain identical setup
registers, initial memory, texture bytes, integer coordinates, boundary,
sample location, and output addresses. Each named position must also use the
same S/T fraction pair across cycle modes. Output geometry is exactly one
tightly packed RGBA32 pixel, one depth halfword, and one normalized
`stored_coverage_u3` value. Missing/duplicate points, reset omissions, false
boundary labels, texture/control/setup drift, or output-geometry drift fail
loudly.

The result preserves exact framebuffer, depth, and coverage observations in
fixed cycle-then-below/on/above order. A one/two-cycle difference sets
`cycle_results_match` false without rejecting or normalizing either result.
`analysis_sha256` binds the validated bundle digest, all numeric controls,
fixed geometry, exact observations, and comparison result through a
domain-separated deterministic JSON wire.

This is capture intent, not a filter implementation or hardware claim. The
tool does not decode opaque RDP commands to prove they implement the declared
coordinates, and its synthetic test matrix performs no RDP execution. A
numeric renderer change still requires review of the out-of-tree command
generator and repeated physical-console captures; divergent hardware runs
must be retained rather than forced into consensus.

## Reciprocal-to-signed-S10.5 boundary

Programming Manual sections 13.7 and 13.11 establish five fractional texture
coordinate bits and the public signed S10.5 input range, but do not publish the
perspective reciprocal or quantization arithmetic that lands on that grid.
`capture_intent.kind: "reciprocal_s10_5_boundary_sweep"` records a bounded
six-point experiment without filling that hardware gap with a host formula.
Analyze one matrix with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-reciprocal-s10-5 reciprocal-grid-v1 /path/to/capture.json
```

Each point declares an exact signed perspective numerator, nonzero unsigned
denominator, signed S10.5 boundary, producer-expected S10.5 output, and its
RGBA32 output marker. The three rational inputs must share one denominator
and use exact numerators `boundary - 1`, `boundary`, and `boundary + 1` after
scaling the signed S10.5 boundary by that denominator. Individual `below`,
`on`, and `above` labels are also checked by exact integer cross-
multiplication. This verifies the intended input geometry; it does not assert
that silicon implements the producer's expected output.

The analyzer requires all three positions in both one-cycle and two-cycle
modes. Every point replays from reset with byte-identical setup, sample
coordinate, boundary, depth and coverage controls, and fixed output
addresses. Each position's numerator, denominator, expected S10.5 output, and
marker must be identical across cycle modes. Equal expected S10.5 outputs must
use equal markers and different expected outputs must use different markers.
Output geometry is exactly one tightly packed RGBA32 pixel, one depth
halfword, and one normalized `stored_coverage_u3` value; depth and coverage
must remain at their declared isolation controls. Missing or duplicate
points, reset omissions, false labels, non-adjacent inputs, cross-cycle
coordinate drift, ambiguous markers, setup/control drift, and geometry drift
fail loudly.

The result retains exact observations in fixed cycle-then-below/on/above
order. A known output marker produces `observed_output_s10_5_i16`; an unknown
color remains as raw RGBA32 with that field absent and increments
`unexpected_output_count`. It is not rejected or rewritten.
`cycle_results_match` likewise preserves one/two-cycle divergence. The
domain-separated `analysis_sha256` binds the source bundle digest, exact
numeric inputs, producer expectations, fixed controls and geometry, all raw
observations, and comparison fields.

This is envelope and producer-intent validation only. The tool does not decode
the opaque RDP commands, prove that they supply the declared perspective
values, infer reciprocal precision or rounding, or promote its synthetic
fixture to hardware evidence. Any renderer arithmetic change requires a
reviewed command producer and repeated physical-console captures; divergent
runs remain evidence rather than being forced into consensus.

## Average-filter output-tie boundary

The public texture-filter description identifies the four-texel average mode,
but does not publish the accumulator width or output tie rule.
`capture_intent.kind: "average_filter_output_tie_sweep"` records a strict
six-point boundary experiment around one isolated RGBA channel without
deriving that missing arithmetic. Analyze it with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-average-filter-tie average-red-tie-v1 /path/to/capture.json
```

Each point declares the exact 2x2 RGBA16 texels and aligned physical texture
address, signed ten-bit integer S/T coordinates, unsigned five-bit S/T
fractions, isolated output channel, rational accumulator numerator and
nonzero denominator, tie numerator, producer-expected eight-bit channel
value, and corresponding RGBA32 output marker. The three points share one
denominator and must use exact accumulator numerators `tie - 1`, `tie`, and
`tie + 1`; their fraction pairs must be distinct. Exact integer comparison
also verifies each `below`, `on`, or `above` label. The analyzer deliberately
does not recompute the declared accumulator from the texels and coordinates:
that relationship remains subject to command-producer review rather than
being filled in with a host averaging formula.

The complete matrix contains all three positions in one-cycle and two-cycle
modes. Every point must replay from reset with byte-identical setup, texture
bytes, integer coordinates, isolated channel, rational tie, sample location,
depth and coverage controls, and output addresses. Each position's fractional
coordinates, accumulator numerator, expected channel value, and marker must
also match across cycle modes. Equal expected values use equal markers and
different values use different markers. Output geometry is exactly one
tightly packed RGBA32 pixel, one depth halfword, and one normalized
`stored_coverage_u3` value; depth and coverage remain fixed isolation
controls. Missing or duplicate points, false/non-adjacent labels, reset
omissions, texture/setup/geometry drift, cross-cycle coordinate or accumulator
drift, and ambiguous markers fail loudly.

The deterministic result orders observations by cycle and then
below/on/above. A recognized marker exposes `observed_output_u8`; an unknown
color remains as raw RGBA32 with no decoded value and increments
`unexpected_output_count`. One/two-cycle differences similarly remain in the
observations and set `cycle_results_match` false. The domain-separated
`analysis_sha256` binds the source bundle digest, texture and rational
controls, fixed geometry, exact raw observations, and comparison results.

This analyzer proves schema completeness and deterministic reduction, not an
average-filter formula or a hardware result. The opaque command stream is not
decoded, and the included synthetic fixture executes no RDP work. A renderer
change requires a reviewed producer plus repeated physical-console captures;
unknown or divergent outputs must remain visible through that process.

## Texture derivative and LOD boundaries

Programming Manual section 13.7 describes adjacent-coordinate derivatives
and the public mip, detail, and sharpen selection families, but does not
publish the complete derivative norm, quantization width, or boundary
rounding. `capture_intent.kind: "texture_lod_boundary_sweep"` records an exact
18-point evidence denominator without supplying those missing rules. Analyze
one matrix with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-texture-lod-boundary lod-boundary-v1 /path/to/capture.json
```

Every point declares the center, +X-neighbor, and +Y-neighbor S/T coordinates
as signed S10.5 codes plus all four signed S10.5 derivatives. The validator
requires each derivative to equal the corresponding neighbor minus center
exactly. It also records a nonzero-denominator rational LOD metric, one
producer-declared boundary numerator, primitive tile, maximum mip level,
minimum LOD, and a producer-expected tile pair and signed S9.8 LOD fraction.
The tool checks the declared metric's below/on/above relation and requires
exact numerator `boundary - 1`, `boundary`, and `boundary + 1` inputs with
distinct derivative tuples. It deliberately does not derive the metric from
the derivatives or infer the expected tile/fraction selection.

The complete matrix is `mip`, `detail`, and `sharpen`, each at all three
boundary positions in both one-cycle and two-cycle modes. Every point replays
from reset and holds setup, sample location, rational boundary, tile/minimum-
LOD controls, depth/coverage controls, and exact one-pixel RGBA32, depth, and
`stored_coverage_u3` geometry fixed. Each named position must use identical
coordinates, derivatives, and metric across all modes and cycles. Each
mode/position expected tile pair, fraction, and RGBA32 marker must match
across cycle modes. Equal expected selections use equal markers; distinct
selections use distinct markers. Missing/duplicate points, false labels,
coordinate/derivative inconsistency, non-adjacent metrics, cross-mode or
cross-cycle drift, ambiguous markers, reset omissions, setup drift, and
output-geometry drift fail loudly. Raw plane contents remain observations.

Results use fixed mode, cycle, then below/on/above order. Known markers expose
`observed_selection`; unknown colors remain as raw RGBA32 with no decoded
selection and increment `unexpected_output_count`. Each mode retains its own
`cycle_results_match`, and `all_cycle_results_match` does not normalize a
difference away. Depth and stored coverage are also retained exactly; values
outside their fixed producer-declared controls increment
`unexpected_depth_count` or `unexpected_coverage_count` rather than failing
validation. The domain-separated `analysis_sha256` binds the source bundle
digest, exact coordinates and derivatives, all producer controls and
expectations, fixed geometry, raw observations, and comparisons.

This analyzer proves only envelope completeness and deterministic reduction.
It neither decodes the opaque command stream nor promotes the synthetic test
fixture to hardware evidence. Derivative/LOD arithmetic may change only after
the producer is reviewed and repeated physical-console captures establish a
stable result; unknown or divergent observations remain evidence.

## ZMODE_INTER admission and stored-coverage sweep

The public Programming Manual describes interpenetrating Z mode and identifies
coverage adjustment as part of its behavior, but it does not publish the
numeric adjustment. `capture_intent.kind:
"z_mode_inter_coverage_sweep"` defines a bounded experiment for observing that
frontier without fitting a host formula to it. Analyze a capture with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-zmode-inter inter-v1 /path/to/capture.json
```

A complete sweep has exactly 384 independently reset points: both `one_cycle`
and `two_cycle`; `in_front_control`, `interpenetrating`, and `behind_control`
relations; incoming coverage 1 through 8; and initial stored coverage 0 through
7. Each point declares the unsigned 18-bit incoming/memory Z values and the
unsigned 16-bit incoming/memory DeltaZ values used by its relation. Controls
must be identical across every point bearing one relation label and across
both cycle modes. Two relation labels may not reuse the same numeric control
tuple. This rejects accidental relabeling while leaving the producer
responsible for proving that the opaque command bytes implement the declared
geometry.

Every point must declare replay from reset and use fixed, distinct RGBA16
pass/reject markers. The output is exactly one RGBA16 pixel, one depth pixel,
and one normalized `stored_coverage_u3` value, with fixed addresses throughout
the sweep. Missing or duplicate matrix points, reset omissions, marker or
geometry drift, inconsistent per-label controls, cross-label control reuse,
unknown admission markers, and out-of-domain inputs fail loudly.

The result contains six summaries in fixed cycle-then-relation order. Each
summary preserves an exact 64-bit admission bitmap and 64 hexadecimal stored
coverage digits, indexed by
`(incoming_coverage - 1) * 8 + initial_stored_coverage`. It also reports the
admitted count, all stored-coverage changes, and changes observed even when the
fragment was rejected. `cycle_results_match` compares both exact channels; a
false value is retained evidence, not a validation failure. The
`analysis_sha256` is SHA-256 over a domain tag plus deterministic compact JSON
containing every result field except the hash itself. It binds the source
bundle digest, sweep controls, geometry, exact observations, and comparison
result.

This analyzer establishes only envelope completeness and deterministic
reduction. A synthetic fixture exercises those checks but says nothing about
hardware. No numeric `ZMODE_INTER` rule may replace the renderer's named trap
until the command generator is reviewed and repeated physical hardware
captures provide the actual result.

## Representative covered-sample selector sweep

Programming Manual section 15.4 requires depth to be evaluated at a point on
the primitive, but it does not publish which of the eight checkerboard samples
silicon selects for a partial fragment. The typed
`representative_sample_selector_sweep` intent exposes that identity without
assuming the bounded selector currently used by fn64. Analyze it with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-representative-sample selector-v1 /path/to/capture.json
```

The complete denominator is 1,530 reset-isolated points: every nonzero
eight-bit coverage mask (`0x01..=0xff`), both one-cycle and two-cycle modes,
and three independent `shade`, `texture`, and `depth` observables. A fixed
control record declares the probed pixel and three eight-element marker sets.
Shade and texture use distinct RGBA32 marker domains; depth uses distinct
16-bit depth markers. Fixed inactive-channel values prove that a color probe
did not change depth and a depth probe did not change color. Marker values are
unique within a domain, the two color domains are disjoint, and neither
inactive-channel control may alias an active marker. This makes changing an
observable label without changing its encoded output fail loudly.

Every point must use the identical controls, replay from reset, and expose
exact one-pixel RGBA32, depth, and `coverage_count_u4` planes at fixed
addresses. The normalized coverage count must equal the population count of
the declared mask. Missing or duplicate mask/cycle/observable points, zero
masks, control or geometry drift, incorrect counts, ambiguous markers,
cross-label outputs, and changed inactive channels are rejected.

The deterministic JSON result contains six exact 255-digit selector tables in
cycle-then-observable order. Mask `0x01` is the first hexadecimal digit and
mask `0xff` is the last; each digit is selected sample index 0 through 7. Per-
sample counts and `uncovered_selection_count` accompany each table. An
uncovered selected index would contradict the public on-primitive premise but
is retained as an observation rather than rewritten or discarded.
Per-observable cross-cycle comparisons and all three pairwise
cross-observable comparisons preserve where tables diverge. The
domain-separated `analysis_sha256` binds the source bundle digest, fixed
controls, geometry, every table, and all comparisons.

The intent remains a producer assertion about opaque raw commands. A hardware
claim requires review of the synthetic command generator, a documented way to
construct each exact coverage mask while keeping attribute gradients
independent, and repeated physical captures. Synthetic tests prove only the
schema, completeness checks, deterministic reduction, and honest divergence
reporting; they do not establish silicon sample selection or correction
arithmetic.

## Narrow-edge fixed-point coverage correction sweep

Public documentation identifies the eight checkerboard coverage samples and
the requirement that depth be evaluated on the primitive, but it does not
publish the raw edge-accumulator truncation or any centroid/correction
arithmetic used for narrow fragments. The
`narrow_edge_coverage_correction_sweep` intent defines an observation envelope
for that frontier without inserting a host formula. Analyze it with:

```sh
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- \
  analyze-narrow-edge-coverage narrow-edge-v1 /path/to/capture.json
```

Every point declares an integer raw edge accumulator, its fixed-point
fractional-bit count, one selected integer boundary, and whether the value is
exactly one integer LSB below, on, or above that boundary. The analyzer checks
that relation with checked integer arithmetic. A strictly increasing,
nonempty boundary manifest is repeated in the fixed controls, so removing all
cases for one selected boundary remains detectable. These values are producer
assertions: the tool does not decode the opaque command stream, derive an edge
accumulator from it, or assert that the declared boundary has any particular
silicon meaning.

The complete matrix contains 18 independently reset cases per selected
boundary: both cycle modes, all three LSB positions, and separate shade,
texture, and depth observations. Each position declares the exact nonzero
eight-sample coverage mask and its population count. All three observables
must repeat that accumulator, mask, and count exactly, while the normalized
`coverage_count_u4` plane must equal the declaration. The raw mask itself is
retained as producer metadata; a normalized count is not represented as proof
that the producer generated that exact mask.

The same disjoint eight-sample marker domains used by the representative-
sample sweep expose shade, texture, and depth independently. Inactive color or
depth controls must remain unchanged. Exact one-pixel RGBA32, depth, and
coverage planes use fixed addresses throughout the sweep. Missing or duplicate
matrix points, reset omissions, non-adjacent accumulator labels, mask/count
disagreement, cross-observable declaration drift, marker ambiguity,
cross-label output, and geometry/control drift fail loudly.

The deterministic result orders selected boundaries as declared, then cycle,
then below/on/above; each point orders shade, texture, and depth observations.
It preserves raw accumulator, mask, count, framebuffer, depth, coverage count,
and decoded marker index. Observable-index and cycle comparisons report
divergence without rejecting or rewriting it. The domain-separated
`analysis_sha256` binds the source bundle digest, controls, geometry, exact
observations, and comparisons.

The included synthetic fixture proves only schema validation, completeness,
uniqueness, deterministic reduction, and divergence retention. It executes no
RDP work and cannot establish a silicon correction rule. Renderer behavior may
change only after review of a command producer, repeated physical-console
captures, and consensus over the resulting hardware bundles.

## Validation

The tool is a standalone nested Cargo workspace so parallel validation does
not rewrite fn64's root lockfile:

```sh
cargo fmt --manifest-path tools/rdp-silicon-vectors/Cargo.toml --check
cargo clippy --manifest-path tools/rdp-silicon-vectors/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path tools/rdp-silicon-vectors/Cargo.toml

# Validate one producer bundle; exits nonzero before printing a digest when
# any schema, provenance, geometry, range, or content check fails.
cargo run --quiet --manifest-path tools/rdp-silicon-vectors/Cargo.toml -- validate \
  /path/to/capture.json
```

Its local `Cargo.lock` intentionally pins the evidence validator's dependency
graph independently of the runtime workspace.

## Local framebuffer transition evidence

The in-process renderer exhaustively tests all nine transitions among the
public I8, RGBA16, and RGBA32 color-image layouts, all three fill layouts, and
the two admitted direct Copy-cycle layout pairs (undereferenced CI8 to I8 and
RGBA16 to RGBA16). Those tests assert logical RDRAM bytes and physical-address
hidden-bit sidecar ownership. Unsupported raw layouts and Copy-cycle pairs
trap before writeback.

Programming Manual 15.5.6 also defines a non-RDP 16-bit write as replicating
the visible LSB into both hidden bits. Generated-C `MEM_H` assignments and
typed-Rust `store_h` now emit the same post-commit event with canonical
physical offset and exact value. The reference backend applies it only when it
owns the addressed hidden-bit sidecar, so an identical visible-value store is
not lost to byte-difference heuristics. Backends without a Rust-owned sidecar
return an explicit disposition; native RT64 currently reports that boundary,
which is not evidence of RT64 hidden-bit parity. These host tests close event
delivery and local mutation semantics, not silicon validation or the hardware
capture method described below.

## External producer frontier

Closure still requires an out-of-tree producer that starts from reset, copies
the schema's synthetic command and initial-memory bytes to the declared
physical addresses, records the declared setup registers, submits DPC END,
waits for a real Full Sync/DP completion, and captures the exact color-image,
depth-image, and hidden-coverage observations. The producer must then populate
its real binary/settings digests and hardware identity in the envelope and run
this validator before the vector can enter the regression corpus.

A flashcart/console capture needs a documented method for reading RGBA16 hidden
coverage bits; ordinary CPU-visible RDRAM reads do not establish those bits.
Until that method and repeated captures exist, the schema enables reproducible
ingestion only. Its strict ZMODE_INTER intent and analyzer now define the
admission/stored-coverage observable, but no physical bundle has populated
that matrix. The schema does not establish reciprocal precision, filter
rounding, LOD boundaries, the covered-subpixel selector, coverage/alpha ties,
combiner or blender intermediates, the random generator/seed, or any other
unpublished silicon behavior. The strict blender matrix now preserves exact
final RGBA/depth/coverage consequences around declared alpha/denominator
boundaries and an ordered adjacent-pixel sequence, but no reviewed repeated
hardware bundle has populated it and final planes do not directly expose an
intermediate blender register. The
raw and high-level renderers now retain the exact eight-bit coverage-sample
identity until the fragment boundary and have deterministic edge/scissor
sweeps for that mask. The bounded selector additionally exhausts every
nonzero mask and equal-distance tie subset, while raw edge tests probe one Q16
LSB below/on/above every checkerboard X boundary. Partial fragments evaluate raw and high-level shade,
texture, and Z at one typed covered checkerboard point, satisfying the public
Programming Manual 15.4 on-primitive requirement. Nearest-to-center with
stable sample-order ties is explicitly a bounded host selector, not evidence
of silicon centroid behavior. These in-process sweeps do not upgrade the
capture schema: its normalized
coverage plane still records only a count/storage representation, so it cannot
close the representative-sample lookup or correction arithmetic without a new
hardware-observable selector-sensitive shade, texture, or depth output.

### Alpha/coverage boundary inventory

The public Programming Manual sections 15.5.4 and 15.7, SGI *RDP Command
Summary* Table 20, and the public coprocessor patent establish that
`CVG_X_ALPHA` uses coverage multiplied by combined alpha for pixel alpha and
coverage, while `ALPHA_CVG_SEL` selects coverage or that product as pixel
alpha. They do not specify the multiplier width, `/255` versus binary `/256`
normalization, product quantization/ties, or how the eight coverage levels are
encoded onto the documented five-bit blender-alpha path.

A controlled hardware suite should run the complete product analyzer above;
the following points are the minimum review landmarks. Each row is a pair:
the value immediately below a decision and the first value at or above it.
Capture reconstructed `coverage_count_u4`; do not infer the product from RGB
blender output. Clear the target to full coverage and select `CVG_DST_WRAP`:
a nonzero product wraps full memory coverage back to the product count, while
a zero product leaves the cleared pixel untouched.

| Input coverage | Combined alpha pair | Discriminates |
|---:|---:|---|
| 8 | 15 / 16 | nearest versus truncation at the first nonzero result |
| 3 | 212 / 213 | nearest `/255` versus nearest `/256` |
| 8 | 254 / 255 | corrected `/255` endpoint versus truncated binary `/256` |

For coverage-to-alpha conversion, run the complete typed sweep above. The
current reference policy would yield `32, 64, 96, 128, 159, 191, 223, 255`;
those numbers are predictions to test, not accepted hardware observations.

Every probe must be a normal schema case with exact raw command bytes, initial
memory, framebuffer/depth/coverage outputs, and repeated hardware provenance.
Repeat each case in both `G_CYC_1CYCLE` and `G_CYC_2CYCLE`; the documented
selector is upstream of either blender cycle, so a difference would expose an
unmodeled timing dependency rather than license a cycle-specific guess.
The validator must reject a hand-entered result that omits the observable
coverage plane. Ten byte-identical hardware captures are required before any
one hypothesis replaces the current explicitly approximate policy.

Provenance: this is an fn64-owned evidence format designed under `AGENTS.md`'s
clean-room and no-game-content rules. The command/register vocabulary follows
the public *Nintendo 64 RDP Command Summary* and the Programming Manual's
Chapter 15 coverage/depth descriptions. No external runtime implementation was
consulted.
