// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use super::support::*;
use crate::raster::*;
use crate::raster::coverage::*;
use crate::raster::combiner::*;
use crate::raster::blend::*;
use crate::raster::draw::*;
use crate::gbi::*;
use crate::depth::EncodedDepth;

#[test]
fn noise_combiner_source_uses_the_fragment_noise_byte() {
    let state = repeated_state(
        cycle(
            [
                ColorSource::Noise,
                ColorSource::Zero,
                ColorSource::One,
                ColorSource::Zero,
            ],
            [AlphaSource::Zero; 4],
        ),
        [0; 4],
        [0; 4],
    );
    assert_eq!(
        evaluate_combiner(
            state,
            CycleType::OneCycle,
            false,
            CombinerPixel::new(0.0, [0; 4], [0; 4], [0; 4], NoiseSample(0x80)),
        ),
        [128, 128, 128, 0],
    );
}

#[test]
fn partial_line_shade_uses_the_shared_covered_attribute_sample() {
    let line = partial_attribute_line();
    let mask = line_pixel_coverage(&line, ScissorRect::framebuffer(1, 1), 0, 0);
    assert_eq!(mask, CoverageMask(0x55));
    assert_eq!(
        mask.attribute_sample(),
        AttributeSamplePoint::Covered(CoveredAttributeSample {
            sample_index: 2,
            x_eighth: 3,
            y_eighth: 3,
        })
    );

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_line_no_depth(&line);
    assert_eq!(
        &framebuffer.pixels[..4],
        &[150, 0, 0, 255],
        "x=3/8 selects parameter 3/4; the old pixel-center path selected endpoint red 200"
    );
}

#[test]
fn full_coverage_line_attributes_remain_at_pixel_center() {
    let mut line = partial_attribute_line();
    line.v[1].x = 1.0;
    let mask = line_pixel_coverage(&line, ScissorRect::framebuffer(1, 1), 0, 0);
    assert_eq!(mask, CoverageMask(u8::MAX));
    assert_eq!(mask.attribute_sample(), AttributeSamplePoint::PixelCenter);

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_line_no_depth(&line);
    assert_eq!(&framebuffer.pixels[..4], &[100, 0, 0, 255]);
}

#[test]
fn line_raster_uses_public_minimum_width_and_butt_endpoints() {
    let mut framebuffer = Framebuffer::new(10, 8);
    framebuffer.draw_line_no_depth(&test_line(1.5, false));

    let pixel = |x: usize, y: usize| &framebuffer.pixels[(y * 10 + x) * 4..][..4];
    assert_eq!(pixel(3, 3), &[255, 0, 0, 255]);
    assert_eq!(pixel(3, 4), &[255, 0, 0, 255]);
    assert_eq!(pixel(3, 2), &[0, 0, 0, 0]);
    assert_eq!(pixel(1, 4), &[0, 0, 0, 0]);
    assert_eq!(pixel(6, 4), &[0, 0, 0, 0]);
}

#[test]
fn line_width_and_smooth_shading_change_coverage_and_color() {
    let mut narrow = Framebuffer::new(10, 8);
    narrow.draw_line_no_depth(&test_line(1.5, false));
    let mut wide = Framebuffer::new(10, 8);
    wide.draw_line_no_depth(&test_line(3.0, true));

    let painted = |framebuffer: &Framebuffer| {
        framebuffer
            .pixels
            .chunks_exact(4)
            .filter(|pixel| pixel.iter().any(|component| *component != 0))
            .count()
    };
    assert!(painted(&wide) > painted(&narrow));
    let center = (4 * 10 + 4) * 4;
    assert_eq!(&narrow.pixels[center..center + 4], &[255, 0, 0, 255]);
    assert_eq!(&wide.pixels[center..center + 4], &[95, 0, 159, 255]);
}

#[test]
fn line_depth_is_read_only_even_when_update_bit_is_programmed() {
    let mut framebuffer = Framebuffer::new(10, 8);
    let mut line = test_line(1.5, false);
    line.other_mode = OtherMode::from_raw(0xf0, 0x10 | 0x20, 0);
    framebuffer.draw_line(&line);
    assert!(framebuffer.depth.iter().all(|depth| depth.is_infinite()));
    assert!(framebuffer.pixels.iter().any(|component| *component != 0));
}

#[test]
fn partial_line_texture_coordinates_use_the_shared_covered_attribute_sample() {
    let mut line = partial_attribute_line();
    line.v[0].s = 0.0;
    line.v[1].s = 5.0;
    line.v[1].w = 2.0;
    line.other_mode = OtherMode::default();
    line.texture = Some(crate::gbi::Texture {
        format: 0,
        size: 2,
        width: 6,
        height: 1,
        texels: std::rc::Rc::new(
            [10u8, 20, 30, 40, 50, 60]
                .into_iter()
                .flat_map(|red| [red, 0, 0, 255])
                .collect(),
        ),
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
    });
    line.combiner = repeated_state(
        texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0),
        [0; 4],
        [0; 4],
    );

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_line_no_depth(&line);
    assert_eq!(
        &framebuffer.pixels[..4],
        &[40, 0, 0, 255],
        "perspective correction at x=3/8 selects S=3; the old pixel-center path selected endpoint S=5"
    );
}

#[test]
fn partial_line_depth_compare_uses_the_shared_sample_and_remains_read_only() {
    let mut line = partial_attribute_line();
    line.v[0].z = 100.0;
    line.v[1].z = 200.0;
    line.v[0].r = 255;
    line.v[1].r = 255;
    line.smooth_shading = false;
    // G_Z_CMP plus ZMODE_XLU gives a strict in-front comparison. The
    // selected x=3/8 point yields Z=1400 in the RDP 15.3 domain, while
    // the old pixel-center endpoint yielded Z=1600.
    line.other_mode = OtherMode::from_raw(0xf0, 0x0810, 0);

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.depth[0] = 1500.0;
    framebuffer.draw_line(&line);
    assert_eq!(&framebuffer.pixels[..4], &[255, 0, 0, 255]);
    assert_eq!(framebuffer.depth[0], 1500.0);
    assert_eq!(framebuffer.encoded_depth[0], None);
}

