//! Public, non-ROM F3DEX2/Extended-GBI evidence for pinned RT64 HFR.
//!
//! The opt-in transport substitutes only this fixture's F3DEX2 dialect. The
//! task still negotiates Extended v1 and emits typed refresh/transform-group
//! cooperation, while production `process_task` recognition remains strict.

use std::error::Error;
use std::io;

use fn64_render::{
    FrameStatus, OsTask, RefreshRateTarget, RenderBackend, RenderConfig, RenderError,
    RenderFiltering, RenderGraphicsApi, RenderRefreshRate, RenderRuntimeSettings,
    RenderSettingsApply, ViFilterControl, ViPixelType, ViPresentation, M_GFXTASK,
};
use fn64_render_rt64::{
    extended_gbi::{
        AspectMode, Availability, Component, MatrixGroup, MatrixMode, MatrixOrder, Policy, Version1,
    },
    gbi, Rt64Backend, Rt64ExtendedAspectMode, Rt64ExtendedGbiEvidence, Rt64ExtendedPresentedPixels,
    Rt64HfrEvidence, Rt64HfrPresentedPixels, Rt64PresentPixelFormat, Rt64SourceProvenance,
    Rt64TransformClass, Rt64TransformComponentSelector, Rt64TransformOrdering,
};
use sha2::{Digest, Sha256};

const PINNED_SOURCE: &str = "git:f0728a2520d5aa735886240de3fee75cc805f6d6";
const PINNED_OVERLAY: &str =
    "fn64:raster-shader-start-stop:v1+vi-region-rate:v1+hfr-post-present-call:v1";
const RDRAM_LEN: usize = 8 * 1024 * 1024;
const SEGMENT: u8 = 6;
const SEGMENT_BASE: usize = 0x0000_1000;
const VERTICES: usize = SEGMENT_BASE;
const PROJECTION: usize = SEGMENT_BASE + 0x0200;
const MODEL: usize = SEGMENT_BASE + 0x0240;
const VIEWPORT: usize = SEGMENT_BASE + 0x0280;
const VERSION_WORD: usize = 0x0000_1800;
const DISPLAY_LIST: usize = 0x0000_3000;
const TARGET: usize = 0x0040_0000;
const WIDTH: u32 = 64;
const HEIGHT: u32 = 48;
const ORIGINAL_RATE: u16 = 60;
const TARGET_RATE: u32 = 120;
const PACING_BURSTS: usize = 8;
const TARGET_PERIOD_NS: u64 = 1_000_000_000 / TARGET_RATE as u64;
const EXPECTED_PREVIOUS_SHA256: &str =
    "ded8a5aefa1e7e5b77e74de18a8490f4db0cbe03d6f527939c00815aa19c7ad2";
const EXPECTED_MIDPOINT_SHA256: &str =
    "af5e25c1f10351d0fddb503a545a173c2167bcf137ab9df83ab43b6b86dc45b0";
const EXPECTED_ENDPOINT_SHA256: &str =
    "b7116e2234e90cc2eaa468cd8506204c1015285bcb03c5b5672c118c38b22e61";

