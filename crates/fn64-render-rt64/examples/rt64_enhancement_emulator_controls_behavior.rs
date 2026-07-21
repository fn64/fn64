use std::error::Error;
use std::io;

use fn64_render::{
    AspectTarget, FrameStatus, RenderAspectRatio, RenderBackend, RenderConfig,
    RenderEmulatorSettings, RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi,
    RenderPolicyApply, RenderResolution, RenderRuntimeSettings, ViFilterControl, ViPixelType,
    ViPresentation,
};
use fn64_render_rt64::{
    Rt64Backend, Rt64DeferredWorkloadEvidence, Rt64FramebufferCopyPath,
    Rt64FramebufferCopyPathEvidence, Rt64PresentPixelFormat, Rt64PresentSelection,
    Rt64PresentedPixels, Rt64SourceProvenance,
};
use fn64_runtime::{RdramAddr, RdramView, RdramViewMut};
use sha2::{Digest, Sha256};

const RDRAM_LEN: usize = 8 * 1024 * 1024;
const COMMANDS: usize = 0x100;
const TEXTURE: u32 = 0x1000;
const TARGET: u32 = 0x2000;
const WIDTH: u32 = 16;
const HEIGHT: u32 = 16;
const TEXTURE_SIZE: u32 = 16;
const RED: u16 = 0xf801;
const BLUE: u16 = 0x003f;
const COPY_SOURCE: u32 = 0x4000;
const COPY_SAMPLED: u32 = 0x5000;
const COPY_WIDTH: u32 = 8;
const COPY_SOURCE_HEIGHT: u32 = 4;
const COPY_SAMPLED_HEIGHT: u32 = 2;
const STALE_SOURCE: u16 = 0x07c1;
const STALE_SAMPLED: u16 = 0xffff;
const GUARD: u16 = 0x4211;
const BASELINE_POLICY: &str = "12d7f671b217a8abbd86aeddf9a1449bfd65a5159eaab70084eb066a1f398a33";
const BASELINE_PIXELS: &str = "6a845eadee1c6f62daded66c7812f6734f4ceafdb088af053d34c5cc28992180";
const BASELINE_RDRAM: &str = "15203c2e7aa5fc758385427ea56dd9dce619679fb485d8369ec6540f624b3942";
const GPU_COPY_POLICY: &str = "6bea85ab6c10adb405a24b2c4fbd578748f8120812d621a47c0a3898dca7932e";
const CPU_COPY_POLICY: &str = "e37cd03ceab8ca594ae8a75bde0001b1ef1a12f025524b025f53807966db0ace";
const COPY_SOURCE_SHA256: &str = "fd2466a1d9b73a1d94eadad4aa7b8f5c321977a5d1fdca152467bf3243bf73b6";
const COPY_SAMPLED_SHA256: &str =
    "8b89d30ef41be4388505de97a91fc67f206bb67d17e293274aebf4de19614e12";
const COPY_POST_VI_SHA256: &str =
    "43da36ec5d257ff8d5aec0f714d9a5a59c338eae40079813bf0e1fc0ef321b2c";
const COPY_WORKLOAD_CONTENT: u64 = 0x9168_4b9b_1134_f961;

#[derive(Copy, Clone)]
struct Expected {
    policy: &'static str,
    pixels: &'static str,
    rdram: &'static str,
    changed_pixels: usize,
    changed_rdram_pixels: usize,
}

#[derive(Debug)]
struct Observation {
    pixels: Rt64PresentedPixels,
    selection: Rt64PresentSelection,
    target_bytes: Vec<u8>,
    pixel_sha256: String,
    rdram_sha256: String,
}

#[derive(Debug)]
struct CopyRegionObservation {
    policy_sha256: [u8; 32],
    source: Vec<u16>,
    sampled: Vec<u16>,
    capture: Rt64PresentedPixels,
    selection: Rt64PresentSelection,
    workload: Rt64DeferredWorkloadEvidence,
    copy_path: Rt64FramebufferCopyPathEvidence,
}

