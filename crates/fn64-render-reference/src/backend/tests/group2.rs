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
fn raw_shade_texture_z_triangle_executes_maximum_width_layout() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    const TEXTURE: u32 = 0x800;
    let mut rdram = vec![0u8; 0x1000];
    let source = [0xf801u16, 0x07c1, 0x003f, 0xffff, 0, 0, 0, 0];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in source.into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
        }
    }
    let mut offset = START;
    {
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        command(0xff10_0007, TARGET); // RGBA16 width 8
        command(0xfd10_0003, TEXTURE); // RGBA16 width 4
        command(0xf510_0000, 7 << 24); // load tile 7, contiguous TMEM
        command(0xf300_0000, (7 << 24) | (7 << 12) | 0x800); // 8 texels
        command(0xf510_0200, 0x0008_0200); // render tile 0, clamp S/T
        command(0xf200_0000, 0x0000_c004); // 4x2 render extent
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        let dsde = (5.0f32 / 6.0 * 65536.0).round() as u32;
        command(0x0f00_0000 | yl, (ym << 16) | yh);
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        command(0x00ff_00ff, 0x00ff_00ff); // opaque white base shade
        command(0, 0);
        command(0, 0);
        command(0, 0);
        command(0, 0);
        command(0, 0);
        command(0, 0);
        command(0, 0);
        command(0, 1024 << 16); // S=0, T=0, perspective unity W
        command(1 << 16, 0); // dS/dX=1
        command(0, 0);
        command(0, 0);
        command((dsde >> 16) << 16, 0);
        command(0, 0);
        command((dsde & 0xffff) << 16, 0);
        command(0, 0);
        command(4 << 16, 0); // Z
        command(0, 0);
        command(0xe900_0000, 0);
    }
    let end = offset;
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();

    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    let pixel = |x: u32, y: u32| {
        view.read_u16(fn64_runtime::RdramAddr::from_offset(
            TARGET + (y * 8 + x) * 2,
        ))
    };
    assert_eq!(pixel(2, 4), 0x07c1);
    assert_eq!(pixel(3, 4), 0x003f);
}


#[test]
fn raw_command_stream_triangle_selects_mips_and_trilinear_fraction() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    const TEXTURES: [u32; 3] = [0x800, 0x810, 0x820];
    let mut rdram = vec![0u8; 0x1000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (address, texel) in TEXTURES.into_iter().zip([0xf801, 0x0001, 0xffff]) {
            view.write_u16(fn64_runtime::RdramAddr::from_offset(address), texel);
        }
    }

    let mut offset = START;
    {
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        let combine_w0 = 0xfc00_0000
            | (2 << 20) // cycle 0 A = TEXEL1
            | (13 << 15) // cycle 0 C = LOD_FRACTION
            | (2 << 12) // cycle 0 alpha A = TEXEL1
            | (8 << 5) // cycle 1 A = ZERO
            | 31; // cycle 1 C = ZERO
        let combine_w1 = (1 << 28) // cycle 0 B = TEXEL0
            | (8 << 24) // cycle 1 B = ZERO
            | (7 << 21) // cycle 1 alpha A = ZERO
            | (7 << 18) // cycle 1 alpha C = ZERO
            | (1 << 15) // cycle 0 D = TEXEL0
            | (1 << 12) // cycle 0 alpha B = TEXEL0
            | (1 << 9) // cycle 0 alpha D = TEXEL0
            | (7 << 3); // cycle 1 alpha B = ZERO; D = COMBINED

        command(0xff10_0007, TARGET); // RGBA16 width 8
                                      // Two-cycle, texture LOD enabled, clamp-detail mode, filter-only,
                                      // and deterministic dither disable. Raw edge `level=2` below is
                                      // the RDP primitive's maximum mip level.
        command(
            0xef00_0000 | (1 << 20) | (1 << 19) | (1 << 16) | (6 << 9) | 0xf0,
            0,
        );
        command(combine_w0, combine_w1);
        for (tile, address) in TEXTURES.into_iter().enumerate() {
            let tile = tile as u32;
            command(0xfd10_0000, address); // RGBA16 width 1
            command(0xf510_0200 | tile, (tile << 24) | 0x0008_0200);
            command(0xf200_0000, tile << 24); // 1x1 render tile
            command(0xf300_0000, tile << 24); // load into that tile
        }

        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        command(0x0a00_0000 | (2 << 19) | yl, (ym << 16) | yh);
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        // S=T=0, perspective-unity W; dS/dX=dT/dY=2.5. Chapter 13.7 selects
        // tiles 1 and 2 with LOD fraction 0.25.
        command(0, 1024 << 16);
        command(2 << 16, 0);
        command(0, 0);
        command(0x8000_0000, 0);
        command(0, 0);
        command(2, 0);
        command(0, 0);
        command(0x0000_8000, 0);
        command(0xe900_0000, 0);
    }
    let end = offset;
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(
            TARGET + (4 * 8 + 2) * 2
        )),
        0x4211,
        "LOD 2.5 must blend one quarter from black tile 1 toward white tile 2"
    );
}


