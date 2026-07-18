//! Gamepad input via `gilrs` (issue #4), merged with the keyboard `PadState`
//! in the window loop. The LEFT stick is always the N64 analog stick (with
//! the config's radial deadzone); the RIGHT stick always drives the
//! C-buttons past a fixed threshold; everything else goes through the
//! rebindable `InputConfig::gamepad` map.

use crate::input_map::{InputConfig, BTN_CDOWN, BTN_CLEFT, BTN_CRIGHT, BTN_CUP, STICK_FULL};
use gilrs::{Axis, Event, EventType, Gilrs};

/// Right-stick deflection past which a C-button registers. Fixed: C-buttons
/// are digital, 0.5 is the conventional half-travel threshold.
const C_STICK_THRESHOLD: f32 = 0.5;

pub struct Gamepads {
    /// `None` when gilrs failed to initialize (headless CI, missing OS
    /// backend) -- the shell stays keyboard-only, loudly, not fatally.
    gilrs: Option<Gilrs>,
    /// The most recently active gamepad -- the one whose state we read.
    /// First press wins; a different pad becomes active by being used.
    active: Option<gilrs::GamepadId>,
    /// Most recent button press drained by `poll`, for the overlay's
    /// press-to-bind capture. Cleared by `take_pressed`.
    last_pressed: Option<gilrs::Button>,
}

impl Gamepads {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                let names: Vec<String> =
                    g.gamepads().map(|(_, p)| p.name().to_string()).collect();
                if names.is_empty() {
                    println!("[fn64-shell] gamepads: none connected (hotplug supported)");
                } else {
                    println!("[fn64-shell] gamepads: {}", names.join(", "));
                }
                Some(g)
            }
            Err(e) => {
                eprintln!("[fn64-shell] gamepad support unavailable ({e}) -- keyboard only");
                None
            }
        };
        Gamepads {
            gilrs,
            active: None,
            last_pressed: None,
        }
    }

    /// Drain pending gilrs events. Call once per event-loop tick, BEFORE
    /// reading state -- gilrs's cached gamepad state only advances when
    /// events are consumed.
    pub fn poll(&mut self) {
        let Some(gilrs) = self.gilrs.as_mut() else {
            return;
        };
        while let Some(Event { id, event, .. }) = gilrs.next_event() {
            match event {
                EventType::ButtonPressed(button, _) => {
                    self.active = Some(id);
                    self.last_pressed = Some(button);
                }
                EventType::Connected => {
                    let name = gilrs.gamepad(id).name().to_string();
                    println!("[fn64-shell] gamepad connected: {name}");
                    self.active.get_or_insert(id);
                }
                EventType::Disconnected => {
                    println!("[fn64-shell] gamepad disconnected");
                    if self.active == Some(id) {
                        self.active = None;
                    }
                }
                _ => {}
            }
        }
    }

    /// The most recent button press since the last call (capture mode).
    pub fn take_pressed(&mut self) -> Option<gilrs::Button> {
        self.last_pressed.take()
    }

    /// Raw left-stick position in [-1, 1] per axis, for the overlay's live
    /// scope. `(0, 0)` with no active pad.
    pub fn raw_stick(&self) -> (f32, f32) {
        let Some(pad) = self.active_pad() else {
            return (0.0, 0.0);
        };
        (
            pad.value(Axis::LeftStickX),
            pad.value(Axis::LeftStickY),
        )
    }

    /// Resolve the active gamepad against `config`:
    /// `(buttons, stick_x, stick_y)` in `set_controller_state` units.
    /// Neutral when no pad is connected.
    pub fn resolve(&self, config: &InputConfig) -> (u16, i8, i8) {
        let Some(pad) = self.active_pad() else {
            return (0, 0, 0);
        };

        let mut buttons = 0u16;
        for (&n64, &host) in &config.gamepad {
            if pad.is_pressed(host) {
                buttons |= n64.bit();
            }
        }

        // Right stick -> C-buttons (digital, fixed threshold). Additive with
        // any C bindings in the map, same as keyboard/gamepad merging.
        let cx = pad.value(Axis::RightStickX);
        let cy = pad.value(Axis::RightStickY);
        if cy > C_STICK_THRESHOLD {
            buttons |= BTN_CUP;
        }
        if cy < -C_STICK_THRESHOLD {
            buttons |= BTN_CDOWN;
        }
        if cx < -C_STICK_THRESHOLD {
            buttons |= BTN_CLEFT;
        }
        if cx > C_STICK_THRESHOLD {
            buttons |= BTN_CRIGHT;
        }

        let (sx, sy) = apply_deadzone(
            pad.value(Axis::LeftStickX),
            pad.value(Axis::LeftStickY),
            config.deadzone,
        );
        (buttons, sx, sy)
    }

    /// True when a pad bound to `button` is currently held -- used by the
    /// overlay to preview bindings.
    pub fn is_pressed(&self, button: gilrs::Button) -> bool {
        self.active_pad().is_some_and(|p| p.is_pressed(button))
    }

    /// Human name of the active pad, for the overlay header.
    pub fn active_name(&self) -> Option<String> {
        self.active_pad().map(|p| p.name().to_string())
    }

    fn active_pad(&self) -> Option<gilrs::Gamepad<'_>> {
        let gilrs = self.gilrs.as_ref()?;
        // Fall back to any connected pad before its first button press, so
        // the stick works immediately on launch.
        let id = self
            .active
            .or_else(|| gilrs.gamepads().next().map(|(id, _)| id))?;
        let pad = gilrs.connected_gamepad(id)?;
        Some(pad)
    }
}

/// Radial deadzone + rescale: inside the zone -> neutral; outside, the
/// remaining travel is rescaled to the full range so deflection stays
/// continuous at the zone edge, then mapped to N64 `+/-STICK_FULL`.
pub fn apply_deadzone(x: f32, y: f32, deadzone: f32) -> (i8, i8) {
    let mag = (x * x + y * y).sqrt();
    if mag <= deadzone || mag == 0.0 {
        return (0, 0);
    }
    let scale = ((mag - deadzone) / (1.0 - deadzone)).min(1.0) / mag;
    let to_n64 = |v: f32| (v * scale * STICK_FULL as f32).round() as i8;
    (to_n64(x), to_n64(y))
}

/// The un-quantized version of [`apply_deadzone`], for the overlay's scope
/// dot (drawn in [-1, 1] space, not N64 units).
pub fn apply_deadzone_f(x: f32, y: f32, deadzone: f32) -> (f32, f32) {
    let mag = (x * x + y * y).sqrt();
    if mag <= deadzone || mag == 0.0 {
        return (0.0, 0.0);
    }
    let scale = ((mag - deadzone) / (1.0 - deadzone)).min(1.0) / mag;
    (x * scale, y * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadzone_zeroes_small_input_and_keeps_full_deflection() {
        // Inside the zone: neutral.
        assert_eq!(apply_deadzone(0.1, 0.05, 0.15), (0, 0));
        // Full deflection still reaches +/-80.
        assert_eq!(apply_deadzone(1.0, 0.0, 0.15), (STICK_FULL, 0));
        assert_eq!(apply_deadzone(0.0, -1.0, 0.15), (0, -STICK_FULL));
        // Just past the zone edge: small but nonzero, continuous.
        let (sx, _) = apply_deadzone(0.2, 0.0, 0.15);
        assert!((1..=8).contains(&sx), "edge-of-zone x was {sx}");
    }

    #[test]
    fn zero_deadzone_is_identity_scaling() {
        assert_eq!(apply_deadzone(0.5, 0.0, 0.0), (40, 0));
    }
}
