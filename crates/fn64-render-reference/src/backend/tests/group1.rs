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
fn native_programmed_span_excludes_reference_filter_halo() {
    let vi = live_presentation(0x002, 0x100, 4, 1, 1);
    let programmed = fn64_render::programmed_vi_source_footprint(vi)
        .unwrap()
        .unwrap();
    let reference = reference_vi_source_geometry(vi).unwrap().unwrap();
    assert_eq!(programmed.rows, 2);
    assert_eq!(reference.rows, 3);
    assert_eq!(programmed.origin, reference.origin);
    assert_eq!(programmed.stride_pixels, reference.stride_pixels);
}


#[test]
fn reference_backend_create_then_present_succeeds_with_no_geometry() {
    let mut backend = ReferenceBackend::new();
    assert_eq!(
        backend.task_chunking(),
        fn64_render::RenderTaskChunking::Resumable
    );
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    present_resident(&mut backend, ViPresentation::default()).unwrap();
    assert!(!backend
        .framebuffer()
        .unwrap()
        .has_non_uniform_content(0, 0, 0, 255));
}


#[test]
fn reference_renderer_tv_authority_tracks_create_and_survives_resize() {
    let mut backend = ReferenceBackend::new();
    assert_eq!(
        backend.release_environment(),
        fn64_render::RenderBackendEvidence::Unidentified
    );

    backend
        .create(&RenderConfig::for_tv(8, 8, fn64_runtime::TvType::Pal))
        .unwrap();
    assert_eq!(
        backend.release_environment(),
        fn64_render::RenderBackendEvidence::Reference {
            tv_type: fn64_runtime::TvType::Pal,
        }
    );
    backend.resize(16, 12);
    assert_eq!(
        backend.release_environment().tv_type(),
        Some(fn64_runtime::TvType::Pal)
    );

    backend
        .create(&RenderConfig::for_tv(4, 4, fn64_runtime::TvType::Mpal))
        .unwrap();
    assert_eq!(
        backend.release_environment().tv_type(),
        Some(fn64_runtime::TvType::Mpal)
    );
}


#[test]
fn reference_backend_chunks_at_committed_operations_and_consumes_tokens_once() {
    const DL: usize = 0x100;
    const TARGET: u32 = 0x400;
    let make_rdram = || {
        let mut rdram = vec![0u8; 0x1000];
        let commands: [(u32, u32); 8] = [
            (0xef00_0000 | (3 << 20), 0),
            (0xff10_0003, TARGET),
            (0xf700_0000, 0xf801_f801),
            (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
            (0xe900_0000, 0),
            (0xf700_0000, 0x003f_003f),
            (0xf600_0000 | ((2 * 4) << 12), 4 << 12),
            (0xdf00_0000, 0),
        ];
        for (index, (w0, w1)) in commands.into_iter().enumerate() {
            let offset = DL + index * 8;
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        }
        rdram
    };
    let task = OsTask {
        task_type: fn64_render::M_GFXTASK,
        data_ptr: DL as u32,
        ..OsTask::default()
    };
    let make_backend = || {
        let mut backend = ReferenceBackend::new()
            .with_f3dex2()
            .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
        backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
        backend
    };

    let mut chunked = make_backend();
    let mut chunked_rdram = make_rdram();
    let mut chunked_rsp = fn64_runtime::RspMemory::new();
    let first = match chunked
        .process_task_chunk(
            &mut chunked_rdram,
            &mut chunked_rsp,
            &task,
            0,
            fn64_render::RenderTaskStep::Start,
        )
        .unwrap()
    {
        fn64_render::RenderTaskChunkStatus::Continue(token) => token,
        status => panic!("SETCIMG boundary did not retain a continuation: {status:?}"),
    };
    assert_eq!(
        chunked.last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::NotReached
    );
    let second = match chunked
        .process_task_chunk(
            &mut chunked_rdram,
            &mut chunked_rsp,
            &task,
            0,
            fn64_render::RenderTaskStep::Resume(first),
        )
        .unwrap()
    {
        fn64_render::RenderTaskChunkStatus::Continue(token) => token,
        status => panic!("first fill boundary did not retain a continuation: {status:?}"),
    };
    assert_ne!(first, second);
    let red_boundary = chunked_rdram.clone();
    let stale = chunked
        .process_task_chunk(
            &mut chunked_rdram,
            &mut chunked_rsp,
            &task,
            0,
            fn64_render::RenderTaskStep::Resume(first),
        )
        .unwrap_err();
    assert!(stale.to_string().contains("does not own retained token"));
    assert_eq!(chunked_rdram, red_boundary, "stale token replayed a fill");
    let overlapping_start = chunked
        .process_task_chunk(
            &mut chunked_rdram,
            &mut chunked_rsp,
            &task,
            0,
            fn64_render::RenderTaskStep::Start,
        )
        .unwrap_err();
    assert!(overlapping_start
        .to_string()
        .contains("cannot start a new task"));

    let third = match chunked
        .process_task_chunk(
            &mut chunked_rdram,
            &mut chunked_rsp,
            &task,
            0,
            fn64_render::RenderTaskStep::Resume(second),
        )
        .unwrap()
    {
        fn64_render::RenderTaskChunkStatus::Continue(token) => token,
        status => panic!("FullSync boundary did not retain a continuation: {status:?}"),
    };
    assert_eq!(
        chunked.last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Reached,
        "FullSync evidence must be published at its committed boundary"
    );
    assert_eq!(
        chunked
            .process_task_chunk(
                &mut chunked_rdram,
                &mut chunked_rsp,
                &task,
                0,
                fn64_render::RenderTaskStep::Resume(third),
            )
            .unwrap(),
        fn64_render::RenderTaskChunkStatus::Complete
    );
    assert_eq!(
        chunked.last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Reached
    );
    let completed_rdram = chunked_rdram.clone();
    let consumed = chunked
        .process_task_chunk(
            &mut chunked_rdram,
            &mut chunked_rsp,
            &task,
            0,
            fn64_render::RenderTaskStep::Resume(third),
        )
        .unwrap_err();
    assert!(consumed
        .to_string()
        .contains("stale or was already consumed"));
    assert_eq!(chunked_rdram, completed_rdram);

    let mut atomic = make_backend();
    let mut atomic_rdram = make_rdram();
    atomic
        .process_task(
            &mut atomic_rdram,
            &mut fn64_runtime::RspMemory::new(),
            &task,
            0,
        )
        .unwrap();
    assert_eq!(chunked_rdram, atomic_rdram);
    assert_eq!(
        chunked.framebuffer().unwrap().pixels,
        atomic.framebuffer().unwrap().pixels
    );
}


#[test]
fn reference_backend_noise_seed_is_selectable_and_survives_resize() {
    let mut backend = ReferenceBackend::new().with_noise_seed(7);
    backend.create(&RenderConfig::ntsc(4, 4)).unwrap();
    assert_eq!(backend.fb.as_ref().unwrap().noise_position(), (7, 0));

    let vertex = |x, y| gbi::Vertex {
        x,
        y,
        r: 255,
        g: 255,
        b: 255,
        a: 255,
        w: 1.0,
        ..gbi::Vertex::default()
    };
    backend.fb.as_mut().unwrap().draw_triangle(&gbi::Triangle {
        v: [vertex(0.0, 0.0), vertex(4.0, 0.0), vertex(0.0, 4.0)],
        ..gbi::Triangle::default()
    });
    let position = backend.fb.as_ref().unwrap().noise_position();
    assert!(position.1 > 0);

    backend.resize(8, 8);
    assert_eq!(backend.fb.as_ref().unwrap().noise_position(), position);
}


#[test]
fn reference_backend_blanks_scanout_without_destroying_the_rdp_image() {
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
    backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[9, 8, 7, 255]);

    present_resident(&mut backend, ViPresentation::default()).unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[0..4],
        &[9, 8, 7, 255]
    );

    present_resident(
        &mut backend,
        ViPresentation {
            blanked: true,
            ..ViPresentation::default()
        },
    )
    .unwrap();
    assert!(backend
        .presented_framebuffer()
        .unwrap()
        .pixels
        .chunks_exact(4)
        .all(|pixel| pixel == [0, 0, 0, 255]));
    assert_eq!(
        &backend.framebuffer().unwrap().pixels[0..4],
        &[9, 8, 7, 255]
    );

    present_resident(&mut backend, ViPresentation::default()).unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[0..4],
        &[9, 8, 7, 255]
    );
}


