//! **Incremental-vs-exact attribute-walk differential.**
//!
//! `raster_triangle_scalar` evaluates every attribute plane two ways. Inside
//! a run of adjacent covered pixels it advances the previous pixel's value by
//! the masked X slope:
//!
//! ```text
//! AttributeSpanRow::step(plane, value)
//!     = (value as i32).wrapping_add(plane.dx & !0x1f)
//! ```
//!
//! On a run break -- the first covered pixel of a row, or the pixel after a
//! coverage hole -- it re-evaluates the exact span formula from the row latch:
//!
//! ```text
//! AttributeSpanRow::interpolate(plane, x)
//!     = latched + (plane.dx & !0x1f) * (x - base_x)
//! ```
//!
//! These are the same walk expressed twice, and nothing asserted they agree.
//! The property here does: for a generated triangle, rasterizing with every
//! pixel forced onto the exact path must produce byte-identical target
//! contents to rasterizing with production's stepping.
//!
//! **Why this loop specifically.** It is the CPU oracle the live GPU compute
//! rasterizer was certified byte-identical against
//! (`docs/plans/WM2000-COMPUTE-RASTER.md`). A stepping-vs-exact divergence
//! here is a divergence in the reference the GPU path is measured by, and it
//! would be invisible to that certification, which compares GPU against CPU
//! rather than CPU against itself.
//!
//! **What the property does and does not cover.** It covers both attribute
//! families the loop walks -- the four shade planes and the three texture
//! S/T/W planes -- because it drives shaded+textured (`0x0e`) triangles and
//! the exact arm forces the break branch for both. It does NOT cover a
//! per-pixel Z plane: the admitted subset carries only flat prim/pixel Z,
//! resolved once outside the loop, so there is no second Z path to compare
//! against.
//!
//! **What is in this file**, because several of these tests exist to stop
//! the headline property from passing for the wrong reason:
//!
//! - `incremental_stepping_matches_exact_evaluation` -- the headline
//!   differential, over 256 generated triangles.
//! - `the_harness_actually_writes_pixels` -- the draws must change the
//!   target. This caught a real vacuity: under WM2000's own measured
//!   `OtherMode` all 256 cases rasterized while writing ZERO bytes.
//! - `the_generator_reaches_the_raster_loop` -- a representative share of
//!   generated cases must write, so a drifting strategy fails loudly.
//! - `the_exact_arm_is_observable_where_the_loop_reads_it` -- the override
//!   must be visible through the accessor the loop actually reads.
//! - `the_two_arms_agree_across_the_parallel_threshold` -- **the only
//!   coverage of the `&& !exact_stepping` rayon guard in `raster_triangle`.**
//!   The generated property runs on a 32x24 target, far below
//!   `MIN_PARALLEL_PIXELS`, so without this test the row-parallel dispatch
//!   path and the new guard that suppresses it would be entirely untested.
//! - `stepping_equals_exact_evaluation_over_a_run` and
//!   `..._over_generated_planes` -- the direct Q16.16 walk equivalence, three
//!   orders of magnitude more sensitive than the byte differential (below).
//!
//! **Measured sensitivity floor: between `1 << 5` and `1 << 6` Q16.16 units
//! of drift per pixel.** A systematic per-pixel divergence smaller than that
//! is invisible to this test. That is a measurement, not an estimate:
//!
//! | mutation to `step` | byte differential | direct walk tests |
//! |---|---|---|
//! | adds `1 << 20` per pixel | KILLED | KILLED |
//! | masks `!0x1fff` instead of `!0x1f` | KILLED | KILLED |
//! | `interpolate`'s `x_step` gains an `x`-proportional term | KILLED | n/a |
//! | adds `1 << 6` (64) per pixel | KILLED | KILLED |
//! | adds `1 << 5` (32) per pixel | **SURVIVED** | KILLED |
//! | adds `1` per pixel, unconditionally | **SURVIVED** | KILLED |
//! | masks `!0x1e` instead of `!0x1f` | **SURVIVED** | KILLED |
//!
//! **Do not read the floor as "small drifts are harmless".** An
//! unconditional `+1` per pixel is a systematic divergence between the two
//! walks on every plane, every pixel, every case -- exactly the defect class
//! this file exists to catch -- and the byte differential does not see it.
//! Quantization (`>> 14` then `>> 4` for shade, the S10.5 conversion for
//! texture) lowers the RATE at which a small delta reaches a written byte;
//! it does not bound that rate to zero, and an earlier version of this
//! comment claimed it did. That claim was wrong and was falsified by the
//! `+1` row above.
//!
//! `stepping_equals_exact_evaluation_over_a_run` and
//! `stepping_equals_exact_evaluation_over_generated_planes` close the gap.
//! They compare the two walks in their own Q16.16 units with no rasterizer
//! in between, so their floor is one unit -- the smallest divergence that
//! can exist -- and they kill every mutant this differential misses. A
//! change to this loop should be measured against BOTH.
//!
//! **Blast radius: accumulation, not formula.** `interpolate` is
//! `latched + (dx & !0x1f) * (x - base_x)`; `step` is `value + (dx & !0x1f)`.
//! They are a closed form and its recurrence over the *same* `latched` and
//! the *same* masked `dx`. So this is a real oracle for accumulation defects
//! -- wrapping, `as i32` truncation, run-boundary handling -- and nothing
//! else. A defect in the shared subexpressions (the `!0x1f` mask itself, the
//! `do_offset` correction, the `x_fraction` correction, the row latch)
//! appears identically on both sides and cancels exactly. Neither this
//! property nor the direct tests below can see one; that needs an oracle
//! outside this pair, such as the RT64 parity corpus.