fn write_command(rdram: &mut [u8], offset: usize, word0: u32, word1: u32) {
    rdram[offset..offset + 4].copy_from_slice(&word0.to_ne_bytes());
    rdram[offset + 4..offset + 8].copy_from_slice(&word1.to_ne_bytes());
}

fn fixture() -> (Vec<u8>, u32) {
    let commands = [
        (0xff10_000f, TARGET),
        (0xef30_0000, 0),
        (0xf700_0000, 0x0001_0001),
        (0xf600_0000 | (60 << 12) | 60, 0),
        (0xe700_0000, 0),
        (0xef00_0080, 0),
        (0xfc8f_ff1f, 0x88fc_f279),
        (0xfd10_000f, TEXTURE),
        (0xf510_0000, 7 << 24),
        (0xf300_0000, (7 << 24) | (255 << 12) | 0x200),
        (0xf510_0800, 0x0008_0200),
        (0xf200_0000, (60 << 12) | 60),
        (0xed00_0000, (61 << 12) | 62),
        (0xe400_0000 | (57 << 12) | 58, (5 << 12) | 6),
        (0, 0x04ec_04ec),
        (0xe900_0000, 0),
    ];
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = fn64_runtime::RdramViewMut::from_storage(&mut rdram);
        for y in 0..TEXTURE_SIZE {
            for x in 0..TEXTURE_SIZE {
                let color = if x > y { RED } else { BLUE };
                view.write_u16(
                    fn64_runtime::RdramAddr::from_offset(TEXTURE + (y * TEXTURE_SIZE + x) * 2),
                    color,
                );
            }
        }
    }
    for (index, (word0, word1)) in commands.into_iter().enumerate() {
        write_command(&mut rdram, COMMANDS + index * 8, word0, word1);
    }
    (rdram, (COMMANDS + commands.len() * 8) as u32)
}

fn runtime_settings() -> Result<RenderRuntimeSettings, Box<dyn Error>> {
    Ok(RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        resolution: RenderResolution::Manual,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(1.0)?,
        three_point_filtering: false,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    })
}

