//! Raw C-ABI config wire types and their conversions.
use super::*;

#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub(super) struct RawUserConfig {
    graphics_api: u32,
    resolution: u32,
    display_buffering: u32,
    antialiasing: u32,
    resolution_multiplier: f64,
    downsample_multiplier: u32,
    filtering: u32,
    aspect_ratio: u32,
    aspect_target: f64,
    extended_aspect_ratio: u32,
    extended_aspect_target: f64,
    upscale_2d: u32,
    three_point_filtering: u32,
    refresh_rate: u32,
    refresh_rate_target: u32,
    internal_color_format: u32,
    hardware_resolve: u32,
    idle_work_active: u32,
    developer_mode: u32,
}

const _: [(); 96] = [(); std::mem::size_of::<RawUserConfig>()];

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(super) struct RawEnhancementConfig {
    framebuffer_reinterpret_fix_uls: u32,
    presentation_mode: u32,
    remove_black_borders: u32,
    rect_fix_lower_right: u32,
    f3dex_force_branch: u32,
    s2dex_fix_bilerp_mismatch: u32,
    s2dex_framebuffer_fast_path: u32,
    texture_lod_scale: u32,
}

const _: [(); 32] = [(); std::mem::size_of::<RawEnhancementConfig>()];

impl From<&RenderEnhancementSettings> for RawEnhancementConfig {
    fn from(settings: &RenderEnhancementSettings) -> Self {
        Self {
            framebuffer_reinterpret_fix_uls: u32::from(settings.framebuffer_reinterpret_fix_uls),
            presentation_mode: match settings.presentation_mode {
                RenderPresentationMode::Console => 0,
                RenderPresentationMode::SkipBuffering => 1,
                RenderPresentationMode::PresentEarly => 2,
            },
            remove_black_borders: u32::from(settings.remove_black_borders),
            rect_fix_lower_right: u32::from(settings.rect_fix_lower_right),
            f3dex_force_branch: u32::from(settings.f3dex_force_branch),
            s2dex_fix_bilerp_mismatch: u32::from(settings.s2dex_fix_bilerp_mismatch),
            s2dex_framebuffer_fast_path: u32::from(settings.s2dex_framebuffer_fast_path),
            texture_lod_scale: u32::from(settings.texture_lod_scale),
        }
    }
}

impl TryFrom<RawEnhancementConfig> for RenderEnhancementSettings {
    type Error = String;

    fn try_from(raw: RawEnhancementConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            framebuffer_reinterpret_fix_uls: decode_raw_bool(
                raw.framebuffer_reinterpret_fix_uls,
                "framebuffer_reinterpret_fix_uls",
            )?,
            presentation_mode: match raw.presentation_mode {
                0 => RenderPresentationMode::Console,
                1 => RenderPresentationMode::SkipBuffering,
                2 => RenderPresentationMode::PresentEarly,
                value => {
                    return Err(format!(
                        "C++ returned invalid presentation_mode tag {value}"
                    ));
                }
            },
            remove_black_borders: decode_raw_bool(
                raw.remove_black_borders,
                "remove_black_borders",
            )?,
            rect_fix_lower_right: decode_raw_bool(
                raw.rect_fix_lower_right,
                "rect_fix_lower_right",
            )?,
            f3dex_force_branch: decode_raw_bool(raw.f3dex_force_branch, "f3dex_force_branch")?,
            s2dex_fix_bilerp_mismatch: decode_raw_bool(
                raw.s2dex_fix_bilerp_mismatch,
                "s2dex_fix_bilerp_mismatch",
            )?,
            s2dex_framebuffer_fast_path: decode_raw_bool(
                raw.s2dex_framebuffer_fast_path,
                "s2dex_framebuffer_fast_path",
            )?,
            texture_lod_scale: decode_raw_bool(raw.texture_lod_scale, "texture_lod_scale")?,
        })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub(super) struct RawEmulatorConfig {
    post_blend_noise: u32,
    post_blend_noise_negative: u32,
    framebuffer_render_to_ram: u32,
    framebuffer_copy_with_gpu: u32,
}

const _: [(); 16] = [(); std::mem::size_of::<RawEmulatorConfig>()];