#[test]
fn raw_yuv_texture_rectangle_applies_set_convert_into_rdram() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    const TEXTURE: u32 = 0x600;
    let mut rdram = vec![0u8; 0x800];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        // Public RDP YUV16 byte order: Y0, U, Y1, V. Neutral chroma
        // makes the default public conversion table preserve each Y as
        // equal R/G/B, which gives this gate unambiguous expected pixels.
        for (index, value) in [16, 128, 235, 128].into_iter().enumerate() {
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32),
                value,
            );
        }
    }

    let field = |value: i16| u32::from(value as u16) & 0x1ff;
    let [k0, k1, k2, k3, k4, k5] = [175, -43, -89, 222, 114, 42].map(field);
    let set_convert = (
        0xec00_0000 | (k0 << 13) | (k1 << 4) | ((k2 >> 5) & 0x0f),
        ((k2 & 0x1f) << 27) | (k3 << 18) | (k4 << 9) | k5,
    );
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

    let mut offset = START;
    {
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        // One-cycle, point sampled, G_TC_CONV, with color/alpha dither
        // disabled so this gate isolates the conversion table.
        command(0xef00_00f0, 0);
        command(set_convert.0, set_convert.1);
        let (combine_w0, combine_w1) = combine_command([8, 8, 31, 1], [7, 7, 7, 1]);
        command(combine_w0, combine_w1); // (0-0)*0+TEXEL0
        command(0xff10_0001, TARGET); // RGBA16 width 2
        command(0xfd30_0001, TEXTURE); // YUV16 width 2
        command(0xf530_0000, 7 << 24); // YUV16 load tile 7
        command(0xf300_0000, (7 << 24) | (1 << 12) | 0x800); // YUYV pair
        command(0xf530_0200, 0x0008_0200); // YUV16 render tile 0
        command(0xf200_0000, 0x0000_4000); // 2x1 render extent
        command(0xe400_0000 | ((2 * 4) << 12) | 4, 0);
        command(0, 0x0400_0400); // S/T=0, dS/dX=dT/dY=1
        command(0xe900_0000, 0);
    }
    let end = offset;
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(2, 1)).unwrap();

    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
        0x1085
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
        0xef7b
    );
}


#[test]
fn raw_chroma_key_commands_drive_alpha_fixup_and_compare() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    const TEXTURE: u32 = 0x600;
    let mut rdram = vec![0u8; 0x800];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in [0x07c1u16, 0xf801].into_iter().enumerate() {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32 * 2),
                pixel,
            );
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(TARGET + index as u32 * 2),
                0xffff,
            );
        }
    }
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

    let mut offset = START;
    {
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        // One-cycle, filter-only, chroma key enabled, alpha threshold on.
        command(0xef00_0df0, 1);
        command(0xf900_0000, 0x0000_0080); // threshold alpha = 128
        command(0xea10_0100, 0xffff_00ff); // center green, unit widths/scales
        command(0xeb00_0000, 0x0100_00ff);
        let (combine_w0, combine_w1) = combine_command([1, 6, 6, 7], [7, 7, 7, 7]);
        command(combine_w0, combine_w1); // (TEXEL0-CENTER)*SCALE
        command(0xff10_0001, TARGET); // RGBA16 width 2
        command(0xfd10_0001, TEXTURE); // RGBA16 width 2
        command(0xf510_0000, 7 << 24); // load tile 7, contiguous TMEM
        command(0xf300_0000, (7 << 24) | (1 << 12) | 0x800); // 2 texels
        command(0xf510_0200, 0x0008_0200); // render tile 0, clamp S/T
        command(0xf200_0000, 0x0000_4000); // 2x1 render extent
        command(0xe400_0000 | ((2 * 4) << 12) | 4, 0);
        command(0, 0x0400_0400);
        command(0xe900_0000, 0);
    }
    let end = offset;
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(2, 1)).unwrap();

    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
        0x0001
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
        0xffff
    );
}


#[test]
fn reference_backend_auto_dump_can_skip_to_a_late_task_window() {
    let backend = ReferenceBackend::new()
        .with_auto_dump("/tmp", "fn64-render-test", 3)
        .with_auto_dump_skip(4_180);
    let dump = backend.auto_dump.unwrap();
    assert_eq!(dump.task_index, 0);
    assert_eq!(dump.skip_before_task, 4_180);
    assert_eq!(dump.written, 0);
    assert_eq!(dump.limit, 3);
    assert!(!dump.limit_reported);
}


#[test]
fn framebuffer_writer_and_runtime_view_agree_on_logical_pixel_order() {
    let mut framebuffer = Framebuffer::new(2, 1);
    framebuffer.pixels[0..4].copy_from_slice(&[255, 0, 0, 255]);
    framebuffer.pixels[4..8].copy_from_slice(&[0, 0, 255, 255]);
    let mut storage = [0u8; 4];
    let mut hidden_bits = RdramHiddenBits::new();

    write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);

    let view = fn64_runtime::RdramView::from_storage(&storage);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(0)),
        0xF801,
        "pixel 0 must be logical RGBA5551 red"
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(2)),
        0x003F,
        "pixel 1 must be logical RGBA5551 blue"
    );
    assert_eq!(
        storage,
        [0x3F, 0x00, 0x01, 0xF8],
        "native-word storage must contain the two logical halfwords in lane-mapped order"
    );
}


