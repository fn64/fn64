//! `Peripherals`: VI/SI/RSP host-side hardware-model state, extracted from
//! `Executor` so the executor's own surface stays scheduling + queues +
//! timers + the single `inject_event` door.
//!
//! ## Why this split, and why now
//!
//! Before this module existed, `Executor` directly owned `vi: ViState`,
//! `retrace: Option<RetraceSchedule>`, `pif: PifModel`, and `tasks: TaskLog`,
//! with every VI/SI/RSP-facing method (`vi_set_mode`, `vi_swap_buffer`,
//! `pif()`, `submit_task`, `arm_retrace`, etc.) implemented directly as
//! `impl Executor` methods that touched those fields. That made `Executor`
//! a god-object: scheduling/queue/timer logic (its actual job, per
//! `docs/DESIGN.md` section 2) sat in the same `impl` block, and often the
//! same file region, as VI mode-setting and PIF-probe formatting, which have
//! nothing to do with the single-runnable-coroutine invariant `Executor`
//! exists to enforce.
//!
//! This is a **pure structural move, zero behavior change**: every method
//! below has the exact same body it had as an `Executor` method (same field
//! reads/writes, same trace-recording call shape), just relocated. `Executor`
//! now holds one `peripherals: Peripherals` field and re-exposes the same
//! public methods as thin one-line delegations (see `executor.rs`'s "VI"/
//! "SI/PIF"/"RSP task submission" sections) -- callers in `fn64-abi` did not
//! change at all, which is the point: the ABI surface this crate promises
//! (`docs/DESIGN.md` section 1: "fn64-abi... every symbol here is a
//! signature-and-marshalling adapter") is unaffected by where the
//! implementation actually lives.
//!
//! `event_table` (the general `osSetEventMesg`-populated `OS_EVENT_*` ->
//! `(queue, msg)` table) deliberately STAYS on `Executor`, not here: it is
//! genuinely shared scheduling machinery (both a guest `osSetEventMesg`
//! registration and the VI retrace ticker's `OS_EVENT_VI` lookup go through
//! it, and `inject_event`'s `ExternalEvent::OsEvent` arm is peripheral-
//! agnostic -- it has no idea whether a given event code "belongs" to VI, SI,
//! or something else entirely). Moving it here would just relocate the
//! god-object problem one file over; it belongs with the run queue/blocked-
//! list machinery it's peered with in `Executor`.
//!
//! Trace recording (`TraceLog`) also stays on `Executor`: it needs
//! `sim_time`, which is `Executor`'s virtual clock, not a peripheral's own
//! state. `Peripherals`' methods that used to also call
//! `self.trace.record(...)` (`vi_swap_buffer`, `submit_task`) now return the
//! plain data the caller needs to record that same event itself -- see each
//! method's doc comment for the exact shape carried over.

use crate::device::Cycles;
use crate::pfs::{ControllerPak, ControllerPakEvidenceSnapshot};
use crate::rdram::RdramAddr;
use crate::rsp::{OsTaskHeader, TaskLog};
use crate::si::{PifEvidenceSnapshot, PifModel, PortState, RumbleError};
use crate::transfer_pak::{
    HostUnixNanos, Mbc3BatteryMetadata, Mbc3BatteryRestore, TransferPak, TransferPakError,
    TransferPakEvidenceSnapshot,
};
use crate::vi::{RetraceSchedule, RetraceScheduleEvidenceSnapshot, ViEvidenceSnapshot, ViState};
use crate::voice::{VoiceEvidenceSnapshot, VoiceUnit};