fn baseline_emulator() -> RenderEmulatorSettings {
    RenderEmulatorSettings {
        post_blend_noise: false,
        post_blend_noise_negative: false,
        framebuffer_render_to_ram: true,
        framebuffer_copy_with_gpu: true,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn render(backend: &mut Rt64Backend, guest_cycle: u64) -> Result<Observation, Box<dyn Error>> {
    let (mut rdram, end) = fixture();
    let status = backend.process_rdp_commands(&mut rdram, COMMANDS as u32, end, TARGET)?;
    if status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "enhancement/emulator fixture returned {status:?} instead of Complete"
        ))
        .into());
    }
    backend.present_physical_compatibility(
        &rdram,
        ViPresentation {
            noise_seed: guest_cycle,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    let pixels = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    if pixels.width != WIDTH
        || pixels.height != HEIGHT
        || pixels.row_bytes != WIDTH * 4
        || pixels.format != Rt64PresentPixelFormat::Bgra8Unorm
        || selection.present_id != pixels.present_id
        || selection.source_texture_identity == 0
        || selection.target_address != TARGET
        || selection.target_width != WIDTH
        || selection.target_size != 2
    {
        return Err(io::Error::other(format!(
            "invalid presentation/resource evidence: pixels={pixels:?}, selection={selection:?}"
        ))
        .into());
    }
    let start = TARGET as usize;
    let end = start + (WIDTH * HEIGHT * 2) as usize;
    let target_bytes = rdram[start..end].to_vec();
    Ok(Observation {
        pixel_sha256: digest(&pixels.bytes),
        rdram_sha256: digest(&target_bytes),
        pixels,
        selection,
        target_bytes,
    })
}

fn render_profile(
    runtime: &RenderRuntimeSettings,
    enhancement: &RenderEnhancementSettings,
    emulator: &RenderEmulatorSettings,
    guest_cycle: u64,
) -> Result<([u8; 32], Observation), Box<dyn Error>> {
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime.clone());
    let enhancement_result = backend.apply_enhancement_settings(enhancement)?;
    let expected_enhancement = RenderPolicyApply::StagedForCreate {
        policy_sha256: backend.configured_runtime_policy().sha256(),
    };
    if enhancement_result != expected_enhancement {
        return Err(io::Error::other(format!(
            "enhancement profile was not staged exactly: expected={expected_enhancement:?}, actual={enhancement_result:?}"
        ))
        .into());
    }
    let emulator_result = backend.apply_emulator_settings(emulator)?;
    let expected_emulator = RenderPolicyApply::StagedForCreate {
        policy_sha256: backend.configured_runtime_policy().sha256(),
    };
    if emulator_result != expected_emulator {
        return Err(io::Error::other(format!(
            "emulator profile was not staged exactly: expected={expected_emulator:?}, actual={emulator_result:?}"
        ))
        .into());
    }
    backend.create(&RenderConfig::new(WIDTH, HEIGHT))?;
    backend.enable_present_capture()?;
    let policy = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("create did not establish active profile"))?;
    if policy != backend.configured_runtime_policy()
        || policy.enhancement != *enhancement
        || policy.emulator != *emulator
    {
        return Err(io::Error::other(format!(
            "create did not activate the staged profile: active={policy:?}, configured={:?}",
            backend.configured_runtime_policy()
        ))
        .into());
    }
    let policy_sha256 = policy.sha256();
    let observation = render(&mut backend, guest_cycle)?;
    Ok((policy_sha256, observation))
}

fn changed(left: &[u8], right: &[u8], stride: usize) -> usize {
    left.chunks_exact(stride)
        .zip(right.chunks_exact(stride))
        .filter(|(left, right)| left != right)
        .count()
}

fn report(label: &str, policy: &[u8; 32], observation: &Observation, base: &Observation) {
    println!(
        "phase={label} policy_sha256={} present_id={} resource_id={} target={:#010x}/{}x{}/{} pixel_sha256={} rdram_sha256={} changed_pixels={} changed_rdram_pixels={}",
        hex(policy),
        observation.pixels.present_id,
        observation.selection.source_texture_identity,
        observation.selection.target_address,
        observation.selection.target_width,
        observation.selection.target_height,
        observation.selection.target_size,
        observation.pixel_sha256,
        observation.rdram_sha256,
        changed(&base.pixels.bytes, &observation.pixels.bytes, 4),
        changed(&base.target_bytes, &observation.target_bytes, 2),
    );
}

fn validate(
    label: &str,
    policy: &[u8; 32],
    observation: &Observation,
    base: &Observation,
    expected: Expected,
) -> Result<(), Box<dyn Error>> {
    let policy = hex(policy);
    let changed_pixels = changed(&base.pixels.bytes, &observation.pixels.bytes, 4);
    let changed_rdram_pixels = changed(&base.target_bytes, &observation.target_bytes, 2);
    if policy != expected.policy
        || observation.pixel_sha256 != expected.pixels
        || observation.rdram_sha256 != expected.rdram
        || changed_pixels != expected.changed_pixels
        || changed_rdram_pixels != expected.changed_rdram_pixels
    {
        return Err(io::Error::other(format!(
            "{label} exact evidence drifted: policy={policy}, pixels={}, rdram={}, changed_pixels={changed_pixels}, changed_rdram_pixels={changed_rdram_pixels}",
            observation.pixel_sha256, observation.rdram_sha256
        ))
        .into());
    }
    Ok(())
}

fn fill_rect(lrx: u32, lry: u32, ulx: u32, uly: u32) -> (u32, u32) {
    (
        0xf600_0000 | ((lrx * 4) << 12) | (lry * 4),
        ((ulx * 4) << 12) | (uly * 4),
    )
}

