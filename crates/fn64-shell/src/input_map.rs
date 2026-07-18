//! Input bindings -> N64 controller mapping, factored out of the window loop
//! so it can be unit-tested without a live event loop or a linked game.
//!
//! The button bits are the N64 `OSContPad.button` bitmask
//! (`oot-decomp/include/controller.h:4-17`, the same table
//! `fn64_abi::set_controller_state` documents): `A=0x8000`, `B=0x4000`,
//! `Z=0x2000`, `START=0x1000`, d-pad `0x0800..0x0100`, `L=0x0020`,
//! `R=0x0010`, C-buttons `0x0008..0x0001`.
//!
//! Bindings are DATA, not code: [`InputConfig`] holds the keyboard and
//! gamepad maps, serializes to TOML at `~/.config/fn64/input.toml` (or the
//! platform equivalent), and is edited live by the in-game settings overlay
//! (`overlay.rs`). `InputConfig::default()` is the layout the shell has
//! always shipped.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use winit::keyboard::KeyCode;

// N64 OSContPad.button bits (controller.h:4-17).
pub const BTN_A: u16 = 0x8000;
pub const BTN_B: u16 = 0x4000;
pub const BTN_Z: u16 = 0x2000;
pub const BTN_START: u16 = 0x1000;
pub const BTN_DUP: u16 = 0x0800;
pub const BTN_DDOWN: u16 = 0x0400;
pub const BTN_DLEFT: u16 = 0x0200;
pub const BTN_DRIGHT: u16 = 0x0100;
pub const BTN_L: u16 = 0x0020;
pub const BTN_R: u16 = 0x0010;
pub const BTN_CUP: u16 = 0x0008;
pub const BTN_CDOWN: u16 = 0x0004;
pub const BTN_CLEFT: u16 = 0x0002;
pub const BTN_CRIGHT: u16 = 0x0001;

/// The signed analog value a held direction key applies to the stick axis.
/// N64 `OSContPad.stick_x/y` are `i8`, roughly +/-80 at full deflection on
/// real hardware; 80 is a firm, in-range default for keyboard play and the
/// full-scale value gamepad sticks are mapped onto.
pub const STICK_FULL: i8 = 80;

/// Every N64 controller button, in the overlay's display order. `Ord` so the
/// serialized TOML maps are stable (BTreeMap).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum N64Button {
    A,
    B,
    Start,
    Z,
    L,
    R,
    CUp,
    CDown,
    CLeft,
    CRight,
    DUp,
    DDown,
    DLeft,
    DRight,
}

impl N64Button {
    pub const ALL: [N64Button; 14] = [
        N64Button::A,
        N64Button::B,
        N64Button::Start,
        N64Button::Z,
        N64Button::L,
        N64Button::R,
        N64Button::CUp,
        N64Button::CDown,
        N64Button::CLeft,
        N64Button::CRight,
        N64Button::DUp,
        N64Button::DDown,
        N64Button::DLeft,
        N64Button::DRight,
    ];

    pub fn bit(self) -> u16 {
        match self {
            N64Button::A => BTN_A,
            N64Button::B => BTN_B,
            N64Button::Start => BTN_START,
            N64Button::Z => BTN_Z,
            N64Button::L => BTN_L,
            N64Button::R => BTN_R,
            N64Button::CUp => BTN_CUP,
            N64Button::CDown => BTN_CDOWN,
            N64Button::CLeft => BTN_CLEFT,
            N64Button::CRight => BTN_CRIGHT,
            N64Button::DUp => BTN_DUP,
            N64Button::DDown => BTN_DDOWN,
            N64Button::DLeft => BTN_DLEFT,
            N64Button::DRight => BTN_DRIGHT,
        }
    }

    /// Display label, matching the console's own labeling.
    pub fn label(self) -> &'static str {
        match self {
            N64Button::A => "A",
            N64Button::B => "B",
            N64Button::Start => "Start",
            N64Button::Z => "Z",
            N64Button::L => "L",
            N64Button::R => "R",
            N64Button::CUp => "C-Up",
            N64Button::CDown => "C-Down",
            N64Button::CLeft => "C-Left",
            N64Button::CRight => "C-Right",
            N64Button::DUp => "D-Up",
            N64Button::DDown => "D-Down",
            N64Button::DLeft => "D-Left",
            N64Button::DRight => "D-Right",
        }
    }
}

