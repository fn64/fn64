// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use crate::gbi::Texture;
use super::support::*;
use crate::gbi::{CullMode, CycleType, RdpDecodeState, RenderOp, TextureFilter, Triangle, Vertex};
use fn64_render::RenderError;
#[cfg(test)]
use fn64_render::UcodeId;

use crate::s2dex::*;
use crate::s2dex::object_mode::*;
use crate::s2dex::common::*;
use crate::s2dex::background::*;
use crate::s2dex::object_draw::*;
use crate::s2dex::object_ops::*;
use crate::gbi::{ConvertState, OtherMode, TextureRectangle};

#[test]
fn average_shrink_notxclamp_matches_clamped_rectangles_across_public_paths() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x300;
    const MATRIX: u32 = 0x340;
    const IMAGE: u32 = 0x500;
    const SETUP: usize = 0x700;
    let mut template = vec![0u8; 0x800];
    write_block_texture(&mut template, TXSP, IMAGE, 1);
    write_sprite(&mut template, TXSP + 24, 8, 8, 0, 2);
    write_object_matrix(&mut template, MATRIX, 0, 0, 1 << 10, 1 << 10);
    write_command(
        &mut template,
        SETUP,
        0xef00_0000 | 0x0008_0cff | (3 << 12),
        0,
    );
    write_command(&mut template, SETUP + 8, 0xdf00_0000, 0);

    let decode = |mode: u32,
                  family: S2dexWireFamily,
                  relative: bool,
                  compound: bool,
                  image_flags: u8| {
        let (
            render_mode,
            load_texture,
            rectangle,
            rectangle_r,
            load_rectangle,
            load_rectangle_r,
            move_mem,
            end,
        ) = match family {
            S2dexWireFamily::S2dex => (
                S2DEX_G_OBJ_RENDERMODE,
                S2DEX_G_OBJ_LOADTXTR,
                S2DEX_G_OBJ_RECTANGLE,
                S2DEX_G_OBJ_RECTANGLE_R,
                S2DEX_G_OBJ_LDTX_RECT,
                S2DEX_G_OBJ_LDTX_RECT_R,
                S2DEX_G_OBJ_MOVEMEM,
                S2DEX_G_ENDDL,
            ),
            S2dexWireFamily::S2dex2 => (
                G_OBJ_RENDERMODE,
                G_OBJ_LOADTXTR,
                G_OBJ_RECTANGLE,
                G_OBJ_RECTANGLE_R,
                G_OBJ_LDTX_RECT,
                G_OBJ_LDTX_RECT_R,
                G_OBJ_MOVEMEM,
                G_ENDDL,
            ),
        };
        let mut rdram = template.clone();
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
            fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
            image_flags,
        );
        let mut offset = DL;
        write_command(&mut rdram, offset, u32::from(render_mode) << 24, mode);
        offset += 8;
        if relative {
            write_command(
                &mut rdram,
                offset,
                (u32::from(move_mem) << 24) | 0x17,
                MATRIX,
            );
            offset += 8;
        }
        if compound {
            write_command(
                &mut rdram,
                offset,
                (u32::from(if relative {
                    load_rectangle_r
                } else {
                    load_rectangle
                }) << 24)
                    | 0x2f,
                TXSP,
            );
            offset += 8;
        } else {
            write_command(
                &mut rdram,
                offset,
                (u32::from(load_texture) << 24) | 0x17,
                TXSP,
            );
            offset += 8;
            write_command(
                &mut rdram,
                offset,
                u32::from(if relative { rectangle_r } else { rectangle }) << 24,
                TXSP + 24,
            );
            offset += 8;
        }
        write_command(&mut rdram, offset, u32::from(end) << 24, 0);
        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
        decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap()
    };

    for shrink_mode in [G_OBJRM_SHRINKSIZE_1, G_OBJRM_SHRINKSIZE_2] {
        for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
            for relative in [false, true] {
                for compound in [false, true] {
                    for image_flags in [
                        0,
                        G_OBJ_FLAG_FLIPS,
                        G_OBJ_FLAG_FLIPT,
                        G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
                    ] {
                        let clamped =
                            decode(shrink_mode, family, relative, compound, image_flags);
                        let unclamped = decode(
                            shrink_mode | G_OBJRM_NOTXCLAMP,
                            family,
                            relative,
                            compound,
                            image_flags,
                        );
                        let RenderOp::TextureRectangle(clamped) = &clamped[0] else {
                            panic!("Average shrink path must emit one rectangle")
                        };
                        let RenderOp::TextureRectangle(unclamped) = &unclamped[0] else {
                            panic!("unclamped Average shrink path must emit one rectangle")
                        };
                        assert_eq!(
                            (
                                unclamped.ulx,
                                unclamped.uly,
                                unclamped.lrx,
                                unclamped.lry,
                                unclamped.s,
                                unclamped.t,
                                unclamped.dsdx,
                                unclamped.dtdy,
                            ),
                            (
                                clamped.ulx,
                                clamped.uly,
                                clamped.lrx,
                                clamped.lry,
                                clamped.s,
                                clamped.t,
                                clamped.dsdx,
                                clamped.dtdy,
                            )
                        );
                    }
                }
            }
        }
    }
}