use proptest::prelude::*;

use super::super::super::super::targets::raw_triangle::Stepping;
use super::*;
use crate::raw_dpc::triangle_span;

/// The differential's raster domain.
///
/// Deliberately small (`32x24`) so 256 generated cases stay fast, but larger
/// than one run in both axes so run breaks, row latches and multi-row walks
/// all occur.
const WIDTH: u32 = 32;
const HEIGHT: u32 = 24;

/// One generated triangle's plane coefficients, in the RDP's wire units.
///
/// Held as a struct rather than a tuple so the shrunken counterexample
/// proptest prints names its fields.
#[derive(Clone, Copy, Debug)]
struct SteppingCase {
    /// Left and right edge X, in pixels. Generated independently and
    /// normalized so the triangle is non-empty.
    left_x: f64,
    right_x: f64,
    /// Scanlines covered.
    rows: i16,
    /// Shade RGBA at the origin.
    shade_base: [i32; 4],
    /// Shade d/dx, d/de, d/dy in Q16.16.
    shade_dx: [i32; 4],
    shade_de: [i32; 4],
    shade_dy: [i32; 4],
    /// Texture S/T/W at the origin, and their derivatives.
    texture_base: [i32; 4],
    texture_dx: [i32; 4],
    texture_de: [i32; 4],
    texture_dy: [i32; 4],
    /// Left-major (`true`) or right-major. Flips `do_offset` in the span
    /// latch, which is one of the two things `interpolate` corrects for and
    /// `step` inherits.
    left_major: bool,
}

/// A Q16.16 plane derivative.
///
/// **The range is the point of the generator.** `step` masks `dx` with
/// `!0x1f` and truncates the running sum to `i32`; `interpolate` multiplies
/// the same masked `dx` by the pixel offset and also wraps in `i32`. The two
/// agree only if the wrapping is consistent, so the generator must reach
/// magnitudes where a 32-pixel row's accumulation actually overflows.
/// `i32::MIN..=i32::MAX` reaches them: a `dx` above `i32::MAX / 32` wraps
/// within a single row of this target.
///
/// Small values are NOT excluded either -- a `dx` below `0x20` masks to
/// exactly zero, making `step` a no-op, which is the case where a
/// sign-extension bug in `interpolate` would show up alone.
fn derivative() -> BoxedStrategy<i32> {
    prop_oneof![
        // Weighted toward the realistic band first: WM2000's measured planes
        // sit in the low millions of Q16.16 units, and a generator that only
        // sampled the full i32 range would spend nearly every case in
        // overflow and never exercise the ordinary walk.
        4 => -(1i32 << 22)..(1i32 << 22),
        // Sub-mask values: `dx & !0x1f == 0`, so stepping is the identity.
        2 => -0x20i32..0x20i32,
        // **Mask-boundary values.** Both walks discard `dx`'s low five bits,
        // and a mutant that discards a different number of them (`!0x1e`,
        // say) is invisible unless those exact bits are set AND the
        // difference survives the shade path's `>> 14` quantization. So the
        // low bits are forced on beneath a magnitude large enough to reach
        // the output: without this band the `!0x1e` mutant survives.
        3 => (12i32..24).prop_flat_map(|shift| {
            (0x1fi32..0x20, 0i32..8).prop_map(move |(low, high)| (high << shift) | low)
        }),
        // The full domain, including magnitudes that wrap an i32 inside one
        // 32-pixel row.
        1 => any::<i32>(),
    ]
    .boxed()
}

