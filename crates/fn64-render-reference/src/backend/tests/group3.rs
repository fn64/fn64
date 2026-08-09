// The split module trees feed names through use-super glob chains; rustc
// accepts these imports at check time yet its fix pass calls them unused,
// and removing them breaks the build (pattern-bound constants, glob-fed
// children). Suppressed until the trees are normalized to single-source
// imports; see the file-split PR notes.
#![allow(unused_imports)]

use super::support::*;
use crate::raster::Framebuffer;
use crate::{
    depth, gbi, png_dump, raster, render_unsupported_error, s2dex, vi, GeometryWireFamily,
    S2dexWireFamily,
};
use fn64_render::{
    F3dex2UcodeCatalog, FrameStatus, MicrocodeDataImageIdentity, MicrocodePairCatalog,
    NonRdpWrite16, NonRdpWrite16Disposition, OsTask, PresentMemory, PresentRequest, RenderBackend,
    RenderConfig, RenderError, S2dexUcodeCatalog, UcodeId, ViPixelType, ViPresentation,
    ViScanoutRegisters,
};

use crate::backend::*;
use crate::backend::hidden_bits::*;
use crate::backend::vi_source::*;
use crate::backend::validate::*;
use crate::backend::framebuffer_io::*;
use crate::backend::imp::*;
use crate::backend::render_backend::*;
use sha2::Digest;

#[test]
fn f3dex2_color_writes_require_persistent_setcimg_not_output_addr() {
    const DL: usize = 0x100;
    const VERTICES: usize = 0x200;
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    let mut rdram = vec![0u8; 0x2000];
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    write_command(
        &mut rdram,
        DL,
        (u32::from(gbi::G_VTX) << 24) | (3 << 12) | (3 << 1),
        VERTICES as u32,
    );
    write_command(
        &mut rdram,
        DL + 8,
        (u32::from(gbi::G_TRI1) << 24) | (1 << 9) | (2 << 1),
        0,
    );
    write_command(&mut rdram, DL + 16, u32::from(gbi::G_ENDDL) << 24, 0);

    let error = backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0x1000,
        )
        .unwrap_err();

    assert!(error.to_string().contains("no persistent G_SETCIMG"));
    assert!(error.to_string().contains("output_addr state is not"));
}


#[test]
fn one_cycle_fillrect_uses_primitive_combiner_and_excludes_lower_right_edges() {
    let mut rdram = vec![0u8; 0x1000];
    let commands = [
        (0xff10_0003u32, 0x400u32),
        (0xfcff_ffff, 0xfffd_f6fb),
        (0xfa00_0000, 0xff00_00ff),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = 0x100 + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: 0x100,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    for x in 0..4u32 {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(0x400 + x * 2)),
            if x < 3 { 0xf801 } else { 0 },
            "one-cycle lower/right edges are exclusive at x={x}"
        );
    }
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(0x408)),
        0,
        "one-cycle lower edge must exclude row 1"
    );
}


#[test]
fn one_cycle_ordered_rgb_dither_reaches_index8_color_image_bytes() {
    const DISPLAY_LIST: usize = 0x100;
    const TARGET: u32 = 0x400;
    let mut rdram = vec![0u8; 0x1000];
    let commands = [
        // One-cycle plus G_CD_MAGICSQ in the full other-mode register.
        (0xef00_0000u32, 0),
        // I8/CI8 is the public one-byte color-image memory layout.
        (0xff48_0003, TARGET),
        // (0 - 0) * 0 + PRIMITIVE for color and alpha.
        (0xfcff_ffff, 0xfffd_f6fb),
        (0xfa00_0000, 0x0707_07ff),
        // Magic-square RGB dither is the reset selector. One-cycle
        // lower/right bounds are exclusive, producing x=0..3 at y=0.
        (0xf600_0000 | ((4 * 4) << 12) | 4, 0),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = DISPLAY_LIST + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }

    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(4, 1)).unwrap();
    backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DISPLAY_LIST as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    let actual = std::array::from_fn(|index| {
        view.read_u8(fn64_runtime::RdramAddr::from_offset(TARGET + index as u32))
    });
    assert_eq!(
        actual,
        [8, 8, 8, 7],
        "magic-square row zero thresholds [0,6,1,7] must perturb the common pre-write intensity lane"
    );
}


