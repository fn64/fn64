// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::raster::*;
use crate::raster::coverage::*;
use crate::raster::combiner::*;
use crate::raster::blend::*;
use crate::raster::draw::*;
use crate::gbi::*;
use crate::depth::EncodedDepth;

pub(super) fn cycle(rgb: [ColorSource; 4], alpha: [AlphaSource; 4]) -> CombinerCycle {
    CombinerCycle { rgb, alpha }
}

pub(super) fn repeated_state(
    cycle: CombinerCycle,
    primitive: [u8; 4],
    environment: [u8; 4],
) -> CombinerState {
    CombinerState {
        mode: crate::gbi::CombinerMode { cycles: [cycle; 2] },
        primitive,
        environment,
        min_lod_level: 0,
        prim_lod_fraction: 0,
        convert: crate::gbi::ConvertState::default(),
        key: crate::gbi::KeyState::default(),
    }
}

pub(super) fn v(x: f32, y: f32, r: u8, g: u8, b: u8, a: u8) -> Vertex {
    Vertex {
        x,
        y,
        w: 1.0,
        r,
        g,
        b,
        a,
        ..Default::default()
    }
}

pub(super) fn standard_alpha_blender(cycle_count: u8) -> BlenderState {
    let cycle = BlendCycle {
        p: BlendColorInput::Combined,
        a: BlendAlphaInput::Combined,
        m: BlendColorInput::Framebuffer,
        b: BlendBInput::OneMinusA,
    };
    BlenderState {
        cycle_count,
        force_blend: true,
        cycles: [cycle; 2],
        ..Default::default()
    }
}

pub(super) fn shade_only_combiner() -> CombinerState {
    repeated_state(
        cycle(
            [
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Zero,
                ColorSource::Shade,
            ],
            [
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Zero,
                AlphaSource::Shade,
            ],
        ),
        [0; 4],
        [0; 4],
    )
}

pub(super) fn test_line(width: f32, smooth_shading: bool) -> Line {
    Line {
        v: [v(2.0, 4.0, 255, 0, 0, 255), v(6.0, 4.0, 0, 0, 255, 255)],
        width,
        smooth_shading,
        scissor: None,
        texture: None,
        other_mode: OtherMode::default(),
        combiner: shade_only_combiner(),
        blender: BlenderState::default(),
    }
}

pub(super) fn partial_attribute_line() -> Line {
    Line {
        v: [v(0.0, 0.5, 0, 0, 0, 255), v(0.5, 0.5, 200, 0, 0, 255)],
        width: 1.5,
        smooth_shading: true,
        scissor: None,
        texture: None,
        other_mode: OtherMode::from_raw(0xf0, 0, 0),
        combiner: shade_only_combiner(),
        blender: BlenderState::default(),
    }
}

pub(super) fn solid_texture(rgba: [u8; 4]) -> crate::gbi::Texture {
    crate::gbi::Texture {
        format: 0,
        size: 2,
        width: 1,
        height: 1,
        texels: std::rc::Rc::new(rgba.to_vec()),
        clamp_s: true,
        clamp_t: true,
        mirror_s: false,
        mirror_t: false,
        mask_s: 0,
        mask_t: 0,
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    }
}

pub(super) fn texel_passthrough_cycle(source: ColorSource, alpha: AlphaSource) -> CombinerCycle {
    cycle(
        [
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Zero,
            source,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            alpha,
        ],
    )
}

pub(super) fn texture_rectangle(
    texture: crate::gbi::Texture,
    other_mode: crate::gbi::OtherMode,
    combiner: CombinerState,
) -> TextureRectangle {
    TextureRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 1.0,
        lry: 1.0,
        tile: 0,
        s: 0.0,
        t: 0.0,
        dsdx: 1 << 10,
        dtdy: 1 << 10,
        flip: false,
        other_mode,
        combiner,
        blender: BlenderState {
            cycle_count: match other_mode.cycle_type() {
                CycleType::OneCycle => 1,
                CycleType::TwoCycle => 2,
                _ => 0,
            },
            ..BlenderState::default()
        },
        scissor: None,
        texture: Some(texture),
        texture1: None,
        fill_color: 0,
    }
}

