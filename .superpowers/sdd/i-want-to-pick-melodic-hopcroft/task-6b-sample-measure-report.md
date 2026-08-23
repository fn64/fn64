# Task 6b (measure): sample_point per-pixel attribution — Candidate B CLOSED

**Verdict: Candidate B (hoist per-pixel-invariant setup out of the texel read) is a
red herring — CLOSED.** The entire texel read (`read_texel`) is ~1.6 ns/covered-pixel,
of which the hoistable setup is at most ~0.7 ns/px. Against the ~507 ns/px pure-CPU
raster baseline that is **~0.3% (whole read) / ≤~0.14% (hoistable part)** — below the
resolution floor, indistinguishable from the ~1% plane-arithmetic red herring Task 27
already closed. **Do not send a writer after it.**

All numbers below are **pure-CPU raster** (the headless `raster_triangle` microbench),
`FN64_RENDER` not involved. This is NOT a shipped-frame figure; the shipped-frame
attribution needs the GUI census (out of scope, GUI-blocked). Method per
`fn64-perf-method`: per-run bracketing (never per-pixel), shares read not absolutes,
closed-lines ledger re-checked (plane-stepping closed; Candidate B was not in it).

## Baseline (substrate)

`texture_plane_raster_microbench` (`crates/fn64-render-wgpu/src/targets/raw_triangle/tests.rs:1181`),
release, `--ignored`, machine load ~1.7 on 15 cores. Full pipeline (raster + plane
step + sample + combine + blend + write) over a fixed 66,000-covered-pixel textured
triangle, 400 iters:

| rep | ns/covered-pixel |
|---|---:|
| 1 | 503.3 |
| 2 | 515.4 |
| 3 | 502.8 |

**Baseline ≈ 507 ns/px, run-to-run spread ~12 ns (~2.5%).** Consistent with Task 27's
~490 ns/px. The ~12 ns noise and the ~0.5 ns/px resolution floor bound what is worth
optimizing.

## sample_point / read_texel attribution

`sample_point` (`tmem/sample.rs:406`) = `address_point_texel` + `read_texel`
(`tmem/read.rs:419`). Per pixel, `read_texel` does:

