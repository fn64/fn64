//! The player-facing settings menu: Video/Audio/Input tabs rendered with
//! RmlUi, toggled by F1 from `Shell::window_event`/`about_to_wait`.
//!
//! ## The one piece this module cannot wire up yet
//!
//! [`SettingsMenu::ui`] is `None` for the whole life of this binary today.
//! Constructing a live `fn64_rmlui::Context` needs a raw `Fn64Rt64Context*`
//! (see `fn64_rmlui::Context::create`'s safety doc), and there is currently
//! NO route from `Shell` to one: `register_render_backend` moves the
//! concrete `Rt64Backend` into `fn64_abi::set_render_backend_with_policy`,
//! which stores it behind `Box<dyn RenderBackend>` in a `pub(crate)`
//! thread-local (`fn64_abi::task_dispatch::lifecycle::RENDER_BACKEND`) that
//! is deliberately never downcast or reached into again --
//! `capture_render_release_frame`'s own doc comment states the rule
//! explicitly ("a host neither downcasts the backend nor reaches into RT64
//! after registration"). `fn64-abi` exposes no `with_raw_rt64_context`-style
//! escape hatch, and adding one is a new, cross-crate seam through a
//! deliberately closed boundary -- exactly the kind of "separate, large,
//! not-yet-designed problem" this work is scoped to flag rather than
//! silently build. See `Shell::ensure_settings_ui` below for where
//! construction would happen once that seam exists.
//!
//! A second, independent gap sits behind that first one even once a context
//! handle is reachable: `fn64_rt64_shim.cpp`'s `draw_hook_dispatch` runs
//! present-capture's own GPU-copy command (stage 1, the read the window
//! blit path uses) BEFORE any registered overlay draw callback (stage 2),
//! by explicit design ("Stage 1 is present-capture's own readback,
//! unchanged, so it keeps seeing a UI-free frame" -- that file's own
//! comment on `draw_hook_dispatch`). Present-capture is the readback
//! `present_rt64` in `shell.rs` uses for every frame the player sees, so
//! today's stage ordering means an RmlUi overlay would render into RT64's
//! target too late to ever reach the window, even after the first gap is
//! closed. Reordering those stages is a change to tested, evidence-critical
//! C++ this work's own brief says not to touch without cause ("don't touch
//! unless you find an actual bug").
//!
//! Everything else in this module -- markup, live settings state,
//! persistence, F1/Escape/mouse/gamepad arbitration -- is real and wired,
//! so the moment a context is constructible the `ui.is_none()` branches
//! below are the only places that need to change.
//!
//! One consequence of that gap shows up as `dead_code` in a plain (non-test)
//! build: `ensure_settings_ui`'s real body -- the one place that would call
//! `SETTINGS_RML`, `Tab::panel_id`/`tab_button_id`, `SettingsMenu::
//! binding_slots`, the `Capture` variants, and each settings enum's
//! `to_render`/`from_select_value` conversions -- never runs, because it
//! has nothing to construct against yet. `#[cfg(test)]` exercises all of
//! it (see this file's own test module), which is why `cargo test` does
//! not show these warnings the way `cargo build` does. Allowed at the
//! module level rather than item-by-item, since it is one root cause, not
//! several unrelated ones -- remove this once `ensure_settings_ui` has a
//! real body to call through.
#![allow(dead_code)]

use crate::input_map::{BindTarget, InputConfig, N64Button, StickDir};

/// fn64's settings UI markup, embedded rather than loaded from a loose file
/// at runtime (matching the convention `fn64_rmlui_shim.h` documents for
/// `fn64_rmlui_load_document_from_memory`: fn64 embeds its own UI markup in
/// the binary).
pub(crate) const SETTINGS_RML: &str =
    include_str!("../../../crates/fn64-rmlui/assets/settings.rml");

