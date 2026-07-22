# VI analog capture program

Status: strict schema, artifact verifier, complete power-cycle cohort validator,
fail-closed operator campaign handoff, and reviewed pixel-comparison analyzer
implemented. No physical-console capture is present in this repository, no
analog-VI behavior row is closed, and no hardware result is claimed.

The same tool now generates a deterministic, public, game-independent NTSC
digital-vector corpus. It does not generate PAL or MPAL vectors: no complete
PAL/MPAL register preset is established by the allowed local evidence, and
relabelling NTSC timing would manufacture evidence rather than extend it.

`tools/vi-analog-captures` admits synthetic VI stimuli and lossless physical
video captures without confusing them with each other. It is a clean-room
evidence boundary: the public VI manuals and patents define the programmed
mechanisms, while a retail/development console and an external capture chain
must supply behavior not published there. No GPL runtime implementation is an
input to this program.

## Two separately hashed artifacts

Every run is one strict `fn64.vi-analog-capture.v2` JSON manifest beside two
regular, non-symlink files:

1. `digital_input.vector_artifact` is a public
   `fn64.vi-digital-input.v2` JSON vector. It embeds synthetic framebuffer
   bytes, complete per-pixel resident coverage counts, their SHA-256 values,
   geometry and encoding, the complete programmed VI
   register image, typed filter controls, television region, and field
   identity. The manifest repeats that metadata and the verifier requires an
   exact match. Its artifact SHA-256 is the input-vector identity.
2. `analog_output.capture_artifact` is the lossless composite or S-Video
   observation. Its path, byte length, and SHA-256 are checked against the real
   file. The manifest separately records the capture device and unit hash,
   firmware, cable, termination, sampling rate, lossless encoding, capture
   program/binary/settings identities, first field, and field count.

The two references may not name the same path or content digest. Artifact
paths must be contained relative paths: absolute paths, `..`, symlinks,
missing files, length mismatches, and digest mismatches fail before a receipt
is produced. Analog artifacts are hashed as a stream rather than loaded into
memory.

The only admitted input content class is `synthetic_vi_probe`. ROM bytes,
game-derived frames, and recompiled-game output remain outside this public
schema.

## Public synthetic vector corpus

Generate the corpus into a path that does not already exist:

```sh
cargo run --quiet --manifest-path tools/vi-analog-captures/Cargo.toml -- \
  generate-vectors --region ntsc /capture/fn64-public-vi-ntsc-v1
```

`corpus.json` uses `fn64.vi-digital-corpus.v1` and hashes every separately
validated `fn64.vi-digital-input.v2` file under `vectors/`. Generation is pure
and byte-deterministic. The v1 corpus index is 13,020 bytes with SHA-256
`13a28331286a2bfb0b23e623357b96c21e55dd93001113cc787c6bbcbfca8c86`;
the test pins that identity. An existing output path is rejected so stale
files cannot survive regeneration. `--region pal` and `--region mpal` fail
loudly until allowed evidence establishes their complete register presets.

The nineteen vectors use only generated RGBA16 gradients and silhouettes.
Every vector contains one coverage count in `1..=8` per pixel and encodes the
same count's high bit in RGBA16 storage. Together they exercise:

- NTSC progressive, interlaced-even, and interlaced-odd identity;
- partial-coverage silhouettes and full-coverage restoration with the dither
  filter off and on;
- divot off/on and all four gamma/gamma-dither combinations;
- identity, half-subpixel offset, minimum and maximum nonzero U2.10 scale,
  exact-last, and beyond-active-window X/Y positions.

The objective list lives in the hashed corpus index and generation fails if
any declared objective is absent. The sync/burst image is the documented NTSC
LAN timing program from the public libultra `OSViMode` interface: `BURST`
`0x03e52239`, `V_SYNC` 525, `H_SYNC` `0x0c15`, `LEAP` `0x0c150c15`, and
`V_BURST` `0x000e0204`. The public `H_START`/`V_START` start/end fields narrow
the active window instead of replaying a 640-dot window over a tiny fixture:
ordinary vectors program eight dots and eight lines; the two boundary vectors
program nine. `CURRENT` carries the declared field parity.