impl From<&RenderEmulatorSettings> for RawEmulatorConfig {
    fn from(settings: &RenderEmulatorSettings) -> Self {
        Self {
            post_blend_noise: u32::from(settings.post_blend_noise),
            post_blend_noise_negative: u32::from(settings.post_blend_noise_negative),
            framebuffer_render_to_ram: u32::from(settings.framebuffer_render_to_ram),
            framebuffer_copy_with_gpu: u32::from(settings.framebuffer_copy_with_gpu),
        }
    }
}

impl TryFrom<RawEmulatorConfig> for RenderEmulatorSettings {
    type Error = String;

    fn try_from(raw: RawEmulatorConfig) -> Result<Self, Self::Error> {
        Ok(Self {
            post_blend_noise: decode_raw_bool(raw.post_blend_noise, "post_blend_noise")?,
            post_blend_noise_negative: decode_raw_bool(
                raw.post_blend_noise_negative,
                "post_blend_noise_negative",
            )?,
            framebuffer_render_to_ram: decode_raw_bool(
                raw.framebuffer_render_to_ram,
                "framebuffer_render_to_ram",
            )?,
            framebuffer_copy_with_gpu: decode_raw_bool(
                raw.framebuffer_copy_with_gpu,
                "framebuffer_copy_with_gpu",
            )?,
        })
    }
}

pub(super) fn decode_raw_bool(value: u32, field: &str) -> Result<bool, String> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!("C++ returned invalid {field} boolean {value}")),
    }
}

impl From<&RenderRuntimeSettings> for RawUserConfig {
    fn from(settings: &RenderRuntimeSettings) -> Self {
        Self {
            graphics_api: match settings.graphics_api {
                RenderGraphicsApi::D3d12 => 0,
                RenderGraphicsApi::Vulkan => 1,
                RenderGraphicsApi::Metal => 2,
                RenderGraphicsApi::Automatic => 3,
            },
            resolution: match settings.resolution {
                RenderResolution::Original => 0,
                RenderResolution::WindowIntegerScale => 1,
                RenderResolution::Manual => 2,
            },
            display_buffering: match settings.display_buffering {
                RenderDisplayBuffering::Double => 0,
                RenderDisplayBuffering::Triple => 1,
            },
            antialiasing: match settings.antialiasing {
                RenderAntialiasing::None => 0,
                RenderAntialiasing::Msaa2x => 1,
                RenderAntialiasing::Msaa4x => 2,
                RenderAntialiasing::Msaa8x => 3,
            },
            resolution_multiplier: settings.resolution_multiplier.get(),
            downsample_multiplier: u32::from(settings.downsample_multiplier.get()),
            filtering: match settings.filtering {
                RenderFiltering::Nearest => 0,
                RenderFiltering::Linear => 1,
                RenderFiltering::AntiAliasedPixelScaling => 2,
            },
            aspect_ratio: aspect_tag(settings.aspect_ratio),
            aspect_target: settings.aspect_target.get(),
            extended_aspect_ratio: aspect_tag(settings.extended_aspect_ratio),
            extended_aspect_target: settings.extended_aspect_target.get(),
            upscale_2d: match settings.upscale_2d {
                RenderUpscale2d::Original => 0,
                RenderUpscale2d::ScaledOnly => 1,
                RenderUpscale2d::All => 2,
            },
            three_point_filtering: u32::from(settings.three_point_filtering),
            refresh_rate: match settings.refresh_rate {
                RenderRefreshRate::Original => 0,
                RenderRefreshRate::Display => 1,
                RenderRefreshRate::Manual => 2,
            },
            refresh_rate_target: u32::from(settings.refresh_rate_target.get()),
            internal_color_format: match settings.internal_color_format {
                RenderInternalColorFormat::Standard => 0,
                RenderInternalColorFormat::High => 1,
                RenderInternalColorFormat::Automatic => 2,
            },
            hardware_resolve: match settings.hardware_resolve {
                RenderHardwareResolve::Disabled => 0,
                RenderHardwareResolve::Enabled => 1,
                RenderHardwareResolve::Automatic => 2,
            },
            idle_work_active: u32::from(settings.idle_work_active),
            developer_mode: u32::from(settings.developer_mode),
        }
    }
}

pub(super) fn aspect_tag(value: RenderAspectRatio) -> u32 {
    match value {
        RenderAspectRatio::Original => 0,
        RenderAspectRatio::Expand => 1,
        RenderAspectRatio::Manual => 2,
    }
}