/// A Q16.16 plane origin. Same reasoning as [`derivative`]: the full `i32`
/// domain is reachable because `interpolate` adds the origin into the same
/// wrapping accumulator.
fn origin() -> BoxedStrategy<i32> {
    prop_oneof![
        4 => -(1i32 << 24)..(1i32 << 24),
        1 => any::<i32>(),
    ]
    .boxed()
}

/// Four independently generated components. `prop_oneof!` strategies are not
/// `Clone`, so the factory is invoked once per component rather than a single
/// strategy being duplicated -- which is what we want anyway: the four
/// channels should be able to land in different weighted bands.
fn plane_quad(component: fn() -> BoxedStrategy<i32>) -> impl Strategy<Value = [i32; 4]> {
    [component(), component(), component(), component()]
}

fn arbitrary_stepping_case() -> impl Strategy<Value = SteppingCase> {
    (
        // Edges span the target and overhang it on both sides, so clipped
        // rows -- where `row_pixel_range` shortens the walk and the row latch
        // no longer starts at the triangle's own left edge -- are generated.
        (-8.0f64..40.0f64, -8.0f64..40.0f64),
        1i16..(HEIGHT as i16 + 4),
        (
            plane_quad(origin),
            plane_quad(derivative),
            plane_quad(derivative),
            plane_quad(derivative),
        ),
        (
            plane_quad(origin),
            plane_quad(derivative),
            plane_quad(derivative),
            plane_quad(derivative),
        ),
        any::<bool>(),
    )
        .prop_map(
            |(
                (left_x, right_x),
                rows,
                (shade_base, shade_dx, shade_de, shade_dy),
                (texture_base, texture_dx, texture_de, texture_dy),
                left_major,
            )| {
                SteppingCase {
                    left_x,
                    right_x,
                    rows,
                    shade_base,
                    shade_dx,
                    shade_de,
                    shade_dy,
                    texture_base,
                    texture_dx,
                    texture_de,
                    texture_dy,
                    left_major,
                }
            },
        )
}

impl SteppingCase {
    /// Builds the `0x0e` shaded+textured triangle this case describes, or
    /// `None` if the generated edges do not decode to a rasterizable
    /// triangle. A refusal is not a property failure: it means the generator
    /// produced a degenerate wire packet, which the decoder is entitled to
    /// reject, and there is then nothing to compare.
    fn triangle(&self) -> Option<RawTriangle> {
        use crate::rdp_harness::Tri;
        // Normalize: `edges` wants left <= right, and a zero-width span
        // covers no pixels and would make every case vacuously equal.
        let (low, high) = if self.left_x <= self.right_x {
            (self.left_x, self.right_x)
        } else {
            (self.right_x, self.left_x)
        };
        let high = if (high - low) < 0.5 { low + 0.5 } else { high };

        let mut tri = Tri::flat().edges(low, high).rows(0..self.rows);
        if self.left_major {
            tri = tri.left_major();
        }
        let tri = tri
            .shade(self.shade_base, self.shade_dx, self.shade_de, self.shade_dy)
            .texture_planes(
                self.texture_base,
                self.texture_dx,
                self.texture_de,
                self.texture_dy,
            );
        let words = tri.words();
        let mut bytes = Vec::with_capacity(words.len() * 4);
        for word in words {
            bytes.extend_from_slice(&word.to_be_bytes());
        }
        RawTriangle::decode(0x0e, &bytes).ok()
    }
}

