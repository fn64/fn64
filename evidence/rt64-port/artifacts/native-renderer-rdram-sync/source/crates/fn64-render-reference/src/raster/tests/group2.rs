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
fn alpha_pattern_uses_rgb_pattern_fallback_and_inverse_before_blending() {
    assert_eq!(
        apply_alpha_dither(
            93,
            AlphaDither::Pattern,
            RgbDither::MagicSquare,
            0,
            0,
            NoiseSample::ZERO,
        ),
        99
    );
    assert_eq!(
        apply_alpha_dither(
            93,
            AlphaDither::InversePattern,
            RgbDither::MagicSquare,
            0,
            0,
            NoiseSample::ZERO,
        ),
        90
    );
    assert_eq!(
        apply_alpha_dither(
            93,
            AlphaDither::Pattern,
            RgbDither::Disabled,
            0,
            0,
            NoiseSample::ZERO,
        ),
        99,
        "disabled RGB dither must route the standard Bayer alpha pattern"
    );
    assert_eq!(
        apply_alpha_dither(
            93,
            AlphaDither::Pattern,
            RgbDither::Noise,
            3,
            0,
            NoiseSample::ZERO,
        ),
        90,
        "RGB noise must route the magic-square alpha fallback"
    );
}

#[test]
fn noise_selectors_share_one_fragment_sample_at_their_documented_widths() {
    let noise = NoiseSample(5);
    assert_eq!(
        apply_rgb_dither([6, 5, 4, 255], RgbDither::Noise, 99, 99, noise),
        [8, 5, 4, 255]
    );
    assert_eq!(
        apply_alpha_dither(6, AlphaDither::Noise, RgbDither::Disabled, 99, 99, noise),
        8
    );
    assert!(alpha_compare_value(AlphaCompare::Dither, 6, 0, noise));
    assert!(!alpha_compare_value(AlphaCompare::Dither, 4, 0, noise));
    assert!(!alpha_compare_value(
        AlphaCompare::Dither,
        0,
        0,
        NoiseSample(0)
    ));
    assert!(alpha_compare_value(
        AlphaCompare::Dither,
        255,
        0,
        NoiseSample(255)
    ));
}

#[test]
fn deterministic_noise_policy_is_seeded_reproducible_and_temporally_advancing() {
    let mut first = NoiseState::default();
    let mut second = NoiseState::default();
    let a: Vec<_> = (0..64).map(|_| first.next_sample()).collect();
    let b: Vec<_> = (0..64).map(|_| second.next_sample()).collect();
    assert_eq!(a, b);
    assert!(a.windows(2).any(|pair| pair[0] != pair[1]));
    assert_ne!(first.next_sample(), a[0]);

    second.reseed(7);
    assert_ne!(second.next_sample(), a[0]);
}

#[test]
fn degenerate_triangle_paints_nothing() {
    let mut fb = Framebuffer::new(8, 8);
    fb.clear(1, 2, 3, 4);
    let tri = Triangle {
        v: [
            v(1.0, 1.0, 9, 9, 9, 9),
            v(1.0, 1.0, 9, 9, 9, 9),
            v(1.0, 1.0, 9, 9, 9, 9),
        ],
        ..Default::default()
    };
    fb.draw_triangle(&tri);
    assert!(!fb.has_non_uniform_content(1, 2, 3, 4));
}

#[test]
fn programmed_depth_compare_and_update_bits_are_independent() {
    let draw = |fb: &mut Framebuffer, z: f32, rgba: [u8; 4], depth: DepthControl| {
        fb.coverage[0] = Coverage::new(1);
        fb.set_depth_controlled_blended(
            0,
            0,
            DepthFragment {
                z,
                delta_z: 0,
                encoded_depth: None,
                coverage: Coverage::new(1),
                rgba,
                shade_alpha: 255,
                noise: NoiseSample::ZERO,
            },
            BlenderState::default(),
            depth,
            OtherMode::default(),
        )
    };

    let mut disabled = Framebuffer::new(1, 1);
    disabled.depth[0] = 5.0;
    assert!(draw(
        &mut disabled,
        8.0,
        [8, 0, 0, 255],
        DepthControl::DISABLED
    ));
    assert_eq!(disabled.depth[0], 5.0);

    let mut update_only = Framebuffer::new(1, 1);
    update_only.depth[0] = 5.0;
    assert!(draw(
        &mut update_only,
        8.0,
        [8, 0, 0, 255],
        DepthControl {
            compare: false,
            update: true,
            ..DepthControl::DISABLED
        }
    ));
    assert_eq!(update_only.depth[0], 8.0);

    let mut compare_only = Framebuffer::new(1, 1);
    compare_only.depth[0] = 5.0;
    assert!(draw(
        &mut compare_only,
        2.0,
        [0, 2, 0, 255],
        DepthControl {
            compare: true,
            update: false,
            ..DepthControl::DISABLED
        }
    ));
    assert_eq!(compare_only.depth[0], 5.0);
    assert!(!draw(
        &mut compare_only,
        8.0,
        [8, 0, 0, 255],
        DepthControl {
            compare: true,
            update: false,
            ..DepthControl::DISABLED
        }
    ));
    assert_eq!(&compare_only.pixels[..4], &[0, 2, 0, 255]);
}

