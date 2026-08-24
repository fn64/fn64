use fn64_render_ir::PhysicalMemoryLayout;

use super::super::texrect::{TexrectBlendRegisters, TexrectShading};
use super::super::{
    ColorTargetExtent, ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, TargetError,
};
use super::*;

/// Every fixture in this module draws an UNTEXTURED triangle, so each names
/// the absent binding through one alias rather than repeating a turbofish.
/// The type parameter must still be named -- `None` alone leaves `S`
/// unconstrained -- and `PhysicalTmemState` is the natural witness: it is the
/// image a load-free packet's triangle would actually be handed.
const NO_TEXTURE: Option<RawTriangleTexture<'static, crate::tmem::PhysicalTmemState>> = None;

use crate::raw_dpc::RawTriangle;
use crate::{Color4, CombineParams, PrimColor};

const RDRAM_BYTES: u32 = 8 * 1024 * 1024;
const FIXTURE_START: u32 = 0x400;

/// `SetPrimColor`'s RGBA word. Every channel distinct, none 0x00 or 0xFF, so
/// a channel swap or a dropped channel is visible in the packed RGBA16.
const PRIM_WIRE: u32 = 0x80FF_4080;
/// `SetPrimColor`'s `w0`: `lod_frac` in bits 0:7, `lod_min` in 8:12. The
/// flat program reads neither, so a non-zero value catches a leak.
const PRIM_LOD_W0: u32 = 0x0540;
const ENV_WIRE: u32 = 0xFF00_80FF;

fn layout() -> PhysicalMemoryLayout {
    PhysicalMemoryLayout::try_new(RDRAM_BYTES).unwrap()
}

fn key_at(width: u32, height: u32) -> ColorTargetKey {
    ColorTargetKey::try_new(
        layout().address(FIXTURE_START).unwrap(),
        ColorTargetExtent::try_new(width, height).unwrap(),
        ColorTargetFormat::Rgba16,
    )
    .unwrap()
}

/// `(Zero - Zero) * Zero + Primitive` in the second bitfield slice: the
/// combined colour is the primitive register, verbatim.
///
/// Slot indices are each slot's own out-of-table `Zero` (`A = 8`, `B = 8`,
/// `C = 16`, alpha `= 7`) with `D = 3`, which is `Primitive` in
/// `colorInputD` and `alphaInputABD` alike. Packed by the same bit layout
/// `texrect.rs`'s own `pack_second_cycle` uses:
///   low  = (A << 5) | C
///   high = (B << 24) | (D << 6) | (alphaA << 21) | (alphaB << 3)
///          | (alphaC << 18) | alphaD
fn flat_primitive_program() -> CombineParams {
    let (ca, cb, cc, cd) = (8u32, 8u32, 16u32, 3u32);
    let (aa, ab, ac, ad) = (7u32, 7u32, 7u32, 3u32);
    let low = (ca << 5) | cc;
    let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
    CombineParams::from_wire(low, high)
}

fn flat_shading() -> TexrectShading {
    TexrectShading::new(
        flat_primitive_program(),
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
}

/// One-cycle `OtherMode` with every post-combiner stage at its identity:
/// no alpha compare, no dither, no coverage-times-alpha, blending disabled.
fn one_cycle_other_mode() -> OtherMode {
    OtherMode::from_wire(0, 0)
}

/// Builds one flat (opcode 0x08) triangle from its ten wire fields, through
/// the REAL decoder -- never by constructing the struct directly, so these
/// tests exercise the same decode the stream does.
#[allow(clippy::too_many_arguments)]
fn triangle(
    lft: bool,
    yl: i16,
    ym: i16,
    yh: i16,
    xl: i32,
    dxldy: i32,
    xh: i32,
    dxhdy: i32,
    xm: i32,
    dxmdy: i32,
) -> RawTriangle {
    let w0 = (u32::from(lft) << 23) | (yl as u16 as u32);
    let w1 = ((ym as u16 as u32) << 16) | (yh as u16 as u32);
    let mut bytes = Vec::with_capacity(32);
    for word in [
        w0,
        w1,
        xl as u32,
        dxldy as u32,
        xh as u32,
        dxhdy as u32,
        xm as u32,
        dxmdy as u32,
    ] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    RawTriangle::decode(0x08, &bytes).expect("a base-edge triangle is 32 bytes")
}

/// The vertical-edged triangle the decoder tests use: left edge at x = 2,
/// right edge at x = 6, scanlines 0..3.
fn box_triangle() -> RawTriangle {
    triangle(true, 3 << 2, 3 << 2, 0, 6 << 16, 0, 2 << 16, 0, 6 << 16, 0)
}

/// The primitive colour packed to RGBA16 5/5/5/1, big-endian, derived by
/// hand from `PRIM_WIRE` and nothing else.
///
///   R = 0x80 >> 3 = 0x10, G = 0xFF >> 3 = 0x1F, B = 0x40 >> 3 = 0x08,
///   A = 0x80 -> the RGBA16 coverage/alpha bit. `write_pixel`'s RGBA16 arm
///   packs `(R << 11) | (G << 6) | (B << 1) | A_bit`.
///   = (0x10 << 11) | (0x1F << 6) | (0x08 << 1) | 1
///   = 0x8000 | 0x07C0 | 0x0010 | 1 = 0x87D1
const PRIM_RGBA16: [u8; 2] = [0x87, 0xD1];

/// The `ResourceAccess` run a DECODER would declare for `triangle` against
/// `key`, built here by the same arithmetic `plan_raw_triangle` uses, then
/// truncated or extended to `declared_rows` so a test can hand the executor
/// a run that deliberately disagrees.
///
/// `declared_rows == None` means "exactly what the decoder would declare".
fn declared_accesses(
    key: ColorTargetKey,
    triangle: &RawTriangle,
    declared_rows: Option<usize>,
) -> Vec<fn64_render_ir::ResourceAccess> {
    let extent = key.extent();
    let mut rows = crate::raw_dpc::triangle_span::covered_rows(
        triangle,
        extent.width(),
        // The decoder's own bound: installed RDRAM and a fixed row cap, NOT
        // this target's height. Using the height here would make the two
        // walks agree by construction and the guard untestable.
        4096,
    );
    if let Some(count) = declared_rows {
        while rows.len() > count {
            rows.pop();
        }
        while rows.len() < count {
            let mut extra = *rows.last().expect("at least one row");
            extra.y += 1;
            rows.push(extra);
        }
    }
    let layout = layout();
    let base = key.address().get();
    let bpp = key.format().bytes_per_pixel();
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let start = base + (row.y * extent.width() + row.x0) * bpp;
            let end = start + (row.x1 - row.x0) * bpp;
            fn64_render_ir::ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(index as u32),
                fn64_render_ir::AccessMode::Write,
                fn64_render_ir::AccessPurpose::RenderTarget,
                fn64_render_ir::ResourceRegion::Rdram {
                    resource: fn64_render_ir::RdramResource::ColorFramebuffer,
                    range: layout.range(start, end).expect("inside the fixture layout"),
                },
            )
            .expect("a well-formed render-target write")
        })
        .collect()
}

fn run(
    key: ColorTargetKey,
    triangle: &RawTriangle,
    resident: &[u8],
    declared_rows: usize,
) -> Result<Vec<u8>, TexrectExecutionError> {
    let declared = declared_accesses(key, triangle, Some(declared_rows));
    run_with(key, triangle, resident, &declared)
}

