#![cfg(feature = "rt64")]

use fn64_render::{
    AspectTarget, DownsampleMultiplier, OsTask, RefreshRateTarget, RenderAntialiasing,
    RenderAspectRatio, RenderConfig, RenderDisplayBuffering, RenderEmulatorSettings,
    RenderEnhancementSettings, RenderFiltering, RenderGraphicsApi, RenderHardwareResolve,
    RenderInternalColorFormat, RenderPresentationMode, RenderRefreshRate, RenderResolution,
    RenderRuntimeSettings, RenderUpscale2d, ResolutionMultiplier, ViPresentation,
    ViScanoutRegisters, ViScanoutState,
};
use fn64_render_rt64::{
    capture_rt64_adapter_inputs, roundtrip_rt64_emulator_settings,
    roundtrip_rt64_enhancement_settings, roundtrip_rt64_runtime_settings,
};

fn fixture() -> (OsTask, RenderConfig, ViPresentation) {
    let mut vi_registers = [0u32; ViScanoutRegisters::WORD_COUNT];
    vi_registers[0] = 0x0001_001e;
    vi_registers[1] = 0x0010_0000;
    vi_registers[2] = 320;
    vi_registers[3] = 2;
    vi_registers[4] = 0;
    vi_registers[5] = 0x03e5_2239;
    vi_registers[6] = 525;
    vi_registers[7] = 3093;
    vi_registers[8] = 0x0c15_0c15;
    vi_registers[9] = 0x006c_02ec;
    vi_registers[10] = 0x0025_01ff;
    vi_registers[11] = 0x000e_0204;
    vi_registers[12] = 0x0000_0200;
    vi_registers[13] = 0x0000_0400;
    (
        OsTask {
            task_type: 1,
            flags: 2,
            ucode_boot: 3,
            ucode_boot_size: 4,
            ucode: 5,
            ucode_size: 6,
            ucode_data: 7,
            ucode_data_size: 8,
            dram_stack: 9,
            dram_stack_size: 10,
            output_buff: 11,
            output_buff_size: 12,
            data_ptr: 13,
            data_size: 14,
        },
        RenderConfig::ntsc(640, 480),
        ViPresentation {
            scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(vi_registers)),
            ..ViPresentation::default()
        },
    )
}

#[test]
fn typed_task_and_vi_cross_cpp_boundary_without_a_graphics_device() {
    let (task, cfg, vi) = fixture();
    let first = capture_rt64_adapter_inputs(&task, 0xab12_3456, cfg, vi).unwrap();
    let second = capture_rt64_adapter_inputs(&task, 0xab12_3456, cfg, vi).unwrap();

    assert_eq!(first, second);
    assert_eq!(first.sha256(), second.sha256());
    assert_eq!(
        first.task_words,
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
    );
    assert_eq!(first.output_addr, 0x0012_3456);
    assert_eq!((first.width, first.height), (640, 480));
    assert!(first.aa_mode_specified);
    assert_eq!(first.vi_filter_flags, (1 << 0) | (1 << 1) | (1 << 2));
    assert_eq!(first.noise_seed, 0);

    let mut expected_registers = [0; 24];
    expected_registers[9] = 0x0001_001e;
    // The shim preserves the guest origin because pinned RT64 owns the
    // leading-row normalization used to match VI scanout to managed targets.
    expected_registers[10] = 0x0010_0000;
    expected_registers[11] = 320;
    expected_registers[12] = 2;
    expected_registers[14] = 0x03e5_2239;
    expected_registers[15] = 525;
    expected_registers[16] = 3093;
    expected_registers[17] = 0x0c15_0c15;
    expected_registers[18] = 0x006c_02ec;
    expected_registers[19] = 0x0025_01ff;
    expected_registers[20] = 0x000e_0204;
    expected_registers[21] = 0x200;
    expected_registers[22] = 0x400;
    assert_eq!(first.registers, expected_registers);
    assert_eq!(first.registers_after_submission, expected_registers);
}

#[test]
fn cpp_capture_distinguishes_compatibility_unspecified_from_explicit_mode_zero() {
    let task = OsTask::default();
    let cfg = RenderConfig::ntsc(640, 480);
    let compatibility =
        capture_rt64_adapter_inputs(&task, 0, cfg, ViPresentation::default()).unwrap();

    let mut words = [0; ViScanoutRegisters::WORD_COUNT];
    words[0] = 2;
    // Compatibility output address zero names the color-image base; the
    // equivalent live VI register convention points one RGBA16 row later.
    words[1] = cfg.width * 2;
    words[2] = cfg.width;
    words[6] = 525;
    words[7] = 3093;
    words[9] = (108 << 16) | 748;
    words[10] = (34 << 16) | (34 + cfg.height * 2);
    words[12] = 0x400;
    words[13] = 0x400;
    let explicit = capture_rt64_adapter_inputs(
        &task,
        0,
        cfg,
        ViPresentation {
            scanout: ViScanoutState::Registers(ViScanoutRegisters::from_words(words)),
            ..ViPresentation::default()
        },
    )
    .unwrap();

    assert_eq!(compatibility.registers, explicit.registers);
    assert!(!compatibility.aa_mode_specified);
    assert!(explicit.aa_mode_specified);
    assert_eq!(compatibility.vi_filter_flags, 1 << 2);
    assert_eq!(explicit.vi_filter_flags, (1 << 1) | (1 << 2));
    assert_ne!(compatibility.sha256(), explicit.sha256());
}