#[test]
fn programmed_z_modes_distinguish_front_correlated_and_behind_fragments() {
    let draw = |mode: crate::gbi::DepthMode, z: u32| {
        let mut framebuffer = Framebuffer::new(1, 1);
        let memory = crate::depth::pack(128, 8);
        framebuffer.depth[0] = crate::depth::unpack(memory).0 as f32;
        framebuffer.encoded_depth[0] = Some(memory);
        framebuffer.coverage[0] = Coverage::new(1);
        let wrote = framebuffer.set_depth_controlled_blended(
            0,
            0,
            DepthFragment {
                z: z as f32,
                delta_z: 4,
                encoded_depth: Some(crate::depth::pack(z, 4)),
                coverage: Coverage::new(1),
                rgba: [255, 0, 0, 255],
                shade_alpha: 255,
                noise: NoiseSample::ZERO,
            },
            BlenderState::default(),
            DepthControl {
                compare: true,
                update: false,
                mode,
            },
            OtherMode::default(),
        );
        assert_eq!(framebuffer.depth[0], 128.0, "compare-only changed Z");
        wrote
    };

    use crate::gbi::DepthMode;
    let clearly_front = 119;
    let correlated_far_side = 136;
    let clearly_behind = 137;

    assert!(draw(DepthMode::Opaque, clearly_front));
    assert!(draw(DepthMode::Opaque, correlated_far_side));
    assert!(!draw(DepthMode::Opaque, clearly_behind));

    assert!(draw(DepthMode::Interpenetrating, clearly_front));
    assert!(draw(DepthMode::Interpenetrating, correlated_far_side));
    assert!(!draw(DepthMode::Interpenetrating, clearly_behind));

    assert!(draw(DepthMode::Translucent, clearly_front));
    assert!(!draw(DepthMode::Translucent, correlated_far_side));

    assert!(!draw(DepthMode::Decal, clearly_front));
    assert!(draw(DepthMode::Decal, correlated_far_side));
    assert!(!draw(DepthMode::Decal, clearly_behind));
}

#[test]
fn depth_mode_and_wrap_routing_exhaustively_preserves_supported_relations() {
    use crate::gbi::DepthMode;

    for mode in [
        DepthMode::Opaque,
        DepthMode::Interpenetrating,
        DepthMode::Translucent,
        DepthMode::Decal,
    ] {
        for coverage_wraps in [false, true] {
            for relation_bits in 0u8..16 {
                let relations = crate::depth::DepthRelations {
                    memory_is_max: relation_bits & 1 != 0,
                    farther: relation_bits & 2 != 0,
                    nearer: relation_bits & 4 != 0,
                    in_front: relation_bits & 8 != 0,
                };
                let actual = depth_coverage_decision(mode, relations, coverage_wraps);
                let expected = if mode == DepthMode::Interpenetrating && coverage_wraps {
                    DepthCoverageDecision::UnsupportedInterpenetratingCoverageAdjustment
                } else {
                    let passes = if mode == DepthMode::Opaque && coverage_wraps {
                        relations.in_front
                    } else {
                        crate::depth::mode_passes(mode, relations)
                    };
                    if passes {
                        DepthCoverageDecision::Pass
                    } else {
                        DepthCoverageDecision::Reject
                    }
                };
                assert_eq!(
                    actual, expected,
                    "depth routing differs for {mode:?}, wraps={coverage_wraps}, \
                     relations={relations:?}"
                );
            }
        }
    }
}

#[test]
fn interpenetrating_coverage_wrap_traps_before_silently_using_opaque_routing() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    let mut framebuffer = Framebuffer::new(1, 1);
    let memory = crate::depth::pack(128, 8);
    framebuffer.depth[0] = crate::depth::unpack(memory).0 as f32;
    framebuffer.encoded_depth[0] = Some(memory);
    framebuffer.coverage[0] = Coverage::FULL;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        framebuffer.set_depth_controlled_blended(
            0,
            0,
            DepthFragment {
                z: 119.0,
                delta_z: 4,
                encoded_depth: Some(crate::depth::pack(119, 4)),
                coverage: Coverage::new(1),
                rgba: [255, 0, 0, 255],
                shade_alpha: 255,
                noise: NoiseSample::ZERO,
            },
            BlenderState::default(),
            DepthControl {
                compare: true,
                update: false,
                mode: crate::gbi::DepthMode::Interpenetrating,
            },
            OtherMode::from_raw(0, 0x0150, 0),
        );
    }))
    .expect_err("wrapping ZMODE_INTER must trap before rendering");
    let panic = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("unsupported raster panic payload must be text");
    assert!(panic.contains(
        "ZMODE_INTER coverage wrap requires unsupported interpenetration coverage adjustment"
    ));
    assert!(panic.contains("pixel_coverage=1 memory_coverage=8"));

    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].subsystem,
        fn64_runtime::UnsupportedSubsystem::Render
    );
    assert_eq!(
        events[0].operation,
        "render.reference.raster.interpenetration-coverage-adjustment"
    );
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::LoudTrap
    );
    assert!(events[0]
        .context
        .contains("pixel_coverage=1 memory_coverage=8"));
}

#[test]
fn raw_coverage_uses_the_public_eight_sample_checkerboard_mask() {
    let vertical_strip = |left: f32, right: f32| crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 4,
        ym: 2,
        yh: 0,
        xl: (left * 65536.0) as i32,
        dxldy: 0,
        xh: (right * 65536.0) as i32,
        dxhdy: 0,
        xm: (left * 65536.0) as i32,
        dxmdy: 0,
    };
    let scissor = ScissorRect {
        ulx: 0.0,
        uly: 0.0,
        lrx: 1.0,
        lry: 1.0,
        field: false,
        keep_odd: false,
    };
    assert_eq!(
        raw_pixel_coverage(vertical_strip(0.0, 1.0), scissor, 0, 0),
        CoverageMask(0xff)
    );
    let left = raw_pixel_coverage(vertical_strip(0.0, 0.5), scissor, 0, 0);
    let right = raw_pixel_coverage(vertical_strip(0.5, 1.0), scissor, 0, 0);
    assert_eq!(left, CoverageMask(0x55));
    assert_eq!(right, CoverageMask(0xaa));
    assert_eq!(left.0 | right.0, u8::MAX);
    assert_eq!(left.0 & right.0, 0);

    let top_half = crate::gbi::RdpEdgeCoefficients {
        yl: 2,
        ym: 1,
        ..vertical_strip(0.0, 1.0)
    };
    let bottom_half = crate::gbi::RdpEdgeCoefficients {
        yh: 2,
        ym: 3,
        ..vertical_strip(0.0, 1.0)
    };
    let top = raw_pixel_coverage(top_half, scissor, 0, 0);
    let bottom = raw_pixel_coverage(bottom_half, scissor, 0, 0);
    assert_eq!(top, CoverageMask(0x0f));
    assert_eq!(bottom, CoverageMask(0xf0));
    assert_eq!(top.0 | bottom.0, u8::MAX);
    assert_eq!(top.0 & bottom.0, 0);
}