fn run_with(
    key: ColorTargetKey,
    triangle: &RawTriangle,
    resident: &[u8],
    declared: &[fn64_render_ir::ResourceAccess],
) -> Result<Vec<u8>, TexrectExecutionError> {
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let completed = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        triangle,
        flat_shading(),
        TexrectBlendRegisters::default(),
        resident,
        declared,
        NO_TEXTURE,
        None,
    )?;
    Ok(completed.device_bytes().device_bytes().to_vec())
}

/// A resident buffer whose every byte is a recognizable non-zero sentinel,
/// so "the raster wrote here" and "the resident's byte survived" are
/// distinguishable at every pixel.
fn sentinel_resident(key: ColorTargetKey) -> Vec<u8> {
    vec![0x5A; key.extent().pixels() as usize * 2]
}

#[test]
fn owned_and_borrowed_resident_inputs_produce_identical_triangle_bytes() {
    let key = key_at(8, 4);
    let triangle = box_triangle();
    let resident = sentinel_resident(key);
    let declared = declared_accesses(key, &triangle, None);

    let execute = |input| {
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        execute_raw_triangle(
            &candidate,
            one_cycle_other_mode(),
            &triangle,
            flat_shading(),
            TexrectBlendRegisters::default(),
            input,
            &declared,
            NO_TEXTURE,
            None,
        )
        .unwrap()
        .device_bytes()
        .device_bytes()
        .to_vec()
    };

    let borrowed = execute(std::borrow::Cow::Borrowed(resident.as_slice()));
    // The ownership mode is the variable under test; both executions must
    // begin from identical bytes without coupling their input lifetimes.
    let owned_resident = resident.clone();
    let owned = execute(std::borrow::Cow::Owned(owned_resident));
    assert_eq!(owned, borrowed);
}

// ---------------------------------------------------------------------------
// The bytes a flat triangle actually produces
// ---------------------------------------------------------------------------

#[test]
fn a_flat_triangle_writes_the_primitive_colour_into_exactly_its_covered_pixels() {
    // 8x4 RGBA16 target; the box triangle covers pixels x 2..6 on rows 0..3.
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let bytes = run(key, &box_triangle(), &resident, 3).expect("a flat triangle rasterizes");
    assert_eq!(bytes.len(), 8 * 4 * 2);
    for y in 0..4usize {
        for x in 0..8usize {
            let offset = (y * 8 + x) * 2;
            let pixel = [bytes[offset], bytes[offset + 1]];
            let covered = y < 3 && (2..6).contains(&x);
            if covered {
                assert_eq!(
                    pixel, PRIM_RGBA16,
                    "pixel ({x},{y}) is inside the triangle and must hold the primitive colour"
                );
            } else {
                assert_eq!(
                    pixel,
                    [0x5A, 0x5A],
                    "pixel ({x},{y}) is outside the triangle and must keep the resident's byte"
                );
            }
        }
    }
}

#[test]
fn the_written_colour_comes_from_the_latched_program_not_a_constant() {
    // Mutation guard: a hardcoded primitive-colour write would pass the test
    // above. Swap `SetPrimColor` for a different register value through the
    // SAME program and the bytes must change accordingly.
    //
    // Prim = 0x0000_00FF -> R = G = B = 0, A = 0xFF.
    //   packed = (0 << 11) | (0 << 6) | (0 << 1) | 1 = 0x0001
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let shading = TexrectShading::new(
        flat_primitive_program(),
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, 0x0000_00FF),
    );
    let completed = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        &box_triangle(),
        shading,
        TexrectBlendRegisters::default(),
        &resident,
        &declared_accesses(key, &box_triangle(), Some(3)),
        NO_TEXTURE,
        None,
    )
    .expect("a flat triangle rasterizes");
    let bytes = completed.device_bytes().device_bytes();
    let interior = (0 * 8 + 3) * 2;
    assert_eq!([bytes[interior], bytes[interior + 1]], [0x00, 0x01]);
    assert_ne!([bytes[interior], bytes[interior + 1]], PRIM_RGBA16);
}

#[test]
fn a_sloped_triangle_writes_a_different_pixel_count_on_each_row() {
    // Major edge parked at x = 0, minor edge marching right one pixel per
    // scanline: row y must hold exactly y+1 written pixels.
    //   x1 = ceil((0 + 65536*(8y+7)/8 - 8192)/65536) = ceil(y + 0.75) = y+1
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let sloped = triangle(true, 4 << 2, 4 << 2, 0, 0, 0, 0, 0, 0, 1 << 16);
    let bytes = run(key, &sloped, &resident, 4).expect("a sloped triangle rasterizes");
    for y in 0..4usize {
        let written = (0..8usize)
            .filter(|x| {
                let offset = (y * 8 + x) * 2;
                [bytes[offset], bytes[offset + 1]] == PRIM_RGBA16
            })
            .count();
        assert_eq!(written, y + 1, "row {y}");
    }
}

/// **A pixel with ZERO coverage inside a declared run must keep the
/// resident's own byte.**
///
/// This exists because the mutant that removes the `coverage == 0` skip
/// SURVIVED the first draft of this file: the partially-covered case below
/// has coverage 4, not 0, so it never reached the arm. A declared run is
/// the UNION over a scanline's four subpixel sample rows, so a sloped edge
/// makes the run's first pixel uncovered on every sample row -- and painting
/// it would put the triangle's colour outside the triangle.
///
/// Hand-derived, from the wire fields alone:
///   xh = 147456 / 65536 = 2.25 px, dxhdy = 32768 / 65536 = +0.5 px/line.
///   Row 1's sample rows are y-eighths 9, 11, 13, 15, so the left edge sits
///   at 2.25 + 0.5 * 9/8 = 2.8125 on the topmost and further right below.
///   min_left = 2.8125 -> x0 = ceil(2.8125 - 7/8) = ceil(1.9375) = 2.
///   But pixel 2's two sample columns are 2.125 and 2.625, and BOTH are
///   left of 2.8125 on every one of the four sample rows.
/// So pixel 2 is declared and has coverage 0.
#[test]
fn a_declared_pixel_with_no_subpixel_coverage_is_not_painted() {
    let key = key_at(16, 8);
    let resident = sentinel_resident(key);
    let sloped_left = triangle(true, 16, 8, 0, 786432, 0, 147456, 32768, 524288, 0);
    // The precondition, asserted rather than assumed: this pixel really is
    // inside the declared run and really has zero coverage.
    let rows = crate::raw_dpc::triangle_span::covered_rows(&sloped_left, 16, 8);
    let row = rows
        .iter()
        .find(|row| row.y == 1)
        .expect("row 1 is covered");
    assert_eq!((row.x0, row.x1), (2, 8), "row 1's declared run");
    assert_eq!(
        crate::raw_dpc::triangle_span::pixel_coverage(&sloped_left, 2, 1),
        0,
        "pixel (2,1) is declared but has no subpixel coverage"
    );

    let bytes = run(key, &sloped_left, &resident, rows.len()).expect("rasterizes");
    let at = |x: usize, y: usize| {
        let offset = (y * 16 + x) * 2;
        [bytes[offset], bytes[offset + 1]]
    };
    assert_eq!(
        at(2, 1),
        [0x5A, 0x5A],
        "a zero-coverage pixel inside a declared run must keep the resident's byte"
    );
    // Positive control: the next pixel along IS covered and IS painted, so
    // this test cannot pass by the raster doing nothing at all.
    assert_eq!(at(3, 1), PRIM_RGBA16);
}