fn wr_u32(rdram: &mut [u8], offset: usize, value: u32) {
    rdram[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn rd_u32(rdram: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(
        rdram[offset..offset + 4]
            .try_into()
            .expect("four-byte Extended version response"),
    )
}

fn wr_u8(rdram: &mut [u8], offset: usize, value: u8) {
    rdram[offset ^ 3] = value;
}

fn wr_i16(rdram: &mut [u8], offset: usize, value: i16) {
    for (index, byte) in (value as u16).to_be_bytes().into_iter().enumerate() {
        wr_u8(rdram, offset + index, byte);
    }
}

fn write_matrix(rdram: &mut [u8], offset: usize, elements: [f32; 16]) {
    for (index, value) in elements.into_iter().enumerate() {
        let fixed = (value * 65536.0) as i32;
        wr_i16(rdram, offset + index * 2, (fixed >> 16) as i16);
        wr_i16(rdram, offset + 32 + index * 2, fixed as u16 as i16);
    }
}

fn write_scene(rdram: &mut [u8], translation_x: f32, prefix: &[(u32, u32)]) {
    assert!(DISPLAY_LIST + 0x100 <= rdram.len());
    assert!(TARGET + WIDTH as usize * HEIGHT as usize * 2 <= rdram.len());
    let vertices = [
        ([-5_i16, -5_i16, 0_i16], [255_u8, 255_u8, 255_u8, 255_u8]),
        ([5, -5, 0], [255, 255, 255, 255]),
        ([0, 6, 0], [255, 255, 255, 255]),
    ];
    for (index, (position, color)) in vertices.into_iter().enumerate() {
        let offset = VERTICES + index * 16;
        wr_i16(rdram, offset, position[0]);
        wr_i16(rdram, offset + 2, position[1]);
        wr_i16(rdram, offset + 4, position[2]);
        for (channel, value) in color.into_iter().enumerate() {
            wr_u8(rdram, offset + 12 + channel, value);
        }
    }

    let mut projection = [0.0_f32; 16];
    projection[0] = 1.0 / 16.0;
    projection[5] = 1.0 / 16.0;
    projection[10] = 1.0;
    projection[15] = 1.0;
    write_matrix(rdram, PROJECTION, projection);

    let mut model = [0.0_f32; 16];
    model[0] = 1.0;
    model[5] = 1.0;
    model[10] = 1.0;
    model[12] = translation_x;
    model[15] = 1.0;
    write_matrix(rdram, MODEL, model);

    for (index, value) in [
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
        (WIDTH * 2) as i16,
        (HEIGHT * 2) as i16,
        511,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        wr_i16(rdram, VIEWPORT + index * 2, value);
    }

    let mut commands = prefix.to_vec();
    let mut push = |word0: u32, word1: u32| {
        commands.push((word0, word1));
    };
    push(
        ((gbi::G_MOVEWORD as u32) << 24) | (0x06 << 16) | (u32::from(SEGMENT) * 4),
        SEGMENT_BASE as u32,
    );
    push(
        ((gbi::G_MOVEMEM as u32) << 24) | (1 << 19) | 8,
        (u32::from(SEGMENT) << 24) | 0x0280,
    );
    push(0xff10_0000 | (WIDTH - 1), TARGET as u32);
    push(0xed00_0000, (WIDTH * 4) << 12 | (HEIGHT * 4));
    push(0xef30_00f0, 0);
    push(0xf700_0000, 0x0001_0001);
    push(
        0xf600_0000 | (((WIDTH - 1) * 4) << 12) | ((HEIGHT - 1) * 4),
        0,
    );
    push(0xe700_0000, 0);
    let matrix_length = (((64_u32 - 1) / 8) & 0x1f) << 19;
    push(
        ((gbi::G_MTX as u32) << 24) | matrix_length | 0x07,
        (u32::from(SEGMENT) << 24) | 0x0200,
    );
    push(
        ((gbi::G_MTX as u32) << 24) | matrix_length | 0x03,
        (u32::from(SEGMENT) << 24) | 0x0240,
    );
    push(0xfcff_ffff, 0xfffd_f6fb);
    push(0xfa00_0000, 0xf800_00ff);
    push(0xef00_00f0, 0);
    push(
        ((gbi::G_VTX as u32) << 24) | (3 << 12) | (3 << 1),
        u32::from(SEGMENT) << 24,
    );
    push(((gbi::G_TRI1 as u32) << 24) | (1 << 9) | (2 << 1), 0);
    push(0xe900_0000, 0);
    push((gbi::G_ENDDL as u32) << 24, 0);
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        wr_u32(rdram, DISPLAY_LIST + index * 8, word0);
        wr_u32(rdram, DISPLAY_LIST + index * 8 + 4, word1);
    }
}

fn cooperation_commands(version: Version1) -> Vec<(u32, u32)> {
    let mut commands = vec![
        version.enable_command().words(),
        version
            .set_refresh_rate(ORIGINAL_RATE)
            .expect("the fixture source rate is nonzero")
            .words(),
    ];
    commands.extend(
        version
            .matrix_group(MatrixGroup {
                id: 7,
                mode: MatrixMode::Decompose,
                position: Component::Interpolate,
                rotation: Component::Interpolate,
                order: MatrixOrder::Auto,
                editable: true,
                aspect: AspectMode::Auto,
                ..MatrixGroup::default()
            })
            .map(|command| command.words()),
    );
    commands
}

fn negotiate_v1(backend: &mut Rt64Backend, rdram: &mut [u8]) -> Result<Version1, Box<dyn Error>> {
    let probe = Policy::Required
        .probe(VERSION_WORD as u32)?
        .expect("required cooperation emits a probe");
    wr_u32(
        rdram,
        VERSION_WORD,
        fn64_render_rt64::extended_gbi::Probe::RETURN_WORD_INITIALIZER,
    );
    write_scene(rdram, -6.0, &[probe.command().words()]);
    backend.process_synthetic_hfr_f3dex2(
        rdram,
        DISPLAY_LIST as u32,
        TARGET as u32,
        ORIGINAL_RATE,
    )?;
    match probe.resolve(rd_u32(rdram, VERSION_WORD))? {
        Availability::Version1(version) => Ok(version),
        Availability::Unavailable => Err(io::Error::other(
            "required synthetic F3DEX2 Extended-GBI cooperation was unavailable",
        )
        .into()),
    }
}

fn presentation() -> ViPresentation {
    ViPresentation {
        noise_seed: 0x4846_5231,
        scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
            pixel_type: ViPixelType::Rgba16,
            ..ViFilterControl::default()
        }),
        ..ViPresentation::default()
    }
}

