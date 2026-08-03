// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

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
fn loadtx_rect_block_loads_before_rectangle_binding() {
    const DL: usize = 0x100;
    const TXSP: u32 = 0x200;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    let pixels = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in pixels.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                pixel,
            );
        }
    }
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0700_002f, TXSP);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let sprite = read_object_sprite(&rdram, TXSP + 24, "test").unwrap();
    let RenderOp::TextureRectangle(before_load) = rdp.clone().object_rectangle(sprite).unwrap()
    else {
        panic!("object rectangle must lower to a texture rectangle");
    };
    assert!(
        before_load.texture.is_none(),
        "fresh RDP state must not already contain the compound texture"
    );
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    assert_eq!(operations.len(), 1);
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
        [255, 0, 0, 255]
    );
    assert_eq!(
        texture.sample_rdp(0.0, 1.0, other_mode, ConvertState::default()),
        [0, 255, 255, 255],
        "LoadBlock DXT row exchange must survive the object lowering"
    );
}


#[test]
fn alias_backed_rdram_uses_only_the_required_physical_prefix() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    const SPRITE: u32 = 0x240;
    const IMAGE: u32 = 0x400;
    let mut physical = vec![0u8; 0x600];
    for index in 0..8 {
        fn64_runtime::RdramViewMut::from_storage(&mut physical).write_u16(
            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
            0xf801,
        );
    }
    write_block_texture(&mut physical, TX, IMAGE, IMAGE);
    write_sprite(&mut physical, SPRITE, 4, 2, 0, 2);
    write_command(&mut physical, DL, 0x0500_0017, TX);
    write_command(&mut physical, DL + 8, 0x0100_0000, SPRITE);
    write_command(&mut physical, DL + 16, 0xdf00_0000, 0);

    let mut alias_backing = fn64_runtime::Rdram::new_with_mmio(PHYSICAL_RDRAM_BYTES);
    alias_backing.write_bytes(0, &physical);
    assert!(
        alias_backing.len() > 0x0100_0000,
        "the regression requires a generated-C alias backing larger than the raw decoder's 24-bit command space"
    );
    let rdram = alias_backing.read_bytes(0, alias_backing.len());
    let mut rdp = RdpDecodeState::default();
    let operations = decode_ops(rdram, DL as u32, &mut rdp).unwrap();
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
        [255, 0, 0, 255]
    );
}


#[test]
fn near_end_object_status_misses_reuse_bounded_staging() {
    const DL: usize = 0x100;
    const RED_TX: u32 = 0x200;
    const BLUE_TX: u32 = 0x218;
    const SPRITE: u32 = 0x230;
    const RED: u32 = (PHYSICAL_RDRAM_BYTES - 32) as u32;
    const BLUE: u32 = (PHYSICAL_RDRAM_BYTES - 16) as u32;
    let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES];
    assert_eq!(
        ObjectTextureScratch::new().bytes.len(),
        OBJECT_TEXTURE_SCRATCH_BYTES
    );
    assert!(OBJECT_TEXTURE_SCRATCH_BYTES < RED as usize);
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
    write_block_texture(&mut rdram, RED_TX, RED, 1);
    write_block_texture(&mut rdram, BLUE_TX, BLUE, 2);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0500_0017, RED_TX);
    write_command(&mut rdram, DL + 8, 0x0500_0017, BLUE_TX);
    write_command(&mut rdram, DL + 16, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

    let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    let (texture, _) = rectangle_texture(&operations[0]);
    assert_eq!(texture.sample(0.0, 0.0), [0, 0, 255, 255]);
}


#[test]
fn texture_image_cannot_escape_physical_rdram_into_alias_backing() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    let mut rdram = vec![0u8; PHYSICAL_RDRAM_BYTES + 0x100];
    write_block_texture(&mut rdram, TX, PHYSICAL_RDRAM_BYTES as u32, 0x1234_5678);
    write_command(&mut rdram, DL, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);

    let error = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap_err();
    assert!(error.to_string().contains("exceeds physical 8 MiB RDRAM"));
}


#[test]
fn standalone_loadtx_tile_then_rectangle_uses_loaded_tmem() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    const SPRITE: u32 = 0x240;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    let pixels = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in pixels.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(IMAGE + index as u32 * 2),
                pixel,
            );
        }
    }
    write_tile_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 8, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(3.0, 1.0, other_mode, ConvertState::default()),
        [0, 0, 0, 255]
    );
}


#[test]
fn object_matrix_drives_standalone_matrix_relative_rectangle() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    const MATRIX: u32 = 0x240;
    const SPRITE: u32 = 0x258;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    for index in 0..8 {
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
            0xf801,
        );
    }
    write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_object_matrix(&mut rdram, MATRIX, 8, 12, 2 << 10, 1 << 10);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, DL + 16, 0xda00_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

    let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
        panic!("matrix-relative object must lower to a texture rectangle");
    };
    assert_eq!((rectangle.ulx, rectangle.uly), (2.0, 3.0));
    assert_eq!((rectangle.lrx, rectangle.lry), (4.0, 5.0));
    assert_eq!((rectangle.dsdx, rectangle.dtdy), (2 << 10, 1 << 10));
    assert!(rectangle.texture.is_some());
}