#[test]
fn reference_backend_executes_public_fade_and_repeat_line_scanout() {
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
    backend.fb.as_mut().unwrap().pixels.copy_from_slice(&[
        10, 20, 30, 255, 40, 50, 60, 255, 110, 120, 130, 255, 140, 150, 160, 255,
    ]);

    present_resident(
        &mut backend,
        ViPresentation {
            fade: Some(0x03ff),
            ..ViPresentation::default()
        },
    )
    .unwrap();
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        [110, 120, 130, 255, 140, 150, 160, 255, 110, 120, 130, 255, 140, 150, 160, 255,]
    );

    present_resident(
        &mut backend,
        ViPresentation {
            repeat_line: true,
            ..ViPresentation::default()
        },
    )
    .unwrap();
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        [10, 20, 30, 255, 40, 50, 60, 255, 10, 20, 30, 255, 40, 50, 60, 255,]
    );
}


#[test]
fn reference_backend_executes_vi_dither_divot_and_gamma_filters() {
    let rgba16 = fn64_render::ViFilterControl {
        pixel_type: ViPixelType::Rgba16,
        dither_filter: true,
        ..Default::default()
    };
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(3, 3)).unwrap();
    let fb = backend.fb.as_mut().unwrap();
    for pixel in fb.pixels.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[88, 88, 88, 255]);
    }
    fb.pixels[4 * 4..4 * 4 + 4].copy_from_slice(&[80, 80, 80, 255]);
    present_resident(
        &mut backend,
        ViPresentation {
            scanout: fn64_render::ViScanoutState::BackendOnly(rgba16),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[4 * 4..4 * 4 + 4],
        &[88, 88, 88, 255]
    );

    let fb = backend.fb.as_mut().unwrap();
    fb.pixels[0..12].copy_from_slice(&[10, 10, 10, 255, 200, 200, 200, 255, 20, 20, 20, 255]);
    fb.coverage[1] = raster::Coverage::new(4);
    present_resident(
        &mut backend,
        ViPresentation {
            scanout: fn64_render::ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                divot: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[4..8],
        &[20, 20, 20, 255]
    );

    backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[64, 0, 255, 255]);
    present_resident(
        &mut backend,
        ViPresentation {
            scanout: fn64_render::ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
                pixel_type: ViPixelType::Rgba32,
                gamma: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[0..4],
        &[127, 0, 255, 255]
    );
}


#[test]
fn reference_backend_gamma_dither_is_seeded_and_frame_varying() {
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(1, 1)).unwrap();
    backend.fb.as_mut().unwrap().pixels[0..4].copy_from_slice(&[101, 101, 101, 255]);
    let presentation = |noise_seed| ViPresentation {
        scanout: fn64_render::ViScanoutState::BackendOnly(fn64_render::ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            gamma_dither: true,
            ..Default::default()
        }),
        noise_seed,
        ..Default::default()
    };
    present_resident(&mut backend, presentation(0)).unwrap();
    let first = backend.presented_framebuffer().unwrap().pixels[0..3].to_vec();
    present_resident(&mut backend, presentation(0)).unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[0..3],
        first
    );

    let variants = (0..64)
        .map(|seed| {
            present_resident(&mut backend, presentation(seed)).unwrap();
            backend.presented_framebuffer().unwrap().pixels[0]
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(variants, [100, 102].into_iter().collect());
}


#[test]
fn reference_vi_reads_rgba16_from_live_origin_and_effective_padded_stride() {
    const ORIGIN: u32 = 0x120;
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, pixel) in [
            0xf801u16, 0x07c1, 0xf83f, 0xffc1, 0x003f, 0xffff, 0x0001, 0x07ff,
        ]
        .into_iter()
        .enumerate()
        {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(ORIGIN + index as u32 * 2),
                pixel,
            );
        }
    }
    assert_eq!(
        &rdram[ORIGIN as usize..ORIGIN as usize + 16],
        &[
            0xc1, 0x07, 0x01, 0xf8, 0xc1, 0xff, 0x3f, 0xf8, 0xff, 0xff, 0x3f, 0x00, 0xff, 0x07,
            0x01, 0x00
        ]
    );

    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    backend.fb.as_mut().unwrap().clear(9, 8, 7, 255);
    let vi = live_presentation(0x302, ORIGIN, 0xf000_0004, 2, 2);
    present_physical(&mut backend, &rdram, vi).unwrap();
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,]
    );
    assert!(backend
        .framebuffer()
        .unwrap()
        .pixels
        .chunks_exact(4)
        .all(|pixel| pixel == [9, 8, 7, 255]));

    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_u16(fn64_runtime::RdramAddr::from_offset(ORIGIN), 0x07ff);
    present_physical(&mut backend, &rdram, vi).unwrap();
    assert_eq!(
        &backend.presented_framebuffer().unwrap().pixels[..4],
        &[0, 255, 255, 255],
        "a repeated field retained stale task-time or prior-present bytes"
    );
}