/// Controller/accessory family whose successful guest operation reached the
/// authoritative ABI/device boundary. This is historical release evidence,
/// not future device state, so it is intentionally absent from
/// [`PeripheralsEvidenceSnapshot`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControllerOperationDevice {
    StandardController,
    RumblePak,
    TransferPak,
    VoiceRecognitionUnit,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ControllerOperationKind {
    Read,
    Write,
    Control,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ControllerOperationEvent {
    pub at: Cycles,
    pub port: u8,
    pub device: ControllerOperationDevice,
    pub operation: ControllerOperationKind,
}

/// VI (video interface) + SI/PIF (controller probe) + RSP (task submission)
/// host-side hardware-model state. See module doc for why these three are
/// grouped: they are the peripherals `docs/DESIGN.md` section 1/2 describes
/// as host-driven models with no coroutine of their own, as distinct from
/// `Executor`'s scheduling/queue/timer state.
#[derive(Default)]
pub struct Peripherals {
    /// VI hardware state (mode/features/y-scale/blanked/last-swapped
    /// framebuffer) -- see `vi.rs` module doc.
    vi: ViState,
    /// The periodic retrace ticker, driving `OS_EVENT_VI` delivery from
    /// `Executor::advance_time`. `None` until a host driver calls
    /// `arm_retrace`/`fn64-shell` picks a real interval -- no default
    /// interval is invented here (see `vi.rs`'s "not a hardware timing
    /// model" note); a boot harness that never arms it simply never
    /// receives VI retrace events, an honest state rather than a fabricated
    /// default NTSC constant.
    retrace: Option<RetraceSchedule>,
    /// Minimal SI/PIF controller-probe model (`si.rs`).
    pif: PifModel,
    /// Persistent physical images and bank latches for attached Controller Paks.
    controller_paks: [Option<ControllerPak>; 4],
    /// Persistent Transfer Pak register and inserted-cartridge state.
    transfer_paks: [Option<TransferPak>; 4],
    /// Persistent state for ports configured with Voice hardware.
    voice_units: [Option<VoiceUnit>; 4],
    /// RSP task submissions observed (`rsp.rs`).
    tasks: TaskLog,
}

/// Complete future-affecting evidence view of the executor-owned peripheral
/// state. Accessory slots are retained independently of current port identity:
/// detaching and later reattaching an accessory restores the same modeled
/// object, so hashing only active accessors would omit real future state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeripheralsEvidenceSnapshot {
    pub vi: ViEvidenceSnapshot,
    pub retrace: Option<RetraceScheduleEvidenceSnapshot>,
    pub pif: PifEvidenceSnapshot,
    pub controller_paks: [Option<ControllerPakEvidenceSnapshot>; 4],
    pub transfer_paks: [Option<TransferPakEvidenceSnapshot>; 4],
    pub voice_units: [Option<VoiceEvidenceSnapshot>; 4],
}

/// What `Peripherals::advance_retrace` found this tick -- the caller
/// (`Executor::advance_time`) still owns actually delivering these through
/// `inject_event`/`deliver_or_enqueue`, since delivery needs executor-owned
/// queue/blocked-list state `Peripherals` has no access to (by design --
/// see module doc).
pub struct RetraceTick {
    /// How many `OS_EVENT_VI` retrace ticks fired this call (see
    /// `RetraceSchedule::advance`'s doc comment for why this can be >1).
    pub event_vi_ticks: u32,
}

impl Peripherals {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn evidence_snapshot(&self) -> PeripheralsEvidenceSnapshot {
        PeripheralsEvidenceSnapshot {
            vi: self.vi.evidence_snapshot(),
            retrace: self
                .retrace
                .as_ref()
                .map(RetraceSchedule::evidence_snapshot),
            pif: self.pif.evidence_snapshot(),
            controller_paks: std::array::from_fn(|port| {
                self.controller_paks[port]
                    .as_ref()
                    .map(ControllerPak::evidence_snapshot)
            }),
            transfer_paks: std::array::from_fn(|port| {
                self.transfer_paks[port]
                    .as_ref()
                    .map(TransferPak::evidence_snapshot)
            }),
            voice_units: std::array::from_fn(|port| {
                self.voice_units[port]
                    .as_ref()
                    .map(VoiceUnit::evidence_snapshot)
            }),
        }
    }

    // ---- VI (video interface) -------------------------------------------

    pub fn vi(&self) -> &ViState {
        &self.vi
    }

    pub fn vi_set_mode(&mut self, mode_ptr: u32) {
        self.vi.set_mode(mode_ptr);
    }

    pub fn vi_set_special_features(&mut self, features: u32) {
        self.vi.set_special_features(features);
    }

    pub fn vi_set_y_scale(&mut self, scale: f32) {
        self.vi.set_y_scale(scale);
    }

    pub fn vi_set_x_scale(&mut self, scale: f32) {
        self.vi.set_x_scale(scale);
    }

    /// `osViSetEvent(mq, msg, retraceCount)` -- see `ViState::set_event`'s
    /// doc comment for why this is a separate delivery path from
    /// `osSetEventMesg`.
    pub fn vi_set_event(
        &mut self,
        mq_addr: RdramAddr,
        msg: crate::mesgqueue::Mesg,
        retrace_count: u32,
    ) {
        self.vi.set_event(mq_addr, msg, retrace_count);
    }

    pub fn vi_set_black(&mut self, active: bool) {
        self.vi.set_black(active);
    }

    pub fn vi_set_fade(&mut self, active: bool, factor: u16) {
        self.vi.set_fade(active, factor);
    }

    pub fn vi_set_repeat_line(&mut self, active: bool) {
        self.vi.set_repeat_line(active);
    }

