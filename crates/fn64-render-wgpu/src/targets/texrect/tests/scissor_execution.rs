use std::collections::BTreeMap;

use fn64_render::{NeutralImageFormat, NeutralPixelSize, NeutralTileAddressMode};
use fn64_render_ir::PhysicalMemoryLayout;

use super::*;
use crate::targets::{ColorTargetExtent, ColorTargetRegistry, CompletedColorTargetWrite, Rgba8};

const TARGET_WIDTH: u32 = 16;
const TARGET_HEIGHT: u32 = 16;
const FIXTURE_START: u32 = 0x400;
/// The RGBA16 halfword every fixture texel decodes to: red, opaque.
/// Distinct from the target's initialized blue so a written pixel is
/// unambiguous.
const TEXEL: u16 = 0xF801;
/// The colour the target is initialized to before the texrect runs.
/// Any pixel still holding this afterwards was NOT written.
const BACKGROUND: Rgba8 = Rgba8::new(0, 0, 255, 255);

fn layout() -> PhysicalMemoryLayout {
    PhysicalMemoryLayout::try_new(8 * 1024 * 1024).unwrap()
}

fn key() -> ColorTargetKey {
    ColorTargetKey::try_new(
        layout().address(FIXTURE_START).unwrap(),
        ColorTargetExtent::try_new(TARGET_WIDTH, TARGET_HEIGHT).unwrap(),
        ColorTargetFormat::Rgba16,
    )
    .unwrap()
}

/// A TMEM source holding one 16x16 RGBA16 tile of the single colour
/// [`TEXEL`], so every sampled texel is the same and a written pixel is
/// identified by its colour alone rather than by which texel it read.
struct FlatTmem {
    bytes: BTreeMap<u16, u8>,
}

impl FlatTmem {
    fn new() -> Self {
        let mut bytes = BTreeMap::new();
        // 16 rows of 16 RGBA16 texels: 32 bytes per row, 512 total.
        for address in 0..512u16 {
            bytes.insert(
                address,
                if address % 2 == 0 {
                    (TEXEL >> 8) as u8
                } else {
                    (TEXEL & 0xff) as u8
                },
            );
        }
        Self { bytes }
    }
}

impl crate::TmemByteSource for FlatTmem {
    fn snapshot(&self) -> crate::TmemSnapshotIdentity {
        crate::TmemByteSource::snapshot(&crate::PhysicalTmemState::try_new().unwrap())
    }

    fn valid_byte(&self, address: u16) -> Option<u8> {
        self.bytes.get(&address).copied()
    }
}

fn tile() -> TexrectTileBinding {
    TexrectTileBinding::try_from_neutral(
        fn64_render::NeutralTileDescriptor {
            format: NeutralImageFormat::Rgba,
            size: NeutralPixelSize::Bits16,
            // 16 texels * 2 bytes = 32 bytes = 4 TMEM words per row.
            line_words: 4,
            tmem_word_address: 0,
            palette: 0,
            s_mode: NeutralTileAddressMode::default(),
            mask_s: 0,
            shift_s: 0,
            t_mode: NeutralTileAddressMode::default(),
            mask_t: 0,
            shift_t: 0,
        },
        fn64_render::NeutralTileSize {
            low_s: 0,
            low_t: 0,
            // 10.2 fixed point: 15 pixels = 60 quarter-pixels.
            high_s: 60,
            high_t: 60,
        },
    )
    .unwrap()
}

/// Copy cycle: the sampled texel is blitted with no combiner and no
/// blender consulted, which is what the RDP does in that mode. Chosen
/// so this fixture needs no `SetCombine`, keeping the case about the
/// clip rather than about combiner setup.
fn copy_cycle_other_mode() -> OtherMode {
    // `cycle_type` is wire bits 20:21 of the high word; `2` is Copy
    // (`state.rs`'s `cycle_type()` decode).
    OtherMode::from_wire(2 << 20, 0)
}

/// Runs one texrect over a target pre-filled with [`BACKGROUND`], and
/// returns the resulting pixels plus the rectangle the write claimed.
fn run(
    draw: TexrectDraw,
    scissor: RdpScissorRect,
) -> Result<(Vec<Rgba8>, TargetRectangle), TexrectExecutionError> {
    run_with_input_ownership(draw, scissor, false)
}