#[test]
fn disabled_dither_rgba16_truncates_low_three_bits() {
    let mut framebuffer = Framebuffer::new(1, 1);
    framebuffer.pixels.copy_from_slice(&[7, 8, 15, 255]);
    let mut storage = [0u8; 4];
    let mut hidden_bits = RdramHiddenBits::new();

    write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);

    let view = fn64_runtime::RdramView::from_storage(&storage);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(0)),
        0x0043,
        "7 must remain zero while 8 and 15 truncate to one; round-to-nearest would change both boundary channels"
    );
}


#[test]
fn rgba16_coverage_round_trips_visible_and_hidden_storage_bits() {
    let mut framebuffer = Framebuffer::new(8, 1);
    framebuffer.pixels.fill(255);
    for (index, coverage) in framebuffer.coverage.iter_mut().enumerate() {
        *coverage = raster::Coverage::new(index as u8 + 1);
    }
    let mut storage = [0u8; 16];
    let mut hidden_bits = RdramHiddenBits::new();

    write_rgba5551_framebuffer(&mut storage, 0, &framebuffer, &mut hidden_bits);
    let view = fn64_runtime::RdramView::from_storage(&storage);
    for index in 0..8u32 {
        let address = index * 2;
        let visible = view.read_u16(fn64_runtime::RdramAddr::from_offset(address));
        let stored = index as u8;
        assert_eq!((visible & 1) as u8, stored >> 2);
        assert_eq!(
            hidden_bits.get(&address).map(|sample| sample.bits),
            Some(stored & 3)
        );
    }

    let mut loaded = Framebuffer::new(8, 1);
    load_rgba5551_framebuffer(
        &storage,
        gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_16,
            width: 8,
            address: 0,
        },
        &mut loaded,
        &mut hidden_bits,
    );
    assert_eq!(loaded.coverage, framebuffer.coverage);
}


#[test]
fn rgba32_round_trips_five_bit_alpha_and_three_bit_coverage() {
    let mut framebuffer = Framebuffer::new(2, 1);
    framebuffer
        .pixels
        .copy_from_slice(&[0x12, 0x34, 0x56, 0x29, 0xab, 0xcd, 0xef, 0xbd]);
    framebuffer.coverage[0] = raster::Coverage::new(3);
    framebuffer.coverage[1] = raster::Coverage::FULL;
    let mut storage = [0u8; 8];

    write_rgba8888_framebuffer(&mut storage, 0, &framebuffer);
    let view = fn64_runtime::RdramView::from_storage(&storage);
    assert_eq!(
        view.read_u32(fn64_runtime::RdramAddr::from_offset(0)),
        0x1234_5645
    );
    assert_eq!(
        view.read_u32(fn64_runtime::RdramAddr::from_offset(4)),
        0xabcd_eff7
    );

    let mut loaded = Framebuffer::new(2, 1);
    load_rgba8888_framebuffer(
        &storage,
        gbi::ColorImage {
            format: gbi::ColorImage::RGBA_FORMAT,
            size: gbi::ColorImage::BITS_32,
            width: 2,
            address: 0,
        },
        &mut loaded,
    );
    assert_eq!(loaded.pixels, framebuffer.pixels);
    assert_eq!(loaded.coverage, framebuffer.coverage);
}


#[test]
fn rgba32_memory_alpha_truncates_low_three_bits() {
    let mut framebuffer = Framebuffer::new(2, 1);
    framebuffer
        .pixels
        .copy_from_slice(&[1, 2, 3, 7, 4, 5, 6, 8]);
    let mut storage = [0u8; 8];

    write_rgba8888_framebuffer(&mut storage, 0, &framebuffer);

    let view = fn64_runtime::RdramView::from_storage(&storage);
    assert_eq!(
        view.read_u32(fn64_runtime::RdramAddr::from_offset(0)),
        0x0102_03e0
    );
    assert_eq!(
        view.read_u32(fn64_runtime::RdramAddr::from_offset(4)),
        0x0405_06e1
    );
}


#[test]
fn changed_cpu_visible_word_reconstructs_its_hidden_bits_from_the_lsb() {
    let mut hidden_bits = RdramHiddenBits::from([(
        0,
        RdramHiddenSample {
            visible: 1,
            bits: 1,
        },
    )]);
    assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 0), 0);
    assert_eq!(
        hidden_bits.get(&0),
        Some(RdramHiddenSample {
            visible: 0,
            bits: 0,
        })
    );
    assert_eq!(read_rdram_hidden_bits(&mut hidden_bits, 0, 1), 3);
}