Every corpus entry binds its computed fetch footprint. The generated
36-by-36 RGBA16 plane has three leading guard columns/rows, logical source
coordinates 3 through 34, and one trailing guard column/row at 35. The raw
U2.10 offset starts at 3.0. Normal-vector positions and their adjacent linear
sample remain inside the logical source. The exact-last vector uses step
`0x0f80`, making output coordinate eight equal source coordinate 34 exactly;
the beyond-active vector changes the step to `0x0f81`, making that coordinate
34 + 8/1024 and binding its adjacent read to guard coordinate 35. Boundary
vectors must have full coverage with restoration and divot disabled. The
validator separately proves every partial-coverage AA neighborhood (±2 X and
±1 progressive/±2 interlaced Y), restoration neighborhood (±1 X/Y), and divot
neighborhood (±1 X) is resident in the declared plane.

The scale/offset interpretation follows the twelve-bit fields and ten
fractional bits described by US 6,166,748 Figures 35M/35N. The narrow active
windows and bound guards prevent unspecified RDRAM from entering a capture;
they do not prescribe unpublished accumulator, border-fetch, or analog
behavior. These vectors are capture inputs, not physical observations, and
cannot close the analog-VI row by themselves.

## Fail-closed operator handoff

A capture manifest cannot honestly exist before capture: its lossless output
length and SHA-256, UTC observation time, console-unit identity, capture-device
identity, firmware, settings, and capture-program identities must describe the
actual physical run. Zero hashes and invented hardware names would create a
syntactically plausible lie. The tool therefore emits a separate
`fn64.vi-analog-campaign-plan.v2` document instead of placeholder manifests:

```sh
cargo run --quiet --manifest-path tools/vi-analog-captures/Cargo.toml -- \
  plan-campaign --campaign-id ntsc-aa-composite-v1 \
  --vector partial-aa-dither-filter-off --signal composite --runs 10 \
  /capture/fn64-public-vi-ntsc-v1 > /capture/ntsc-aa-composite-v1.plan.json
```

Planning revalidates the byte-exact generated `corpus.json` and selected vector
artifact before producing output. A missing, changed, symlinked, noncanonical,
or unknown vector fails loud. A campaign shorter than ten runs is rejected.
The resulting plan deterministically binds the corpus/index digest, vector
path/length/digest and objectives, signal, power-cycle reset sequence, run and
repeat indices, run IDs, and intended manifest paths.

The plan carries `evidence_status: planned_not_captured` and
`capture_manifests_emitted: false`. It contains requirement names—not values—
for all hardware provenance, capture-chain provenance, per-run timestamp/field
range, and capture artifact path/length/SHA-256 fields. It deliberately has no
`provenance`, `analog_output`, capture hash, timestamp, or hardware unit value,
and cannot deserialize as `fn64.vi-analog-capture.v2`. After each physical run,
the operator records those observed values in a real manifest and runs
`validate`; the plan itself is never hardware evidence.

## Programmed identity

Each manifest binds:

- all fourteen VI registers (`STATUS`, `ORIGIN`, `WIDTH`, `INTR`, `CURRENT`,
  `BURST`, `V_SYNC`, `H_SYNC`, `LEAP`, `H_START`, `V_START`, `V_BURST`,
  `X_SCALE`, and `Y_SCALE`), including the sampled line/field provenance in
  `CURRENT`;
- RGBA16/RGBA32 source geometry, exact framebuffer content SHA-256, and one
  complete resident coverage count in `1..=8` for every active pixel. RGBA16
  vectors therefore bind the two physical hidden bits absent from visible
  bytes; RGBA32 vectors bind the coverage stored in memory alpha. The verifier
  rejects disagreement between either visible encoding and the declared count;
