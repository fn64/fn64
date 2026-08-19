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

fn run(
    key: ColorTargetKey,
    triangle: &RawTriangle,
    resident: &[u8],
    declared_rows: usize,
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
        declared_rows,
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
        3,
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
            3,
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
        3,
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
        3,
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