#[test]
fn raw_fillrect_g_ac_dither_is_seeded_and_differs_from_g_ac_none() {
    const DL: usize = 0x100;
    const TARGET: u32 = 0x400;
    let render = |alpha_compare: u32| {
        let mut rdram = vec![0u8; 0x1000];
        let commands = [
            // One-cycle mode with only the alpha-compare selector changed.
            (0xef00_0000u32, alpha_compare),
            (0xff10_0007, TARGET),
            // (0 - 0) * 0 + PRIMITIVE for both color and alpha.
            (0xfcff_ffff, 0xfffd_f6fb),
            (0xfa00_0000, 0xff00_0080),
            // One-cycle lower/right edges are exclusive: eight pixels.
            (0xf600_0000 | ((8 * 4) << 12) | 4, 0),
            (0xdf00_0000, 0),
        ];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = DL + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }

        let mut backend = ReferenceBackend::new()
            .with_noise_seed(0x1234)
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::ntsc(8, 1)).unwrap();
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap();

        let view = fn64_runtime::RdramView::from_storage(&rdram);
        std::array::from_fn(|index| {
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + index as u32 * 2,
            ))
        })
    };

    assert_eq!(render(0), [0xf801; 8]);
    assert_eq!(
        render(3),
        [0xf801, 0, 0, 0, 0xf801, 0, 0xf801, 0],
        "seed 0x1234 yields noise bytes [54, 136, 181, 166, 58, 188, 62, 189]"
    );
}


#[test]
fn copy_texture_rectangle_samples_rgba16_into_color_image() {
    const DL: usize = 0x100;
    const TEXTURE: u32 = 0x600;
    const TARGET: u32 = 0x800;
    let mut rdram = vec![0u8; 0x1000];
    let source = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in source.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
        }
    }
    let mut offset = DL;
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    // Copy cycle, explicit RGBA16 destination, and RGBA16 source image.
    write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 0);
    offset += 8;
    write_command(&mut rdram, offset, 0xff10_0003, TARGET);
    offset += 8;
    write_command(&mut rdram, offset, 0xfd10_0003, TEXTURE);
    offset += 8;
    // Load tile 7 is contiguous; render tile 0 supplies the row stride.
    write_command(&mut rdram, offset, 0xf510_0000, 7 << 24);
    offset += 8;
    write_command(
        &mut rdram,
        offset,
        0xf300_0000,
        (7 << 24) | (7 << 12) | 0x800,
    );
    offset += 8;
    write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
    offset += 8;
    write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c004);
    offset += 8;
    // Inclusive copy rectangle (0,0)..(3,1), tile 0.
    write_command(&mut rdram, offset, 0xe400_0000 | ((3 * 4) << 12) | 4, 0);
    offset += 8;
    // s=t=0; dsdx=4<<10 means one texel/pixel in copy mode, dtdy=1<<10.
    write_command(&mut rdram, offset, 0, 0x1000_0400);
    offset += 8;
    write_command(&mut rdram, offset, 0xe900_0000, 0);
    offset += 8;
    write_command(&mut rdram, offset, 0xdf00_0000, 0);

    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    for (index, expected) in source.into_iter().enumerate() {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + index as u32 * 2
            )),
            expected,
            "copied pixel {index}"
        );
    }
}