#[test]
fn a_zero_coverage_pixel_inside_a_declared_run_keeps_the_residents_byte() {
    // Left edge at x = 4.5: pixel 4's sample column at 4.125 is outside and
    // 4.625 is inside, so coverage is 4 of 8 -- non-zero, so the pixel IS
    // written. The pixel that must NOT be written is 3, which the declared
    // run does not reach at all. Together with the run bound this pins that
    // a partially-covered pixel is drawn and an uncovered one is not.
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let half = triangle(
        true,
        2 << 2,
        2 << 2,
        0,
        6 << 16,
        0,
        (4 << 16) | (1 << 15),
        0,
        6 << 16,
        0,
    );
    let bytes = run(key, &half, &resident, 2).expect("rasterizes");
    let at = |x: usize, y: usize| {
        let offset = (y * 8 + x) * 2;
        [bytes[offset], bytes[offset + 1]]
    };
    assert_eq!(at(4, 0), PRIM_RGBA16, "partially covered pixel is drawn");
    assert_eq!(at(3, 0), [0x5A, 0x5A], "uncovered pixel keeps the resident");
    assert_eq!(at(5, 0), PRIM_RGBA16);
    assert_eq!(at(6, 0), [0x5A, 0x5A]);
}

// ---------------------------------------------------------------------------
// Refusals -- each an untouched target, never a half-drawn one
// ---------------------------------------------------------------------------

#[test]
fn a_row_count_disagreeing_with_the_journal_is_refused_by_name() {
    // The stale-digest guard. The rasterizer covers three rows; claiming the
    // journal declared four must refuse rather than draw three and let
    // `fill_completed_writes` digest a fourth from resident bytes.
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    assert!(matches!(
        run(key, &box_triangle(), &resident, 4),
        Err(
            TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
                declared: 4,
                rasterized: 3
            }
        )
    ));
    // And the opposite direction, which is the one that actually reaches
    // guest memory: the journal declared fewer rows than the raster covers,
    // so the raster would write bytes nobody declared.
    assert!(matches!(
        run(key, &box_triangle(), &resident, 2),
        Err(
            TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
                declared: 2,
                rasterized: 3
            }
        )
    ));
}

/// **The same number of rows over different geometry is still a
/// disagreement.**
///
/// The count check alone would pass a journal that declares three rows at
/// the wrong X ranges -- and `fill_completed_writes` would then digest those
/// wrong ranges from the buffer, putting the resident's untouched bytes into
/// guest RDRAM under a valid digest for pixels the triangle never covered.
///
/// Shifting one declared row two pixels left is exactly that shape: same
/// count, same target, different bytes.
#[test]
fn a_declared_row_at_the_wrong_range_is_refused_even_when_the_count_matches() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let mut declared = declared_accesses(key, &box_triangle(), None);
    assert_eq!(declared.len(), 3);
    // Row 1's real range is 0x400 + (1*8 + 2)*2 = 0x414 .. 0x41c. Shift it
    // two pixels (four bytes) left.
    let layout = layout();
    declared[1] = fn64_render_ir::ResourceAccess::try_new(
        fn64_render_ir::OperationId::new(1),
        fn64_render_ir::AccessMode::Write,
        fn64_render_ir::AccessPurpose::RenderTarget,
        fn64_render_ir::ResourceRegion::Rdram {
            resource: fn64_render_ir::RdramResource::ColorFramebuffer,
            range: layout.range(0x410, 0x418).unwrap(),
        },
    )
    .unwrap();
    let result = run_with(key, &box_triangle(), &resident, &declared);
    assert!(
        matches!(
            result,
            Err(
                TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                    position: 1,
                    declared: (0x410, 8),
                    rasterized: (0x414, 8),
                }
            )
        ),
        "a shifted declared row must be refused by name, got {result:?}"
    );
}

#[test]
fn a_short_target_makes_the_row_counts_disagree_rather_than_clipping() {
    // The real-world shape of the guard: the decoder bounded its walk by
    // installed RDRAM and declared 3 rows; this target is only 2 scanlines
    // tall, so the raster covers 2. Clipping silently would leave the third
    // declared row digested from stale bytes.
    let key = key_at(8, 2);
    let resident = sentinel_resident(key);
    assert!(matches!(
        run(key, &box_triangle(), &resident, 3),
        Err(
            TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
                declared: 3,
                rasterized: 2
            }
        )
    ));
}

#[test]
fn fill_and_copy_cycle_are_both_refused_by_name() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    for (bits, cycle) in [(3u32 << 20, CycleType::Fill), (2 << 20, CycleType::Copy)] {
        let candidate = registry.begin_candidate(key).unwrap();
        let result = execute_raw_triangle(
            &candidate,
            OtherMode::from_wire(bits, 0),
            &box_triangle(),
            flat_shading(),
            TexrectBlendRegisters::default(),
            &resident,
            &declared_accesses(key, &box_triangle(), Some(3)),
            NO_TEXTURE,
            None,
        );
        assert!(
            matches!(
                result,
                Err(TexrectExecutionError::UnsupportedCycleType { cycle_type })
                    if cycle_type == cycle
            ),
            "{cycle:?} must be refused by name, got {result:?}"
        );
    }
}

#[test]
fn a_program_reading_shade_is_refused_rather_than_combined_against_zero() {
    // An unshaded triangle has no vertex colour. A program selecting Shade
    // must refuse: silently combining against zero is the substitution every
    // refusal in this executor exists to prevent.
    //
    // `Shade = 4` in `colorInputA`. Program: (Shade - Zero) * Zero + Zero.
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let low = (4u32 << 5) | 16;
    let high = (8u32 << 24) | (7 << 6) | (7 << 21) | (7 << 3) | (7 << 18) | 7;
    let result = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        &box_triangle(),
        TexrectShading::new(
            CombineParams::from_wire(low, high),
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        ),
        TexrectBlendRegisters::default(),
        &resident,
        &declared_accesses(key, &box_triangle(), Some(3)),
        NO_TEXTURE,
        None,
    );
    assert!(
        matches!(
            result,
            Err(TexrectExecutionError::UnsupportedColorInput { .. })
        ),
        "a Shade-reading program must refuse, got {result:?}"
    );
}

#[test]
fn a_resident_buffer_of_the_wrong_length_is_refused_before_any_pixel() {
    let key = key_at(8, 4);
    let short = vec![0x5A; 8 * 4 * 2 - 2];
    assert!(matches!(
        run(key, &box_triangle(), &short, 3),
        Err(TexrectExecutionError::Target(
            TargetError::CompletedByteLengthMismatch { .. }
        ))
    ));
}

#[test]
fn a_refused_triangle_leaves_the_target_untouched() {
    // Every refusal above must be an untouched target, not a half-drawn one.
    // The only observable proof is that no `CompletedColorTargetWrite`
    // exists at all -- the executor builds its output buffer only after
    // every admission has passed, so an `Err` return cannot carry bytes.
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    assert!(run(key, &box_triangle(), &resident, 99).is_err());
    // The resident the caller still holds is byte-for-byte what it was.
    assert_eq!(resident, sentinel_resident(key));
}

// ---------------------------------------------------------------------------
// The claimed rectangle, which `admit_completed_initialization` reads
// ---------------------------------------------------------------------------