#[test]
fn reference_vi_reads_unaligned_rgba32_rows_from_odd_live_stride() {
    const ORIGIN: u32 = 0x181;
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    let logical = [
        0x10, 0x20, 0x30, 0xe4, 0x40, 0x50, 0x60, 0x63, 0xd1, 0xd2, 0xd3, 0xff, 0x70, 0x80,
        0x90, 0xa2, 0xa0, 0xb0, 0xc0, 0xff, 0xe1, 0xe2, 0xe3, 0x00,
    ];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram)
        .write_logical_bytes(fn64_runtime::RdramAddr::from_offset(ORIGIN), &logical);

    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(3, 2)).unwrap();
    present_physical(
        &mut backend,
        &rdram,
        live_presentation(0x303, ORIGIN, 3, 2, 2),
    )
    .unwrap();
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        [
            0x10, 0x20, 0x30, 33, 0x40, 0x50, 0x60, 24, 0x70, 0x80, 0x90, 16, 0xa0, 0xb0, 0xc0,
            255,
        ]
    );
}


#[test]
fn reference_vi_uses_each_field_exact_live_origin_without_extra_bias() {
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(0x220), 0xf801);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(0x222), 0x07c1);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(0x280), 0x003f);
        view.write_u16(fn64_runtime::RdramAddr::from_offset(0x282), 0xffff);
    }

    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 1)).unwrap();
    let odd = live_presentation(0x342, 0x280, 2, 2, 1);
    let mut odd_words = odd.scanout.registers().unwrap().words();
    odd_words[4] = 1;
    let odd = ViPresentation {
        scanout: fn64_render::ViScanoutState::Registers(
            fn64_render::ViScanoutRegisters::from_words(odd_words),
        ),
        ..odd
    };
    present_physical(&mut backend, &rdram, odd).unwrap();
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        [0, 0, 255, 255, 255, 255, 255, 255]
    );
    present_physical(
        &mut backend,
        &rdram,
        live_presentation(0x342, 0x220, 2, 2, 1),
    )
    .unwrap();
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        [255, 0, 0, 255, 0, 255, 0, 255]
    );
}


#[test]
fn reference_vi_bounds_fail_transactionally_and_exact_edge_succeeds() {
    let mut rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();
    present_resident(&mut backend, ViPresentation::default()).unwrap();
    let before = backend.presented_framebuffer().unwrap().clone();

    let error = present_physical(
        &mut backend,
        &rdram,
        live_presentation(0x302, 0x7f_fff8, 4, 2, 2),
    )
    .unwrap_err();
    assert!(matches!(error, RenderError::InvalidViSourceBounds { .. }));
    assert_eq!(
        backend.presented_framebuffer().unwrap().pixels,
        before.pixels
    );

    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(0x7f_fff8),
        &[1, 2, 3, 0xff, 4, 5, 6, 0xff],
    );
    present_physical(
        &mut backend,
        &rdram,
        live_presentation(0x303, 0x7f_fff8, 2, 2, 1),
    )
    .unwrap();
    let one_row = backend.presented_framebuffer().unwrap().pixels.clone();
    let error = present_physical(
        &mut backend,
        &rdram,
        live_presentation(0x303, 0x7f_fff8, 2, 2, 2),
    )
    .unwrap_err();
    assert!(matches!(error, RenderError::InvalidViSourceBounds { .. }));
    assert_eq!(backend.presented_framebuffer().unwrap().pixels, one_row);
}


