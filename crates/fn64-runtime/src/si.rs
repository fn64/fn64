//! Minimal SI/PIF model: `__osSiRawStartDma`'s controller-probe/read path.
//!
//! ## Provenance
//!
//! Public libultra manual (Controller Manager section: `osContInit`,
//! `osContStartQuery`/`osContGetQuery` PIF-format probe response,
//! `osContStartReadData`/`osContGetReadData` button/stick response) plus
//! N64 hardware public documentation of the PIF RAM command-byte protocol
//! (0xFF status-query command -> 3-byte response
//! `[type_hi, type_lo, status]`; 0x01 read-buttons command -> 4-byte
//! response). No GPL runtime SI/PIF implementation was read.
//!
//! ## Scope
//!
//! This is deliberately the SMALLEST model that answers "one standard
//! controller, no pak" the task asks for: a single port (port 0) reports
//! present with a standard N64 controller (`CONT_TYPE_STANDARD = 0x0500`)
//! and no accessory (status byte clears `CONT_CARD_ON`/pak-present bits);
//! ports 1-3 report not-present (status byte's `CONT_ABSENT` bit set), which
//! is the documented PIF response for an empty port. `osContStartReadData`'s
//! button/stick response is modeled as "everything neutral, no buttons
//! held" -- a real, honest idle-controller state, not a fabricated
//! button-mash.
//!
//! This module has no host input-device polling of its own (that's
//! `fn64-shell`'s wave-5 concern per `docs/DESIGN.md` section 1) -- it is
//! the PIF-format RESPONSE SHAPE only, parameterized by which ports are
//! "connected" so a future real-input wave can flip that without touching
//! this module's protocol-formatting logic.

/// PIF-format controller-type response for a standard N64 controller (no
/// Controller Pak, no Rumble Pak accessory) -- the public libultra manual's
/// documented `CONT_TYPE_STANDARD` constant, high byte first per PIF wire
/// order.
pub const CONT_TYPE_STANDARD: u16 = 0x0500;

/// Status byte bits (public libultra manual, `contquery.h`/`libultra.h`
/// documented constants): `CONT_CARD_ON` (accessory present) and
/// `CONT_ABSENT` (no controller in this port at all).
pub const CONT_CARD_ON: u8 = 0x01;
#[allow(dead_code)]
pub const CONT_ADDR_CRC_ER: u8 = 0x02;
pub const CONT_ABSENT: u8 = 0x80;

/// One port's static identity for this milestone's model: present-as-a-
/// standard-controller-with-no-pak, or absent. Real hardware can report
/// richer states (mouse, VRU, absent-but-was-present-last-poll); not
/// modeled here since no boot-rung evidence exercises them yet.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PortState {
    StandardControllerNoPak,
    Absent,
}

/// A single controller's live button/stick state -- the four values a real
/// `osContGetReadData` read-buttons response carries, in the game-visible
/// `OSContPad` layout (`oot-decomp/include/ultra64/controller.h:127`:
/// `button` u16, `stick_x`/`stick_y` s8, `errno` u8). This is the input SEAM:
/// a host harness (`fn64-shell`, or a scripted boot harness) writes this and
/// the PIF read-data response reflects it, without the SI/PIF protocol-
/// formatting code below needing to know where the bytes came from. Defaults
/// to a genuinely idle controller (no buttons, sticks centered) so an
/// un-driven boot sees an honest neutral pad, not a fabricated input.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ContInput {
    /// The 16 N64 face/shoulder/dpad button bits (`OSContPad.button`), high
    /// byte first per the PIF wire order the read-data response uses.
    pub button: u16,
    /// Analog stick X, signed (`OSContPad.stick_x`), centered at 0.
    pub stick_x: i8,
    /// Analog stick Y, signed (`OSContPad.stick_y`), centered at 0.
    pub stick_y: i8,
}

/// The minimal PIF/SI model: which of the 4 controller ports are populated.
/// Per the task ("minimal PIF model reporting one standard controller, no
/// pak"), port 0 defaults to `StandardControllerNoPak`, ports 1-3 to
/// `Absent` -- the smallest configuration that answers "what does the game
/// see when it probes" honestly for a single-player boot.
#[derive(Copy, Clone, Debug)]
pub struct PifModel {
    ports: [PortState; 4],
    /// Live button/stick state per port -- what `read_data_response` returns.
    /// The input seam: a host harness sets these (`set_input`) and the game's
    /// `osContGetReadData` sees them. Ports with no controller keep the idle
    /// default, which is inert anyway (the game checks `errno`/absence first).
    inputs: [ContInput; 4],
}

impl Default for PifModel {
    fn default() -> Self {
        PifModel {
            ports: [
                PortState::StandardControllerNoPak,
                PortState::Absent,
                PortState::Absent,
                PortState::Absent,
            ],
            inputs: [ContInput::default(); 4],
        }
    }
}