pub(super) fn decode_aspect(value: u32, field: &str) -> Result<RenderAspectRatio, String> {
    match value {
        0 => Ok(RenderAspectRatio::Original),
        1 => Ok(RenderAspectRatio::Expand),
        2 => Ok(RenderAspectRatio::Manual),
        _ => Err(format!("C++ returned invalid {field} tag {value}")),
    }
}

impl TryFrom<RawUserConfig> for RenderRuntimeSettings {
    type Error = String;

    fn try_from(raw: RawUserConfig) -> Result<Self, Self::Error> {
        let boolean = |value, field| match value {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(format!("C++ returned invalid {field} boolean {value}")),
        };
        Ok(Self {
            graphics_api: match raw.graphics_api {
                0 => RenderGraphicsApi::D3d12,
                1 => RenderGraphicsApi::Vulkan,
                2 => RenderGraphicsApi::Metal,
                3 => RenderGraphicsApi::Automatic,
                value => return Err(format!("C++ returned invalid graphics_api tag {value}")),
            },
            resolution: match raw.resolution {
                0 => RenderResolution::Original,
                1 => RenderResolution::WindowIntegerScale,
                2 => RenderResolution::Manual,
                value => return Err(format!("C++ returned invalid resolution tag {value}")),
            },
            display_buffering: match raw.display_buffering {
                0 => RenderDisplayBuffering::Double,
                1 => RenderDisplayBuffering::Triple,
                value => {
                    return Err(format!(
                        "C++ returned invalid display_buffering tag {value}"
                    ));
                }
            },
            antialiasing: match raw.antialiasing {
                0 => RenderAntialiasing::None,
                1 => RenderAntialiasing::Msaa2x,
                2 => RenderAntialiasing::Msaa4x,
                3 => RenderAntialiasing::Msaa8x,
                value => return Err(format!("C++ returned invalid antialiasing tag {value}")),
            },
            resolution_multiplier: ResolutionMultiplier::new(raw.resolution_multiplier)
                .map_err(|error| error.to_string())?,
            downsample_multiplier: DownsampleMultiplier::new(raw.downsample_multiplier)
                .map_err(|error| error.to_string())?,
            filtering: match raw.filtering {
                0 => RenderFiltering::Nearest,
                1 => RenderFiltering::Linear,
                2 => RenderFiltering::AntiAliasedPixelScaling,
                value => return Err(format!("C++ returned invalid filtering tag {value}")),
            },
            aspect_ratio: decode_aspect(raw.aspect_ratio, "aspect_ratio")?,
            aspect_target: AspectTarget::new(raw.aspect_target)
                .map_err(|error| error.to_string())?,
            extended_aspect_ratio: decode_aspect(
                raw.extended_aspect_ratio,
                "extended_aspect_ratio",
            )?,
            extended_aspect_target: AspectTarget::new(raw.extended_aspect_target)
                .map_err(|error| error.to_string())?,
            upscale_2d: match raw.upscale_2d {
                0 => RenderUpscale2d::Original,
                1 => RenderUpscale2d::ScaledOnly,
                2 => RenderUpscale2d::All,
                value => return Err(format!("C++ returned invalid upscale_2d tag {value}")),
            },
            three_point_filtering: boolean(raw.three_point_filtering, "three_point_filtering")?,
            refresh_rate: match raw.refresh_rate {
                0 => RenderRefreshRate::Original,
                1 => RenderRefreshRate::Display,
                2 => RenderRefreshRate::Manual,
                value => return Err(format!("C++ returned invalid refresh_rate tag {value}")),
            },
            refresh_rate_target: RefreshRateTarget::new(raw.refresh_rate_target)
                .map_err(|error| error.to_string())?,
            internal_color_format: match raw.internal_color_format {
                0 => RenderInternalColorFormat::Standard,
                1 => RenderInternalColorFormat::High,
                2 => RenderInternalColorFormat::Automatic,
                value => {
                    return Err(format!(
                        "C++ returned invalid internal_color_format tag {value}"
                    ));
                }
            },
            hardware_resolve: match raw.hardware_resolve {
                0 => RenderHardwareResolve::Disabled,
                1 => RenderHardwareResolve::Enabled,
                2 => RenderHardwareResolve::Automatic,
                value => return Err(format!("C++ returned invalid hardware_resolve tag {value}")),
            },
            idle_work_active: boolean(raw.idle_work_active, "idle_work_active")?,
            developer_mode: boolean(raw.developer_mode, "developer_mode")?,
        })
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
pub(super) struct RawVi {
    registers: [u32; 14],
    registers_present: u8,
    blanked: u8,
    fade_enabled: u8,
    repeat_line: u8,
    fade_factor: u16,
    aa_mode_specified: u8,
    reserved: u8,
    noise_seed: u64,
}

const _: [(); 72] = [(); std::mem::size_of::<RawVi>()];

pub(super) fn raw_vi(vi: ViPresentation) -> Result<RawVi, String> {
    let filters = vi.scanout.filters();
    let pixel_type = match filters.pixel_type {
        ViPixelType::Unspecified | ViPixelType::Rgba16 => 2u32,
        ViPixelType::Rgba32 => 3u32,
        ViPixelType::Blank => 0u32,
        ViPixelType::Reserved => return Err("VI STATUS selects reserved pixel type 1".into()),
    };
    let (registers_present, mut registers) = match vi.scanout {
        ViScanoutState::BackendOnly(_) => (0, [0; 14]),
        ViScanoutState::Registers(registers) => (1, registers.words()),
    };
    if registers_present == 0 {
        registers[0] = pixel_type
            | filters.antialias_mode.status_bits().unwrap_or(0)
            | (u32::from(filters.gamma_dither) << 2)
            | (u32::from(filters.gamma) << 3)
            | (u32::from(filters.divot) << 4)
            | (u32::from(filters.dither_filter) << 16);
    }
    Ok(RawVi {
        registers,
        registers_present,
        blanked: u8::from(vi.blanked),
        fade_enabled: u8::from(vi.fade.is_some()),
        repeat_line: u8::from(vi.repeat_line),
        fade_factor: vi.fade.unwrap_or(0),
        aa_mode_specified: u8::from(filters.antialias_mode.status_bits().is_some()),
        reserved: 0,
        noise_seed: vi.noise_seed,
    })
}

pub(super) fn validate_native_vi_filters(vi: &ViPresentation) -> Result<(), String> {
    let filters = vi.scanout.filters();
    if filters.dither_filter && filters.pixel_type == ViPixelType::Rgba32 {
        return Err("VI dither restoration requires an RGBA16 scanout image".into());
    }
    Ok(())
}

unsafe extern "C" {
    fn fn64_rt64_roundtrip_user_config(
        input: *const RawUserConfig,
        output: *mut RawUserConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_roundtrip_enhancement_config(
        input: *const RawEnhancementConfig,
        output: *mut RawEnhancementConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_roundtrip_emulator_config(
        input: *const RawEmulatorConfig,
        output: *mut RawEmulatorConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_inspect_replacement_pack(
        path_utf8: *const c_char,
        config: *mut RawReplacementDatabaseConfig,
        database_bytes: *mut u8,
        database_capacity: usize,
        database_size: *mut usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_capture_adapter_inputs(
        task: *const RawTask,
        output_addr: u32,
        width: u32,
        height: u32,
        vi: *const RawVi,
        capture: *mut RawAdapterCapture,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(test)]
    fn fn64_rt64_probe_logical_rate(
        nominal_refresh_rate: u32,
        factor: u32,
        logical_rate: *mut u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_create(
        width: u32,
        height: u32,
        nominal_refresh_rate: u32,
        user_config: *const RawUserConfig,
        enhancement_config: *const RawEnhancementConfig,
        emulator_config: *const RawEmulatorConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> *mut RawContext;
    fn fn64_rt64_apply_user_config(
        context: *mut RawContext,
        user_config: *const RawUserConfig,
        framebuffers_discarded: *mut u8,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_apply_enhancement_config(
        context: *mut RawContext,
        enhancement_config: *const RawEnhancementConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_apply_emulator_config(
        context: *mut RawContext,
        emulator_config: *const RawEmulatorConfig,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_load_replacement_packs(
        context: *mut RawContext,
        packs: *const RawReplacementPack,
        pack_count: usize,
        enabled: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_reload_replacement_packs(
        context: *mut RawContext,
        packs: *const RawReplacementPack,
        pack_count: usize,
        enabled: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_set_replacement_enabled(
        context: *mut RawContext,
        enabled: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_wait_texture_replacement_state(
        context: *mut RawContext,
        texture_hash: u64,
        require_replacement: u32,
        state: *mut RawTextureReplacementState,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_set_stream_workers_paused(
        context: *mut RawContext,
        paused: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_wait_stream_fallback_state(
        context: *mut RawContext,
        texture_hash: u64,
        state: *mut RawTextureReplacementState,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_process_task(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        dmem: *mut u8,
        dmem_len: usize,
        imem: *mut u8,
        imem_len: usize,
        task: *const RawTask,
        output_addr: u32,
        ucode_plan: *const RawUcodePlan,
        result: *mut RawTaskResult,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_process_rdp_commands(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        start: u32,
        end: u32,
        output_addr: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_present(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        vi: *const RawVi,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_present_capture(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_present_capture(
        context: *mut RawContext,
        capture: *mut RawPresentCapture,
        bytes: *mut u8,
        bytes_capacity: usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_present_selection(
        context: *mut RawContext,
        selection: *mut RawPresentSelection,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_deferred_workload_capture(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_deferred_workload_evidence(
        context: *mut RawContext,
        evidence: *mut RawDeferredWorkloadEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_framebuffer_copy_path_evidence(
        context: *mut RawContext,
        evidence: *mut RawFramebufferCopyPathEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_s2dex_fast_path_evidence(
        context: *mut RawContext,
        evidence: *mut RawS2dexFastPathEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_extended_gbi_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_extended_gbi_evidence(
        context: *mut RawContext,
        evidence: *mut RawExtendedGbiEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_extended_present_capture(
        context: *mut RawContext,
        capture_index: u32,
        capture: *mut RawExtendedPresentCapture,
        bytes: *mut u8,
        bytes_capacity: usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_enable_hfr_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "synthetic-f3dex2-evidence")]
    fn fn64_rt64_process_synthetic_hfr_f3dex2(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        display_list: u32,
        output_addr: u32,
        original_refresh_rate: u16,
        region_rate_evidence: *mut RawRegionRateEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "synthetic-s2dex-evidence")]
    fn fn64_rt64_process_synthetic_s2dex2(
        context: *mut RawContext,
        rdram: *mut u8,
        rdram_len: usize,
        display_list: u32,
        output_addr: u32,
        legacy_wire: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_read_hfr_evidence(
        context: *mut RawContext,
        evidence: *mut RawHfrEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_read_hfr_present_capture(
        context: *mut RawContext,
        capture_index: u32,
        capture: *mut RawExtendedPresentCapture,
        bytes: *mut u8,
        bytes_capacity: usize,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_enable_hfr_pacing_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    #[cfg(feature = "hfr-evidence")]
    fn fn64_rt64_read_hfr_pacing_evidence(
        context: *mut RawContext,
        evidence: *mut RawHfrPacingEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_set_debugger_inspection_for_evidence(
        context: *mut RawContext,
        paused: u32,
        framebuffer_index: i32,
        draw_call_index: i32,
        framebuffer_depth: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_enable_ubershader_evidence(
        context: *mut RawContext,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_read_ubershader_evidence(
        context: *mut RawContext,
        evidence: *mut RawUbershaderEvidence,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_resize(
        context: *mut RawContext,
        width: u32,
        height: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> c_int;
    fn fn64_rt64_destroy(context: *mut RawContext);
}

pub(crate) fn capture_adapter_inputs(
    task: &OsTask,
    output_addr: u32,
    width: u32,
    height: u32,
    vi: ViPresentation,
) -> Result<crate::Rt64AdapterCapture, String> {
    validate_native_vi_filters(&vi)?;
    let raw_task = RawTask::from(task);
    let vi = raw_vi(vi)?;
    let mut capture = RawAdapterCapture::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: both repr(C) values are live for the synchronous call and the
    // output/error pointers are writable for their advertised full sizes.
    // This entry performs scalar marshalling only and creates no RT64 device.
    let ok = unsafe {
        fn64_rt64_capture_adapter_inputs(
            &raw_task,
            output_addr,
            width,
            height,
            &vi,
            &mut capture,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 adapter capture failed without a diagnostic",
        ));
    }
    if capture.aa_mode_specified > 1 {
        return Err("RT64 adapter capture returned an invalid AA-selector marker".into());
    }
    let status = capture.registers[9];
    let rgba16 = (status & 3) == 2;
    let expected_filter_flags = u32::from((status & (1 << 16)) != 0 && rgba16)
        | (u32::from(capture.aa_mode_specified != 0 && ((status >> 8) & 3) <= 1) << 1)
        | (u32::from(rgba16) << 2)
        | (u32::from((status & (1 << 6)) != 0) << 3);
    if capture.vi_filter_flags != expected_filter_flags {
        return Err("RT64 adapter capture returned inconsistent VI filter flags".into());
    }
    Ok(crate::Rt64AdapterCapture {
        task_words: capture.task.words(),
        output_addr: capture.output_addr,
        width: capture.width,
        height: capture.height,
        aa_mode_specified: capture.aa_mode_specified != 0,
        vi_filter_flags: capture.vi_filter_flags,
        noise_seed: u64::from(capture.noise_seed_low) | (u64::from(capture.noise_seed_high) << 32),
        registers: capture.registers,
        registers_after_submission: capture.registers_after_submission,
    })
}

pub(crate) fn roundtrip_user_config(
    settings: &RenderRuntimeSettings,
) -> Result<RenderRuntimeSettings, String> {
    let input = RawUserConfig::from(settings);
    let mut output = RawUserConfig::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: both repr(C) settings values and the writable error buffer are
    // live for the synchronous scalar-only call. This entry creates no RT64
    // device and retains no pointer.
    let ok = unsafe {
        fn64_rt64_roundtrip_user_config(&input, &mut output, error.as_mut_ptr(), error.len())
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 user-config roundtrip failed without a diagnostic",
        ));
    }
    RenderRuntimeSettings::try_from(output)
}

pub(crate) fn roundtrip_enhancement_config(
    settings: &RenderEnhancementSettings,
) -> Result<RenderEnhancementSettings, String> {
    let input = RawEnhancementConfig::from(settings);
    let mut output = RawEnhancementConfig::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: scalar repr(C) input/output and the error buffer remain live
    // for this synchronous device-free validation call.
    let ok = unsafe {
        fn64_rt64_roundtrip_enhancement_config(&input, &mut output, error.as_mut_ptr(), error.len())
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 enhancement-config roundtrip failed without a diagnostic",
        ));
    }
    RenderEnhancementSettings::try_from(output)
}

pub(crate) fn roundtrip_emulator_config(
    settings: &RenderEmulatorSettings,
) -> Result<RenderEmulatorSettings, String> {
    let input = RawEmulatorConfig::from(settings);
    let mut output = RawEmulatorConfig::default();
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: scalar repr(C) input/output and the error buffer remain live
    // for this synchronous device-free validation call.
    let ok = unsafe {
        fn64_rt64_roundtrip_emulator_config(&input, &mut output, error.as_mut_ptr(), error.len())
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 emulator-config roundtrip failed without a diagnostic",
        ));
    }
    RenderEmulatorSettings::try_from(output)
}

pub(crate) fn inspect_replacement_pack(
    path: &CString,
) -> Result<(RenderReplacementPackIdentity, Vec<u8>), String> {
    let mut config = RawReplacementDatabaseConfig::default();
    let mut database_size = 0usize;
    let mut error = [0; ERROR_CAPACITY];
    // SAFETY: path and scalar outputs remain live. A null database pointer is
    // paired with zero capacity for the documented size query.
    let ok = unsafe {
        fn64_rt64_inspect_replacement_pack(
            path.as_ptr(),
            &mut config,
            std::ptr::null_mut(),
            0,
            &mut database_size,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 replacement-pack inspection failed without a diagnostic",
        ));
    }
    let mut database = vec![0u8; database_size];
    let mut second_config = RawReplacementDatabaseConfig::default();
    let mut second_size = 0usize;
    error.fill(0);
    // SAFETY: the exact capacity returned by the first pass is writable and
    // all other pointers remain live for the synchronous device-free call.
    let ok = unsafe {
        fn64_rt64_inspect_replacement_pack(
            path.as_ptr(),
            &mut second_config,
            database.as_mut_ptr(),
            database.len(),
            &mut second_size,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ok == 0 {
        return Err(error_string(
            &error,
            "RT64 replacement-pack second inspection failed without a diagnostic",
        ));
    }
    if second_size != database.len() || second_config != config {
        return Err("replacement database changed between inspection passes".into());
    }
    Ok((replacement_identity_from_raw(config)?, database))
}