#[test]
fn copy_layout_matrix_admits_only_public_direct_pairs() {
    let target = |layout| gbi::ColorImage {
        format: match layout {
            gbi::ColorImageLayout::Index8 => gbi::ColorImage::CI_FORMAT,
            gbi::ColorImageLayout::Rgba16 | gbi::ColorImageLayout::Rgba32 => {
                gbi::ColorImage::RGBA_FORMAT
            }
        },
        size: match layout {
            gbi::ColorImageLayout::Index8 => gbi::ColorImage::BITS_8,
            gbi::ColorImageLayout::Rgba16 => gbi::ColorImage::BITS_16,
            gbi::ColorImageLayout::Rgba32 => gbi::ColorImage::BITS_32,
        },
        width: 1,
        address: 0,
    };
    for source in gbi::ColorImageLayout::ALL {
        for destination in gbi::ColorImageLayout::ALL {
            let source_image = target(source);
            let rectangle = gbi::TextureRectangle {
                ulx: 0.0,
                uly: 0.0,
                lrx: 0.0,
                lry: 0.0,
                tile: 0,
                s: 0.0,
                t: 0.0,
                dsdx: 4 << 10,
                dtdy: 1 << 10,
                flip: false,
                other_mode: gbi::OtherMode::from_raw(2 << 20, 0, 0),
                combiner: gbi::CombinerState::default(),
                blender: gbi::BlenderState::default(),
                scissor: None,
                texture: Some(gbi::Texture {
                    format: source_image.format,
                    size: source_image.size,
                    width: 1,
                    height: 1,
                    texels: std::rc::Rc::new(vec![255; 4]),
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
                }),
                texture1: None,
                fill_color: 0,
            };
            let admitted =
                validate_copy_texture_rectangle(&rectangle, Some(target(destination))).is_ok();
            let expected = source == destination
                && matches!(
                    source,
                    gbi::ColorImageLayout::Index8 | gbi::ColorImageLayout::Rgba16
                );
            assert_eq!(admitted, expected, "{source:?} -> {destination:?}");
        }
    }

    for source_format in [gbi::ColorImage::I_FORMAT, gbi::ColorImage::IA_FORMAT] {
        for destination in gbi::ColorImageLayout::ALL {
            let rectangle = gbi::TextureRectangle {
                ulx: 0.0,
                uly: 0.0,
                lrx: 0.0,
                lry: 0.0,
                tile: 0,
                s: 0.0,
                t: 0.0,
                dsdx: 4 << 10,
                dtdy: 1 << 10,
                flip: false,
                other_mode: gbi::OtherMode::from_raw(2 << 20, 0, 0),
                combiner: gbi::CombinerState::default(),
                blender: gbi::BlenderState::default(),
                scissor: None,
                texture: Some(gbi::Texture {
                    format: source_format,
                    size: gbi::ColorImage::BITS_8,
                    width: 1,
                    height: 1,
                    texels: std::rc::Rc::new(vec![255; 4]),
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
                }),
                texture1: None,
                fill_color: 0,
            };
            assert_eq!(
                validate_copy_texture_rectangle(&rectangle, Some(target(destination))).is_ok(),
                destination == gbi::ColorImageLayout::Index8,
                "format {source_format} -> {destination:?}"
            );
        }
    }
}


#[test]
fn copy_source_gate_rejects_ci8_tlut_and_undefined_eight_bit_formats() {
    let target = gbi::ColorImage {
        format: gbi::ColorImage::I_FORMAT,
        size: gbi::ColorImage::BITS_8,
        width: 1,
        address: 0,
    };
    let mut rectangle = gbi::TextureRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 0.0,
        lry: 0.0,
        tile: 0,
        s: 0.0,
        t: 0.0,
        dsdx: 4 << 10,
        dtdy: 1 << 10,
        flip: false,
        other_mode: gbi::OtherMode::from_raw((2 << 20) | (2 << 14), 0, 0),
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState::default(),
        scissor: None,
        texture: Some(gbi::Texture {
            format: gbi::ColorImage::CI_FORMAT,
            size: gbi::ColorImage::BITS_8,
            width: 1,
            height: 1,
            texels: std::rc::Rc::new(vec![255; 4]),
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
        }),
        texture1: None,
        fill_color: 0,
    };
    assert!(validate_copy_texture_rectangle(&rectangle, Some(target)).is_err());

    rectangle.other_mode = gbi::OtherMode::from_raw(2 << 20, 0, 0);
    rectangle.texture.as_mut().unwrap().format = gbi::ColorImage::RGBA_FORMAT;
    assert!(validate_copy_texture_rectangle(&rectangle, Some(target)).is_err());
    rectangle.texture.as_mut().unwrap().format = 1;
    assert!(validate_copy_texture_rectangle(&rectangle, Some(target)).is_err());
}


#[test]
fn copy_ci8_indices_directly_to_eight_bit_color_image() {
    assert_eq!(
        run_direct_8bit_copy(
            gbi::ColorImage::CI_FORMAT,
            4,
            1,
            &[0, 1, 0x7f, 0xff],
            Some(1),
        ),
        [0xaa, 1, 0x7f, 0xff]
    );
}