#[test]
fn textured_line_uses_perspective_attribute_path_and_scissor() {
    let mut line = test_line(1.5, true);
    line.v[0].s = 0.0;
    line.v[1].s = 1.0;
    line.v[0].w = 1.0;
    line.v[1].w = 2.0;
    line.texture = Some(solid_texture([0, 255, 0, 255]));
    line.combiner = repeated_state(
        texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0),
        [0; 4],
        [0; 4],
    );
    line.scissor = Some(ScissorRect {
        ulx: 4.0,
        uly: 0.0,
        lrx: 5.0,
        lry: 8.0,
        field: false,
        keep_odd: false,
    });

    let mut framebuffer = Framebuffer::new(10, 8);
    framebuffer.draw_line_no_depth(&line);
    let inside = (4 * 10 + 4) * 4;
    let outside = (4 * 10 + 3) * 4;
    assert_eq!(&framebuffer.pixels[inside..inside + 4], &[0, 255, 0, 255]);
    assert_eq!(&framebuffer.pixels[outside..outside + 4], &[0, 0, 0, 0]);
}

#[test]
fn clear_fills_every_pixel() {
    let mut fb = Framebuffer::new(4, 4);
    fb.clear(10, 20, 30, 255);
    assert!(!fb.has_non_uniform_content(10, 20, 30, 255));
    assert_eq!(&fb.pixels[0..4], &[10, 20, 30, 255]);
}

#[test]
fn field_scissor_rejects_opposite_parity_in_every_raster_path() {
    let odd_field = Some(ScissorRect {
        ulx: 0.0,
        uly: 0.0,
        lrx: 4.0,
        lry: 4.0,
        field: true,
        keep_odd: true,
    });
    let assert_rows = |framebuffer: &Framebuffer, painted: [u8; 4]| {
        for y in 0..4usize {
            for x in 0..4usize {
                let offset = (y * 4 + x) * 4;
                let expected = if y % 2 == 1 { painted } else { [0, 0, 0, 255] };
                assert_eq!(
                    &framebuffer.pixels[offset..offset + 4],
                    &expected,
                    "field scissor mismatch at ({x}, {y})"
                );
            }
        }
    };

    let fill = FillRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 3.0,
        lry: 3.0,
        fill_color: 0xffff_ffff,
        cycle_type: CycleType::Fill,
        scissor: odd_field,
        other_mode: OtherMode::default(),
        combiner: CombinerState::default(),
        blender: BlenderState::default(),
    };
    let mut fill_fb = Framebuffer::new(4, 4);
    fill_fb.clear(0, 0, 0, 255);
    fill_fb.draw_fill_rectangle(
        &fill,
        ColorImage {
            format: ColorImage::RGBA_FORMAT,
            size: ColorImage::BITS_16,
            width: 4,
            address: 0,
        },
    );
    assert_rows(&fill_fb, [255; 4]);

    let mut depth_fb = Framebuffer::new(4, 4);
    depth_fb.clear_depth_rectangle(&fill);
    for y in 0..4usize {
        for x in 0..4usize {
            let depth = depth_fb.depth[y * 4 + x];
            assert_eq!(
                depth.is_finite(),
                y % 2 == 1,
                "depth field scissor mismatch at ({x}, {y})"
            );
        }
    }

    let passthrough = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
    let mut combined = texture_rectangle(
        solid_texture([255, 0, 0, 255]),
        OtherMode::default(),
        repeated_state(passthrough, [0; 4], [0; 4]),
    );
    combined.lrx = 4.0;
    combined.lry = 4.0;
    combined.scissor = odd_field;
    let mut combined_fb = Framebuffer::new(4, 4);
    combined_fb.clear(0, 0, 0, 255);
    combined_fb.draw_texture_rectangle(&combined);
    assert_rows(&combined_fb, [255, 0, 0, 255]);

    let mut copy_texture = solid_texture([255, 0, 0, 255]);
    copy_texture.width = 4;
    copy_texture.height = 4;
    copy_texture.texels = std::rc::Rc::new([255, 0, 0, 255].repeat(16));
    let mut copy = texture_rectangle(
        copy_texture,
        OtherMode::from_raw(2 << 20, 0, 0),
        CombinerState::default(),
    );
    copy.lrx = 3.0;
    copy.lry = 3.0;
    copy.dsdx = 4 << 10;
    copy.scissor = odd_field;
    let mut copy_fb = Framebuffer::new(4, 4);
    copy_fb.clear(0, 0, 0, 255);
    copy_fb.draw_copy_texture_rectangle(&copy);
    assert_rows(&copy_fb, [255, 0, 0, 255]);

    let high = Triangle {
        v: [
            v(-10.0, -10.0, 255, 0, 0, 255),
            v(20.0, -10.0, 255, 0, 0, 255),
            v(-10.0, 20.0, 255, 0, 0, 255),
        ],
        scissor: odd_field,
        ..Triangle::default()
    };
    let mut high_fb = Framebuffer::new(4, 4);
    high_fb.clear(0, 0, 0, 255);
    high_fb.draw_triangle(&high);
    assert_rows(&high_fb, [255, 0, 0, 255]);

    let raw = RawRdpTriangle {
        edge: crate::gbi::RdpEdgeCoefficients {
            left_major: true,
            level: 0,
            tile: 0,
            yl: 16,
            ym: 8,
            yh: 0,
            xl: 4 << 16,
            dxldy: 0,
            xh: 0,
            dxhdy: 0,
            xm: 4 << 16,
            dxmdy: 0,
        },
        shade: None,
        texture_coefficients: None,
        z: None,
        texture: None,
        other_mode: OtherMode::default(),
        combiner: CombinerState {
            primitive: [255, 0, 0, 255],
            ..CombinerState::default()
        },
        blender: BlenderState::default(),
        scissor: odd_field,
    };
    let mut raw_fb = Framebuffer::new(4, 4);
    raw_fb.clear(0, 0, 0, 255);
    raw_fb.draw_raw_rdp_triangle(&raw);
    assert_rows(&raw_fb, [255, 0, 0, 255]);
}

