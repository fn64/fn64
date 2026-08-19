use fn64_render_ir::PhysicalMemoryLayout;

use super::super::texrect::{TexrectBlendRegisters, TexrectShading};
use super::super::{
    ColorTargetExtent, ColorTargetFormat, ColorTargetKey, ColorTargetRegistry, TargetError,
};
use super::*;
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
    )?;
    Ok(completed.device_bytes().device_bytes().to_vec())
}

/// A resident buffer whose every byte is a recognizable non-zero sentinel,
/// so "the raster wrote here" and "the resident's byte survived" are
/// distinguishable at every pixel.
fn sentinel_resident(key: ColorTargetKey) -> Vec<u8> {
    vec![0x5A; key.extent().pixels() as usize * 2]
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
    let sloped_left = triangle(
        true, 16, 8, 0, 786432, 0, 147456, 32768, 524288, 0,
    );
    // The precondition, asserted rather than assumed: this pixel really is
    // inside the declared run and really has zero coverage.
    let rows = crate::raw_dpc::triangle_span::covered_rows(&sloped_left, 16, 8);
    let row = rows.iter().find(|row| row.y == 1).expect("row 1 is covered");
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
        Err(TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
            declared: 4,
            rasterized: 3
        })
    ));
    // And the opposite direction, which is the one that actually reaches
    // guest memory: the journal declared fewer rows than the raster covers,
    // so the raster would write bytes nobody declared.
    assert!(matches!(
        run(key, &box_triangle(), &resident, 2),
        Err(TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
            declared: 2,
            rasterized: 3
        })
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
            Err(TexrectExecutionError::TriangleRowRangeDisagreesWithJournal {
                position: 1,
                declared: (0x410, 8),
                rasterized: (0x414, 8),
            })
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
        Err(TexrectExecutionError::TriangleRowCountDisagreesWithJournal {
            declared: 3,
            rasterized: 2
        })
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
    );
    assert!(
        matches!(
            result,
            Err(TexrectExecutionError::UnsupportedColorInput { .. })
        ),
        "an untextured triangle reading Texel0 must refuse, got {result:?}"
    );
}
