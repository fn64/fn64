//! Typed RT64 antialiasing experiment presets and strict TOML loading.
//!
//! This module contains no window or renderer ownership. The WM2000 shell
//! includes it by path and applies the resulting complete settings image
//! through fn64-abi's registered-renderer seam.

use fn64_render::{
    DownsampleMultiplier, RenderAntialiasing, RenderResolution, RenderRuntimeSettings,
    ResolutionMultiplier,
};
use serde::Deserialize;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Rt64AaPreset {
    Native,
    HighResolution2x,
    Supersample2x,
    Msaa4x,
}

impl Rt64AaPreset {
    pub const ALL: [Self; 4] = [
        Self::Native,
        Self::HighResolution2x,
        Self::Supersample2x,
        Self::Msaa4x,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Native => "native-1x",
            Self::HighResolution2x => "high-resolution-2x",
            Self::Supersample2x => "supersample-2x-box",
            Self::Msaa4x => "native-1x-msaa4x",
        }
    }

    pub fn settings(self) -> RenderRuntimeSettings {
        let mut settings = RenderRuntimeSettings::default();
        match self {
            Self::Native => {}
            Self::HighResolution2x => {
                settings.resolution = RenderResolution::Manual;
                settings.resolution_multiplier =
                    ResolutionMultiplier::new(2.0).expect("2x is a valid resolution multiplier");
            }
            Self::Supersample2x => {
                settings.resolution = RenderResolution::Manual;
                settings.resolution_multiplier =
                    ResolutionMultiplier::new(2.0).expect("2x is a valid resolution multiplier");
                settings.downsample_multiplier =
                    DownsampleMultiplier::new(2).expect("2x is a valid downsample multiplier");
            }
            Self::Msaa4x => settings.antialiasing = RenderAntialiasing::Msaa4x,
        }
        settings
    }

    pub fn from_settings(settings: &RenderRuntimeSettings) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.settings() == *settings)
    }
}

#[derive(Copy, Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConfigResolution {
    Original,
    WindowIntegerScale,
    Manual,
}

impl From<ConfigResolution> for RenderResolution {
    fn from(value: ConfigResolution) -> Self {
        match value {
            ConfigResolution::Original => Self::Original,
            ConfigResolution::WindowIntegerScale => Self::WindowIntegerScale,
            ConfigResolution::Manual => Self::Manual,
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConfigAntialiasing {
    None,
    Msaa2x,
    Msaa4x,
    Msaa8x,
}

impl From<ConfigAntialiasing> for RenderAntialiasing {
    fn from(value: ConfigAntialiasing) -> Self {
        match value {
            ConfigAntialiasing::None => Self::None,
            ConfigAntialiasing::Msaa2x => Self::Msaa2x,
            ConfigAntialiasing::Msaa4x => Self::Msaa4x,
            ConfigAntialiasing::Msaa8x => Self::Msaa8x,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    resolution: ConfigResolution,
    resolution_multiplier: f64,
    downsample_multiplier: u32,
    antialiasing: ConfigAntialiasing,
}

impl ConfigFile {
    fn into_settings(self) -> Result<RenderRuntimeSettings, Rt64AaConfigError> {
        let mut settings = RenderRuntimeSettings::default();
        settings.resolution = self.resolution.into();
        settings.resolution_multiplier = ResolutionMultiplier::new(self.resolution_multiplier)
            .map_err(Rt64AaConfigError::InvalidSetting)?;
        settings.downsample_multiplier = DownsampleMultiplier::new(self.downsample_multiplier)
            .map_err(Rt64AaConfigError::InvalidSetting)?;
        settings.antialiasing = self.antialiasing.into();
        Ok(settings)
    }
}

#[derive(Debug)]
pub enum Rt64AaConfigError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    InvalidSetting(fn64_render::RenderSettingsError),
}

impl fmt::Display for Rt64AaConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "cannot read RT64 settings file {}: {source}",
                    path.display()
                )
            }
            Self::Parse { path, source } => {
                write!(f, "invalid RT64 settings file {}: {source}", path.display())
            }
            Self::InvalidSetting(source) => source.fmt(f),
        }
    }
}

impl std::error::Error for Rt64AaConfigError {}

pub fn load(path: &Path) -> Result<RenderRuntimeSettings, Rt64AaConfigError> {
    let source = std::fs::read_to_string(path).map_err(|source| Rt64AaConfigError::Read {
        path: path.to_owned(),
        source,
    })?;
    let parsed =
        toml::from_str::<ConfigFile>(&source).map_err(|source| Rt64AaConfigError::Parse {
            path: path.to_owned(),
            source,
        })?;
    parsed.into_settings()
}

pub fn settings_sha256_hex(settings: &RenderRuntimeSettings) -> String {
    settings
        .sha256()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