#[test]
fn known_same_value_non_rdp_write_replicates_the_visible_lsb() {
    let mut backend = ReferenceBackend::new();
    let mut visible = vec![0u8; 8];
    fn64_runtime::RdramViewMut::from_storage(&mut visible)
        .write_u16(fn64_runtime::RdramAddr::from_offset(2), 0x1235);
    backend.rdram_hidden_bits = RdramHiddenBits::from([
        (
            0,
            RdramHiddenSample {
                visible: 0x1234,
                bits: 2,
            },
        ),
        (
            2,
            RdramHiddenSample {
                visible: 0x1235,
                bits: 1,
            },
        ),
    ]);

    assert_eq!(
        backend.observe_non_rdp_write16(NonRdpWrite16::new(0, 0x1234)),
        NonRdpWrite16Disposition::AppliedHiddenSidecar
    );
    assert_eq!(
        backend.observe_non_rdp_write16(NonRdpWrite16::new(2, 0x1235)),
        NonRdpWrite16Disposition::AppliedHiddenSidecar
    );
    assert_eq!(backend.rdram_hidden_bits.get(&0).unwrap().bits, 0);
    assert_eq!(backend.rdram_hidden_bits.get(&2).unwrap().bits, 3);
    assert_eq!(
        fn64_runtime::RdramView::from_storage(&visible)
            .read_u16(fn64_runtime::RdramAddr::from_offset(2)),
        0x1235,
        "renderer-owned hidden-bit repair must not mutate coherent CPU-visible bytes"
    );
    assert_eq!(
        backend.observe_non_rdp_write16(NonRdpWrite16::new(4, 0xffff)),
        NonRdpWrite16Disposition::NoRustHiddenSidecar
    );
}


#[test]
fn index8_commit_preserves_hidden_bits_across_partial_halfword_overlap() {
    let index8 = gbi::ColorImage {
        format: gbi::ColorImage::CI_FORMAT,
        size: gbi::ColorImage::BITS_8,
        width: 3,
        address: 0,
    };
    let rgba16 = gbi::ColorImage {
        format: gbi::ColorImage::RGBA_FORMAT,
        size: gbi::ColorImage::BITS_16,
        width: 2,
        address: 0,
    };
    let mut rdram = vec![0u8; 8];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u8(fn64_runtime::RdramAddr::from_offset(3), 0x79);
    let untouched = RdramHiddenSample {
        visible: 0xcafe,
        bits: 3,
    };
    let mut hidden_bits = RdramHiddenBits::from([
        (
            0,
            RdramHiddenSample {
                visible: 0xaaaa,
                bits: 2,
            },
        ),
        (
            2,
            RdramHiddenSample {
                visible: 0xbbbb,
                bits: 1,
            },
        ),
        (4, untouched),
    ]);
    let mut source = Framebuffer::new(3, 1);
    for (pixel, intensity) in source.pixels.chunks_exact_mut(4).zip([0x12, 0x34, 0x56]) {
        pixel.copy_from_slice(&[intensity, intensity, intensity, 255]);
    }

    commit_color_image(&mut rdram, index8, &source, &mut hidden_bits);

    assert_eq!(
        hidden_bits.get(&0).unwrap(),
        RdramHiddenSample {
            visible: 0x1234,
            bits: 2
        }
    );
    assert_eq!(
        hidden_bits.get(&2).unwrap(),
        RdramHiddenSample {
            visible: 0x5679,
            bits: 1
        }
    );
    assert_eq!(hidden_bits.get(&4), Some(untouched));
    let mut imported = Framebuffer::new(2, 1);
    load_color_image(&rdram, rgba16, &mut imported, &mut hidden_bits);
    assert_eq!(imported.coverage[0].stored(), 2);
    assert_eq!(imported.coverage[1].stored(), 5);
    assert_eq!(hidden_bits.get(&4), Some(untouched));
}


#[test]
fn rgba32_commit_preserves_each_overlapping_halfword_hidden_pair() {
    let rgba32 = gbi::ColorImage {
        format: gbi::ColorImage::RGBA_FORMAT,
        size: gbi::ColorImage::BITS_32,
        width: 2,
        address: 0,
    };
    let rgba16 = gbi::ColorImage {
        format: gbi::ColorImage::RGBA_FORMAT,
        size: gbi::ColorImage::BITS_16,
        width: 4,
        address: 0,
    };
    let untouched = RdramHiddenSample {
        visible: 0xdead,
        bits: 2,
    };
    let mut hidden_bits = RdramHiddenBits::from([
        (
            0,
            RdramHiddenSample {
                visible: 0,
                bits: 2,
            },
        ),
        (
            2,
            RdramHiddenSample {
                visible: 0,
                bits: 1,
            },
        ),
        (
            4,
            RdramHiddenSample {
                visible: 0,
                bits: 3,
            },
        ),
        (
            6,
            RdramHiddenSample {
                visible: 0,
                bits: 0,
            },
        ),
        (8, untouched),
    ]);
    let mut source = Framebuffer::new(2, 1);
    source
        .pixels
        .copy_from_slice(&[0x10, 0x20, 0x30, 0x08, 0x40, 0x51, 0x60, 0x00]);
    source.coverage.fill(raster::Coverage::new(1));
    let mut rdram = vec![0u8; 12];

    commit_color_image(&mut rdram, rgba32, &source, &mut hidden_bits);

    assert_eq!(
        hidden_bits.get(&0).unwrap(),
        RdramHiddenSample {
            visible: 0x1020,
            bits: 2
        }
    );
    assert_eq!(
        hidden_bits.get(&2).unwrap(),
        RdramHiddenSample {
            visible: 0x3001,
            bits: 1
        }
    );
    assert_eq!(
        hidden_bits.get(&4).unwrap(),
        RdramHiddenSample {
            visible: 0x4051,
            bits: 3
        }
    );
    assert_eq!(
        hidden_bits.get(&6).unwrap(),
        RdramHiddenSample {
            visible: 0x6000,
            bits: 0
        }
    );
    assert_eq!(hidden_bits.get(&8), Some(untouched));
    let mut imported = Framebuffer::new(4, 1);
    load_color_image(&rdram, rgba16, &mut imported, &mut hidden_bits);
    assert_eq!(
        imported
            .coverage
            .iter()
            .map(|coverage| coverage.stored())
            .collect::<Vec<_>>(),
        [2, 5, 7, 0]
    );
    assert_eq!(hidden_bits.get(&8), Some(untouched));
}