/// The four keyboard keys that drive the analog stick.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum StickDir {
    Up,
    Down,
    Left,
    Right,
}

impl StickDir {
    pub const ALL: [StickDir; 4] = [
        StickDir::Up,
        StickDir::Down,
        StickDir::Left,
        StickDir::Right,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StickDir::Up => "Stick Up",
            StickDir::Down => "Stick Down",
            StickDir::Left => "Stick Left",
            StickDir::Right => "Stick Right",
        }
    }

    /// `(dx, dy)` in `OSContPad.stick_x/stick_y` units (+x right, +y up).
    pub fn delta(self) -> (i8, i8) {
        match self {
            StickDir::Up => (0, STICK_FULL),
            StickDir::Down => (0, -STICK_FULL),
            StickDir::Left => (-STICK_FULL, 0),
            StickDir::Right => (STICK_FULL, 0),
        }
    }
}

/// User-editable input bindings + tuning. One binding per control
/// (ponytail: no alternate bindings; add a second map if anyone asks).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Keyboard key -> N64 button (stored button->key so each control has
    /// exactly one slot the overlay can rebind).
    pub keyboard: BTreeMap<N64Button, KeyCode>,
    /// Keyboard keys driving the analog stick.
    pub keyboard_stick: BTreeMap<StickDir, KeyCode>,
    /// Gamepad button -> N64 button. C-buttons additionally come from the
    /// right stick regardless of this map (see `gamepad.rs`).
    pub gamepad: BTreeMap<N64Button, gilrs::Button>,
    /// Radial deadzone on the gamepad left stick, as a fraction of full
    /// deflection (0.0..=0.5).
    pub deadzone: f32,
}

impl Default for InputConfig {
    fn default() -> Self {
        use gilrs::Button as GB;
        use KeyCode as K;
        let keyboard = BTreeMap::from([
            (N64Button::A, K::KeyX),
            (N64Button::B, K::KeyZ),
            (N64Button::Z, K::ShiftLeft),
            (N64Button::L, K::KeyQ),
            (N64Button::R, K::KeyE),
            (N64Button::Start, K::Enter),
            (N64Button::CUp, K::KeyI),
            (N64Button::CDown, K::KeyK),
            (N64Button::CLeft, K::KeyJ),
            (N64Button::CRight, K::KeyL),
            (N64Button::DUp, K::KeyT),
            (N64Button::DDown, K::KeyG),
            (N64Button::DLeft, K::KeyF),
            (N64Button::DRight, K::KeyH),
        ]);
        let keyboard_stick = BTreeMap::from([
            (StickDir::Up, K::KeyW),
            (StickDir::Down, K::KeyS),
            (StickDir::Left, K::KeyA),
            (StickDir::Right, K::KeyD),
        ]);
        // Standard-layout pad: face South/West = A/B (the common emulator
        // convention), left trigger = Z (it IS a trigger on the N64), bumpers
        // = L/R, d-pad = d-pad. C-buttons default to the RIGHT STICK (axis
        // path in gamepad.rs), so they carry no button binding here.
        let gamepad = BTreeMap::from([
            (N64Button::A, GB::South),
            (N64Button::B, GB::West),
            (N64Button::Start, GB::Start),
            (N64Button::Z, GB::LeftTrigger2),
            (N64Button::L, GB::LeftTrigger),
            (N64Button::R, GB::RightTrigger),
            (N64Button::DUp, GB::DPadUp),
            (N64Button::DDown, GB::DPadDown),
            (N64Button::DLeft, GB::DPadLeft),
            (N64Button::DRight, GB::DPadRight),
        ]);
        InputConfig {
            keyboard,
            keyboard_stick,
            gamepad,
            deadzone: 0.15,
        }
    }
}

