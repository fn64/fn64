//! Live RT64 production-context region-rate evidence.
//!
//! The synthetic transport substitutes only the hand-authored fixture's
//! F3DEX2 identity. It deliberately emits no Extended GBI SetRefreshRate
//! command, then reads the `viOriginalRate` stored by RT64 FullSync.

use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, OsTask, RenderBackend, RenderConfig, RenderFiltering, RenderGraphicsApi,
    RenderRuntimeSettings, ViPresentation, ViScaleAxis, ViScanoutRegisters, ViScanoutState,
    M_GFXTASK,
};
use fn64_render_rt64::{Rt64Backend, Rt64RegionRateEvidence, Rt64SourceProvenance};
use fn64_runtime::{RspMemory, TvType};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const PINNED_OVERLAY: &str =
    "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+ucode-generation-admission:v1+vi-gamma-dither:v1+vi-retrace-cadence:v1";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const DISPLAY_LIST: usize = 0x0000_2000;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const HISTORY_DEPTH: usize = 3;

fn install_display_list(rdram: &mut [u8], fill: u16) {
    let lower_right = (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4);
    let fill = u32::from(fill) * 0x1_0001;
    let commands = [
        (0xef30_00f0_u32, 0_u32),
        (0xff10_0000 | (WIDTH - 1), TARGET as u32),
        (0xf700_0000, fill),
        (0xf600_0000 | lower_right, 0),
        (0xe900_0000, 0),
        (0xdf00_0000, 0),
    ];
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        let offset = DISPLAY_LIST + index * 8;
        rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
        rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
    }
}

fn presentation(repeat_line: bool, ordinal: u64) -> ViPresentation {
    let mut words = [0_u32; ViScanoutRegisters::WORD_COUNT];
    words[0] = 0x302;
    words[1] = TARGET as u32;
    words[2] = WIDTH;
    words[6] = 525;
    words[7] = 3093;
    words[9] = (108 << 16) | (108 + WIDTH);
    words[10] = (34 << 16) | (34 + HEIGHT * 2);
    words[12] = u32::from(ViScaleAxis::ONE);
    words[13] = u32::from(ViScaleAxis::ONE);
    ViPresentation {
        repeat_line,
        scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
        noise_seed: 0x5245_4749_4f4e_0000 | ordinal,
        ..ViPresentation::default()
    }
}

fn settings() -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    }
}

fn require_production_rejection(backend: &mut Rt64Backend) -> Result<(), Box<dyn Error>> {
    let mut rdram = vec![0; RDRAM_LEN];
    let status = backend.process_task(
        &mut rdram,
        &mut RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ..OsTask::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other(
            "region-rate fixture broadened production microcode admission",
        )
        .into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RegionEvidence {
    tv_type: TvType,
    workloads: Vec<Rt64RegionRateEvidence>,
}

fn capture_region(tv_type: TvType) -> Result<RegionEvidence, Box<dyn Error>> {
    let expected_rate = tv_type.nominal_field_hz();
    let mut backend = Rt64Backend::new().with_runtime_settings(settings());
    backend.create(&RenderConfig::for_tv(WIDTH, HEIGHT, tv_type))?;
    require_production_rejection(&mut backend)?;

    let mut rdram = vec![0; RDRAM_LEN];
    let mut workloads = Vec::with_capacity(HISTORY_DEPTH + 1);
    for ordinal in 0..=HISTORY_DEPTH {
        let fill = if ordinal % 2 == 0 { 0xf801 } else { 0x07c1 };
        install_display_list(&mut rdram, fill);
        let evidence = backend.process_synthetic_region_rate_f3dex2(
            &mut rdram,
            DISPLAY_LIST as u32,
            TARGET as u32,
        )?;
        if evidence.configured_nominal_refresh_rate != expected_rate
            || evidence.registered_nominal_refresh_rate != expected_rate
            || workloads
                .last()
                .is_some_and(|previous: &Rt64RegionRateEvidence| {
                    previous.workload_id >= evidence.workload_id
                })
        {
            return Err(io::Error::other(format!(
                "RT64 region authority or workload order drifted for {tv_type:?}: {evidence:?}",
            ))
            .into());
        }
        workloads.push(evidence);

        // Alternating an ordinary live VI register changes one real VI field
        // between workloads. Three stable factor-one transitions populate the
        // complete pinned VIHistory without manufacturing its contents.
        backend.present_live(&rdram, presentation(ordinal % 2 != 0, ordinal as u64))?;
    }

    let final_workload = workloads.last().expect("the capture loop is nonempty");
    if final_workload.workload_original_refresh_rate != expected_rate {
        return Err(io::Error::other(format!(
            "RT64 FullSync inferred {} Hz instead of {expected_rate} Hz for {tv_type:?}: {workloads:?}",
            final_workload.workload_original_refresh_rate,
        ))
        .into());
    }
    Ok(RegionEvidence { tv_type, workloads })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SuiteEvidence {
    pal: RegionEvidence,
    mpal: RegionEvidence,
}

fn run_once() -> Result<SuiteEvidence, Box<dyn Error>> {
    Ok(SuiteEvidence {
        pal: capture_region(TvType::Pal)?,
        mpal: capture_region(TvType::Mpal)?,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let run_count = match args.next().as_deref() {
        None | Some("--once") if args.next().is_none() => 1,
        Some("--runs") => {
            let count = args
                .next()
                .ok_or_else(|| io::Error::other("--runs requires a positive count"))?
                .parse::<usize>()
                .map_err(|_| io::Error::other("--runs requires a positive count"))?;
            if count == 0 || args.next().is_some() {
                return Err(io::Error::other("--runs requires one positive count").into());
            }
            count
        }
        _ => return Err(io::Error::other("expected no arguments, --once, or --runs N").into()),
    };

    let identity = Rt64Backend::release_identity();
    if identity.source_id != PINNED_SOURCE
        || identity.source_provenance != Rt64SourceProvenance::GitClean
        || identity.source_overlay_id != PINNED_OVERLAY
        || identity.post_vi_api != "metal-bgra8-unorm"
    {
        return Err(io::Error::other(
            "region-rate evidence requires the clean pinned Metal RT64 overlay",
        )
        .into());
    }

    let expected = run_once()?;
    for _ in 1..run_count {
        let observed = run_once()?;
        if observed != expected {
            return Err(io::Error::other(
                "RT64 region-rate evidence drifted between context lifecycles",
            )
            .into());
        }
    }
    println!(
        "RT64 region-rate passed {run_count} run(s): PAL={:?} MPAL={:?}",
        expected.pal.workloads, expected.mpal.workloads,
    );
    Ok(())
}
