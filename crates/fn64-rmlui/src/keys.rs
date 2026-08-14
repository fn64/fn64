//! RmlUi's `Rml::Input::KeyIdentifier`/`KeyModifier` values, transcribed from
//! the enum in `Include/RmlUi/Core/Input.h` of the vendored MIT RmlUi
//! checkout -- the subset of keys fn64's settings menu and its
//! keyboard/gamepad-navigation forwarding actually need. See the bottom of
//! this file for why the `winit::keyboard::KeyCode` -> `KeyIdentifier`
//! translation itself lives in the shell binary, not here.
//!
//! The shim's C ABI (`fn64_rmlui_context_process_key_down/up`) takes these as
//! plain `i32`s, matching `Rml::Input::KeyIdentifier`'s underlying
//! `unsigned char` values exactly -- see `fn64_rmlui_shim.h`'s own comment
//! pointing callers at `Input.h` for the real enum.

/// `Rml::Input::KeyIdentifier` values needed here. Not exhaustive: only the
/// keys fn64's settings menu (text-free -- no text fields) and its
/// gamepad-to-key navigation synthesis use.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum KeyIdentifier {
    Tab = 70,
    Return = 72,
    Escape = 81,
    Left = 90,
    Up = 91,
    Right = 92,
    Down = 93,
    F1 = 107,
}

impl KeyIdentifier {
    pub const fn as_i32(self) -> i32 {
        self as i32
    }
}

/// `Rml::Input::KeyModifier` bitflags.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyModifiers(i32);

impl KeyModifiers {
    pub const NONE: Self = Self(0);
    const CTRL: i32 = 1 << 0;
    const SHIFT: i32 = 1 << 1;
    const ALT: i32 = 1 << 2;
    const META: i32 = 1 << 3;

    pub const fn new(ctrl: bool, shift: bool, alt: bool, meta: bool) -> Self {
        let mut bits = 0;
        if ctrl {
            bits |= Self::CTRL;
        }
        if shift {
            bits |= Self::SHIFT;
        }
        if alt {
            bits |= Self::ALT;
        }
        if meta {
            bits |= Self::META;
        }
        Self(bits)
    }

    pub(crate) const fn as_i32(self) -> i32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned against `Include/RmlUi/Core/Input.h`'s real
    /// `Rml::Input::KeyIdentifier` values (checked against the vendored MIT
    /// checkout while writing this module) so a future edit to this list
    /// cannot silently drift from what the shim's C++ side actually expects
    /// -- there is no compile-time link between this Rust enum and RmlUi's
    /// C++ one, only this test.
    #[test]
    fn key_identifiers_match_rmlui_input_h() {
        assert_eq!(KeyIdentifier::Tab.as_i32(), 70);
        assert_eq!(KeyIdentifier::Return.as_i32(), 72);
        assert_eq!(KeyIdentifier::Escape.as_i32(), 81);
        assert_eq!(KeyIdentifier::Left.as_i32(), 90);
        assert_eq!(KeyIdentifier::Up.as_i32(), 91);
        assert_eq!(KeyIdentifier::Right.as_i32(), 92);
        assert_eq!(KeyIdentifier::Down.as_i32(), 93);
        assert_eq!(KeyIdentifier::F1.as_i32(), 107);
    }

    /// Pinned against `Include/RmlUi/Core/Input.h`'s real `KeyModifier`
    /// bitflag values, same rationale as the identifier test above.
    #[test]
    fn key_modifiers_match_rmlui_input_h_bits() {
        assert_eq!(KeyModifiers::NONE.as_i32(), 0);
        assert_eq!(KeyModifiers::new(true, false, false, false).as_i32(), 1);
        assert_eq!(KeyModifiers::new(false, true, false, false).as_i32(), 2);
        assert_eq!(KeyModifiers::new(false, false, true, false).as_i32(), 4);
        assert_eq!(KeyModifiers::new(false, false, false, true).as_i32(), 8);
        assert_eq!(KeyModifiers::new(true, true, true, true).as_i32(), 15);
    }
}

// A `winit::keyboard::KeyCode` -> `KeyIdentifier` translation deliberately
// does NOT live here: `fn64-rmlui` is a member of the main fn64 Cargo
// workspace, while `winit` is only ever a dependency of the two standalone
// shell packages (`crates/fn64-shell`, `examples/wm2000-block-boot`), each
// its OWN separate Cargo workspace pinning its own `winit` version. Taking a
// `winit` dependency here would (a) pull windowing into a crate that has
// nothing to do with it and (b) still not let a shell binary's `winit`
// version resolve against this crate's -- they are different workspaces
// with no shared lockfile. The translation instead lives in the shell
// binary that already depends on `winit` and already owns this exact
// problem for its own `PadState`/`InputConfig` (see
// `examples/wm2000-block-boot/src/shell.rs`'s settings-menu integration).
