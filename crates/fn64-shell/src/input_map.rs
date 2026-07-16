//! Keyboard -> N64 controller mapping, factored out of the window loop so it
//! can be unit-tested without a live event loop or a linked game.
//!
//! The button bits are the N64 `OSContPad.button` bitmask
//! (`oot-decomp/include/controller.h:4-17`, the same table
//! `fn64_abi::set_controller_state` documents): `A=0x8000`, `B=0x4000`,
//! `Z=0x2000`, `START=0x1000`, d-pad `0x0800..0x0100`, `L=0x0020`,
//! `R=0x0010`, C-buttons `0x0008..0x0001`.

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
/// real hardware; 80 is a firm, in-range default for keyboard play.
pub const STICK_FULL: i8 = 80;

/// Map a physical key to the N64 controller button bit it drives, or `None`
/// if the key is an analog-stick key (handled by [`stick_axis`]) or unbound.
///
/// The default layout:
/// - **Analog stick**: arrow keys OR W/A/S/D (see [`stick_axis`]).
/// - **A** = X, **B** = Z (the common emulator face-button convention).
/// - **Z (trigger)** = left Shift, **L** = Q, **R** = E.
/// - **Start** = Enter.
/// - **C-buttons** = I/K/J/L (up/down/left/right).
/// - **D-pad** = T/G/F/H (up/down/left/right) -- kept off the WASD/arrow keys
///   so it doesn't collide with the analog stick.
pub fn button_for_key(key: KeyCode) -> Option<u16> {
    Some(match key {
        KeyCode::KeyX => BTN_A,
        KeyCode::KeyZ => BTN_B,
        KeyCode::ShiftLeft => BTN_Z,
        KeyCode::KeyQ => BTN_L,
        KeyCode::KeyE => BTN_R,
        KeyCode::Enter => BTN_START,
        KeyCode::KeyI => BTN_CUP,
        KeyCode::KeyK => BTN_CDOWN,
        KeyCode::KeyJ => BTN_CLEFT,
        KeyCode::KeyL => BTN_CRIGHT,
        KeyCode::KeyT => BTN_DUP,
        KeyCode::KeyG => BTN_DDOWN,
        KeyCode::KeyF => BTN_DLEFT,
        KeyCode::KeyH => BTN_DRIGHT,
        _ => return None,
    })
}

/// The analog-stick axis a key drives, as `(dx, dy)` deltas in
/// `OSContPad.stick_x/stick_y` units (N64 stick: +x right, +y up). `None` for
/// non-stick keys. Arrow keys and W/A/S/D both map to the stick so either
/// hand position works.
pub fn stick_axis(key: KeyCode) -> Option<(i8, i8)> {
    Some(match key {
        KeyCode::ArrowUp | KeyCode::KeyW => (0, STICK_FULL),
        KeyCode::ArrowDown | KeyCode::KeyS => (0, -STICK_FULL),
        KeyCode::ArrowLeft | KeyCode::KeyA => (-STICK_FULL, 0),
        KeyCode::ArrowRight | KeyCode::KeyD => (STICK_FULL, 0),
        _ => return None,
    })
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
}

impl PadState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a key press/release. `pressed=true` on key-down, `false` on
    /// key-up. Returns `true` if the resolved pad state changed, so a caller
    /// can skip a redundant `set_controller_state` call.
    pub fn apply(&mut self, key: KeyCode, pressed: bool) -> bool {
        let before = self.resolve();
        if let Some(bit) = button_for_key(key) {
            if pressed {
                self.buttons |= bit;
            } else {
                self.buttons &= !bit;
            }
        } else if let Some((dx, dy)) = stick_axis(key) {
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
    fn x_maps_to_a_z_maps_to_b() {
        // The two face buttons a player will actually press first.
        assert_eq!(button_for_key(KeyCode::KeyX), Some(BTN_A));
        assert_eq!(button_for_key(KeyCode::KeyZ), Some(BTN_B));
        assert_eq!(button_for_key(KeyCode::Enter), Some(BTN_START));
    }

    #[test]
    fn arrows_and_wasd_both_drive_stick() {
        assert_eq!(stick_axis(KeyCode::ArrowUp), Some((0, STICK_FULL)));
        assert_eq!(stick_axis(KeyCode::KeyW), Some((0, STICK_FULL)));
        assert_eq!(stick_axis(KeyCode::ArrowLeft), Some((-STICK_FULL, 0)));
        assert_eq!(stick_axis(KeyCode::KeyD), Some((STICK_FULL, 0)));
        // A stick key is NOT also a button.
        assert_eq!(button_for_key(KeyCode::ArrowUp), None);
    }

    #[test]
    fn press_start_sets_exactly_the_start_bit() {
        let mut pad = PadState::new();
        assert!(pad.apply(KeyCode::Enter, true));
        assert_eq!(pad.resolve(), (BTN_START, 0, 0));
        // Release clears it, back to neutral.
        assert!(pad.apply(KeyCode::Enter, false));
        assert_eq!(pad.resolve(), (0, 0, 0));
    }

    #[test]
    fn two_buttons_or_together() {
        let mut pad = PadState::new();
        pad.apply(KeyCode::KeyX, true); // A
        pad.apply(KeyCode::ShiftLeft, true); // Z
        assert_eq!(pad.resolve(), (BTN_A | BTN_Z, 0, 0));
    }

    #[test]
    fn opposing_stick_keys_cancel() {
        let mut pad = PadState::new();
        pad.apply(KeyCode::ArrowLeft, true);
        pad.apply(KeyCode::ArrowRight, true);
        // Left (-80) + Right (+80) net to zero on X.
        assert_eq!(pad.resolve(), (0, 0, 0));
        // Releasing right leaves a clean full-left deflection.
        pad.apply(KeyCode::ArrowRight, false);
        assert_eq!(pad.resolve(), (0, -STICK_FULL, 0));
    }

    #[test]
    fn full_up_deflection() {
        // W = up = +Y on the N64 stick, so resolve() -> (buttons, x, y) is
        // (0, 0, +STICK_FULL).
        let mut pad = PadState::new();
        assert!(pad.apply(KeyCode::KeyW, true));
        assert_eq!(pad.resolve(), (0, 0, STICK_FULL));
    }

    #[test]
    fn unbound_key_changes_nothing() {
        let mut pad = PadState::new();
        assert!(!pad.apply(KeyCode::F12, true));
        assert_eq!(pad.resolve(), (0, 0, 0));
    }
}