/// Rasterizes one triangle under the requested attribute-walk path and
/// returns the resulting target bytes.
///
/// Both arms are handed the identical resident image, tile binding, TMEM and
/// combiner program, so the ONLY difference between the two calls is the
/// stepping path.
fn raster_under(
    stepping: Stepping,
    triangle: &RawTriangle,
    resident: &[u8],
    tmem: &BenchTmem,
) -> Option<Vec<u8>> {
    let key = key_at(WIDTH, HEIGHT);
    let declared = declared_accesses(key, triangle, None);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    crate::targets::raw_triangle::with_stepping(stepping, || {
        execute_raw_triangle(
            &candidate,
            // **Blending off, so every covered pixel is written.**
            //
            // Measured, not assumed: WM2000's own measured mode
            // (`0x0008_acef / 0x0050_41c8`) rasterizes this fixture without
            // changing a single byte of the target -- its blend equation
            // reproduces the resident value -- which would make the
            // differential below compare two untouched buffers and pass
            // vacuously. `from_wire(0, 0)` is the mode the crate's own
            // hand-derived shade tests use precisely because its writes are
            // observable.
            OtherMode::from_wire(0, 0),
            triangle,
            TexrectShading::new(
                CombineParams::from_wire(0xfc51_96a3, 0x112c_fe7f),
                Color4::from_wire(ENV_WIRE),
                PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
            ),
            TexrectBlendRegisters::default(),
            resident,
            &declared,
            Some(RawTriangleTexture {
                tile: bench_tile_binding(),
                tmem,
                lut_mode: crate::TextureLutMode::Disabled,
            }),
            None,
        )
        .ok()
        .map(|completed| completed.device_bytes().device_bytes().to_vec())
    })
}

/// A resident image with every pixel distinct, so a rasterizer that wrote the
/// wrong pixel -- rather than the wrong value -- also shows up as a mismatch.
fn resident_image() -> Vec<u8> {
    let mut resident = Vec::with_capacity((WIDTH * HEIGHT * 2) as usize);
    for pixel in 0..WIDTH * HEIGHT {
        let rgba16 = (((pixel * 13) & 0x1f) << 11)
            | (((pixel * 7) & 0x1f) << 6)
            | (((pixel * 3) & 0x1f) << 1)
            | (pixel & 1);
        resident.extend_from_slice(&(rgba16 as u16).to_be_bytes());
    }
    resident
}

proptest! {
    // 256 cases: enough to cover the weighted derivative bands without
    // making the suite's wall time depend on a proptest run. The default
    // RNG is seeded from the environment, and any failure proptest finds is
    // persisted under `proptest-regressions/` and replayed first on the next
    // run, so a discovered counterexample becomes deterministic.
    #![proptest_config(ProptestConfig {
        cases: 256,
        ..ProptestConfig::default()
    })]

    /// **The two attribute-walk paths must agree byte for byte.**
    ///
    /// Rasterizing with production's incremental stepping and rasterizing
    /// with every pixel re-evaluated from the exact span formula must write
    /// the same target. A failure is a real defect in the rasterizer, not a
    /// tolerance to widen: the GPU compute path is certified against the
    /// incremental arm, so whichever arm is wrong, the certified pixels are
    /// wrong with it.
    #[test]
    fn incremental_stepping_matches_exact_evaluation(case in arbitrary_stepping_case()) {
        let Some(triangle) = case.triangle() else {
            // A degenerate wire packet the decoder refuses. Nothing to
            // compare; not a property violation.
            return Ok(());
        };
        let resident = resident_image();
        let tmem = BenchTmem::new();

        let incremental = raster_under(Stepping::Incremental, &triangle, &resident, &tmem);
        let exact = raster_under(Stepping::Exact, &triangle, &resident, &tmem);

        // Both arms must reach the same VERDICT as well as the same bytes: a
        // triangle one arm rasterizes and the other refuses is itself a
        // divergence between the paths.
        prop_assert_eq!(
            incremental.is_some(),
            exact.is_some(),
            "one stepping path rasterized and the other refused"
        );
        if let (Some(incremental), Some(exact)) = (incremental, exact) {
            if incremental != exact {
                let byte = incremental
                    .iter()
                    .zip(&exact)
                    .position(|(a, b)| a != b)
                    .expect("unequal buffers have a mismatching byte");
                prop_assert!(
                    false,
                    "incremental stepping diverged from exact evaluation at byte {byte} \
                     (pixel {}): incremental={:#04x}, exact={:#04x}",
                    byte / 2,
                    incremental[byte],
                    exact[byte]
                );
            }
        }
    }
}