#[test]
fn average_shrink_rectangles_exhaust_families_paths_scales_and_flips() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x300;
    const MATRIX: u32 = 0x340;
    const IMAGE: u32 = 0x500;
    const SETUP: usize = 0x700;
    let mut template = vec![0u8; 0x800];
    write_block_texture(&mut template, TXSP, IMAGE, 1);
    write_sprite(&mut template, TXSP + 24, 8, 8, 0, 2);
    write_object_matrix(&mut template, MATRIX, 0, 0, 1 << 10, 1 << 10);
    write_command(
        &mut template,
        SETUP,
        0xef00_0000 | 0x0008_0cff | (3 << 12),
        0,
    );
    write_command(&mut template, SETUP + 8, 0xdf00_0000, 0);

    let mut saw_positive_edge_clamp = false;
    for shrink_half_texels in [1u8, 2] {
        let render_mode_value = if shrink_half_texels == 1 {
            G_OBJRM_SHRINKSIZE_1
        } else {
            G_OBJRM_SHRINKSIZE_2
        };
        for scale in [512u16, 1024, 2048, 4096] {
            for image_flags in [
                0,
                G_OBJ_FLAG_FLIPS,
                G_OBJ_FLAG_FLIPT,
                G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
            ] {
                let mut expected = None;
                for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
                    let (
                        render_mode,
                        load_texture,
                        rectangle,
                        rectangle_r,
                        load_rectangle,
                        load_rectangle_r,
                        move_mem,
                        end,
                    ) = match family {
                        S2dexWireFamily::S2dex => (
                            S2DEX_G_OBJ_RENDERMODE,
                            S2DEX_G_OBJ_LOADTXTR,
                            S2DEX_G_OBJ_RECTANGLE,
                            S2DEX_G_OBJ_RECTANGLE_R,
                            S2DEX_G_OBJ_LDTX_RECT,
                            S2DEX_G_OBJ_LDTX_RECT_R,
                            S2DEX_G_OBJ_MOVEMEM,
                            S2DEX_G_ENDDL,
                        ),
                        S2dexWireFamily::S2dex2 => (
                            G_OBJ_RENDERMODE,
                            G_OBJ_LOADTXTR,
                            G_OBJ_RECTANGLE,
                            G_OBJ_RECTANGLE_R,
                            G_OBJ_LDTX_RECT,
                            G_OBJ_LDTX_RECT_R,
                            G_OBJ_MOVEMEM,
                            G_ENDDL,
                        ),
                    };
                    for compound in [false, true] {
                        for relative in [false, true] {
                            let mut rdram = template.clone();
                            {
                                let mut view =
                                    fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                                let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
                                view.write_u16(sprite.checked_add(2).unwrap(), scale);
                                view.write_u16(sprite.checked_add(10).unwrap(), scale);
                                view.write_u8(sprite.checked_add(23).unwrap(), image_flags);
                            }
                            let mut offset = DL;
                            write_command(
                                &mut rdram,
                                offset,
                                u32::from(render_mode) << 24,
                                render_mode_value,
                            );
                            offset += 8;
                            if relative {
                                write_command(
                                    &mut rdram,
                                    offset,
                                    (u32::from(move_mem) << 24) | 0x17,
                                    MATRIX,
                                );
                                offset += 8;
                            }
                            if compound {
                                let opcode = if relative {
                                    load_rectangle_r
                                } else {
                                    load_rectangle
                                };
                                write_command(
                                    &mut rdram,
                                    offset,
                                    (u32::from(opcode) << 24) | 0x2f,
                                    TXSP,
                                );
                                offset += 8;
                            } else {
                                write_command(
                                    &mut rdram,
                                    offset,
                                    (u32::from(load_texture) << 24) | 0x17,
                                    TXSP,
                                );
                                offset += 8;
                                let opcode = if relative { rectangle_r } else { rectangle };
                                write_command(
                                    &mut rdram,
                                    offset,
                                    u32::from(opcode) << 24,
                                    TXSP + 24,
                                );
                                offset += 8;
                            }
                            write_command(&mut rdram, offset, u32::from(end) << 24, 0);

                            let mut rdp = RdpDecodeState::default();
                            crate::gbi::decode_raw_rdp_ops_with_state(
                                &rdram,
                                SETUP as u32,
                                &mut rdp,
                            )
                            .unwrap();
                            let operations =
                                decode_ops_for_family(&rdram, DL as u32, &mut rdp, family)
                                    .unwrap();
                            let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
                                panic!("Average shrink path must emit one rectangle")
                            };
                            let identity = (
                                rectangle.ulx,
                                rectangle.uly,
                                rectangle.lrx,
                                rectangle.lry,
                                rectangle.s,
                                rectangle.t,
                                rectangle.dsdx,
                                rectangle.dtdy,
                            );
                            if let Some(expected) = expected {
                                assert_eq!(identity, expected);
                            } else {
                                expected = Some(identity);
                            }

                            let inset = f32::from(shrink_half_texels) * 0.5;
                            let expected_start = |flipped| {
                                if flipped {
                                    8.0 - 1.0 - inset
                                } else {
                                    inset
                                }
                            };
                            assert_eq!(
                                (rectangle.s, rectangle.t),
                                (
                                    expected_start(image_flags & G_OBJ_FLAG_FLIPS != 0),
                                    expected_start(image_flags & G_OBJ_FLAG_FLIPT != 0),
                                )
                            );
                            let full_extent = 8192.0 / f32::from(scale);
                            let shrink_extent =
                                f32::from(shrink_half_texels) * 1024.0 / f32::from(scale);
                            assert_eq!(
                                (rectangle.lrx, rectangle.lry),
                                (full_extent - shrink_extent, full_extent - shrink_extent)
                            );

                            let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
                            let footprint = ObjectAverageShrinkFootprint {
                                inset_half_texels: shrink_half_texels,
                            };
                            for x in pixel_min(rectangle.ulx)..pixel_min(rectangle.lrx) {
                                let s = rectangle.s
                                    + (x as f32 - rectangle.ulx.floor())
                                        * f32::from(rectangle.dsdx)
                                        / 1024.0;
                                let s0 = s.floor() as i32;
                                let cell = footprint
                                    .classify_cell(s, 8, "S", "G_OBJ_RECTANGLE")
                                    .unwrap();
                                assert!((0..=8).contains(&s0));
                                assert!((0..=8).contains(&(s0 + 1)));
                                assert!((0..8).contains(&s0.clamp(0, 7)));
                                assert!((0..8).contains(&(s0 + 1).clamp(0, 7)));
                                saw_positive_edge_clamp |=
                                    cell == ObjectAverageCell::PositiveEdgeClamped;
                            }
                            for y in pixel_min(rectangle.uly)..pixel_min(rectangle.lry) {
                                let t = rectangle.t
                                    + (y as f32 - rectangle.uly.floor())
                                        * f32::from(rectangle.dtdy)
                                        / 1024.0;
                                let t0 = t.floor() as i32;
                                let cell = footprint
                                    .classify_cell(t, 8, "T", "G_OBJ_RECTANGLE")
                                    .unwrap();
                                assert!((0..=8).contains(&t0));
                                assert!((0..=8).contains(&(t0 + 1)));
                                assert!((0..8).contains(&t0.clamp(0, 7)));
                                assert!((0..8).contains(&(t0 + 1).clamp(0, 7)));
                                saw_positive_edge_clamp |=
                                    cell == ObjectAverageCell::PositiveEdgeClamped;
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        saw_positive_edge_clamp,
        "the exhaustive sweep must exercise the public positive-edge clamp"
    );
}


#[test]
fn average_shrink_matrix_relative_base_scale_cross_term_is_exact() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x300;
    const MATRIX: u32 = 0x340;
    const IMAGE: u32 = 0x500;
    const SETUP: usize = 0x700;
    let mut template = vec![0u8; 0x800];
    write_block_texture(&mut template, TXSP, IMAGE, 1);
    write_sprite(&mut template, TXSP + 24, 4, 4, 0, 2);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut template);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3); // four 64-bit words
        for (index, color) in [
            0xf801u16, 0xf801, 0x07c1, 0x07c1, 0xf801, 0xf801, 0x07c1, 0x07c1, 0x003f, 0x003f,
            0xffff, 0xffff, 0x003f, 0x003f, 0xffff, 0xffff,
        ]
        .into_iter()
        .enumerate()
        {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                color,
            );
        }
    }
    write_command(
        &mut template,
        SETUP,
        0xef00_0000 | 0x0008_0cff | (3 << 12),
        0,
    );
    write_command(&mut template, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
    write_command(&mut template, SETUP + 16, 0xdf00_0000, 0);

    for (base_scale, effective_scale, expected_extent, drawn_pixels, expected_last) in [
        (
            512,
            512,
            6.0,
            6usize,
            ObjectAverageCell::PositiveEdgeClamped,
        ),
        (2048, 2048, 1.5, 1usize, ObjectAverageCell::Interior),
    ] {
        let mut rdram = template.clone();
        write_object_matrix(&mut rdram, MATRIX, 0, 0, base_scale, base_scale);
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            // matrix_relative_sprite applies object scale * BaseScale / 1024.
            view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
            view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
        }
        write_command(
            &mut rdram,
            DL,
            u32::from(G_OBJ_RENDERMODE) << 24,
            G_OBJRM_SHRINKSIZE_1,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_OBJ_MOVEMEM) << 24) | 0x17,
            MATRIX,
        );
        write_command(
            &mut rdram,
            DL + 16,
            (u32::from(G_OBJ_LDTX_RECT_R) << 24) | 0x2f,
            TXSP,
        );
        write_command(&mut rdram, DL + 24, u32::from(G_ENDDL) << 24, 0);

        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("Average shrink RectangleR must remain a texture rectangle")
        };
        assert_eq!(
            (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
            (0.0, 0.0, expected_extent, expected_extent)
        );
        assert_eq!(
            (rectangle.dsdx, rectangle.dtdy),
            (effective_scale, effective_scale)
        );
        assert_eq!((rectangle.s, rectangle.t), (0.5, 0.5));
        let footprint = ObjectAverageShrinkFootprint {
            inset_half_texels: 1,
        };
        let expected_axis = ObjectAverageAxisFootprint::Samples {
            first: ObjectAverageCell::Interior,
            last: expected_last,
        };
        assert_eq!(
            footprint
                .validate_axis(
                    rectangle.s,
                    f32::from(rectangle.dsdx) / 1024.0,
                    rectangle.ulx,
                    rectangle.lrx,
                    4,
                    "S",
                    "G_OBJ_LDTX_RECT_R",
                )
                .unwrap(),
            expected_axis
        );
        assert_eq!(
            footprint
                .validate_axis(
                    rectangle.t,
                    f32::from(rectangle.dtdy) / 1024.0,
                    rectangle.uly,
                    rectangle.lry,
                    4,
                    "T",
                    "G_OBJ_LDTX_RECT_R",
                )
                .unwrap(),
            expected_axis
        );

        let mut framebuffer = crate::raster::Framebuffer::new(6, 6);
        framebuffer.clear(0, 0, 0, 0);
        framebuffer.draw_texture_rectangle(rectangle);
        assert_eq!(
            framebuffer
                .pixels
                .chunks_exact(4)
                .filter(|pixel| pixel[3] != 0)
                .count(),
            drawn_pixels * drawn_pixels
        );
        assert_eq!(&framebuffer.pixels[..4], [255, 0, 0, 255]);
        if base_scale == 512 {
            let pixel = |x: usize, y: usize| {
                let offset = (y * 6 + x) * 4;
                &framebuffer.pixels[offset..offset + 4]
            };
            assert_eq!(pixel(1, 0), [128, 128, 0, 255]);
            assert_eq!(pixel(5, 0), [0, 255, 0, 255]);
            assert_eq!(pixel(0, 1), [128, 0, 128, 255]);
            assert_eq!(pixel(1, 1), [128, 128, 128, 255]);
            assert_eq!(pixel(5, 5), [255, 255, 255, 255]);
        }
    }
}