impl PifModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn port_state(&self, port: usize) -> PortState {
        self.ports.get(port).copied().unwrap_or(PortState::Absent)
    }

    /// Feed a controller's live button/stick state for `port` -- the host-
    /// facing half of the input seam. A subsequent `read_data_response(port)`
    /// (i.e. the game's next `osContGetReadData`) reflects it. An out-of-range
    /// port is ignored (there are only 4 physical ports); this never touches
    /// the port's present/absent identity, only what a present controller
    /// reports.
    pub fn set_input(&mut self, port: usize, input: ContInput) {
        if let Some(slot) = self.inputs.get_mut(port) {
            *slot = input;
        }
    }

    /// The current input state for `port` (idle default if never set / out of
    /// range) -- lets a harness read back what it fed.
    pub fn input(&self, port: usize) -> ContInput {
        self.inputs.get(port).copied().unwrap_or_default()
    }

    /// `osContStartQuery`/`__osSiRawStartDma`'s status-query response for
    /// `port`: the 3-byte PIF-format `[type_hi, type_lo, status]` a real
    /// 0xFF PIF command produces (public libultra manual). `Absent` reports
    /// `CONT_ABSENT` set and a zeroed type, matching the documented
    /// empty-port response rather than a fabricated controller identity.
    pub fn query_response(&self, port: usize) -> [u8; 3] {
        match self.port_state(port) {
            PortState::StandardControllerNoPak => {
                let ty = CONT_TYPE_STANDARD;
                [(ty >> 8) as u8, (ty & 0xFF) as u8, 0]
            }
            PortState::Absent => [0, 0, CONT_ABSENT],
        }
    }

    /// `osContStartReadData`/`osContGetReadData`'s button/stick response for
    /// `port`: the documented 4-byte `[button_hi, button_lo, stick_x, stick_y]`
    /// PIF response shape, filled from the live input state a host harness fed
    /// via `set_input` (idle/all-neutral by default -- an honest un-driven
    /// controller, not a fabricated button-mash). `Absent` ports still return
    /// their (idle) input here; a caller must check `query_response`'s
    /// `CONT_ABSENT` bit / the pad's `errno` first, matching real libultra's
    /// own documented query-before-read usage pattern.
    pub fn read_data_response(&self, port: usize) -> [u8; 4] {
        let input = self.input(port);
        let [bhi, blo] = input.button.to_be_bytes();
        [bhi, blo, input.stick_x as u8, input.stick_y as u8]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_zero_reports_standard_controller_no_pak() {
        let pif = PifModel::new();
        assert_eq!(pif.port_state(0), PortState::StandardControllerNoPak);
        let resp = pif.query_response(0);
        assert_eq!(resp, [0x05, 0x00, 0x00]);
        assert_eq!(resp[2] & CONT_CARD_ON, 0, "no pak present");
        assert_eq!(resp[2] & CONT_ABSENT, 0, "port 0 is populated");
    }

    #[test]
    fn ports_1_to_3_report_absent() {
        let pif = PifModel::new();
        for port in 1..4 {
            assert_eq!(pif.port_state(port), PortState::Absent);
            let resp = pif.query_response(port);
            assert_eq!(resp[2] & CONT_ABSENT, CONT_ABSENT);
        }
    }

    #[test]
    fn idle_read_data_is_all_neutral() {
        let pif = PifModel::new();
        assert_eq!(pif.read_data_response(0), [0, 0, 0, 0]);
    }

    #[test]
    fn set_input_flows_into_read_data_response() {
        // The input seam: what a host harness feeds is what the read-data
        // response reports, in the `[button_hi, button_lo, stick_x, stick_y]`
        // PIF wire shape.
        let mut pif = PifModel::new();
        pif.set_input(
            0,
            ContInput {
                button: 0x1234,
                stick_x: -40,
                stick_y: 55,
            },
        );
        assert_eq!(
            pif.read_data_response(0),
            [0x12, 0x34, (-40i8) as u8, 55u8],
            "read-data must reflect the fed input, button high-byte first"
        );
        // Un-driven ports stay idle -- feeding port 0 doesn't bleed into 1.
        assert_eq!(pif.read_data_response(1), [0, 0, 0, 0]);
    }

    #[test]
    fn set_input_out_of_range_port_is_ignored_not_a_panic() {
        let mut pif = PifModel::new();
        pif.set_input(
            9,
            ContInput {
                button: 0xFFFF,
                stick_x: 1,
                stick_y: 1,
            },
        );
        // No panic, and no real port was disturbed.
        assert_eq!(pif.read_data_response(0), [0, 0, 0, 0]);
    }

    #[test]
    fn out_of_range_port_reports_absent_not_a_panic() {
        // 4 ports max on real hardware; a caller asking about port 7 is a
        // caller bug, but this is a pure query API with no rdram/DMA side
        // effect -- returning Absent (rather than panicking) matches "no
        // controller there" being a true statement for a nonexistent port,
        // not a silent success/no-op over guest-visible state.
        let pif = PifModel::new();
        assert_eq!(pif.port_state(7), PortState::Absent);
    }
}