/// **The differential must not pass vacuously.**
///
/// This guard is not decoration: it caught a real vacuity. The harness first
/// drove WM2000's own measured `OtherMode` (`0x0008_acef / 0x0050_41c8`), and
/// under it all 256 generated cases rasterized successfully while changing
/// **zero** bytes of the target -- its blend equation reproduced the resident
/// value. The property above passed by comparing two untouched buffers, and
/// would have kept passing against any stepping bug whatsoever.
///
/// So the differential's precondition is asserted directly: a triangle drawn
/// under the harness's mode must actually change the target. If a future
/// change to the combiner program, the tile binding or the resident image
/// makes the draws invisible again, this fails instead of the suite going
/// quietly green.
#[test]
fn the_harness_actually_writes_pixels() {
    let triangle = bench_textured_triangle(20.0, 16);
    let resident = resident_image();
    let tmem = BenchTmem::new();

    let drawn = raster_under(Stepping::Incremental, &triangle, &resident, &tmem)
        .expect("the reference textured triangle rasterizes under the harness");

    assert_ne!(
        drawn, resident,
        "the differential's draws changed no bytes -- the property above would \
         compare two untouched buffers and pass against any stepping bug"
    );
}

/// **A representative share of generated cases must reach the loop.**
///
/// The guard above proves ONE hand-built triangle writes. This proves the
/// GENERATOR does: a strategy that drifted into producing only degenerate or
/// fully-clipped packets would leave the property technically non-vacuous but
/// statistically empty.
///
/// The threshold is deliberately loose. Roughly 45% of cases wrote pixels
/// when the generator was written (116 of 256); a quarter is well below that
/// and well above the zero a broken generator produces, so this asserts the
/// generator still works without pinning a number that ordinary strategy
/// tuning would break.
#[test]
fn the_generator_reaches_the_raster_loop() {
    use proptest::strategy::{Strategy, ValueTree};
    use proptest::test_runner::TestRunner;

    let resident = resident_image();
    let tmem = BenchTmem::new();
    let strategy = arbitrary_stepping_case();
    let mut runner = TestRunner::deterministic();

    const SAMPLES: usize = 128;
    let mut wrote = 0usize;
    for _ in 0..SAMPLES {
        let case = strategy
            .new_tree(&mut runner)
            .expect("the stepping-case strategy produces a value")
            .current();
        let Some(triangle) = case.triangle() else {
            continue;
        };
        if let Some(drawn) = raster_under(Stepping::Incremental, &triangle, &resident, &tmem) {
            if drawn != resident {
                wrote += 1;
            }
        }
    }

    assert!(
        wrote >= SAMPLES / 4,
        "only {wrote} of {SAMPLES} generated triangles wrote any pixel; the \
         differential is running mostly on empty draws"
    );
}

/// **The exact arm must actually reach the raster loop.**
///
/// The other half of the vacuity question. Even with draws that write, the
/// property proves nothing if `with_stepping(Stepping::Exact, ..)` never
/// reaches the loop -- a thread-local left unread, or a rayon worker that
/// does not inherit it -- because both arms would then run production
/// stepping and compare identical work.
///
/// The property itself cannot detect that, since a correct rasterizer makes
/// both arms equal either way. So this asserts the override is OBSERVABLE at
/// the one place it is read, through the same accessor the loop uses:
/// `force_exact_stepping()` must be false outside the scope, true inside it,
/// and restored on the way out -- including across nesting, which the
/// row-parallel suppression relies on.
#[test]
fn the_exact_arm_is_observable_where_the_loop_reads_it() {
    use crate::targets::raw_triangle::{force_exact_stepping, with_stepping};

    assert!(
        !force_exact_stepping(),
        "production stepping is the default outside any override"
    );

    with_stepping(Stepping::Exact, || {
        assert!(
            force_exact_stepping(),
            "with_stepping(Exact) is not visible to the accessor the raster \
             loop reads -- the differential would compare two identical arms"
        );
        with_stepping(Stepping::Incremental, || {
            assert!(
                !force_exact_stepping(),
                "a nested Incremental scope must apply"
            );
        });
        assert!(
            force_exact_stepping(),
            "leaving a nested scope must restore the enclosing arm"
        );
    });

    assert!(
        !force_exact_stepping(),
        "the override must not leak past its scope into other tests"
    );
}