#[test]
fn average_shrink_one_raster_matches_exact_four_texel_cells_under_all_flips() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x300;
    const IMAGE: u32 = 0x500;
    const SETUP: usize = 0x700;
    let mut template = vec![0u8; 0x800];
    write_block_texture(&mut template, TXSP, IMAGE, 1);
    write_sprite(&mut template, TXSP + 24, 4, 4, 0, 2);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut template);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3); // four 64-bit words
        for (index, color) in [
            0xf801u16, 0xf801, 0x07c1, 0x07c1, 0xf801, 0xf801, 0x07c1, 0x07c1, 0x003f, 0x003f,
            0xffff, 0xffff, 0x003f, 0x003f, 0xffff, 0xffff,
        ]
        .into_iter()
        .enumerate()
        {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                color,
            );
        }
    }
    write_command(
        &mut template,
        SETUP,
        0xef00_0000 | 0x0008_0cff | (3 << 12),
        0,
    );
    write_command(&mut template, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
    write_command(&mut template, SETUP + 16, 0xdf00_0000, 0);
    let expected = [
        [[255, 0, 0, 255], [128, 128, 0, 255], [0, 255, 0, 255]],
        [
            [128, 0, 128, 255],
            [128, 128, 128, 255],
            [128, 255, 128, 255],
        ],
        [[0, 0, 255, 255], [128, 128, 255, 255], [255, 255, 255, 255]],
    ];

    for image_flags in [
        0,
        G_OBJ_FLAG_FLIPS,
        G_OBJ_FLAG_FLIPT,
        G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
    ] {
        let mut rdram = template.clone();
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
            fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
            image_flags,
        );
        write_command(
            &mut rdram,
            DL,
            u32::from(G_OBJ_RENDERMODE) << 24,
            G_OBJRM_SHRINKSIZE_1,
        );
        write_command(
            &mut rdram,
            DL + 8,
            (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
            TXSP,
        );
        write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        let mut rdp = RdpDecodeState::default();
        crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
        let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("Average shrink raster must remain a texture rectangle")
        };
        assert_eq!(
            (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
            (0.0, 0.0, 3.0, 3.0)
        );
        let mut framebuffer = crate::raster::Framebuffer::new(3, 3);
        framebuffer.clear(0, 0, 0, 0);
        framebuffer.draw_texture_rectangle(rectangle);
        for y in 0..3usize {
            for x in 0..3usize {
                let source_x = if image_flags & G_OBJ_FLAG_FLIPS != 0 {
                    2 - x
                } else {
                    x
                };
                let source_y = if image_flags & G_OBJ_FLAG_FLIPT != 0 {
                    2 - y
                } else {
                    y
                };
                let offset = (y * 3 + x) * 4;
                assert_eq!(
                    &framebuffer.pixels[offset..offset + 4],
                    expected[source_y][source_x],
                    "flags={image_flags:#04x} output=({x},{y})"
                );
            }
        }
    }
}