#[test]
fn sub_matrix_then_compound_loadtx_rect_r_loads_before_drawing() {
    const DL: usize = 0x100;
    const SUB_MATRIX: u32 = 0x200;
    const TXSP: u32 = 0x240;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    for index in 0..8 {
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
            0x003f,
        );
    }
    write_object_sub_matrix(&mut rdram, SUB_MATRIX, 16, 20, 1 << 10, 1 << 10);
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0xdc02_0007, SUB_MATRIX);
    write_command(&mut rdram, DL + 8, 0x0800_002f, TXSP);
    write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
        panic!("matrix-relative compound must lower to a texture rectangle");
    };
    assert_eq!((rectangle.ulx, rectangle.uly), (4.0, 5.0));
    let (texture, other_mode) = rectangle_texture(&operations[0]);
    assert_eq!(
        texture.sample_rdp(0.0, 0.0, other_mode, ConvertState::default()),
        [0, 0, 255, 255]
    );
}


#[test]
fn full_matrix_rotates_standalone_sprite_into_two_textured_triangles() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    const MATRIX: u32 = 0x240;
    const SPRITE: u32 = 0x258;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    for index in 0..8 {
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
            0xf801,
        );
    }
    write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_object_rotation_matrix(&mut rdram, MATRIX, [0, 1 << 16, -(1 << 16), 0], 32, 32);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, DL + 16, 0x0200_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

    let operations = decode_ops(&rdram, DL as u32, &mut RdpDecodeState::default()).unwrap();
    assert_eq!(operations.len(), 2);
    let triangles = operations
        .iter()
        .map(|operation| match operation {
            RenderOp::Triangle(triangle) => triangle,
            _ => panic!("rotating sprite must lower only to triangles"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        triangles[0].v.map(|vertex| (vertex.x, vertex.y)),
        [(8.0, 8.0), (8.0, 4.0), (10.0, 4.0)]
    );
    assert_eq!(
        triangles[1].v.map(|vertex| (vertex.x, vertex.y)),
        [(8.0, 8.0), (10.0, 4.0), (10.0, 8.0)]
    );
    assert_eq!(
        triangles[0].v.map(|vertex| (vertex.s, vertex.t)),
        [(0.0, 0.0), (4.0, 0.0), (4.0, 2.0)]
    );
    assert!(triangles.iter().all(|triangle| triangle.texture.is_some()));

    let mut framebuffer = crate::raster::Framebuffer::new(12, 12);
    framebuffer.clear(0, 0, 0, 0);
    for triangle in triangles {
        framebuffer.draw_triangle(triangle);
    }
    let pixel = |x: usize, y: usize| {
        let offset = (y * framebuffer.width as usize + x) * 4;
        &framebuffer.pixels[offset..offset + 4]
    };
    assert_eq!(pixel(8, 5), [255, 0, 0, 255]);
    assert_eq!(pixel(7, 5), [0, 0, 0, 0]);
}


#[test]
fn texel1_gap_rotating_sprite_preserves_tile_pair_across_both_wire_families() {
    const DL: usize = 0x100;
    const MATRIX: u32 = 0x200;
    const TXSP: u32 = 0x240;
    const SPRITE: u32 = TXSP + 24;
    const IMAGE: u32 = 0x400;
    const MODE: usize = 0x500;

    for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
        for compound in [false, true] {
            let mut rdram = vec![0u8; 0x600];
            {
                let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
                for index in 0..8 {
                    view.write_u16(
                        fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
                        if index < 4 { 0xf801 } else { 0x003f },
                    );
                }
            }
            write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 4, 4);
            write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
            write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);

            // Two-cycle mode; cycle one passes TEXEL0 and cycle two
            // passes TEXEL1. Tile 1 starts at the second TMEM word loaded
            // by the object's eight-texel block.
            write_command(&mut rdram, MODE, 0xef00_0000 | 0x0018_0cff, 0);
            write_command(&mut rdram, MODE + 8, 0xfc00_0000, 0x0000_8282);
            write_command(
                &mut rdram,
                MODE + 16,
                0xf500_0000 | (2 << 19) | (1 << 9) | 1,
                1 << 24,
            );
            write_command(&mut rdram, MODE + 24, 0xf200_0000, 1 << 24);
            write_command(&mut rdram, MODE + 32, 0xdf00_0000, 0);

            let mut rdp = RdpDecodeState::default();
            crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();

            let (matrix_opcode, load_opcode, sprite_opcode, compound_opcode, end_opcode) =
                match family {
                    S2dexWireFamily::S2dex => (
                        S2DEX_G_OBJ_MOVEMEM,
                        S2DEX_G_OBJ_LOADTXTR,
                        S2DEX_G_OBJ_SPRITE,
                        S2DEX_G_OBJ_LDTX_SPRITE,
                        S2DEX_G_ENDDL,
                    ),
                    S2dexWireFamily::S2dex2 => (
                        G_OBJ_MOVEMEM,
                        G_OBJ_LOADTXTR,
                        G_OBJ_SPRITE,
                        G_OBJ_LDTX_SPRITE,
                        G_ENDDL,
                    ),
                };
            write_command(
                &mut rdram,
                DL,
                (u32::from(matrix_opcode) << 24) | 23,
                MATRIX,
            );
            let mut offset = DL + 8;
            if compound {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(compound_opcode) << 24) | 47,
                    TXSP,
                );
                offset += 8;
            } else {
                write_command(
                    &mut rdram,
                    offset,
                    (u32::from(load_opcode) << 24) | 23,
                    TXSP,
                );
                write_command(
                    &mut rdram,
                    offset + 8,
                    u32::from(sprite_opcode) << 24,
                    SPRITE,
                );
                offset += 16;
            }
            write_command(&mut rdram, offset, u32::from(end_opcode) << 24, 0);

            let operations =
                decode_ops_for_family(&rdram, DL as u32, &mut rdp, family).unwrap();
            assert_eq!(operations.len(), 2);
            let mut framebuffer = crate::raster::Framebuffer::new(4, 4);
            framebuffer.clear(0, 0, 0, 0);
            for operation in &operations {
                let RenderOp::Triangle(triangle) = operation else {
                    panic!("rotating sprite must lower only to triangles")
                };
                framebuffer.draw_triangle(triangle);
            }
            let pixel_offset = (framebuffer.width as usize + 1) * 4;
            assert_eq!(
                &framebuffer.pixels[pixel_offset..pixel_offset + 4],
                &[0, 0, 255, 255],
                "family={family:?} compound={compound} must source TEXEL1 from tile 1"
            );
        }
    }
}


