//! Typed runtime controls for pinned RT64 user, enhancement, emulator, and
//! replacement-pack configuration families.
//!
//! The field inventory and numeric bounds come from RT64's MIT
//! `src/common/rt64_user_configuration.{h,cpp}` at
//! `f0728a2520d5aa735886240de3fee75cc805f6d6`. Invalid values are rejected at
//! construction instead of inheriting upstream's clamping behavior.

use std::fmt;

use sha2::{Digest, Sha256};

const SETTINGS_SCHEMA: &[u8] = b"fn64.render-runtime-settings.v1\0";
const ENHANCEMENT_SCHEMA: &[u8] = b"fn64.render-enhancement-settings.v1\0";
const EMULATOR_SCHEMA: &[u8] = b"fn64.render-emulator-settings.v1\0";
const REPLACEMENT_SCHEMA: &[u8] = b"fn64.render-replacement-settings.v1\0";
const POLICY_SCHEMA: &[u8] = b"fn64.render-runtime-policy.v2\0";

macro_rules! tagged_enum {
    ($(#[$meta:meta])* pub enum $name:ident { $($variant:ident = $tag:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            const fn tag(self) -> u8 {
                match self {
                    $(Self::$variant => $tag),+
                }
            }
        }
    };
}

tagged_enum! {
    /// Graphics API chosen while RT64 creates its device. Changing this after
    /// creation requires backend recreation.
    pub enum RenderGraphicsApi {
        D3d12 = 0,
        Vulkan = 1,
        Metal = 2,
        Automatic = 3,
    }
}

tagged_enum! {
    pub enum RenderResolution {
        Original = 0,
        WindowIntegerScale = 1,
        Manual = 2,
    }
}

tagged_enum! {
    /// Swapchain image count. RT64 consumes this during setup, so changing it
    /// requires backend recreation.
    pub enum RenderDisplayBuffering {
        Double = 0,
        Triple = 1,
    }
}

tagged_enum! {
    pub enum RenderAntialiasing {
        None = 0,
        Msaa2x = 1,
        Msaa4x = 2,
        Msaa8x = 3,
    }
}

tagged_enum! {
    pub enum RenderFiltering {
        Nearest = 0,
        Linear = 1,
        AntiAliasedPixelScaling = 2,
    }
}

tagged_enum! {
    pub enum RenderAspectRatio {
        Original = 0,
        Expand = 1,
        Manual = 2,
    }
}

tagged_enum! {
    pub enum RenderUpscale2d {
        Original = 0,
        ScaledOnly = 1,
        All = 2,
    }
}

tagged_enum! {
    pub enum RenderRefreshRate {
        Original = 0,
        Display = 1,
        Manual = 2,
    }
}

tagged_enum! {
    /// Internal render-target precision. RT64 selects target formats during
    /// setup, so changing this after creation requires backend recreation.
    pub enum RenderInternalColorFormat {
        Standard = 0,
        High = 1,
        Automatic = 2,
    }
}

tagged_enum! {
    pub enum RenderHardwareResolve {
        Disabled = 0,
        Enabled = 1,
        Automatic = 2,
    }
}

macro_rules! bounded_float {
    ($name:ident, $field:literal, $min:expr, $max:expr) => {
        #[derive(Copy, Clone, Debug, PartialEq)]
        pub struct $name(f64);

        impl $name {
            pub const MIN: f64 = $min;
            pub const MAX: f64 = $max;

            pub fn new(value: f64) -> Result<Self, RenderSettingsError> {
                if !value.is_finite() {
                    return Err(RenderSettingsError::NonFinite { field: $field });
                }
                if !(Self::MIN..=Self::MAX).contains(&value) {
                    return Err(RenderSettingsError::OutOfRange {
                        field: $field,
                        value,
                        min: Self::MIN,
                        max: Self::MAX,
                    });
                }
                // Canonical bytes must not distinguish IEEE -0 from +0 when
                // the typed value compares equal.
                Ok(Self(if value == 0.0 { 0.0 } else { value }))
            }

            pub const fn get(self) -> f64 {
                self.0
            }
        }
    };
}