#[test]
fn average_shrink_keeps_unpublished_neighbor_classes_loud_and_transactional() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x300;
    const MATRIX: u32 = 0x340;
    const IMAGE: u32 = 0x500;
    const AVERAGE_SETUP: usize = 0x700;
    const COPY_SETUP: usize = 0x720;
    let mut rdram = vec![0u8; 0x800];
    write_block_texture(&mut rdram, TXSP, IMAGE, 1);
    write_sprite(&mut rdram, TXSP + 24, 8, 8, 0, 2);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 0, 0);
    write_command(
        &mut rdram,
        AVERAGE_SETUP,
        0xef00_0000 | 0x0008_0cff | (3 << 12),
        0,
    );
    write_command(&mut rdram, AVERAGE_SETUP + 8, 0xdf00_0000, 0);
    write_command(
        &mut rdram,
        COPY_SETUP,
        0xef00_0000 | 0x0008_0cff | (3 << 12) | (2 << 20),
        0,
    );
    write_command(&mut rdram, COPY_SETUP + 8, 0xdf00_0000, 0);

    let rectangle_error = |rdram: &mut [u8], mode, rdp: &mut RdpDecodeState| {
        write_command(rdram, DL, u32::from(G_OBJ_RENDERMODE) << 24, mode);
        write_command(
            rdram,
            DL + 8,
            (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
            TXSP,
        );
        write_command(rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
        let before = format!("{rdp:?}");
        let error = decode_ops(rdram, DL as u32, rdp).unwrap_err();
        assert_eq!(format!("{rdp:?}"), before);
        error.to_string()
    };

    let mut average_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, AVERAGE_SETUP as u32, &mut average_rdp)
        .unwrap();
    write_command(
        &mut rdram,
        DL,
        u32::from(G_OBJ_RENDERMODE) << 24,
        G_OBJRM_SHRINKSIZE_1 | G_OBJRM_NOTXCLAMP,
    );
    write_command(
        &mut rdram,
        DL + 8,
        (u32::from(G_OBJ_LDTX_RECT) << 24) | 0x2f,
        TXSP,
    );
    write_command(&mut rdram, DL + 16, u32::from(G_ENDDL) << 24, 0);
    decode_ops(&rdram, DL as u32, &mut average_rdp.clone())
        .expect("inward Average cells make NOTXCLAMP unobservable");
    let error = rectangle_error(
        &mut rdram,
        G_OBJRM_SHRINKSIZE_1 | G_OBJRM_WIDEN,
        &mut average_rdp,
    );
    assert!(
        error.contains("positive-edge four-texel footprint"),
        "{error}"
    );

    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1536);
        view.write_u16(sprite.checked_add(10).unwrap(), 1536);
    }
    let error = rectangle_error(&mut rdram, G_OBJRM_SHRINKSIZE_1, &mut average_rdp);
    assert!(error.contains("sub-quarter-pixel rounding"), "{error}");
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
        view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
    }

    let mut copy_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, COPY_SETUP as u32, &mut copy_rdp)
        .unwrap();
    let error = rectangle_error(&mut rdram, G_OBJRM_SHRINKSIZE_1, &mut copy_rdp);
    assert!(error.contains("Copy cycle does not support"), "{error}");

    write_command(
        &mut rdram,
        DL,
        u32::from(G_OBJ_RENDERMODE) << 24,
        G_OBJRM_SHRINKSIZE_1,
    );
    write_command(
        &mut rdram,
        DL + 8,
        (u32::from(G_OBJ_LOADTXTR) << 24) | 0x17,
        TXSP,
    );
    write_command(
        &mut rdram,
        DL + 16,
        (u32::from(G_OBJ_MOVEMEM) << 24) | 0x17,
        MATRIX,
    );
    write_command(
        &mut rdram,
        DL + 24,
        u32::from(G_OBJ_SPRITE) << 24,
        TXSP + 24,
    );
    write_command(&mut rdram, DL + 32, u32::from(G_ENDDL) << 24, 0);
    let before = format!("{average_rdp:?}");
    let error = decode_ops(&rdram, DL as u32, &mut average_rdp).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("rotating polygon requires a separately evidenced pixel-center"),
        "{error}"
    );
    assert_eq!(format!("{average_rdp:?}"), before);
}


#[test]
fn widen_expands_only_exact_positive_edges_and_rasterizes() {
    const BASE_DL: usize = 0x100;
    const WIDEN_DL: usize = 0x120;
    const INEXACT_DL: usize = 0x140;
    const TXSP: u32 = 0x200;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x500];
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 6, 6, 0, 2);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1536);
        view.write_u16(sprite.checked_add(10).unwrap(), 1536);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(IMAGE), 0xffff);
    }
    write_command(&mut rdram, BASE_DL, 0x0700_002f, TXSP);
    write_command(&mut rdram, BASE_DL + 8, 0xdf00_0000, 0);
    write_command(&mut rdram, WIDEN_DL, 0x0b00_0000, G_OBJRM_WIDEN);
    write_command(&mut rdram, WIDEN_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, WIDEN_DL + 16, 0xdf00_0000, 0);

    let base = decode_ops(&rdram, BASE_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let widened = decode_ops(&rdram, WIDEN_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(base) = &base[0] else {
        unreachable!()
    };
    let RenderOp::TextureRectangle(widened) = &widened[0] else {
        unreachable!()
    };
    assert_eq!((base.lrx, base.lry), (4.0, 4.0));
    assert_eq!((widened.lrx, widened.lry), (4.25, 4.25));
    let mut framebuffer = crate::raster::Framebuffer::new(1, 1);
    framebuffer.clear(0, 0, 0, 0);
    framebuffer.draw_texture_rectangle(widened);
    assert_ne!(&framebuffer.pixels[..4], [0, 0, 0, 0]);

    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1024);
        view.write_u16(sprite.checked_add(10).unwrap(), 1024);
    }
    write_command(&mut rdram, INEXACT_DL, 0x0b00_0000, G_OBJRM_WIDEN);
    write_command(&mut rdram, INEXACT_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, INEXACT_DL + 16, 0xdf00_0000, 0);
    let error =
        decode_ops(&rdram, INEXACT_DL as u32, &mut RdpDecodeState::default()).unwrap_err();
    assert!(error
        .to_string()
        .contains("unpublished sub-quarter-pixel rounding"));
}