#[test]
fn every_public_color_image_transition_commits_then_imports_exact_layouts() {
    const SOURCE: u32 = 0x100;
    const DESTINATION: u32 = 0x200;
    let image = |layout, address| gbi::ColorImage {
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
        width: 4,
        address,
    };
    let expected_bytes = |layout| -> &'static [u8] {
        match layout {
            gbi::ColorImageLayout::Index8 => &[0x18, 0x80, 0xf8, 0x08],
            gbi::ColorImageLayout::Rgba16 => &[0x19, 0x4e, 0x85, 0x30, 0xf8, 0x1f, 0x0f, 0xc1],
            gbi::ColorImageLayout::Rgba32 => &[
                0x18, 0x28, 0x38, 0x09, 0x80, 0xa0, 0xc0, 0x5c, 0xf8, 0x00, 0x78, 0xa4, 0x08,
                0xf8, 0x00, 0xff,
            ],
        }
    };
    let mut original = Framebuffer::new(4, 1);
    original.pixels.copy_from_slice(&[
        0x18, 0x28, 0x38, 0x48, 0x80, 0xa0, 0xc0, 0xe0, 0xf8, 0x00, 0x78, 0x20, 0x08, 0xf8,
        0x00, 0xff,
    ]);
    for (coverage, count) in original.coverage.iter_mut().zip([1, 3, 6, 8]) {
        *coverage = raster::Coverage::new(count);
    }

    for from in gbi::ColorImageLayout::ALL {
        for to in gbi::ColorImageLayout::ALL {
            let source = image(from, SOURCE);
            let destination = image(to, DESTINATION);
            assert_eq!(source.transition_to(destination).from, from);

            let mut rdram = vec![0xcc; 0x400];
            let mut hidden_bits = RdramHiddenBits::new();
            commit_color_image(&mut rdram, destination, &original, &mut hidden_bits);
            commit_color_image(&mut rdram, source, &original, &mut hidden_bits);

            let view = fn64_runtime::RdramView::from_storage(&rdram);
            let actual = (0..expected_bytes(from).len())
                .map(|offset| {
                    view.read_u8(fn64_runtime::RdramAddr::from_offset(SOURCE + offset as u32))
                })
                .collect::<Vec<_>>();
            assert_eq!(actual, expected_bytes(from), "{from:?} -> {to:?}");

            let mut loaded = Framebuffer::new(4, 1);
            load_color_image(&rdram, destination, &mut loaded, &mut hidden_bits);
            match to {
                gbi::ColorImageLayout::Index8 => {
                    assert_eq!(
                        loaded.pixels,
                        [
                            0x18, 0x18, 0x18, 255, 0x80, 0x80, 0x80, 255, 0xf8, 0xf8, 0xf8,
                            255, 0x08, 0x08, 0x08, 255,
                        ],
                        "{from:?} -> {to:?}"
                    );
                    assert!(loaded
                        .coverage
                        .iter()
                        .all(|value| *value == raster::Coverage::FULL));
                }
                gbi::ColorImageLayout::Rgba16 => {
                    assert_eq!(
                        loaded.pixels,
                        [
                            0x18, 0x29, 0x39, 255, 0x84, 0xa5, 0xc6, 255, 0xff, 0x00, 0x7b,
                            255, 0x08, 0xff, 0x00, 255,
                        ],
                        "{from:?} -> {to:?}"
                    );
                    assert_eq!(loaded.coverage, original.coverage);
                }
                gbi::ColorImageLayout::Rgba32 => {
                    assert_eq!(
                        loaded.pixels,
                        [
                            0x18, 0x28, 0x38, 0x4a, 0x80, 0xa0, 0xc0, 0xe7, 0xf8, 0x00, 0x78,
                            0x21, 0x08, 0xf8, 0x00, 0xff,
                        ],
                        "{from:?} -> {to:?}"
                    );
                    assert_eq!(loaded.coverage, original.coverage);
                }
            }
        }
    }
}