/// What the next keyboard or gamepad-button event will bind, once armed by
/// clicking (or, once gamepad menu navigation lands, activating) a binding
/// slot in the Input tab. Mirrors `crates/fn64-shell/src/overlay.rs`'s
/// `Capture` enum -- same two-variant shape, same meaning -- since this
/// replaces that egui screen's functionality on RmlUi markup, not its
/// design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Capture {
    Key(BindTarget),
    Pad(N64Button),
}

/// Video+audio settings this menu exposes, persisted independently of
/// `InputConfig` (which keeps its own existing TOML file unchanged, per the
/// brief's persistence design). Deliberately narrower than
/// `fn64_render::RenderRuntimeSettings`'s full 19 fields -- see this
/// module's own scope-discipline note at the bottom of this file for what
/// is intentionally left out.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct VideoAudioSettings {
    pub resolution: ResolutionMode,
    /// Only meaningful when `resolution == Manual`; RT64's own bounds are
    /// 0.0..=32.0 (`fn64_render::ResolutionMultiplier::{MIN,MAX}`).
    pub resolution_multiplier: f64,
    pub antialiasing: AntialiasingMode,
    pub aspect_ratio: AspectRatioMode,
    /// Only meaningful when `aspect_ratio == Manual`; RT64's own bounds are
    /// 0.1..=100.0 (`fn64_render::AspectTarget::{MIN,MAX}`).
    pub aspect_target: f64,
    /// The window's own present-mode knob (`pixels`/`wgpu`'s
    /// `PresentMode`), NOT an RT64 `UserConfiguration` field -- see
    /// `Shell::resumed`'s existing `FN64_PRESENT_MODE` handling in
    /// `shell.rs`, which this setting now supersedes as the persisted
    /// source of truth (the env var remains a one-off override for anyone
    /// who sets it, same precedence `InputConfig`'s own load path uses
    /// nowhere else in this binary, i.e. none -- env wins if set, since
    /// `Shell::resumed` reads it directly and this module does not erase
    /// that check).
    pub present_mode: PresentModeSetting,
    /// 0.0..=1.0 linear gain. See this file's audio-volume note near
    /// [`VideoAudioSettings::default`] for why this field currently has no
    /// live audio-backend consumer.
    pub master_volume: f32,
}

impl Default for VideoAudioSettings {
    fn default() -> Self {
        // Matches fn64_render::RenderRuntimeSettings::default() for the
        // fields this menu exposes (Original resolution/aspect, no AA),
        // NOT RenderRuntimeSettings::upstream_default() -- fn64's own
        // chosen default differs from pinned RT64's, and this menu should
        // not silently reintroduce upstream's 2x/integer-scale default.
        Self {
            resolution: ResolutionMode::Original,
            resolution_multiplier: 1.0,
            antialiasing: AntialiasingMode::Off,
            aspect_ratio: AspectRatioMode::Original,
            aspect_target: 16.0 / 9.0,
            present_mode: PresentModeSetting::NoVsync,
            master_volume: 1.0,
        }
    }
}