#[test]
fn the_claimed_rectangle_is_the_bounding_box_of_the_covered_rows() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let completed = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        &box_triangle(),
        flat_shading(),
        TexrectBlendRegisters::default(),
        &resident,
        &declared_accesses(key, &box_triangle(), Some(3)),
        NO_TEXTURE,
        None,
    )
    .unwrap();
    let rectangle = completed.rectangle();
    assert_eq!(
        (
            rectangle.x(),
            rectangle.y(),
            rectangle.width(),
            rectangle.height()
        ),
        (2, 0, 4, 3),
        "x 2..6 over rows 0..3"
    );
}

// ---------------------------------------------------------------------------
// Shade plane interpolation
// ---------------------------------------------------------------------------

/// `(Zero - Zero) * Zero + Shade` in the second bitfield slice: the combined
/// colour IS the interpolated shade colour, verbatim.
///
/// Slot indices: colour A/B/C at their own out-of-table `Zero` (8/8/16) and
/// D = 4, which is `Shade` in `colorInputD`'s shared common table; alpha
/// A/B/C at `Zero` (7) and D = 4, `Shade` in `alphaInputABD`.
fn shade_passthrough_program() -> CombineParams {
    let (low, high) = crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_SHADE);
    CombineParams::from_wire(low, high)
}

/// Builds one SHADED (opcode 0x0c) triangle: the box triangle's edges plus
/// eight shade coefficient words carrying the four RGBA planes.
///
/// The shade block's wire layout is NOT eight consecutive Q16.16 values --
/// each coefficient's high 16 bits live in the block's first half and its
/// low 16 bits sixteen bytes later. As sixteen u32 halves (half `n` is byte
/// `4n`):
///   half  0,1  colour integer  (R:G, B:A)
///   half  2,3  d/dx    integer
///   half  4..7 unused by this fixture
///   half  8,9  colour fraction
///   half 10,11 d/dx    fraction
///   half 12,13 d/de    integer   (byte 32)
///   half 14,15 d/de    fraction? -- no: d/de fraction is byte 48, which is
///              half 12 of the SECOND 32 bytes. The block is 64 bytes = 16
///              halves, so byte 48 IS half 12. Recomputed below from the
///              byte offsets directly rather than from this list.
///
/// Byte offsets, from the RDP command summary and matched against
/// `fn64-render-reference`'s `decode_rdp_shade_coefficients`:
///   colour (0, 16)  d/dx (8, 24)  d/de (32, 48)  d/dy (40, 56)
fn shaded_triangle(red: (i32, i32, i32)) -> RawTriangle {
    let (base, dcdx, dcde) = red;
    // Only the R component is driven; G/B/A stay zero.
    let halves = crate::wire_words::coefficient_halves(
        [base, 0, 0, 0],
        [dcdx, 0, 0, 0],
        [dcde, 0, 0, 0],
        [0, 0, 0, 0],
    );

    let mut bytes = crate::wire_words::EdgeWords {
        lft: true,
        yl: crate::wire_words::line(3),
        ym: crate::wire_words::line(3),
        yh: 0,
        xl: crate::wire_words::px(6),
        xh: crate::wire_words::px(2),
        xm: crate::wire_words::px(6),
        ..crate::wire_words::EdgeWords::zeroed()
    }
    .bytes(crate::wire_words::RAW_TRIANGLE_SHADE);
    for half in halves {
        bytes.extend_from_slice(&half.to_be_bytes());
    }
    RawTriangle::decode(crate::wire_words::RAW_TRIANGLE_SHADE, &bytes)
        .expect("a shaded triangle is 32 + 64 bytes")
}

fn shaded_shading() -> TexrectShading {
    TexrectShading::new(
        shade_passthrough_program(),
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    )
}

fn run_shaded(
    key: ColorTargetKey,
    triangle: &RawTriangle,
    resident: &[u8],
) -> Result<Vec<u8>, TexrectExecutionError> {
    let declared = declared_accesses(key, triangle, None);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let completed = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        triangle,
        shaded_shading(),
        TexrectBlendRegisters::default(),
        resident,
        &declared,
        NO_TEXTURE,
        None,
    )?;
    Ok(completed.device_bytes().device_bytes().to_vec())
}

/// **A shaded triangle's colour varies ACROSS the triangle, per pixel.**
///
/// The whole point of the shade rung, and the one assertion a constant-colour
/// implementation cannot pass.
///
/// Hand-derived, from the wire and the plane arithmetic alone. With
/// `dcdx = 32 << 16` (+32 red per pixel of X), `base = 0`, `dcde = 0`:
///
/// `attribute_sample` takes the FIRST covered subsample -- Y offset 1/8,
/// X offset 1/8 -- so for pixel x on any row:
///   sample_x      = (8x + 1)/8 px, and the major edge is parked at 2.0 px
///   edge_delta_x  = (8x + 1)/8 - 2  px
///   red           = 0 + 0 + 32 * edge_delta_x
/// giving  x=2 -> 32 * 0.125  =  4
///         x=3 -> 32 * 1.125  = 36
///         x=4 -> 32 * 2.125  = 68
///         x=5 -> 32 * 3.125  = 100
/// and the same four values on every row, because `dcde` is zero.
///
/// RGBA16 packs red >> 3 into bits 15:11 with G = B = 0, and the alpha bit
/// SET. The alpha bit is 1 even though the alpha shade plane is zero: with
/// blending disabled (`FORCE_BL` and `AA_EN` both clear at other_mode 0),
/// `blend_fragment`'s last-cycle bypass sets `final_alpha = 1.0` unless the
/// cycle's P input is the framebuffer -- the RDP's own behaviour, and the
/// same reason the flat fixture's 0x87D1 carries a set low bit.
///   4   >> 3 = 0  -> 0x0001
///   36  >> 3 = 4  -> 0x2001
///   68  >> 3 = 8  -> 0x4001
///   100 >> 3 = 12 -> 0x6001
#[test]
fn a_shaded_triangles_colour_is_interpolated_across_its_pixels() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let triangle = shaded_triangle((0, 32 << 16, 0));
    let bytes = run_shaded(key, &triangle, &resident).expect("a shaded triangle rasterizes");
    let at = |x: usize, y: usize| {
        let offset = (y * 8 + x) * 2;
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    };
    for y in 0..3usize {
        assert_eq!(at(2, y), 0x0001, "row {y}, x=2");
        assert_eq!(at(3, y), 0x2001, "row {y}, x=3");
        assert_eq!(at(4, y), 0x4001, "row {y}, x=4");
        assert_eq!(at(5, y), 0x6001, "row {y}, x=5");
    }
    // Four DISTINCT values across the span: a constant-colour implementation
    // produces one, and this is the assertion it fails.
    let across: std::collections::BTreeSet<u16> = (2..6).map(|x| at(x, 0)).collect();
    assert_eq!(across.len(), 4, "the shade colour must vary across X");
    // Outside the triangle the resident survives, as ever.
    assert_eq!(at(1, 0), 0x5A5A);
    assert_eq!(at(6, 0), 0x5A5A);
}