#[test]
fn texel1_gap_rotating_sprite_without_tile_one_stays_loud_and_transactional() {
    const DL: usize = 0x100;
    const MATRIX: u32 = 0x200;
    const TX: u32 = 0x240;
    const SPRITE: u32 = 0x258;
    const IMAGE: u32 = 0x400;
    const MODE: usize = 0x500;
    let mut rdram = vec![0u8; 0x600];
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 4, 4);
    write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    write_command(&mut rdram, MODE, 0xef00_0000 | 0x0018_0cff, 0);
    write_command(&mut rdram, MODE + 8, 0xfc00_0000, 0x0000_8282);
    write_command(
        &mut rdram,
        MODE + 16,
        0xf500_0000 | (2 << 19) | (1 << 9) | 100,
        1 << 24,
    );
    write_command(&mut rdram, MODE + 24, 0xf200_0000 | (4 << 12), 1 << 24);
    write_command(&mut rdram, MODE + 32, 0xdf00_0000, 0);
    write_command(
        &mut rdram,
        DL,
        (u32::from(G_OBJ_MOVEMEM) << 24) | 23,
        MATRIX,
    );
    write_command(
        &mut rdram,
        DL + 8,
        (u32::from(G_OBJ_LOADTXTR) << 24) | 23,
        TX,
    );
    write_command(&mut rdram, DL + 16, u32::from(G_OBJ_SPRITE) << 24, SPRITE);
    write_command(&mut rdram, DL + 24, u32::from(G_ENDDL) << 24, 0);

    let mut rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, MODE as u32, &mut rdp).unwrap();
    let before = format!("{rdp:?}");
    let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("combiner selects TEXEL1 without an initialized tile 1 image"),
        "{error}"
    );
    assert_eq!(format!("{rdp:?}"), before);
}


#[test]
fn compound_loadtx_sprite_loads_then_draws_and_rejected_tail_is_atomic() {
    const DL: usize = 0x100;
    const MATRIX: u32 = 0x200;
    const TXSP: u32 = 0x240;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    for index in 0..8 {
        fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u16(
            fn64_runtime::RdramAddr::from_offset(IMAGE + index * 2),
            0x003f,
        );
    }
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 16, 20);
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
    write_command(&mut rdram, DL, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, DL + 8, 0x0600_002f, TXSP);
    write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let operations = decode_ops(&rdram, DL as u32, &mut rdp).unwrap();
    assert_eq!(operations.len(), 2);
    let RenderOp::Triangle(triangle) = &operations[0] else {
        panic!("compound rotating sprite must lower to triangles");
    };
    assert_eq!((triangle.v[0].x, triangle.v[0].y), (4.0, 5.0));
    assert!(triangle.texture.is_some());

    write_command(&mut rdram, DL + 16, 0x0900_0000, 0);
    let mut fresh = RdpDecodeState::default();
    let before = format!("{fresh:?}");
    let error = decode_ops(&rdram, DL as u32, &mut fresh).unwrap_err();
    assert!(error.to_string().contains("G_BG_1CYC"));
    assert_eq!(format!("{fresh:?}"), before);
}