#[test]
fn object_perimeter_shrink_and_widen_compose_across_families_and_draw_paths() {
    const RECT_DL: usize = 0x100;
    const RELATIVE_DL: usize = 0x140;
    const ROTATING_DL: usize = 0x180;
    const STANDALONE_DL: usize = 0x1c0;
    const TXSP: u32 = 0x280;
    const MATRIX: u32 = 0x2b0;
    const IMAGE: u32 = 0x400;
    const COPY_SETUP: usize = 0x500;
    let mut rdram = vec![0u8; 0x600];
    write_block_texture(&mut rdram, TXSP, IMAGE, 1);
    write_sprite(&mut rdram, TXSP + 24, 4, 4, 0, 2);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 0);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3);
        view.write_u16(sprite.checked_add(2).unwrap(), 512);
        view.write_u16(sprite.checked_add(10).unwrap(), 512);
        for index in 0..16 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                if index == 0 { 0xffff } else { 0xf801 },
            );
        }
    }
    let mode = G_OBJRM_SHRINKSIZE_1 | G_OBJRM_WIDEN;

    for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
        let (render_mode, load_rect, end) = match family {
            S2dexWireFamily::S2dex => {
                (S2DEX_G_OBJ_RENDERMODE, S2DEX_G_OBJ_LDTX_RECT, S2DEX_G_ENDDL)
            }
            S2dexWireFamily::S2dex2 => (G_OBJ_RENDERMODE, G_OBJ_LDTX_RECT, G_ENDDL),
        };
        write_command(&mut rdram, RECT_DL, u32::from(render_mode) << 24, mode);
        write_command(
            &mut rdram,
            RECT_DL + 8,
            (u32::from(load_rect) << 24) | 0x2f,
            TXSP,
        );
        write_command(&mut rdram, RECT_DL + 16, u32::from(end) << 24, 0);
        let operations = decode_ops_for_family(
            &rdram,
            RECT_DL as u32,
            &mut RdpDecodeState::default(),
            family,
        )
        .unwrap();
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("combined perimeter rectangle must retain rectangle lowering")
        };
        assert_eq!(
            (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
            (0.0, 0.0, 6.75, 6.75),
            "family={family:?}"
        );
        assert_eq!((rectangle.s, rectangle.t), (0.5, 0.5));
    }

    write_command(&mut rdram, RELATIVE_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, RELATIVE_DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, RELATIVE_DL + 16, 0x0800_002f, TXSP);
    write_command(&mut rdram, RELATIVE_DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, ROTATING_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, ROTATING_DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, ROTATING_DL + 16, 0x0600_002f, TXSP);
    write_command(&mut rdram, ROTATING_DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, STANDALONE_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, STANDALONE_DL + 8, 0x0500_0017, TXSP);
    write_command(&mut rdram, STANDALONE_DL + 16, 0x0100_0000, TXSP + 24);
    write_command(&mut rdram, STANDALONE_DL + 24, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, STANDALONE_DL + 32, 0xda00_0000, TXSP + 24);
    write_command(&mut rdram, STANDALONE_DL + 40, 0x0200_0000, TXSP + 24);
    write_command(&mut rdram, STANDALONE_DL + 48, 0xdf00_0000, 0);

    let relative_ops =
        decode_ops(&rdram, RELATIVE_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let rotating_ops =
        decode_ops(&rdram, ROTATING_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let standalone =
        decode_ops(&rdram, STANDALONE_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(relative) = &relative_ops[0] else {
        unreachable!()
    };
    assert_eq!((relative.ulx, relative.lrx, relative.s), (4.0, 10.75, 0.5));
    let RenderOp::Triangle(rotating) = &rotating_ops[0] else {
        unreachable!()
    };
    assert_eq!(
        rotating
            .v
            .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t)),
        [
            (4.0, 0.0, 0.5, 0.5),
            (10.75, 0.0, 3.875, 0.5),
            (10.75, 6.75, 3.875, 3.875),
        ]
    );
    let draw = |operations: &[RenderOp]| {
        let mut framebuffer = crate::raster::Framebuffer::new(12, 8);
        framebuffer.clear(0, 0, 0, 0);
        for operation in operations {
            match operation {
                RenderOp::TextureRectangle(rectangle) => {
                    framebuffer.draw_texture_rectangle(rectangle)
                }
                RenderOp::Triangle(triangle) => framebuffer.draw_triangle(triangle),
                _ => panic!("perimeter fixture emitted an unexpected operation"),
            }
        }
        framebuffer
    };
    let relative_fb = draw(&relative_ops);
    let rotating_fb = draw(&rotating_ops);
    for y in 0..7usize {
        for x in 4..11usize {
            let offset = (y * 12 + x) * 4;
            assert_ne!(&relative_fb.pixels[offset..offset + 4], [0, 0, 0, 0]);
            assert_ne!(&rotating_fb.pixels[offset..offset + 4], [0, 0, 0, 0]);
        }
    }
    let RenderOp::TextureRectangle(standalone_rectangle) = &standalone[0] else {
        unreachable!()
    };
    let RenderOp::TextureRectangle(standalone_relative) = &standalone[1] else {
        unreachable!()
    };
    let RenderOp::Triangle(standalone_rotating) = &standalone[2] else {
        unreachable!()
    };
    assert_eq!(
        (standalone_rectangle.lrx, standalone_rectangle.s),
        (6.75, 0.5)
    );
    assert_eq!(
        (standalone_relative.ulx, standalone_relative.lrx),
        (relative.ulx, relative.lrx)
    );
    assert_eq!(standalone_rotating.v, rotating.v);

    write_command(
        &mut rdram,
        RECT_DL,
        0x0b00_0000,
        G_OBJRM_SHRINKSIZE_2 | G_OBJRM_WIDEN,
    );
    write_command(&mut rdram, RECT_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, RECT_DL + 16, 0xdf00_0000, 0);
    let shrink_two =
        decode_ops(&rdram, RECT_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(shrink_two) = &shrink_two[0] else {
        unreachable!()
    };
    assert_eq!((shrink_two.lrx, shrink_two.lry), (4.75, 4.75));
    assert_eq!((shrink_two.s, shrink_two.t), (1.0, 1.0));

    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
        fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
        G_OBJ_FLAG_FLIPS,
    );
    let error = decode_ops(&rdram, RECT_DL as u32, &mut RdpDecodeState::default()).unwrap_err();
    assert!(error.to_string().contains("positive-edge selection"));
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u8(fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23), 0);

    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
        view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
    }
    write_command(&mut rdram, COPY_SETUP, 0xef00_0000 | (2 << 20), 0);
    write_command(&mut rdram, COPY_SETUP + 8, 0xdf00_0000, 0);
    let mut copy_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, COPY_SETUP as u32, &mut copy_rdp)
        .unwrap();
    let error = decode_ops(&rdram, RECT_DL as u32, &mut copy_rdp).unwrap_err();
    assert!(error.to_string().contains("Copy cycle does not support"));
}


