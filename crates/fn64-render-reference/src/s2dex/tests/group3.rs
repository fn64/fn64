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
fn segment_and_general_status_writes_drive_compound_objects_and_selected_lists() {
    const ROOT: usize = 0x100;
    const CALLEE: usize = 0x180;
    const BLUE_TX: u32 = 0x200;
    const RED_TXSP: u32 = 0x218;
    const SPRITE: u32 = 0x230;
    const MATRIX: u32 = 0x248;
    const BLUE: u32 = 0x400;
    const RED: u32 = 0x410;
    const RED_STATUS: u32 = 0x55;
    let mut rdram = vec![0u8; 0x500];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for index in 0..8 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                0x003f,
            );
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                0xf801,
            );
        }
    }
    write_block_texture(&mut rdram, BLUE_TX, 0x0200_0000, 0x22);
    write_block_texture(&mut rdram, RED_TXSP, 0x0200_0010, RED_STATUS);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_object_matrix(&mut rdram, MATRIX, 16, 20, 1 << 10, 1 << 10);

    write_command(&mut rdram, ROOT, 0xdb06_0004, 0x200);
    write_command(&mut rdram, ROOT + 8, 0xdb06_0008, 0x400);
    write_command(&mut rdram, ROOT + 16, 0xdb06_000c, 0x100);
    write_command(&mut rdram, ROOT + 24, 0x0500_0017, 0x0100_0000);
    write_command(&mut rdram, ROOT + 32, 0xdc00_0017, 0x0100_0048);
    write_command(&mut rdram, ROOT + 40, 0xdb08_0000, RED_STATUS);
    write_command(&mut rdram, ROOT + 48, 0x0800_002f, 0x0100_0018);
    write_command(&mut rdram, ROOT + 56, 0xe404_0080, 1);
    write_command(&mut rdram, ROOT + 64, 0x0400_0300, 1);
    write_command(&mut rdram, ROOT + 72, 0xdf00_0000, 0);
    write_command(&mut rdram, CALLEE, 0x0100_0000, 0x0100_0030);
    write_command(&mut rdram, CALLEE + 8, 0xdf00_0000, 0);

    let operations = decode_ops(&rdram, ROOT as u32, &mut RdpDecodeState::default()).unwrap();
    assert_eq!(operations.len(), 2);
    let RenderOp::TextureRectangle(relative) = &operations[0] else {
        panic!("segmented compound rectangle must remain typed")
    };
    let RenderOp::TextureRectangle(callee) = &operations[1] else {
        panic!("segmented selected list must emit its rectangle")
    };
    assert_eq!((relative.ulx, relative.uly), (4.0, 5.0));
    assert_eq!((callee.ulx, callee.uly), (0.0, 0.0));
    for rectangle in [relative, callee] {
        assert_eq!(
            rectangle.texture.as_ref().unwrap().sample(0.0, 0.0),
            [0, 0, 255, 255],
            "G_MW_GENSTAT must make the red compound reload a cache hit"
        );
    }
}


#[test]
fn segment_table_resolves_background_payload_and_image_together() {
    const DL: usize = 0x100;
    const BG: u32 = 0x200;
    const MODE: usize = 0x300;
    const IMAGE: u32 = 0x1000;
    let mut rdram = vec![0u8; 0x1100];
    write_background_common(&mut rdram, BG, 0x0200_0000, 4, 2, 4, 2, G_BGLT_LOADTILE, 2);
    write_copy_background_init(&mut rdram, BG, 4, 4, G_BGLT_LOADTILE, 2);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for index in 0..8 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                0x07c1,
            );
        }
    }
    write_command(&mut rdram, DL, 0xdb06_0004, BG);
    write_command(&mut rdram, DL + 8, 0xdb06_0008, IMAGE);
    write_command(&mut rdram, DL + 16, 0x0a00_0000, 0x0100_0000);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, MODE, 0xef00_0000 | (2 << 20), 0);
    write_command(&mut rdram, MODE + 8, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
        panic!("segmented background must remain a texture rectangle")
    };
    assert_eq!(
        rectangle.texture.as_ref().unwrap().sample(0.0, 0.0),
        [0, 255, 0, 255]
    );
}