    /// `osViSwapBuffer(frameBufPtr)`. Returns the newly-current framebuffer
    /// address, matching `Executor::vi_swap_buffer`'s previous return shape
    /// exactly -- the caller (`Executor`) still records the shared
    /// `TaskSubmit` trace event itself (see module doc: trace recording
    /// needs `sim_time`, which lives on `Executor`, not here).
    pub fn vi_swap_buffer(&mut self, frame_buf: RdramAddr) -> RdramAddr {
        self.vi.swap_buffer(frame_buf);
        frame_buf
    }

    pub fn vi_latch_retrace(&mut self) -> bool {
        self.vi.latch_retrace()
    }

    pub fn vi_manager_target_for_retrace(&mut self) -> Option<(u32, u32)> {
        self.vi.manager_target_for_retrace()
    }

    /// Arm the standalone compatibility VI ticker at `interval` virtual-time
    /// units per field. Integrated device execution instead derives timing
    /// from a typed television standard and the live VI timing registers.
    pub fn arm_retrace(&mut self, interval: u64) {
        self.retrace = Some(RetraceSchedule::new(interval));
    }

    /// Advance the retrace ticker to `now`, if armed. Returns `None` if
    /// never armed (matching `Executor::advance_time`'s prior "no `if let
    /// Some(sched)`, nothing happens" behavior exactly), else the tick
    /// counts the caller needs to actually deliver (see `RetraceTick`'s doc
    /// comment for why delivery itself stays the caller's job).
    pub fn advance_retrace(&mut self, now: u64) -> Option<RetraceTick> {
        let sched = self.retrace.as_mut()?;
        let event_vi_ticks = sched.advance(now);
        Some(RetraceTick { event_vi_ticks })
    }

    // ---- SI/PIF (controller probe) ---------------------------------------

    pub fn pif(&self) -> &PifModel {
        &self.pif
    }

    /// Feed a controller's live button/stick state for `port` -- the host
    /// side of the input seam (see `si::PifModel::set_input`). A subsequent
    /// `osContGetReadData` for that port reflects it.
    pub fn set_controller_input(&mut self, port: usize, input: crate::si::ContInput) {
        self.pif.set_input(port, input);
    }

    pub fn set_controller_port_state(&mut self, port: usize, state: PortState) {
        self.pif.set_port_state(port, state);
        if matches!(state, PortState::StandardControllerControllerPak)
            && self.controller_paks[port].is_none()
        {
            self.controller_paks[port] = Some(ControllerPak::new());
        }
        if matches!(state, PortState::StandardControllerTransferPak)
            && self.transfer_paks[port].is_none()
        {
            self.transfer_paks[port] = Some(TransferPak::new());
        }
        if matches!(state, PortState::VoiceRecognitionUnit) && self.voice_units[port].is_none() {
            self.voice_units[port] = Some(VoiceUnit::new());
        }
    }

    /// Install one host-configured Controller Pak and make it the active
    /// accessory on `port`. Capacity is carried by the Pak's validated bank
    /// count rather than a compile-time game setting.
    pub fn attach_controller_pak(&mut self, port: usize, pak: ControllerPak) {
        self.pif
            .set_port_state(port, PortState::StandardControllerControllerPak);
        self.controller_paks[port] = Some(pak);
    }

    pub fn set_rumble(&mut self, port: usize, active: bool) -> Result<(), RumbleError> {
        self.pif.set_rumble(port, active)
    }

    pub fn controller_pak(&self, port: usize) -> Option<&ControllerPak> {
        if !matches!(
            self.pif.port_state(port),
            PortState::StandardControllerControllerPak
        ) {
            return None;
        }
        self.controller_paks.get(port)?.as_ref()
    }

    pub fn controller_pak_mut(&mut self, port: usize) -> Option<&mut ControllerPak> {
        if !matches!(
            self.pif.port_state(port),
            PortState::StandardControllerControllerPak
        ) {
            return None;
        }
        self.controller_paks.get_mut(port)?.as_mut()
    }

    pub fn transfer_pak(&self, port: usize) -> Option<&TransferPak> {
        if !matches!(
            self.pif.port_state(port),
            PortState::StandardControllerTransferPak
        ) {
            return None;
        }
        self.transfer_paks.get(port)?.as_ref()
    }

    pub fn transfer_pak_mut(&mut self, port: usize) -> Option<&mut TransferPak> {
        if !matches!(
            self.pif.port_state(port),
            PortState::StandardControllerTransferPak
        ) {
            return None;
        }
        self.transfer_paks.get_mut(port)?.as_mut()
    }