#[test]
fn capture_rejects_vi_state_the_live_boundary_rejects() {
    let (task, cfg, mut vi) = fixture();
    vi.fade = Some(0x155);
    vi.repeat_line = true;
    let error = capture_rt64_adapter_inputs(&task, 0, cfg, vi).unwrap_err();
    assert!(
        error.to_string().contains("fade and repeat-line"),
        "unexpected adapter diagnostic: {error}"
    );
}

#[test]
fn capture_rejects_rgba32_dither_restoration_like_live_presentation() {
    let (task, cfg, mut vi) = fixture();
    let mut words = vi.scanout.registers().unwrap().words();
    words[0] = (words[0] & !3) | 3;
    vi.scanout = ViScanoutState::Registers(ViScanoutRegisters::from_words(words));
    let error = capture_rt64_adapter_inputs(&task, 0, cfg, vi).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("VI dither restoration requires an RGBA16 scanout image"),
        "unexpected adapter diagnostic: {error}"
    );
}

#[test]
fn odd_interlaced_origin_is_not_precompensated_a_second_time() {
    let (task, cfg, mut vi) = fixture();
    let mut words = vi.scanout.registers().unwrap().words();
    words[0] |= 1 << 6;
    words[2] = 0x1140;
    words[4] = 1;
    vi.scanout = ViScanoutState::Registers(ViScanoutRegisters::from_words(words));

    let capture = capture_rt64_adapter_inputs(&task, 0, cfg, vi).unwrap();
    assert_eq!(capture.registers[11], 0x1140);
    assert_eq!(capture.registers[10], 0x0010_0000);
    assert_eq!(capture.registers_after_submission, capture.registers);
}

#[test]
fn complete_user_configuration_round_trips_without_a_graphics_device() {
    let settings = RenderRuntimeSettings {
        graphics_api: RenderGraphicsApi::Metal,
        resolution: RenderResolution::Manual,
        display_buffering: RenderDisplayBuffering::Triple,
        antialiasing: RenderAntialiasing::Msaa8x,
        resolution_multiplier: ResolutionMultiplier::new(32.0).unwrap(),
        downsample_multiplier: DownsampleMultiplier::new(32).unwrap(),
        filtering: RenderFiltering::Linear,
        aspect_ratio: RenderAspectRatio::Manual,
        aspect_target: AspectTarget::new(100.0).unwrap(),
        extended_aspect_ratio: RenderAspectRatio::Expand,
        extended_aspect_target: AspectTarget::new(0.1).unwrap(),
        upscale_2d: RenderUpscale2d::All,
        three_point_filtering: false,
        refresh_rate: RenderRefreshRate::Manual,
        refresh_rate_target: RefreshRateTarget::new(1000).unwrap(),
        internal_color_format: RenderInternalColorFormat::High,
        hardware_resolve: RenderHardwareResolve::Enabled,
        idle_work_active: false,
        developer_mode: true,
    };
    assert_eq!(
        roundtrip_rt64_runtime_settings(&settings).unwrap(),
        settings
    );
    assert_eq!(
        roundtrip_rt64_runtime_settings(&RenderRuntimeSettings::upstream_default()).unwrap(),
        RenderRuntimeSettings::upstream_default()
    );
}

#[test]
fn complete_enhancement_and_emulator_configuration_round_trip_without_a_device() {
    for presentation_mode in [
        RenderPresentationMode::Console,
        RenderPresentationMode::SkipBuffering,
        RenderPresentationMode::PresentEarly,
    ] {
        let settings = RenderEnhancementSettings {
            framebuffer_reinterpret_fix_uls: true,
            presentation_mode,
            remove_black_borders: true,
            rect_fix_lower_right: true,
            f3dex_force_branch: true,
            s2dex_fix_bilerp_mismatch: true,
            s2dex_framebuffer_fast_path: true,
            texture_lod_scale: true,
        };
        assert_eq!(
            roundtrip_rt64_enhancement_settings(&settings).unwrap(),
            settings
        );
    }
    assert_eq!(
        roundtrip_rt64_enhancement_settings(&RenderEnhancementSettings::default()).unwrap(),
        RenderEnhancementSettings::default()
    );
    assert_eq!(
        roundtrip_rt64_enhancement_settings(&RenderEnhancementSettings::upstream_default())
            .unwrap(),
        RenderEnhancementSettings::upstream_default()
    );

    for bits in 0..16u32 {
        let settings = RenderEmulatorSettings {
            post_blend_noise: bits & 1 != 0,
            post_blend_noise_negative: bits & 2 != 0,
            framebuffer_render_to_ram: bits & 4 != 0,
            framebuffer_copy_with_gpu: bits & 8 != 0,
        };
        assert_eq!(
            roundtrip_rt64_emulator_settings(&settings).unwrap(),
            settings
        );
    }
}