#[test]
fn object_bilerp_mode_matches_filter_and_preserves_corrected_texel_centers() {
    const POINT_DL: usize = 0x100;
    const BILERP_DL: usize = 0x140;
    const BILERP_SPRITE_DL: usize = 0x180;
    const TXSP: u32 = 0x200;
    const MATRIX: u32 = 0x240;
    const SPRITE: u32 = 0x258;
    const IMAGE: u32 = 0x400;
    const POINT_SETUP: usize = 0x500;
    const BILERP_SETUP: usize = 0x520;
    let mut rdram = vec![0u8; 0x600];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, color) in [0xf801, 0x003f, 0x07c1, 0xffff]
            .into_iter()
            .cycle()
            .take(8)
            .enumerate()
        {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                color,
            );
        }
    }
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 0, 0);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, POINT_DL, 0x0700_002f, TXSP);
    write_command(&mut rdram, POINT_DL + 8, 0xdf00_0000, 0);
    write_command(&mut rdram, BILERP_DL, 0x0b00_0000, G_OBJRM_BILERP);
    write_command(&mut rdram, BILERP_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, BILERP_DL + 16, 0xdf00_0000, 0);
    write_command(&mut rdram, BILERP_SPRITE_DL, 0x0b00_0000, G_OBJRM_BILERP);
    write_command(&mut rdram, BILERP_SPRITE_DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, BILERP_SPRITE_DL + 16, 0x0200_0000, SPRITE);
    write_command(&mut rdram, BILERP_SPRITE_DL + 24, 0xdf00_0000, 0);
    let combine_texel0 = (0xfc8f_ff1f, 0x88fc_f279);
    write_command(&mut rdram, POINT_SETUP, combine_texel0.0, combine_texel0.1);
    write_command(&mut rdram, POINT_SETUP + 8, 0xdf00_0000, 0);
    write_command(
        &mut rdram,
        BILERP_SETUP,
        0xef00_0000 | 0x0008_0cff | (2 << 12),
        0,
    );
    write_command(
        &mut rdram,
        BILERP_SETUP + 8,
        combine_texel0.0,
        combine_texel0.1,
    );
    write_command(&mut rdram, BILERP_SETUP + 16, 0xdf00_0000, 0);

    let mut point_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, POINT_SETUP as u32, &mut point_rdp)
        .unwrap();
    let point = decode_ops(&rdram, POINT_DL as u32, &mut point_rdp).unwrap();
    let mut bilerp_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, BILERP_SETUP as u32, &mut bilerp_rdp)
        .unwrap();
    let bilerp = decode_ops(&rdram, BILERP_DL as u32, &mut bilerp_rdp).unwrap();
    let (RenderOp::TextureRectangle(point), RenderOp::TextureRectangle(bilerp)) =
        (&point[0], &bilerp[0])
    else {
        panic!("object rectangles must retain their typed operations")
    };
    assert_eq!((point.s, point.t), (0.0, 0.0));
    assert_eq!((bilerp.s, bilerp.t), (0.0, 0.0));

    let mut point_fb = crate::raster::Framebuffer::new(4, 2);
    point_fb.clear(0, 0, 0, 0);
    point_fb.draw_texture_rectangle(point);
    let mut bilerp_fb = crate::raster::Framebuffer::new(4, 2);
    bilerp_fb.clear(0, 0, 0, 0);
    bilerp_fb.draw_texture_rectangle(bilerp);
    assert_eq!(bilerp_fb.pixels, point_fb.pixels);
    assert_eq!(&bilerp_fb.pixels[..4], [255, 0, 0, 255]);
    assert_eq!(&bilerp_fb.pixels[4..8], [0, 0, 255, 255]);

    let sprite_ops = decode_ops(&rdram, BILERP_SPRITE_DL as u32, &mut bilerp_rdp).unwrap();
    let RenderOp::Triangle(first) = &sprite_ops[0] else {
        panic!("bilerp sprite must lower to triangles")
    };
    assert_eq!(
        first.v.map(|vertex| (vertex.s, vertex.t)),
        [(-0.5, -0.5), (3.5, -0.5), (3.5, 1.5)]
    );
    let mut sprite_fb = crate::raster::Framebuffer::new(4, 2);
    sprite_fb.clear(0, 0, 0, 0);
    for operation in &sprite_ops {
        let RenderOp::Triangle(triangle) = operation else {
            unreachable!("bilerp sprite emits only triangles")
        };
        sprite_fb.draw_triangle(triangle);
    }
    let sample_at = |operation: &RenderOp, x, y| {
        let RenderOp::Triangle(triangle) = operation else {
            unreachable!()
        };
        crate::raster::test_triangle_attribute_sample(
            triangle.v,
            triangle
                .scissor
                .unwrap_or_else(|| crate::gbi::ScissorRect::framebuffer(4, 2)),
            x,
            y,
        )
    };
    assert_eq!(sample_at(&sprite_ops[0], 1, 0), (0xaf, Some((2, 3, 3))));
    assert_eq!(sample_at(&sprite_ops[1], 1, 0), (0x50, Some((4, 1, 5))));
    assert_ne!(0xaf & (1 << 2), 0);
    assert_ne!(0x50 & (1 << 4), 0);
    assert_eq!(
        sprite_fb.pixels,
        [
            255, 0, 0, 255, 96, 0, 159, 255, 0, 255, 0, 255, 255, 255, 255, 255, 255, 0, 0,
            255, 0, 0, 255, 255, 0, 223, 32, 255, 159, 255, 159, 255,
        ]
    );

    write_command(&mut rdram, BILERP_DL, 0x0700_002f, TXSP);
    write_command(&mut rdram, BILERP_DL + 8, 0xdf00_0000, 0);
    let before = format!("{bilerp_rdp:?}");
    let error = decode_ops(&rdram, BILERP_DL as u32, &mut bilerp_rdp).unwrap_err();
    assert!(error.to_string().contains("requires G_OBJRM_BILERP"));
    assert_eq!(format!("{bilerp_rdp:?}"), before);

    write_command(
        &mut rdram,
        BILERP_DL,
        0x0b00_0000,
        G_OBJRM_BILERP | G_OBJRM_WIDEN,
    );
    write_command(&mut rdram, BILERP_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, BILERP_DL + 16, 0xdf00_0000, 0);
    let error = decode_ops(&rdram, BILERP_DL as u32, &mut bilerp_rdp).unwrap_err();
    assert!(error
        .to_string()
        .contains("G_OBJRM_WIDEN with filtered sampling"));
}