#[test]
fn direct_fill_and_depth_entries_share_the_bypass_hazard_trap() {
    let rectangle = FillRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 0.0,
        lry: 0.0,
        fill_color: 0xffff_ffff,
        cycle_type: CycleType::Fill,
        scissor: None,
        other_mode: OtherMode::from_raw(3 << 20, 1 << 6, 0),
        combiner: CombinerState::default(),
        blender: BlenderState::default(),
    };
    let target = ColorImage {
        format: ColorImage::RGBA_FORMAT,
        size: ColorImage::BITS_32,
        width: 1,
        address: 0,
    };

    for depth_entry in [false, true] {
        let mut framebuffer = Framebuffer::new(1, 1);
        let before_pixels = framebuffer.pixels.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if depth_entry {
                framebuffer.clear_depth_rectangle(&rectangle);
            } else {
                framebuffer.draw_fill_rectangle(&rectangle, target);
            }
        }));
        let payload = result.expect_err("direct Fill entry must retain the loud hazard trap");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .expect("panic payload must be text");
        assert!(message.contains("unsafe IM_RD state"));
        assert_eq!(framebuffer.pixels, before_pixels);
        assert!(framebuffer.depth.iter().all(|value| value.is_infinite()));
        assert_eq!(framebuffer.color_layout(), ColorImageLayout::Rgba16);
    }
}

#[test]
fn primitive_depth_drives_texture_rectangle_compare_and_update() {
    let passthrough = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
    let other_mode =
        crate::gbi::OtherMode::from_raw(crate::gbi::OtherMode::default().raw_high(), 0x34, 0);
    let rectangle = texture_rectangle(
        solid_texture([255, 0, 0, 255]),
        other_mode,
        repeated_state(passthrough, [0; 4], [0; 4]),
    );
    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.depth[0] = 0x3ffff as f32;
    framebuffer.encoded_depth[0] = Some(crate::depth::EncodedDepth {
        visible: 0xfffc,
        hidden: 0,
    });
    framebuffer.set_primitive_depth(Some(PrimitiveDepth { z: 8, delta_z: 32 }));

    framebuffer.draw_texture_rectangle(&rectangle);

    let expected = crate::depth::pack(8 << 3, 32);
    assert_eq!(&framebuffer.pixels, &[255, 0, 0, 255]);
    assert_eq!(framebuffer.encoded_depth[0], Some(expected));
    assert_eq!(
        framebuffer.depth[0],
        crate::depth::unpack(expected).0 as f32
    );
}