#[test]
fn copy_i8_preserves_source_bytes_and_uses_intensity_as_alpha() {
    assert_eq!(
        run_direct_8bit_copy(
            gbi::ColorImage::I_FORMAT,
            8,
            1,
            &[0, 0x7f, 0x80, 0xff, 0x20, 0x81, 0x01, 0xfe],
            Some(0x80),
        ),
        [0xaa, 0xaa, 0x80, 0xff, 0xaa, 0x81, 0xaa, 0xfe]
    );
}


#[test]
fn copy_ia8_preserves_packed_bytes_and_compares_expanded_alpha_nibble() {
    assert_eq!(
        run_direct_8bit_copy(
            gbi::ColorImage::IA_FORMAT,
            8,
            1,
            &[0x10, 0x17, 0x28, 0x4f, 0xf8, 0xe9, 0xa0, 0xbf],
            Some(0x88),
        ),
        [0xaa, 0xaa, 0x28, 0x4f, 0xf8, 0xe9, 0xaa, 0xbf]
    );
}


#[test]
fn copy_ia8_preserves_odd_tmem_row_layout_without_alpha_compare() {
    let source = [
        0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xa9, 0xba, 0xcb, 0xdc, 0xed,
        0xfe, 0x0f,
    ];
    assert_eq!(
        run_direct_8bit_copy(gbi::ColorImage::IA_FORMAT, 8, 2, &source, None),
        source
    );
}


#[test]
fn flipped_copy_texture_rectangle_transposes_rgba16_into_color_image() {
    const DL: usize = 0x100;
    const TEXTURE: u32 = 0x600;
    const TARGET: u32 = 0x800;
    let mut rdram = vec![0u8; 0x1000];
    let source = [0xf801u16, 0x07c1, 0x003f, 0xffff];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in source.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
        }
    }
    let mut offset = DL;
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    write_command(&mut rdram, offset, 0xef00_0000 | (2 << 20), 0);
    offset += 8;
    write_command(&mut rdram, offset, 0xff10_0001, TARGET);
    offset += 8;
    write_command(&mut rdram, offset, 0xfd10_0001, TEXTURE);
    offset += 8;
    write_command(&mut rdram, offset, 0xf510_0200, 7 << 24);
    offset += 8;
    write_command(&mut rdram, offset, 0xf400_0000, (7 << 24) | (4 << 12) | 4);
    offset += 8;
    write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
    offset += 8;
    write_command(&mut rdram, offset, 0xf200_0000, 0x0000_4004);
    offset += 8;
    // Inclusive 2x2 copy rectangle. FLIP makes S advance down screen Y
    // and T advance across screen X while copy-mode dsdx retains 4<<10.
    write_command(&mut rdram, offset, 0xe500_0000 | (4 << 12) | 4, 0);
    offset += 8;
    write_command(&mut rdram, offset, 0, 0x1000_0400);
    offset += 8;
    write_command(&mut rdram, offset, 0xe900_0000, 0);
    offset += 8;
    write_command(&mut rdram, offset, 0xdf00_0000, 0);

    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
    backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    let expected = [source[0], source[2], source[1], source[3]];
    for (index, pixel) in expected.into_iter().enumerate() {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + index as u32 * 2
            )),
            pixel,
            "transposed copy pixel {index}"
        );
    }
}