#[test]
fn raw_coverage_axis_aligned_boundaries_exhaustively_preserve_sample_identity() {
    let full_scissor = ScissorRect::framebuffer(1, 1);
    let edge = |left_eighth: i32, right_eighth: i32, top_quarter: i16, bottom_quarter: i16| {
        crate::gbi::RdpEdgeCoefficients {
            left_major: false,
            level: 0,
            tile: 0,
            yl: bottom_quarter,
            ym: top_quarter,
            yh: top_quarter,
            xl: left_eighth * (Q16_ONE as i32 / 8),
            dxldy: 0,
            xh: right_eighth * (Q16_ONE as i32 / 8),
            dxhdy: 0,
            xm: left_eighth * (Q16_ONE as i32 / 8),
            dxmdy: 0,
        }
    };

    for top_quarter in 0..=4 {
        for bottom_quarter in top_quarter..=4 {
            for left_eighth in 0..=8 {
                for right_eighth in left_eighth..=8 {
                    let actual = raw_pixel_coverage(
                        edge(left_eighth, right_eighth, top_quarter, bottom_quarter),
                        full_scissor,
                        0,
                        0,
                    );
                    let expected = CoverageMask::from_samples(|sample_x, sample_y| {
                        sample_x >= left_eighth
                            && sample_x < right_eighth
                            && sample_y >= i32::from(top_quarter) * 2
                            && sample_y < i32::from(bottom_quarter) * 2
                    });
                    assert_eq!(
                        actual, expected,
                        "raw coverage identity differs for x [{left_eighth}/8, {right_eighth}/8), y [{top_quarter}/4, {bottom_quarter}/4)"
                    );
                }
            }
        }
    }
}

#[test]
fn raw_coverage_q16_lsb_sweep_preserves_every_checkerboard_boundary() {
    let edge = |left_q16: i32, right_q16: i32| crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 4,
        ym: 0,
        yh: 0,
        xl: left_q16,
        dxldy: 0,
        xh: right_q16,
        dxhdy: 0,
        xm: left_q16,
        dxmdy: 0,
    };
    let scissor = ScissorRect::framebuffer(1, 1);

    for (sample_index, &(x_eighth, _)) in COVERAGE_SAMPLES.iter().enumerate() {
        let sample_q16 = x_eighth * (Q16_ONE as i32 / 8);
        for delta_lsb in -1..=1 {
            let left = raw_pixel_coverage(
                edge(sample_q16 + delta_lsb, 2 * Q16_ONE as i32),
                scissor,
                0,
                0,
            );
            assert_eq!(
                left.contains(sample_index),
                delta_lsb <= 0,
                "left-inclusive edge differs for sample {sample_index} at {delta_lsb:+} Q16 LSB"
            );

            let right = raw_pixel_coverage(
                edge(-(Q16_ONE as i32), sample_q16 + delta_lsb),
                scissor,
                0,
                0,
            );
            assert_eq!(
                right.contains(sample_index),
                delta_lsb > 0,
                "right-exclusive edge differs for sample {sample_index} at {delta_lsb:+} Q16 LSB"
            );
        }
    }
}

#[test]
fn raw_coverage_scissor_boundaries_exhaustively_preserve_sample_identity() {
    let full_pixel = crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 4,
        ym: 0,
        yh: 0,
        xl: 0,
        dxldy: 0,
        xh: Q16_ONE as i32,
        dxhdy: 0,
        xm: 0,
        dxmdy: 0,
    };
    for top_quarter in 0..=4 {
        for bottom_quarter in top_quarter..=4 {
            for left_quarter in 0..=4 {
                for right_quarter in left_quarter..=4 {
                    let scissor = ScissorRect {
                        ulx: left_quarter as f32 / 4.0,
                        uly: top_quarter as f32 / 4.0,
                        lrx: right_quarter as f32 / 4.0,
                        lry: bottom_quarter as f32 / 4.0,
                        field: false,
                        keep_odd: false,
                    };
                    let actual = raw_pixel_coverage(full_pixel, scissor, 0, 0);
                    let expected = CoverageMask::from_samples(|sample_x, sample_y| {
                        sample_x >= left_quarter * 2
                            && sample_x < right_quarter * 2
                            && sample_y >= top_quarter * 2
                            && sample_y < bottom_quarter * 2
                    });
                    assert_eq!(
                        actual, expected,
                        "raw scissor identity differs for x [{left_quarter}/4, {right_quarter}/4), y [{top_quarter}/4, {bottom_quarter}/4)"
                    );
                }
            }
        }
    }
}

#[test]
fn high_level_shared_edge_assigns_each_checkerboard_sample_once() {
    let vertex = |x, y| Vertex {
        x,
        y,
        ..Vertex::default()
    };
    let upper_right = [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(1.0, 1.0)];
    let lower_left = [vertex(0.0, 0.0), vertex(1.0, 1.0), vertex(0.0, 1.0)];
    let scissor = ScissorRect::framebuffer(1, 1);
    let coverage = |vertices: [Vertex; 3]| {
        triangle_pixel_coverage(
            vertices,
            edge(vertices[0], vertices[1], vertices[2]),
            scissor,
            0,
            0,
        )
    };

    let first = coverage(upper_right);
    let second = coverage(lower_left);
    assert_eq!(first, CoverageMask(0xaf));
    assert_eq!(second, CoverageMask(0x50));
    assert_eq!(first.0 | second.0, u8::MAX);
    assert_eq!(first.0 & second.0, 0);
    assert_eq!(
        first.coverage().count() + second.coverage().count(),
        Coverage::FULL.count()
    );
    assert_eq!(first.coverage(), Coverage::new(6));
    assert_eq!(second.coverage(), Coverage::new(2));

    let reversed = |[a, b, c]: [Vertex; 3]| [a, c, b];
    assert_eq!(coverage(reversed(upper_right)), first);
    assert_eq!(coverage(reversed(lower_left)), second);
}