    /// Advance every retained Transfer Pak cartridge, including an accessory
    /// temporarily detached from its controller port: MBC3's battery-backed
    /// oscillator is independent of both N64 controller and Pak power.
    pub fn advance_transfer_paks_to(&mut self, now: Cycles) {
        for pak in self.transfer_paks.iter_mut().flatten() {
            pak.advance_to(now);
        }
    }

    pub fn insert_transfer_pak_cartridge(
        &mut self,
        port: usize,
        rom: Vec<u8>,
        ram: Option<Vec<u8>>,
    ) -> Result<(), TransferPakError> {
        self.transfer_pak_mut(port)
            .unwrap_or_else(|| panic!("no Transfer Pak attached to controller port {port}"))
            .insert_cartridge(rom, ram)
    }

    pub fn insert_transfer_pak_cartridge_with_battery(
        &mut self,
        port: usize,
        rom: Vec<u8>,
        ram: Option<Vec<u8>>,
        restore: Option<Mbc3BatteryRestore>,
    ) -> Result<(), TransferPakError> {
        self.transfer_pak_mut(port)
            .unwrap_or_else(|| panic!("no Transfer Pak attached to controller port {port}"))
            .insert_cartridge_with_battery(rom, ram, restore)
    }

    pub fn checkpoint_transfer_pak_battery(
        &mut self,
        port: usize,
        now: Cycles,
        checkpoint: HostUnixNanos,
    ) -> Result<Option<Mbc3BatteryMetadata>, TransferPakError> {
        self.transfer_pak_mut(port)
            .unwrap_or_else(|| panic!("no Transfer Pak attached to controller port {port}"))
            .checkpoint_mbc3_battery(now, checkpoint)
    }

    pub fn voice_unit_mut(&mut self, port: usize) -> Option<&mut VoiceUnit> {
        if !matches!(self.pif.port_state(port), PortState::VoiceRecognitionUnit) {
            return None;
        }
        self.voice_units.get_mut(port)?.as_mut()
    }

    pub fn voice_unit(&self, port: usize) -> Option<&VoiceUnit> {
        if !matches!(self.pif.port_state(port), PortState::VoiceRecognitionUnit) {
            return None;
        }
        self.voice_units.get(port)?.as_ref()
    }

    // ---- RSP task submission -----------------------------------------------

    pub fn task_log(&self) -> &TaskLog {
        &self.tasks
    }

    /// Record an RSP task submission. Returns the task's `TaskKind`, if any,
    /// so the caller (`Executor::submit_task`) can still emit the shared
    /// `TaskSubmit` trace event itself (see module doc: trace recording
    /// needs `sim_time`, not modeled here) -- same information
    /// `Executor::submit_task`'s prior single-body version derived from
    /// `header.kind()` before calling `self.tasks.record(header)`.
    pub fn submit_task(&mut self, header: OsTaskHeader) -> Option<crate::trace::TaskKind> {
        let kind = header.kind();
        self.tasks.record(header);
        kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pfs::PfsKey;

    #[test]
    fn evidence_retains_detached_accessories_and_all_controller_state() {
        let mut peripherals = Peripherals::new();
        peripherals.set_controller_input(
            0,
            crate::si::ContInput {
                button: 0x9000,
                stick_x: -12,
                stick_y: 34,
            },
        );

        peripherals.set_controller_port_state(0, PortState::StandardControllerControllerPak);
        peripherals
            .controller_pak_mut(0)
            .unwrap()
            .allocate(
                PfsKey {
                    company_code: 1,
                    game_code: 2,
                    game_name: [3; 16],
                    ext_name: [4; 4],
                },
                256,
            )
            .unwrap();

        peripherals.set_controller_port_state(0, PortState::StandardControllerTransferPak);
        let mut rom = vec![0xff; 0x8000];
        rom[0x147] = 0;
        rom[0x149] = 0;
        peripherals
            .insert_transfer_pak_cartridge(0, rom, None)
            .unwrap();

        peripherals.set_controller_port_state(0, PortState::VoiceRecognitionUnit);
        peripherals.voice_unit_mut(0).unwrap().initialize();
        peripherals.set_controller_port_state(0, PortState::Absent);

        let snapshot = peripherals.evidence_snapshot();
        assert_eq!(snapshot.pif.ports[0], PortState::Absent);
        assert_eq!(snapshot.pif.inputs[0].button, 0x9000);
        assert!(snapshot.controller_paks[0].is_some());
        assert!(snapshot.controller_paks[0].as_ref().unwrap().notes[0].is_some());
        assert!(snapshot.transfer_paks[0]
            .as_ref()
            .unwrap()
            .cartridge
            .is_some());
        assert!(snapshot.voice_units[0].as_ref().unwrap().initialized);
    }
}