/// Fails against the pre-alpha-compare rasterizer: the transparent black
/// half of a cutout texture used to overwrite the clear color as an opaque
/// black box. With G_AC_THRESHOLD + blend alpha 128, it is discarded while
/// the opaque half still draws.
#[test]
pub(super) fn threshold_alpha_compare_cuts_out_transparent_texels() {
    use crate::gbi::Texture;

    let cutout = Texture {
        format: 0,
        size: 2,
        width: 2,
        height: 1,
        texels: std::rc::Rc::new(vec![0, 0, 0, 0, 255, 255, 255, 255]),
        clamp_s: true,
        clamp_t: true,
        mirror_s: false,
        mirror_t: false,
        mask_s: 0,
        mask_t: 0,
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    };
    let mut tri = Triangle {
        v: [
            v(0.0, 0.0, 255, 255, 255, 255),
            v(8.0, 0.0, 255, 255, 255, 255),
            v(0.0, 8.0, 255, 255, 255, 255),
        ],
        texture: Some(cutout),
        other_mode: crate::gbi::OtherMode::from_raw((6 << 9) | 0xf0, 1 | 0x10 | 0x20, 128),
        ..Default::default()
    };
    tri.v[0].s = 0.0;
    tri.v[1].s = 2.0;
    tri.v[2].s = 0.0;

    let mut fb = Framebuffer::new(8, 8);
    fb.clear(9, 8, 7, 255);
    fb.draw_triangle(&tri);

    let transparent = (fb.width + 1) as usize * 4;
    let opaque = (fb.width + 5) as usize * 4;
    assert_eq!(&fb.pixels[transparent..transparent + 4], &[9, 8, 7, 255]);
    assert_eq!(&fb.pixels[opaque..opaque + 4], &[255, 255, 255, 255]);
}

/// Alpha rejection must precede both color and z writes. Otherwise a
/// transparent near cutout poisons depth and wrongly occludes opaque
/// geometry behind it even though its color was discarded.
#[test]
pub(super) fn rejected_alpha_does_not_update_depth() {
    let near_cutout = Triangle {
        v: [
            vz(2.0, 2.0, 1.0, 0, 0, 0, 0),
            vz(12.0, 2.0, 1.0, 0, 0, 0, 0),
            vz(7.0, 12.0, 1.0, 0, 0, 0, 0),
        ],
        other_mode: crate::gbi::OtherMode::from_raw(0xf0, 1 | 0x10 | 0x20, 128),
        ..Default::default()
    };
    let far_opaque = Triangle {
        v: [
            vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
            vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
            vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
        ],
        other_mode: crate::gbi::OtherMode::from_raw(0xf0, 0x10 | 0x20, 0),
        ..Default::default()
    };

    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    fb.draw_triangle_culled(&near_cutout, CullMode::None);
    fb.draw_triangle_culled(&far_opaque, CullMode::None);
    let overlap = (6u32 * fb.width + 7) as usize * 4;
    assert_eq!(&fb.pixels[overlap..overlap + 4], &[255, 0, 0, 255]);
    assert_eq!(fb.depth[overlap / 4], 64.0);
}

/// Fails against the overwrite bug: a half-alpha red fragment used to
/// replace the blue framebuffer with `[255,0,0,128]`. The standard OoT
/// XLU tuple must evaluate IN*A_IN + MEM*(1-A), retaining both colors.
#[test]
pub(super) fn translucent_triangle_composites_over_existing_framebuffer() {
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 255, 255);
    let tri = Triangle {
        v: [
            v(2.0, 2.0, 255, 0, 0, 128),
            v(12.0, 2.0, 255, 0, 0, 128),
            v(7.0, 12.0, 255, 0, 0, 128),
        ],
        other_mode: OtherMode::from_raw(0xf0, 0x4040, 0),
        blender: standard_alpha_blender(1),
        ..Default::default()
    };
    fb.draw_triangle(&tri);
    let idx = (6u32 * fb.width + 7u32) as usize * 4;
    // Barycentric interpolation truncates the nominal 128 alpha to 127 at
    // this sample, so the exact source-over result is 127 red / 128 blue.
    assert_eq!(&fb.pixels[idx..idx + 4], &[127, 0, 128, 255]);
}