#[test]
fn covered_attribute_sample_exhausts_every_nonzero_mask() {
    for bits in 1u16..=u16::from(u8::MAX) {
        let mask = CoverageMask(bits as u8);
        let actual = mask.attribute_sample();
        if bits == u16::from(u8::MAX) {
            assert_eq!(actual, AttributeSamplePoint::PixelCenter);
            continue;
        }

        let AttributeSamplePoint::Covered(actual) = actual else {
            panic!("partial coverage {bits:#04x} selected pixel center");
        };
        assert!(mask.contains(usize::from(actual.sample_index)));
        assert_eq!(
            COVERAGE_SAMPLES[usize::from(actual.sample_index)],
            (actual.x_eighth, actual.y_eighth)
        );
        let expected_index = COVERAGE_SAMPLES
            .iter()
            .enumerate()
            .filter(|(index, _)| mask.contains(*index))
            .min_by_key(|(_, &(x, y))| {
                let dx = x - 4;
                let dy = y - 4;
                dx * dx + dy * dy
            })
            .map(|(index, _)| index)
            .unwrap();
        assert_eq!(usize::from(actual.sample_index), expected_index);
    }
}

#[test]
fn partial_attribute_sample_policy_exhausts_every_equal_distance_tie() {
    let equal_distance_groups: [&[usize]; 3] = [&[2, 5], &[1, 3, 4, 6], &[0, 7]];
    for group in equal_distance_groups {
        let first = COVERAGE_SAMPLES[group[0]];
        let first_distance = (first.0 - 4).pow(2) + (first.1 - 4).pow(2);
        assert!(group.iter().all(|&index| {
            let sample = COVERAGE_SAMPLES[index];
            (sample.0 - 4).pow(2) + (sample.1 - 4).pow(2) == first_distance
        }));

        for subset in 1usize..(1usize << group.len()) {
            let bits = group
                .iter()
                .enumerate()
                .filter(|(position, _)| subset & (1usize << position) != 0)
                .fold(0u8, |bits, (_, &sample_index)| bits | (1u8 << sample_index));
            let expected = group
                .iter()
                .enumerate()
                .find(|(position, _)| subset & (1usize << position) != 0)
                .map(|(_, &sample_index)| sample_index)
                .expect("nonempty tie subset lost every sample");
            let AttributeSamplePoint::Covered(actual) = CoverageMask(bits).attribute_sample()
            else {
                panic!("partial tie mask {bits:#04x} selected pixel center");
            };
            assert_eq!(usize::from(actual.sample_index), expected);
        }
    }
}

#[test]
fn full_coverage_attributes_preserve_pixel_center() {
    let full = CoverageMask::from_samples(|_, _| true);
    assert_eq!(full, CoverageMask(u8::MAX));
    assert_eq!(full.attribute_sample(), AttributeSamplePoint::PixelCenter);
    assert_eq!(full.attribute_sample().offsets_eighth(), (4, 4));
}

#[test]
#[should_panic(expected = "zero coverage has no attribute sample")]
fn zero_coverage_has_no_attribute_sample() {
    CoverageMask(0).attribute_sample();
}

#[test]
fn raw_and_high_level_partial_attributes_use_the_shared_covered_sample() {
    let raw_edge = crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 4,
        ym: 0,
        yh: 0,
        xl: 0,
        dxldy: 0,
        xh: Q16_ONE as i32 / 2,
        dxhdy: 0,
        xm: 0,
        dxmdy: 0,
    };
    let raw_mask = raw_pixel_coverage(raw_edge, ScissorRect::framebuffer(1, 1), 0, 0);
    assert_eq!(raw_mask, CoverageMask(0x55));
    assert_eq!(
        raw_mask.attribute_sample(),
        AttributeSamplePoint::Covered(CoveredAttributeSample {
            sample_index: 2,
            x_eighth: 3,
            y_eighth: 3,
        })
    );
    let raw = RawRdpTriangle {
        edge: raw_edge,
        shade: Some(crate::gbi::RdpShadeCoefficients {
            // The plane is red=100+8*x. At the selected x=3/8 the
            // result is 103; the old pixel-center path produced 104.
            color: [104 << 16, 0, 0, 255 << 16],
            dcdx: [8 << 16, 0, 0, 0],
            dcde: [0; 4],
            dcdy: [0; 4],
        }),
        texture_coefficients: None,
        z: Some(crate::gbi::RdpZCoefficients {
            z: 104 << 16,
            dzdx: 8 << 16,
            dzde: 0,
            dzdy: 0,
        }),
        texture: None,
        other_mode: OtherMode::from_raw(OtherMode::default().raw_high(), 0x20, 0),
        combiner: shade_only_combiner(),
        blender: BlenderState::default(),
        scissor: None,
    };
    let mut raw_framebuffer = Framebuffer::new(1, 1);
    raw_framebuffer.draw_raw_rdp_triangle(&raw);
    assert_eq!(&raw_framebuffer.pixels[..4], &[103, 0, 0, 255]);
    let selected_depth = crate::depth::pack(103 * 8, 8 * 8);
    assert_eq!(raw_framebuffer.encoded_depth[0], Some(selected_depth));

    let mut high_vertices = [
        v(-10.0, -10.0, 20, 0, 0, 255),
        v(0.5, -10.0, 104, 0, 0, 255),
        v(0.5, 10.0, 104, 0, 0, 255),
    ];
    high_vertices[0].z = 20.0;
    high_vertices[1].z = 104.0;
    high_vertices[2].z = 104.0;
    let high = Triangle {
        v: high_vertices,
        other_mode: OtherMode::from_raw(OtherMode::default().raw_high(), 0x20, 0),
        combiner: shade_only_combiner(),
        ..Triangle::default()
    };
    let high_mask = triangle_pixel_coverage(
        high.v,
        edge(high.v[0], high.v[1], high.v[2]),
        ScissorRect::framebuffer(1, 1),
        0,
        0,
    );
    assert_eq!(high_mask, raw_mask);
    let mut high_framebuffer = Framebuffer::new(1, 1);
    high_framebuffer.draw_triangle_culled(&high, CullMode::None);
    assert_eq!(&high_framebuffer.pixels[..4], &[103, 0, 0, 255]);
    assert_eq!(high_framebuffer.encoded_depth[0], Some(selected_depth));
}