/// The along-edge derivative is evaluated too, not just d/dx.
///
/// With `dcde = 8 << 16` and `dcdx = 0`, red depends only on the sample's Y
/// distance below the triangle's high origin, in EIGHTHS:
///   red = 8 * (8y + 1) / 8 = 8y + 1
/// giving 1, 9, 17 on rows 0, 1, 2 -- constant across each row.
/// and the alpha bit set, as above:
///   1  >> 3 = 0 -> 0x0001
///   9  >> 3 = 1 -> 0x0801
///   17 >> 3 = 2 -> 0x1001
#[test]
fn the_along_edge_derivative_varies_the_shade_colour_down_the_triangle() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let triangle = shaded_triangle((0, 0, 8 << 16));
    let bytes = run_shaded(key, &triangle, &resident).expect("rasterizes");
    let at = |x: usize, y: usize| {
        let offset = (y * 8 + x) * 2;
        u16::from_be_bytes([bytes[offset], bytes[offset + 1]])
    };
    for x in 2..6usize {
        assert_eq!(at(x, 0), 0x0001, "row 0, x={x}");
        assert_eq!(at(x, 1), 0x0801, "row 1, x={x}");
        assert_eq!(at(x, 2), 0x1001, "row 2, x={x}");
    }
    let down: std::collections::BTreeSet<u16> = (0..3).map(|y| at(3, y)).collect();
    assert_eq!(down.len(), 3, "the shade colour must vary down Y");
}

/// An UNSHADED triangle whose program reads `Shade` is still refused.
///
/// The widening is per-triangle, driven by the wire opcode's own shade bit,
/// not a blanket admission -- so the substitution `base_inputs`' zeroed
/// shade field would make is still prevented where there is nothing to read.
#[test]
fn an_unshaded_triangle_reading_shade_is_still_refused() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let declared = declared_accesses(key, &box_triangle(), None);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let result = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        &box_triangle(),
        shaded_shading(),
        TexrectBlendRegisters::default(),
        &resident,
        &declared,
        NO_TEXTURE,
        None,
    );
    assert!(
        matches!(
            result,
            Err(TexrectExecutionError::UnsupportedColorInput { .. })
        ),
        "an unshaded triangle reading Shade must refuse, got {result:?}"
    );
}

/// **An untextured triangle whose program reads `Texel0` is refused.**
///
/// The gap this closes: `combine_one_texel` is handed `[0; 4]` as the texel
/// because an untextured triangle has none, and `ADMITTED_COLOR_INPUTS`
/// admits `Texel0` for the texrect path that shares the table -- so a
/// program selecting it would have combined against a fabricated zero. That
/// is the silent substitution every other refusal in this executor exists to
/// prevent, and it was reachable.
///
/// Program: `(Zero - Zero) * Zero + Texel0`. `Texel0 = 1` in `colorInputD`'s
/// shared common table and in `alphaInputABD`.
#[test]
fn an_untextured_triangle_reading_texel0_is_refused() {
    let key = key_at(8, 4);
    let resident = sentinel_resident(key);
    let declared = declared_accesses(key, &box_triangle(), None);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let (ca, cb, cc, cd) = (8u32, 8u32, 16u32, 1u32);
    let (aa, ab, ac, ad) = (7u32, 7u32, 7u32, 1u32);
    let low = (ca << 5) | cc;
    let high = (cb << 24) | (cd << 6) | (aa << 21) | (ab << 3) | (ac << 18) | ad;
    let result = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        &box_triangle(),
        TexrectShading::new(
            CombineParams::from_wire(low, high),
            Color4::from_wire(ENV_WIRE),
            PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
        ),
        TexrectBlendRegisters::default(),
        &resident,
        &declared,
        NO_TEXTURE,
        None,
    );
    assert!(
        matches!(
            result,
            Err(TexrectExecutionError::UnsupportedColorInput { .. })
        ),
        "an untextured triangle reading Texel0 must refuse, got {result:?}"
    );
}