#[test]
fn reference_vi_blank_and_inactive_paths_do_not_fetch_live_source() {
    let rdram = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();

    let mut inactive_words = [0u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
    inactive_words[0] = 0x302;
    inactive_words[1] = 0x00ff_ffff;
    let inactive = ViPresentation {
        scanout: fn64_render::ViScanoutState::Registers(
            fn64_render::ViScanoutRegisters::from_words(inactive_words),
        ),
        ..Default::default()
    };
    present_physical(&mut backend, &rdram, inactive).unwrap();
    assert_eq!(backend.presented_framebuffer().unwrap().width, 0);
    assert_eq!(backend.presented_framebuffer().unwrap().height, 0);

    let blanked = ViPresentation {
        blanked: true,
        ..live_presentation(0x302, 0x00ff_ffff, 2, 2, 2)
    };
    present_physical(&mut backend, &rdram, blanked).unwrap();
    assert!(backend
        .presented_framebuffer()
        .unwrap()
        .pixels
        .chunks_exact(4)
        .all(|pixel| pixel == [0, 0, 0, 255]));

    let status_blank = live_presentation(0x300, 0x00ff_ffff, 2, 2, 2);
    present_physical(&mut backend, &rdram, status_blank).unwrap();
    let reserved = ViPresentation {
        blanked: true,
        ..live_presentation(0x301, 0x00ff_ffff, 2, 2, 2)
    };
    let error = present_physical(&mut backend, &rdram, reserved).unwrap_err();
    assert!(error.to_string().contains("reserved pixel type"));

    let misaligned = live_presentation(0x302, 0x121, 2, 2, 1);
    assert!(matches!(
        present_physical(&mut backend, &rdram, misaligned).unwrap_err(),
        RenderError::InvalidViSourceAlignment { .. }
    ));
}


#[test]
fn reference_backend_rejects_process_task_before_create() {
    let mut backend = ReferenceBackend::new();
    let mut rdram = vec![0u8; 64];
    let err = backend
        .process_task(
            &mut rdram,
            &mut fn64_runtime::RspMemory::new(),
            &OsTask::default(),
            0,
        )
        .unwrap_err();
    assert!(matches!(err, RenderError::NotReady(_)));
}


#[test]
fn reference_backend_lle_preflight_is_transactional() {
    const DL: usize = 0x1000;
    const TEXT: usize = 0x2000;
    const DATA: usize = 0x3200;
    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    let mut rdram = vec![0u8; 0x4000];
    fn64_runtime::RdramViewMut::from_storage(&mut rdram).write_logical_bytes(
        fn64_runtime::RdramAddr::from_offset(TEXT as u32),
        &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE],
    );
    let write_word = |rdram: &mut [u8], offset: usize, word: u32| {
        rdram[offset..offset + 4].copy_from_slice(&word.to_ne_bytes());
    };
    write_word(&mut rdram, DL, 0xe100_0000);
    write_word(&mut rdram, DL + 4, DATA as u32);
    write_word(&mut rdram, DL + 8, 0xdd00_0007);
    write_word(&mut rdram, DL + 12, TEXT as u32);
    write_word(&mut rdram, DL + 16, 0xd500_0000);
    write_word(&mut rdram, DL + 20, 0);

    let mut rsp_memory = fn64_runtime::RspMemory::new();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &[0x11; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        )
        .unwrap();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Dmem, 0x40),
            b"task-entry",
        )
        .unwrap();
    let rdram_before = rdram.clone();
    let rsp_before = rsp_memory.clone();
    let status = backend
        .process_task(
            &mut rdram,
            &mut rsp_memory,
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    assert_eq!(
        status,
        FrameStatus::NeedsLle {
            ucode_sha256: gbi::UcodeDigest::from_text(
                &[0x5a; fn64_runtime::RSP_MEMORY_BANK_SIZE]
            )
            .as_bytes(),
        }
    );
    assert_eq!(rdram, rdram_before);
    assert_eq!(rsp_memory, rsp_before);
}