#[test]
fn every_public_fill_layout_commits_exact_bytes_and_hidden_ownership() {
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
        width: 4,
        address: 0,
    };
    let rectangle = gbi::FillRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 3.0,
        lry: 0.0,
        fill_color: 0x1234_5678,
        cycle_type: gbi::CycleType::Fill,
        scissor: None,
        other_mode: gbi::OtherMode::default(),
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState::default(),
    };
    for layout in gbi::ColorImageLayout::ALL {
        let mut framebuffer = Framebuffer::new(4, 1);
        framebuffer.draw_fill_rectangle(&rectangle, target(layout));
        let mut rdram = vec![0xcc; 16];
        let sentinel = RdramHiddenSample {
            visible: 0xaaaa,
            bits: 2,
        };
        let mut hidden_bits =
            RdramHiddenBits::from([(0, sentinel), (2, sentinel), (4, sentinel), (6, sentinel)]);
        commit_color_image(&mut rdram, target(layout), &framebuffer, &mut hidden_bits);

        let expected: &[u8] = match layout {
            gbi::ColorImageLayout::Index8 => &[0x12, 0x34, 0x56, 0x78],
            gbi::ColorImageLayout::Rgba16 => &[0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78],
            gbi::ColorImageLayout::Rgba32 => &[
                0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78, 0x12, 0x34, 0x56, 0x78, 0x12,
                0x34, 0x56, 0x78,
            ],
        };
        let view = fn64_runtime::RdramView::from_storage(&rdram);
        let actual = (0..expected.len())
            .map(|offset| view.read_u8(fn64_runtime::RdramAddr::from_offset(offset as u32)))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "{layout:?}");
        for address in [0u32, 2, 4, 6] {
            let fill_halfword = if address.is_multiple_of(4) {
                0x1234
            } else {
                0x5678
            };
            let expected_hidden = match layout {
                gbi::ColorImageLayout::Rgba16 => RdramHiddenSample {
                    visible: fill_halfword,
                    bits: 0,
                },
                gbi::ColorImageLayout::Index8 if address < 4 => RdramHiddenSample {
                    visible: fill_halfword,
                    bits: sentinel.bits,
                },
                gbi::ColorImageLayout::Rgba32 => RdramHiddenSample {
                    visible: fill_halfword,
                    bits: sentinel.bits,
                },
                gbi::ColorImageLayout::Index8 => sentinel,
            };
            assert_eq!(
                hidden_bits.get(&address),
                Some(expected_hidden),
                "{layout:?} at {address}"
            );
        }
    }
}


#[test]
fn fill_cycle_rejects_every_unsafe_bypass_state_before_mutation() {
    let rectangle = |low| gbi::FillRectangle {
        ulx: 0.0,
        uly: 0.0,
        lrx: 1.0,
        lry: 0.0,
        fill_color: 0xffff_ffff,
        cycle_type: gbi::CycleType::Fill,
        scissor: None,
        other_mode: gbi::OtherMode::from_raw(3 << 20, low, 0),
        combiner: gbi::CombinerState::default(),
        blender: gbi::BlenderState::default(),
    };

    assert!(validate_fill_rectangle(&rectangle(0)).is_ok());
    for hazards in 1u32..8 {
        let low = ((hazards & 1) << 4) | ((hazards & 2) << 4) | ((hazards & 4) << 4);
        let error = validate_fill_rectangle(&rectangle(low))
            .expect_err("every nonempty Fill-cycle hazard set must fail closed");
        let message = error.to_string();
        assert!(message.contains("unsafe"));
        assert!(message.contains("G_RM_NOOP/G_RM_NOOP2"));
    }

    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    let commands: [(u32, u32); 5] = [
        (0xff10_0001, TARGET),
        // Fill cycle with IM_RD retained. This was the silent old path:
        // it wrote the target even though the public fill contract
        // requires the bypass-safe NOOP render mode.
        (0xef00_0000 | (3 << 20), 1 << 6),
        (0xf700_0000, 0xffff_ffff),
        (0xf600_0000 | (4 << 12), 0),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0xa5; 0x800];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = START + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let before = rdram.clone();
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
    let error = backend
        .process_rdp_commands(
            &mut rdram,
            START as u32,
            (START + commands.len() * 8) as u32,
            0,
        )
        .expect_err("unsafe Fill-cycle IM_RD must reject before target writeback");
    assert!(error.to_string().contains("unsafe IM_RD state"));
    assert_eq!(rdram, before);
}


#[test]
fn ordered_fill_rectangles_write_the_explicit_color_image() {
    const DL: usize = 0x100;
    const TARGET: u32 = 0x400;
    let mut rdram = vec![0u8; 0x1000];
    let mut offset = DL;
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    // G_RDPSETOTHERMODE: G_CYC_FILL.
    write_command(&mut rdram, offset, 0xef00_0000 | (3 << 20), 0);
    offset += 8;
    // G_SETCIMG RGBA16 width 4.
    write_command(&mut rdram, offset, 0xff10_0003, TARGET);
    offset += 8;
    // Red fill across the full 4x2 target.
    write_command(&mut rdram, offset, 0xf700_0000, 0xf801_f801);
    offset += 8;
    write_command(&mut rdram, offset, 0xf600_0000 | ((3 * 4) << 12) | 4, 0);
    offset += 8;
    // Blue overwrites row 0 pixels 1..2. Keeping two fill operations in
    // one stream proves the decoder/backend no longer groups by primitive.
    write_command(&mut rdram, offset, 0xf700_0000, 0x003f_003f);
    offset += 8;
    write_command(&mut rdram, offset, 0xf600_0000 | ((2 * 4) << 12), 4 << 12);
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
    let expected = [
        0xf801, 0x003f, 0x003f, 0xf801, 0xf801, 0xf801, 0xf801, 0xf801,
    ];
    for (index, expected) in expected.into_iter().enumerate() {
        let address = fn64_runtime::RdramAddr::from_offset(TARGET + index as u32 * 2);
        assert_eq!(view.read_u16(address), expected, "pixel {index}");
    }
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2), 0xffff);
    // RDP target state survives task boundaries. A second task omits
    // G_SETCIMG and must continue drawing the prior color image rather
    // than falling back to output_addr/VI state. The task-boundary import
    // must also retain the CPU's intervening white write to pixel 1.
    let mut second = DL;
    write_command(&mut rdram, second, 0xef00_0000 | (3 << 20), 0);
    second += 8;
    write_command(&mut rdram, second, 0xf700_0000, 0x07c1_07c1);
    second += 8;
    write_command(&mut rdram, second, 0xf600_0000, 0);
    second += 8;
    write_command(&mut rdram, second, 0xe900_0000, 0);
    second += 8;
    write_command(&mut rdram, second, 0xdf00_0000, 0);
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
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
        0x07c1
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET + 2)),
        0xffff,
        "second task must re-import CPU-visible writes to untouched persistent-target pixels"
    );
}