fn copy_region_fixture() -> (Vec<u8>, u32, u32) {
    let mut rdram = vec![0; RDRAM_LEN];
    {
        let mut view = RdramViewMut::from_storage(&mut rdram);
        for index in 0..COPY_WIDTH * COPY_SOURCE_HEIGHT {
            view.write_u16(
                RdramAddr::from_offset(COPY_SOURCE + index * 2),
                STALE_SOURCE,
            );
        }
        for index in 0..COPY_WIDTH * COPY_SAMPLED_HEIGHT {
            view.write_u16(
                RdramAddr::from_offset(COPY_SAMPLED + index * 2),
                STALE_SAMPLED,
            );
        }
        for address in [
            COPY_SOURCE - 2,
            COPY_SOURCE + COPY_WIDTH * COPY_SOURCE_HEIGHT * 2,
            COPY_SAMPLED - 2,
            COPY_SAMPLED + COPY_WIDTH * COPY_SAMPLED_HEIGHT * 2,
        ] {
            view.write_u16(RdramAddr::from_offset(address), GUARD);
        }
    }

    let mut commands = vec![
        (0xef30_00f0, 0),
        (0xff10_0007, COPY_SOURCE),
        (0xf700_0000, u32::from(RED) * 0x1_0001),
        fill_rect(COPY_WIDTH - 1, COPY_SOURCE_HEIGHT - 1, 0, 0),
        (0xf700_0000, u32::from(BLUE) * 0x1_0001),
        fill_rect(COPY_WIDTH - 1, COPY_SOURCE_HEIGHT - 1, 0, 2),
        (0xe900_0000, 0),
        (0xfd10_0007, COPY_SOURCE),
        (0xe800_0000, 0),
        (0xf510_0400, 7 << 24),
        (0xe600_0000, 0),
        (0xf400_0000 | 4, (7 << 24) | (28 << 12) | 8),
        (0xf510_0400, 0x0008_0200),
        (0xf200_0000, (28 << 12) | 4),
        (0xfc8f_ff1f, 0x88fc_f279),
        (0xef00_00f0, 0),
        (0xff10_0007, COPY_SAMPLED),
        (
            0xe400_0000 | ((COPY_WIDTH * 4) << 12) | (COPY_SAMPLED_HEIGHT * 4),
            0,
        ),
        (0, 0x0400_0400),
        (0xe900_0000, 0),
    ];
    let sampled_start = COMMANDS + 7 * 8;
    let end = COMMANDS + commands.len() * 8;
    for (index, (word0, word1)) in commands.drain(..).enumerate() {
        write_command(&mut rdram, COMMANDS + index * 8, word0, word1);
    }
    (rdram, sampled_start as u32, end as u32)
}

fn copy_region_pixels(rdram: &[u8], address: u32, count: u32) -> Vec<u16> {
    let view = RdramView::from_storage(rdram);
    (0..count)
        .map(|index| view.read_u16(RdramAddr::from_offset(address + index * 2)))
        .collect()
}

fn digest_u16(values: &[u16]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value.to_be_bytes());
    }
    hex(&hasher.finalize())
}

