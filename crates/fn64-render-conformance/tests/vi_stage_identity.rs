//! Synthetic evidence that the wgpu source-field and filtered-field receipts
//! name different VI stages.
//!
//! The fixture uses only the public VI register image and RGBA5551 storage
//! contract. Its filter arithmetic is independently hand-derived from
//! US 5,699,079's signed-neighbor restoration rule, already cited by
//! `docs/VI-FILTERS.md`. It does not choose which receipt an interactive host
//! should consume and makes no physical-console, analog-video, or full-ROM
//! fidelity claim.

#![cfg(feature = "wgpu-runner")]

use fn64_render::{
    PresentRequest, PresentedSourceFieldAvailability, RenderBackend, ViPresentation,
    ViScanoutRegisters, ViScanoutState,
};
use fn64_render_wgpu::WgpuBackend;
use fn64_runtime::{PhysicalRdramRead, RdramAddr, RdramViewMut};

const ORIGIN: u32 = 0x400;
const WIDTH: u32 = 3;
const OUTPUT_HEIGHT: u32 = 3;
const SOURCE_ROWS: u32 = 4;

const fn pack_rgba5551(red: u8, green: u8, blue: u8) -> u16 {
    ((red as u16) << 11) | ((green as u16) << 6) | ((blue as u16) << 1) | 1
}

const fn expand_five(value: u8) -> u8 {
    (value << 3) | (value >> 2)
}

fn presentation() -> ViPresentation {
    let mut words = [0_u32; ViScanoutRegisters::WORD_COUNT];
    // Public VI STATUS: RGBA16, AA mode 2 (resample only), pixel advance 3,
    // and dither restoration. This is the complete filter-relevant word, not
    // a backend configuration knob.
    words[0] = 0x0001_3202;
    words[1] = ORIGIN;
    words[2] = WIDTH;
    words[9] = WIDTH;
    words[10] = OUTPUT_HEIGHT * 2;
    words[12] = 0x400;
    words[13] = 0x400;
    ViPresentation {
        blanked: false,
        fade: None,
        repeat_line: false,
        scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
        noise_seed: 0,
    }
}

fn fixture_rdram() -> Vec<u8> {
    let mut rdram = vec![0_u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
    // Center red: four greater and four lesser neighbors cancel, producing
    // 8 << 3 = 64 instead of plain RGB5 expansion 66. Center blue: all eight
    // neighbors are lesser, producing (5 << 3) - 8 = 32 instead of 41.
    let red = [[4_u8, 9, 4], [9, 8, 9], [4, 9, 4]];
    let blue = [[2_u8, 2, 2], [2, 5, 2], [2, 2, 2]];
    let mut view = RdramViewMut::from_storage(&mut rdram);
    for row in 0..SOURCE_ROWS {
        for column in 0..WIDTH {
            let (red, blue) = if row < OUTPUT_HEIGHT {
                (
                    red[row as usize][column as usize],
                    blue[row as usize][column as usize],
                )
            } else {
                (4, 2)
            };
            let offset = ORIGIN + (row * WIDTH + column) * 2;
            view.write_u16(RdramAddr::from_offset(offset), pack_rgba5551(red, 0, blue));
        }
    }
    rdram
}

#[test]
fn source_and_post_vi_receipts_keep_exact_distinct_stage_identities() {
    let rdram = fixture_rdram();
    let vi = presentation();

    let (mut source_backend, _source_session) = WgpuBackend::try_new().unwrap();
    source_backend.enable_presented_source_field_delivery();
    source_backend
        .present(PresentRequest::live(
            vi,
            PhysicalRdramRead::from_storage(&rdram),
        ))
        .unwrap();
    let source = match source_backend.take_presented_source_field() {
        PresentedSourceFieldAvailability::Ready(source) => source,
        PresentedSourceFieldAvailability::Unsupported => {
            panic!("source-field mode must return its pre-filter receipt")
        }
    };

    let (mut filtered_backend, _filtered_session) = WgpuBackend::try_new().unwrap();
    filtered_backend
        .present(PresentRequest::live(
            vi,
            PhysicalRdramRead::from_storage(&rdram),
        ))
        .unwrap();
    let filtered = filtered_backend
        .presented_field()
        .expect("ordinary presentation must retain its filtered field");

    assert_eq!(source.presentation(), vi);
    assert_eq!(filtered.presentation, vi);
    assert_eq!(
        (source.stride_pixels(), source.height()),
        (WIDTH, OUTPUT_HEIGHT)
    );
    assert_eq!((filtered.width, filtered.height), (WIDTH, OUTPUT_HEIGHT));

    let center = ((WIDTH + 1) * 4) as usize;
    assert_eq!(
        &source.rgba8()[center..center + 4],
        &[expand_five(8), 0, expand_five(5), 255],
        "the source receipt must remain the plain pre-filter RGB5 expansion"
    );
    assert_eq!(
        &filtered.rgba8[center..center + 4],
        &[64, 0, 32, 255],
        "the post-VI receipt must contain the independently derived restoration"
    );
    assert_ne!(
        source.rgba8(),
        filtered.rgba8,
        "a filter-causal fixture must fail closed if the two stage receipts collapse"
    );
}