#[test]
fn one_cycle_texture_rectangle_runs_combiner_into_commanded_rdram_image() {
    const DL: usize = 0x100;
    const TEXTURE: u32 = 0x600;
    const TARGET: u32 = 0x800;
    let mut rdram = vec![0u8; 0x1000];
    let source = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in source.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
        }
    }
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    let combine_command = |rgb: [u32; 4], alpha: [u32; 4]| {
        let w0 = 0xfc00_0000
            | ((rgb[0] & 0x0f) << 20)
            | ((rgb[2] & 0x1f) << 15)
            | ((alpha[0] & 0x07) << 12)
            | ((alpha[2] & 0x07) << 9)
            | ((rgb[0] & 0x0f) << 5)
            | (rgb[2] & 0x1f);
        let w1 = ((rgb[1] & 0x0f) << 28)
            | ((rgb[1] & 0x0f) << 24)
            | ((alpha[0] & 0x07) << 21)
            | ((alpha[2] & 0x07) << 18)
            | ((rgb[3] & 0x07) << 15)
            | ((alpha[1] & 0x07) << 12)
            | ((alpha[3] & 0x07) << 9)
            | ((rgb[3] & 0x07) << 6)
            | ((alpha[1] & 0x07) << 3)
            | (alpha[3] & 0x07);
        (w0, w1)
    };

    let mut offset = DL;
    // (0-0)*0+TEXEL0 for RGBA in both programmed combiner slots.
    let (combine_w0, combine_w1) = combine_command([8, 8, 31, 1], [7, 7, 7, 1]);
    write_command(&mut rdram, offset, combine_w0, combine_w1);
    offset += 8;
    write_command(&mut rdram, offset, 0xff10_0003, TARGET);
    offset += 8;
    write_command(&mut rdram, offset, 0xfd10_0003, TEXTURE);
    offset += 8;
    write_command(&mut rdram, offset, 0xf510_0000, 7 << 24);
    offset += 8;
    write_command(
        &mut rdram,
        offset,
        0xf300_0000,
        (7 << 24) | (7 << 12) | 0x800,
    );
    offset += 8;
    write_command(&mut rdram, offset, 0xf510_0200, 0x0008_0200);
    offset += 8;
    write_command(&mut rdram, offset, 0xf200_0000, 0x0000_c004);
    offset += 8;
    // One-cycle lower/right bounds are exclusive: (0,0)..(4,2).
    write_command(
        &mut rdram,
        offset,
        0xe400_0000 | ((4 * 4) << 12) | (2 * 4),
        0,
    );
    offset += 8;
    write_command(&mut rdram, offset, 0, 0x0400_0400);
    offset += 8;
    write_command(&mut rdram, offset, 0xe900_0000, 0);
    offset += 8;
    write_command(&mut rdram, offset, 0xdf00_0000, 0);

    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    for (index, expected) in source.into_iter().enumerate() {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + index as u32 * 2
            )),
            expected,
            "combined pixel {index}"
        );
    }
}