fn run_with_input_ownership(
    draw: TexrectDraw,
    scissor: RdpScissorRect,
    owned: bool,
) -> Result<(Vec<Rgba8>, TargetRectangle), TexrectExecutionError> {
    let key = key();
    let registry = ColorTargetRegistry::try_new(layout(), 2).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let full = TargetRectangle::try_new(0, 0, TARGET_WIDTH, TARGET_HEIGHT).unwrap();
    let plan = candidate.plan_rows(full).unwrap();
    let background = vec![BACKGROUND; key.extent().pixels() as usize];
    let device = crate::targets::pack_device_pixels(&candidate, &background).unwrap();
    let resident_bytes = device.device_bytes().to_vec();
    let _ = candidate
        .admit_completed_initialization(CompletedColorTargetWrite {
            key,
            generation: plan.generation,
            range: key.range(),
            rectangle: plan.rectangle,
            device_bytes: device,
            coverage: crate::targets::ColorCoverageState::unknown(key.extent()),
        })
        .unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let input = if owned {
        Cow::Owned(resident_bytes)
    } else {
        Cow::Borrowed(resident_bytes.as_slice())
    };
    let completed = execute_texture_rectangle(
        &candidate,
        copy_cycle_other_mode(),
        draw,
        tile(),
        &FlatTmem::new(),
        TextureLutMode::Disabled,
        TexrectShading::new(
            CombineParams::from_wire(0, 0),
            Color4::from_wire(0),
            PrimColor::from_wire(0, 0),
        ),
        TexrectBlendRegisters::new(Color4::from_wire(0), Color4::from_wire(0)),
        scissor,
        input,
        None,
    )?;
    let rectangle = completed.rectangle;
    let pixels = crate::targets::unpack_device_pixels(
        ColorTargetFormat::Rgba16,
        completed.device_bytes.device_bytes(),
    )
    .expect("the write's own bytes unpack");
    Ok((pixels.into_vec(), rectangle))
}

fn full_rect_draw() -> TexrectDraw {
    TexrectDraw::try_from_viewport_and_texcoords(
        RectViewportPixels {
            left: 0,
            top: 0,
            right: TARGET_WIDTH as i32,
            bottom: TARGET_HEIGHT as i32,
        },
        [0.0, 0.0],
        // 16 pixels across a 16-pixel rect: one S10.5 raw unit per
        // pixel, well inside the tile's 0..60 quarter-pixel bounds.
        [16.0 / 32.0, 16.0 / 32.0],
    )
    .unwrap()
}

fn pixel(pixels: &[Rgba8], x: u32, y: u32) -> Rgba8 {
    pixels[(y * TARGET_WIDTH + x) as usize]
}

#[test]
fn owned_and_borrowed_resident_inputs_produce_identical_texrect_bytes() {
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 16, 16, 48, 48);
    let borrowed = run_with_input_ownership(full_rect_draw(), scissor, false).unwrap();
    let owned = run_with_input_ownership(full_rect_draw(), scissor, true).unwrap();
    assert_eq!(owned, borrowed);
}

/// **The executor writes only the scissored span.**
///
/// A full-target rectangle under a scissor covering pixels 4..12 on
/// both axes leaves everything outside that box untouched. Derived by
/// hand from the wire layout: `ulx = uly = 16` quarter-pixels is pixel
/// 4, `lrx = lry = 48` quarter-pixels is pixel 12 exclusive. Public
/// libultra's `gDPSetScissor` encodes all four coordinates multiplied by
/// four into twelve-bit fields
/// (`/Users/jer/Code/aki-recomp/refs/oot-decomp/include/ultra64/gbi.h:4794-4804`).
///
/// The scissor is strictly inside the 16x16 target on all four edges,
/// so a clip that consulted the target extent instead would write the
/// whole surface and fail every corner assertion below.
#[test]
fn the_executor_writes_only_the_scissored_span() {
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 16, 16, 48, 48);
    let (pixels, _) = run(full_rect_draw(), scissor).expect("a clipped rectangle draws");
    // Inside the scissor: written.
    for (x, y) in [(4, 4), (11, 11), (8, 8), (4, 11), (11, 4)] {
        assert_ne!(
            pixel(&pixels, x, y),
            BACKGROUND,
            "({x}, {y}) is inside the scissor and must be written"
        );
    }
    // Outside it on each of the four sides, and at a corner: untouched.
    for (x, y) in [(3, 8), (12, 8), (8, 3), (8, 12), (0, 0), (15, 15)] {
        assert_eq!(
            pixel(&pixels, x, y),
            BACKGROUND,
            "({x}, {y}) is outside the scissor and must be untouched"
        );
    }
}