#[test]
fn reference_backend_selects_l3dex_wire_family_from_admitted_imem_digest() {
    const DL: usize = 0x1000;
    let text = [0x4c; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let mut backend =
        ReferenceBackend::new().with_geometry_ucode_text(GeometryWireFamily::L3dex, &text);
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    assert_eq!(backend.supported_ucodes(), &[UcodeId::L3dex]);

    let mut rdram = vec![0u8; 0x2000];
    rdram[DL..DL + 4].copy_from_slice(&0xb800_0000u32.to_ne_bytes());
    rdram[DL + 4..DL + 8].copy_from_slice(&0u32.to_ne_bytes());
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &text,
        )
        .unwrap();

    assert_eq!(
        backend
            .process_task(
                &mut rdram,
                &mut rsp_memory,
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
fn reference_backend_reports_only_admitted_polygon_wire_families() {
    let fast3d = [0x31; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dex = [0x32; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dlx = [0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dlx_rej = [0x34; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dex2 = [0x35; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dex2_non = [0x36; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dex2_rej = [0x37; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let f3dlx2_rej = [0x38; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let backend = ReferenceBackend::new()
        .with_geometry_ucode_text(GeometryWireFamily::F3dlx2Rej, &f3dlx2_rej)
        .with_geometry_ucode_text(GeometryWireFamily::F3dex2Rej, &f3dex2_rej)
        .with_geometry_ucode_text(GeometryWireFamily::F3dex2NoN, &f3dex2_non)
        .with_geometry_ucode_text(GeometryWireFamily::F3dex2, &f3dex2)
        .with_geometry_ucode_text(GeometryWireFamily::F3dlxRej, &f3dlx_rej)
        .with_geometry_ucode_text(GeometryWireFamily::F3dlx, &f3dlx)
        .with_geometry_ucode_text(GeometryWireFamily::F3dex, &f3dex)
        .with_geometry_ucode_text(GeometryWireFamily::Fast3d, &fast3d);
    assert_eq!(
        backend.supported_ucodes(),
        &[
            UcodeId::Fast3d,
            UcodeId::F3dex,
            UcodeId::F3dlx,
            UcodeId::F3dlxRej,
            UcodeId::F3dex2,
            UcodeId::F3dex2NoN,
            UcodeId::F3dex2Rej,
            UcodeId::F3dlx2Rej
        ]
    );
}


#[test]
fn reference_backend_identifies_only_exact_admitted_imem_images() {
    let geometry = [0x71; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let sprite = [0x72; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let unadmitted = [0x73; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let backend = ReferenceBackend::new()
        .with_geometry_ucode_text(GeometryWireFamily::L3dex2, &geometry)
        .with_s2dex_ucode_text_for(S2dexWireFamily::S2dex, &sprite);

    assert_eq!(backend.identify_microcode(&geometry), Some(UcodeId::L3dex2));
    assert_eq!(backend.identify_microcode(&sprite), Some(UcodeId::S2dex));
    assert_eq!(backend.identify_microcode(&unadmitted), None);
    assert_eq!(backend.supported_ucodes(), &[UcodeId::L3dex2]);
}


#[test]
fn reference_pair_recognition_is_separate_from_text_hle_admission() {
    let text = [0x71; fn64_runtime::RSP_MEMORY_BANK_SIZE];
    let data = [0x10, 0x20, 0x30];
    let identity = MicrocodeDataImageIdentity {
        bytes: data.len() as u32,
        sha256: sha2::Sha256::digest(data).into(),
    };
    let text_only =
        ReferenceBackend::new().with_geometry_ucode_text(GeometryWireFamily::L3dex2, &text);
    assert_eq!(text_only.identify_microcode_pair(&text, identity), None);
    let paired = text_only.with_microcode_pair(UcodeId::L3dex2, &text, &data);
    assert_eq!(
        paired.identify_microcode_pair(&text, identity),
        Some(UcodeId::L3dex2)
    );
}


#[test]
fn reference_backend_requires_exact_task_entry_admission() {
    const DL: usize = 0x100;
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    let mut rdram = vec![0u8; 0x200];
    rdram[DL..DL + 4].copy_from_slice(&0xdf00_0000u32.to_ne_bytes());
    let mut rsp_memory = fn64_runtime::RspMemory::new();
    rsp_memory
        .write_bytes(
            fn64_runtime::RspMemAddr::from_parts(fn64_runtime::RspMemoryBank::Imem, 0),
            &[0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE],
        )
        .unwrap();
    let rdram_before = rdram.clone();
    let rsp_before = rsp_memory.clone();

    let status = backend
        .process_task(
            &mut rdram,
            &mut rsp_memory,
            &OsTask {
                task_type: fn64_render::M_GFXTASK,
                data_ptr: DL as u32,
                ..OsTask::default()
            },
            0,
        )
        .unwrap();

    assert_eq!(
        status,
        FrameStatus::NeedsLle {
            ucode_sha256: gbi::UcodeDigest::from_text(
                &[0x33; fn64_runtime::RSP_MEMORY_BANK_SIZE]
            )
            .as_bytes(),
        }
    );
    assert_eq!(rdram, rdram_before);
    assert_eq!(rsp_memory, rsp_before);
}


#[test]
fn reference_raw_dpc_batch_commits_mixed_sources_in_one_boundary() {
    let submissions = vec![
        raw_submission(fn64_render::RawDpcSource::Rdram, 0x100, 0xe6),
        raw_submission(fn64_render::RawDpcSource::XbusDmem, 0x20, 0xe9),
    ];
    let identities = submissions
        .iter()
        .map(fn64_render::OwnedRawDpcSubmission::identity)
        .collect::<Vec<_>>();
    let mut rdram = vec![0x5a; 0x400];
    let before = rdram.clone();
    let batch = fn64_render::RawDpcBatch::new(submissions)
        .unwrap()
        .preflight(rdram.len())
        .unwrap();
    let mut backend = ReferenceBackend::new();
    backend.create(&RenderConfig::ntsc(2, 2)).unwrap();

    let outcome = backend.process_raw_dpc_batch(&mut rdram, batch, 0).unwrap();

    assert_eq!(
        backend.raw_dpc_batch_capability(),
        fn64_render::RawDpcBatchCapability::DiagnosticOnly
    );
    assert_eq!(outcome.identities.as_ref(), identities);
    assert_eq!(outcome.full_sync, fn64_render::DpFullSyncStatus::Reached);
    assert_eq!(outcome.stream_groups.len(), 2);
    assert_eq!(backend.last_dp_full_sync(), outcome.full_sync);
    assert_eq!(rdram, before, "private command staging leaked into RDRAM");
}


#[test]
fn reference_raw_dpc_batch_not_ready_rejects_without_mutation() {
    let mut rdram = vec![0x5a; 0x400];
    let before = rdram.clone();
    let batch = fn64_render::RawDpcBatch::new(vec![raw_submission(
        fn64_render::RawDpcSource::Rdram,
        0x100,
        0xe9,
    )])
    .unwrap()
    .preflight(rdram.len())
    .unwrap();
    let mut backend = ReferenceBackend::new();

    let error = backend
        .process_raw_dpc_batch(&mut rdram, batch, 0)
        .unwrap_err();

    assert!(matches!(error, RenderError::NotReady(_)));
    assert_eq!(rdram, before);
    assert_eq!(
        backend.last_dp_full_sync(),
        fn64_render::DpFullSyncStatus::Unidentified
    );
    assert!(backend.framebuffer().is_none());
}


#[test]
fn raw_depth_image_fill_clears_persistent_depth_across_color_switch() {
    const START: usize = 0x100;
    const Z_IMAGE: u32 = 0x400;
    const COLOR_IMAGE: u32 = 0x600;
    let commands: [(u32, u32); 7] = [
        (0xfe00_0000, Z_IMAGE),
        (0xff10_0003, Z_IMAGE),
        (0xef00_0000 | (3 << 20), 0),
        (0xf700_0000, 0xfffc_fffc),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xff10_0003, COLOR_IMAGE),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0u8; 0x1000];
    for (index, (w0, w1)) in commands.into_iter().enumerate() {
        let offset = START + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
    }
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();
    backend.fb.as_mut().unwrap().depth.fill(1.0);

    backend
        .process_rdp_commands(
            &mut rdram,
            START as u32,
            (START + commands.len() * 8) as u32,
            0,
        )
        .unwrap();

    assert_eq!(
        backend.depth_image,
        Some(gbi::DepthImage { address: Z_IMAGE })
    );
    assert!(backend
        .fb
        .as_ref()
        .unwrap()
        .depth
        .iter()
        .all(|&value| value == 0x3ffff as f32));
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    for pixel in 0..8 {
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2)),
            0xfffc
        );
    }
}


#[test]
fn raw_depth_fill_halfwords_replicate_lsbs_into_hidden_delta_bits() {
    const START: usize = 0x100;
    const Z_IMAGE: u32 = 0x400;
    const COLOR_IMAGE: u32 = 0x600;
    let commands: [(u32, u32); 7] = [
        (0xfe00_0000, Z_IMAGE),
        (0xff10_0003, Z_IMAGE),
        (0xef00_0000 | (3 << 20), 0),
        // Both halves retain maximum encoded Z. Their low pairs are 01
        // and 10; MI fill replication supplies hidden pairs 11 and 00,
        // yielding complete stored DeltaZ exponents 7 and 8.
        (0xf700_0000, 0xfffd_fffe),
        (0xf600_0000 | ((3 * 4) << 12) | 4, 0),
        (0xff10_0003, COLOR_IMAGE),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0u8; 0x1000];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = START + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(4, 2)).unwrap();

    backend
        .process_rdp_commands(
            &mut rdram,
            START as u32,
            (START + commands.len() * 8) as u32,
            0,
        )
        .unwrap();

    let view = fn64_runtime::RdramView::from_storage(&rdram);
    let framebuffer = backend.fb.as_ref().unwrap();
    for pixel in 0..8u32 {
        let even = pixel.is_multiple_of(2);
        let address = Z_IMAGE + pixel * 2;
        let visible = if even { 0xfffd } else { 0xfffe };
        let hidden = if even { 3 } else { 0 };
        let delta = if even { 7 } else { 8 };
        assert_eq!(
            view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
            visible,
            "visible fill halfword at pixel {pixel}"
        );
        assert_eq!(
            backend.rdram_hidden_bits.get(&address),
            Some(RdramHiddenSample {
                visible,
                bits: hidden,
            }),
            "hidden fill pair at pixel {pixel}"
        );
        assert_eq!(
            depth::unpack(framebuffer.encoded_depth[pixel as usize].unwrap()),
            (0x3ffff, delta),
            "reloaded depth sample at pixel {pixel}"
        );
    }
}


#[test]
fn raw_edge_triangle_rasterizes_into_commanded_color_image() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    let mut rdram = vec![0u8; 0x1000];
    let mut offset = START;
    {
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        command(0xff10_0007, TARGET); // RGBA16 width 8
        command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        command(0x0800_0000 | yl, (ym << 16) | yh);
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
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
        0xf801,
        "raw edge triangle must cover its interior pixel in primitive red"
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(TARGET)),
        0,
        "raw edge triangle must not paint outside its edges"
    );
    let partial_pixel = 4 * 8 + 3;
    assert_eq!(
        backend.fb.as_ref().unwrap().coverage[partial_pixel as usize],
        raster::Coverage::new(6),
        "the raw edge must retain six of the public checkerboard samples"
    );
    let partial_address = TARGET + partial_pixel * 2;
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(partial_address)),
        0xf801
    );
    assert_eq!(
        backend
            .rdram_hidden_bits
            .get(&partial_address)
            .map(|sample| sample.bits),
        Some(1),
        "coverage six stores code five as visible bit 1 plus hidden bits 01"
    );
}


#[test]
fn raw_z_triangles_use_near_zero_depth_regardless_of_submission_order() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    const Z_IMAGE: u32 = 0x600;
    let mut rdram = vec![0u8; 0x1000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for pixel in 0..64 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                0xfffc,
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
        command(0xfe00_0000, Z_IMAGE);
        command(0xff10_0007, TARGET); // RGBA16 width 8
        command(0xef00_00f0, 0x30); // dither off | Z_CMP | Z_UPD
        command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        command(0x0900_0000 | yl, (ym << 16) | yh);
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        command(2 << 16, 0); // near plane is Z=0
        command(0, 0);
        command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
        command(0x0900_0000 | yl, (ym << 16) | yh);
        command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
        command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
        command(1 << 16, 0);
        command(4 << 16, 0); // submitted later, but farther
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
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(
            TARGET + (4 * 8 + 2) * 2
        )),
        0x003f,
        "near blue raw triangle must reject the later far red fragment"
    );
}


#[test]
fn raw_depth_update_persists_visible_and_hidden_bits_across_image_switches() {
    const START: usize = 0x100;
    const Z_IMAGE_A: u32 = 0x1000;
    const Z_IMAGE_B: u32 = 0x1200;
    const COLOR_IMAGE: u32 = 0x1400;
    let mut rdram = vec![0u8; 0x2000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for pixel in 0..64 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(Z_IMAGE_A + pixel * 2),
                0xfffc,
            );
        }
    }

    let mut offset = START;
    let mut command = |w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        offset += 8;
    };
    let yh = 4;
    let ym = 4 * 4;
    let yl = 7 * 4;
    let triangle = 0x0900_0000 | yl;
    let edge_ym_yh = (ym << 16) | yh;
    let major_slope = (5.0f32 / 3.0 * 65536.0).round() as u32;
    let minor_slope = (5.0f32 / 6.0 * 65536.0).round() as u32;

    command(0xfe00_0000, Z_IMAGE_A);
    command(0xff10_0007, COLOR_IMAGE);
    command(0xef00_00f0, 0x30); // dither off | Z_CMP | Z_UPD
    command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
    command(triangle, edge_ym_yh);
    command(1 << 16, major_slope);
    command(1 << 16, minor_slope);
    command(1 << 16, 0);
    command(8 << 16, 0); // working Z = 64
    command(0, 4 << 16); // DeltaZ = |0| + |4|, then *8 = 32
    command(0xfe00_0000, Z_IMAGE_B); // commits A, then loads B
    command(0xfe00_0000, Z_IMAGE_A); // reloads A, including hidden bits
    command(0xef00_00f0, 0x10); // dither off, compare only: must not mutate A
    command(0xfa00_0000, 0xff00_00ff); // opaque red primitive
    command(triangle, edge_ym_yh);
    command(1 << 16, major_slope);
    command(1 << 16, minor_slope);
    command(1 << 16, 0);
    command(16 << 16, 0); // farther working Z = 128, rejected
    command(0, 0);
    command(0xe900_0000, 0);
    let end = offset;

    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let pixel = 4 * 8 + 2;
    let address = Z_IMAGE_A + pixel * 2;
    let expected = depth::pack(64, 32);
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(address)),
        expected.visible
    );
    assert_eq!(
        backend
            .rdram_hidden_bits
            .get(&address)
            .map(|sample| sample.bits),
        Some(expected.hidden)
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(
            COLOR_IMAGE + pixel * 2
        )),
        0x003f,
        "far compare-only red fragment must not replace the persisted near blue sample"
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE_B + pixel * 2)),
        0,
        "switching through a second depth image must not alias its visible samples"
    );
}