fn settings(refresh_rate: RenderRefreshRate) -> RenderRuntimeSettings {
    RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        refresh_rate,
        refresh_rate_target: RefreshRateTarget::new(TARGET_RATE)
            .expect("120 Hz is in the typed range"),
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Burst {
    evidence: Rt64HfrEvidence,
    frames: Vec<Rt64HfrPresentedPixels>,
}

fn submit(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    x: f32,
    cooperation: Option<Version1>,
) -> Result<(), Box<dyn Error>> {
    let prefix = cooperation.map(cooperation_commands).unwrap_or_default();
    write_scene(rdram, x, &prefix);
    backend.process_synthetic_hfr_f3dex2(
        rdram,
        DISPLAY_LIST as u32,
        TARGET as u32,
        ORIGINAL_RATE,
    )?;
    backend.present_physical_compatibility(&*rdram, presentation())?;
    Ok(())
}

fn capture_burst(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    x: f32,
) -> Result<Burst, Box<dyn Error>> {
    backend.enable_hfr_evidence()?;
    submit(backend, rdram, x, None)?;
    let evidence = backend.hfr_evidence()?;
    let frames = backend.hfr_presented_pixels()?;
    Ok(Burst { evidence, frames })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CooperationBurst {
    evidence: Rt64ExtendedGbiEvidence,
    frames: Vec<Rt64ExtendedPresentedPixels>,
}

fn capture_cooperation_burst(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    x: f32,
    version: Version1,
) -> Result<CooperationBurst, Box<dyn Error>> {
    backend.enable_extended_gbi_evidence()?;
    submit(backend, rdram, x, Some(version))?;
    let evidence = backend.extended_gbi_evidence()?;
    let frames = backend.extended_presented_pixels()?;
    Ok(CooperationBurst { evidence, frames })
}

fn frame_digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ColoredShape {
    count: u32,
    min_x: u32,
    max_x: u32,
    min_y: u32,
    max_y: u32,
    sum_x: u64,
    sum_y: u64,
}

const EXPECTED_PREVIOUS_SHAPE: ColoredShape = ColoredShape {
    count: 161,
    min_x: 19,
    max_x: 37,
    min_y: 15,
    max_y: 29,
    sum_x: 4508,
    sum_y: 3902,
};
const EXPECTED_MIDPOINT_SHAPE: ColoredShape = ColoredShape {
    count: 161,
    min_x: 21,
    max_x: 39,
    min_y: 15,
    max_y: 29,
    sum_x: 4830,
    sum_y: 3902,
};
const EXPECTED_ENDPOINT_SHAPE: ColoredShape = ColoredShape {
    count: 161,
    min_x: 23,
    max_x: 41,
    min_y: 15,
    max_y: 29,
    sum_x: 5152,
    sum_y: 3902,
};

fn colored_shape(
    bytes: &[u8],
    width: u32,
    height: u32,
    row_bytes: u32,
    format: Rt64PresentPixelFormat,
) -> Result<ColoredShape, Box<dyn Error>> {
    let mut shape = ColoredShape {
        count: 0,
        min_x: width,
        max_x: 0,
        min_y: height,
        max_y: 0,
        sum_x: 0,
        sum_y: 0,
    };
    for y in 0..height {
        for x in 0..width {
            let offset = (y * row_bytes + x * 4) as usize;
            let (red, green, blue) = match format {
                Rt64PresentPixelFormat::Bgra8Unorm => {
                    (bytes[offset + 2], bytes[offset + 1], bytes[offset])
                }
                Rt64PresentPixelFormat::Rgba8Unorm => {
                    (bytes[offset], bytes[offset + 1], bytes[offset + 2])
                }
            };
            if red > 128 && green < 64 && blue < 64 {
                shape.count += 1;
                shape.min_x = shape.min_x.min(x);
                shape.max_x = shape.max_x.max(x);
                shape.min_y = shape.min_y.min(y);
                shape.max_y = shape.max_y.max(y);
                shape.sum_x += u64::from(x);
                shape.sum_y += u64::from(y);
            }
        }
    }
    if shape.count == 0 {
        return Err(io::Error::other("HFR image contains no exact red foreground shape").into());
    }
    Ok(shape)
}

fn hex_digest(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn active_digest(backend: &Rt64Backend) -> Result<[u8; 32], Box<dyn Error>> {
    backend
        .active_runtime_policy()
        .map(|policy| policy.sha256())
        .ok_or_else(|| io::Error::other("RT64 has no active runtime policy").into())
}

fn apply_live(
    backend: &mut Rt64Backend,
    runtime: &RenderRuntimeSettings,
) -> Result<(), Box<dyn Error>> {
    match backend.apply_runtime_settings(runtime)? {
        RenderSettingsApply::LiveApplied {
            settings_sha256, ..
        } if settings_sha256 == runtime.sha256() => Ok(()),
        result => Err(io::Error::other(format!(
            "RT64 refresh policy was not applied live: {result:?}"
        ))
        .into()),
    }
}

fn require_production_rejection(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
) -> Result<(), Box<dyn Error>> {
    let status = backend.process_task(
        rdram,
        &mut fn64_runtime::RspMemory::new(),
        &OsTask {
            task_type: M_GFXTASK,
            ..Default::default()
        },
        0,
    )?;
    if !matches!(status, FrameStatus::NeedsLle { .. }) {
        return Err(io::Error::other(
            "production process_task bypassed recognized-microcode admission",
        )
        .into());
    }
    Ok(())
}

fn require_hfr_first_arm_overlap_rejection(
    runtime: &RenderRuntimeSettings,
) -> Result<(), Box<dyn Error>> {
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime.clone());
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    backend.enable_hfr_evidence()?;
    match backend.enable_extended_gbi_evidence() {
        Err(RenderError::Backend { reason, .. })
            if reason
                == "RT64 Extended-GBI evidence cannot overlap another armed presentation history" =>
        {
            Ok(())
        }
        result => Err(io::Error::other(format!(
            "Extended evidence did not reject HFR-first overlapping arm: {result:?}"
        ))
        .into()),
    }
}

#[derive(Debug, Eq, PartialEq)]
struct HfrEvidence {
    previous_endpoint: [u8; 32],
    intermediate: [u8; 32],
    current_endpoint: [u8; 32],
    original_policy: [u8; 32],
    previous_shape: ColoredShape,
    intermediate_shape: ColoredShape,
    current_shape: ColoredShape,
    cooperation: CooperationSummary,
    pacing: PacingSummary,
}

impl HfrEvidence {
    fn deterministic_eq(&self, other: &Self) -> bool {
        self.previous_endpoint == other.previous_endpoint
            && self.intermediate == other.intermediate
            && self.current_endpoint == other.current_endpoint
            && self.original_policy == other.original_policy
            && self.previous_shape == other.previous_shape
            && self.intermediate_shape == other.intermediate_shape
            && self.current_shape == other.current_shape
            && self.cooperation == other.cooperation
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
struct CooperationSummary {
    workload_id: u64,
    previous_workload_id: u64,
    current_workload_id: u64,
    present_id: u64,
    source_rate: u32,
    target_rate: u32,
    group_id: u32,
}

#[derive(Debug, Eq, PartialEq)]
struct PacingSummary {
    within_burst_start_intervals_ns: Vec<u64>,
    median_interval_ns: u64,
    tight_interval_count: usize,
    maximum_present_call_duration_ns: u64,
}

fn capture_pacing(
    backend: &mut Rt64Backend,
    rdram: &mut [u8],
    version: Version1,
) -> Result<PacingSummary, Box<dyn Error>> {
    backend.enable_hfr_pacing_evidence()?;
    for burst in 0..PACING_BURSTS {
        write_scene(rdram, 0.0, &cooperation_commands(version));
        backend.process_synthetic_hfr_f3dex2(
            rdram,
            DISPLAY_LIST as u32,
            TARGET as u32,
            ORIGINAL_RATE,
        )?;
        let mut pacing_presentation = presentation();
        // Force a real VI event for every workload without changing its color
        // target. A repeated identical VI can be coalesced before it reaches
        // the swapchain-present path this evidence is measuring.
        pacing_presentation.repeat_line = burst % 2 == 0;
        backend.present_physical_compatibility(&*rdram, pacing_presentation)?;
    }
    let pacing = backend.hfr_pacing_evidence()?;
    if pacing.samples.len() != PACING_BURSTS * 2 {
        return Err(io::Error::other(format!(
            "HFR pacing captured {} calls instead of {}",
            pacing.samples.len(),
            PACING_BURSTS * 2,
        ))
        .into());
    }

    let broad_min = TARGET_PERIOD_NS * 70 / 100;
    let broad_max = TARGET_PERIOD_NS * 250 / 100;
    let tight_min = TARGET_PERIOD_NS * 85 / 100;
    let tight_max = TARGET_PERIOD_NS * 135 / 100;
    let median_min = TARGET_PERIOD_NS * 90 / 100;
    let median_max = TARGET_PERIOD_NS * 120 / 100;
    let mut intervals = Vec::with_capacity(PACING_BURSTS);
    let mut maximum_call_duration = 0;
    for pair in pacing.samples.chunks_exact(2) {
        if pair[0].present_id != pair[1].present_id
            || pair[0].burst_ordinal != 0
            || pair[1].burst_ordinal != 1
            || pair[0].burst_count != 2
            || pair[1].burst_count != 2
            || pair[0].original_refresh_rate != u32::from(ORIGINAL_RATE)
            || pair[1].original_refresh_rate != u32::from(ORIGINAL_RATE)
            || pair[0].target_refresh_rate != TARGET_RATE
            || pair[1].target_refresh_rate != TARGET_RATE
        {
            return Err(io::Error::other(
                "HFR pacing call pairs lost exact 120/60 burst provenance",
            )
            .into());
        }
        intervals.push(
            pair[1]
                .call_start_monotonic_ns
                .checked_sub(pair[0].call_start_monotonic_ns)
                .ok_or_else(|| io::Error::other("HFR pacing timestamps regressed"))?,
        );
        for sample in pair {
            maximum_call_duration = maximum_call_duration.max(
                sample
                    .call_return_monotonic_ns
                    .checked_sub(sample.call_start_monotonic_ns)
                    .ok_or_else(|| io::Error::other("HFR present call returned before start"))?,
            );
        }
    }
    if intervals
        .iter()
        .any(|interval| !((broad_min..=broad_max).contains(interval)))
    {
        return Err(io::Error::other(format!(
            "HFR post-sleep present-call interval escaped predeclared broad bounds {broad_min}..={broad_max}: {intervals:?}"
        ))
        .into());
    }
    let tight_count = intervals
        .iter()
        .filter(|interval| (tight_min..=tight_max).contains(interval))
        .count();
    let mut sorted = intervals.clone();
    sorted.sort_unstable();
    let median = (sorted[PACING_BURSTS / 2 - 1] + sorted[PACING_BURSTS / 2]) / 2;
    if tight_count < PACING_BURSTS - 1 || !(median_min..=median_max).contains(&median) {
        return Err(io::Error::other(format!(
            "HFR post-sleep present-call cadence missed predeclared scheduler bounds: tight={tight_count}/{PACING_BURSTS} median={median} bounds={median_min}..={median_max} intervals={intervals:?}"
        ))
        .into());
    }
    Ok(PacingSummary {
        within_burst_start_intervals_ns: intervals,
        median_interval_ns: median,
        tight_interval_count: tight_count,
        maximum_present_call_duration_ns: maximum_call_duration,
    })
}

fn run_pacing_lane(
    original: &RenderRuntimeSettings,
    manual: &RenderRuntimeSettings,
) -> Result<PacingSummary, Box<dyn Error>> {
    let mut backend = Rt64Backend::new().with_runtime_settings(original.clone());
    backend.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    let mut rdram = vec![0_u8; RDRAM_LEN];
    require_production_rejection(&mut backend, &mut rdram)?;
    let version = negotiate_v1(&mut backend, &mut rdram)?;
    submit(&mut backend, &mut rdram, -4.0, Some(version))?;
    apply_live(&mut backend, manual)?;
    submit(&mut backend, &mut rdram, -2.0, Some(version))?;
    let pacing = capture_pacing(&mut backend, &mut rdram, version)?;
    require_production_rejection(&mut backend, &mut rdram)?;
    Ok(pacing)
}

fn run_once() -> Result<HfrEvidence, Box<dyn Error>> {
    let original = settings(RenderRefreshRate::Original);
    let manual = settings(RenderRefreshRate::Manual);
    require_hfr_first_arm_overlap_rejection(&original)?;

    let mut control = Rt64Backend::new().with_runtime_settings(original.clone());
    control.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    control.enable_present_capture()?;
    let mut control_rdram = vec![0_u8; RDRAM_LEN];
    submit(&mut control, &mut control_rdram, -4.0, None)?;
    submit(&mut control, &mut control_rdram, -2.0, None)?;
    let control_burst = capture_burst(&mut control, &mut control_rdram, 0.0)?;
    if control_burst.evidence.presentation_count != 1
        || control_burst.evidence.target_refresh_rate != 0
        || !control_burst.evidence.presentations.is_empty()
        || control_burst.frames.len() != 1
        || control_burst.frames[0].burst_ordinal.is_some()
    {
        return Err(io::Error::other("Original refresh control generated an HFR burst").into());
    }
    let control_endpoint = frame_digest(&control_burst.frames[0].bytes);

    let mut enabled = Rt64Backend::new().with_runtime_settings(original.clone());
    enabled.create(&RenderConfig::ntsc(WIDTH, HEIGHT))?;
    enabled.enable_present_capture()?;
    let mut enabled_rdram = vec![0_u8; RDRAM_LEN];
    require_production_rejection(&mut enabled, &mut enabled_rdram)?;
    let version = negotiate_v1(&mut enabled, &mut enabled_rdram)?;
    submit(&mut enabled, &mut enabled_rdram, -4.0, Some(version))?;
    let original_policy = active_digest(&enabled)?;
    apply_live(&mut enabled, &manual)?;
    let manual_policy = active_digest(&enabled)?;
    if original_policy == manual_policy {
        return Err(io::Error::other("HFR runtime toggle did not change active policy").into());
    }
    // Matching and rigid-body velocity tracking only run while HFR is active.
    // This first +2 step seeds velocity; the captured second +2 step exercises
    // AUTO translation interpolation with a stable prior velocity.
    submit(&mut enabled, &mut enabled_rdram, -2.0, Some(version))?;
    let previous_capture = enabled.presented_pixels()?;
    let previous_endpoint = Sha256::digest(&previous_capture.bytes).into();
    let generated = capture_cooperation_burst(&mut enabled, &mut enabled_rdram, 0.0, version)?;
    if generated.evidence.enabled_opcode != Some(0x64)
        || generated.evidence.hook_enable_count != 1
        || generated.evidence.command_counts[0x09] != 1
        || generated.evidence.command_counts[0x0c] != 1
        || generated.evidence.command_counts.iter().sum::<u32>() != 2
        || generated.evidence.refresh_rate != Some(ORIGINAL_RATE)
        || generated.evidence.groups.len() != 1
        || generated.evidence.generated_presents.len() != 2
        || generated.frames.len() != 2
    {
        return Err(io::Error::other(
            "Manual 120 Hz did not retain exact Extended cooperation and two HFR frames",
        )
        .into());
    }
    let group = generated.evidence.groups[0];
    if group.group_id != 7
        || group.class != Rt64TransformClass::Model
        || !group.decompose
        || !group.editable
        || group.position != Rt64TransformComponentSelector::Interpolate
        || group.rotation != Rt64TransformComponentSelector::Interpolate
        || group.ordering != Rt64TransformOrdering::Auto
        || group.aspect_mode != Rt64ExtendedAspectMode::Auto
    {
        return Err(io::Error::other(format!(
            "HFR Extended transform-group cooperation drifted: {group:?}"
        ))
        .into());
    }
    for (ordinal, generated_present) in generated.evidence.generated_presents.iter().enumerate() {
        if generated_present.current_workload_id != generated.evidence.workload_id
            || generated_present.previous_workload_id == generated_present.current_workload_id
            || generated_present.present_id != generated.evidence.present_id
            || generated_present.presentation_ordinal != ordinal as u32
            || generated_present.interpolation_numerator != ordinal as u32 + 1
            || generated_present.interpolation_denominator != 2
            || generated_present.original_refresh_rate != u32::from(ORIGINAL_RATE)
            || generated_present.target_refresh_rate != TARGET_RATE
        {
            return Err(io::Error::other(format!(
                "HFR Extended generated-presentation provenance drifted at {ordinal}: {generated_present:?}"
            ))
            .into());
        }
    }
    let half = frame_digest(&generated.frames[0].bytes);
    let current_endpoint = frame_digest(&generated.frames[1].bytes);
    if generated.frames[0].workload_id != generated.evidence.workload_id
        || generated.frames[1].workload_id != generated.evidence.workload_id
        || generated.frames[0].present_id != generated.evidence.present_id
        || generated.frames[1].present_id != generated.evidence.present_id
        || generated.frames[0].capture_ordinal != 0
        || generated.frames[1].capture_ordinal != 1
        || generated.frames[0].capture_generation >= generated.frames[1].capture_generation
        || generated.frames[0].generated_ordinal != Some(0)
        || generated.frames[0].interpolation_numerator != 1
        || generated.frames[0].interpolation_denominator != 2
        || generated.frames[1].generated_ordinal != Some(1)
        || generated.frames[1].interpolation_numerator != 2
        || generated.frames[1].interpolation_denominator != 2
        || half == previous_endpoint
        || half == current_endpoint
        || previous_endpoint == current_endpoint
        || current_endpoint != control_endpoint
    {
        return Err(io::Error::other(format!(
            "HFR intermediate/endpoint mismatch: previous={} half={} current={} control={}",
            hex_digest(previous_endpoint),
            hex_digest(half),
            hex_digest(current_endpoint),
            hex_digest(control_endpoint),
        ))
        .into());
    }
    if hex_digest(previous_endpoint) != EXPECTED_PREVIOUS_SHA256
        || hex_digest(half) != EXPECTED_MIDPOINT_SHA256
        || hex_digest(current_endpoint) != EXPECTED_ENDPOINT_SHA256
    {
        return Err(io::Error::other(format!(
            "HFR exact pixel digests drifted: previous={} midpoint={} endpoint={}",
            hex_digest(previous_endpoint),
            hex_digest(half),
            hex_digest(current_endpoint),
        ))
        .into());
    }

    let previous_shape = colored_shape(
        &previous_capture.bytes,
        previous_capture.width,
        previous_capture.height,
        previous_capture.row_bytes,
        previous_capture.format,
    )?;
    let intermediate_shape = colored_shape(
        &generated.frames[0].bytes,
        generated.frames[0].width,
        generated.frames[0].height,
        generated.frames[0].row_bytes,
        generated.frames[0].format,
    )?;
    let current_shape = colored_shape(
        &generated.frames[1].bytes,
        generated.frames[1].width,
        generated.frames[1].height,
        generated.frames[1].row_bytes,
        generated.frames[1].format,
    )?;
    let cooperation = CooperationSummary {
        workload_id: generated.evidence.workload_id,
        previous_workload_id: generated.evidence.generated_presents[0].previous_workload_id,
        current_workload_id: generated.evidence.generated_presents[0].current_workload_id,
        present_id: generated.evidence.present_id,
        source_rate: generated.evidence.generated_presents[0].original_refresh_rate,
        target_rate: generated.evidence.generated_presents[0].target_refresh_rate,
        group_id: group.group_id,
    };
    if previous_shape != EXPECTED_PREVIOUS_SHAPE
        || intermediate_shape != EXPECTED_MIDPOINT_SHAPE
        || current_shape != EXPECTED_ENDPOINT_SHAPE
    {
        return Err(io::Error::other(format!(
            "HFR exact spatial shapes drifted: previous={previous_shape:?} midpoint={intermediate_shape:?} endpoint={current_shape:?}"
        ))
        .into());
    }
    if previous_shape.count != intermediate_shape.count
        || intermediate_shape.count != current_shape.count
        || previous_shape.min_y != intermediate_shape.min_y
        || intermediate_shape.min_y != current_shape.min_y
        || previous_shape.max_y != intermediate_shape.max_y
        || intermediate_shape.max_y != current_shape.max_y
        || !(previous_shape.min_x < intermediate_shape.min_x
            && intermediate_shape.min_x < current_shape.min_x)
        || !(previous_shape.max_x < intermediate_shape.max_x
            && intermediate_shape.max_x < current_shape.max_x)
        || !(previous_shape.sum_x < intermediate_shape.sum_x
            && intermediate_shape.sum_x < current_shape.sum_x)
        || previous_shape.sum_y != intermediate_shape.sum_y
        || intermediate_shape.sum_y != current_shape.sum_y
    {
        return Err(io::Error::other(format!(
            "HFR spatial shape is not a stable ordered intermediate: previous={previous_shape:?} intermediate={intermediate_shape:?} current={current_shape:?}"
        ))
        .into());
    }

    // Synthetic admission must not persist into the production task path.
    require_production_rejection(&mut enabled, &mut enabled_rdram)?;
    require_production_rejection(&mut enabled, &mut enabled_rdram)?;
    let pacing = run_pacing_lane(&original, &manual)?;

    apply_live(&mut enabled, &original)?;
    if active_digest(&enabled)? != original_policy {
        return Err(io::Error::other("HFR toggle-back did not restore Original policy").into());
    }
    let toggle_back = capture_burst(&mut enabled, &mut enabled_rdram, 0.0)?;
    if toggle_back.evidence.presentation_count != 1
        || toggle_back.evidence.target_refresh_rate != 0
        || toggle_back.frames.len() != 1
        || frame_digest(&toggle_back.frames[0].bytes) != control_endpoint
    {
        return Err(io::Error::other(
            "HFR toggle-back did not restore exact Original endpoint behavior",
        )
        .into());
    }
    Ok(HfrEvidence {
        previous_endpoint,
        intermediate: half,
        current_endpoint,
        original_policy,
        previous_shape,
        intermediate_shape,
        current_shape,
        cooperation,
        pacing,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let run_count = match args.next().as_deref() {
        None => 1,
        Some("--once") if args.next().is_none() => 1,
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
        return Err(io::Error::other("HFR evidence requires clean pinned Metal RT64").into());
    }
    let expected = run_once()?;
    for _ in 1..run_count {
        let observed = run_once()?;
        if !observed.deterministic_eq(&expected) {
            return Err(
                io::Error::other("RT64 deterministic HFR evidence drifted between runs").into(),
            );
        }
    }
    println!(
        "RT64 HFR passed {run_count} run(s): previous={} half={} current={} shapes={:?}->{:?}->{:?} cooperation={:?} pacing={:?}",
        hex_digest(expected.previous_endpoint),
        hex_digest(expected.intermediate),
        hex_digest(expected.current_endpoint),
        expected.previous_shape,
        expected.intermediate_shape,
        expected.current_shape,
        expected.cooperation,
        expected.pacing,
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> Version1 {
        Policy::Required
            .probe(VERSION_WORD as u32)
            .unwrap()
            .unwrap()
            .resolve(1)
            .unwrap()
            .require_v1()
            .unwrap()
    }

    #[test]
    fn cooperation_prefix_is_exact_and_runtime_optional() {
        let commands = cooperation_commands(version());
        let selectors = (1 << 2) | (1 << 3) | (1 << 5) | (1 << 17) | (1 << 19);
        assert_eq!(
            commands,
            vec![
                (0xe052_5464, 0x1000_0064),
                (0x6400_0009, u32::from(ORIGINAL_RATE)),
                (0x6400_000c, 7),
                (selectors, 0),
            ]
        );
        assert!(Option::<Version1>::None
            .map(cooperation_commands)
            .unwrap_or_default()
            .is_empty());
    }

    #[test]
    fn cooperation_prefix_precedes_the_public_f3dex2_scene() {
        let commands = cooperation_commands(version());
        let mut rdram = vec![0; RDRAM_LEN];
        write_scene(&mut rdram, 0.0, &commands);
        for (index, (word0, word1)) in commands.into_iter().enumerate() {
            assert_eq!(rd_u32(&rdram, DISPLAY_LIST + index * 8), word0);
            assert_eq!(rd_u32(&rdram, DISPLAY_LIST + index * 8 + 4), word1);
        }
        assert_eq!(
            rd_u32(&rdram, DISPLAY_LIST + 4 * 8) >> 24,
            u32::from(gbi::G_MOVEWORD)
        );
    }
}