impl InputConfig {
    /// `~/.config/fn64/input.toml` (platform equivalent via `dirs`).
    pub fn path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("fn64").join("input.toml"))
    }

    /// Load the saved config, or defaults. Never fatal: a missing file is
    /// the common first-run case, a malformed one is logged and replaced by
    /// defaults in memory (the file is only overwritten on the next save).
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<InputConfig>(&text) {
                Ok(mut config) => {
                    // A hand-edited deadzone outside the slider's range
                    // breaks apply_deadzone (1.0 divides by zero; >1.0 kills
                    // the stick). Clamp to what the UI allows.
                    config.deadzone = config.deadzone.clamp(0.0, 0.5);
                    println!("[fn64-shell] input config loaded from {}", path.display());
                    config
                }
                Err(e) => {
                    eprintln!(
                        "[fn64-shell] input config {} is malformed ({e}) -- using defaults",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// Persist to disk. Failures are logged, never fatal -- the in-memory
    /// config stays live for this session either way.
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            eprintln!("[fn64-shell] no config directory on this platform -- input config not saved");
            return;
        };
        let text = toml::to_string_pretty(self).expect("InputConfig serializes to TOML");
        let result = std::fs::create_dir_all(path.parent().expect("config path has a parent"))
            .and_then(|()| std::fs::write(&path, text));
        if let Err(e) = result {
            eprintln!(
                "[fn64-shell] failed to save input config {}: {e}",
                path.display()
            );
        }
    }

    /// The N64 button a keyboard key drives, or `None` if the key is a
    /// stick key or unbound.
    pub fn button_for_key(&self, key: KeyCode) -> Option<u16> {
        self.keyboard
            .iter()
            .find(|&(_, &k)| k == key)
            .map(|(&b, _)| b.bit())
    }

    /// The stick delta a keyboard key drives, or `None` for non-stick keys.
    pub fn stick_axis(&self, key: KeyCode) -> Option<(i8, i8)> {
        self.keyboard_stick
            .iter()
            .find(|&(_, &k)| k == key)
            .map(|(&d, _)| d.delta())
    }

    /// Bind `key` to a control, clearing any other control that key was
    /// bound to (a key drives at most one control).
    pub fn bind_key(&mut self, target: BindTarget, key: KeyCode) {
        self.keyboard.retain(|_, &mut k| k != key);
        self.keyboard_stick.retain(|_, &mut k| k != key);
        match target {
            BindTarget::Button(b) => {
                self.keyboard.insert(b, key);
            }
            BindTarget::Stick(d) => {
                self.keyboard_stick.insert(d, key);
            }
        }
    }

    /// Bind a gamepad button to an N64 button, clearing any other N64
    /// button it was bound to. (The analog stick isn't a gamepad-bind
    /// target: it's always the physical left stick.)
    pub fn bind_pad(&mut self, target: N64Button, button: gilrs::Button) {
        self.gamepad.retain(|_, &mut b| b != button);
        self.gamepad.insert(target, button);
    }
}

/// A rebindable keyboard slot: an N64 button or a stick direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindTarget {
    Button(N64Button),
    Stick(StickDir),
}

/// Map a human key name (as passed via `FN64_INPUT_PROBE`) to its
/// [`KeyCode`], for the headless input-seam self-test. Accepts the winit
/// `KeyCode` debug spellings (`Enter`, `KeyX`, `ArrowUp`, ...) plus a few
/// friendly aliases (`x`, `z`, `up`).
pub fn key_from_name(name: &str) -> Option<KeyCode> {
    Some(match name.trim() {
        "Enter" | "enter" | "start" | "Start" => KeyCode::Enter,
        "KeyX" | "x" | "X" | "a" | "A" => KeyCode::KeyX,
        "KeyZ" | "z" | "Z" | "b" | "B" => KeyCode::KeyZ,
        "KeyW" | "w" | "W" => KeyCode::KeyW,
        "KeyS" | "s" | "S" => KeyCode::KeyS,
        "KeyA" | "left_stick" => KeyCode::KeyA,
        "KeyD" => KeyCode::KeyD,
        "ArrowUp" | "up" | "Up" => KeyCode::ArrowUp,
        "ArrowDown" | "down" | "Down" => KeyCode::ArrowDown,
        "ArrowLeft" | "ArrowRight" => {
            if name.contains("Left") {
                KeyCode::ArrowLeft
            } else {
                KeyCode::ArrowRight
            }
        }
        "ShiftLeft" | "shift" | "z_trigger" => KeyCode::ShiftLeft,
        _ => return None,
    })
}