#[test]
fn raw_and_high_level_partial_texture_coordinates_use_the_shared_covered_sample() {
    let raw_edge = crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 4,
        ym: 0,
        yh: 0,
        xl: 0,
        dxldy: 0,
        xh: Q16_ONE as i32 / 2,
        dxhdy: 0,
        xm: 0,
        dxmdy: 0,
    };
    let texture = crate::gbi::Texture {
        format: 0,
        size: 2,
        width: 5,
        height: 1,
        texels: std::rc::Rc::new(
            [10u8, 20, 30, 40, 50]
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
    };
    let texel_combiner = repeated_state(
        texel_passthrough_cycle(ColorSource::Texel0, AlphaSource::Texel0),
        [0; 4],
        [0; 4],
    );
    let raw = RawRdpTriangle {
        edge: raw_edge,
        shade: None,
        texture_coefficients: Some(crate::gbi::RdpTextureCoefficients {
            // S=8*x: selected x=3/8 samples texel 3, while the old
            // pixel-center path sampled texel 4.
            stw: [4 << 16, 0, 1024 << 16],
            dstdx: [8 << 16, 0, 0],
            dstde: [0; 3],
            dstdy: [0; 3],
        }),
        z: None,
        texture: Some(texture.clone()),
        other_mode: OtherMode::default(),
        combiner: texel_combiner,
        blender: BlenderState::default(),
        scissor: None,
    };
    let mut raw_framebuffer = Framebuffer::new(1, 1);
    raw_framebuffer.draw_raw_rdp_triangle(&raw);
    assert_eq!(&raw_framebuffer.pixels[..4], &[40, 0, 0, 255]);

    let mut high_vertices = [
        v(-10.0, -10.0, 255, 255, 255, 255),
        v(0.5, -10.0, 255, 255, 255, 255),
        v(0.5, 10.0, 255, 255, 255, 255),
    ];
    high_vertices[0].s = -80.0;
    high_vertices[1].s = 4.0;
    high_vertices[2].s = 4.0;
    let high = Triangle {
        v: high_vertices,
        texture: Some(texture),
        combiner: texel_combiner,
        ..Triangle::default()
    };
    let mut high_framebuffer = Framebuffer::new(1, 1);
    high_framebuffer.draw_triangle(&high);
    assert_eq!(&high_framebuffer.pixels[..4], &[40, 0, 0, 255]);
}

#[test]
fn shared_edge_attribute_samples_stay_on_their_own_triangle() {
    let vertex = |x: f32, y: f32| v(x, y, (100.0 + 64.0 * x + 32.0 * y) as u8, 0, 0, 255);
    let triangles = [
        [vertex(0.0, 0.0), vertex(1.0, 0.0), vertex(1.0, 1.0)],
        [vertex(0.0, 0.0), vertex(1.0, 1.0), vertex(0.0, 1.0)],
    ];
    let expected = [
        (CoverageMask(0xaf), [136, 0, 0, 255]),
        (CoverageMask(0x50), [128, 0, 0, 255]),
    ];

    for (vertices, (expected_mask, expected_pixel)) in triangles.into_iter().zip(expected) {
        let area = edge(vertices[0], vertices[1], vertices[2]);
        let mask =
            triangle_pixel_coverage(vertices, area, ScissorRect::framebuffer(1, 1), 0, 0);
        assert_eq!(mask, expected_mask);
        let AttributeSamplePoint::Covered(sample) = mask.attribute_sample() else {
            panic!("shared-edge partial mask selected pixel center");
        };
        assert!(mask.contains(usize::from(sample.sample_index)));
        let point = Vertex {
            x: sample.x_eighth as f32 / 8.0,
            y: sample.y_eighth as f32 / 8.0,
            ..Vertex::default()
        };
        let signs = [
            edge(vertices[1], vertices[2], point),
            edge(vertices[2], vertices[0], point),
            edge(vertices[0], vertices[1], point),
        ];
        assert!(signs.iter().all(|value| *value * area >= 0.0));

        let mut framebuffer = Framebuffer::new(1, 1);
        framebuffer.draw_triangle(&Triangle {
            v: vertices,
            combiner: shade_only_combiner(),
            ..Triangle::default()
        });
        assert_eq!(&framebuffer.pixels[..4], &expected_pixel);
    }
}

#[test]
fn raw_edges_use_the_public_preceding_scanline_reference_point() {
    let edge = crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 8,
        ym: 4,
        yh: 1,
        xl: 0,
        dxldy: 0,
        xh: 1 << 16,
        dxhdy: 1 << 16,
        xm: 0,
        dxmdy: 0,
    };
    let (_, major) = raw_span_edges_at_y_eighth(edge, 1);
    assert_eq!(
        major,
        (1 << 16) + (1 << 13),
        "XH at YH=.25 is referenced to scanline zero, so y=.125 adds one eighth of the slope"
    );
}