#[test]
fn flipped_texture_rectangle_swaps_s_and_t_screen_axes() {
    let texture = crate::gbi::Texture {
        format: 0,
        size: 2,
        width: 2,
        height: 2,
        texels: std::rc::Rc::new(vec![
            255, 0, 0, 255, // top-left
            0, 255, 0, 255, // top-right
            0, 0, 255, 255, // bottom-left
            255, 255, 255, 255, // bottom-right
        ]),
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
    let passthrough = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
    let mut rectangle = texture_rectangle(
        texture,
        crate::gbi::OtherMode::default(),
        repeated_state(passthrough, [0; 4], [0; 4]),
    );
    rectangle.lrx = 2.0;
    rectangle.lry = 2.0;
    rectangle.flip = true;

    let mut framebuffer = Framebuffer::new(2, 2);
    framebuffer.draw_texture_rectangle(&rectangle);
    assert_eq!(
        framebuffer.pixels,
        vec![
            255, 0, 0, 255, // source (0,0)
            0, 0, 255, 255, // source (0,1)
            0, 255, 0, 255, // source (1,0)
            255, 255, 255, 255, // source (1,1)
        ]
    );
}

#[test]
fn flipped_copy_texture_rectangle_swaps_axes_with_copy_gradient_scaling() {
    let texture = crate::gbi::Texture {
        format: 0,
        size: 2,
        width: 2,
        height: 2,
        texels: std::rc::Rc::new(vec![
            255, 0, 0, 255, // top-left
            0, 255, 0, 255, // top-right
            0, 0, 255, 255, // bottom-left
            255, 255, 255, 255, // bottom-right
        ]),
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
    let mut rectangle = texture_rectangle(
        texture,
        crate::gbi::OtherMode::from_raw(2 << 20, 0, 0),
        CombinerState::default(),
    );
    rectangle.lrx = 1.0;
    rectangle.lry = 1.0;
    rectangle.dsdx = 4 << 10;
    rectangle.dtdy = 1 << 10;
    rectangle.flip = true;

    let mut framebuffer = Framebuffer::new(2, 2);
    framebuffer.draw_copy_texture_rectangle(&rectangle);
    assert_eq!(
        framebuffer.pixels,
        vec![
            255, 0, 0, 255, // source (0,0)
            0, 0, 255, 255, // source (0,1)
            0, 255, 0, 255, // source (1,0)
            255, 255, 255, 255, // source (1,1)
        ]
    );
}

#[test]
fn copy_clamp_bits_are_ignored_before_mask_wrap() {
    let texture = crate::gbi::Texture {
        format: 0,
        size: 2,
        width: 4,
        height: 1,
        texels: std::rc::Rc::new(
            (0..4u8)
                .flat_map(|value| [value, value, value, 255])
                .collect(),
        ),
        clamp_s: true,
        clamp_t: true,
        mirror_s: false,
        mirror_t: false,
        mask_s: 2,
        mask_t: 0,
        shift_s: 0,
        shift_t: 0,
        origin_s: 0.0,
        origin_t: 0.0,
        tmem: None,
        lod: None,
    };
    let mut rectangle = texture_rectangle(
        texture,
        OtherMode::from_raw(2 << 20, 0, 0),
        CombinerState::default(),
    );
    rectangle.lrx = 7.0;
    rectangle.lry = 0.0;
    rectangle.dsdx = 4 << 10;

    let mut framebuffer = Framebuffer::new(8, 1);
    framebuffer.draw_copy_texture_rectangle(&rectangle);
    assert_eq!(
        framebuffer
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>(),
        [0, 1, 2, 3, 0, 1, 2, 3]
    );
}

#[test]
fn rgba16_copy_alpha_compare_uses_alpha_bit_even_when_blend_threshold_is_zero() {
    let mut texture = solid_texture([0; 4]);
    texture.width = 2;
    texture.texels = std::rc::Rc::new(vec![
        255, 0, 0, 0, // RGBA16 alpha bit clear: write disabled
        0, 255, 0, 255, // RGBA16 alpha bit set: write enabled
    ]);
    for alpha_compare in [AlphaCompare::Threshold, AlphaCompare::Dither] {
        let low = match alpha_compare {
            AlphaCompare::Threshold => 1,
            AlphaCompare::Dither => 3,
            _ => unreachable!(),
        };
        let mut rectangle = texture_rectangle(
            texture.clone(),
            OtherMode::from_raw(2 << 20, low, 0),
            CombinerState::default(),
        );
        rectangle.lrx = 1.0;
        rectangle.lry = 0.0;
        rectangle.dsdx = 4 << 10;

        let mut framebuffer = Framebuffer::new(2, 1);
        framebuffer.clear(9, 8, 7, 255);
        framebuffer.draw_copy_texture_rectangle(&rectangle);

        assert_eq!(&framebuffer.pixels[0..4], &[9, 8, 7, 255]);
        assert_eq!(&framebuffer.pixels[4..8], &[0, 255, 0, 255]);
    }
}

#[test]
fn rgba16_copy_without_alpha_compare_writes_alpha_zero_texel() {
    let mut rectangle = texture_rectangle(
        solid_texture([255, 0, 0, 0]),
        OtherMode::from_raw(2 << 20, 0, 0),
        CombinerState::default(),
    );
    rectangle.lrx = 0.0;
    rectangle.lry = 0.0;
    rectangle.dsdx = 4 << 10;

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.clear(9, 8, 7, 255);
    framebuffer.draw_copy_texture_rectangle(&rectangle);

    assert_eq!(framebuffer.pixels, [255, 0, 0, 0]);
}

#[test]
fn two_cycle_texture_rectangle_combines_distinct_texel_tiles() {
    let first = texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0);
    let second = cycle(
        [
            ColorSource::Texel1,
            ColorSource::Combined,
            ColorSource::EnvironmentAlpha,
            ColorSource::Combined,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Combined,
        ],
    );
    let high = (crate::gbi::OtherMode::default().raw_high() & !(3 << 20)) | (1 << 20);
    let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
    let mut rectangle = texture_rectangle(
        solid_texture([100, 100, 100, 255]),
        other_mode,
        CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [first, second],
            },
            primitive: [0; 4],
            environment: [0, 0, 0, 128],
            min_lod_level: 0,
            prim_lod_fraction: 0,
            convert: crate::gbi::ConvertState::default(),
            key: crate::gbi::KeyState::default(),
        },
    );
    rectangle.texture1 = Some(solid_texture([200, 200, 200, 255]));

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_texture_rectangle(&rectangle);
    assert_eq!(framebuffer.pixels, vec![150, 150, 150, 255]);
}

#[test]
fn texture_rectangle_lod_selects_adjacent_mips_and_feeds_fraction() {
    let tile0 = solid_texture([255, 255, 255, 255]);
    let tile1 = solid_texture([0, 0, 0, 255]);
    let tile2 = solid_texture([200, 200, 200, 255]);
    let mut tiles: [Option<crate::gbi::Texture>; 8] = std::array::from_fn(|_| None);
    tiles[0] = Some(tile0.clone());
    tiles[1] = Some(tile1.clone());
    tiles[2] = Some(tile2);
    let base = tile0.with_lod_snapshot(tiles, 0, 2);

    let trilerp = cycle(
        [
            ColorSource::Texel1,
            ColorSource::Texel0,
            ColorSource::LodFraction,
            ColorSource::Texel0,
        ],
        [
            AlphaSource::Texel1,
            AlphaSource::Texel0,
            AlphaSource::LodFraction,
            AlphaSource::Texel0,
        ],
    );
    let pass = cycle(
        [
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Combined,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Combined,
        ],
    );
    let high = (crate::gbi::OtherMode::default().raw_high() & !((3 << 20) | (1 << 16)))
        | (1 << 20)
        | (1 << 16);
    let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
    let mut rectangle = texture_rectangle(
        base,
        other_mode,
        CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [trilerp, pass],
            },
            ..CombinerState::default()
        },
    );
    rectangle.texture1 = Some(tile1);
    rectangle.dsdx = (2.5 * 1024.0) as i16;
    rectangle.dtdy = 0;

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_texture_rectangle(&rectangle);
    assert_eq!(framebuffer.pixels, vec![50, 50, 50, 255]);
}