/// Cycle 2 consumes cycle 1's blender result, not the original combined
/// color. Reusing the original red source in cycle 2 would produce
/// `[128,0,127]` rather than retaining cycle 1's green contribution.
#[test]
pub(super) fn two_cycle_blender_feeds_cycle_one_result_into_cycle_two() {
    let state = BlenderState {
        cycle_count: 2,
        force_blend: true,
        cycles: [
            BlendCycle {
                p: BlendColorInput::Combined,
                a: BlendAlphaInput::Combined,
                m: BlendColorInput::Blend,
                b: BlendBInput::OneMinusA,
            },
            BlendCycle {
                p: BlendColorInput::Combined,
                a: BlendAlphaInput::Combined,
                m: BlendColorInput::Fog,
                b: BlendBInput::OneMinusA,
            },
        ],
        blend_color: [0, 255, 0, 255],
        fog_color: [0, 0, 255, 255],
    };
    assert_eq!(
        blend_fragment(
            [255, 0, 0, 128],
            Some(ReadFramebufferMemory {
                rgba: [0, 0, 0, 255],
                coverage: Coverage::FULL,
            }),
            128,
            state,
            true,
        ),
        [64, 64, 127, 255]
    );
}

/// The common two-cycle fog arrangement blends fog by SHADE alpha in c1,
/// then uses a non-forced c2 P-input pass. This covers selector sources
/// beyond the standard framebuffer-alpha tuple.
#[test]
pub(super) fn fog_cycle_then_pass_uses_shade_alpha_and_prior_cycle_color() {
    let fog_then_pass = BlenderState {
        cycle_count: 2,
        force_blend: false,
        cycles: [
            BlendCycle {
                p: BlendColorInput::Fog,
                a: BlendAlphaInput::Shade,
                m: BlendColorInput::Combined,
                b: BlendBInput::OneMinusA,
            },
            BlendCycle {
                p: BlendColorInput::Combined,
                a: BlendAlphaInput::Zero,
                m: BlendColorInput::Combined,
                b: BlendBInput::One,
            },
        ],
        fog_color: [255, 0, 0, 255],
        ..Default::default()
    };
    assert_eq!(
        blend_fragment(
            [0, 0, 255, 255],
            Some(ReadFramebufferMemory {
                rgba: [0, 255, 0, 255],
                coverage: Coverage::FULL,
            }),
            64,
            fog_then_pass,
            false,
        ),
        [64, 0, 191, 255]
    );
}

// --- Depth / z-buffer occlusion regression ---------------------------
//
// These prove the z-buffer resolves overlapping geometry by DEPTH, not by
// submission (painter's) order, and in the correct DIRECTION (nearer =
// smaller `z` wins the `z < depth` compare, matching the OoT viewport z
// mapping `pz = ndc_z*sz + tz` with sz>0, verified live: sz=tz=127.75,
// ndc_z↑ with distance -> pz↑ with distance -> nearer has smaller pz).

/// A vertex with an explicit screen-space depth `z`.
pub(super) fn vz(x: f32, y: f32, z: f32, r: u8, g: u8, b: u8, a: u8) -> Vertex {
    Vertex {
        x,
        y,
        z,
        r,
        g,
        b,
        a,
        ..Default::default()
    }
}