/// **Both dispatch paths must agree under both stepping arms.**
///
/// `raster_triangle` refuses row parallelism under the exact arm (the
/// thread-local does not cross into rayon workers), so a draw large enough
/// to clear `MIN_PARALLEL_PIXELS` takes the parallel path incrementally and
/// the scalar path exactly. Equal bytes here therefore assert two things at
/// once: the two attribute walks agree, and the parallel row split does not
/// change the image.
///
/// The generated property above runs on a 32x24 target, well under the
/// parallel threshold, so without this the parallel dispatch would be
/// untested by this file entirely.
#[test]
fn the_two_arms_agree_across_the_parallel_threshold() {
    let triangle = bench_textured_triangle(300.0, 220);
    let width = 320u32;
    let height = 240u32;

    let mut resident = Vec::with_capacity((width * height * 2) as usize);
    for pixel in 0..width * height {
        resident.extend_from_slice(&(pixel as u16).wrapping_mul(7).to_be_bytes());
    }
    let tmem = BenchTmem::new();

    let render = |stepping| {
        let key = key_at(width, height);
        let declared = declared_accesses(key, &triangle, None);
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        crate::targets::raw_triangle::with_stepping(stepping, || {
            execute_raw_triangle(
                &candidate,
                OtherMode::from_wire(0, 0),
                &triangle,
                TexrectShading::new(
                    CombineParams::from_wire(0xfc51_96a3, 0x112c_fe7f),
                    Color4::from_wire(ENV_WIRE),
                    PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
                ),
                TexrectBlendRegisters::default(),
                &resident,
                &declared,
                Some(RawTriangleTexture {
                    tile: bench_tile_binding(),
                    tmem: &tmem,
                    lut_mode: crate::TextureLutMode::Disabled,
                }),
                None,
            )
            .expect("the large textured triangle rasterizes")
            .device_bytes()
            .device_bytes()
            .to_vec()
        })
    };

    let incremental = render(Stepping::Incremental);
    let exact = render(Stepping::Exact);

    assert_ne!(
        incremental, resident,
        "the parallel-sized fixture must actually write pixels"
    );
    if incremental != exact {
        let byte = incremental
            .iter()
            .zip(&exact)
            .position(|(a, b)| a != b)
            .expect("unequal buffers have a mismatching byte");
        panic!(
            "incremental stepping diverged from exact evaluation across the \
             parallel threshold at byte {byte} (pixel {}): incremental={:#04x}, \
             exact={:#04x}",
            byte / 2,
            incremental[byte],
            exact[byte]
        );
    }
}

// ---------------------------------------------------------------------------
// The direct walk equivalence, at full Q16.16 precision
// ---------------------------------------------------------------------------

/// **`step` must equal `interpolate` exactly, at every pixel of a run.**
///
/// This is the differential above with the rasterizer taken out of the
/// middle, and it exists because taking the rasterizer out raises the
/// sensitivity by three orders of magnitude.
///
/// The byte-level property is bounded by the RDP's output quantization: it
/// compares written pixels, and the shade path discards the low 18 bits of
/// every attribute (`>> 14` then `>> 4`) while the texture path collapses to
/// S10.5. A per-pixel drift smaller than roughly `1 << 8` Q16.16 units is
/// therefore invisible to it -- **measured**, not assumed: an unconditional
/// `+1` per pixel in `step` passes the byte differential, and so does `+32`.
///
/// This test compares the two walks in their own units, before any
/// quantization, so its floor is one Q16.16 unit -- the smallest divergence
/// that can exist. Every mutant the byte differential misses dies here.
///
/// The property is the recurrence relation itself: walking a run by
/// repeated `step` from an `interpolate`d start must land on exactly what
/// `interpolate` would have returned at each pixel.
fn assert_walks_agree(
    triangle: &RawTriangle,
    plane: triangle_span::AttributePlane,
    y: i32,
    x0: i32,
    run: i32,
) {
    let span = triangle_span::AttributeSpanRow::new(triangle, y);
    let mut stepped = span.interpolate(plane, x0);

    for offset in 1..run {
        let x = x0 + offset;
        stepped = triangle_span::AttributeSpanRow::step(plane, stepped);
        let exact = span.interpolate(plane, x);
        assert_eq!(
            stepped,
            exact,
            "stepping diverged from exact evaluation at x={x} (offset {offset} \
             into a run from x0={x0}, y={y}) for plane {plane:?}: \
             stepped={stepped}, exact={exact}, delta={}",
            stepped - exact
        );
    }
}