bounded_float!(ResolutionMultiplier, "resolution_multiplier", 0.0, 32.0);
bounded_float!(AspectTarget, "aspect_target", 0.1, 100.0);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct DownsampleMultiplier(u8);

impl DownsampleMultiplier {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 32;

    pub fn new(value: u32) -> Result<Self, RenderSettingsError> {
        let value = u8::try_from(value).map_err(|_| RenderSettingsError::OutOfRange {
            field: "downsample_multiplier",
            value: f64::from(value),
            min: f64::from(Self::MIN),
            max: f64::from(Self::MAX),
        })?;
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(RenderSettingsError::OutOfRange {
                field: "downsample_multiplier",
                value: f64::from(value),
                min: f64::from(Self::MIN),
                max: f64::from(Self::MAX),
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RefreshRateTarget(u16);

impl RefreshRateTarget {
    pub const MIN: u16 = 10;
    pub const MAX: u16 = 1000;

    pub fn new(value: u32) -> Result<Self, RenderSettingsError> {
        let value = u16::try_from(value).map_err(|_| RenderSettingsError::OutOfRange {
            field: "refresh_rate_target",
            value: f64::from(value),
            min: f64::from(Self::MIN),
            max: f64::from(Self::MAX),
        })?;
        if !(Self::MIN..=Self::MAX).contains(&value) {
            return Err(RenderSettingsError::OutOfRange {
                field: "refresh_rate_target",
                value: f64::from(value),
                min: f64::from(Self::MIN),
                max: f64::from(Self::MAX),
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Complete typed image of RT64's public `UserConfiguration` fields.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderRuntimeSettings {
    pub graphics_api: RenderGraphicsApi,
    pub resolution: RenderResolution,
    pub display_buffering: RenderDisplayBuffering,
    pub antialiasing: RenderAntialiasing,
    pub resolution_multiplier: ResolutionMultiplier,
    pub downsample_multiplier: DownsampleMultiplier,
    pub filtering: RenderFiltering,
    pub aspect_ratio: RenderAspectRatio,
    pub aspect_target: AspectTarget,
    pub extended_aspect_ratio: RenderAspectRatio,
    pub extended_aspect_target: AspectTarget,
    pub upscale_2d: RenderUpscale2d,
    pub three_point_filtering: bool,
    pub refresh_rate: RenderRefreshRate,
    pub refresh_rate_target: RefreshRateTarget,
    pub internal_color_format: RenderInternalColorFormat,
    pub hardware_resolve: RenderHardwareResolve,
    pub idle_work_active: bool,
    pub developer_mode: bool,
}

impl Default for RenderRuntimeSettings {
    fn default() -> Self {
        Self {
            graphics_api: RenderGraphicsApi::Automatic,
            resolution: RenderResolution::Original,
            display_buffering: RenderDisplayBuffering::Double,
            antialiasing: RenderAntialiasing::None,
            resolution_multiplier: ResolutionMultiplier::new(1.0)
                .expect("one is a valid resolution multiplier"),
            downsample_multiplier: DownsampleMultiplier::new(1)
                .expect("one is a valid downsample multiplier"),
            filtering: RenderFiltering::AntiAliasedPixelScaling,
            aspect_ratio: RenderAspectRatio::Original,
            aspect_target: AspectTarget::new(16.0 / 9.0).expect("16:9 is a valid aspect target"),
            extended_aspect_ratio: RenderAspectRatio::Original,
            extended_aspect_target: AspectTarget::new(16.0 / 9.0)
                .expect("16:9 is a valid extended aspect target"),
            upscale_2d: RenderUpscale2d::Original,
            three_point_filtering: true,
            refresh_rate: RenderRefreshRate::Original,
            refresh_rate_target: RefreshRateTarget::new(60)
                .expect("60 Hz is a valid refresh-rate target"),
            internal_color_format: RenderInternalColorFormat::Automatic,
            hardware_resolve: RenderHardwareResolve::Automatic,
            idle_work_active: true,
            developer_mode: false,
        }
    }
}

impl RenderRuntimeSettings {
    /// Pinned RT64's constructor defaults. [`Default`] intentionally differs:
    /// fn64 starts at original N64 resolution/2D scale while upstream starts
    /// at integer-scaled 2x rendering.
    pub fn upstream_default() -> Self {
        Self {
            resolution: RenderResolution::WindowIntegerScale,
            resolution_multiplier: ResolutionMultiplier::new(2.0)
                .expect("two is a valid resolution multiplier"),
            upscale_2d: RenderUpscale2d::ScaledOnly,
            ..Self::default()
        }
    }

    /// Versioned canonical wire image. Enum tags follow the pinned RT64 public
    /// order and all numeric values are big-endian.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(SETTINGS_SCHEMA.len() + 64);
        out.extend_from_slice(SETTINGS_SCHEMA);
        out.extend([
            self.graphics_api.tag(),
            self.resolution.tag(),
            self.display_buffering.tag(),
            self.antialiasing.tag(),
        ]);
        out.extend_from_slice(&self.resolution_multiplier.get().to_bits().to_be_bytes());
        out.push(self.downsample_multiplier.get());
        out.extend([self.filtering.tag(), self.aspect_ratio.tag()]);
        out.extend_from_slice(&self.aspect_target.get().to_bits().to_be_bytes());
        out.push(self.extended_aspect_ratio.tag());
        out.extend_from_slice(&self.extended_aspect_target.get().to_bits().to_be_bytes());
        out.extend([
            self.upscale_2d.tag(),
            u8::from(self.three_point_filtering),
            self.refresh_rate.tag(),
        ]);
        out.extend_from_slice(&self.refresh_rate_target.get().to_be_bytes());
        out.extend([
            self.internal_color_format.tag(),
            self.hardware_resolve.tag(),
            u8::from(self.idle_work_active),
            u8::from(self.developer_mode),
        ]);
        out
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }

    /// Setup-owned fields that cannot be changed by RT64's live
    /// `updateUserConfig` path.
    pub fn restart_changes_from(&self, active: &Self) -> Vec<RenderRestartField> {
        let mut fields = Vec::new();
        if self.graphics_api != active.graphics_api {
            fields.push(RenderRestartField::GraphicsApi);
        }
        if self.display_buffering != active.display_buffering {
            fields.push(RenderRestartField::DisplayBuffering);
        }
        if self.internal_color_format != active.internal_color_format {
            fields.push(RenderRestartField::InternalColorFormat);
        }
        fields
    }

    /// RT64's MIT inspector passes `discardFBs=true` only for its
    /// `resConfigChanged` group: resolution mode/manual multiplier and aspect
    /// mode/manual target (`rt64_state.cpp`, lines 2079-2119 and 2613-2618).
    pub fn discards_framebuffers_from(&self, active: &Self) -> bool {
        self.resolution != active.resolution
            || (self.resolution == RenderResolution::Manual
                && self.resolution_multiplier != active.resolution_multiplier)
            || self.aspect_ratio != active.aspect_ratio
            || (self.aspect_ratio == RenderAspectRatio::Manual
                && self.aspect_target != active.aspect_target)
    }
}

tagged_enum! {
    /// Pinned RT64 presentation scheduling policy.
    pub enum RenderPresentationMode {
        Console = 0,
        SkipBuffering = 1,
        PresentEarly = 2,
    }
}

/// Complete typed image of pinned RT64 `EnhancementConfiguration`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderEnhancementSettings {
    pub framebuffer_reinterpret_fix_uls: bool,
    pub presentation_mode: RenderPresentationMode,
    pub remove_black_borders: bool,
    pub rect_fix_lower_right: bool,
    pub f3dex_force_branch: bool,
    pub s2dex_fix_bilerp_mismatch: bool,
    pub s2dex_framebuffer_fast_path: bool,
    pub texture_lod_scale: bool,
}

impl Default for RenderEnhancementSettings {
    /// fn64's faithful/off profile. Unlike pinned RT64's constructor, this
    /// retains console presentation ordering and enables no corrective or
    /// fast-path enhancement implicitly.
    fn default() -> Self {
        Self {
            framebuffer_reinterpret_fix_uls: false,
            presentation_mode: RenderPresentationMode::Console,
            remove_black_borders: false,
            rect_fix_lower_right: false,
            f3dex_force_branch: false,
            s2dex_fix_bilerp_mismatch: false,
            s2dex_framebuffer_fast_path: false,
            texture_lod_scale: false,
        }
    }
}

impl RenderEnhancementSettings {
    /// Exact constructor defaults in the pinned RT64 source.
    pub fn upstream_default() -> Self {
        Self {
            framebuffer_reinterpret_fix_uls: true,
            presentation_mode: RenderPresentationMode::SkipBuffering,
            remove_black_borders: true,
            rect_fix_lower_right: true,
            f3dex_force_branch: false,
            s2dex_fix_bilerp_mismatch: true,
            s2dex_framebuffer_fast_path: true,
            texture_lod_scale: false,
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ENHANCEMENT_SCHEMA.len() + 8);
        out.extend_from_slice(ENHANCEMENT_SCHEMA);
        out.extend([
            u8::from(self.framebuffer_reinterpret_fix_uls),
            self.presentation_mode.tag(),
            u8::from(self.remove_black_borders),
            u8::from(self.rect_fix_lower_right),
            u8::from(self.f3dex_force_branch),
            u8::from(self.s2dex_fix_bilerp_mismatch),
            u8::from(self.s2dex_framebuffer_fast_path),
            u8::from(self.texture_lod_scale),
        ]);
        out
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

/// Complete typed image of pinned RT64 `EmulatorConfiguration`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderEmulatorSettings {
    pub post_blend_noise: bool,
    pub post_blend_noise_negative: bool,
    pub framebuffer_render_to_ram: bool,
    pub framebuffer_copy_with_gpu: bool,
}

impl Default for RenderEmulatorSettings {
    /// The pinned upstream defaults are also fn64's integration defaults.
    fn default() -> Self {
        Self {
            post_blend_noise: true,
            post_blend_noise_negative: false,
            framebuffer_render_to_ram: true,
            framebuffer_copy_with_gpu: true,
        }
    }
}

impl RenderEmulatorSettings {
    pub fn upstream_default() -> Self {
        Self::default()
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(EMULATOR_SCHEMA.len() + 4);
        out.extend_from_slice(EMULATOR_SCHEMA);
        out.extend([
            u8::from(self.post_blend_noise),
            u8::from(self.post_blend_noise_negative),
            u8::from(self.framebuffer_render_to_ram),
            u8::from(self.framebuffer_copy_with_gpu),
        ]);
        out
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

tagged_enum! {
    /// Filename-to-texture-hash convention selected by one `rt64.json`.
    pub enum RenderReplacementAutoPath {
        Rt64 = 0,
        Rice = 1,
    }
}

tagged_enum! {
    /// Default load behavior selected by one `rt64.json`. Per-texture and
    /// ordered filter overrides remain covered by the database digest.
    pub enum RenderReplacementOperation {
        Preload = 0,
        Stream = 1,
        Stall = 2,
    }
}

tagged_enum! {
    /// Default half-texel shift selected by one `rt64.json`.
    pub enum RenderReplacementShift {
        None = 0,
        Half = 1,
    }
}

/// Reproducible identity of one successfully inspected replacement pack.
/// Host paths are deliberately absent: release evidence identifies ordered
/// bytes and database behavior, not machine-local mount points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderReplacementPackIdentity {
    pub content_sha256: [u8; 32],
    pub database_sha256: [u8; 32],
    pub auto_path: RenderReplacementAutoPath,
    pub default_operation: RenderReplacementOperation,
    pub default_shift: RenderReplacementShift,
    pub configuration_version: u32,
    pub hash_version: u32,
}

/// Complete active texture-replacement policy. Pack order is semantic: pinned
/// RT64 scans databases in reverse order and later inputs therefore win.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderReplacementSettings {
    pub enabled: bool,
    pub packs: Vec<RenderReplacementPackIdentity>,
}

impl Default for RenderReplacementSettings {
    fn default() -> Self {
        Self {
            // Matches TextureMap's pinned constructor. With no packs this has
            // no visual effect, but retaining the state prevents an implicit
            // default from entering evidence later.
            enabled: true,
            packs: Vec::new(),
        }
    }
}

impl RenderReplacementSettings {
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(REPLACEMENT_SCHEMA.len() + 5 + self.packs.len() * 78);
        out.extend_from_slice(REPLACEMENT_SCHEMA);
        out.push(u8::from(self.enabled));
        out.extend_from_slice(
            &u32::try_from(self.packs.len())
                .expect("replacement-pack count exceeds u32")
                .to_be_bytes(),
        );
        for pack in &self.packs {
            out.extend_from_slice(&pack.content_sha256);
            out.extend_from_slice(&pack.database_sha256);
            out.extend([
                pack.auto_path.tag(),
                pack.default_operation.tag(),
                pack.default_shift.tag(),
            ]);
            out.extend_from_slice(&pack.configuration_version.to_be_bytes());
            out.extend_from_slice(&pack.hash_version.to_be_bytes());
        }
        out
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

/// Complete active render policy currently admitted into release evidence.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderRuntimePolicy {
    pub user: RenderRuntimeSettings,
    pub enhancement: RenderEnhancementSettings,
    pub emulator: RenderEmulatorSettings,
    pub replacement: RenderReplacementSettings,
}

impl RenderRuntimePolicy {
    pub fn upstream_default() -> Self {
        Self {
            user: RenderRuntimeSettings::upstream_default(),
            enhancement: RenderEnhancementSettings::upstream_default(),
            emulator: RenderEmulatorSettings::upstream_default(),
            replacement: RenderReplacementSettings::default(),
        }
    }

    pub fn canonical_bytes(&self) -> Vec<u8> {
        let user = self.user.canonical_bytes();
        let enhancement = self.enhancement.canonical_bytes();
        let emulator = self.emulator.canonical_bytes();
        let replacement = self.replacement.canonical_bytes();
        let mut out = Vec::with_capacity(
            POLICY_SCHEMA.len()
                + user.len()
                + enhancement.len()
                + emulator.len()
                + replacement.len()
                + 16,
        );
        out.extend_from_slice(POLICY_SCHEMA);
        push_family(&mut out, &user);
        push_family(&mut out, &enhancement);
        push_family(&mut out, &emulator);
        push_family(&mut out, &replacement);
        out
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(self.canonical_bytes()).into()
    }
}

fn push_family(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = u32::try_from(bytes.len()).expect("render-policy family encoding exceeds u32");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RenderRestartField {
    GraphicsApi,
    DisplayBuffering,
    InternalColorFormat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderSettingsApply {
    StagedForCreate {
        settings_sha256: [u8; 32],
    },
    LiveApplied {
        settings_sha256: [u8; 32],
        framebuffers_discarded: bool,
    },
    RestartRequired {
        fields: Vec<RenderRestartField>,
        active_settings_sha256: [u8; 32],
        requested_settings_sha256: [u8; 32],
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderPolicyApply {
    StagedForCreate { policy_sha256: [u8; 32] },
    LiveApplied { policy_sha256: [u8; 32] },
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderSettingsError {
    NonFinite {
        field: &'static str,
    },
    OutOfRange {
        field: &'static str,
        value: f64,
        min: f64,
        max: f64,
    },
}

impl fmt::Display for RenderSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinite { field } => write!(f, "render setting {field} must be finite"),
            Self::OutOfRange {
                field,
                value,
                min,
                max,
            } => write!(
                f,
                "render setting {field}={value} is outside the inclusive range {min}..={max}"
            ),
        }
    }
}

impl std::error::Error for RenderSettingsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_default_is_stable_and_enhancements_are_off() {
        let settings = RenderRuntimeSettings::default();
        assert_eq!(settings.resolution, RenderResolution::Original);
        assert_eq!(settings.resolution_multiplier.get(), 1.0);
        assert_eq!(settings.downsample_multiplier.get(), 1);
        assert_eq!(settings.aspect_ratio, RenderAspectRatio::Original);
        assert_eq!(settings.extended_aspect_ratio, RenderAspectRatio::Original);
        assert_eq!(settings.upscale_2d, RenderUpscale2d::Original);
        assert_eq!(settings.refresh_rate, RenderRefreshRate::Original);
        assert_eq!(settings.antialiasing, RenderAntialiasing::None);
    }

    #[test]
    fn named_upstream_default_exposes_the_three_intentional_fn64_differences() {
        let fn64 = RenderRuntimeSettings::default();
        let upstream = RenderRuntimeSettings::upstream_default();
        assert_eq!(upstream.resolution, RenderResolution::WindowIntegerScale);
        assert_eq!(upstream.resolution_multiplier.get(), 2.0);
        assert_eq!(upstream.upscale_2d, RenderUpscale2d::ScaledOnly);
        let restored = RenderRuntimeSettings {
            resolution: fn64.resolution,
            resolution_multiplier: fn64.resolution_multiplier,
            upscale_2d: fn64.upscale_2d,
            ..upstream
        };
        assert_eq!(restored, fn64);
    }

    #[test]
    fn numeric_bounds_reject_instead_of_clamping() {
        assert!(ResolutionMultiplier::new(f64::NAN).is_err());
        assert!(ResolutionMultiplier::new(32.000_1).is_err());
        assert!(DownsampleMultiplier::new(0).is_err());
        assert!(DownsampleMultiplier::new(33).is_err());
        assert!(AspectTarget::new(0.099).is_err());
        assert!(AspectTarget::new(100.001).is_err());
        assert!(RefreshRateTarget::new(9).is_err());
        assert!(RefreshRateTarget::new(1001).is_err());
    }

    #[test]
    fn canonical_sha_changes_for_every_public_field() {
        let base = RenderRuntimeSettings::default();
        let base_sha = base.sha256();
        let variants = [
            RenderRuntimeSettings {
                graphics_api: RenderGraphicsApi::Vulkan,
                ..base.clone()
            },
            RenderRuntimeSettings {
                resolution: RenderResolution::Manual,
                ..base.clone()
            },
            RenderRuntimeSettings {
                display_buffering: RenderDisplayBuffering::Triple,
                ..base.clone()
            },
            RenderRuntimeSettings {
                antialiasing: RenderAntialiasing::Msaa2x,
                ..base.clone()
            },
            RenderRuntimeSettings {
                resolution_multiplier: ResolutionMultiplier::new(2.0).unwrap(),
                ..base.clone()
            },
            RenderRuntimeSettings {
                downsample_multiplier: DownsampleMultiplier::new(2).unwrap(),
                ..base.clone()
            },
            RenderRuntimeSettings {
                filtering: RenderFiltering::Nearest,
                ..base.clone()
            },
            RenderRuntimeSettings {
                aspect_ratio: RenderAspectRatio::Expand,
                ..base.clone()
            },
            RenderRuntimeSettings {
                aspect_target: AspectTarget::new(4.0 / 3.0).unwrap(),
                ..base.clone()
            },
            RenderRuntimeSettings {
                extended_aspect_ratio: RenderAspectRatio::Expand,
                ..base.clone()
            },
            RenderRuntimeSettings {
                extended_aspect_target: AspectTarget::new(21.0 / 9.0).unwrap(),
                ..base.clone()
            },
            RenderRuntimeSettings {
                upscale_2d: RenderUpscale2d::All,
                ..base.clone()
            },
            RenderRuntimeSettings {
                three_point_filtering: false,
                ..base.clone()
            },
            RenderRuntimeSettings {
                refresh_rate: RenderRefreshRate::Manual,
                ..base.clone()
            },
            RenderRuntimeSettings {
                refresh_rate_target: RefreshRateTarget::new(120).unwrap(),
                ..base.clone()
            },
            RenderRuntimeSettings {
                internal_color_format: RenderInternalColorFormat::High,
                ..base.clone()
            },
            RenderRuntimeSettings {
                hardware_resolve: RenderHardwareResolve::Disabled,
                ..base.clone()
            },
            RenderRuntimeSettings {
                idle_work_active: false,
                ..base.clone()
            },
            RenderRuntimeSettings {
                developer_mode: true,
                ..base.clone()
            },
        ];
        assert!(variants
            .into_iter()
            .all(|settings| settings.sha256() != base_sha));
    }

    #[test]
    fn restart_and_framebuffer_discard_classification_matches_upstream_paths() {
        let base = RenderRuntimeSettings::default();
        let restart = RenderRuntimeSettings {
            graphics_api: RenderGraphicsApi::Metal,
            display_buffering: RenderDisplayBuffering::Triple,
            internal_color_format: RenderInternalColorFormat::High,
            ..base.clone()
        };
        assert_eq!(
            restart.restart_changes_from(&base),
            vec![
                RenderRestartField::GraphicsApi,
                RenderRestartField::DisplayBuffering,
                RenderRestartField::InternalColorFormat,
            ]
        );

        let latent_multiplier = RenderRuntimeSettings {
            resolution_multiplier: ResolutionMultiplier::new(3.0).unwrap(),
            ..base.clone()
        };
        assert!(!latent_multiplier.discards_framebuffers_from(&base));
        let manual_multiplier = RenderRuntimeSettings {
            resolution: RenderResolution::Manual,
            resolution_multiplier: ResolutionMultiplier::new(3.0).unwrap(),
            ..base.clone()
        };
        assert!(manual_multiplier.discards_framebuffers_from(&base));
    }

    #[test]
    fn enhancement_profiles_and_every_field_are_canonically_distinct() {
        let faithful = RenderEnhancementSettings::default();
        assert_eq!(faithful.presentation_mode, RenderPresentationMode::Console);
        assert!(!faithful.framebuffer_reinterpret_fix_uls);
        assert!(!faithful.remove_black_borders);
        assert!(!faithful.rect_fix_lower_right);
        assert!(!faithful.f3dex_force_branch);
        assert!(!faithful.s2dex_fix_bilerp_mismatch);
        assert!(!faithful.s2dex_framebuffer_fast_path);
        assert!(!faithful.texture_lod_scale);

        let upstream = RenderEnhancementSettings::upstream_default();
        assert_eq!(
            upstream.presentation_mode,
            RenderPresentationMode::SkipBuffering
        );
        assert!(upstream.framebuffer_reinterpret_fix_uls);
        assert!(upstream.remove_black_borders);
        assert!(upstream.rect_fix_lower_right);
        assert!(upstream.s2dex_fix_bilerp_mismatch);
        assert!(upstream.s2dex_framebuffer_fast_path);

        let base_sha = faithful.sha256();
        let variants = [
            RenderEnhancementSettings {
                framebuffer_reinterpret_fix_uls: true,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                presentation_mode: RenderPresentationMode::PresentEarly,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                remove_black_borders: true,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                rect_fix_lower_right: true,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                f3dex_force_branch: true,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                s2dex_fix_bilerp_mismatch: true,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                s2dex_framebuffer_fast_path: true,
                ..faithful.clone()
            },
            RenderEnhancementSettings {
                texture_lod_scale: true,
                ..faithful.clone()
            },
        ];
        assert!(variants
            .into_iter()
            .all(|settings| settings.sha256() != base_sha));
    }

    #[test]
    fn emulator_and_composite_hash_bind_every_new_policy_field() {
        let emulator = RenderEmulatorSettings::default();
        assert_eq!(emulator, RenderEmulatorSettings::upstream_default());
        let base = RenderRuntimePolicy::default();
        let base_sha = base.sha256();
        for changed in [
            RenderEmulatorSettings {
                post_blend_noise: false,
                ..emulator.clone()
            },
            RenderEmulatorSettings {
                post_blend_noise_negative: true,
                ..emulator.clone()
            },
            RenderEmulatorSettings {
                framebuffer_render_to_ram: false,
                ..emulator.clone()
            },
            RenderEmulatorSettings {
                framebuffer_copy_with_gpu: false,
                ..emulator.clone()
            },
        ] {
            assert_ne!(changed.sha256(), emulator.sha256());
            assert_ne!(
                RenderRuntimePolicy {
                    emulator: changed,
                    ..base.clone()
                }
                .sha256(),
                base_sha
            );
        }
        assert_ne!(
            RenderRuntimePolicy {
                enhancement: RenderEnhancementSettings::upstream_default(),
                ..base.clone()
            }
            .sha256(),
            base_sha
        );
        assert_ne!(
            RenderRuntimePolicy {
                user: RenderRuntimeSettings::upstream_default(),
                ..base
            }
            .sha256(),
            base_sha
        );
    }

    #[test]
    fn replacement_hash_binds_order_bytes_database_and_behavior() {
        let pack = RenderReplacementPackIdentity {
            content_sha256: [1; 32],
            database_sha256: [2; 32],
            auto_path: RenderReplacementAutoPath::Rt64,
            default_operation: RenderReplacementOperation::Stream,
            default_shift: RenderReplacementShift::Half,
            configuration_version: 3,
            hash_version: 5,
        };
        let base = RenderReplacementSettings {
            enabled: true,
            packs: vec![
                pack.clone(),
                RenderReplacementPackIdentity {
                    content_sha256: [3; 32],
                    ..pack.clone()
                },
            ],
        };
        let variants = [
            RenderReplacementSettings {
                enabled: false,
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![base.packs[1].clone(), base.packs[0].clone()],
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![RenderReplacementPackIdentity {
                    database_sha256: [4; 32],
                    ..pack.clone()
                }],
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![RenderReplacementPackIdentity {
                    auto_path: RenderReplacementAutoPath::Rice,
                    ..pack.clone()
                }],
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![RenderReplacementPackIdentity {
                    default_operation: RenderReplacementOperation::Preload,
                    ..pack.clone()
                }],
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![RenderReplacementPackIdentity {
                    default_shift: RenderReplacementShift::None,
                    ..pack.clone()
                }],
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![RenderReplacementPackIdentity {
                    configuration_version: 2,
                    ..pack.clone()
                }],
                ..base.clone()
            },
            RenderReplacementSettings {
                packs: vec![RenderReplacementPackIdentity {
                    hash_version: 4,
                    ..pack
                }],
                ..base.clone()
            },
        ];
        assert!(variants
            .into_iter()
            .all(|variant| variant.sha256() != base.sha256()));
        let policy = RenderRuntimePolicy::default();
        assert_ne!(
            RenderRuntimePolicy {
                replacement: base,
                ..policy.clone()
            }
            .sha256(),
            policy.sha256()
        );
    }
}