#[test]
fn shrink_modes_match_across_compound_rectangle_matrix_and_rotating_paths() {
    const RECT_DL: usize = 0x100;
    const RELATIVE_DL: usize = 0x120;
    const ROTATING_DL: usize = 0x148;
    const STANDALONE_DL: usize = 0x170;
    const TXSP: u32 = 0x200;
    const MATRIX: u32 = 0x240;
    const IMAGE: u32 = 0x500;
    const SETUP: usize = 0x600;
    let mut rdram = vec![0u8; 0x700];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for y in 0..4 {
            for x in 0..4 {
                let color = match (x, y) {
                    (0, _) => 0xf801,
                    (3, _) => 0x003f,
                    (_, 0) => 0x07c1,
                    (_, 3) => 0xffff,
                    _ => 0xffc1,
                };
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(IMAGE + (y * 4 + x) * 2),
                    color,
                );
            }
        }
    }
    write_block_texture(&mut rdram, TXSP, IMAGE, 1);
    write_sprite(&mut rdram, TXSP + 24, 4, 4, 0, 2);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 0);
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 10), 3);
    let mode = G_OBJRM_BILERP | G_OBJRM_SHRINKSIZE_1;
    write_command(&mut rdram, RECT_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, RECT_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, RECT_DL + 16, 0xdf00_0000, 0);
    write_command(&mut rdram, RELATIVE_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, RELATIVE_DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, RELATIVE_DL + 16, 0x0800_002f, TXSP);
    write_command(&mut rdram, RELATIVE_DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, ROTATING_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, ROTATING_DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, ROTATING_DL + 16, 0x0600_002f, TXSP);
    write_command(&mut rdram, ROTATING_DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, STANDALONE_DL, 0x0b00_0000, mode);
    write_command(&mut rdram, STANDALONE_DL + 8, 0x0500_0017, TXSP);
    write_command(&mut rdram, STANDALONE_DL + 16, 0x0100_0000, TXSP + 24);
    write_command(&mut rdram, STANDALONE_DL + 24, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, STANDALONE_DL + 32, 0xda00_0000, TXSP + 24);
    write_command(&mut rdram, STANDALONE_DL + 40, 0x0200_0000, TXSP + 24);
    write_command(&mut rdram, STANDALONE_DL + 48, 0xdf00_0000, 0);
    write_command(&mut rdram, SETUP, 0xef00_0000 | 0x0008_0cff | (2 << 12), 0);
    write_command(&mut rdram, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
    write_command(&mut rdram, SETUP + 16, 0xdf00_0000, 0);

    let mut base_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut base_rdp).unwrap();
    let rectangle_ops = decode_ops(&rdram, RECT_DL as u32, &mut base_rdp.clone()).unwrap();
    let relative_ops = decode_ops(&rdram, RELATIVE_DL as u32, &mut base_rdp.clone()).unwrap();
    let rotating_ops = decode_ops(&rdram, ROTATING_DL as u32, &mut base_rdp.clone()).unwrap();
    let standalone_ops =
        decode_ops(&rdram, STANDALONE_DL as u32, &mut base_rdp.clone()).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &rectangle_ops[0] else {
        panic!("compound rectangle must remain a rectangle")
    };
    let RenderOp::TextureRectangle(relative) = &relative_ops[0] else {
        panic!("compound matrix-relative rectangle must remain a rectangle")
    };
    assert_eq!(
        (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
        (0.0, 0.0, 3.0, 3.0)
    );
    assert_eq!((rectangle.s, rectangle.t), (0.5, 0.5));
    assert_eq!(
        (relative.ulx, relative.uly, relative.lrx, relative.lry),
        (4.0, 0.0, 7.0, 3.0)
    );
    let RenderOp::Triangle(first) = &rotating_ops[0] else {
        panic!("compound rotating sprite must lower to triangles")
    };
    assert_eq!(
        first
            .v
            .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t)),
        [
            (4.0, 0.0, 0.0, 0.0),
            (7.0, 0.0, 3.0, 0.0),
            (7.0, 3.0, 3.0, 3.0),
        ]
    );
    let RenderOp::TextureRectangle(standalone_rectangle) = &standalone_ops[0] else {
        unreachable!()
    };
    let RenderOp::TextureRectangle(standalone_relative) = &standalone_ops[1] else {
        unreachable!()
    };
    let RenderOp::Triangle(standalone_rotating) = &standalone_ops[2] else {
        unreachable!()
    };
    assert_eq!(
        (
            standalone_rectangle.ulx,
            standalone_rectangle.lrx,
            standalone_rectangle.s,
        ),
        (rectangle.ulx, rectangle.lrx, rectangle.s)
    );
    assert_eq!(
        (
            standalone_relative.ulx,
            standalone_relative.lrx,
            standalone_relative.s,
        ),
        (relative.ulx, relative.lrx, relative.s)
    );
    assert_eq!(
        standalone_rotating
            .v
            .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t)),
        first
            .v
            .map(|vertex| (vertex.x, vertex.y, vertex.s, vertex.t))
    );

    let draw = |operations: &[RenderOp]| {
        let mut framebuffer = crate::raster::Framebuffer::new(8, 4);
        framebuffer.clear(0, 0, 0, 0);
        for operation in operations {
            match operation {
                RenderOp::TextureRectangle(rectangle) => {
                    framebuffer.draw_texture_rectangle(rectangle)
                }
                RenderOp::Triangle(triangle) => framebuffer.draw_triangle(triangle),
                _ => panic!("object path emitted an unexpected operation"),
            }
        }
        framebuffer
    };
    let rectangle_fb = draw(&rectangle_ops);
    let relative_fb = draw(&relative_ops);
    let rotating_fb = draw(&rotating_ops);
    let pixel = |framebuffer: &crate::raster::Framebuffer, x: usize, y: usize| {
        let offset = (y * framebuffer.width as usize + x) * 4;
        framebuffer.pixels[offset..offset + 4].to_vec()
    };
    let triangle_sample = |operation: &RenderOp, x, y| {
        let RenderOp::Triangle(triangle) = operation else {
            unreachable!()
        };
        crate::raster::test_triangle_attribute_sample(
            triangle.v,
            triangle
                .scissor
                .unwrap_or_else(|| crate::gbi::ScissorRect::framebuffer(8, 4)),
            x,
            y,
        )
    };
    for coordinate in 0..3 {
        assert_eq!(
            triangle_sample(&rotating_ops[0], coordinate + 4, coordinate),
            (0xaf, Some((2, 3, 3)))
        );
        assert_eq!(
            triangle_sample(&rotating_ops[1], coordinate + 4, coordinate),
            (0x50, Some((4, 1, 5)))
        );
    }
    let corrected_diagonal = [[223, 32, 0, 255], [255, 255, 0, 255], [223, 223, 191, 255]];
    for y in 0..3 {
        for (x, diagonal) in corrected_diagonal.iter().enumerate() {
            let expected = pixel(&rectangle_fb, x, y);
            assert_ne!(expected, [0, 0, 0, 0]);
            assert_eq!(
                pixel(&relative_fb, x + 4, y),
                expected,
                "relative ({x},{y})"
            );
            if x == y {
                assert_eq!(pixel(&rotating_fb, x + 4, y), *diagonal);
            } else {
                assert_eq!(pixel(&rotating_fb, x + 4, y), expected);
            }
        }
    }

    write_command(
        &mut rdram,
        RECT_DL,
        0x0b00_0000,
        G_OBJRM_BILERP | G_OBJRM_SHRINKSIZE_2,
    );
    let shrink_two = decode_ops(&rdram, RECT_DL as u32, &mut base_rdp.clone()).unwrap();
    let RenderOp::TextureRectangle(shrink_two) = &shrink_two[0] else {
        unreachable!()
    };
    assert_eq!((shrink_two.lrx, shrink_two.lry), (2.0, 2.0));
    assert_eq!((shrink_two.s, shrink_two.t), (1.0, 1.0));

    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 2), 1536);
    let error = decode_ops(&rdram, RECT_DL as u32, &mut base_rdp).unwrap_err();
    assert!(error.to_string().contains("sub-quarter-pixel rounding"));
}