- **Hoistable setup** (constant across a triangle's covered run — same tile + lut_mode):
  `preflight(tile, lut_mode)` (derives `ReadKind` by decoding a zero texel),
  `validate_address_scope`, `AddressScope::of`, `state.snapshot()`.
- **Irreducible per-pixel**: `read_raw_texel` (TMEM byte fetch + shift/mask/mirror/clamp
  addressing via `AddressScope`) + `decode_direct_texel` (format decode) + (TLUT path
  when indexed: `read_tlut_entry` + `decode_tlut_entry`).

Attribution measured with a **temporary** in-module bench (added to `tmem/read.rs`'s
test module so it could call the private `preflight`/`read_raw_texel`/`AddressScope`;
**reverted, tree clean**). Three tight loops at the 66,000×400 scale, each bracketed
per-loop and divided — RGBA16-direct tile matching the microbench, an **array-backed**
byte source (matching production `PhysicalTmemState`'s O(1) index), a **precomputed**
cheap snapshot value (matching production's field-read `snapshot()`):

- **FULL**: `read_texel` per pixel (setup recomputed every pixel, as production does).
- **HOISTED**: setup once per triangle, then `read_raw_texel` + `decode_direct_texel`
  per pixel.
- **SETUP-ONLY**: only preflight + validate + scope + snapshot per pixel.

| quantity | ns/px |
|---|---:|
| FULL `read_texel` | 0.76 – 1.69 |
| HOISTED (read+decode only) | 0.75 – 1.00 |
| SETUP-ONLY | 0.50 – 0.66 |
| **hoistable = FULL − HOISTED** | **0.004 – 0.69** |

The whole `read_texel` is **~1.6 ns/px at worst**; the hoistable setup is
**≤ ~0.7 ns/px** and swings into the noise (0.004 ns/px on two of three reps) because
it sits **below the ~0.5 ns/px resolution floor**. `preflight`/`AddressScope`/snapshot
are trivial integer arithmetic and a struct field read — there is no heavy invariant
work to hoist.

### Two measurement artifacts caught and corrected (recorded per method)

Both were the "a label 95% something else is a wrong measurement" trap; the first
draft reported **97.9% hoistable** and was false:

1. **`snapshot()` calling `PhysicalTmemState::try_new()` per pixel.** The test byte
   source's `snapshot()` allocated + zeroed a fresh 4 KiB TMEM state every call —
   ~457 ns/px of pure allocation, dwarfing everything and appearing as "setup". Production
   `PhysicalTmemState::snapshot()` is a field read. Fixed to a precomputed value → the
   457 ns vanished, read_texel fell to ~1.6 ns/px.
2. **`black_box` around `preflight`'s inputs** forced a `TileDescriptor` memory
   round-trip per pixel. Moved the opacity to a once-outside-the-loop `black_box` local.

The corrected numbers are physically consistent: read_texel is integer shifts + one
array read + a match; ~1.6 ns/px is right, 457 ns/px was an allocation artifact.

## Candidate B: CLOSE

Hoistable setup is ≤ ~0.14% of the ~507 ns/px pipeline (≤ ~0.7 ns/px, below the
~0.5 ns/px floor and far below the ~12 ns run noise). This is the same magnitude as the
texture-plane arithmetic Task 27 closed as a null. **Hoisting it is not measurable and
not worth a writer.** Added to the closed-lines ledger below.

## Where the ~505 ns/px actually is (ranked alternatives)

`read_texel` is ~1.6 ns/px, so **the texel read + TMEM decode + TLUT is NOT the wall
either** — Task 27's residual "sample_point/TMEM-read" candidate is also effectively
refuted for the read itself. With ~505 of 507 ns/px outside the texel read, the cost is
in **raster/plane stepping (closed), the combiner, and the blender+write**:

1. **`blend_and_write_pixel` (`targets/texrect.rs:2647`) — top remaining candidate.**
   Substantial unconditional per-pixel work: `apply_coverage_alpha`, memory-coverage
   read, `coverage_for`, `alpha_compare_texrect_fragment`, `apply_alpha_dither`,
   `read_pixel` + `blend_texrect_fragment` + write. Multiple `Result`-returning stage
   calls per pixel even when admitted as identity (alpha dither, RGB dither routed as
   no-ops). Joint-first in the old profile; the largest concrete per-pixel body left.
2. **Combiner (`combine_one_texel` / `combiner.rs`) — second.** Per-pixel combine
   evaluation feeding the blend; the other half of the old joint-first pair.
3. **Texel read (`read_texel`) — MEASURED ~1.6 ns/px, ~0.3%. Not a candidate.**
4. **Hoistable setup — MEASURED ≤0.7 ns/px, ≤0.14%. CLOSED (this task).**
5. **Texture-plane arithmetic — CLOSED (Task 27), ~1%.**

Note (method rule 32): "combine vs blend" is likely a *both-halves-must-fall* case, not
a single bottleneck. Neither has been bracketed yet — that is the next measurement, and
it must precede any writer (same discipline that closed B).

## Highest-value confirmed candidate + kill-evidence sketch

**`blend_and_write_pixel` per-pixel stage cost** (only *candidate*, not yet
size-confirmed — measure before dispatching a writer):

- **What to probe/hoist**: the per-pixel stage dispatch when stages are admitted as
  identity (alpha dither `Disabled`, RGB dither `Disabled`, no image-read) — these still
  cost a call + branch per pixel. Candidate change: pre-resolve the admitted stage set
  once per primitive into a specialized fast path that skips the identity stages, rather
  than routing every pixel through `apply_alpha_dither`/coverage machinery.
- **Expected mechanism**: removes N `Result`-returning calls per pixel on the dominant
  admitted mode; the win scales with covered-pixel count, unlike setup.
- **Microbench A/B**: extend `texture_plane_raster_microbench` with a per-run bracket
  around the blend stage (or an isolated `blend_and_write_pixel` loop at the 66k×400
  scale, array dest, precomputed stages) — FULL vs a fast-path variant. Read the ns/px
  **share** of the 507 ns/px baseline; only proceed if it clears the ~0.5 ns/px floor by
  a comfortable margin.
- **Byte-identity plan**: the microbench's own device-bytes output must be unchanged
  (`first.device_bytes()` byte-for-byte); the RDP parity corpus must stay green; add an
  identity test asserting the fast path == the general path across the admitted stage
  matrix (the same "make them incapable of disagreeing" discipline `read_texel` uses).

**But the honest headline for the rasterizer as a whole:** at ~507 ns/px pure-CPU with
the texel read at ~1.6 ns/px and plane arith + setup both closed as <1%, the remaining
cost is spread across combine + blend + write with no single dominant micro-hotspot yet
proven. **If a bracket of combine and of blend each also lands as diffuse sub-floor
per-stage costs, the rasterizer is near its per-pixel floor and the 1.47x gap needs an
architectural move (the GPU raster path), not micro-opt.** That bracket is the required
next measurement.

## Closed-lines ledger — new entry

- **hoisting sample_point per-pixel-invariant setup (Candidate B; ≤0.14% of 507 ns/px
  pure-CPU raster)** — measured 2026-08-23. `read_texel` total ~1.6 ns/px; hoistable
  `preflight`/`validate_address_scope`/`AddressScope::of`/`snapshot()` ≤0.7 ns/px,
  below the ~0.5 ns/px floor and swinging into noise (0.004–0.69 across 3 reps). Same
  magnitude as the closed plane-arith null. Two artifacts corrected en route (per-pixel
  `try_new()` alloc = 457 ns; `black_box` input round-trip) — the raw "97.9% hoistable"
  first read was an allocation artifact, not a finding. Setup is trivial integer
  arithmetic; there is no heavy invariant to hoist. Do not re-propose.
- **Corollary closed by the same run: the texel READ itself (read_raw_texel + format
  decode) is ~1.6 ns/px = ~0.3%** — the "sample_point/TMEM-read" residual is refuted for
  the read; remaining raster cost is combine + blend + write, not the sampler.