#[test]
fn rotating_sprite_traps_unknown_matrix_rounding_without_committing() {
    const DL: usize = 0x100;
    const TX: u32 = 0x200;
    const MATRIX: u32 = 0x240;
    const SPRITE: u32 = 0x258;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x600];
    write_block_texture(&mut rdram, TX, IMAGE, IMAGE);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 15, 0, 0, 1 << 16], 0, 0);
    write_sprite(&mut rdram, SPRITE, 4, 2, 0, 2);
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(SPRITE), 1);
    write_command(&mut rdram, DL, 0x0500_0017, TX);
    write_command(&mut rdram, DL + 8, 0xdc00_0017, MATRIX);
    write_command(&mut rdram, DL + 16, 0x0200_0000, SPRITE);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    let before = format!("{rdp:?}");
    let error = decode_ops(&rdram, DL as u32, &mut rdp).unwrap_err();
    assert!(error
        .to_string()
        .contains("sub-quarter-pixel matrix rounding"));
    assert_eq!(format!("{rdp:?}"), before);
}


#[test]
fn object_render_mode_retains_public_modes_as_typed_state() {
    let mode = read_object_render_mode(
        G_OBJRM_NOTXCLAMP | G_OBJRM_XLU | G_OBJRM_ANTIALIAS | G_OBJRM_BILERP | G_OBJRM_WIDEN,
        0x100,
    )
    .unwrap();
    assert_eq!(mode.texture_clamp, ObjectTextureClamp::Disabled);
    assert_eq!(mode.filter_correction, ObjectFilterCorrection::Bilinear);
    assert_eq!(mode.perimeter.shrink_half_texels, 0);
    assert!(mode.perimeter.widen_three_eighths_texel);
    assert_eq!(
        mode.ignored_edge_flags,
        IgnoredObjectEdgeFlags {
            xlu: true,
            antialias: true,
        }
    );

    let combined =
        read_object_render_mode(G_OBJRM_WIDEN | G_OBJRM_SHRINKSIZE_1, 0x108).unwrap();
    assert_eq!(combined.perimeter.shrink_half_texels, 1);
    assert!(combined.perimeter.widen_three_eighths_texel);

    let error = read_object_render_mode(G_OBJRM_SHRINKSIZE_1 | G_OBJRM_SHRINKSIZE_2, 0x110)
        .unwrap_err();
    assert!(error.to_string().contains("mutually exclusive"));
}


#[test]
fn object_perimeter_composition_exhaustively_preserves_public_fixed_units() {
    for shrink_half_texels in 0..=2u8 {
        for widen_three_eighths_texel in [false, true] {
            let perimeter = ObjectPerimeter {
                shrink_half_texels,
                widen_three_eighths_texel,
            };
            let shrink_numerator = u32::from(shrink_half_texels) * 4096;
            let widen_numerator: u32 = if widen_three_eighths_texel { 1536 } else { 0 };
            for scale_10 in 1..=i16::MAX as u16 {
                let result =
                    perimeter.exact_screen_adjustments(scale_10, "X", "G_OBJ_RECTANGLE");
                let exact = shrink_numerator.is_multiple_of(u32::from(scale_10))
                    && widen_numerator.is_multiple_of(u32::from(scale_10));
                assert_eq!(
                    result.is_ok(),
                    exact,
                    "shrink={shrink_half_texels} widen={widen_three_eighths_texel} scale={scale_10}"
                );
                if let Ok((shrink_pixels, widen_pixels)) = result {
                    assert_eq!(
                        shrink_pixels,
                        shrink_numerator as f32 / scale_10 as f32 / 4.0
                    );
                    assert_eq!(widen_pixels, widen_numerator as f32 / scale_10 as f32 / 4.0);
                }
            }

            for image_texels in 1..=2047u32 {
                let image_5 = (image_texels * 32) as u16;
                let result = perimeter.corrected_image_5(image_5, "width", "G_OBJ_RECTANGLE");
                let shrink_5 = u16::from(shrink_half_texels) * 32;
                let expected = image_5.checked_sub(shrink_5).and_then(|value| {
                    value.checked_add(if widen_three_eighths_texel { 12 } else { 0 })
                });
                assert_eq!(result.ok(), expected);
                let (source_start, source_end) = perimeter.source_bounds(image_5);
                assert_eq!(source_start, f32::from(shrink_half_texels) * 0.5);
                assert_eq!(
                    source_end,
                    image_texels as f32 - f32::from(shrink_half_texels) * 0.5
                        + if widen_three_eighths_texel {
                            0.375
                        } else {
                            0.0
                        }
                );
            }
        }
    }
    let error = ObjectPerimeter::default()
        .exact_screen_adjustments(0, "X", "G_OBJ_RECTANGLE")
        .unwrap_err();
    assert!(error.to_string().contains("scale must be nonzero"));
}