/// A triangle whose span latch is well-defined, for the walk comparison.
/// The planes are supplied per-case; only the edge geometry matters here,
/// because `AttributeSpanRow::new` reads only the edges.
fn walk_fixture_triangle() -> RawTriangle {
    bench_textured_triangle(28.0, 20)
}

/// Hand-picked plane coefficients that between them cover every structural
/// case in the two walks.
#[test]
fn stepping_equals_exact_evaluation_over_a_run() {
    let triangle = walk_fixture_triangle();

    let planes = [
        // Zero: the degenerate walk.
        triangle_span::AttributePlane {
            base: 0,
            dx: 0,
            de: 0,
            dy: 0,
        },
        // Sub-mask dx: `dx & !0x1f == 0`, so `step` is the identity and
        // `interpolate`'s x term must also vanish.
        triangle_span::AttributePlane {
            base: 12345,
            dx: 0x1f,
            de: 7,
            dy: 3,
        },
        // Exactly the mask boundary.
        triangle_span::AttributePlane {
            base: -9876,
            dx: 0x20,
            de: 0,
            dy: 0,
        },
        // Low bit set above the mask: the `!0x1e`-vs-`!0x1f` discriminator.
        triangle_span::AttributePlane {
            base: 1 << 20,
            dx: (1 << 20) | 1,
            de: 0,
            dy: 0,
        },
        // WM2000's own measured magnitude band.
        triangle_span::AttributePlane {
            base: 0x0020_0000,
            dx: (3 << 10) + 17,
            de: 1 << 8,
            dy: 1 << 6,
        },
        // Negative slope.
        triangle_span::AttributePlane {
            base: 0,
            dx: -((5 << 10) + 11),
            de: -256,
            dy: -64,
        },
        // Large enough to wrap an i32 within a single row.
        triangle_span::AttributePlane {
            base: i32::MAX - 1024,
            dx: i32::MAX / 4,
            de: 0,
            dy: 0,
        },
        triangle_span::AttributePlane {
            base: i32::MIN + 1024,
            dx: i32::MIN / 4,
            de: 0,
            dy: 0,
        },
        // Extremes.
        triangle_span::AttributePlane {
            base: i32::MAX,
            dx: i32::MAX,
            de: i32::MAX,
            dy: i32::MAX,
        },
        triangle_span::AttributePlane {
            base: i32::MIN,
            dx: i32::MIN,
            de: i32::MIN,
            dy: i32::MIN,
        },
    ];

    for plane in planes {
        for y in [0, 1, 7, 19] {
            for x0 in [0, 2, 17] {
                assert_walks_agree(&triangle, plane, y, x0, 32);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        ..ProptestConfig::default()
    })]

    /// The same equivalence over a generated plane domain.
    ///
    /// 512 cases rather than the byte differential's 256: each case is pure
    /// arithmetic with no rasterization, so the run is far cheaper.
    #[test]
    fn stepping_equals_exact_evaluation_over_generated_planes(
        base in origin(),
        dx in derivative(),
        de in derivative(),
        dy in derivative(),
        y in 0i32..24,
        x0 in -4i32..36,
        run in 1i32..48,
    ) {
        let triangle = walk_fixture_triangle();
        let plane = triangle_span::AttributePlane { base, dx, de, dy };
        let span = triangle_span::AttributeSpanRow::new(&triangle, y);

        let mut stepped = span.interpolate(plane, x0);
        for offset in 1..run {
            let x = x0 + offset;
            stepped = triangle_span::AttributeSpanRow::step(plane, stepped);
            let exact = span.interpolate(plane, x);
            prop_assert_eq!(
                stepped,
                exact,
                "stepping diverged from exact evaluation at x={} (offset {} into a run \
                 from x0={}, y={}) for plane {:?}",
                x, offset, x0, y, plane
            );
        }
    }
}