#[test]
fn raw_primitive_depth_supplies_z_and_delta_without_triangle_coefficients() {
    const START: usize = 0x100;
    const Z_IMAGE: u32 = 0x1000;
    const COLOR_IMAGE: u32 = 0x1400;
    let mut rdram = vec![0u8; 0x2000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for pixel in 0..64 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                0xfffc,
            );
        }
    }
    let mut offset = START;
    let mut command = |w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        offset += 8;
    };
    let yh = 4;
    let ym = 4 * 4;
    let yl = 7 * 4;
    command(0xfe00_0000, Z_IMAGE);
    command(0xff10_0007, COLOR_IMAGE);
    command(0xee00_0000, (8 << 16) | 32); // primitive Z=8, DeltaZ=32
    command(0xef00_00f0, 0x34); // dither off | G_ZS_PRIM | Z_CMP | Z_UPD
    command(0xfa00_0000, 0x0000_ffff); // opaque blue primitive
    command(0x0800_0000 | yl, (ym << 16) | yh); // no Z coefficient words
    command(1 << 16, (5.0f32 / 3.0 * 65536.0).round() as u32);
    command(1 << 16, (5.0f32 / 6.0 * 65536.0).round() as u32);
    command(1 << 16, 0);
    command(0xe900_0000, 0);
    let end = offset;

    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let pixel = 4 * 8 + 2;
    let depth_address = Z_IMAGE + pixel * 2;
    let expected = depth::pack(8 << 3, 32);
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(depth_address)),
        expected.visible
    );
    assert_eq!(
        backend
            .rdram_hidden_bits
            .get(&depth_address)
            .map(|sample| sample.bits),
        Some(expected.hidden)
    );
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(
            COLOR_IMAGE + pixel * 2
        )),
        0x003f
    );
    assert_eq!(
        backend.primitive_depth,
        Some(gbi::PrimitiveDepth { z: 8, delta_z: 32 })
    );
}