impl VideoAudioSettings {
    /// `~/.config/fn64/settings.toml` (platform equivalent via `dirs`),
    /// matching `InputConfig::path`'s own `~/.config/fn64/`-adjacent
    /// convention and its `dirs` crate choice exactly, so the two files
    /// live side by side under one config directory.
    pub fn path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("fn64").join("settings.toml"))
    }

    /// Load the saved settings, or defaults. Never fatal, same discipline as
    /// `InputConfig::load`: a missing file is the common first-run case, a
    /// malformed one is logged and replaced by defaults in memory.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<VideoAudioSettings>(&text) {
                Ok(settings) => {
                    println!(
                        "[wm2000-shell] video/audio settings loaded from {}",
                        path.display()
                    );
                    settings
                }
                Err(e) => {
                    eprintln!(
                        "[wm2000-shell] video/audio settings {} is malformed ({e}) -- using \
                         defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Persist to disk. Failures are logged, never fatal, same discipline as
    /// `InputConfig::save`.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            eprintln!(
                "[wm2000-shell] no config directory on this platform -- video/audio settings not \
                 saved"
            );
            return;
        };
        let text = toml::to_string_pretty(self).expect("VideoAudioSettings serializes to TOML");
        let result = std::fs::create_dir_all(path.parent().expect("config path has a parent"))
            .and_then(|()| std::fs::write(&path, text));
        if let Err(e) = result {
            eprintln!(
                "[wm2000-shell] failed to save video/audio settings {}: {e}",
                path.display()
            );
        }
    }

    /// Build the subset of `fn64_render::RenderRuntimeSettings` this menu
    /// controls, starting from fn64's own default for every field the menu
    /// does not expose (graphics API stays `Automatic`, display buffering
    /// stays `Double`, etc. -- see this module's scope-discipline note for
    /// the full excluded list). The live-apply call site
    /// (`Rt64Backend::apply_user_config`, via `fn64_abi`) is unreachable for
    /// the same "no route to the registered backend" reason
    /// `SettingsMenu::ui` is always `None` today; this conversion exists so
    /// the wiring is ready the moment that seam does.
    pub fn to_render_runtime_settings(&self) -> Result<fn64_render::RenderRuntimeSettings, String> {
        let resolution_multiplier =
            fn64_render::ResolutionMultiplier::new(self.resolution_multiplier)
                .map_err(|e| e.to_string())?;
        let aspect_target =
            fn64_render::AspectTarget::new(self.aspect_target).map_err(|e| e.to_string())?;
        Ok(fn64_render::RenderRuntimeSettings {
            resolution: self.resolution.to_render(),
            antialiasing: self.antialiasing.to_render(),
            resolution_multiplier,
            aspect_ratio: self.aspect_ratio.to_render(),
            aspect_target,
            ..fn64_render::RenderRuntimeSettings::default()
        })
    }
}

/// Mirrors `fn64_render::RenderResolution`'s three variants with the same
/// integer tags, so `RenderResolution::tag()`'s wire values and this menu's
/// `<select>` option values agree without importing `fn64_render`'s
/// `tagged_enum!`-generated (crate-private) `tag()` method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum ResolutionMode {
    Original = 0,
    WindowIntegerScale = 1,
    Manual = 2,
}

impl ResolutionMode {
    pub fn to_render(self) -> fn64_render::RenderResolution {
        match self {
            Self::Original => fn64_render::RenderResolution::Original,
            Self::WindowIntegerScale => fn64_render::RenderResolution::WindowIntegerScale,
            Self::Manual => fn64_render::RenderResolution::Manual,
        }
    }