- NTSC/PAL/MPAL, progressive/interlaced-even/interlaced-odd, and the filter
  bits for pixel type, gamma, gamma dither, divot, and dither restoration;
- reset kind, reset sequence identity, a privacy-preserving SHA-256 identity
  for the observed reset event, and repeat index;
- composite versus S-Video and the complete controlled capture-chain identity.

The verifier rejects a reserved STATUS pixel type, an out-of-RDRAM origin or
last active framebuffer row, zero/mismatched width, impossible framebuffer
geometry, typed filters that do not match STATUS, field identity that does not
match the serrate bit, and an interlaced even/odd declaration that disagrees
with `CURRENT` parity.
Unknown JSON fields fail through `deny_unknown_fields` instead of being
silently ignored.

## Hardware provenance and consensus

`validate` checks one manifest and both artifacts:

```sh
cargo run --quiet --manifest-path tools/vi-analog-captures/Cargo.toml -- \
  validate /capture/run-00/manifest.json
```

A synthetic fixture receives `hardware_provenance: false` and
`closure_eligible: false`. A hardware declaration must include console class,
privacy-preserving console-unit SHA-256, motherboard and video-encoder
revisions, modification state, operator, and a UTC timestamp. This metadata is
necessary provenance, not mechanical proof that the named console was used or
that the identified reset event physically occurred; admission still requires
human review of the capture procedure and producer.
One valid hardware manifest also remains non-certifying.

Capture schema v2 accepts timestamps only as canonical
`YYYY-MM-DDTHH:MM:SSZ`. Calendar dates, leap years, and clock ranges are
validated; fractional seconds and numeric UTC offsets are deliberately not
alternate spellings in this version.

`consensus` requires at least ten fully validated manifests:

```sh
cargo run --quiet --manifest-path tools/vi-analog-captures/Cargo.toml -- \
  consensus /capture/run-00/manifest.json /capture/run-01/manifest.json \
  /capture/run-02/manifest.json /capture/run-03/manifest.json \
  /capture/run-04/manifest.json /capture/run-05/manifest.json \
  /capture/run-06/manifest.json /capture/run-07/manifest.json \
  /capture/run-08/manifest.json /capture/run-09/manifest.json
```

Every run must declare hardware provenance and replay from a power cycle.
Manifest digests, run IDs, UTC timestamps, and reset-event identities must be
distinct. Repeat indices must exactly cover `0..run_count` with no gaps,
offset cohort, or substituted later index; this binds consensus back to the
complete run set emitted by the campaign plan instead of admitting any ten
distinct integers.
The console, operator, public input vector, framebuffer/register/filter state,
region, field, reset sequence, signal, capture chain, and captured field range
must be identical across runs. Caller order does not affect the consensus
digest.

Physical analog noise can make lossless artifacts differ, so the result does
not force unlike raw observations into a false byte consensus. It retains the
SHA-256 of every run, reports `distinct_output_count`, and supplies
`exact_output_sha256` only when every capture is byte-identical. A later
analysis must compare these retained observations with a declared fn64 output;
the cohort result always emits `base_matrix_row_closed: false` because an
ingest tool neither implements the missing analog pipeline nor performs that
comparison.

## Reviewed pixel-through-video analysis

After a real ten-run cohort and an independently reviewed extraction exist,
the strict `fn64.vi-pixel-comparison.v2` stage compares the extracted hardware
planes to one fn64 reference plane:

```sh
cargo run --quiet --manifest-path tools/vi-analog-captures/Cargo.toml -- \
  compare-pixels /capture/ntsc-aa-composite-v1.comparison.json \
  > /capture/ntsc-aa-composite-v1.comparison-report.json
```

The comparison manifest references all ten capture manifests rather than
trusting a copied cohort label. The analyzer revalidates every raw capture and
digital vector, recomputes the controlled hardware consensus, and requires its
SHA-256 to equal `expected_consensus_sha256`. Run IDs and the declared source
capture digests must match those validated manifests. This preserves the path
from a reported pixel residual back to each lossless physical observation.