#[test]
fn high_level_triangle_uses_the_shared_lod_tile_and_fraction_path() {
    let tile0 = solid_texture([255, 255, 255, 255]);
    let tile1 = solid_texture([0, 0, 0, 255]);
    let tile2 = solid_texture([200, 200, 200, 255]);
    let mut tiles: [Option<crate::gbi::Texture>; 8] = std::array::from_fn(|_| None);
    tiles[0] = Some(tile0.clone());
    tiles[1] = Some(tile1);
    tiles[2] = Some(tile2);
    let texture = tile0.with_lod_snapshot(tiles, 0, 2);
    let trilerp = cycle(
        [
            ColorSource::Texel1,
            ColorSource::Texel0,
            ColorSource::LodFraction,
            ColorSource::Texel0,
        ],
        [
            AlphaSource::Texel1,
            AlphaSource::Texel0,
            AlphaSource::LodFraction,
            AlphaSource::Texel0,
        ],
    );
    let pass = cycle(
        [
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Combined,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Combined,
        ],
    );
    let high = (crate::gbi::OtherMode::default().raw_high() & !((3 << 20) | (1 << 16)))
        | (1 << 20)
        | (1 << 16);
    let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
    let textured = |x: f32, y: f32, s: f32, t: f32| Vertex {
        x,
        y,
        s,
        t,
        w: 1.0,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
        ..Vertex::default()
    };
    let triangle = Triangle {
        v: [
            textured(0.0, 0.0, 0.0, 0.0),
            textured(2.0, 0.0, 5.0, 0.0),
            textured(0.0, 2.0, 0.0, 5.0),
        ],
        texture: Some(texture),
        other_mode,
        combiner: CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [trilerp, pass],
            },
            ..CombinerState::default()
        },
        blender: BlenderState {
            cycle_count: 2,
            ..BlenderState::default()
        },
        ..Triangle::default()
    };

    let mut framebuffer = Framebuffer::new(2, 2);
    framebuffer.draw_triangle(&triangle);
    assert_eq!(&framebuffer.pixels[..4], &[50, 50, 50, 255]);
}

#[test]
fn raw_triangle_uses_the_shared_lod_tile_and_fraction_path() {
    let tile0 = solid_texture([255, 255, 255, 255]);
    let tile1 = solid_texture([0, 0, 0, 255]);
    let tile2 = solid_texture([200, 200, 200, 255]);
    let mut tiles: [Option<crate::gbi::Texture>; 8] = std::array::from_fn(|_| None);
    tiles[0] = Some(tile0.clone());
    tiles[1] = Some(tile1);
    tiles[2] = Some(tile2);
    let texture = tile0.with_lod_snapshot(tiles, 0, 2);
    let trilerp = cycle(
        [
            ColorSource::Texel1,
            ColorSource::Texel0,
            ColorSource::LodFraction,
            ColorSource::Texel0,
        ],
        [
            AlphaSource::Texel1,
            AlphaSource::Texel0,
            AlphaSource::LodFraction,
            AlphaSource::Texel0,
        ],
    );
    let pass = cycle(
        [
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Combined,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Combined,
        ],
    );
    let high = (crate::gbi::OtherMode::default().raw_high() & !((3 << 20) | (1 << 16)))
        | (1 << 20)
        | (1 << 16);
    let other_mode = crate::gbi::OtherMode::from_raw(high, 0, 0);
    let triangle = RawRdpTriangle {
        edge: crate::gbi::RdpEdgeCoefficients {
            left_major: false,
            level: 2,
            tile: 0,
            yl: 4,
            ym: 2,
            yh: 0,
            xl: 0,
            dxldy: 0,
            xh: 1 << 16,
            dxhdy: 0,
            xm: 0,
            dxmdy: 0,
        },
        shade: None,
        texture_coefficients: Some(crate::gbi::RdpTextureCoefficients {
            // W = 1024: tcdiv unity under the hardware persp scale
            // (S/W * 2^10 texels), so dstdx/dstdy below read directly
            // in texels per pixel.
            stw: [0, 0, 1024 << 16],
            dstdx: [(2.5 * 65536.0) as i32, 0, 0],
            dstde: [0; 3],
            dstdy: [0, (2.5 * 65536.0) as i32, 0],
        }),
        z: None,
        texture: Some(texture),
        other_mode,
        combiner: CombinerState {
            mode: crate::gbi::CombinerMode {
                cycles: [trilerp, pass],
            },
            ..CombinerState::default()
        },
        blender: BlenderState {
            cycle_count: 2,
            ..BlenderState::default()
        },
        scissor: None,
    };

    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_raw_rdp_triangle(&triangle);
    assert_eq!(framebuffer.pixels, vec![50, 50, 50, 255]);

    // G_TP_NONE variant of the same primitive: hardware `tcdiv_nopersp`
    // skips the divide entirely and the plane's INTEGER part is the
    // S10.5 texel coordinate, so the same 2.5-texel-per-pixel gradient
    // is spelled `2.5 * 2^21` in plane units and W is irrelevant.
    let mut nopersp = triangle;
    nopersp.other_mode =
        crate::gbi::OtherMode::from_raw(other_mode.raw_high() & !(1 << 19), 0, 0);
    nopersp.texture_coefficients = Some(crate::gbi::RdpTextureCoefficients {
        stw: [0, 0, 0],
        dstdx: [(2.5 * (1u32 << 21) as f64) as i32, 0, 0],
        dstde: [0; 3],
        dstdy: [0, (2.5 * (1u32 << 21) as f64) as i32, 0],
    });
    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.draw_raw_rdp_triangle(&nopersp);
    assert_eq!(framebuffer.pixels, vec![50, 50, 50, 255]);
}