/// Two fully-overlapping triangles at different depths: a NEAR blue one
/// (z=1) and a FAR red one (z=9), covering the same pixels. The nearer
/// (blue) color must survive at the overlap REGARDLESS of the order they
/// are submitted -- proving z-test, not painter's order.
#[test]
pub(super) fn nearer_triangle_wins_over_farther_regardless_of_submission_order() {
    // Same screen footprint for both; only z (and color) differ.
    let near = Triangle {
        v: [
            vz(2.0, 2.0, 1.0, 0, 0, 255, 255),
            vz(12.0, 2.0, 1.0, 0, 0, 255, 255),
            vz(7.0, 12.0, 1.0, 0, 0, 255, 255),
        ],
        other_mode: crate::gbi::OtherMode::from_raw(0xf0, 0x10 | 0x20, 0),
        ..Default::default()
    };
    let far = Triangle {
        v: [
            vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
            vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
            vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
        ],
        other_mode: crate::gbi::OtherMode::from_raw(0xf0, 0x10 | 0x20, 0),
        ..Default::default()
    };
    let overlap = (6u32 * 16 + 7u32) as usize * 4; // interior pixel (7,6)

    // Order A: far first, then near. Near must overwrite far.
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    fb.draw_triangle_culled(&far, CullMode::None);
    fb.draw_triangle_culled(&near, CullMode::None);
    assert_eq!(
        &fb.pixels[overlap..overlap + 4],
        &[0, 0, 255, 255],
        "near (blue) must win at overlap when drawn AFTER far"
    );

    // Order B: near first, then far. Near must STILL win (far is z-rejected).
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    fb.draw_triangle_culled(&near, CullMode::None);
    fb.draw_triangle_culled(&far, CullMode::None);
    assert_eq!(
        &fb.pixels[overlap..overlap + 4],
        &[0, 0, 255, 255],
        "near (blue) must STILL win when drawn BEFORE far -- this is what \
         separates a real z-test from painter's order"
    );
}

/// The whole point of a z-buffer over painter's order: WITHOUT the depth
/// test, submission order decides the overlap (last drawn wins), so the
/// far triangle drawn last would incorrectly show through. This documents
/// the difference the z-test makes and would catch a regression that
/// silently dropped the z-test on the culled path.
#[test]
pub(super) fn without_depth_test_painter_order_lets_farther_show_through() {
    let far = Triangle {
        v: [
            vz(2.0, 2.0, 9.0, 255, 0, 0, 255),
            vz(12.0, 2.0, 9.0, 255, 0, 0, 255),
            vz(7.0, 12.0, 9.0, 255, 0, 0, 255),
        ],
        ..Default::default()
    };
    let near = Triangle {
        v: [
            vz(2.0, 2.0, 1.0, 0, 0, 255, 255),
            vz(12.0, 2.0, 1.0, 0, 0, 255, 255),
            vz(7.0, 12.0, 1.0, 0, 0, 255, 255),
        ],
        ..Default::default()
    };
    let overlap = (6u32 * 16 + 7u32) as usize * 4;
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    // No-depth path: near first, far last -> far shows through (WRONG for a
    // real scene; this is exactly the artifact the z-buffer removes).
    fb.draw_triangle_no_depth_culled(&near, CullMode::None);
    fb.draw_triangle_no_depth_culled(&far, CullMode::None);
    assert_eq!(
        &fb.pixels[overlap..overlap + 4],
        &[255, 0, 0, 255],
        "without depth test, last-drawn (far/red) wins -- the painter's-order \
         artifact the z-buffer exists to prevent"
    );
}

/// Directly proves the z-test DIRECTION. `set_depth_tested` returns
/// whether it wrote. A nearer z (smaller) must pass over an existing
/// farther z; a farther z (larger) must be rejected. If the compare were
/// inverted (`z > depth`), the first assert would fail -- so this test
/// fails against a sign-flipped z-test bug.
#[test]
pub(super) fn set_depth_tested_passes_nearer_rejects_farther() {
    let mut fb = Framebuffer::new(2, 2);
    fb.clear(0, 0, 0, 255);
    // Write a mid-depth fragment.
    assert!(fb.set_depth_tested(0, 0, 5.0, [1, 1, 1, 1]));
    // A NEARER (smaller z) fragment must PASS and overwrite.
    assert!(
        fb.set_depth_tested(0, 0, 2.0, [2, 2, 2, 2]),
        "nearer z (2 < 5) must pass -- if this fails the z-test is inverted"
    );
    assert_eq!(&fb.pixels[0..4], &[2, 2, 2, 2]);
    // A FARTHER (larger z) fragment must be REJECTED (color unchanged).
    assert!(
        !fb.set_depth_tested(0, 0, 8.0, [3, 3, 3, 3]),
        "farther z (8 > 2) must be rejected"
    );
    assert_eq!(&fb.pixels[0..4], &[2, 2, 2, 2]);
}