#[test]
fn object_st_flips_reach_rectangle_matrix_and_rotating_paths() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    const MATRIX: u32 = 0x240;
    const RECT: u32 = 0x258;
    const RECT_R: u32 = 0x270;
    const SPRITE: u32 = 0x288;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    for index in 0..8 {
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
            if index == 3 { 0x003f } else { 0xf801 },
        );
    }
    write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 20);
    write_sprite(&mut rdram, RECT, 4, 2, 0, 2);
    write_sprite(&mut rdram, RECT_R, 4, 2, 0, 2);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u8(
            fn64_runtime::RdramAddr::from_offset(RECT + 23),
            G_OBJ_FLAG_FLIPS,
        );
        view.write_u8(
            fn64_runtime::RdramAddr::from_offset(RECT_R + 23),
            G_OBJ_FLAG_FLIPT,
        );
        view.write_u8(
            fn64_runtime::RdramAddr::from_offset(SPRITE + 23),
            G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
        );
    }
    write_command(&mut rdram, DL, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 8, 0x0100_0000, RECT);
    write_command(&mut rdram, DL + 16, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, DL + 24, 0xda00_0000, RECT_R);
    write_command(&mut rdram, DL + 32, 0x0200_0000, SPRITE);
    write_command(&mut rdram, DL + 40, 0xdf00_0000, 0);

    let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
        panic!("base object must lower to a rectangle")
    };
    assert_eq!((rectangle.s, rectangle.dsdx), (3.0, -(1 << 10)));
    assert_eq!(
        rectangle.texture.as_ref().unwrap().sample(rectangle.s, 0.0),
        [0, 0, 255, 255]
    );
    let RenderOp::TextureRectangle(relative) = &operations[1] else {
        panic!("matrix-relative object must lower to a rectangle")
    };
    assert_eq!((relative.ulx, relative.uly), (4.0, 5.0));
    assert_eq!((relative.t, relative.dtdy), (1.0, -(1 << 10)));
    let RenderOp::Triangle(rotating) = &operations[2] else {
        panic!("rotating object must lower to triangles")
    };
    assert_eq!(
        rotating.v.map(|vertex| (vertex.s, vertex.t)),
        [(4.0, 2.0), (0.0, 2.0), (0.0, 0.0)]
    );
}


#[test]
fn conditional_display_lists_call_branch_and_skip_from_public_status_equation() {
    const ROOT: usize = 0x100;
    const CALLEE: usize = 0x180;
    const BRANCH: usize = 0x1c0;
    const ROOT_SPRITE: u32 = 0x240;
    const CALLEE_SPRITE: u32 = 0x258;
    const BRANCH_SPRITE: u32 = 0x270;
    let mut rdram = vec![0u8; 0x400];
    for sprite in [ROOT_SPRITE, CALLEE_SPRITE, BRANCH_SPRITE] {
        write_sprite(&mut rdram, sprite, 4, 2, 0, 2);
    }
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(CALLEE_SPRITE), 16);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(BRANCH_SPRITE), 32);
    }
    let select_pair = |rdram: &mut [u8], pc: usize, target: usize, push: u32| {
        write_command(
            rdram,
            pc,
            (u32::from(G_RDPHALF_0) << 24) | target as u32 & 0xffff,
            1,
        );
        write_command(
            rdram,
            pc + 8,
            (u32::from(G_SELECT_DL) << 24) | (push << 16) | ((target as u32 >> 16) & 0xffff),
            1,
        );
    };
    select_pair(&mut rdram, ROOT, CALLEE, 0);
    select_pair(&mut rdram, ROOT + 16, CALLEE, 0);
    write_command(&mut rdram, ROOT + 32, 0x0100_0000, ROOT_SPRITE);
    write_command(&mut rdram, ROOT + 40, 0xdf00_0000, 0);
    write_command(&mut rdram, CALLEE, 0x0100_0000, CALLEE_SPRITE);
    write_command(&mut rdram, CALLEE + 8, 0xdf00_0000, 0);

    let called = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap();
    assert_eq!(called.len(), 2, "second matching select must skip the call");
    let rectangle_x = |operation: &RenderOp| match operation {
        RenderOp::TextureRectangle(rectangle) => rectangle.ulx,
        _ => panic!("selected object lists must emit rectangles"),
    };
    assert_eq!(
        (rectangle_x(&called[0]), rectangle_x(&called[1])),
        (4.0, 0.0)
    );

    select_pair(&mut rdram, ROOT, BRANCH, 1);
    write_command(&mut rdram, BRANCH, 0x0100_0000, BRANCH_SPRITE);
    write_command(&mut rdram, BRANCH + 8, 0xdf00_0000, 0);
    let branched = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap();
    assert_eq!(branched.len(), 1, "branch must not resume the root list");
    assert_eq!(rectangle_x(&branched[0]), 8.0);

    write_command(&mut rdram, ROOT, 0x0400_0000, 0);
    let error = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap_err();
    assert!(error.to_string().contains("preceding G_RDPHALF_0"));
}