#[test]
fn raw_attribute_plane_keeps_fractional_terms_in_fixed_point() {
    let value = raw_attribute_plane(
        (10 << 16) + (1 << 14),
        -(1 << 14),
        1 << 15,
        3,
        Q16_ONE + Q16_ONE / 2,
    );
    assert_eq!(value, (10 << 16) + (1 << 12));
}

#[test]
fn coverage_destinations_follow_public_clamp_wrap_full_and_save_rules() {
    let mode = |low| OtherMode::from_raw(0, low, 0);
    let pixel = Coverage::new(3);
    let memory = Coverage::new(5);

    let clamp_blend = coverage_result(pixel, memory, mode(0x0048));
    assert!(clamp_blend.blend_enabled);
    assert!(!clamp_blend.wraps);
    assert_eq!(clamp_blend.destination, Coverage::FULL);

    let clamp_new = coverage_result(pixel, Coverage::new(6), mode(0x0048));
    assert!(!clamp_new.blend_enabled);
    assert!(clamp_new.wraps);
    assert_eq!(clamp_new.destination, pixel);

    let force_clamp = coverage_result(pixel, Coverage::new(6), mode(0x4040));
    assert!(force_clamp.blend_enabled);
    assert_eq!(force_clamp.destination, Coverage::FULL);

    let wrap_at_unity = coverage_result(pixel, memory, mode(0x0140));
    assert!(!wrap_at_unity.wraps);
    assert_eq!(wrap_at_unity.destination, Coverage::FULL);
    let wrap_over_unity = coverage_result(pixel, Coverage::new(6), mode(0x0140));
    assert!(wrap_over_unity.wraps);
    assert_eq!(wrap_over_unity.destination, Coverage::new(1));

    assert_eq!(
        coverage_result(pixel, memory, mode(0x0240)).destination,
        Coverage::FULL
    );
    assert_eq!(
        coverage_result(pixel, memory, mode(0x0340)).destination,
        memory
    );
}