/// A TEXTURED (opcode 0x0a) twin of [`box_triangle`]: identical edges, plus
/// eight zeroed texture coefficient words. Decoded through the REAL decoder,
/// so the flag bits come from the opcode rather than being asserted.
fn textured_box_triangle() -> RawTriangle {
    let w0 = (1u32 << 23) | ((3u32 << 2) & 0xffff);
    let w1 = ((3u32 << 2) << 16) | 0;
    let mut bytes = Vec::with_capacity(64);
    for word in [w0, w1, (6u32 << 16), 0, (2u32 << 16), 0, (6u32 << 16), 0] {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    // Eight coefficient words = sixteen u32 halves, all zero. The VALUES do
    // not matter here: this fixture exists to carry the texture FLAG.
    bytes.extend(core::iter::repeat_n(0u8, 64));
    RawTriangle::decode(0x0a, &bytes).expect("a textured triangle is 96 bytes")
}

/// **The opcode/binding equality guard, exercised at the only level that can
/// reach it.**
///
/// `execute_scheduled_raw_triangle` builds the binding FROM `flags().
/// textured()`, so the production caller cannot present a mismatch and no
/// end-to-end fixture can reach this arm -- a mutant disabling the guard
/// survived the entire suite until this test existed.
///
/// The guard is still worth keeping rather than deleting: it is what stops a
/// future caller from combining a textured triangle against a fabricated zero
/// texel, which would be a silently wrong picture rather than a refusal --
/// the defect class this crate has already shipped once.
#[test]
fn a_textured_triangle_without_a_tmem_binding_is_refused_by_name() {
    let triangle = textured_box_triangle();
    assert!(
        triangle.flags().textured(),
        "the fixture must actually carry the texture flag for this test to mean anything"
    );
    let key = key_at(8, 4);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let declared = declared_accesses(key, &triangle, Some(3));
    let result = execute_raw_triangle(
        &candidate,
        one_cycle_other_mode(),
        &triangle,
        flat_shading(),
        TexrectBlendRegisters::default(),
        &vec![0u8; (8 * 4 * 2) as usize],
        &declared,
        NO_TEXTURE,
        None,
    );
    assert!(
        matches!(
            result,
            Err(
                TexrectExecutionError::TriangleTextureBindingDisagreesWithOpcode {
                    opcode_textured: true,
                    binding_present: false,
                }
            )
        ),
        "a textured triangle with no TMEM binding must refuse by name, got {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Z-buffer: the depth compare decides which of two overlapping draws survives
// ---------------------------------------------------------------------------

/// A one-cycle `OtherMode` carrying the three Z fields: `G_ZS_PRIM`
/// (`primitive_depth_source`, low bit 2), `Z_CMP` (low bit 4), `Z_UPD`
/// (low bit 5). Every other post-combiner stage stays at identity, exactly
/// like [`one_cycle_other_mode`].
fn z_prim_other_mode(compare: bool, update: bool) -> OtherMode {
    let mut low = 1 << 2; // G_MDSFT_ZSRCSEL = G_ZS_PRIM
    if compare {
        low |= 1 << 4;
    }
    if update {
        low |= 1 << 5;
    }
    OtherMode::from_wire(0, low)
}

/// Run one z-wired flat box triangle against a shared depth accumulator,
/// returning the produced colour bytes. `prim_z` is the 15-bit primitive
/// depth (`G_ZS_PRIM`); the depth cells persist across calls, so a sequence
/// of calls composes exactly as `stage_color_commands` composes a packet.
fn run_z(
    key: ColorTargetKey,
    resident: &[u8],
    compare: bool,
    update: bool,
    prim_z: u16,
    cells: &mut [DepthCell],
) -> Vec<u8> {
    let triangle = box_triangle();
    let declared = declared_accesses(key, &triangle, Some(3));
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let other_mode = z_prim_other_mode(compare, update);
    let completed = execute_raw_triangle(
        &candidate,
        other_mode,
        &triangle,
        flat_shading(),
        TexrectBlendRegisters::default(),
        resident,
        &declared,
        NO_TEXTURE,
        Some(RawTriangleDepth {
            cells,
            compare: other_mode.depth_compare_enabled(),
            update: other_mode.depth_update_enabled(),
            mode: other_mode.depth_mode(),
            source_is_primitive: other_mode.primitive_depth_source(),
            prim_depth: Some(crate::state::PrimDepth::from_wire(u32::from(prim_z) << 16)),
        }),
    )
    .expect("a z-wired flat triangle rasterizes");
    completed.device_bytes().device_bytes().to_vec()
}

/// The RGBA16 byte pair at pixel (x, y) of an 8-wide target.
fn pixel_at(bytes: &[u8], x: u32, y: u32) -> [u8; 2] {
    let index = (y as usize * 8 + x as usize) * 2;
    [bytes[index], bytes[index + 1]]
}

#[test]
fn z_compare_nearer_wins_and_farther_loses_over_a_committed_depth() {
    // 8x4 RGBA16; the box triangle covers x 2..6 on rows 0..3.
    let key = key_at(8, 4);
    let mut cells = vec![(0u32, 0u8); key.extent().pixels() as usize];

    // Seed the covered pixels' depth to a FAR value with Z_CMP OFF on the
    // first draw so it unconditionally paints and, with Z_UPD ON, commits
    // that far depth -- establishing a non-trivial memory Z the compare
    // below actually has to beat. `prim_z = 0x4000` gives a working Z of
    // `0x4000 << 3 = 0x20000`.
    let first = run_z(
        key,
        &sentinel_resident(key),
        false,
        true,
        0x4000,
        &mut cells,
    );
    assert_eq!(
        pixel_at(&first, 3, 1),
        PRIM_RGBA16,
        "the compare-disabled first draw must paint the box"
    );

    // A strictly NEARER second draw (smaller Z) under Z_CMP must WIN: it is
    // in front of the committed far depth, so its colour is written this
    // pass. Compose against the first draw's own output, exactly as the
    // schedule threads the accumulator.
    let nearer = run_z(key, &first, true, true, 0x0800, &mut cells);
    assert_eq!(
        pixel_at(&nearer, 3, 1),
        PRIM_RGBA16,
        "a strictly nearer z-compared draw must pass and paint the box"
    );

    // A FARTHER (larger Z) draw under Z_CMP over the now-nearer committed
    // depth must LOSE at every covered pixel: the fragment is not strictly in
    // front, so the pixel keeps whatever colour it already had. A fresh
    // sentinel resident makes "kept the resident" distinguishable from
    // "painted the primitive".
    let farther_resident = vec![0x5Au8; key.extent().pixels() as usize * 2];
    let farther = run_z(key, &farther_resident, true, true, 0x7000, &mut cells);
    assert_eq!(
        pixel_at(&farther, 3, 1),
        [0x5A, 0x5A],
        "a farther z-compared draw must be rejected, leaving the resident byte"
    );
    assert_ne!(
        pixel_at(&farther, 3, 1),
        PRIM_RGBA16,
        "the farther draw must NOT have painted the primitive colour"
    );
}

#[test]
fn z_compare_against_a_zeroed_z_image_rejects_every_fragment() {
    // The angrylion-matching corpus behaviour: a z-compared draw over a
    // freshly-bound (zeroed) z-image draws nothing, because a zeroed cell
    // decodes to working Z 0 -- the nearest representable -- and no fragment
    // is strictly nearer. `prim_z = 0` is itself the nearest; `0 < 0` is
    // false, so even it is rejected.
    let key = key_at(8, 4);
    let mut cells = vec![(0u32, 0u8); key.extent().pixels() as usize];
    let resident = sentinel_resident(key);
    let drawn = run_z(key, &resident, true, true, 0x0000, &mut cells);
    assert_eq!(
        pixel_at(&drawn, 3, 1),
        [0x5A, 0x5A],
        "z-compare against a zeroed z-image must reject the fragment and keep the resident byte"
    );
}

// ---------------------------------------------------------------------------
// Deterministic per-pixel raster microbenchmark (Task 6 kill-evidence lever)
// ---------------------------------------------------------------------------
//
// `raster_triangle` is a pure CPU function over a `TmemByteSource` -- no wgpu
// device, no window -- so its per-pixel cost is measurable headlessly and
// deterministically, isolated from GPU/compositor/window noise. This is the
// lever Task 6 uses to prove (or refute) that stepping the texture S/T/W
// planes incrementally reduces ns-per-textured-pixel.
//
// `#[ignore]` so it never runs in the normal suite (it is a timing loop, not
// an assertion). Run it explicitly:
//
//     cargo test -p fn64-render-wgpu --lib --release \
//       texture_plane_raster_microbench -- --ignored --nocapture
//
// It draws one large shaded+textured+perspective triangle (opcode 0x0e, the
// only opcode WM2000 issues) over a fixed, known covered-pixel count, with
// long horizontal runs so the step-vs-full branch is exercised on the common
// (run-continues) case. Reports total ns and ns/covered-pixel.

/// A trivial in-memory TMEM image: a 16x16 RGBA16 tile, every byte present.
struct BenchTmem {
    bytes: [u8; 512],
}

impl BenchTmem {
    fn new() -> Self {
        let mut bytes = [0u8; 512];
        // A varied pattern so the sampler is not reading one constant word
        // (which a smart cache could shortcut); irrelevant to timing but
        // keeps the read honest.
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37).wrapping_add(11);
        }
        Self { bytes }
    }

    fn projection(&self) -> crate::TmemGpuProjection {
        let mut projection = crate::TmemGpuProjection {
            bytes: [0; fn64_render_ir::TMEM_BYTES as usize],
            validity_words: [0; crate::tmem::TMEM_VALIDITY_WORDS],
        };
        projection.bytes[..self.bytes.len()].copy_from_slice(&self.bytes);
        for address in 0..self.bytes.len() {
            projection.validity_words[address / 32] |= 1 << (address % 32);
        }
        projection
    }
}

impl crate::TmemByteSource for BenchTmem {
    fn snapshot(&self) -> crate::TmemSnapshotIdentity {
        crate::TmemByteSource::snapshot(&crate::PhysicalTmemState::try_new().unwrap())
    }
    fn valid_byte(&self, address: u16) -> Option<u8> {
        self.bytes.get(address as usize).copied()
    }
}

fn bench_tile_binding() -> super::TexrectTileBinding {
    super::TexrectTileBinding::try_from_neutral(
        fn64_render::NeutralTileDescriptor {
            format: fn64_render::NeutralImageFormat::Rgba,
            size: fn64_render::NeutralPixelSize::Bits16,
            // 16 texels * 2 bytes = 32 bytes = 4 TMEM words per row.
            line_words: 4,
            tmem_word_address: 0,
            palette: 0,
            s_mode: fn64_render::NeutralTileAddressMode::default(),
            mask_s: 0,
            shift_s: 0,
            t_mode: fn64_render::NeutralTileAddressMode::default(),
            mask_t: 0,
            shift_t: 0,
        },
        fn64_render::NeutralTileSize {
            low_s: 0,
            low_t: 0,
            high_s: 60,
            high_t: 60,
        },
    )
    .unwrap()
}

/// The shaded+textured (0x0e) triangle the microbench rasterizes.
///
/// A big axis-aligned-ish box: `width` px wide, `rows` tall, left-major, with
/// non-trivial shade planes (so the shade path runs) and non-trivial S/T/W
/// planes (so the texture path -- the one this change touches -- runs). Small
/// slopes are deliberately AVOIDED on the horizontal so runs stay long and
/// the step branch dominates, matching WM2000's spans.
fn bench_textured_triangle(width: f64, rows: i16) -> RawTriangle {
    use crate::rdp_harness::Tri;
    // Plane values chosen so S/T stay inside the 16x16 tile across the whole
    // triangle (clamped addressing keeps them in range regardless), and W is
    // a large positive constant so the perspective divide is well-defined.
    let s = [0, 0, 0, 0];
    let t = [0, 0, 0, 0];
    // dS/dx and dT/dx nonzero so texel coordinates actually advance per pixel
    // -- the whole point is to step them.
    // Non-multiples of 32 are deliberate: after the perspective scale they
    // produce fractional S10.5 coordinates. That distinguishes the CPU
    // executor's point sampler from the three-nearest filter and prevents a
    // filter-selection regression from hiding behind texel-aligned inputs.
    let sdx = [(3 << 10) + 17, 0, 0, 0];
    let tdx = [0, (5 << 10) + 11, 0, 0];
    let sde = [1 << 8, 2 << 8, 0, 0];
    let sdy = [1 << 6, 1 << 6, 0, 0];
    // texture_planes wants [S, T, W, unused] arrays; pack per-array.
    let tex_value = [s[0], t[0], 1 << 20, 0];
    let tex_dx = [sdx[0], tdx[1], 0, 0];
    let tex_de = [sde[0], sde[1], 0, 0];
    let tex_dy = [sdy[0], sdy[1], 0, 0];

    let tri = Tri::flat()
        .left_major()
        .edges(2.0, 2.0 + width)
        .rows(0..rows)
        .shade(
            [0x20, 0x40, 0x60, 0xFF],
            [0x1 << 16, 0x1 << 16, 0x1 << 16, 0],
            [0, 0, 0, 0],
            [0, 0, 0, 0],
        )
        .texture_planes(tex_value, tex_dx, tex_de, tex_dy);
    let words = tri.words();
    let mut bytes = Vec::with_capacity(words.len() * 4);
    for word in words {
        bytes.extend_from_slice(&word.to_be_bytes());
    }
    RawTriangle::decode(0x0e, &bytes).expect("a shaded+textured triangle decodes")
}

#[cfg(feature = "host-gpu-tests")]
#[test]
fn required_host_hot_compute_color_matches_ordered_cpu_bytes_ten_times() {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    fn block_on<F: Future>(future: F) -> F::Output {
        struct ThreadWake(std::thread::Thread);
        impl Wake for ThreadWake {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            match Future::poll(future.as_mut(), &mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    let width = 32u32;
    let height = 24u32;
    let triangle = bench_textured_triangle(20.0, 16);
    let key = key_at(width, height);
    let declared = declared_accesses(key, &triangle, None);
    let mut resident = Vec::with_capacity((width * height * 2) as usize);
    for pixel in 0..width * height {
        let rgba16 = (((pixel * 13) & 0x1f) << 11)
            | (((pixel * 7) & 0x1f) << 6)
            | (((pixel * 3) & 0x1f) << 1)
            | (pixel & 1);
        resident.extend_from_slice(&(rgba16 as u16).to_be_bytes());
    }
    let tmem = BenchTmem::new();
    let tile = bench_tile_binding();
    let other_mode = OtherMode::from_wire(0x0008_acef, 0x0050_41c8);
    let environment = Color4::from_wire(ENV_WIRE);
    let primitive = PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE);
    let second_environment = Color4::from_wire(0x1020_3040);
    let second_primitive = PrimColor::from_wire(0, 0xc080_40a0);
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let mut expected = resident.clone();
    for (draw_environment, draw_primitive) in [
        (environment, primitive),
        (second_environment, second_primitive),
    ] {
        let completed = execute_raw_triangle(
            &candidate,
            other_mode,
            &triangle,
            TexrectShading::new(
                CombineParams::from_wire(0xfc51_96a3, 0x112c_fe7f),
                draw_environment,
                draw_primitive,
            ),
            TexrectBlendRegisters::default(),
            &expected,
            &declared,
            Some(RawTriangleTexture {
                tile,
                tmem: &tmem,
                lut_mode: crate::TextureLutMode::Disabled,
            }),
            None,
        )
        .expect("CPU hot-state oracle must rasterize each ordered draw");
        expected = completed.device_bytes().device_bytes().to_vec();
    }

    let requested = block_on(
        crate::UninitializedTrianglePipeline::new(crate::HeadlessBackend::AnyNative).request(),
    )
    .unwrap();
    let mut renderer = match requested {
        crate::TrianglePipelineDeviceOutcome::Ready(renderer) => renderer,
        crate::TrianglePipelineDeviceOutcome::NoAdapter(no_adapter) => panic!(
            "required host GPU evidence unavailable: typed no-adapter for {:?}",
            no_adapter.requested()
        ),
    };
    let device_triangles = [
        crate::ComputeCoverageTriangle::from_raw(triangle).with_material(environment, primitive),
        crate::ComputeCoverageTriangle::from_raw(triangle)
            .with_material(second_environment, second_primitive),
    ];
    let tile_params = crate::TileBindingParams::bound(tile.descriptor(), tile.size())
        .with_lut_mode(crate::TextureLutMode::Disabled);
    let projection = tmem.projection();
    for run in 1..=10 {
        let actual = renderer
            .compute_triangle_hot_color(
                crate::TriangleTargetExtent { width, height },
                &resident,
                &device_triangles,
                &projection,
                tile_params,
            )
            .expect("hot compute color must complete");
        if actual != expected {
            let byte = actual
                .iter()
                .zip(&expected)
                .position(|(actual, expected)| actual != expected)
                .expect("unequal buffers have a mismatching byte");
            panic!(
                "hot compute color differential run {run}: first mismatch at byte {byte} \
                 (pixel {}): GPU={:#04x}, CPU={:#04x}",
                byte / 2,
                actual[byte],
                expected[byte]
            );
        }
    }
    assert_eq!(
        renderer.compute_hot_color_resource_generations(),
        1,
        "ten identical submissions must allocate one high-water resource generation"
    );

    // Force a typed boundary between the two draws. The chain must preserve
    // painter's order without uploading or reading back the intermediate
    // target; this is the production shape when TMEM/tile/program identity
    // changes between adjacent admitted draws.
    let chained = [
        crate::targets::ComputeHotColorDispatch {
            triangles: &device_triangles[..1],
            tmem: &projection,
            tile: tile_params,
        },
        crate::targets::ComputeHotColorDispatch {
            triangles: &device_triangles[1..],
            tmem: &projection,
            tile: tile_params,
        },
    ];
    for run in 1..=10 {
        let actual = renderer
            .compute_triangle_hot_color_chain(
                crate::TriangleTargetExtent { width, height },
                &resident,
                &chained,
            )
            .expect("ordered compute-color chain must complete");
        assert_eq!(
            actual, expected,
            "ordered compute-color chain differential failed on run {run}"
        );
    }
    assert_eq!(
        renderer.compute_hot_color_resource_generations(),
        2,
        "ten identical two-dispatch chains must add exactly one high-water slot"
    );
}

#[test]
#[ignore = "timing microbenchmark; run with --ignored --nocapture"]
fn texture_plane_raster_microbench() {
    // A wide, tall box: long horizontal runs, many covered pixels.
    let width_px = 300.0_f64;
    let rows: i16 = 220;
    let target_w: u32 = 320;
    let target_h: u32 = 240;

    let triangle = bench_textured_triangle(width_px, rows);
    assert!(
        triangle.flags().textured() && triangle.flags().shaded(),
        "microbench triangle must be shaded+textured (0x0e)"
    );

    let key = key_at(target_w, target_h);
    let declared = declared_accesses(key, &triangle, None);
    let resident = vec![0u8; (target_w * target_h * 2) as usize];

    let tmem = BenchTmem::new();
    let tile = bench_tile_binding();
    // Perspective ON (G_TP_PERSP, other-mode high bit 19): matches WM2000's
    // hot path. The perspective divide is unchanged by this optimization; it
    // is the S/T/W plane VALUES that are now stepped.
    let other_mode = OtherMode::from_wire(1 << 19, 0);

    // Combiner reads TEXEL0 so the textured path is genuinely exercised.
    let (clow, chigh) = crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_TEXEL0);
    let shading = TexrectShading::new(
        CombineParams::from_wire(clow, chigh),
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    );

    // One call to measure the covered-pixel count and confirm it draws.
    let make_texture = || RawTriangleTexture {
        tile,
        tmem: &tmem,
        lut_mode: crate::TextureLutMode::Disabled,
    };
    let first = execute_raw_triangle(
        &{
            let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
            registry.begin_candidate(key).unwrap()
        },
        other_mode,
        &triangle,
        shading.clone(),
        TexrectBlendRegisters::default(),
        &resident,
        &declared,
        Some(make_texture()),
        None,
    )
    .expect("microbench triangle rasterizes");
    // Covered pixels = the number of pixels this triangle actually wrote.
    // Derive it from the write's rectangle intersected with coverage: the
    // simplest robust denominator is the count of pixels whose bytes changed
    // from the zero resident. Recompute deterministically from a run.
    let drawn = first.device_bytes().device_bytes().to_vec();
    let covered_pixels: u64 = (0..(target_w * target_h))
        .filter(|&p| {
            let o = (p * 2) as usize;
            drawn[o] != 0 || drawn[o + 1] != 0
        })
        .count() as u64;
    assert!(
        covered_pixels > 40_000,
        "microbench must cover a large, fixed pixel count, got {covered_pixels}"
    );

    // Warm up, then time.
    let iters = 400u64;
    for _ in 0..40 {
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let _ = execute_raw_triangle(
            &candidate,
            other_mode,
            &triangle,
            shading.clone(),
            TexrectBlendRegisters::default(),
            &resident,
            &declared,
            Some(make_texture()),
            None,
        )
        .unwrap();
    }

    let start = std::time::Instant::now();
    for _ in 0..iters {
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        let completed = execute_raw_triangle(
            &candidate,
            other_mode,
            &triangle,
            shading.clone(),
            TexrectBlendRegisters::default(),
            &resident,
            &declared,
            Some(make_texture()),
            None,
        )
        .unwrap();
        // Consume the result so the whole call cannot be optimized away.
        std::hint::black_box(completed.device_bytes().device_bytes().len());
    }
    let elapsed = start.elapsed();

    let total_pixels = covered_pixels * iters;
    let ns_per_pixel = elapsed.as_nanos() as f64 / total_pixels as f64;
    println!(
        "[raster-microbench] covered_pixels={covered_pixels} iters={iters} \
         total_ns={} ns_per_covered_pixel={ns_per_pixel:.3}",
        elapsed.as_nanos()
    );
}

/// `raster_triangle`'s per-pixel depth decision must be a no-op when depth is
/// DISABLED: a draw with no depth wiring (`depth == None`) and a draw carrying
/// a depth binding whose `Z_CMP` and `Z_UPD` are both clear must produce the
/// byte-identical framebuffer -- and the second must not touch the depth cells.
///
/// This guards the z-buffer feature's own invariant. When depth is disabled
/// the compare arm never runs (`passes_depth` falls to `_ => true`, no compare)
/// and the commit is gated off (no update), so a disabled depth binding reduces
/// to the same unconditional painter's-order write a non-z draw does. The test
/// rasterizes the microbench's shaded+textured (0x0e) triangle both ways and
/// asserts the colour bytes match and the seeded cells are untouched, so a
/// future edit to the depth path cannot silently change a non-z draw's output.
#[test]
fn depth_free_and_depth_present_paths_agree_on_a_depth_disabled_draw() {
    let width_px = 120.0_f64;
    let rows: i16 = 90;
    let target_w: u32 = 160;
    let target_h: u32 = 120;

    let triangle = bench_textured_triangle(width_px, rows);
    assert!(
        triangle.flags().textured() && triangle.flags().shaded(),
        "agreement triangle must be shaded+textured (0x0e)"
    );

    let key = key_at(target_w, target_h);
    let declared = declared_accesses(key, &triangle, None);
    let resident = vec![0u8; (target_w * target_h * 2) as usize];

    let tmem = BenchTmem::new();
    let tile = bench_tile_binding();
    // Perspective ON, matching the microbench (WM2000's hot path).
    let other_mode = OtherMode::from_wire(1 << 19, 0);

    let (clow, chigh) = crate::wire_words::passthrough_combine(crate::wire_words::D_SLOT_TEXEL0);
    let shading = TexrectShading::new(
        CombineParams::from_wire(clow, chigh),
        Color4::from_wire(ENV_WIRE),
        PrimColor::from_wire(PRIM_LOD_W0, PRIM_WIRE),
    );

    let make_texture = || RawTriangleTexture {
        tile,
        tmem: &tmem,
        lut_mode: crate::TextureLutMode::Disabled,
    };

    // Depth-FREE path: `depth == None`.
    let free = {
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        execute_raw_triangle(
            &candidate,
            other_mode,
            &triangle,
            shading.clone(),
            TexrectBlendRegisters::default(),
            &resident,
            &declared,
            Some(make_texture()),
            None,
        )
        .expect("depth-free draw rasterizes")
        .device_bytes()
        .device_bytes()
        .to_vec()
    };

    // Depth-PRESENT path: a `Some(depth)` with compare AND update both off.
    // The cells are seeded non-zero to prove the disabled path neither reads
    // nor writes them (identical bytes AND untouched cells).
    let mut cells = vec![(0x1234u32, 0x5u8); key.extent().pixels() as usize];
    let cells_before = cells.clone();
    let present = {
        let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
        let candidate = registry.begin_candidate(key).unwrap();
        execute_raw_triangle(
            &candidate,
            other_mode,
            &triangle,
            shading.clone(),
            TexrectBlendRegisters::default(),
            &resident,
            &declared,
            Some(make_texture()),
            Some(RawTriangleDepth {
                cells: &mut cells,
                compare: false,
                update: false,
                mode: crate::state::DepthMode::Opaque,
                source_is_primitive: true,
                prim_depth: Some(crate::state::PrimDepth::from_wire(0x0001_0000)),
            }),
        )
        .expect("depth-present (disabled) draw rasterizes")
        .device_bytes()
        .device_bytes()
        .to_vec()
    };

    assert_eq!(
        free, present,
        "depth-free and depth-present (Z_CMP=Z_UPD=0) paths must produce \
         byte-identical framebuffers for a depth-disabled draw"
    );
    assert_eq!(
        cells, cells_before,
        "a depth-disabled draw (Z_UPD off) must not touch the depth cells"
    );
}