#[test]
fn raw_decal_mode_accepts_correlated_depth_and_rejects_behind_depth() {
    const START: usize = 0x100;
    const Z_IMAGE: u32 = 0x1000;
    const COLOR_IMAGE: u32 = 0x1400;
    let mut rdram = vec![0u8; 0x2000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for pixel in 0..64 {
            view.write_u16(
                fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2),
                0xfffc,
            );
        }
    }
    let mut offset = START;
    let mut command = |w0: u32, w1: u32| {
        rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
        offset += 8;
    };
    let yh = 4;
    let ym = 4 * 4;
    let yl = 7 * 4;
    let triangle = 0x0800_0000 | yl;
    let edge_ym_yh = (ym << 16) | yh;
    let major_slope = (5.0f32 / 3.0 * 65536.0).round() as u32;
    let minor_slope = (5.0f32 / 6.0 * 65536.0).round() as u32;

    command(0xfe00_0000, Z_IMAGE);
    command(0xff10_0007, COLOR_IMAGE);
    command(0xef00_00f0, 0x34); // dither off | G_ZS_PRIM | Z_CMP | Z_UPD | ZMODE_OPA
    command(0xee00_0000, (16 << 16) | 8); // working Z=128, DeltaZ=8
    command(0xfa00_0000, 0x0000_ffff); // blue depth seed
    command(triangle, edge_ym_yh);
    command(1 << 16, major_slope);
    command(1 << 16, minor_slope);
    command(1 << 16, 0);
    command(0xef00_00f0, 0x0c14); // dither off | G_ZS_PRIM | Z_CMP | ZMODE_DEC
    command(0xee00_0000, (17 << 16) | 4); // working Z=136: correlated boundary
    command(0xfa00_0000, 0xff00_00ff); // red decal must pass
    command(triangle, edge_ym_yh);
    command(1 << 16, major_slope);
    command(1 << 16, minor_slope);
    command(1 << 16, 0);
    command(0xee00_0000, (18 << 16) | 4); // working Z=144: clearly behind
    command(0xfa00_0000, 0x00ff_00ff); // green decal must reject
    command(triangle, edge_ym_yh);
    command(1 << 16, major_slope);
    command(1 << 16, minor_slope);
    command(1 << 16, 0);
    command(0xe900_0000, 0);
    let end = offset;

    let mut backend = ReferenceBackend::new().with_f3dex2();
    backend.create(&RenderConfig::ntsc(8, 8)).unwrap();
    backend
        .process_rdp_commands(&mut rdram, START as u32, end as u32, 0)
        .unwrap();

    let pixel = 4 * 8 + 2;
    let view = fn64_runtime::RdramView::from_storage(&rdram);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(
            COLOR_IMAGE + pixel * 2
        )),
        0xf801,
        "correlated red decal must pass while clearly-behind green rejects"
    );
    let seeded = depth::pack(128, 8);
    assert_eq!(
        view.read_u16(fn64_runtime::RdramAddr::from_offset(Z_IMAGE + pixel * 2)),
        seeded.visible,
        "compare-only decals must retain the opaque seed depth"
    );
}