#[test]
fn reference_backend_preserves_rdp_mode_and_fill_registers_between_tasks() {
    const DL: usize = 0x100;
    const TARGET: u32 = 0x400;
    let mut rdram = vec![0u8; 0x800];
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(1, 1)).unwrap();

    // Task one only programs device registers; it emits no pixels.
    write_command(&mut rdram, DL, 0xef00_0000 | (3 << 20), 0);
    write_command(&mut rdram, DL + 8, 0xff10_0000, TARGET);
    write_command(&mut rdram, DL + 16, 0xf700_0000, 0xf801_f801);
    write_command(&mut rdram, DL + 24, 0xdf00_0000, 0);
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

    // Task two deliberately omits SETOTHERMODE, SETCIMG, and SETFILLCOLOR.
    // All three registers belong to the RDP and remain selected.
    write_command(&mut rdram, DL, 0xf600_0000, 0);
    write_command(&mut rdram, DL + 8, 0xe900_0000, 0);
    write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);
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

    assert_eq!(
        fn64_runtime::RdramView::from_storage(&rdram)
            .read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
        0xf801
    );
}


#[test]
fn raw_dpc_and_f3dex2_hle_share_one_persistent_rdp_register_file() {
    const RAW: usize = 0x100;
    const DL: usize = 0x200;
    const TARGET: u32 = 0x400;
    let mut rdram = vec![0u8; 0x800];
    let write_command = |rdram: &mut [u8], offset: usize, w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    };
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(1, 1)).unwrap();

    // A bounded raw DPC submission programs the device without drawing.
    write_command(&mut rdram, RAW, 0xef00_0000 | (3 << 20), 0);
    write_command(&mut rdram, RAW + 8, 0xff10_0000, TARGET);
    write_command(&mut rdram, RAW + 16, 0xf700_0000, 0x07c1_07c1);
    backend
        .process_rdp_commands(&mut rdram, RAW as u32, (RAW + 24) as u32, 0)
        .unwrap();

    // The next admitted HLE task consumes those same registers.
    write_command(&mut rdram, DL, 0xf600_0000, 0);
    write_command(&mut rdram, DL + 8, 0xe900_0000, 0);
    write_command(&mut rdram, DL + 16, 0xdf00_0000, 0);
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

    assert_eq!(
        fn64_runtime::RdramView::from_storage(&rdram)
            .read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
        0x07c1
    );
}


#[test]
fn rgba32_fill_cycle_writes_rgb_alpha_and_coverage_packing() {
    let mut rdram = vec![0u8; 0x1000];
    let commands: [(u32, u32); 6] = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff18_0003, 0x400),
        (0xf700_0000, 0x1234_56e5),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = 0x100 + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
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
    for index in 0..8 {
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(0x400 + index * 4)),
            0x1234_56e5,
            "RGBA32 fill pixel {index}"
        );
    }
    let framebuffer = backend.framebuffer().unwrap();
    assert_eq!(&framebuffer.pixels[..4], &[0x12, 0x34, 0x56, 0x29]);
    assert_eq!(framebuffer.coverage[0], raster::Coverage::FULL);
}


#[test]
fn ordered_target_switch_commits_each_rgba_format_with_its_own_packing() {
    let mut rdram = vec![0u8; 0x1000];
    let commands: [(u32, u32); 9] = [
        (0xef00_0000 | (3 << 20), 0),
        (0xff10_0001, 0x400),
        (0xf700_0000, 0xf801_f801),
        (0xf600_0000 | (4 << 12), 0),
        (0xff18_0001, 0x500),
        (0xf700_0000, 0x1234_56e5),
        (0xf600_0000 | (4 << 12), 0),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = 0x100 + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
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
    for address in [0x400, 0x402] {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
            0xf801
        );
    }
    for address in [0x500, 0x504] {
        assert_eq!(
            view.read_u32(fn64_runtime::RdramAddr::from_offset(address)),
            0x1234_56e5
        );
    }
}