#[test]
fn object_render_mode_opcode_collision_is_selected_only_by_admitted_family() {
    const DL: usize = 0x100;
    let mut legacy = vec![0u8; 0x120];
    write_command(
        &mut legacy,
        DL,
        (u32::from(S2DEX_G_OBJ_RENDERMODE)) << 24,
        G_OBJRM_XLU | G_OBJRM_ANTIALIAS,
    );
    write_command(&mut legacy, DL + 8, (u32::from(S2DEX_G_ENDDL)) << 24, 0);
    let mut modern = vec![0u8; 0x120];
    write_command(
        &mut modern,
        DL,
        (u32::from(G_OBJ_RENDERMODE)) << 24,
        G_OBJRM_XLU | G_OBJRM_ANTIALIAS,
    );
    write_command(&mut modern, DL + 8, (u32::from(G_ENDDL)) << 24, 0);

    assert!(decode_ops_for_family(
        &legacy,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex,
    )
    .unwrap()
    .is_empty());
    assert!(decode_ops_for_family(
        &modern,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex2,
    )
    .unwrap()
    .is_empty());
    let legacy_as_modern = decode_ops_for_family(
        &legacy,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex2,
    )
    .unwrap_err();
    assert!(
        legacy_as_modern
            .to_string()
            .contains("unsupported S2dex2 command"),
        "{legacy_as_modern}"
    );
    let modern_as_legacy = decode_ops_for_family(
        &modern,
        DL as u32,
        &mut RdpDecodeState::default(),
        S2dexWireFamily::S2dex,
    )
    .unwrap_err();
    assert!(
        modern_as_legacy
            .to_string()
            .contains("unsupported S2dex command"),
        "{modern_as_legacy}"
    );
}


#[test]
fn current_header_ignored_edge_flags_and_safe_notxclamp_preserve_point_raster() {
    const BASE_DL: usize = 0x100;
    const EDGE_DL: usize = 0x120;
    const NO_CLAMP_DL: usize = 0x140;
    const TXSP: u32 = 0x200;
    const IMAGE: u32 = 0x400;
    let mut rdram = vec![0u8; 0x500];
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
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
    write_command(&mut rdram, BASE_DL, 0x0700_002f, TXSP);
    write_command(&mut rdram, BASE_DL + 8, 0xdf00_0000, 0);
    write_command(
        &mut rdram,
        EDGE_DL,
        0x0b00_0000,
        G_OBJRM_XLU | G_OBJRM_ANTIALIAS,
    );
    write_command(&mut rdram, EDGE_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, EDGE_DL + 16, 0xdf00_0000, 0);
    write_command(&mut rdram, NO_CLAMP_DL, 0x0b00_0000, G_OBJRM_NOTXCLAMP);
    write_command(&mut rdram, NO_CLAMP_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, NO_CLAMP_DL + 16, 0xdf00_0000, 0);

    let draw = |operations: &[RenderOp]| {
        let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
            panic!("object mode fixture must emit one rectangle")
        };
        let mut framebuffer = crate::raster::Framebuffer::new(4, 2);
        framebuffer.clear(0, 0, 0, 0);
        framebuffer.draw_texture_rectangle(rectangle);
        framebuffer.pixels
    };
    let base = decode_ops(&rdram, BASE_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let edge = decode_ops(&rdram, EDGE_DL as u32, &mut RdpDecodeState::default()).unwrap();
    let no_clamp =
        decode_ops(&rdram, NO_CLAMP_DL as u32, &mut RdpDecodeState::default()).unwrap();
    assert_eq!(edge.len(), base.len());
    assert_eq!(no_clamp.len(), base.len());
    assert_eq!(draw(&edge), draw(&base));
    assert_eq!(draw(&no_clamp), draw(&base));
}