#[test]
fn combined_texture_rectangle_rejects_unmodeled_state_by_name() {
    let texture = gbi::Texture {
        format: 0,
        size: 2,
        width: 1,
        height: 1,
        texels: std::rc::Rc::new(vec![255; 4]),
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
    let mut rectangle = gbi::TextureRectangle {
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
        other_mode: gbi::OtherMode::default(),
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState {
            cycle_count: 1,
            ..gbi::BlenderState::default()
        },
        scissor: None,
        texture: Some(texture),
        texture1: None,
        fill_color: 0,
    };

    let shade_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
    assert!(shade_error.to_string().contains("selects SHADE"));
    assert!(shade_error
        .to_string()
        .contains("rectangle commands carry no shade attributes"));

    let passthrough = gbi::CombinerCycle {
        rgb: [
            gbi::ColorSource::Zero,
            gbi::ColorSource::Zero,
            gbi::ColorSource::Zero,
            gbi::ColorSource::Texel0,
        ],
        alpha: [
            gbi::AlphaSource::Zero,
            gbi::AlphaSource::Zero,
            gbi::AlphaSource::Zero,
            gbi::AlphaSource::Texel0,
        ],
    };
    rectangle.combiner.mode.cycles = [passthrough; 2];
    rectangle.other_mode = gbi::OtherMode::from_raw(gbi::OtherMode::default().raw_high(), 3, 0);
    validate_texture_rectangle(&rectangle, None)
        .expect("G_AC_DITHER is implemented for combined rectangles");

    rectangle.other_mode =
        gbi::OtherMode::from_raw(gbi::OtherMode::default().raw_high(), 0x10, 0);
    let depth_error = validate_texture_rectangle(&rectangle, None).unwrap_err();
    assert!(depth_error
        .to_string()
        .contains("rectangles require G_ZS_PRIM"));
}


#[test]
fn copy_texture_rectangle_rejects_mismatched_memory_layouts() {
    let texture = gbi::Texture {
        format: gbi::ColorImage::CI_FORMAT,
        size: gbi::ColorImage::BITS_8,
        width: 1,
        height: 1,
        texels: std::rc::Rc::new(vec![1; 4]),
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
    let mut rectangle = gbi::TextureRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 1.0,
        lry: 1.0,
        tile: 0,
        s: 0.0,
        t: 0.0,
        dsdx: 4 << 10,
        dtdy: 1 << 10,
        flip: false,
        other_mode: gbi::OtherMode::from_raw(2 << 20, 0, 0),
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState::default(),
        scissor: None,
        texture: Some(texture),
        texture1: None,
        fill_color: 0,
    };
    let rgba16_target = gbi::ColorImage {
        format: gbi::ColorImage::RGBA_FORMAT,
        size: gbi::ColorImage::BITS_16,
        width: 1,
        address: 0,
    };
    let index8_target = gbi::ColorImage {
        format: gbi::ColorImage::CI_FORMAT,
        size: gbi::ColorImage::BITS_8,
        width: 1,
        address: 0,
    };
    rectangle.other_mode = gbi::OtherMode::from_raw(2 << 20, 3, 0);
    validate_texture_rectangle(&rectangle, Some(index8_target))
        .expect("G_AC_DITHER is implemented for direct CI8 copy rectangles");
    let error = validate_texture_rectangle(&rectangle, Some(rgba16_target)).unwrap_err();
    assert!(error.to_string().contains("does not match color target"));
    assert!(error.to_string().contains("format=0 size=2"));
}


#[test]
fn admitted_s2dex_object_rectangle_renders_preloaded_tmem_to_rdram() {
    const SETUP: usize = 0x100;
    const DL: usize = 0x300;
    const SPRITE: u32 = 0x400;
    const TEXTURE: u32 = 0x800;
    const TARGET: u32 = 0x1000;
    let mut rdram = vec![0u8; 0x2000];
    let source = [
        0xf801u16, 0x07c1, 0x003f, 0xffff, 0x07ff, 0xf83f, 0xffc1, 0x0001,
    ];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in source.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
        }
        let base = fn64_runtime::RdramAddr::from_offset(SPRITE);
        let mut half = |offset, value| view.write_u16(base.checked_add(offset).unwrap(), value);
        half(0, 0); // objX, s10.2
        half(2, 1 << 10); // scaleW, u5.10
        half(4, 4 << 5); // imageW, u10.5
        half(6, 0);
        half(8, 0); // objY, s10.2
        half(10, 1 << 10); // scaleH, u5.10
        half(12, 2 << 5); // imageH, u10.5
        half(14, 0);
        half(16, 1); // one 64-bit word per four-pixel RGBA16 row
        half(18, 0); // TMEM word zero
        view.write_u8(base.checked_add(20).unwrap(), 0); // RGBA
        view.write_u8(base.checked_add(21).unwrap(), 2); // 16-bit
        view.write_u8(base.checked_add(22).unwrap(), 0); // palette
        view.write_u8(base.checked_add(23).unwrap(), 0); // no flips
    }
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };

    // Establish persistent RDP state/TMEM through the existing raw-DPC
    // path. Public S2DEX keeps texture loading separate from sprite draw.
    // (0-0)*0+TEXEL0 in both programmed combiner cycles.
    let combine_texel0 = (0xfc8f_ff1f, 0x88fc_f279);
    let setup = [
        combine_texel0,
        (0xff10_0003, TARGET),
        (0xfd10_0003, TEXTURE),
        (0xf510_0000, 7 << 24),
        (0xf300_0000, (7 << 24) | (7 << 12) | 0x800),
    ];
    for (index, (w0, w1)) in setup.into_iter().enumerate() {
        write_command(&mut rdram, SETUP + index * 8, w0, w1);
    }
    write_command(&mut rdram, DL, 0x0100_0000, SPRITE);
    write_command(&mut rdram, DL + 8, 0xdf00_0000, 0);
    let mut direct_rdram = rdram.clone();

    let mut backend = ReferenceBackend::new()
        .with_s2dex()
        .with_s2dex_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    backend
        .process_rdp_commands(
            &mut rdram,
            SETUP as u32,
            (SETUP + setup.len() * 8) as u32,
            0,
        )
        .unwrap();
    assert_eq!(backend.supported_ucodes(), &[UcodeId::S2dex2]);
    assert_eq!(
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap(),
        FrameStatus::Complete
    );

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    for (index, expected) in source.into_iter().enumerate() {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(
                TARGET + index as u32 * 2
            )),
            expected,
            "S2DEX object pixel {index} must come from preloaded TMEM"
        );
    }

    // Differential: execute the exact RDP tile + texture-rectangle state
    // S2DEX is documented to generate and require byte-identical output.
    const DIRECT: usize = 0x500;
    let equivalent_rdp = [
        (0xf510_0200, 0x0008_0200), // RGBA16, line=1, clamp S/T
        (0xf200_0000, 0x0000_c004), // 4x2 render-tile extent
        (0xe401_0008, 0),           // exclusive (0,0)..(4,2)
        (0, 0x0400_0400),           // s=t=0, unit S/T gradients
        (0xe900_0000, 0),
    ];
    for (index, (w0, w1)) in equivalent_rdp.into_iter().enumerate() {
        write_command(&mut direct_rdram, DIRECT + index * 8, w0, w1);
    }
    let mut direct = ReferenceBackend::new();
    direct.create(&RenderConfig::ntsc(4, 2)).unwrap();
    direct
        .process_rdp_commands(
            &mut direct_rdram,
            SETUP as u32,
            (SETUP + setup.len() * 8) as u32,
            0,
        )
        .unwrap();
    direct
        .process_rdp_commands(
            &mut direct_rdram,
            DIRECT as u32,
            (DIRECT + equivalent_rdp.len() * 8) as u32,
            0,
        )
        .unwrap();
    let s2dex_target = &rdram[TARGET as usize..TARGET as usize + source.len() * 2];
    let direct_target = &direct_rdram[TARGET as usize..TARGET as usize + source.len() * 2];
    assert_eq!(
        s2dex_target, direct_target,
        "S2DEX lowering must match the equivalent raw RDP rectangle byte-for-byte"
    );
}