The hardware and fn64 pixel planes are tightly packed `rgb8` or
`rgb16_big_endian` integer samples and must use the same encoding. A separately
hashed `sample_domain_spec` owns the reviewed decoding and common color-domain
definition; the analyzer treats it as evidence and does not invent YUV/RGB
matrices, transfer functions, DAC coefficients, chroma filters, or analog
levels. Every hardware plane binds one extractor name, version, binary
SHA-256, and settings SHA-256, and all cohort members must use the same
extractor identity. V2 also rejects any per-run drift in the reviewed
extraction or alignment. The fn64 reference plane separately binds its producer
and settings identities plus the exact public input-vector digest.

Each observation binds the exact source field number and raw-capture sample
window (`first_line`, `line_count`, `first_sample`, and `samples_per_line`),
plus the active-output rectangle in its extracted plane. The selected field
must lie inside the capture manifest's retained field range and, for
interlaced vectors, match the programmed even/odd field identity. Source
windows and active rectangles must be nonempty and nonoverflowing. The fn64
reference declares its own active-output rectangle; the reviewed integer
alignment must be contained in both planes and exactly cover both active
rectangles. A tiny partial crop therefore cannot stand in for the declared
active output. Every cohort member must use identical source-window,
active-rectangle, and alignment metadata.

The manifest records a human reviewer, canonical UTC review time, and
alignment method. Those fields and coordinates are provenance, not mechanical
proof that the extractor selected the physically correct samples or active
window; the raw captures, extractor, sample-domain specification, and review
remain independently inspectable.

The `fn64.vi-pixel-comparison-report.v2` result retains the suite, vector,
signal, region, field, complete VI register image, and typed filter controls.
It also retains the reference active rectangle and each run's exact source
window, active rectangle, and alignment, plus the cohort-wide extractor
name/version/binary/settings identity.
For every run and RGB channel it reports exact signed error extrema
(`hardware - fn64`), maximum absolute error, integer sums of absolute and
squared error, exact pixel/sample counts, and a SHA-256 over every row-major
signed RGB residual encoded as big-endian `i32`. Aggregate metrics use checked
integer arithmetic; no floating-point rounding enters the report. The cohort
delta digest binds the sorted run identities and spatial residual digests.

This stage deliberately has no tolerance parameter and emits
`tolerance_applied: false`, `hardware_parity_claimed: false`, and
`base_matrix_row_closed: false`. It supplies a stable review artifact for the
gamma, gamma-dither, divot, restoration/AA, field, and resampling vectors. A
human may compare paired control reports only after reviewing their extraction
domains and alignments; the tool does not infer silicon coefficients or turn a
small numeric residual into a parity claim.

## Digital active-window and neighborhood boundary envelope

The analog manifest and pixel comparison are not the right interchange for
pre-DAC digital VI observations. A separate
`fn64.vi-digital-boundary-capture.v1` bundle records that narrower boundary
without treating an analog decode as a digital oracle. Analyze one complete
bundle with:

```sh
cargo run --quiet --manifest-path tools/vi-analog-captures/Cargo.toml -- \
  analyze-digital-boundaries /path/to/vi-digital-boundaries.json
```

The bundle owns exactly three immutable VI control profiles: progressive,
interlaced even, and interlaced odd. Each contains all fourteen VI registers,
typed STATUS/filter controls, region, and field identity. The validator checks
source width, pixel format, STATUS bits, serrate state, and CURRENT parity.
Every case references one profile instead of copying its registers, preventing
per-point register drift. Shared source and post-VI geometry are similarly
declared once.

The required 44-point matrix is:

- horizontal and vertical active-window start/end, each exactly one declared
  integer code before, on, and after its producer-declared boundary (12);
- left, right, top, and bottom border fetch, again before/on/after (12);
- horizontal and vertical start/end neighborhoods with exactly one and two
  available members of the requested three-sample neighborhood (8);