/// Accumulates the live pad state from key-down/key-up events, then resolves
/// it to the `(buttons, stick_x, stick_y)` tuple
/// `fn64_abi::set_controller_state` expects. Holding two opposing stick keys
/// cancels on that axis (matching how a real stick can't point both ways).
#[derive(Default, Debug, Clone)]
pub struct PadState {
    /// Bitwise-OR of every currently-held button key's bit.
    buttons: u16,
    /// Held stick contributions, summed then clamped at resolve time.
    stick_x: i32,
    stick_y: i32,
    /// Keys with a latched press, so a release without one (e.g. after
    /// `clear()` ran at overlay-open with the key still physically held)
    /// can't subtract a stick delta that was never added.
    held: std::collections::HashSet<KeyCode>,
}

impl PadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a key press/release under `config`'s bindings. `pressed=true`
    /// on key-down, `false` on key-up. Returns `true` if the resolved pad
    /// state changed, so a caller can skip a redundant
    /// `set_controller_state` call.
    pub fn apply(&mut self, config: &InputConfig, key: KeyCode, pressed: bool) -> bool {
        // The +/- stick accumulator below only balances if every release is
        // paired with a latched press: drop repeats and unpaired releases.
        if pressed {
            if !self.held.insert(key) {
                return false;
            }
        } else if !self.held.remove(&key) {
            return false;
        }
        let before = self.resolve();
        if let Some(bit) = config.button_for_key(key) {
            if pressed {
                self.buttons |= bit;
            } else {
                self.buttons &= !bit;
            }
        } else if let Some((dx, dy)) = config.stick_axis(key) {
            // +/- so releasing removes exactly what pressing added, and two
            // opposed keys held at once net to zero on that axis.
            let sign = if pressed { 1 } else { -1 };
            self.stick_x += sign * dx as i32;
            self.stick_y += sign * dy as i32;
        } else {
            return false;
        }
        self.resolve() != before
    }

    /// Drop all held state (used when the settings overlay opens, so keys
    /// held at that moment don't stay latched into the game).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The current `(buttons, stick_x, stick_y)` to hand to
    /// `fn64_abi::set_controller_state`. Stick axes are clamped to `i8` range.
    pub fn resolve(&self) -> (u16, i8, i8) {
        let clamp = |v: i32| v.clamp(i8::MIN as i32, i8::MAX as i32) as i8;
        (self.buttons, clamp(self.stick_x), clamp(self.stick_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_the_shipped_layout() {
        // The two face buttons a player will actually press first.
        let c = InputConfig::default();
        assert_eq!(c.button_for_key(KeyCode::KeyX), Some(BTN_A));
        assert_eq!(c.button_for_key(KeyCode::KeyZ), Some(BTN_B));
        assert_eq!(c.button_for_key(KeyCode::Enter), Some(BTN_START));
    }

    #[test]
    fn wasd_drives_stick() {
        let c = InputConfig::default();
        assert_eq!(c.stick_axis(KeyCode::KeyW), Some((0, STICK_FULL)));
        assert_eq!(c.stick_axis(KeyCode::KeyA), Some((-STICK_FULL, 0)));
        assert_eq!(c.stick_axis(KeyCode::KeyD), Some((STICK_FULL, 0)));
        // A stick key is NOT also a button.
        assert_eq!(c.button_for_key(KeyCode::KeyW), None);
    }

    #[test]
    fn press_start_sets_exactly_the_start_bit() {
        let c = InputConfig::default();
        let mut pad = PadState::new();
        assert!(pad.apply(&c, KeyCode::Enter, true));
        assert_eq!(pad.resolve(), (BTN_START, 0, 0));
        // Release clears it, back to neutral.
        assert!(pad.apply(&c, KeyCode::Enter, false));
        assert_eq!(pad.resolve(), (0, 0, 0));
    }

    #[test]
    fn two_buttons_or_together() {
        let c = InputConfig::default();
        let mut pad = PadState::new();
        pad.apply(&c, KeyCode::KeyX, true); // A
        pad.apply(&c, KeyCode::ShiftLeft, true); // Z
        assert_eq!(pad.resolve(), (BTN_A | BTN_Z, 0, 0));
    }

    #[test]
    fn opposing_stick_keys_cancel() {
        let c = InputConfig::default();
        let mut pad = PadState::new();
        pad.apply(&c, KeyCode::KeyA, true);
        pad.apply(&c, KeyCode::KeyD, true);
        // Left (-80) + Right (+80) net to zero on X.
        assert_eq!(pad.resolve(), (0, 0, 0));
        // Releasing right leaves a clean full-left deflection.
        pad.apply(&c, KeyCode::KeyD, false);
        assert_eq!(pad.resolve(), (0, -STICK_FULL, 0));
    }

    #[test]
    fn unbound_key_changes_nothing() {
        let c = InputConfig::default();
        let mut pad = PadState::new();
        assert!(!pad.apply(&c, KeyCode::F12, true));
        assert_eq!(pad.resolve(), (0, 0, 0));
    }

    #[test]
    fn rebind_steals_the_key_from_its_old_control() {
        let mut c = InputConfig::default();
        // X currently drives A; rebind X to B.
        c.bind_key(BindTarget::Button(N64Button::B), KeyCode::KeyX);
        assert_eq!(c.button_for_key(KeyCode::KeyX), Some(BTN_B));
        // A no longer has a keyboard binding at all.
        assert!(!c.keyboard.contains_key(&N64Button::A));
        // Rebinding a stick key clears it from the stick map too.
        c.bind_key(BindTarget::Button(N64Button::A), KeyCode::KeyW);
        assert_eq!(c.button_for_key(KeyCode::KeyW), Some(BTN_A));
        assert_eq!(c.stick_axis(KeyCode::KeyW), None);
    }

    #[test]
    fn release_without_latched_press_is_ignored() {
        // The overlay-open path clears the pad while a stick key may still
        // be physically held; the eventual release after the overlay closes
        // must not drive the stick negative.
        let c = InputConfig::default();
        let mut pad = PadState::new();
        pad.apply(&c, KeyCode::KeyW, true);
        pad.clear();
        assert!(!pad.apply(&c, KeyCode::KeyW, false));
        assert_eq!(pad.resolve(), (0, 0, 0));
    }

    #[test]
    fn repeated_press_does_not_accumulate() {
        let c = InputConfig::default();
        let mut pad = PadState::new();
        pad.apply(&c, KeyCode::KeyW, true);
        assert!(!pad.apply(&c, KeyCode::KeyW, true));
        // One release fully returns to neutral.
        pad.apply(&c, KeyCode::KeyW, false);
        assert_eq!(pad.resolve(), (0, 0, 0));
    }

    #[test]
    fn config_roundtrips_through_toml() {
        let mut c = InputConfig {
            deadzone: 0.25,
            ..Default::default()
        };
        c.bind_key(BindTarget::Button(N64Button::A), KeyCode::Space);
        c.bind_pad(N64Button::CDown, gilrs::Button::East);
        let text = toml::to_string_pretty(&c).expect("serializes");
        let back: InputConfig = toml::from_str(&text).expect("deserializes");
        assert_eq!(back.deadzone, 0.25);
        assert_eq!(back.button_for_key(KeyCode::Space), Some(BTN_A));
        assert_eq!(back.gamepad[&N64Button::CDown], gilrs::Button::East);
    }

    #[test]
    fn partial_config_file_fills_missing_fields_with_defaults() {
        // A user hand-editing the TOML shouldn't lose everything they
        // didn't mention: serde(default) fills the rest.
        let back: InputConfig = toml::from_str("deadzone = 0.3\n").expect("deserializes");
        assert_eq!(back.deadzone, 0.3);
        assert_eq!(back.button_for_key(KeyCode::KeyX), Some(BTN_A));
    }
}