    pub fn from_select_value(value: &str) -> Option<Self> {
        match value {
            "0" => Some(Self::Original),
            "1" => Some(Self::WindowIntegerScale),
            "2" => Some(Self::Manual),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum AntialiasingMode {
    Off = 0,
    Msaa2x = 1,
    Msaa4x = 2,
    Msaa8x = 3,
}

impl AntialiasingMode {
    pub fn to_render(self) -> fn64_render::RenderAntialiasing {
        match self {
            Self::Off => fn64_render::RenderAntialiasing::None,
            Self::Msaa2x => fn64_render::RenderAntialiasing::Msaa2x,
            Self::Msaa4x => fn64_render::RenderAntialiasing::Msaa4x,
            Self::Msaa8x => fn64_render::RenderAntialiasing::Msaa8x,
        }
    }

    pub fn from_select_value(value: &str) -> Option<Self> {
        match value {
            "0" => Some(Self::Off),
            "1" => Some(Self::Msaa2x),
            "2" => Some(Self::Msaa4x),
            "3" => Some(Self::Msaa8x),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum AspectRatioMode {
    Original = 0,
    Expand = 1,
    Manual = 2,
}

impl AspectRatioMode {
    pub fn to_render(self) -> fn64_render::RenderAspectRatio {
        match self {
            Self::Original => fn64_render::RenderAspectRatio::Original,
            Self::Expand => fn64_render::RenderAspectRatio::Expand,
            Self::Manual => fn64_render::RenderAspectRatio::Manual,
        }
    }

    pub fn from_select_value(value: &str) -> Option<Self> {
        match value {
            "0" => Some(Self::Original),
            "1" => Some(Self::Expand),
            "2" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// Mirrors `Shell::resumed`'s existing `FN64_PRESENT_MODE` match arms
/// exactly (`"vsync"` -> `AutoVsync`, `"mailbox"` -> `Mailbox`, default ->
/// `AutoNoVsync`), so this setting and that env var name the same three
/// options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum PresentModeSetting {
    NoVsync = 0,
    Vsync = 1,
    Mailbox = 2,
}

impl PresentModeSetting {
    pub fn to_wgpu(self) -> pixels::wgpu::PresentMode {
        match self {
            Self::NoVsync => pixels::wgpu::PresentMode::AutoNoVsync,
            Self::Vsync => pixels::wgpu::PresentMode::AutoVsync,
            Self::Mailbox => pixels::wgpu::PresentMode::Mailbox,
        }
    }

    pub fn from_select_value(value: &str) -> Option<Self> {
        match value {
            "0" => Some(Self::NoVsync),
            "1" => Some(Self::Vsync),
            "2" => Some(Self::Mailbox),
            _ => None,
        }
    }
}

/// Modal settings-menu state. A sibling struct held by `Shell`, mirroring
/// how `crates/fn64-shell/src/overlay.rs`'s `Overlay` is a sibling struct to
/// that binary's `App` rather than being inlined into it.
pub(crate) struct SettingsMenu {
    pub open: bool,
    /// `active_tab` indexes `Tab::ALL`; kept as a plain field (not derived
    /// from RmlUi element state) so tab-switching logic and gamepad
    /// navigation can drive it without round-tripping through the DOM.
    pub active_tab: Tab,
    pub capture: Option<Capture>,
    pub settings: VideoAudioSettings,
    /// Set by any UI change while the menu is open; drives the
    /// save-on-close (not save-on-every-tick) persistence the brief calls
    /// for.
    pub dirty: bool,
    /// Live RmlUi context/document, once constructible -- see this module's
    /// own top-of-file doc comment for why this is `None` for the whole
    /// life of this binary today.
    #[cfg(feature = "rmlui")]
    pub ui: Option<SettingsUi>,
}

#[cfg(feature = "rmlui")]
pub(crate) struct SettingsUi {
    pub context: fn64_rmlui::Context,
    pub document: fn64_rmlui::Document,
}

/// The three tabs, in tab-strip order. `ALL` backs both mouse-click
/// tab-switching (matching a clicked button's index) and a future
/// gamepad-shoulder-button tab cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Video,
    Audio,
    Input,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Video, Tab::Audio, Tab::Input];

    pub fn panel_id(self) -> &'static str {
        match self {
            Tab::Video => "panel_video",
            Tab::Audio => "panel_audio",
            Tab::Input => "panel_input",
        }
    }

    pub fn tab_button_id(self) -> &'static str {
        match self {
            Tab::Video => "tab_button_video",
            Tab::Audio => "tab_button_audio",
            Tab::Input => "tab_button_input",
        }
    }
}

impl SettingsMenu {
    pub fn new() -> Self {
        Self {
            open: false,
            active_tab: Tab::Video,
            capture: None,
            settings: VideoAudioSettings::load(),
            dirty: false,
            #[cfg(feature = "rmlui")]
            ui: None,
        }
    }

    /// Save if anything changed since the last save, then clear the dirty
    /// flag. Called on menu close, matching the brief's "save-on-close"
    /// choice (drag-tick saves would spam disk writes for a slider; this
    /// mirrors `overlay.rs`'s own `drag_stopped()`-gated save for the same
    /// reason, generalized to "close" since RmlUi's `<input type="range">`
    /// change event does not distinguish mid-drag from drag-released the
    /// way egui's `Response` does).
    pub fn save_if_dirty(&mut self) {
        if self.dirty {
            self.settings.save();
            self.dirty = false;
        }
    }

    /// Every keyboard/gamepad-button binding slot's element id, alongside
    /// the `Capture` it arms -- the Input tab's complete bindings-grid
    /// table, replacing `overlay.rs::bindings_grid`'s egui construction
    /// with the same data driving RmlUi element lookups instead.
    pub fn binding_slots() -> [(&'static str, Capture); 18] {
        [
            ("bind_stick_up", Capture::Key(BindTarget::Stick(StickDir::Up))),
            ("bind_stick_down", Capture::Key(BindTarget::Stick(StickDir::Down))),
            ("bind_stick_left", Capture::Key(BindTarget::Stick(StickDir::Left))),
            ("bind_stick_right", Capture::Key(BindTarget::Stick(StickDir::Right))),
            ("bind_button_a", Capture::Key(BindTarget::Button(N64Button::A))),
            ("bind_button_b", Capture::Key(BindTarget::Button(N64Button::B))),
            ("bind_button_start", Capture::Key(BindTarget::Button(N64Button::Start))),
            ("bind_button_z", Capture::Key(BindTarget::Button(N64Button::Z))),
            ("bind_button_l", Capture::Key(BindTarget::Button(N64Button::L))),
            ("bind_button_r", Capture::Key(BindTarget::Button(N64Button::R))),
            ("bind_button_cup", Capture::Key(BindTarget::Button(N64Button::CUp))),
            ("bind_button_cdown", Capture::Key(BindTarget::Button(N64Button::CDown))),
            ("bind_button_cleft", Capture::Key(BindTarget::Button(N64Button::CLeft))),
            ("bind_button_cright", Capture::Key(BindTarget::Button(N64Button::CRight))),
            ("bind_button_dup", Capture::Key(BindTarget::Button(N64Button::DUp))),
            ("bind_button_ddown", Capture::Key(BindTarget::Button(N64Button::DDown))),
            ("bind_button_dleft", Capture::Key(BindTarget::Button(N64Button::DLeft))),
            ("bind_button_dright", Capture::Key(BindTarget::Button(N64Button::DRight))),
        ]
    }

    /// A keyboard key arrived while a keyboard capture was armed. Returns
    /// `true` if it was consumed as a binding (mirrors
    /// `overlay.rs::apply_key_capture`'s return-value contract exactly, so
    /// the calling convention at the `window_event` call site is familiar).
    pub fn apply_key_capture(&mut self, config: &mut InputConfig, key: winit::keyboard::KeyCode) -> bool {
        let Some(Capture::Key(target)) = self.capture else {
            return false;
        };
        config.bind_key(target, key);
        config.save();
        self.capture = None;
        println!("[wm2000-shell] input: bound {key:?} via settings menu");
        true
    }

    /// A gamepad button arrived while a gamepad capture was armed.
    pub fn apply_pad_capture(&mut self, config: &mut InputConfig, button: gilrs::Button) {
        let Some(Capture::Pad(target)) = self.capture else {
            return;
        };
        config.bind_pad(target, button);
        config.save();
        self.capture = None;
        println!("[wm2000-shell] input: bound gamepad {button:?} via settings menu");
    }
}

impl Default for SettingsMenu {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `SETTINGS_RML` is only referenced from `shell.rs`'s
    /// `ensure_settings_ui` sketch today (a comment describing the wiring
    /// that lands once the raw-`Fn64Rt64Context*` seam exists, per this
    /// module's own top-of-file doc comment) -- this test is what actually
    /// exercises the constant, so a typo in the embedded path or a future
    /// edit that empties the file fails loudly here instead of silently at
    /// the first real `load_document_from_memory` call, whenever that lands.
    #[test]
    fn settings_rml_embeds_every_id_this_module_binds_by_name() {
        assert!(!SETTINGS_RML.trim().is_empty());
        assert!(SETTINGS_RML.contains("<rml>"));
        for tab in Tab::ALL {
            let panel_needle = format!("id=\"{}\"", tab.panel_id());
            let button_needle = format!("id=\"{}\"", tab.tab_button_id());
            assert!(
                SETTINGS_RML.contains(&panel_needle),
                "settings.rml is missing {panel_needle}"
            );
            assert!(
                SETTINGS_RML.contains(&button_needle),
                "settings.rml is missing {button_needle}"
            );
        }
        for (id, _) in SettingsMenu::binding_slots() {
            let needle = format!("id=\"{id}\"");
            assert!(SETTINGS_RML.contains(&needle), "settings.rml is missing {needle}");
        }
    }

    #[test]
    fn video_audio_settings_roundtrips_through_toml() {
        let settings = VideoAudioSettings {
            resolution: ResolutionMode::Manual,
            resolution_multiplier: 2.5,
            antialiasing: AntialiasingMode::Msaa4x,
            aspect_ratio: AspectRatioMode::Manual,
            aspect_target: 21.0 / 9.0,
            present_mode: PresentModeSetting::Vsync,
            master_volume: 0.5,
        };
        let text = toml::to_string_pretty(&settings).expect("serializes");
        let back: VideoAudioSettings = toml::from_str(&text).expect("deserializes");
        assert_eq!(back, settings);
    }

    #[test]
    fn partial_settings_file_fills_missing_fields_with_defaults() {
        let back: VideoAudioSettings =
            toml::from_str("master_volume = 0.25\n").expect("deserializes");
        assert_eq!(back.master_volume, 0.25);
        assert_eq!(back.resolution, ResolutionMode::Original);
    }

    #[test]
    fn to_render_runtime_settings_maps_every_exposed_field() {
        let settings = VideoAudioSettings {
            resolution: ResolutionMode::Manual,
            resolution_multiplier: 3.0,
            antialiasing: AntialiasingMode::Msaa8x,
            aspect_ratio: AspectRatioMode::Manual,
            aspect_target: 4.0 / 3.0,
            present_mode: PresentModeSetting::NoVsync,
            master_volume: 1.0,
        };
        let render = settings
            .to_render_runtime_settings()
            .expect("in-range settings convert");
        assert_eq!(render.resolution, fn64_render::RenderResolution::Manual);
        assert_eq!(render.resolution_multiplier.get(), 3.0);
        assert_eq!(render.antialiasing, fn64_render::RenderAntialiasing::Msaa8x);
        assert_eq!(render.aspect_ratio, fn64_render::RenderAspectRatio::Manual);
        assert_eq!(render.aspect_target.get(), 4.0 / 3.0);
    }

    #[test]
    fn out_of_range_resolution_multiplier_is_rejected_not_clamped() {
        let settings = VideoAudioSettings {
            resolution_multiplier: 999.0,
            ..VideoAudioSettings::default()
        };
        assert!(settings.to_render_runtime_settings().is_err());
    }

    #[test]
    fn select_value_round_trips_for_every_variant() {
        for mode in [
            ResolutionMode::Original,
            ResolutionMode::WindowIntegerScale,
            ResolutionMode::Manual,
        ] {
            let value = (mode as i32).to_string();
            assert_eq!(ResolutionMode::from_select_value(&value), Some(mode));
        }
        for mode in [
            AntialiasingMode::Off,
            AntialiasingMode::Msaa2x,
            AntialiasingMode::Msaa4x,
            AntialiasingMode::Msaa8x,
        ] {
            let value = (mode as i32).to_string();
            assert_eq!(AntialiasingMode::from_select_value(&value), Some(mode));
        }
        for mode in [
            AspectRatioMode::Original,
            AspectRatioMode::Expand,
            AspectRatioMode::Manual,
        ] {
            let value = (mode as i32).to_string();
            assert_eq!(AspectRatioMode::from_select_value(&value), Some(mode));
        }
        for mode in [
            PresentModeSetting::NoVsync,
            PresentModeSetting::Vsync,
            PresentModeSetting::Mailbox,
        ] {
            let value = (mode as i32).to_string();
            assert_eq!(PresentModeSetting::from_select_value(&value), Some(mode));
        }
    }
}