- eight independently labelled partial-coverage AA centroid candidates, each
  with unique producer-declared Q2 coordinates and an exact nonzero, non-full
  coverage mask/count pair (8);
- interlaced even and odd fields at producer-declared one-line and two-line
  phase offsets (4).

These relationships define capture coordinates, not VI behavior. The analyzer
does not derive H/V boundaries from register values, reconstruct a border
address, select a centroid, infer a three-sample fallback, or fit a field-phase
formula. The producer remains responsible for showing that its observation
method realizes the declarations.

Every point must replay from a distinct declared power-cycle reset event and
carry a distinct retrace-event digest, a contiguous repeat index, nonzero
retrace index, and observed field/CURRENT values equal to its VI profile.
Case IDs and logical matrix keys are unique. Boundary values remain fixed
within each before/on/after group; interlaced phase origins remain fixed across
even and odd fields.

Each observation embeds the exact source RDRAM framebuffer bytes and complete
resident coverage counts plus the exact digital post-VI RGB8 or RGBA8 bytes.
RGBA16/RGBA32 coverage coherence, RDRAM bounds, blob lengths, lowercase hex,
and SHA-256 values are validated. The deterministic analysis retains those
bytes, profiles, intents, and reset/retrace provenance in fixed matrix order
and binds them with a domain-separated `analysis_sha256`.

Only `synthetic_fixture` and `black_box_observation` producer labels exist;
there is no hardware-provenance variant. Every result emits
`evidence_status: non_parity_capture_envelope`, `parity_claimed: false`, and
`base_matrix_row_closed: false`. The included synthetic fixture exercises the
schema and reducer only. It is neither a physical-console capture nor evidence
for a silicon formula.

## Required capture matrix

The first external capture campaign must use separately identified vectors,
not toggle metadata around one observation. At minimum it needs:

- NTSC progressive and interlaced-even/interlaced-odd fields, then equivalent
  PAL and MPAL timing vectors where compatible hardware is available;
- RGBA16 partial-coverage edges with AA/dither-filter selection, full-coverage
  restoration controls, divot on/off, and gamma/gamma-dither on/off;
- identity, fractional-offset, min/max nonzero scale, exact-last, and
  beyond-active-window X/Y resampling probes;
- reset replay and ten distinct repeats for every controlled vector/signal;
- composite, plus S-Video on hardware that exposes it without an internal
  modification to the measured path.

The capture program deliberately does not prescribe a tolerance or infer
silicon fixed-point arithmetic from decoded video. The strict analyzer exposes
exact residuals without interpreting them; mechanism-specific conclusions are
claims to add only after real artifacts show which measurements are stable and
which carry analog noise.

## Validation

```sh
cargo fmt --manifest-path tools/vi-analog-captures/Cargo.toml --check
cargo clippy --manifest-path tools/vi-analog-captures/Cargo.toml \
  --all-targets -- -D warnings
cargo test --manifest-path tools/vi-analog-captures/Cargo.toml
```

The adversarial suite covers missing/changed artifacts, digest and length
errors, lexical and symlink-parent path escape, final-file and manifest
symlinks, unknown fields, forbidden ROM content,
digital/analog conflation, STATUS/filter/field disagreement, fewer than ten
runs, synthetic provenance, duplicate trials and reset-event identities,
gapped/nonzero-based repeat cohorts, changed controlled inputs, CLI receipts,
order-independent cohort digests, deterministic corpus generation,
complete objective/coverage binding, existing-output refusal, and loud
PAL/MPAL rejection. It also covers deterministic campaign plans, the ten-run
floor, unknown vectors, changed selected artifacts, required provenance field
names, and the structural inability to parse a plan as a capture manifest. The
comparison tests cover recomputed cohort binding, exact residual metrics and
digests, changed extractors, legacy schemas, source-field range, extraction and
alignment drift, partial active-output crops, short cohorts, out-of-bounds
reviewed crops, and CLI reports that cannot assert tolerance, parity, or row closure. These are
schema tests using constructed files; they are not hardware evidence.
