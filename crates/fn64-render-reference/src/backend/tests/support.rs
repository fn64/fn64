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


pub(super) fn run_direct_8bit_copy(
    source_format: u8,
    width: u16,
    height: u16,
    source: &[u8],
    threshold: Option<u8>,
) -> Vec<u8> {
    const DL: usize = 0x100;
    const TEXTURE: u32 = 0x600;
    const TARGET: u32 = 0x800;
    let pixel_count = usize::from(width) * usize::from(height);
    assert_eq!(source.len(), pixel_count);
    let mut rdram = vec![0u8; 0x1000];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for (index, value) in source.iter().copied().enumerate() {
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(TEXTURE + index as u32),
                value,
            );
            view.write_u8(
                fn64_runtime::RdramAddr::from_offset(TARGET + index as u32),
                0xaa,
            );
        }
    }
    let mut commands = Vec::new();
    let alpha_compare = u32::from(threshold.is_some());
    commands.push((0xef00_0000 | (2 << 20), alpha_compare));
    if let Some(threshold) = threshold {
        commands.push((0xf900_0000, u32::from(threshold)));
    }
    let width_field = u32::from(width - 1);
    let format_field = u32::from(source_format) << 21;
    let size_field = u32::from(gbi::ColorImage::BITS_8) << 19;
    commands.push((
        0xff00_0000 | (u32::from(gbi::ColorImage::I_FORMAT) << 21) | size_field | width_field,
        TARGET,
    ));
    commands.push((
        0xfd00_0000 | format_field | size_field | width_field,
        TEXTURE,
    ));
    let line_words = u32::from(width).div_ceil(8);
    let tile_word0 = 0xf500_0000 | format_field | size_field | (line_words << 9);
    commands.push((tile_word0, 7 << 24));
    let lrs = u32::from(width - 1) * 4;
    let lrt = u32::from(height - 1) * 4;
    commands.push((0xf400_0000, (7 << 24) | (lrs << 12) | lrt));
    commands.push((tile_word0, 0x0008_0200));
    commands.push((0xf200_0000, (lrs << 12) | lrt));
    commands.push((0xe400_0000 | (lrs << 12) | lrt, 0));
    commands.push((0, 0x1000_0400));
    commands.push((0xe900_0000, 0));
    commands.push((0xdf00_0000, 0));
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = DL + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }

    let mut backend = ReferenceBackend::new()
        .with_f3dex2()
        .with_f3dex2_ucode_text(&[0; fn64_runtime::RSP_MEMORY_BANK_SIZE]);
    backend
        .create(&RenderConfig::ntsc(u32::from(width), u32::from(height)))
        .unwrap();
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
    (0..pixel_count)
        .map(|index| view.read_u8(fn64_runtime::RdramAddr::from_offset(TARGET + index as u32)))
        .collect()
}


pub(super) fn present_resident(
    backend: &mut ReferenceBackend,
    vi: ViPresentation,
) -> Result<(), RenderError> {
    backend.present(PresentRequest::backend_resident(vi))
}


pub(super) fn live_presentation(
    status: u32,
    origin: u32,
    width: u32,
    output_width: u32,
    output_height: u32,
) -> ViPresentation {
    let mut words = [0u32; fn64_render::ViScanoutRegisters::WORD_COUNT];
    words[0] = status;
    words[1] = origin;
    words[2] = width;
    words[9] = (100 << 16) | (100 + output_width);
    words[10] = (20 << 16) | (20 + output_height * 2);
    words[12] = u32::from(fn64_render::ViScaleAxis::ONE);
    words[13] = u32::from(fn64_render::ViScaleAxis::ONE);
    ViPresentation {
        scanout: fn64_render::ViScanoutState::Registers(
            fn64_render::ViScanoutRegisters::from_words(words),
        ),
        ..ViPresentation::default()
    }
}


pub(super) fn present_physical(
    backend: &mut ReferenceBackend,
    rdram: &[u8],
    vi: ViPresentation,
) -> Result<(), RenderError> {
    backend.present(PresentRequest::live(
        vi,
        fn64_runtime::PhysicalRdramRead::from_storage(rdram),
    ))
}


pub(super) fn raw_submission(
    source: fn64_render::RawDpcSource,
    start: u32,
    opcode: u8,
) -> fn64_render::OwnedRawDpcSubmission {
    let words = vec![u32::from(opcode) << 24, 0];
    match source {
        fn64_render::RawDpcSource::Rdram => {
            fn64_render::OwnedRawDpcSubmission::from_rdram_words(start, start + 8, words)
                .unwrap()
        }
        fn64_render::RawDpcSource::XbusDmem => {
            fn64_render::OwnedRawDpcSubmission::from_xbus_payload(
                start,
                start + 8,
                words.into_iter().flat_map(u32::to_be_bytes).collect(),
            )
            .unwrap()
        }
    }
}