#[test]
fn combiner_presets_select_decal_primitive_environment_and_shade_sources() {
    // Fail-against-bug: the old rasterizer always returned TEXEL0*SHADE,
    // so every assertion except MODULATE below produced the same wrong
    // color regardless of the decoded primitive/environment registers.
    let shade = [50, 100, 150, 220];
    let texel = [128, 64, 255, 180];

    let shade_only = cycle(
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
    );
    assert_eq!(
        evaluate_combiner(
            repeated_state(shade_only, [0; 4], [0; 4]),
            CycleType::OneCycle,
            false,
            CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
        ),
        shade
    );

    let decal = cycle(
        [
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Zero,
            ColorSource::Texel0,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Texel0,
        ],
    );
    assert_eq!(
        evaluate_combiner(
            repeated_state(decal, [0; 4], [0; 4]),
            CycleType::OneCycle,
            false,
            CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
        ),
        texel
    );

    let primitive_tint = cycle(
        [
            ColorSource::Texel0,
            ColorSource::Zero,
            ColorSource::Primitive,
            ColorSource::Zero,
        ],
        [
            AlphaSource::Texel0,
            AlphaSource::Zero,
            AlphaSource::Primitive,
            AlphaSource::Zero,
        ],
    );
    let primitive = [128, 255, 64, 128];
    assert_eq!(
        evaluate_combiner(
            repeated_state(primitive_tint, primitive, [0; 4]),
            CycleType::OneCycle,
            false,
            CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
        ),
        [64, 64, 64, 90]
    );

    // G_CC_BLENDI: (ENVIRONMENT - SHADE) * TEXEL0 + SHADE.
    let env_blend = cycle(
        [
            ColorSource::Environment,
            ColorSource::Shade,
            ColorSource::Texel0,
            ColorSource::Shade,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Shade,
        ],
    );
    assert_eq!(
        evaluate_combiner(
            repeated_state(env_blend, [0; 4], [250, 200, 100, 255]),
            CycleType::OneCycle,
            false,
            CombinerPixel::new(0.0, shade, texel, texel, NoiseSample::ZERO),
        ),
        [150, 125, 100, 220]
    );
}

#[test]
fn combiner_second_cycle_consumes_first_cycle_combined_result() {
    // Cycle 0: TEXEL0*SHADE. Cycle 1: COMBINED*PRIMITIVE. This fails if
    // only the second programmed tuple is evaluated or COMBINED is not
    // carried between cycles.
    let first = cycle(
        [
            ColorSource::Texel0,
            ColorSource::Zero,
            ColorSource::Shade,
            ColorSource::Zero,
        ],
        [
            AlphaSource::Texel0,
            AlphaSource::Zero,
            AlphaSource::Shade,
            AlphaSource::Zero,
        ],
    );
    let second = cycle(
        [
            ColorSource::Combined,
            ColorSource::Zero,
            ColorSource::Primitive,
            ColorSource::Zero,
        ],
        [
            AlphaSource::Combined,
            AlphaSource::Zero,
            AlphaSource::Primitive,
            AlphaSource::Zero,
        ],
    );
    let state = CombinerState {
        mode: crate::gbi::CombinerMode {
            cycles: [first, second],
        },
        primitive: [128; 4],
        environment: [0; 4],
        min_lod_level: 0,
        prim_lod_fraction: 0,
        convert: crate::gbi::ConvertState::default(),
        key: crate::gbi::KeyState::default(),
    };
    assert_eq!(
        evaluate_combiner(
            state,
            CycleType::TwoCycle,
            false,
            CombinerPixel::new(0.0, [128; 4], [200; 4], [200; 4], NoiseSample::ZERO),
        ),
        [50; 4]
    );
}

#[test]
fn conversion_k4_and_k5_feed_the_color_combiner() {
    // Public Set Convert stage two: (R' - K4) * K5 + R'. K4 is the
    // 8-bit offset and K5 is its 8-bit fractional scale.
    let conversion = cycle(
        [
            ColorSource::Texel0,
            ColorSource::K4,
            ColorSource::K5,
            ColorSource::Texel0,
        ],
        [
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Zero,
            AlphaSource::Texel0,
        ],
    );
    let state = CombinerState {
        mode: crate::gbi::CombinerMode {
            cycles: [conversion; 2],
        },
        ..CombinerState::default()
    };
    assert_eq!(
        evaluate_combiner(
            state,
            CycleType::OneCycle,
            false,
            CombinerPixel::new(0.0, [0; 4], [100, 150, 200, 255], [0; 4], NoiseSample::ZERO,),
        ),
        [98, 156, 214, 255]
    );
}

#[test]
fn chroma_key_center_scale_and_width_drive_alpha_fixup() {
    let key_cycle = cycle(
        [
            ColorSource::Texel0,
            ColorSource::KeyCenter,
            ColorSource::KeyScale,
            ColorSource::Zero,
        ],
        [AlphaSource::Zero; 4],
    );
    let state = CombinerState {
        mode: crate::gbi::CombinerMode {
            cycles: [key_cycle; 2],
        },
        key: crate::gbi::KeyState {
            center: [100; 3],
            scale: [255; 3],
            width: [0x100; 3],
        },
        ..CombinerState::default()
    };

    assert_eq!(
        evaluate_combiner(
            state,
            CycleType::OneCycle,
            true,
            CombinerPixel::new(0.0, [0; 4], [100, 100, 100, 255], [0; 4], NoiseSample::ZERO,),
        ),
        [0, 0, 0, 255]
    );
    assert_eq!(
        evaluate_combiner(
            state,
            CycleType::OneCycle,
            true,
            CombinerPixel::new(0.0, [0; 4], [200, 100, 100, 255], [0; 4], NoiseSample::ZERO,),
        ),
        [100, 0, 0, 155]
    );
}

#[test]
fn triangle_paints_interior_pixels_and_leaves_exterior_clear() {
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    let tri = Triangle {
        v: [
            v(2.0, 2.0, 255, 0, 0, 255),
            v(12.0, 2.0, 255, 0, 0, 255),
            v(7.0, 12.0, 255, 0, 0, 255),
        ],
        ..Default::default()
    };
    fb.draw_triangle(&tri);
    assert!(fb.has_non_uniform_content(0, 0, 0, 255));
    // Centroid should be red.
    let cx = 7u32;
    let cy = 6u32;
    let idx = (cy * fb.width + cx) as usize * 4;
    assert_eq!(&fb.pixels[idx..idx + 4], &[255, 0, 0, 255]);
    // A far corner should remain untouched (still clear color).
    let idx0 = 0usize;
    assert_eq!(&fb.pixels[idx0..idx0 + 4], &[0, 0, 0, 255]);
}