#[test]
fn s2dex_backend_reports_only_admitted_wire_families() {
    let legacy = [1; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let modern = [2; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let backend = ReferenceBackend::new().with_s2dex();
    assert!(backend.supported_ucodes().is_empty());

    let backend = backend.with_s2dex_ucode_text_for(S2dexWireFamily::S2dex, &legacy);
    assert_eq!(backend.supported_ucodes(), &[UcodeId::S2dex]);

    let backend = backend.with_s2dex_ucode_text(&modern);
    assert_eq!(
        backend.supported_ucodes(),
        &[UcodeId::S2dex, UcodeId::S2dex2]
    );
}


#[test]
fn admitted_legacy_s2dex_digest_selects_legacy_command_bytes() {
    const DL: usize = 0x100;
    let text = [0; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let mut rdram = vec![0u8; 0x200];
    rdram[DL..DL + 4].copy_from_slice(&0xb800_0000u32.to_ne_bytes());
    let mut backend = ReferenceBackend::new()
        .with_s2dex()
        .with_s2dex_ucode_text_for(S2dexWireFamily::S2dex, &text);
    backend.create(&RenderConfig::ntsc(1, 1)).unwrap();
    assert_eq!(
        backend
            .process_task(
                &mut rdram,
                &mut fn64_runtime::RspMemory::new(),
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap(),
        FrameStatus::Complete
    );
}


#[test]
fn s2dex_unsupported_load_command_traps_by_public_name() {
    fn64_runtime::arm_unsupported_events(None).unwrap();
    const DL: usize = 0x100;
    let mut rdram = vec![0u8; 0x200];
    rdram[DL..DL + 4].copy_from_slice(&0x0500_0017u32.to_ne_bytes());
    rdram[DL + 4..DL + 8].copy_from_slice(&0x180u32.to_ne_bytes());
    let before = rdram.clone();
    let mut backend = ReferenceBackend::new()
        .with_s2dex()
        .with_s2dex_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
    let error = backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap_err();
    assert!(error.to_string().contains("G_OBJ_LOADTXTR"));
    assert!(error.to_string().contains("unsupported S2DEX command"));
    assert_eq!(rdram, before, "rejected S2DEX decode must not mutate RDRAM");
    let events = fn64_runtime::copy_unsupported_events();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].subsystem,
        fn64_runtime::UnsupportedSubsystem::Render
    );
    assert_eq!(events[0].operation, "render.s2dex.object-texture-type");
    assert_eq!(
        events[0].disposition,
        fn64_runtime::UnsupportedDisposition::ReturnedError
    );
    assert!(events[0].context.contains("G_OBJ_LOADTXTR"));
}


#[test]
fn unadmitted_s2dex_image_requests_lle_without_task_mutation() {
    const DL: usize = 0x100;
    let mut rdram = vec![0u8; 0x200];
    rdram[DL..DL + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
    let before = rdram.clone();
    let mut rsp = fn64_runtime::RspMemory::new();
    rsp.write_bytes(
        fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
        &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    )
    .unwrap();
    let rsp_before = rsp.clone();
    let expected =
        gbi::UcodeDigest::from_text(rsp.bank(fn64_runtime::RspMemoryBank::Imem)).as_bytes();
    let mut backend = ReferenceBackend::new().with_s2dex();
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
    assert_eq!(
        backend
            .process_task(
                &mut rdram,
                &mut rsp,
                &OsTask {
                    task_type: fn64_render::M_GFXTASK,
                    data_ptr: DL as u32,
                    ..OsTask::default()
                },
                0,
            )
            .unwrap(),
        FrameStatus::NeedsLle {
            ucode_sha256: expected
        }
    );
    assert_eq!(rdram, before);
    assert_eq!(rsp, rsp_before);
}

/// A FILL-cycle texture rectangle is not invalid. The N64brew RDP command
/// table says, in the Texture Rectangle section: "In FILL mode this behaves
/// identically to Fill Rectangle, the texturing properties are ignored."
/// The reference backend used to reject it, which aborted a WCW/nWo Revenge
/// frame over a combination the hardware defines.
#[test]
fn fill_cycle_texture_rectangle_is_accepted_and_converts_to_a_fill_rectangle() {
    let fill_cycle = gbi::OtherMode::from_raw(3 << 20, 0, 0);
    assert_eq!(fill_cycle.cycle_type(), gbi::CycleType::Fill);

    let rectangle = gbi::TextureRectangle {
        ulx: 1.0,
        uly: 2.0,
        lrx: 5.0,
        lry: 6.0,
        tile: 0,
        s: 0.0,
        t: 0.0,
        dsdx: 1 << 10,
        dtdy: 1 << 10,
        flip: false,
        other_mode: fill_cycle,
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState::default(),
        scissor: None,
        texture: None,
        texture1: None,
        fill_color: 0x07c1_07c1,
    };

    validate_texture_rectangle(&rectangle, None)
        .expect("a FILL-cycle texture rectangle is documented, not invalid");

    // The conversion the backend dispatches through must carry the geometry
    // unchanged and take its colour from the fill register.
    let fill = rectangle.as_fill_cycle_rectangle();
    assert_eq!((fill.ulx, fill.uly, fill.lrx, fill.lry), (1.0, 2.0, 5.0, 6.0));
    assert_eq!(fill.fill_color, 0x07c1_07c1);
    assert_eq!(fill.cycle_type, gbi::CycleType::Fill);
}

/// The fill-cycle blender hazard is a property of the cycle, not the command,
/// so it must still reject -- and must name the texture rectangle rather than
/// G_FILLRECT, so the diagnostic points at what the guest submitted.
#[test]
fn fill_cycle_texture_rectangle_still_rejects_an_unsafe_blender_contract() {
    // IM_RD (bit 6) retains a framebuffer consumer; a depth read in fill
    // cycle can hang the RDP.
    let hazardous = gbi::OtherMode::from_raw(3 << 20, 1 << 6, 0);
    let rectangle = gbi::TextureRectangle {
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
        other_mode: hazardous,
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState::default(),
        scissor: None,
        texture: None,
        texture1: None,
        fill_color: 0,
    };

    let error = validate_texture_rectangle(&rectangle, None).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("G_TEXRECT"), "must name the command: {text}");
    assert!(text.contains("Fill cycle retains unsafe"), "{text}");
}