/// **The claimed rectangle is the clipped one, not the command's.**
///
/// `admit_completed_initialization` reads this rectangle as proof of
/// which pixels a write established. Claiming the unclipped rect would
/// assert proof over the pixels the scissor kept the executor away
/// from -- pixels that still hold whatever was there before.
#[test]
fn the_claimed_rectangle_is_the_clipped_rectangle() {
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 16, 16, 48, 48);
    let (_, rectangle) = run(full_rect_draw(), scissor).expect("a clipped rectangle draws");
    assert_eq!(rectangle.x(), 4);
    assert_eq!(rectangle.y(), 4);
    assert_eq!(rectangle.width(), 8);
    assert_eq!(rectangle.height(), 8);
    // The command's own rectangle was the full 16x16 surface.
    assert_ne!(rectangle.width(), TARGET_WIDTH);
}

/// **A rectangle overhanging the target is DRAWN, not refused.**
///
/// This is the case the old `TexrectExecutionError::OutsideTarget`
/// refusal rejected outright, on the reasoning that "a clamped
/// rectangle would write pixels the RDP never covers." Pinned RT64
/// intersects the scissor and draw rectangles and keeps a non-empty
/// intersection instead of rejecting it
/// (`/Users/jer/Code/no-mercy-recompiled/third_party/rt64/src/hle/rt64_rdp.cpp:1214-1223`),
/// and fn64's own reference renderer does the
/// same at `fn64-render-reference/src/raster/draw.rs:197-203`.
///
/// FAILS BEFORE this change with `OutsideTarget`; passes after, with
/// the surviving half of the rectangle drawn.
#[test]
fn a_rectangle_overhanging_the_target_is_drawn_rather_than_refused() {
    // Starts at pixel 8 and runs to 24 -- eight pixels past the
    // 16-pixel target on each axis.
    let draw = TexrectDraw::try_from_viewport_and_texcoords(
        RectViewportPixels {
            left: 8,
            top: 8,
            right: 24,
            bottom: 24,
        },
        [0.0, 0.0],
        [16.0 / 32.0, 16.0 / 32.0],
    )
    .unwrap();
    // Wide open: 16 pixels = 64 quarter-pixels, so the scissor bounds
    // nothing and the TARGET extent is what clips. That makes this case
    // about the overhang specifically.
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 64, 64);
    let (pixels, rectangle) = run(draw, scissor).expect("an overhanging rectangle draws");
    assert_eq!((rectangle.x(), rectangle.y()), (8, 8));
    assert_eq!((rectangle.width(), rectangle.height()), (8, 8));
    assert_ne!(pixel(&pixels, 8, 8), BACKGROUND, "the surviving quarter");
    assert_ne!(pixel(&pixels, 15, 15), BACKGROUND, "up to the last pixel");
    assert_eq!(pixel(&pixels, 7, 7), BACKGROUND, "outside the rectangle");
}

/// The kept refusal, reached through the executor rather than the clip
/// helper: a rectangle with no surviving pixel is named, not silently
/// reported as a successful zero-pixel write.
#[test]
fn a_fully_scissored_rectangle_is_refused_through_the_executor() {
    // Scissor admits pixels 0..2; the rectangle starts at 8.
    let draw = TexrectDraw::try_from_viewport_and_texcoords(
        RectViewportPixels {
            left: 8,
            top: 8,
            right: 16,
            bottom: 16,
        },
        [0.0, 0.0],
        [8.0 / 32.0, 8.0 / 32.0],
    )
    .unwrap();
    let scissor = RdpScissorRect::from_wire_quarter_pixels(0, 0, 0, 8, 8);
    let error = run(draw, scissor).expect_err("nothing survives");
    assert!(
        matches!(error, TexrectExecutionError::ScissoredAway { .. }),
        "expected ScissoredAway, got {error:?}"
    );
}