fn render_copy_region_profile(
    copy_with_gpu: bool,
    guest_cycle: u64,
) -> Result<CopyRegionObservation, Box<dyn Error>> {
    let runtime = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        filtering: RenderFiltering::Nearest,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(COPY_WIDTH as f64 / COPY_SOURCE_HEIGHT as f64)?,
        idle_work_active: false,
        ..RenderRuntimeSettings::default()
    };
    let emulator = RenderEmulatorSettings {
        post_blend_noise: false,
        post_blend_noise_negative: false,
        framebuffer_render_to_ram: true,
        framebuffer_copy_with_gpu: copy_with_gpu,
    };
    let mut backend = Rt64Backend::new().with_runtime_settings(runtime);
    let staged = backend.apply_emulator_settings(&emulator)?;
    let expected_staged = RenderPolicyApply::StagedForCreate {
        policy_sha256: backend.configured_runtime_policy().sha256(),
    };
    if staged != expected_staged {
        return Err(io::Error::other(format!(
            "copy-mode profile was not staged exactly: expected={expected_staged:?}, actual={staged:?}"
        ))
        .into());
    }
    backend.create(&RenderConfig::new(COPY_WIDTH, COPY_SOURCE_HEIGHT))?;
    backend.enable_present_capture()?;
    let policy = backend
        .active_runtime_policy()
        .ok_or_else(|| io::Error::other("copy-mode create did not establish active policy"))?;
    if policy != backend.configured_runtime_policy()
        || policy.emulator.framebuffer_copy_with_gpu != copy_with_gpu
    {
        return Err(io::Error::other(format!(
            "copy-mode create did not activate requested policy: active={policy:?}, requested_copy_with_gpu={copy_with_gpu}"
        ))
        .into());
    }

    let (mut rdram, sampled_start, end) = copy_region_fixture();
    let source_status =
        backend.process_rdp_commands(&mut rdram, COMMANDS as u32, sampled_start, COPY_SOURCE)?;
    if source_status != FrameStatus::Complete {
        return Err(io::Error::other(format!(
            "copy-mode source fixture returned {source_status:?}"
        ))
        .into());
    }
    backend.enable_deferred_workload_capture_for_evidence()?;
    let status = backend.process_rdp_commands(&mut rdram, sampled_start, end, COPY_SAMPLED)?;
    if status != FrameStatus::Complete {
        return Err(
            io::Error::other(format!("copy-mode region fixture returned {status:?}")).into(),
        );
    }
    let workload = backend.deferred_workload_evidence()?;
    let copy_path = backend.framebuffer_copy_path_evidence()?;
    backend.present_physical_compatibility(
        &rdram,
        ViPresentation {
            noise_seed: guest_cycle,
            scanout: fn64_render::ViScanoutState::BackendOnly(ViFilterControl {
                pixel_type: ViPixelType::Rgba16,
                ..ViFilterControl::default()
            }),
            ..ViPresentation::default()
        },
    )?;
    let capture = backend.presented_pixels()?;
    let selection = backend.present_selection()?;
    let source = copy_region_pixels(&rdram, COPY_SOURCE, COPY_WIDTH * COPY_SOURCE_HEIGHT);
    let sampled = copy_region_pixels(&rdram, COPY_SAMPLED, COPY_WIDTH * COPY_SAMPLED_HEIGHT);
    let expected_source: Vec<u16> = (0..COPY_SOURCE_HEIGHT)
        .flat_map(|row| {
            let color = if row < 2 { RED } else { BLUE };
            std::iter::repeat_n(color, COPY_WIDTH as usize)
        })
        .collect();
    let expected_sampled: Vec<u16> = [RED, BLUE]
        .into_iter()
        .flat_map(|color| std::iter::repeat_n(color, COPY_WIDTH as usize))
        .collect();
    let view = RdramView::from_storage(&rdram);
    let guards = [
        COPY_SOURCE - 2,
        COPY_SOURCE + COPY_WIDTH * COPY_SOURCE_HEIGHT * 2,
        COPY_SAMPLED - 2,
        COPY_SAMPLED + COPY_WIDTH * COPY_SAMPLED_HEIGHT * 2,
    ];
    let guards_valid = guards
        .into_iter()
        .all(|address| view.read_u16(RdramAddr::from_offset(address)) == GUARD);
    let current = &workload.current;
    if source != expected_source
        || sampled != expected_sampled
        || sampled.contains(&STALE_SOURCE)
        || !guards_valid
        || current.workload_id == 0
        || copy_path.workload_id != current.workload_id
        || current != &workload.pre_submission
        || current.content_digest != COPY_WORKLOAD_CONTENT
        || current.framebuffer_pair_count != 1
        || current.projection_count != 1
        || current.game_call_count != 1
        || current.triangle_count != 2
        || current.load_operation_count != 1
        || current.pair_color_addresses[0] != COPY_SAMPLED
        || current.pair_game_call_counts[0] != 1
        || current.pair_projection_counts[0] != 1
        || copy_path.source_framebuffer_identity == 0
        || copy_path.source_framebuffer_address != COPY_SOURCE
        || capture.present_id != 1
        || capture.workload_id != current.workload_id
        || selection.present_id != capture.present_id
        || selection.source_texture_identity == 0
        || selection.target_address != COPY_SAMPLED
        || selection.target_width != COPY_WIDTH
        || selection.target_height != COPY_SAMPLED_HEIGHT
        || selection.target_size != 2
    {
        return Err(io::Error::other(format!(
            "copy-mode region evidence mismatch: copy_with_gpu={copy_with_gpu}, source_sha={}, sampled_sha={}, guards_valid={guards_valid}, copy_path={copy_path:?}, capture={capture:?}, selection={selection:?}, workload={workload:#?}",
            digest_u16(&source),
            digest_u16(&sampled)
        ))
        .into());
    }
    Ok(CopyRegionObservation {
        policy_sha256: policy.sha256(),
        source,
        sampled,
        capture,
        selection,
        workload,
        copy_path,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let source = Rt64Backend::release_identity();
    if source.source_provenance != Rt64SourceProvenance::GitClean
        || source.source_id != "git:f0728a2520d5aa735886240de3fee75cc805f6d6"
    {
        return Err(io::Error::other(format!(
            "enhancement/emulator evidence requires the clean pinned RT64 source: {source:?}"
        ))
        .into());
    }

    let runtime = runtime_settings()?;
    let baseline_enhancement = RenderEnhancementSettings::default();
    let baseline_emulator = baseline_emulator();
    let (baseline_policy, baseline) =
        render_profile(&runtime, &baseline_enhancement, &baseline_emulator, 1)?;
    report("baseline", &baseline_policy, &baseline, &baseline);
    validate(
        "baseline",
        &baseline_policy,
        &baseline,
        &baseline,
        Expected {
            policy: BASELINE_POLICY,
            pixels: BASELINE_PIXELS,
            rdram: BASELINE_RDRAM,
            changed_pixels: 0,
            changed_rdram_pixels: 0,
        },
    )?;

    let remove_borders = RenderEnhancementSettings {
        remove_black_borders: true,
        ..baseline_enhancement.clone()
    };
    let (policy, observation) = render_profile(&runtime, &remove_borders, &baseline_emulator, 2)?;
    report("remove_black_borders", &policy, &observation, &baseline);
    validate(
        "remove_black_borders",
        &policy,
        &observation,
        &baseline,
        Expected {
            policy: "0c26d736bfff8d076efcec529b93b2983f955b7118f9fc7218baa90516bf1d3b",
            pixels: "c9584609260dd580d4c043fddc9c1b1328fd74f9522a9c57ca6382231ccc26a4",
            rdram: BASELINE_RDRAM,
            changed_pixels: 60,
            changed_rdram_pixels: 0,
        },
    )?;

    let fix_lower_right = RenderEnhancementSettings {
        rect_fix_lower_right: true,
        ..baseline_enhancement.clone()
    };
    let (policy, observation) = render_profile(&runtime, &fix_lower_right, &baseline_emulator, 3)?;
    report("rect_fix_lower_right", &policy, &observation, &baseline);
    validate(
        "rect_fix_lower_right",
        &policy,
        &observation,
        &baseline,
        Expected {
            policy: "e60c0fa4818d9f8a661400ce7605a25a23be628674736f8beb1895334c67358d",
            pixels: "a9717d101eba5f0c54caa544f841967ac4dffc0937cd3cded75fd5f44cf25ac6",
            rdram: "aa6b72f1dfe348ea53991f382823505ffd76c7247e913e9870909cab828f8928",
            changed_pixels: 38,
            changed_rdram_pixels: 31,
        },
    )?;

    let noise_positive = RenderEmulatorSettings {
        post_blend_noise: true,
        post_blend_noise_negative: false,
        ..baseline_emulator.clone()
    };
    let (policy, observation) =
        render_profile(&runtime, &baseline_enhancement, &noise_positive, 4)?;
    report(
        "post_blend_noise_positive",
        &policy,
        &observation,
        &baseline,
    );
    validate(
        "post_blend_noise_positive",
        &policy,
        &observation,
        &baseline,
        Expected {
            policy: "b421784f28ce4bbef430514aad9c3a88c0c0e04b180e95c3fb1bef1bd3754270",
            pixels: "235c7208cff3d320f87d752e45c41fe998b7b2e47172b071cb388a6e69e7da56",
            rdram: "7ce04e6d36f62eb658bc265d9ba125f54b95b7c4c187ad235a56145fcfbe71ac",
            changed_pixels: 101,
            changed_rdram_pixels: 33,
        },
    )?;

    let noise_negative = RenderEmulatorSettings {
        post_blend_noise: true,
        post_blend_noise_negative: true,
        ..baseline_emulator.clone()
    };
    let (policy, observation) =
        render_profile(&runtime, &baseline_enhancement, &noise_negative, 5)?;
    report(
        "post_blend_noise_negative",
        &policy,
        &observation,
        &baseline,
    );
    validate(
        "post_blend_noise_negative",
        &policy,
        &observation,
        &baseline,
        Expected {
            policy: "50c0d010d066a0e9bd6eed3602d9e050f368d9800abce04874177ee5cc6c986d",
            pixels: "564626fb00b0dd114dded9e3bea21ed5f97cb27d97b89f8b151e7e16bfec8c0e",
            rdram: "12d3f6af640434addab5d212b111b177de4d99e6477851f2556ae07b1494e18d",
            changed_pixels: 110,
            changed_rdram_pixels: 15,
        },
    )?;

    let render_to_ram_off = RenderEmulatorSettings {
        post_blend_noise: false,
        framebuffer_render_to_ram: false,
        ..baseline_emulator.clone()
    };
    let (policy, observation) =
        render_profile(&runtime, &baseline_enhancement, &render_to_ram_off, 6)?;
    report("render_to_ram_off", &policy, &observation, &baseline);
    validate(
        "render_to_ram_off",
        &policy,
        &observation,
        &baseline,
        Expected {
            policy: "e974d0190710eb4ab426c6cc8acfd4a593d30f83911e8654a2c98f18313300d7",
            pixels: "a62d3af830378f1653bdb339d9682a0f87f54d22018c6a1d882dfeab7cd82483",
            rdram: "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
            changed_pixels: 108,
            changed_rdram_pixels: 256,
        },
    )?;

    let gpu_copy = render_copy_region_profile(true, 7)?;
    let cpu_copy = render_copy_region_profile(false, 8)?;
    if hex(&gpu_copy.policy_sha256) != GPU_COPY_POLICY
        || hex(&cpu_copy.policy_sha256) != CPU_COPY_POLICY
        || digest_u16(&cpu_copy.source) != COPY_SOURCE_SHA256
        || digest_u16(&cpu_copy.sampled) != COPY_SAMPLED_SHA256
        || digest(&cpu_copy.capture.bytes) != COPY_POST_VI_SHA256
        || gpu_copy.policy_sha256 == cpu_copy.policy_sha256
        || gpu_copy.source != cpu_copy.source
        || gpu_copy.sampled != cpu_copy.sampled
        || gpu_copy.capture.bytes != cpu_copy.capture.bytes
        || gpu_copy.workload.pre_submission.content_digest
            != cpu_copy.workload.pre_submission.content_digest
        || gpu_copy.copy_path.path != Rt64FramebufferCopyPath::GpuTileCopy
        || gpu_copy.copy_path.source_framebuffer_identity == 0
        || gpu_copy.copy_path.source_framebuffer_address != COPY_SOURCE
        || gpu_copy.copy_path.gpu_create_tile_copy_operation_count != 1
        || gpu_copy.copy_path.gpu_tile_dispatch_count != 1
        || gpu_copy.copy_path.cpu_rdram_tmem_upload_count != 0
        || gpu_copy.copy_path.raw_tmem_tile_count != 0
        || gpu_copy.copy_path.sync_framebuffer_pair_count != 1
        || cpu_copy.copy_path.path != Rt64FramebufferCopyPath::CpuRdramTmemUpload
        || cpu_copy.copy_path.source_framebuffer_identity == 0
        || cpu_copy.copy_path.source_framebuffer_address != COPY_SOURCE
        || cpu_copy.copy_path.gpu_create_tile_copy_operation_count != 0
        || cpu_copy.copy_path.gpu_tile_dispatch_count != 0
        || cpu_copy.copy_path.cpu_rdram_tmem_upload_count != 1
        || cpu_copy.copy_path.raw_tmem_tile_count != 0
        || cpu_copy.copy_path.sync_framebuffer_pair_count != 0
    {
        return Err(io::Error::other(format!(
            "copy-mode profiles did not select distinct policies over the same exact region-copy behavior: gpu={gpu_copy:#?}, cpu={cpu_copy:#?}"
        ))
        .into());
    }
    println!(
        "phase=copy_with_gpu_off classification=mechanism_causal gpu_path={:?} gpu_source_fb_id={} gpu_operations={} gpu_tiles={} gpu_ordinary_uploads={} gpu_raw_tmem_tiles={} gpu_sync_pairs={} cpu_path={:?} cpu_source_fb_id={} cpu_ordinary_uploads={} cpu_raw_tmem_tiles={} cpu_sync_pairs={} gpu_policy_sha256={} cpu_policy_sha256={} source_sha256={} sampled_sha256={} post_vi_sha256={} workload_content={:#018x} present_id={} workload_id={} resource_id={} target={:#010x}/{}x{}/{}",
        gpu_copy.copy_path.path,
        gpu_copy.copy_path.source_framebuffer_identity,
        gpu_copy.copy_path.gpu_create_tile_copy_operation_count,
        gpu_copy.copy_path.gpu_tile_dispatch_count,
        gpu_copy.copy_path.cpu_rdram_tmem_upload_count,
        gpu_copy.copy_path.raw_tmem_tile_count,
        gpu_copy.copy_path.sync_framebuffer_pair_count,
        cpu_copy.copy_path.path,
        cpu_copy.copy_path.source_framebuffer_identity,
        cpu_copy.copy_path.cpu_rdram_tmem_upload_count,
        cpu_copy.copy_path.raw_tmem_tile_count,
        cpu_copy.copy_path.sync_framebuffer_pair_count,
        hex(&gpu_copy.policy_sha256),
        hex(&cpu_copy.policy_sha256),
        digest_u16(&cpu_copy.source),
        digest_u16(&cpu_copy.sampled),
        digest(&cpu_copy.capture.bytes),
        cpu_copy.workload.current.content_digest,
        cpu_copy.capture.present_id,
        cpu_copy.capture.workload_id,
        cpu_copy.selection.source_texture_identity,
        cpu_copy.selection.target_address,
        cpu_copy.selection.target_width,
        cpu_copy.selection.target_height,
        cpu_copy.selection.target_size,
    );

    println!(
        "rt64 enhancement/emulator controls: exact causal evidence pass; copy_with_gpu_off=mechanism_causal"
    );

    Ok(())
}