#[test]
fn legacy_s2dex_move_word_packing_shares_segment_and_status_mechanisms() {
    const DL: usize = 0x100;
    const BLUE_TX: u32 = 0x200;
    const RED_TXSP: u32 = 0x218;
    const SPRITE: u32 = 0x230;
    const BLUE: u32 = 0x400;
    const RED: u32 = 0x410;
    const RED_STATUS: u32 = 0x55;
    let mut rdram = vec![0u8; 0x500];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for index in 0..8 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                0x003f,
            );
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                0xf801,
            );
        }
    }
    write_block_texture(&mut rdram, BLUE_TX, 0x0200_0000, 0x22);
    write_block_texture(&mut rdram, RED_TXSP, 0x0200_0010, RED_STATUS);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);

    // Legacy gMoveWd packs offset in bits 23:8 and index in bits 7:0.
    write_command(&mut rdram, DL, 0xbc00_0406, 0x200);
    write_command(&mut rdram, DL + 8, 0xbc00_0806, 0x400);
    write_command(&mut rdram, DL + 16, 0xc100_0017, 0x0100_0000);
    write_command(&mut rdram, DL + 24, 0xbc00_0008, RED_STATUS);
    write_command(&mut rdram, DL + 32, 0xc300_002f, 0x0100_0018);
    write_command(&mut rdram, DL + 40, 0x0300_0000, 0x0100_0030);
    write_command(&mut rdram, DL + 48, 0xb800_0000, 0);

    let operations = decode_ops_for_family(
        &rdram,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex,
    )
    .unwrap();
    assert_eq!(operations.len(), 2);
    for operation in operations {
        let RenderOp::TextureRectangle(rectangle) = operation else {
            panic!("legacy object rectangles must use the shared typed path")
        };
        assert_eq!(
            rectangle.texture.as_ref().unwrap().sample(0.0, 0.0),
            [0, 0, 255, 255],
            "legacy G_MW_GENSTAT must make the red reload a status hit"
        );
    }
}


#[test]
fn colliding_opcode_requires_an_admitted_wire_family() {
    const DL: usize = 0x100;
    const SPRITE: u32 = 0x200;
    let mut rdram = vec![0u8; 0x300];
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

    let operations = decode_ops_for_family(
        &rdram,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex2,
    )
    .unwrap();
    assert_eq!(operations.len(), 1, "S2DEX2 byte 0x01 is ObjRectangle");

    let error = decode_ops_for_family(
        &rdram,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex,
    )
    .unwrap_err();
    assert!(error.to_string().contains("G_BG_1CYC"));
}