#[test]
fn notxclamp_point_perimeters_exhaust_families_paths_flips_and_base_scales() {
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
    write_command(&mut template, SETUP, 0xef00_0000 | 0x0008_0cff, 0);
    write_command(&mut template, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
    write_command(&mut template, SETUP + 16, 0xdf00_0000, 0);

    let decode = |mode: u32,
                  family: S2dexWireFamily,
                  relative: bool,
                  compound: bool,
                  base_scale: u16,
                  effective_scale: u16,
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
        write_object_matrix(&mut rdram, MATRIX, 0, 0, base_scale, base_scale);
        let object_scale = if relative {
            let numerator = u32::from(effective_scale) * 1024;
            assert_eq!(numerator % u32::from(base_scale), 0);
            u16::try_from(numerator / u32::from(base_scale)).unwrap()
        } else {
            effective_scale
        };
        {
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
            let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
            view.write_u16(sprite.checked_add(2).unwrap(), object_scale);
            view.write_u16(sprite.checked_add(10).unwrap(), object_scale);
            view.write_u8(sprite.checked_add(23).unwrap(), image_flags);
        }
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

    let mut saw_empty = false;
    let mut saw_nonidentity_base_scale = false;
    let footprint = ObjectUnclampedPointFootprint;
    for shrink_half_texels in [0u8, 1, 2] {
        let shrink_mode = match shrink_half_texels {
            0 => 0,
            1 => G_OBJRM_SHRINKSIZE_1,
            2 => G_OBJRM_SHRINKSIZE_2,
            _ => unreachable!(),
        };
        for widen in [false, true] {
            if !widen && shrink_half_texels == 0 {
                continue;
            }
            // WIDEN and shrink must each land exactly in s10.2. The
            // no-shrink 768/1536 cases and the shrink+WIDEN scale-512
            // cases also exercise distinct admitted raster sequences.
            let effective_scales: &[u16] = match (widen, shrink_half_texels) {
                (true, 0) => &[768, 1536],
                (true, 1 | 2) => &[512],
                (false, _) => &[512, 1024, 2048, 4096],
                _ => unreachable!(),
            };
            let image_flags: &[u8] = if widen {
                // Which screen edge owns positive S/T after a flip is not
                // published, so this combination remains a loud frontier.
                &[0]
            } else {
                &[
                    0,
                    G_OBJ_FLAG_FLIPS,
                    G_OBJ_FLAG_FLIPT,
                    G_OBJ_FLAG_FLIPS | G_OBJ_FLAG_FLIPT,
                ]
            };
            let perimeter_mode = shrink_mode | if widen { G_OBJRM_WIDEN } else { 0 };
            for &effective_scale in effective_scales {
                for &image_flags in image_flags {
                    for family in [S2dexWireFamily::S2dex, S2dexWireFamily::S2dex2] {
                        for relative in [false, true] {
                            let base_scales: &[u16] = if relative {
                                &[512, 1024, 2048]
                            } else {
                                &[1024]
                            };
                            for &base_scale in base_scales {
                                saw_nonidentity_base_scale |= relative && base_scale != 1024;
                                for compound in [false, true] {
                                    let clamped = decode(
                                        perimeter_mode,
                                        family,
                                        relative,
                                        compound,
                                        base_scale,
                                        effective_scale,
                                        image_flags,
                                    );
                                    let unclamped = decode(
                                        perimeter_mode | G_OBJRM_NOTXCLAMP,
                                        family,
                                        relative,
                                        compound,
                                        base_scale,
                                        effective_scale,
                                        image_flags,
                                    );
                                    let RenderOp::TextureRectangle(clamped) = &clamped[0]
                                    else {
                                        panic!("point perimeter path must emit a texture rectangle")
                                    };
                                    let RenderOp::TextureRectangle(unclamped) = &unclamped[0]
                                    else {
                                        panic!("unclamped point perimeter path must emit a texture rectangle")
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

                                    for (start, gradient, screen_start, screen_end, axis) in [
                                        (
                                            unclamped.s,
                                            f32::from(unclamped.dsdx) / 1024.0,
                                            unclamped.ulx,
                                            unclamped.lrx,
                                            "S",
                                        ),
                                        (
                                            unclamped.t,
                                            f32::from(unclamped.dtdy) / 1024.0,
                                            unclamped.uly,
                                            unclamped.lry,
                                            "T",
                                        ),
                                    ] {
                                        let axis_footprint = footprint
                                            .validate_axis(
                                                start,
                                                gradient,
                                                screen_start,
                                                screen_end,
                                                4,
                                                axis,
                                                "G_OBJ_RECTANGLE",
                                            )
                                            .unwrap();
                                        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
                                        let pixels =
                                            pixel_min(screen_start)..pixel_min(screen_end);
                                        let samples = pixels
                                            .map(|pixel| {
                                                (start
                                                    + (pixel as f32 - screen_start.floor())
                                                        * gradient)
                                                    .floor()
                                                    as i32
                                            })
                                            .collect::<Vec<_>>();
                                        if samples.is_empty() {
                                            saw_empty = true;
                                            assert_eq!(
                                                axis_footprint,
                                                ObjectPointAxisFootprint::Empty
                                            );
                                        } else {
                                            assert!(samples
                                                .iter()
                                                .all(|texel| (0..4).contains(texel)));
                                            if gradient > 0.0 {
                                                assert!(samples
                                                    .windows(2)
                                                    .all(|pair| pair[0] <= pair[1]));
                                            } else {
                                                assert!(samples
                                                    .windows(2)
                                                    .all(|pair| pair[0] >= pair[1]));
                                            }
                                            assert_eq!(
                                                axis_footprint,
                                                ObjectPointAxisFootprint::MonotonicInterior {
                                                    direction: if gradient > 0.0 {
                                                        ObjectPointDirection::Increasing
                                                    } else {
                                                        ObjectPointDirection::Decreasing
                                                    },
                                                    first_texel: samples[0] as u16,
                                                    last_texel: samples[samples.len() - 1]
                                                        as u16,
                                                }
                                            );
                                        }
                                    }

                                    let draw = |rectangle: &TextureRectangle| {
                                        let mut framebuffer =
                                            crate::raster::Framebuffer::new(8, 8);
                                        framebuffer.clear(0, 0, 0, 0);
                                        framebuffer.draw_texture_rectangle(rectangle);
                                        framebuffer.pixels
                                    };
                                    assert_eq!(draw(unclamped), draw(clamped));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    assert!(
        saw_empty,
        "exact subpixel extents must exercise the empty raster sequence"
    );
    assert!(
        saw_nonidentity_base_scale,
        "RectangleR must exercise non-identity BaseScale cross-terms"
    );
}


#[test]
fn notxclamp_point_perimeters_reject_spills_and_unpublished_paths_transactionally() {
    let footprint = ObjectUnclampedPointFootprint;
    let negative = footprint
        .validate_axis(-0.5, 1.0, 0.0, 2.0, 4, "S", "G_OBJ_RECTANGLE")
        .unwrap_err();
    assert!(
        negative.to_string().contains("texel -1 outside"),
        "{negative}"
    );
    let positive = footprint
        .validate_axis(3.5, 1.0, 0.0, 2.0, 4, "T", "G_OBJ_RECTANGLE")
        .unwrap_err();
    assert!(
        positive.to_string().contains("texel 4 outside"),
        "{positive}"
    );

    const DL: usize = 0x100;
    const TXSP: u32 = 0x300;
    const MATRIX: u32 = 0x340;
    const IMAGE: u32 = 0x500;
    const BILERP_SETUP: usize = 0x700;
    const COPY_SETUP: usize = 0x720;
    let mut rdram = vec![0u8; 0x800];
    write_block_texture(&mut rdram, TXSP, IMAGE, 1);
    write_sprite(&mut rdram, TXSP + 24, 4, 4, 0, 2);
    write_object_rotation_matrix(&mut rdram, MATRIX, [1 << 16, 0, 0, 1 << 16], 0, 0);
    write_command(
        &mut rdram,
        BILERP_SETUP,
        0xef00_0000 | 0x0008_0cff | (2 << 12),
        0,
    );
    write_command(&mut rdram, BILERP_SETUP + 8, 0xdf00_0000, 0);
    write_command(&mut rdram, COPY_SETUP, 0xef00_0000 | (2 << 20), 0);
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

    let mut point_rdp = RdpDecodeState::default();
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 512);
        view.write_u16(sprite.checked_add(10).unwrap(), 512);
    }
    let error = rectangle_error(
        &mut rdram,
        G_OBJRM_NOTXCLAMP | G_OBJRM_WIDEN,
        &mut point_rdp,
    );
    assert!(error.contains("texel 4 outside"), "{error}");

    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1536);
        view.write_u16(sprite.checked_add(10).unwrap(), 1536);
    }
    let error = rectangle_error(
        &mut rdram,
        G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1,
        &mut point_rdp,
    );
    assert!(error.contains("sub-quarter-pixel rounding"), "{error}");

    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_u8(
        fn64_runtime::RdramAddr::from_offset(TXSP + 24 + 23),
        G_OBJ_FLAG_FLIPS,
    );
    let error = rectangle_error(
        &mut rdram,
        G_OBJRM_NOTXCLAMP | G_OBJRM_WIDEN,
        &mut point_rdp,
    );
    assert!(error.contains("positive-edge selection"), "{error}");
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        let sprite = fn64_runtime::RdramAddr::from_offset(TXSP + 24);
        view.write_u16(sprite.checked_add(2).unwrap(), 1 << 10);
        view.write_u16(sprite.checked_add(10).unwrap(), 1 << 10);
        view.write_u8(sprite.checked_add(23).unwrap(), 0);
    }

    let mut bilerp_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, BILERP_SETUP as u32, &mut bilerp_rdp)
        .unwrap();
    let error = rectangle_error(
        &mut rdram,
        G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1 | G_OBJRM_BILERP,
        &mut bilerp_rdp,
    );
    assert!(error.contains("filter-footprint arithmetic"), "{error}");

    let mut copy_rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, COPY_SETUP as u32, &mut copy_rdp)
        .unwrap();
    let error = rectangle_error(
        &mut rdram,
        G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1,
        &mut copy_rdp,
    );
    assert!(error.contains("Copy cycle does not support"), "{error}");

    write_command(
        &mut rdram,
        DL,
        u32::from(G_OBJ_RENDERMODE) << 24,
        G_OBJRM_NOTXCLAMP | G_OBJRM_SHRINKSIZE_1,
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
    let before = format!("{point_rdp:?}");
    let error = decode_ops(&rdram, DL as u32, &mut point_rdp).unwrap_err();
    assert!(error.to_string().contains("G_OBJRM_NOTXCLAMP on a polygon"));
    assert_eq!(format!("{point_rdp:?}"), before);
}


#[test]
fn average_filter_uses_box_samples_and_loudly_rejects_unknown_corrections() {
    const AVERAGE_DL: usize = 0x100;
    const BILERP_DL: usize = 0x120;
    const NO_CLAMP_DL: usize = 0x140;
    const TXSP: u32 = 0x200;
    const IMAGE: u32 = 0x400;
    const SETUP: usize = 0x500;
    let mut rdram = vec![0u8; 0x600];
    write_block_texture(&mut rdram, TXSP, IMAGE, IMAGE);
    write_sprite(&mut rdram, TXSP + 24, 4, 2, 0, 2);
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
    write_command(&mut rdram, AVERAGE_DL, 0x0700_002f, TXSP);
    write_command(&mut rdram, AVERAGE_DL + 8, 0xdf00_0000, 0);
    write_command(&mut rdram, BILERP_DL, 0x0b00_0000, G_OBJRM_BILERP);
    write_command(&mut rdram, BILERP_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, BILERP_DL + 16, 0xdf00_0000, 0);
    write_command(&mut rdram, NO_CLAMP_DL, 0x0b00_0000, G_OBJRM_NOTXCLAMP);
    write_command(&mut rdram, NO_CLAMP_DL + 8, 0x0700_002f, TXSP);
    write_command(&mut rdram, NO_CLAMP_DL + 16, 0xdf00_0000, 0);
    write_command(&mut rdram, SETUP, 0xef00_0000 | 0x0008_0cff | (3 << 12), 0);
    write_command(&mut rdram, SETUP + 8, 0xfc8f_ff1f, 0x88fc_f279);
    write_command(&mut rdram, SETUP + 16, 0xdf00_0000, 0);

    let mut rdp = RdpDecodeState::default();
    crate::gbi::decode_raw_rdp_ops_with_state(&rdram, SETUP as u32, &mut rdp).unwrap();
    let operations = decode_ops(&rdram, AVERAGE_DL as u32, &mut rdp).unwrap();
    let RenderOp::TextureRectangle(rectangle) = &operations[0] else {
        panic!("average fixture must emit one rectangle")
    };
    let mut framebuffer = crate::raster::Framebuffer::new(4, 2);
    framebuffer.clear(0, 0, 0, 0);
    framebuffer.draw_texture_rectangle(rectangle);
    assert_eq!(&framebuffer.pixels[..4], [128, 0, 128, 255]);

    let bilerp_error = decode_ops(&rdram, BILERP_DL as u32, &mut rdp.clone()).unwrap_err();
    assert!(bilerp_error
        .to_string()
        .contains("Average texture filter does not use G_OBJRM_BILERP"));
    let unclamped_error = decode_ops(&rdram, NO_CLAMP_DL as u32, &mut rdp.clone()).unwrap_err();
    assert!(unclamped_error
        .to_string()
        .contains("Average four-texel cell"));
}


#[test]
fn average_shrink_footprint_exhaustively_classifies_public_inward_cells() {
    for filter in [
        TextureFilter::Point,
        TextureFilter::Average,
        TextureFilter::Bilinear,
        TextureFilter::Reserved,
    ] {
        for inset_half_texels in 0..=2 {
            for texture_clamp in [ObjectTextureClamp::Perimeter, ObjectTextureClamp::Disabled] {
                for widen_three_eighths_texel in [false, true] {
                    let mode = ObjectRenderMode {
                        texture_clamp,
                        perimeter: ObjectPerimeter {
                            shrink_half_texels: inset_half_texels,
                            widen_three_eighths_texel,
                        },
                        ..ObjectRenderMode::default()
                    };
                    let result = ObjectAverageShrinkFootprint::from_mode(
                        mode,
                        filter,
                        "G_OBJ_RECTANGLE",
                    );
                    let admitted = filter == TextureFilter::Average
                        && inset_half_texels != 0
                        && !widen_three_eighths_texel;
                    if admitted {
                        let footprint = result.unwrap().expect("admitted Average inset");
                        assert_eq!(footprint.inset_half_texels, inset_half_texels);
                        for image_width in 3..=32u16 {
                            for flipped in [false, true] {
                                let start =
                                    footprint.rectangle_start(image_width * 32, flipped);
                                let first = start.floor() as i32;
                                assert!(first >= 0);
                                assert!(first + 1 < i32::from(image_width));
                            }
                        }
                    } else if filter == TextureFilter::Average
                        && inset_half_texels != 0
                        && widen_three_eighths_texel
                    {
                        assert!(result.is_err());
                    } else {
                        assert_eq!(result.unwrap(), None);
                    }
                }
            }
        }
    }
}


#[test]
fn unclamped_average_endpoint_proof_matches_every_emitted_four_texel_cell() {
    let footprint = ObjectUnclampedAverageFootprint;
    for image_texels in 2..=16u16 {
        for texture_start_quarters in -4..=i32::from(image_texels) * 4 + 4 {
            for gradient_quarters in -8..=8 {
                if gradient_quarters == 0 {
                    continue;
                }
                for screen_start_quarters in -3..=3 {
                    for pixel_count in 0..=12 {
                        let texture_start = texture_start_quarters as f32 / 4.0;
                        let gradient = gradient_quarters as f32 / 4.0;
                        let screen_start = screen_start_quarters as f32 / 4.0;
                        let screen_end = screen_start + pixel_count as f32;
                        let pixel_min = |edge: f32| (edge - 0.5).ceil() as i32;
                        let first_pixel = pixel_min(screen_start);
                        let last_pixel = pixel_min(screen_end) - 1;
                        let coordinate = |pixel: i32| {
                            texture_start + (pixel as f32 - screen_start.floor()) * gradient
                        };
                        let every_cell_is_interior = (first_pixel..=last_pixel).all(|pixel| {
                            let first = coordinate(pixel).floor() as i32;
                            first >= 0 && first + 1 < i32::from(image_texels)
                        });
                        let result = footprint.validate_axis(
                            texture_start,
                            gradient,
                            screen_start,
                            screen_end,
                            image_texels,
                            "S",
                            "G_OBJ_RECTANGLE",
                        );
                        assert_eq!(
                            result.is_ok(),
                            every_cell_is_interior,
                            "image={image_texels} start={texture_start} gradient={gradient} screen=({screen_start},{screen_end})"
                        );
                    }
                }
            }
        }
    }
}
