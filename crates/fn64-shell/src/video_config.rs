//! Player-facing video/display settings, persisted to TOML at
//! `~/.config/fn64/video.toml` (platform equivalent via `dirs`), edited live
//! from the settings overlay's Video tab. Mirrors [`crate::input_map::InputConfig`]'s
//! load/save/persist pattern.
//!
//! ## Why overscan is a setting, not a derived crop
//!
//! WM2000 fills framebuffer columns 0..478 of a 480-wide line while the VI
//! scans out all 480, so the uncovered rightmost column presents stale RDRAM.
//! A real N64 hides that column behind the TV's overscan; fn64 shows the whole
//! scanned rectangle. Which columns are "overscan" is a *display policy*, not
//! something any RDP/VI oracle can adjudicate (the column IS genuinely
//! scanned) -- so it's a player setting with a default that hides the artifact,
//! not a geometry-derived crop. `overscan=0` presents the raw full scanout.

use serde::{Deserialize, Serialize};

/// The default right-edge crop: exactly the one uncovered overscan column on
/// the standard 480-active NTSC framebuffer (WM2000's col 479). Small enough
/// to never eat real content, enough to hide the stale column.
const DEFAULT_OVERSCAN: u32 = 1;

/// `FN64_OVERSCAN=<px>` forces the overscan crop for headless runs, gates, and
/// captures. Read once at boot (perf-method: no per-frame env reads); it
/// overrides the persisted value for this session but is not saved back.
pub const OVERSCAN_ENV: &str = "FN64_OVERSCAN";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    /// Columns cropped from the right edge on present. `0` = full raw scanout
    /// (the stale overscan column shows); `N` = crop N columns so the
    /// uncovered overscan isn't displayed. Display-time only: guest RDRAM and
    /// the framebuffer stride are untouched, kept columns stay byte-identical.
    pub overscan: u32,
    /// When true, stretch the game to fill the whole window (no letterbox
    /// matte), sacrificing the original display aspect. When false (default),
    /// the game is centered at the N64's original 4:3 aspect with a matte.
    pub zoom_fill: bool,
    /// When false, [`VideoConfig::save`] is a no-op -- a property of this
    /// in-memory copy (e.g. the `--demo` throwaway), not of the user's file.
    #[serde(skip, default = "persist_default")]
    pub persist: bool,
}

fn persist_default() -> bool {
    true
}

impl Default for VideoConfig {
    fn default() -> Self {
        VideoConfig {
            overscan: DEFAULT_OVERSCAN,
            zoom_fill: false,
            persist: true,
        }
    }
}

impl VideoConfig {
    /// `~/.config/fn64/video.toml` (platform equivalent via `dirs`).
    pub fn path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("fn64").join("video.toml"))
    }

    /// Load the saved config, apply the `FN64_OVERSCAN` env override if set,
    /// then defaults. Never fatal: missing file is first-run, malformed is
    /// logged and replaced by defaults in memory.
    pub fn load() -> Self {
        let mut config = Self::path()
            .and_then(|path| std::fs::read_to_string(&path).ok().map(|t| (path, t)))
            .map(|(path, text)| match toml::from_str::<VideoConfig>(&text) {
                Ok(config) => {
                    println!("[fn64-shell] video config loaded from {}", path.display());
                    config
                }
                Err(e) => {
                    eprintln!(
                        "[fn64-shell] video config {} is malformed ({e}) -- using defaults",
                        path.display()
                    );
                    Self::default()
                }
            })
            .unwrap_or_default();
        // Env override wins for this session (gates/captures force a value)
        // but is not persisted back.
        if let Some(px) = std::env::var(OVERSCAN_ENV).ok().and_then(|v| v.parse().ok()) {
            println!("[fn64-shell] {OVERSCAN_ENV}={px} overrides overscan for this session");
            config.overscan = px;
        }
        config
    }

    /// Persist to disk. Failures are logged, never fatal.
    pub fn save(&self) {
        if !self.persist {
            return;
        }
        let Some(path) = Self::path() else {
            eprintln!("[fn64-shell] no config directory on this platform -- video config not saved");
            return;
        };
        let text = toml::to_string_pretty(self).expect("VideoConfig serializes to TOML");
        let result = std::fs::create_dir_all(path.parent().expect("config path has a parent"))
            .and_then(|()| std::fs::write(&path, text));
        if let Err(e) = result {
            eprintln!(
                "[fn64-shell] failed to save video config {}: {e}",
                path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_crops_exactly_the_one_overscan_column() {
        assert_eq!(VideoConfig::default().overscan, 1);
        assert!(!VideoConfig::default().zoom_fill);
    }

    #[test]
    fn roundtrips_through_toml() {
        let c = VideoConfig {
            overscan: 4,
            zoom_fill: true,
            persist: true,
        };
        let text = toml::to_string_pretty(&c).expect("serializes");
        let back: VideoConfig = toml::from_str(&text).expect("deserializes");
        assert_eq!(back.overscan, 4);
        assert!(back.zoom_fill);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let back: VideoConfig = toml::from_str("overscan = 2\n").expect("deserializes");
        assert_eq!(back.overscan, 2);
        assert!(!back.zoom_fill); // #[serde(default)] filled it
    }
}