#[test]
fn image_read_disabled_coverage_sweep_never_merges_prior_coverage() {
    for destination in 0u32..4 {
        for antialias in [false, true] {
            for force_blend in [false, true] {
                let low = (destination << 8)
                    | if antialias { 0x0008 } else { 0 }
                    | if force_blend { 0x4000 } else { 0 };
                let mode = OtherMode::from_raw(0, low, 0);
                assert!(!mode.image_read_enabled());
                for pixel_count in 1..=Coverage::FULL.count() {
                    for memory_count in 1..=Coverage::FULL.count() {
                        let pixel = Coverage::new(pixel_count);
                        let memory = Coverage::new(memory_count);
                        let result = coverage_result(pixel, memory, mode);
                        assert!(!result.wraps);
                        assert_eq!(result.blend_enabled, force_blend || antialias);
                        let expected = match mode.coverage_destination() {
                            CoverageDestination::Clamp | CoverageDestination::Wrap => pixel,
                            CoverageDestination::Full => Coverage::FULL,
                            // SAVE suppresses the coverage write. Retaining
                            // the host-side sample is not an RDP memory read.
                            CoverageDestination::Save => memory,
                        };
                        assert_eq!(
                            result.destination, expected,
                            "IM_RD-off destination differs for destination={destination} aa={antialias} force={force_blend} pixel={pixel_count} memory={memory_count}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn raw_and_high_level_partial_coverage_obey_image_read_gate() {
    let raw_edge = crate::gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 4,
        ym: 0,
        yh: 0,
        xl: 0,
        dxldy: 0,
        xh: Q16_ONE as i32 / 2,
        dxhdy: 0,
        xm: 0,
        dxmdy: 0,
    };
    assert_eq!(
        raw_pixel_coverage(raw_edge, ScissorRect::framebuffer(1, 1), 0, 0).coverage(),
        Coverage::new(4)
    );
    let high_vertices = [
        v(-10.0, -10.0, 255, 0, 0, 255),
        v(0.5, -10.0, 255, 0, 0, 255),
        v(0.5, 10.0, 255, 0, 0, 255),
    ];
    assert_eq!(
        triangle_pixel_coverage(
            high_vertices,
            edge(high_vertices[0], high_vertices[1], high_vertices[2]),
            ScissorRect::framebuffer(1, 1),
            0,
            0,
        )
        .coverage(),
        Coverage::new(4)
    );

    for image_read in [false, true] {
        for raw in [false, true] {
            for memory_count in [2, 6] {
                let low = 0x0100 | if image_read { 0x0040 } else { 0 };
                let other_mode = OtherMode::from_raw(0xf0, low, 0);
                let mut framebuffer = Framebuffer::new(1, 1);
                framebuffer.clear(0, 0, 255, 255);
                framebuffer.coverage[0] = Coverage::new(memory_count);
                if raw {
                    framebuffer.draw_raw_rdp_triangle_no_depth(&RawRdpTriangle {
                        edge: raw_edge,
                        shade: None,
                        texture_coefficients: None,
                        z: None,
                        texture: None,
                        other_mode,
                        combiner: CombinerState {
                            primitive: [255, 0, 0, 255],
                            ..CombinerState::default()
                        },
                        blender: BlenderState::default(),
                        scissor: None,
                    });
                } else {
                    framebuffer.draw_triangle_no_depth_culled(
                        &Triangle {
                            v: high_vertices,
                            other_mode,
                            ..Triangle::default()
                        },
                        CullMode::None,
                    );
                }

                assert_eq!(&framebuffer.pixels[..4], &[255, 0, 0, 255]);
                let expected = if !image_read {
                    4
                } else if memory_count == 2 {
                    6
                } else {
                    2
                };
                assert_eq!(
                    framebuffer.coverage[0],
                    Coverage::new(expected),
                    "raw={raw} IM_RD={image_read} memory={memory_count}"
                );
            }
        }
    }
}

#[test]
fn image_read_disabled_memory_blender_traps_by_public_bit_name() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    let panic = std::panic::catch_unwind(|| {
        blend_fragment([255, 0, 0, 128], None, 128, standard_alpha_blender(1), true)
    })
    .expect_err("a framebuffer color selector without IM_RD must trap");
    let panic = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("unsupported raster panic payload must be text");
    assert!(panic.contains("blender selects framebuffer color while IM_RD is disabled"));

    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].operation,
        "render.reference.raster.image-read-disabled"
    );
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::LoudTrap
    );
}

#[test]
fn coverage_alpha_combiner_can_reduce_a_fragment_to_zero_samples() {
    let multiply_and_select = OtherMode::from_raw(0, 0x3000, 0);
    let (rgba, coverage) =
        apply_coverage_alpha(multiply_and_select, [1, 2, 3, 128], Coverage::FULL);
    assert_eq!(coverage, Coverage::new(4));
    assert_eq!(rgba[3], 128);

    let (rgba, coverage) =
        apply_coverage_alpha(multiply_and_select, [1, 2, 3, 0], Coverage::FULL);
    assert_eq!(coverage.count(), 0);
    assert_eq!(rgba[3], 0);
}

#[test]
fn alpha_coverage_hardware_probe_fixture_distinguishes_rounding_hypotheses() {
    // Programming Manual 15.5.4 and 15.7 prove the normalized product and
    // selector topology, but do not publish the multiplier width,
    // normalization denominator, quantizer, or tie rule. These synthetic
    // inputs are an inventory for a raw-DPC hardware capture: the output
    // coverage channel distinguishes the four common integer hypotheses
    // without treating any one of them as silicon evidence.
    let probes = [
        // coverage, alpha, nearest/255, nearest/256, truncate/255,
        // truncate/256
        (8u8, 15u8, 0u8, 0u8, 0u8, 0u8),
        (8, 16, 1, 1, 0, 0),
        (3, 212, 2, 2, 2, 2),
        (3, 213, 3, 2, 2, 2),
        (8, 254, 8, 8, 7, 7),
        (8, 255, 8, 8, 8, 7),
    ];

    for (coverage, alpha, nearest_255, nearest_256, truncate_255, truncate_256) in probes {
        let product = u16::from(coverage) * u16::from(alpha);
        assert_eq!(((product + 127) / 255) as u8, nearest_255);
        assert_eq!(((product + 128) / 256) as u8, nearest_256);
        assert_eq!((product / 255) as u8, truncate_255);
        assert_eq!((product / 256) as u8, truncate_256);
    }

    assert_eq!(Coverage::FULL.times_alpha(15), Coverage::new(0));
    assert_eq!(Coverage::FULL.times_alpha(16), Coverage::new(1));
    assert_eq!(Coverage::new(3).times_alpha(212), Coverage::new(2));
    assert_eq!(Coverage::new(3).times_alpha(213), Coverage::new(3));
    assert_eq!(Coverage::FULL.times_alpha(254), Coverage::FULL);
    assert_eq!(Coverage::FULL.times_alpha(255), Coverage::FULL);
}

#[test]
fn alpha_coverage_threshold_sweep_fixture_records_the_current_reference_codes() {
    // With ALPHA_CVG_SEL enabled and CVG_X_ALPHA disabled, a synthetic
    // G_AC_THRESHOLD sweep can recover the coverage-to-alpha code without
    // involving blender arithmetic: the largest passing threshold is the
    // selected alpha. These are the current normalized-u8 reference codes,
    // not a claim about the unpublished five-bit silicon path.
    let selected_alpha = [32u8, 64, 96, 128, 159, 191, 223, 255];
    for (index, expected) in selected_alpha.into_iter().enumerate() {
        let coverage = Coverage::new(index as u8 + 1);
        let selected = coverage.alpha();
        let passes_threshold = |threshold| selected >= threshold;
        assert_eq!(selected, expected);
        assert!(passes_threshold(expected));
        if expected < u8::MAX {
            assert!(!passes_threshold(expected + 1));
        }
    }
}

#[test]
fn alpha_coverage_current_policy_is_bounded_monotonic_and_endpoint_exact() {
    for coverage in 0..=Coverage::FULL.count() {
        let coverage = Coverage::new(coverage);
        assert_eq!(coverage.times_alpha(0), Coverage::new(0));
        assert_eq!(coverage.times_alpha(u8::MAX), coverage);

        let mut previous = Coverage::new(0);
        for alpha in 0..=u8::MAX {
            let current = coverage.times_alpha(alpha);
            assert!(current.count() <= coverage.count());
            assert!(current.count() >= previous.count());
            previous = current;
        }
    }
}

#[test]
fn alpha_coverage_selector_precedes_both_one_and_two_cycle_blenders() {
    let one_cycle = OtherMode::from_raw(0, 0x3000, 0);
    let two_cycle = OtherMode::from_raw(1 << 20, 0x3000, 0);
    assert_eq!(one_cycle.cycle_type(), CycleType::OneCycle);
    assert_eq!(two_cycle.cycle_type(), CycleType::TwoCycle);

    let input = [1, 2, 3, 213];
    let coverage = Coverage::new(3);
    let one = apply_coverage_alpha(one_cycle, input, coverage);
    let two = apply_coverage_alpha(two_cycle, input, coverage);
    assert_eq!(one, two);
    assert_eq!(one, ([1, 2, 3, 96], Coverage::new(3)));
}

#[test]
fn clear_on_coverage_updates_coverage_even_when_it_inhibits_color() {
    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.clear(0, 0, 255, 255);
    framebuffer.coverage[0] = Coverage::new(3);
    let mode = OtherMode::from_raw(0, 0x41c0, 0);

    assert!(!framebuffer.set_blended(
        0,
        0,
        ColorFragment {
            rgba: [255, 0, 0, 255],
            coverage: Coverage::new(4),
            shade_alpha: 255,
            noise: NoiseSample::ZERO,
        },
        BlenderState::default(),
        mode,
    ));
    assert_eq!(framebuffer.coverage[0], Coverage::new(7));
    assert_eq!(&framebuffer.pixels[..4], &[0, 0, 255, 255]);

    assert!(framebuffer.set_blended(
        0,
        0,
        ColorFragment {
            rgba: [255, 0, 0, 255],
            coverage: Coverage::new(2),
            shade_alpha: 255,
            noise: NoiseSample::ZERO,
        },
        BlenderState::default(),
        mode,
    ));
    assert_eq!(framebuffer.coverage[0], Coverage::new(1));
    assert_eq!(&framebuffer.pixels[..4], &[255, 0, 0, 255]);
}

#[test]
fn opaque_coverage_wrap_replaces_delta_range_with_strict_front_test() {
    let draw = |memory_coverage: Coverage| {
        let mut framebuffer = Framebuffer::new(1, 1);
        let memory = crate::depth::pack(128, 8);
        framebuffer.depth[0] = crate::depth::unpack(memory).0 as f32;
        framebuffer.encoded_depth[0] = Some(memory);
        framebuffer.coverage[0] = memory_coverage;
        framebuffer.set_depth_controlled_blended(
            0,
            0,
            DepthFragment {
                z: 136.0,
                delta_z: 4,
                encoded_depth: Some(crate::depth::pack(136, 4)),
                coverage: Coverage::new(1),
                rgba: [255, 0, 0, 255],
                shade_alpha: 255,
                noise: NoiseSample::ZERO,
            },
            BlenderState::default(),
            DepthControl {
                compare: true,
                update: false,
                mode: crate::gbi::DepthMode::Opaque,
            },
            OtherMode::from_raw(0, 0x0150, 0),
        )
    };

    assert!(draw(Coverage::new(1)), "correlated non-wrap must pass");
    assert!(
        !draw(Coverage::FULL),
        "wrapped coverage must require the pixel to be strictly in front"
    );
}

#[test]
fn raw_left_major_edge_selects_commanded_span_sides() {
    let major_slope = -(5.0f32 / 6.0 * 65536.0).round() as i32;
    let lower_slope = -(5.0f32 / 3.0 * 65536.0).round() as i32;
    let triangle = RawRdpTriangle {
        edge: crate::gbi::RdpEdgeCoefficients {
            left_major: true,
            level: 0,
            tile: 0,
            yl: 7 * 4,
            ym: 4 * 4,
            yh: 4,
            xl: 6 << 16,
            dxldy: lower_slope,
            xh: 6 << 16,
            dxhdy: major_slope,
            xm: 6 << 16,
            dxmdy: 0,
        },
        shade: None,
        texture_coefficients: None,
        z: None,
        texture: None,
        other_mode: OtherMode::default(),
        combiner: CombinerState {
            primitive: [255; 4],
            ..CombinerState::default()
        },
        blender: BlenderState::default(),
        scissor: None,
    };
    let edge = triangle.edge;
    let mut framebuffer = Framebuffer::new(8, 8);

    framebuffer.draw_raw_rdp_triangle(&triangle);

    let pixel = |x: usize, y: usize| {
        let offset = (y * 8 + x) * 4;
        &framebuffer.pixels[offset..offset + 4]
    };
    assert_eq!(pixel(3, 4), &[255, 255, 255, 255]);
    assert!(
        raw_pixel_coverage(
            edge,
            ScissorRect {
                ulx: 0.0,
                uly: 0.0,
                lrx: 8.0,
                lry: 8.0,
                field: false,
                keep_odd: false,
            },
            2,
            4,
        )
        .coverage()
        .count()
            > 0
    );
    assert_eq!(pixel(2, 4), &[255, 255, 255, 255]);
    assert_eq!(pixel(1, 4), &[0, 0, 0, 0]);
}

#[test]
fn real_stream_left_major_rect_split_triangle_rasterizes_interior() {
    // Byte-exact edge coefficients from WM2000's live title-scene XBUS
    // stream (task #783, first tri): `lft`=1 with the constant XH edge
    // on the LEFT (11.75) and XM marching right at +4.157/line, lower
    // half degenerate (ym == yl) -- the canonical rect-split shape every
    // real F3DEX2 quad decomposes into. Under the inverted lft reading
    // every span computed right < left and the triangle rasterized ZERO
    // pixels (the whole title logo vanished); this pins the corrected
    // convention to live-stream evidence.
    let triangle = RawRdpTriangle {
        edge: crate::gbi::RdpEdgeCoefficients {
            left_major: true,
            level: 0,
            tile: 0,
            yl: 106,
            ym: 106,
            yh: 17,
            xl: 6832128,
            dxldy: -16842729,
            xh: 770048,
            dxhdy: 0,
            xm: 701940,
            dxmdy: 272435,
        },
        shade: None,
        texture_coefficients: None,
        z: None,
        texture: None,
        other_mode: OtherMode::default(),
        combiner: CombinerState {
            primitive: [255; 4],
            ..CombinerState::default()
        },
        blender: BlenderState::default(),
        scissor: None,
    };
    let mut framebuffer = Framebuffer::new(64, 32);
    framebuffer.draw_raw_rdp_triangle(&triangle);
    let pixel = |x: usize, y: usize| {
        let offset = (y * 64 + x) * 4;
        &framebuffer.pixels[offset..offset + 4]
    };
    // Interior at y=15: span is [11.75, 10.71 + 4.157 * 11.5 ~= 58.5).
    assert_eq!(pixel(30, 15), &[255, 255, 255, 255]);
    // Left of the major edge and right of the minor edge stay untouched.
    assert_eq!(pixel(5, 15), &[0, 0, 0, 0]);
    assert_eq!(pixel(60, 10), &[0, 0, 0, 0]);
}