#[test]
fn raw_shade_triangle_rasterizes_component_gradient() {
    const START: usize = 0x100;
    const TARGET: u32 = 0x400;
    let mut rdram = vec![0u8; 0x1000];
    let mut offset = START;
    let major_slope = (5.0f32 / 6.0 * 65536.0).round() as i32;
    let lower_slope = (5.0f32 / 3.0 * 65536.0).round() as i32;
    let drde = (32.0f32 * 5.0 / 6.0 * 65536.0).round() as u32;
    {
        let mut command = |w0: u32, w1: u32| {
            rdram[offset..offset + 4].copy_from_slice(&w0.to_ne_bytes());
            rdram[offset + 4..offset + 8].copy_from_slice(&w1.to_ne_bytes());
            offset += 8;
        };
        command(0xff10_0007, TARGET); // RGBA16 width 8
        let yh = 4;
        let ym = 4 * 4;
        let yl = 7 * 4;
        command(0x0c00_0000 | yl, (ym << 16) | yh);
        command(1 << 16, lower_slope as u32);
        command(1 << 16, major_slope as u32);
        command(1 << 16, 0);
        command(0, 255); // black, opaque base shade
        command(32 << 16, 0); // red increases 32 per X pixel
        command(0, 0);
        command(0, 0);
        command((drde >> 16) << 16, 0);
        command(0, 0);
        command((drde & 0xffff) << 16, 0);
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
    let raw_edge = gbi::RdpEdgeCoefficients {
        left_major: false,
        level: 0,
        tile: 0,
        yl: 7 * 4,
        ym: 4 * 4,
        yh: 4,
        xl: 1 << 16,
        dxldy: lower_slope,
        xh: 1 << 16,
        dxhdy: major_slope,
        xm: 1 << 16,
        dxmdy: 0,
    };
    for x in [2, 3] {
        let (mask, sample) = raster::test_raw_attribute_sample(
            raw_edge,
            gbi::ScissorRect::framebuffer(8, 8),
            x,
            4,
        );
        let Some((sample_index, _, _)) = sample else {
            panic!("raw shade boundary at x={x} must select a covered attribute sample")
        };
        assert_ne!(mask, 0);
        assert_ne!(mask, u8::MAX);
        assert_ne!(mask & (1 << sample_index), 0);
    }
    assert_eq!(pixel(2, 4), 0x2801);
    assert_eq!(pixel(3, 4), 0x4801);
}