#[test]
fn intensity8_fill_uses_all_four_fill_register_bytes_and_ignores_coverage() {
    let mut rdram = vec![0u8; 0x1000];
    let commands: [(u32, u32); 6] = [
        (0xef00_0000 | (3 << 20), 0),
        // Set Color Image: arbitrary format field, public 8-bit size,
        // width four. Figure 15.5.4 defines size=8 as intensity bytes.
        (0xff00_0000 | (4 << 21) | (1 << 19) | 3, 0x400),
        (0xf700_0000, 0x1234_5678),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = 0x100 + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
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
    for row in 0..2 {
        for (column, intensity) in [0x12, 0x34, 0x56, 0x78].into_iter().enumerate() {
            assert_eq!(
                view.read_u8(fn64_runtime::RdramAddr::from_offset(
                    0x400 + row * 4 + column as u32
                )),
                intensity
            );
        }
    }
    let framebuffer = backend.framebuffer().unwrap();
    assert_eq!(
        &framebuffer.pixels[..16],
        &[
            0x12, 0x12, 0x12, 255, 0x34, 0x34, 0x34, 255, 0x56, 0x56, 0x56, 255, 0x78, 0x78,
            0x78, 255
        ]
    );
    assert!(framebuffer
        .coverage
        .iter()
        .all(|coverage| *coverage == raster::Coverage::FULL));
}


#[test]
fn intensity8_target_import_and_commit_share_logical_rdram_bytes() {
    let mut rdram = vec![0u8; 0x500];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, intensity) in [17, 34, 51, 68].into_iter().enumerate() {
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(0x400 + index as u32),
                intensity,
            );
        }
    }
    let target = gbi::ColorImage {
        format: 2,
        size: gbi::ColorImage::BITS_8,
        width: 4,
        address: 0x400,
    };
    let mut framebuffer = Framebuffer::new(4, 1);
    let mut hidden_bits = RdramHiddenBits::new();
    load_color_image(&rdram, target, &mut framebuffer, &mut hidden_bits);
    assert_eq!(
        framebuffer.pixels,
        [17, 17, 17, 255, 34, 34, 34, 255, 51, 51, 51, 255, 68, 68, 68, 255]
    );

    framebuffer.pixels[0] = 0xa5;
    framebuffer.pixels[4] = 0xb6;
    framebuffer.pixels[8] = 0xc7;
    framebuffer.pixels[12] = 0xd8;
    framebuffer.coverage.fill(raster::Coverage::new(1));
    commit_color_image(&mut rdram, target, &framebuffer, &mut hidden_bits);
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        (0..4)
            .map(|index| view.read_u8(fn64_runtime::RdramAddr::from_offset(0x400 + index)))
            .collect::<Vec<_>>(),
        [0xa5, 0xb6, 0xc7, 0xd8]
    );
    assert!(
        hidden_bits.is_empty(),
        "I8 ignores RDRAM hidden coverage bits"
    );
}


#[test]
fn same_color_image_bytes_reinterpret_between_index8_and_rgba16() {
    const ADDRESS: u32 = 0x400;
    let rgba16 = gbi::ColorImage {
        format: gbi::ColorImage::RGBA_FORMAT,
        size: gbi::ColorImage::BITS_16,
        width: 2,
        address: ADDRESS,
    };
    let index8 = gbi::ColorImage {
        format: gbi::ColorImage::CI_FORMAT,
        size: gbi::ColorImage::BITS_8,
        width: 4,
        address: ADDRESS,
    };
    let mut rdram = vec![0u8; 0x500];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(ADDRESS), 0xf801);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(ADDRESS + 2), 0x07c1);
    }

    let mut framebuffer = Framebuffer::new(2, 1);
    let mut hidden_bits = RdramHiddenBits::new();
    load_color_image(&rdram, rgba16, &mut framebuffer, &mut hidden_bits);
    assert_eq!(&framebuffer.pixels[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);

    load_color_image(&rdram, index8, &mut framebuffer, &mut hidden_bits);
    assert_eq!(
        framebuffer
            .pixels
            .chunks_exact(4)
            .map(|pixel| pixel[0])
            .collect::<Vec<_>>(),
        [0xf8, 0x01, 0x07, 0xc1]
    );

    for (pixel, byte) in framebuffer
        .pixels
        .chunks_exact_mut(4)
        .zip([0x00, 0x3f, 0xff, 0xff])
    {
        pixel[..3].fill(byte);
    }
    commit_color_image(&mut rdram, index8, &framebuffer, &mut hidden_bits);
    load_color_image(&rdram, rgba16, &mut framebuffer, &mut hidden_bits);
    assert_eq!(
        &framebuffer.pixels[..8],
        &[0, 0, 255, 255, 255, 255, 255, 255]
    );
}


#[test]
fn reference_renderer_rejects_invalid_non_rgba_16bit_targets_by_name() {
    let mut rdram = vec![0u8; 0x1000];
    rdram[0x100..0x104].copy_from_slice(&0xff70_0003u32.to_ne_bytes());
    rdram[0x104..0x108].copy_from_slice(&0x400u32.to_ne_bytes());
    rdram[0x108..0x10c].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
    rdram[0x10c..0x110].copy_from_slice(&0u32.to_ne_bytes());
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    let error = backend
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
        .unwrap_err();
    assert!(error.to_string().contains("format=3 size=2"));
    assert!(error.to_string().contains("requires 8-bit intensity"));
}