#[test]
fn digest_catalog_reports_exact_admitted_wire_families() {
    let text1 = [1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let text2 = [2; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let mut catalog = UcodeCatalog::default();
    assert!(catalog.supported_ucodes().is_empty());
    catalog.admit_text_for(S2dexWireFamily::S2dex, &text1);
    assert_eq!(catalog.supported_ucodes(), &[UcodeId::S2dex]);
    catalog.admit_text(&text2);
    assert_eq!(catalog.supported_ucodes(), SUPPORTED);
    assert_eq!(
        catalog.require_text(&text1).unwrap(),
        S2dexWireFamily::S2dex
    );
    assert_eq!(
        catalog.require_text(&text2).unwrap(),
        S2dexWireFamily::S2dex2
    );
}


#[test]
#[should_panic(expected = "one S2DEX microcode digest cannot identify two wire families")]
fn digest_catalog_rejects_conflicting_wire_family_metadata() {
    let text = [3; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let mut catalog = UcodeCatalog::default();
    catalog.admit_text_for(S2dexWireFamily::S2dex, &text);
    catalog.admit_text_for(S2dexWireFamily::S2dex2, &text);
}


#[test]
fn copy_background_window_partitions_exhaustively_preserve_wrapped_sample_identity() {
    for image_width in 1..=8 {
        for image_height in 1..=6 {
            for frame_width in 1..=image_width {
                for frame_height in 1..=image_height {
                    for image_x in 0..image_width {
                        for image_y in 0..image_height {
                            for reverse_s in [false, true] {
                                for max_source_rows in 1..=image_height {
                                    let window = BackgroundCopyWindow::new(
                                        image_width,
                                        image_height,
                                        frame_width,
                                        frame_height,
                                        image_x,
                                        image_y,
                                        reverse_s,
                                        max_source_rows,
                                        "G_BG_COPY",
                                    )
                                    .unwrap();
                                    let mut observed =
                                        vec![None; (frame_width * frame_height) as usize];
                                    for slice in window.slices() {
                                        assert!(slice.output_x_start < slice.output_x_end);
                                        assert!(slice.output_y_start < slice.output_y_end);
                                        assert!(slice.source_x_start < slice.source_x_end);
                                        assert!(slice.source_y_start < slice.source_y_end);
                                        assert!(slice.output_x_end <= frame_width);
                                        assert!(slice.output_y_end <= frame_height);
                                        assert!(slice.source_x_end <= image_width);
                                        assert!(slice.source_y_end <= image_height);
                                        assert!(
                                            slice.source_y_end - slice.source_y_start
                                                <= max_source_rows
                                        );
                                        assert_eq!(
                                            slice.output_x_end - slice.output_x_start,
                                            slice.source_x_end - slice.source_x_start
                                        );
                                        assert_eq!(
                                            slice.output_y_end - slice.output_y_start,
                                            slice.source_y_end - slice.source_y_start
                                        );
                                        for output_y in slice.output_y_start..slice.output_y_end
                                        {
                                            for output_x in
                                                slice.output_x_start..slice.output_x_end
                                            {
                                                let local_x = output_x - slice.output_x_start;
                                                let source_x = if slice.reverse_s {
                                                    slice.source_x_end - 1 - local_x
                                                } else {
                                                    slice.source_x_start + local_x
                                                };
                                                let source_y = slice.source_y_start + output_y
                                                    - slice.output_y_start;
                                                let slot = (output_y * frame_width + output_x)
                                                    as usize;
                                                assert_eq!(observed[slot], None);
                                                observed[slot] = Some((source_x, source_y));
                                            }
                                        }
                                    }

                                    for output_y in 0..frame_height {
                                        for output_x in 0..frame_width {
                                            let mapped_x = if reverse_s {
                                                frame_width - 1 - output_x
                                            } else {
                                                output_x
                                            };
                                            let expected_linear = (((image_y + output_y)
                                                % image_height)
                                                * image_width
                                                + image_x
                                                + mapped_x)
                                                % (image_width * image_height);
                                            let expected = (
                                                expected_linear % image_width,
                                                expected_linear / image_width,
                                            );
                                            assert_eq!(
                                                observed[(output_y * frame_width + output_x)
                                                    as usize],
                                                Some(expected),
                                                "image={image_width}x{image_height} frame={frame_width}x{frame_height} origin=({image_x},{image_y}) reverse={reverse_s} rows={max_source_rows} output=({output_x},{output_y})"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}


#[test]
fn copy_background_window_rejects_non_public_geometry_loudly() {
    for (arguments, expected) in [
        ((0, 2, 1, 1, 0, 0, 1), "dimensions must all be nonzero"),
        ((2, 2, 3, 1, 0, 0, 1), "transfer frame 3x1 exceeds"),
        ((2, 2, 1, 1, 2, 0, 1), "origin (2,0) must be wrapped"),
        ((2, 2, 1, 1, 0, 0, 0), "admits zero source rows"),
    ] {
        let (iw, ih, fw, fh, ix, iy, rows) = arguments;
        let error = BackgroundCopyWindow::new(iw, ih, fw, fh, ix, iy, false, rows, "G_BG_COPY")
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}


#[test]
fn scaled_background_point_window_exhaustively_preserves_fixed_point_identity() {
    let scales = [1, 511, 1024, 1536, 3072];
    let mut configurations = 0usize;
    for image_width in 1..=5u32 {
        for image_height in 1..=4u32 {
            for frame_width in 1..=7u32 {
                for frame_height in 1..=4u32 {
                    for image_x_5 in [0, image_width * 16, image_width * 32 - 1] {
                        for image_y in 0..image_height {
                            for scale_w_10 in scales {
                                for scale_h_10 in [1, 1024, 1536, 3072] {
                                    for reverse_s in [false, true] {
                                        let window = ScaledBackgroundWindow::new(
                                            image_width,
                                            image_height,
                                            frame_width,
                                            frame_height,
                                            image_x_5 as u16,
                                            (image_y * 32) as u16,
                                            scale_w_10,
                                            scale_h_10,
                                            reverse_s,
                                            -64,
                                            "G_BG_1CYC",
                                        )
                                        .unwrap();
                                        let slices = window
                                            .slices(
                                                BackgroundFilterFootprint::Point,
                                                "G_BG_1CYC",
                                            )
                                            .unwrap();
                                        let mut observed =
                                            vec![None; (frame_width * frame_height) as usize];
                                        for slice in slices {
                                            assert!(slice.output_x_start < slice.output_x_end);
                                            assert!(slice.output_x_end <= frame_width);
                                            assert!(slice.output_y < frame_height);
                                            assert!(slice.source_x_start < slice.source_x_end);
                                            assert!(slice.source_x_end <= image_width);
                                            assert!(slice.source_y < image_height);
                                            for output_x in
                                                slice.output_x_start..slice.output_x_end
                                            {
                                                let local_x = output_x - slice.output_x_start;
                                                let source_s_10 = i64::from(slice.s_start_10)
                                                    + i64::from(local_x)
                                                        * i64::from(slice.dsdx_10);
                                                let source_x = i64::from(slice.source_x_start)
                                                    + source_s_10.div_euclid(1024);
                                                let source_y = i64::from(slice.source_y)
                                                    + i64::from(slice.t_start_10)
                                                        .div_euclid(1024);
                                                let slot = (slice.output_y * frame_width
                                                    + output_x)
                                                    as usize;
                                                assert_eq!(observed[slot], None);
                                                observed[slot] =
                                                    Some((source_x as u32, source_y as u32));
                                            }
                                        }

                                        let row_extent_10 = image_width * 1024;
                                        for output_y in 0..frame_height {
                                            for output_x in 0..frame_width {
                                                let mapped_x = if reverse_s {
                                                    frame_width - 1 - output_x
                                                } else {
                                                    output_x
                                                };
                                                let source_s_10 = image_x_5 * 32
                                                    + mapped_x * u32::from(scale_w_10);
                                                let row_carry = source_s_10 / row_extent_10;
                                                let source_x =
                                                    source_s_10 % row_extent_10 / 1024;
                                                let source_y_10 = (image_y * 1024
                                                    + output_y * u32::from(scale_h_10)
                                                    + row_carry * 1024)
                                                    % (image_height * 1024);
                                                assert_eq!(
                                                    observed[(output_y * frame_width + output_x)
                                                        as usize],
                                                    Some((source_x, source_y_10 / 1024)),
                                                    "image={image_width}x{image_height} frame={frame_width}x{frame_height} imageX={image_x_5} scale=({scale_w_10},{scale_h_10}) reverse={reverse_s} output=({output_x},{output_y})"
                                                );
                                            }
                                        }
                                        configurations += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert_eq!(configurations, 168_000);
}


#[test]
fn scaled_background_window_keeps_unpublished_footprints_loud() {
    let valid =
        || ScaledBackgroundWindow::new(4, 3, 5, 4, 1, 32, 1536, 1024, false, -64, "G_BG_1CYC");
    assert!(
        valid().is_ok(),
        "fractional imageX and distinct imageYorig are public"
    );
    let error = valid()
        .unwrap()
        .slices(BackgroundFilterFootprint::Bilinear, "G_BG_1CYC")
        .unwrap_err();
    assert!(
        error.to_string().contains("bilinear scaled-background"),
        "{error}"
    );

    for (arguments, expected) in [
        ((4, 3, 5, 4, 128, 32, 1536, 1024, -64), "must be wrapped"),
        ((4, 3, 5, 4, 0, 1, 1536, 1024, -64), "vertical subpixel"),
        ((4, 3, 5, 4, 0, 32, 0, 1024, -64), "nonzero RDP"),
        ((4, 3, 5, 4, 0, 32, 1536, 1024, 1), "sub-texel strip-origin"),
    ] {
        let (iw, ih, fw, fh, ix, iy, sw, sh, origin) = arguments;
        let error = ScaledBackgroundWindow::new(
            iw,
            ih,
            fw,
            fh,
            ix,
            iy,
            sw,
            sh,
            false,
            origin,
            "G_BG_1CYC",
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}


#[test]
fn admitted_s2dex_families_render_wrapped_copy_background_windows_identically() {
    const DL: usize = 0x100;
    const BG: u32 = 0x200;
    const MODE: usize = 0x300;
    const IMAGE: u32 = 0x1000;
    const COLORS: [u16; 12] = [
        0xf801, 0x07c1, 0x003f, 0xffff, 0xffc1, 0xf83f, 0x07ff, 0x0001, 0xf801, 0x07c1, 0x003f,
        0xffff,
    ];
    let rgba = |color: u16| {
        let expand = |value: u16| ((value << 3) | (value >> 2)) as u8;
        [
            expand((color >> 11) & 0x1f),
            expand((color >> 6) & 0x1f),
            expand((color >> 1) & 0x1f),
            if color & 1 != 0 { 255 } else { 0 },
        ]
    };

    for (family_index, family) in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2]
        .into_iter()
        .enumerate()
    {
        let text = vec![family_index as u8 + 1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = UcodeCatalog::default();
        catalog.admit_text_for(family, &text);
        let selected_family = catalog.require_text(&text).unwrap();
        assert_eq!(selected_family, family);
        for (image_load, load_name) in [
            (G_BGLT_LOADTILE, "LoadTile"),
            (G_BGLT_LOADBLOCK, "LoadBlock"),
        ] {
            for flipped in [false, true] {
                let mut rdram = vec![0u8; 0x1100];
                write_background_common(&mut rdram, BG, IMAGE, 4, 3, 3, 3, image_load, 2);
                write_copy_background_init(&mut rdram, BG, 4, 3, image_load, 2);
                write_background_window(&mut rdram, BG, 3, 2, flipped);
                let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                for (index, color) in COLORS.into_iter().enumerate() {
                    view.write_u16(
                        fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                        color,
                    );
                }
                let (background_opcode, end_opcode) = match family {
                    S2dexWireFamily::S2dex => (S2DEX_G_BG_COPY, S2DEX_G_ENDDL),
                    S2dexWireFamily::S2dex2 => (G_BG_COPY, G_ENDDL),
                };
                write_command(&mut rdram, DL, u32::from(background_opcode) << 24, BG);
                write_command(&mut rdram, DL + 8, u32::from(end_opcode) << 24, 0);
                write_command(&mut rdram, MODE, 0xef00_0000 | (2 << 20), 0);
                write_command(&mut rdram, MODE + 8, u32::from(G_ENDDL) << 24, 0);

                let mut rdp = RdpDecodeState::default();
                crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp)
                    .unwrap();
                let operations =
                    decode_ops_for_family(&rdram, DL as u32, &mut rdp, selected_family)
                        .unwrap();
                let mut framebuffer = crate::raster::Framebuffer::new(3, 3);
                for operation in &operations {
                    let RenderOp::TextureRectangle(rectangle) = operation else {
                        panic!("copy window must lower only to texture rectangles");
                    };
                    framebuffer.draw_copy_texture_rectangle(rectangle);
                }
                for output_y in 0..3u32 {
                    for output_x in 0..3u32 {
                        let mapped_x = if flipped { 2 - output_x } else { output_x };
                        let source = (((2 + output_y) % 3) * 4 + 3 + mapped_x) % 12;
                        let offset = ((output_y * 3 + output_x) * 4) as usize;
                        assert_eq!(
                        framebuffer.pixels[offset..offset + 4],
                        rgba(COLORS[source as usize]),
                        "family={family:?} load={load_name} flipped={flipped} output=({output_x},{output_y}) source={source}"
                    );
                    }
                }
            }
        }
    }
}


#[test]
fn copy_background_reuses_bounded_scratch_for_every_wire_and_loader_remainder() {
    const DL: usize = 0x100;
    const BG: u32 = 0x200;
    const MODE: usize = 0x300;
    const WIDTH: u16 = 320;
    const HEIGHT: u16 = 8;
    const IMAGE_BYTES: usize = WIDTH as usize * HEIGHT as usize * 2;
    const IMAGE: u32 = (PHYSICAL_RDRAM_BYTES - IMAGE_BYTES) as u32;
    assert_eq!(BackgroundScratch::new().bytes.len(), 8192 + 48);
    assert!(BACKGROUND_SCRATCH_BYTES < IMAGE as usize);
    for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
        for (image_load, load_name) in [
            (G_BGLT_LOADTILE, "LoadTile"),
            (G_BGLT_LOADBLOCK, "LoadBlock"),
        ] {
            let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES];
            write_background_common(
                &mut rdram, BG, IMAGE, WIDTH, HEIGHT, WIDTH, HEIGHT, image_load, 2,
            );
            write_copy_background_init(&mut rdram, BG, WIDTH, WIDTH, image_load, 2);
            {
                let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                for y in 0..HEIGHT {
                    let color = if y < 6 { 0xf801 } else { 0x003f };
                    for x in 0..WIDTH {
                        view.write_u16(
                            fn64_runtime::RdramAddr::from_offset(
                                IMAGE + (u32::from(y) * u32::from(WIDTH) + u32::from(x)) * 2,
                            ),
                            color,
                        );
                    }
                }
            }
            let (background_opcode, end_opcode) = match family {
                S2dexWireFamily::S2dex => (S2DEX_G_BG_COPY, S2DEX_G_ENDDL),
                S2dexWireFamily::S2dex2 => (G_BG_COPY, G_ENDDL),
            };
            write_command(&mut rdram, DL, u32::from(background_opcode) << 24, BG);
            write_command(&mut rdram, DL + 8, u32::from(end_opcode) << 24, 0);
            write_command(&mut rdram, MODE, 0xef00_0000 | (2 << 20), 0);
            write_command(&mut rdram, MODE + 8, u32::from(G_ENDDL) << 24, 0);

            let mut rdp = RdpDecodeState::default();
            crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
            let operations =
                decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap();
            assert_eq!(
                operations.len(),
                2,
                "family={family:?} load={load_name}: six TMEM rows plus two-row remainder"
            );
            let rectangles = operations
                .iter()
                .map(|operation| match operation {
                    RenderOp::TextureRectangle(rectangle) => rectangle,
                    _ => panic!("background must lower only to texture rectangles"),
                })
                .collect::<Vec<_>>();
            assert_eq!(
                (
                    rectangles[0].ulx,
                    rectangles[0].uly,
                    rectangles[0].lrx,
                    rectangles[0].lry
                ),
                (0.0, 0.0, 319.0, 5.0),
                "family={family:?} load={load_name} first strip"
            );
            assert_eq!(
                (
                    rectangles[1].ulx,
                    rectangles[1].uly,
                    rectangles[1].lrx,
                    rectangles[1].lry
                ),
                (0.0, 6.0, 319.0, 7.0),
                "family={family:?} load={load_name} remainder strip"
            );

            let mut framebuffer = crate::raster::Framebuffer::new(WIDTH.into(), HEIGHT.into());
            framebuffer.clear(0, 0, 0, 0);
            for rectangle in rectangles {
                framebuffer.draw_copy_texture_rectangle(rectangle);
            }
            let pixel = |x: usize, y: usize| {
                let offset = (y * usize::from(WIDTH) + x) * 4;
                &framebuffer.pixels[offset..offset + 4]
            };
            assert_eq!(
                pixel(0, 0),
                [255, 0, 0, 255],
                "family={family:?} load={load_name} first pixel"
            );
            assert_eq!(
                pixel(319, 5),
                [255, 0, 0, 255],
                "family={family:?} load={load_name} final full-strip pixel"
            );
            assert_eq!(
                pixel(0, 6),
                [0, 0, 255, 255],
                "family={family:?} load={load_name} first remainder pixel"
            );
            assert_eq!(
                pixel(319, 7),
                [0, 0, 255, 255],
                "family={family:?} load={load_name} final remainder pixel"
            );
        }
    }
}


#[test]
fn scaled_background_maps_source_gradient_and_is_transactional() {
    const DL: usize = 0x100;
    const BG: u32 = 0x200;
    const IMAGE: u32 = 0x1000;
    let mut rdram = vec![0u8; 0x1100];
    write_background_common(&mut rdram, BG, IMAGE, 8, 4, 4, 4, G_BGLT_LOADBLOCK, 2);
    write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 0);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for index in 0..32 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                if index & 1 == 0 { 0xf801 } else { 0x003f },
            );
        }
    }
    write_command(&mut rdram, DL, 0x0900_0000, BG);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    assert_eq!(operations.len(), 4);
    let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
        panic!("scaled background must lower to a texture rectangle");
    };
    assert_eq!(
        (rectangle.ulx, rectangle.uly, rectangle.lrx, rectangle.lry),
        (0.0, 0.0, 4.0, 1.0)
    );
    assert_eq!((rectangle.dsdx, rectangle.dtdy), (2 << 10, 1 << 10));
    let texture = rectangle.texture.as_ref().unwrap();
    assert_eq!(texture.sample(0.0, 0.0), [255, 0, 0, 255]);
    assert_eq!(texture.sample(1.0, 0.0), [0, 0, 255, 255]);

    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(BG + 4), 1);
    let quarter_pixel = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(quarter_pixel) = &quarter_pixel[0] else {
        unreachable!()
    };
    assert_eq!((quarter_pixel.ulx, quarter_pixel.lrx), (0.25, 4.25));
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(BG + 4), 0);

    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
        fn64_runtime::RdramAddr::from_offset(BG + 26),
        G_BG_FLAG_FLIPS,
    );
    let flipped = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(flipped) = &flipped[0] else {
        panic!("flipped background must remain a texture rectangle");
    };
    assert_eq!((flipped.s, flipped.dsdx), (6.0, -(2 << 10)));

    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(BG + 26), 0);
    write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 32);
    let distinct_origin =
        decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    assert_eq!(distinct_origin.len(), operations.len());
    for (left, right) in operations.iter().zip(&distinct_origin) {
        let (RenderOp::TextureRectangle(left), RenderOp::TextureRectangle(right)) =
            (left, right)
        else {
            panic!("scaled backgrounds must lower only to texture rectangles")
        };
        assert_eq!(
            (left.ulx, left.uly, left.lrx, left.lry, left.s, left.t),
            (right.ulx, right.uly, right.lrx, right.lry, right.s, right.t)
        );
    }

    write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 1);
    let mut fresh = RdpDecodeState::default();
    let before = format!("{fresh:?}");
    let error = decode_ops(&rdram, DL as u32, &mut fresh).unwrap_err();
    assert!(error.to_string().contains("sub-texel strip-origin"));
    assert_eq!(format!("{fresh:?}"), before);
    write_scale_background_tail(&mut rdram, BG, 2 << 10, 1 << 10, 0);

    write_command(&mut rdram, 0x300, 0xef00_0000 | (2 << 12), 0);
    write_command(&mut rdram, 0x308, 0xdf00_0000, 0);
    let mut bilinear = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, 0x300, &mut bilinear).unwrap();
    let before = format!("{bilinear:?}");
    let error = decode_ops(&rdram, DL as u32, &mut bilinear).unwrap_err();
    assert!(
        error.to_string().contains("bilinear scaled-background"),
        "{error}"
    );
    assert_eq!(format!("{bilinear:?}"), before);

    write_command(&mut rdram, DL + 8, 0x0400_0000, 0);
    let before = format!("{fresh:?}");
    let error = decode_ops(&rdram, DL as u32, &mut fresh).unwrap_err();
    assert!(error.to_string().contains("G_SELECT_DL"));
    assert_eq!(format!("{fresh:?}"), before);
}


#[test]
fn admitted_s2dex_families_load_scaled_background_wrapped_point_windows_identically() {
    const DL: usize = 0x100;
    const BG: u32 = 0x200;
    const IMAGE: u32 = 0x1000;
    const COLORS: [u16; 12] = [
        0x0801, 0x1001, 0x1801, 0x2001, 0x2801, 0x3001, 0x3801, 0x4001, 0x4801, 0x5001, 0x5801,
        0x6001,
    ];
    let rgba = |color: u16| {
        let expand = |value: u16| ((value << 3) | (value >> 2)) as u8;
        [
            expand((color >> 11) & 0x1f),
            expand((color >> 6) & 0x1f),
            expand((color >> 1) & 0x1f),
            255,
        ]
    };

    for (family_index, family) in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2]
        .into_iter()
        .enumerate()
    {
        let text = vec![family_index as u8 + 9; fn64_runtime::RSP_MEMORY_BANK_SIZE];
        let mut catalog = UcodeCatalog::default();
        catalog.admit_text_for(family, &text);
        let selected_family = catalog.require_text(&text).unwrap();
        for image_load in [G_BGLT_LOADTILE, G_BGLT_LOADBLOCK] {
            for flipped in [false, true] {
                for image_y_origin in [-64, 64, 128] {
                    let mut rdram = vec![0u8; 0x1100];
                    write_background_common(&mut rdram, BG, IMAGE, 4, 3, 5, 4, image_load, 2);
                    {
                        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                        let base = fn64_runtime::RdramAddr::from_offset(BG);
                        view.write_u16(base, 3 * 32 + 16);
                        view.write_u16(base.checked_add(8).unwrap(), 2 * 32);
                        view.write_u16(
                            base.checked_add(26).unwrap(),
                            if flipped { G_BG_FLAG_FLIPS } else { 0 },
                        );
                        for (index, color) in COLORS.into_iter().enumerate() {
                            view.write_u16(
                                fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                                color,
                            );
                        }
                    }
                    write_scale_background_tail(&mut rdram, BG, 1536, 1536, image_y_origin);
                    let (background_opcode, end_opcode) = match family {
                        S2dexWireFamily::S2dex => (S2DEX_G_BG_1CYC, S2DEX_G_ENDDL),
                        S2dexWireFamily::S2dex2 => (G_BG_1CYC, G_ENDDL),
                    };
                    write_command(&mut rdram, DL, u32::from(background_opcode) << 24, BG);
                    write_command(&mut rdram, DL + 8, u32::from(end_opcode) << 24, 0);

                    let operations = decode_ops_for_family(
                        &rdram,
                        DL as u32,
                        &mut RdpDecodeState::default(),
                        selected_family,
                    )
                    .unwrap();
                    for output_y in 0..4u32 {
                        for output_x in 0..5u32 {
                            let rectangle = operations
                                .iter()
                                .find_map(|operation| match operation {
                                    RenderOp::TextureRectangle(rectangle)
                                        if rectangle.ulx <= output_x as f32
                                            && (output_x as f32) < rectangle.lrx
                                            && rectangle.uly <= output_y as f32
                                            && (output_y as f32) < rectangle.lry =>
                                    {
                                        Some(rectangle)
                                    }
                                    _ => None,
                                })
                                .expect("scaled slices cover each output pixel exactly once");
                            let local_x = output_x as f32 - rectangle.ulx;
                            let actual = rectangle.texture.as_ref().unwrap().sample(
                                rectangle.s + local_x * f32::from(rectangle.dsdx) / 1024.0,
                                rectangle.t,
                            );
                            let mapped_x = if flipped { 4 - output_x } else { output_x };
                            let source_s_10 = (3 * 32 + 16) * 32 + mapped_x * 1536;
                            let row_carry = source_s_10 / (4 * 1024);
                            let source_x = source_s_10 % (4 * 1024) / 1024;
                            let source_y = (2 * 1024 + output_y * 1536 + row_carry * 1024)
                                % (3 * 1024)
                                / 1024;
                            assert_eq!(
                                actual,
                                rgba(COLORS[(source_y * 4 + source_x) as usize]),
                                "family={family:?} load={image_load:#06x} flipped={flipped} imageYorig={image_y_origin} output=({output_x},{output_y}) source=({source_x},{source_y})"
                            );
                        }
                    }
                }
            }
        }
    }
}


#[test]
fn background_image_range_cannot_escape_physical_rdram() {
    const DL: usize = 0x100;
    const BG: u32 = 0x200;
    let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES + 16];
    write_background_common(
        &mut rdram,
        BG,
        PHYSICAL_RDRAM_BYTES as u32,
        4,
        2,
        4,
        2,
        G_BGLT_LOADTILE,
        2,
    );
    write_copy_background_init(&mut rdram, BG, 4, 4, G_BGLT_LOADTILE, 2);
    write_command(&mut rdram, DL, 0x0a00_0000, BG);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

    let error = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap_err();
    assert!(error.to_string().contains("exceeds physical/backed RDRAM"));
}


#[test]
fn tlut_and_ci4_tile_loads_feed_object_rectangle() {
    const DL: usize = 0x100;
    const TLUT: u32 = 0x200;
    const TX: u32 = 0x218;
    const SPRITE: u32 = 0x230;
    const PALETTE: u32 = 0x400;
    const IMAGE: u32 = 0x500;
    const MODE: usize = 0x600;
    let mut rdram = vec![0u8; 0x700];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for index in 0..16 {
            let color = if index == 1 { 0xf801 } else { 0x0001 };
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(PALETTE + index * 2),
                color,
            );
        }
        view.write_u8(fn64_runtime::RdramAddr::from_offset(IMAGE), 0x10);
        for offset in 1..8 {
            view.write_u8(fn64_runtime::RdramAddr::from_offset(IMAGE + offset), 0);
        }
    }
    write_tlut_texture(&mut rdram, TLUT, PALETTE);
    write_tile_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_sprite(&mut rdram, SPRITE, 16, 1, 2, 0);
    write_command(&mut rdram, DL, 0x0500_0017, TLUT);
    write_command(&mut rdram, DL + 8, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, MODE, 0xef00_0000 | 0x0008_0cff | (2 << 14), 0);
    write_command(&mut rdram, MODE + 8, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
        [255, 0, 0, 255]
    );
}


#[test]
fn single_tlut_entry_copies_its_complete_native_storage_word() {
    const DL: usize = 0x100;
    const TLUT: u32 = 0x200;
    const TX: u32 = 0x218;
    const SPRITE: u32 = 0x230;
    const PALETTE: u32 = 0x400;
    const IMAGE: u32 = 0x500;
    const MODE: usize = 0x600;
    let mut rdram = vec![0u8; 0x700];
    write_tlut_texture(&mut rdram, TLUT, PALETTE);
    write_tile_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_sprite(&mut rdram, SPRITE, 16, 1, 2, 0);
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(TLUT + 10), 0);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(PALETTE), 0xf801);
    }
    write_command(&mut rdram, DL, 0x0500_0017, TLUT);
    write_command(&mut rdram, DL + 8, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
    write_command(&mut rdram, MODE, 0xef00_0000 | 0x0008_0cff | (2 << 14), 0);
    write_command(&mut rdram, MODE + 8, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
        [255, 0, 0, 255]
    );
}


#[test]
fn object_status_match_skips_redundant_texture_load() {
    const DL: usize = 0x100;
    const TX_RED: u32 = 0x200;
    const TX_BLUE: u32 = 0x218;
    const SPRITE: u32 = 0x230;
    const RED: u32 = 0x400;
    const BLUE: u32 = 0x500;
    let mut rdram = vec![0u8; 0x600];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for index in 0..8 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(RED + index * 2),
                0xf801,
            );
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(BLUE + index * 2),
                0x003f,
            );
        }
    }
    write_block_texture(&mut rdram, TX_RED, RED, 0x1234_5678);
    write_block_texture(&mut rdram, TX_BLUE, BLUE, 0x1234_5678);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0500_0017, TX_RED);
    write_command(&mut rdram, DL + 8, 0x0500_0017, TX_BLUE);
    write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
        [255, 0, 0, 255],
        "matching (Status & mask) == flag must skip the blue reload"
    );
}


#[test]
fn rejected_tail_and_compound_without_matrix_are_transactional_and_named() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x200;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0500_0017, TXSP);
    write_command(&mut rdram, DL + 8, 0x0900_0000, 0);
    let mut rdp = RdpDecodeState::default();
    let before = format!("{rdp:?}");
    let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
    assert!(error.to_string().contains("G_BG_1CYC"));
    assert_eq!(format!("{rdp:?}"), before);

    write_command(
        &mut rdram,
        DL,
        (u32::from(G_OBJ_LDTX_SPRITE) << 24) | 47,
        TXSP,
    );
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);
    let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
    assert!(error.to_string().contains("G_OBJ_LDTX_SPRITE"));
    assert!(error.to_string().contains("texture load was not applied"));
    assert_eq!(format!("{rdp:?}"), before);

    write_command(&mut rdram, DL, 0x0800_002f, TXSP);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);
    let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
    assert!(error
        .to_string()
        .contains("requires a preceding G_OBJ_MOVEMEM"));
    assert!(error.to_string().contains("texture load was not applied"));
    assert_eq!(format!("{rdp:?}"), before);
}