#[test]
fn high_level_triangle_uses_the_public_eight_sample_coverage_mask() {
    let mut fb = Framebuffer::new(1, 1);
    fb.clear(0, 0, 0, 255);
    let tri = Triangle {
        // The right edge cuts pixel zero at x=1/2. Exactly the four
        // checkerboard samples at x=1/8 or 3/8 are inside.
        v: [
            v(-10.0, -10.0, 255, 0, 0, 255),
            v(0.5, -10.0, 255, 0, 0, 255),
            v(0.5, 10.0, 255, 0, 0, 255),
        ],
        ..Default::default()
    };

    let area = edge(tri.v[0], tri.v[1], tri.v[2]);
    let mask = triangle_pixel_coverage(tri.v, area, ScissorRect::framebuffer(1, 1), 0, 0);
    assert_eq!(mask, CoverageMask(0x55));
    assert_eq!(mask.coverage(), Coverage::new(4));

    fb.draw_triangle(&tri);

    assert_eq!(fb.coverage[0], Coverage::new(4));
    assert_eq!(&fb.pixels[..4], &[255, 0, 0, 255]);
}

#[test]
fn high_level_partial_coverage_retains_the_exact_covered_sample_identity() {
    let tri = [
        Vertex {
            x: 0.0,
            y: 0.0,
            ..Vertex::default()
        },
        Vertex {
            x: 0.4,
            y: 0.0,
            ..Vertex::default()
        },
        Vertex {
            x: 0.0,
            y: 0.4,
            ..Vertex::default()
        },
    ];
    let mask = triangle_pixel_coverage(
        tri,
        edge(tri[0], tri[1], tri[2]),
        ScissorRect::framebuffer(1, 1),
        0,
        0,
    );

    assert_eq!(mask, CoverageMask(0x01));
    assert_eq!(mask.coverage(), Coverage::new(1));
    assert!(mask.contains(0));
    assert!((1..COVERAGE_SAMPLES.len()).all(|index| !mask.contains(index)));

    let covered = Vertex {
        x: COVERAGE_SAMPLES[0].0 as f32 / 8.0,
        y: COVERAGE_SAMPLES[0].1 as f32 / 8.0,
        ..Vertex::default()
    };
    let center = Vertex {
        x: 0.5,
        y: 0.5,
        ..Vertex::default()
    };
    assert!(edge(tri[1], tri[2], covered) > 0.0);
    assert!(edge(tri[1], tri[2], center) < 0.0);
}

#[test]
fn setscissor_bounds_triangle_writes_to_exclusive_rect() {
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    let tri = Triangle {
        v: [
            v(0.0, 0.0, 255, 0, 0, 255),
            v(16.0, 0.0, 255, 0, 0, 255),
            v(0.0, 16.0, 255, 0, 0, 255),
        ],
        scissor: Some(ScissorRect {
            ulx: 4.25,
            uly: 3.75,
            lrx: 9.25,
            lry: 8.75,
            field: false,
            keep_odd: false,
        }),
        ..Default::default()
    };

    fb.draw_triangle(&tri);

    let inside = (5usize * 16 + 5) * 4;
    assert_eq!(&fb.pixels[inside..inside + 4], &[255, 0, 0, 255]);
    for y in 0..16usize {
        for x in 0..16usize {
            // The eight-sample mask reaches x=9 and y=3 even though
            // those pixel centers lie outside the quarter-pixel scissor.
            if !(4..10).contains(&x) || !(3..9).contains(&y) {
                let i = (y * 16 + x) * 4;
                assert_eq!(
                    &fb.pixels[i..i + 4],
                    &[0, 0, 0, 255],
                    "triangle wrote outside exclusive scissor at ({x},{y})"
                );
            }
        }
    }
}

#[test]
fn textured_triangle_modulates_texel_by_shade() {
    use crate::gbi::Texture;
    // 1×1 white texture: modulate leaves the shade color unchanged.
    let white = Texture {
        format: 0,
        size: 2,
        width: 1,
        height: 1,
        texels: std::rc::Rc::new(vec![255, 255, 255, 255]),
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
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    // Green-shaded triangle, all S/T = 0 (samples the one white texel).
    let mut tri = Triangle {
        v: [
            v(2.0, 2.0, 0, 200, 0, 255),
            v(12.0, 2.0, 0, 200, 0, 255),
            v(7.0, 12.0, 0, 200, 0, 255),
        ],
        ..Default::default()
    };
    tri.texture = Some(white);
    fb.draw_triangle(&tri);
    let idx = (6u32 * fb.width + 7u32) as usize * 4;
    // white(255) * shade(200) / 255 == 200: texture didn't tint the shade.
    assert_eq!(&fb.pixels[idx..idx + 4], &[0, 200, 0, 255]);
}

#[test]
fn textured_triangle_paints_texel_color() {
    use crate::gbi::Texture;
    // 1×1 red texture under a white shade -> red pixel (modulate).
    let red = Texture {
        format: 0,
        size: 2,
        width: 1,
        height: 1,
        texels: std::rc::Rc::new(vec![255, 0, 0, 255]),
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
    let mut fb = Framebuffer::new(16, 16);
    fb.clear(0, 0, 0, 255);
    let mut tri = Triangle {
        v: [
            v(2.0, 2.0, 255, 255, 255, 255),
            v(12.0, 2.0, 255, 255, 255, 255),
            v(7.0, 12.0, 255, 255, 255, 255),
        ],
        ..Default::default()
    };
    tri.texture = Some(red);
    fb.draw_triangle(&tri);
    let idx = (6u32 * fb.width + 7u32) as usize * 4;
    assert_eq!(&fb.pixels[idx..idx + 4], &[255, 0, 0, 255]);
}

#[test]
fn perspective_correct_st_uses_reciprocal_clip_w() {
    use crate::gbi::Texture;

    let mut texels = vec![0u8; 4 * 4 * 4];
    texels[0..4].copy_from_slice(&[255, 0, 0, 255]);
    let green = 20usize; // texel (1,1) in a 4-wide RGBA8888 texture
    texels[green..green + 4].copy_from_slice(&[0, 255, 0, 255]);
    let texture = Texture {
        format: 0,
        size: 2,
        width: 4,
        height: 4,
        texels: std::rc::Rc::new(texels),
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
    let textured = |x: f32, y: f32, s: f32, t: f32, w: f32| Vertex {
        x,
        y,
        s,
        t,
        w,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
        ..Default::default()
    };
    let tri = Triangle {
        v: [
            textured(0.0, 0.0, 0.0, 0.0, 1.0),
            textured(8.0, 0.0, 3.0, 3.0, 4.0),
            textured(0.0, 8.0, 0.0, 0.0, 1.0),
        ],
        texture: Some(texture),
        ..Default::default()
    };

    let mut fb = Framebuffer::new(8, 8);
    fb.clear(0, 0, 0, 255);
    fb.draw_triangle(&tri);

    let sample = 4usize * 4;
    assert_eq!(
        &fb.pixels[sample..sample + 4],
        &[255, 0, 0, 255],
        "at pixel center (4.5,0.5), reciprocal-w interpolation gives S/T≈0.73 \
         (red texel 0,0); screen-linear S/T≈1.69 incorrectly samples green 1,1"
    );
}

#[test]
fn dither_alpha_compare_produces_reproducible_stipple_without_ordered_bayer() {
    let tri = Triangle {
        v: [
            v(0.0, 0.0, 255, 255, 255, 128),
            v(16.0, 0.0, 255, 255, 255, 128),
            v(0.0, 16.0, 255, 255, 255, 128),
        ],
        other_mode: crate::gbi::OtherMode::from_raw(0, 3, 0),
        ..Default::default()
    };
    let render = || {
        let mut fb = Framebuffer::new(16, 16);
        fb.set_noise_seed(0x1234);
        fb.clear(0, 0, 0, 255);
        fb.draw_triangle(&tri);
        fb.pixels
    };
    let first = render();
    let second = render();
    assert_eq!(first, second);
    let written = first
        .chunks_exact(4)
        .filter(|px| px[..3] == [255, 255, 255])
        .count();
    assert!(
        written > 0 && written < 120,
        "half-alpha dither must stipple the covered triangle"
    );

    let mut advancing = Framebuffer::new(16, 16);
    advancing.set_noise_seed(0x1234);
    advancing.clear(0, 0, 0, 255);
    advancing.draw_triangle(&tri);
    let first_frame = advancing.pixels.clone();
    advancing.clear(0, 0, 0, 255);
    advancing.draw_triangle(&tri);
    assert_ne!(
        advancing.pixels, first_frame,
        "the noise stream must advance rather than freeze on screen coordinates"
    );
}

#[test]
fn ordered_rgb_dither_tables_are_screen_registered() {
    let magic = [[0, 6, 1, 7], [4, 2, 5, 3], [3, 5, 2, 4], [7, 1, 6, 0]];
    let bayer = [[0, 4, 1, 5], [6, 2, 7, 3], [1, 5, 0, 4], [7, 3, 6, 2]];
    for y in 0..8 {
        for x in 0..8 {
            assert_eq!(
                ordered_rgb_dither_threshold(RgbDither::MagicSquare, x, y),
                magic[y as usize & 3][x as usize & 3]
            );
            assert_eq!(
                ordered_rgb_dither_threshold(RgbDither::Bayer, x, y),
                bayer[y as usize & 3][x as usize & 3]
            );
        }
    }
    assert_eq!(ordered_rgb_dither_threshold(RgbDither::Bayer, -1, -1), 2);
}

#[test]
fn ordered_rgb_dither_applies_before_color_image_format_write() {
    assert_eq!(
        apply_rgb_dither(
            [7, 6, 1, 93],
            RgbDither::MagicSquare,
            0,
            0,
            NoiseSample::ZERO,
        ),
        [8, 8, 8, 93]
    );
    assert_eq!(
        apply_rgb_dither(
            [7, 6, 1, 93],
            RgbDither::MagicSquare,
            3,
            0,
            NoiseSample::ZERO,
        ),
        [7, 6, 1, 93]
    );
    assert_eq!(
        apply_rgb_dither(
            [255, 254, 253, 93],
            RgbDither::Bayer,
            0,
            0,
            NoiseSample::ZERO,
        ),
        [255, 255, 255, 93]
    );
}

#[test]
fn rgb_dither_selector_sweep_matches_every_public_threshold() {
    for mode in [
        RgbDither::MagicSquare,
        RgbDither::Bayer,
        RgbDither::Noise,
        RgbDither::Disabled,
    ] {
        for y in -4..8 {
            for x in -4..8 {
                for noise_threshold in 0..=7 {
                    let noise = NoiseSample(noise_threshold);
                    let threshold = match mode {
                        RgbDither::MagicSquare | RgbDither::Bayer => {
                            ordered_rgb_dither_threshold(mode, x, y)
                        }
                        RgbDither::Noise => noise_threshold,
                        RgbDither::Disabled => 7,
                    };
                    for component in 0..=u8::MAX {
                        let expected =
                            if mode == RgbDither::Disabled || component & 7 <= threshold {
                                component
                            } else {
                                (component & !7).saturating_add(8)
                            };
                        let actual = apply_rgb_dither([component; 4], mode, x, y, noise);
                        assert_eq!(actual[..3], [expected; 3]);
                        assert_eq!(actual[3], component, "RGB dither must preserve alpha");
                    }
                }
            }
        }
    }
}
