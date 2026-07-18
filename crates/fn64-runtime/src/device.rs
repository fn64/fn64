//! Deterministic device clock and the first PI/MI vertical slice.
//!
//! Both raw KSEG1 register access and libultra shims must drive the same state
//! machine.  This module begins that convergence with PI DMA: either entry
//! path schedules one transfer on a typed guest-cycle deadline; completion
//! copies bytes, clears PI busy, raises MI PI pending, and emits the OS-facing
//! notification before guest execution can resume.
//!
//! Provenance: the public libultra `osPiRawStartDma` manual defines the raw PI
//! service and its single-transfer restriction; N64 Programming Manual
//! Chapter 27, "EPI Manager / Description of Handler", defines the two PI
//! domains and their latency/page/pulse/release parameters; the public
//! `rcp.h` register definitions provide the register addresses and field
//! widths. Those sources do not give an exact DMA completion-cycle formula,
//! so [`PiTimingModel`] is an explicit hardware-derived policy boundary.

use std::collections::BTreeMap;
use std::fmt;

use crate::rdram::{Rdram, RdramAddr};
use crate::rom::{DmaCompletion, PiDma, RomStorage};
use crate::trace::DmaDirection;

pub const PI_STATUS_DMA_BUSY: u32 = 1;
pub const PI_STATUS_IO_BUSY: u32 = 1 << 1;
pub const PI_STATUS_ERROR: u32 = 1 << 2;

const MI_INTR_REG: MmioAddr = MmioAddr::new(0xA430_0008);
const PI_DRAM_ADDR_REG: MmioAddr = MmioAddr::new(0xA460_0000);
const PI_CART_ADDR_REG: MmioAddr = MmioAddr::new(0xA460_0004);
const PI_RD_LEN_REG: MmioAddr = MmioAddr::new(0xA460_0008);
const PI_WR_LEN_REG: MmioAddr = MmioAddr::new(0xA460_000C);
const PI_STATUS_REG: MmioAddr = MmioAddr::new(0xA460_0010);
const PI_DOM1_LAT_REG: MmioAddr = MmioAddr::new(0xA460_0014);
const PI_DOM1_PWD_REG: MmioAddr = MmioAddr::new(0xA460_0018);
const PI_DOM1_PGS_REG: MmioAddr = MmioAddr::new(0xA460_001C);
const PI_DOM1_RLS_REG: MmioAddr = MmioAddr::new(0xA460_0020);
const PI_DOM2_LAT_REG: MmioAddr = MmioAddr::new(0xA460_0024);
const PI_DOM2_PWD_REG: MmioAddr = MmioAddr::new(0xA460_0028);
const PI_DOM2_PGS_REG: MmioAddr = MmioAddr::new(0xA460_002C);
const PI_DOM2_RLS_REG: MmioAddr = MmioAddr::new(0xA460_0030);

/// Guest device time. Host wall-clock units cannot be converted implicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cycles(u64);

impl Cycles {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn checked_add(self, other: Self) -> Option<Self> {
        match self.0.checked_add(other.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

/// Raw word-sized hardware-register address in the guest KSEG1 domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MmioAddr(u32);

impl MmioAddr {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn is_word_aligned(self) -> bool {
        self.0 & 3 == 0
    }
}

impl fmt::Display for MmioAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010X}", self.0)
    }
}

/// N64 MI interrupt sources and their public `MI_INTR` bit positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InterruptSource {
    Sp,
    Si,
    Ai,
    Vi,
    Pi,
    Dp,
}

impl InterruptSource {
    pub const fn bit(self) -> u32 {
        match self {
            Self::Sp => 1 << 0,
            Self::Si => 1 << 1,
            Self::Ai => 1 << 2,
            Self::Vi => 1 << 3,
            Self::Pi => 1 << 4,
            Self::Dp => 1 << 5,
        }
    }
}

/// One PI transfer request, shared by shim and raw-MMIO entry paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PiDmaRequest {
    pub direction: DmaDirection,
    pub dram_addr: RdramAddr,
    pub cart_addr: u32,
    pub len: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PiDomain {
    Domain1,
    Domain2,
}

impl PiDmaRequest {
    pub const fn domain(self) -> PiDomain {
        if (self.cart_addr >= 0x0500_0000 && self.cart_addr <= 0x05FF_FFFF)
            || (self.cart_addr >= 0x0800_0000 && self.cart_addr <= 0x0FFF_FFFF)
        {
            PiDomain::Domain2
        } else {
            PiDomain::Domain1
        }
    }
}

/// PI bus parameters exposed by the four domain timing registers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PiDomainTiming {
    pub latency: u8,
    pub pulse_width: u8,
    pub page_size: u8,
    pub release: u8,
}

/// Timing authority supplied to the device fabric.
///
/// The interface is deliberate: this slice does not invent a one-cycle-per-
/// byte rule. A hardware-derived cartridge-domain model can be installed
/// without changing PI state/event semantics or either caller path.
pub trait PiTimingModel {
    fn completion_latency(&self, request: PiDmaRequest, timing: PiDomainTiming) -> Cycles;
}

/// OS-facing work produced after a device event is fully committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceNotification {
    PiDmaComplete(DmaCompletion),
}

/// Observable device transition, ordered at one guest cycle by `sequence`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceTraceEvent {
    pub at: Cycles,
    pub sequence: u64,
    pub kind: DeviceTraceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceTraceKind {
    PiDmaStarted(PiDmaRequest),
    PiBytesCommitted(PiDmaRequest),
    PiBusyCleared,
    MiInterruptRaised(InterruptSource),
    NotificationReady(DeviceNotification),
}

/// Typed failure at the raw/shim device boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceFault {
    UnalignedMmio { addr: MmioAddr },
    UnmodeledMmioRead { addr: MmioAddr },
    UnmodeledMmioWrite { addr: MmioAddr, value: u32 },
    PiBusy,
    ZeroLengthPiDma,
    PiLengthOverflow { encoded: u32 },
    DeadlineOverflow,
    TimeWentBack { now: Cycles, requested: Cycles },
}

impl fmt::Display for DeviceFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::UnalignedMmio { addr } => write!(f, "unaligned MMIO word access at {addr}"),
            Self::UnmodeledMmioRead { addr } => write!(f, "unmodeled MMIO read at {addr}"),
            Self::UnmodeledMmioWrite { addr, value } => {
                write!(f, "unmodeled MMIO write at {addr}: {value:#010X}")
            }
            Self::PiBusy => write!(f, "PI DMA start while the PI channel is busy"),
            Self::ZeroLengthPiDma => write!(f, "PI DMA length must be nonzero"),
            Self::PiLengthOverflow { encoded } => {
                write!(f, "PI encoded DMA length {encoded:#010X} overflows")
            }
            Self::DeadlineOverflow => write!(f, "device-event deadline overflow"),
            Self::TimeWentBack { now, requested } => write!(
                f,
                "device time cannot move backward from {} to {} cycles",
                now.get(),
                requested.get()
            ),
        }
    }
}

impl std::error::Error for DeviceFault {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingPi {
    token: u64,
    request: PiDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceEvent {
    PiComplete { token: u64 },
}

/// Guest-visible PI/MI snapshot used by deterministic traces and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub now: Cycles,
    pub pi_dram_addr: RdramAddr,
    pub pi_cart_addr: u32,
    pub pi_status: u32,
    pub mi_pending: u32,
    pub mi_mask: u32,
    pub pi_domain1: PiDomainTiming,
    pub pi_domain2: PiDomainTiming,
}

/// One authoritative device state machine.
pub struct DeviceFabric<R: RomStorage, T: PiTimingModel> {
    now: Cycles,
    pi_dma: PiDma<R>,
    pi_timing: T,
    pi_dram_addr: RdramAddr,
    pi_cart_addr: u32,
    pi_status: u32,
    mi_pending: u32,
    mi_mask: u32,
    pi_domain1: PiDomainTiming,
    pi_domain2: PiDomainTiming,
    pending_pi: Option<PendingPi>,
    events: BTreeMap<(Cycles, u64), DeviceEvent>,
    next_event_sequence: u64,
    trace: Vec<DeviceTraceEvent>,
    next_trace_sequence: u64,
}

impl<R: RomStorage, T: PiTimingModel> DeviceFabric<R, T> {
    pub fn new(pi_dma: PiDma<R>, pi_timing: T) -> Self {
        Self {
            now: Cycles::ZERO,
            pi_dma,
            pi_timing,
            pi_dram_addr: RdramAddr::from_offset(0),
            pi_cart_addr: 0,
            pi_status: 0,
            mi_pending: 0,
            mi_mask: 0,
            pi_domain1: PiDomainTiming::default(),
            pi_domain2: PiDomainTiming::default(),
            pending_pi: None,
            events: BTreeMap::new(),
            next_event_sequence: 0,
            trace: Vec::new(),
            next_trace_sequence: 0,
        }
    }

    pub const fn now(&self) -> Cycles {
        self.now
    }

    pub fn snapshot(&self) -> DeviceSnapshot {
        DeviceSnapshot {
            now: self.now,
            pi_dram_addr: self.pi_dram_addr,
            pi_cart_addr: self.pi_cart_addr,
            pi_status: self.pi_status,
            mi_pending: self.mi_pending,
            mi_mask: self.mi_mask,
            pi_domain1: self.pi_domain1,
            pi_domain2: self.pi_domain2,
        }
    }

    pub fn set_interrupt_mask(&mut self, source: InterruptSource, enabled: bool) {
        if enabled {
            self.mi_mask |= source.bit();
        } else {
            self.mi_mask &= !source.bit();
        }
    }

    pub fn set_pi_domain_timing(&mut self, domain: PiDomain, timing: PiDomainTiming) {
        match domain {
            PiDomain::Domain1 => self.pi_domain1 = timing,
            PiDomain::Domain2 => self.pi_domain2 = timing,
        }
    }

    pub const fn pi_domain_timing(&self, domain: PiDomain) -> PiDomainTiming {
        match domain {
            PiDomain::Domain1 => self.pi_domain1,
            PiDomain::Domain2 => self.pi_domain2,
        }
    }

    pub fn interrupt_pending(&self, source: InterruptSource) -> bool {
        self.mi_pending & source.bit() != 0
    }

    pub fn cpu_interrupt_pending(&self) -> bool {
        self.mi_pending & self.mi_mask != 0
    }

    pub fn next_deadline(&self) -> Option<Cycles> {
        self.events.first_key_value().map(|(key, _)| key.0)
    }

    pub fn trace(&self) -> &[DeviceTraceEvent] {
        &self.trace
    }

    /// Shim entry path. Raw MMIO converges here after latching its registers.
    pub fn start_pi_dma(&mut self, request: PiDmaRequest) -> Result<(), DeviceFault> {
        if self.pending_pi.is_some() {
            return Err(DeviceFault::PiBusy);
        }
        if request.len == 0 {
            return Err(DeviceFault::ZeroLengthPiDma);
        }
        let timing = self.pi_domain_timing(request.domain());
        let deadline = self
            .now
            .checked_add(self.pi_timing.completion_latency(request, timing))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.pi_dram_addr = request.dram_addr;
        self.pi_cart_addr = request.cart_addr;
        self.pi_status = PI_STATUS_DMA_BUSY;
        self.pending_pi = Some(PendingPi { token, request });
        self.events
            .insert((deadline, token), DeviceEvent::PiComplete { token });
        self.record(DeviceTraceKind::PiDmaStarted(request));
        Ok(())
    }

    pub fn read_mmio(&self, addr: MmioAddr) -> Result<u32, DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            MI_INTR_REG => Ok(self.mi_pending),
            PI_DRAM_ADDR_REG => Ok(self.pi_dram_addr.offset()),
            PI_CART_ADDR_REG => Ok(self.pi_cart_addr),
            PI_STATUS_REG => Ok(self.pi_status),
            PI_DOM1_LAT_REG => Ok(self.pi_domain1.latency as u32),
            PI_DOM1_PWD_REG => Ok(self.pi_domain1.pulse_width as u32),
            PI_DOM1_PGS_REG => Ok(self.pi_domain1.page_size as u32),
            PI_DOM1_RLS_REG => Ok(self.pi_domain1.release as u32),
            PI_DOM2_LAT_REG => Ok(self.pi_domain2.latency as u32),
            PI_DOM2_PWD_REG => Ok(self.pi_domain2.pulse_width as u32),
            PI_DOM2_PGS_REG => Ok(self.pi_domain2.page_size as u32),
            PI_DOM2_RLS_REG => Ok(self.pi_domain2.release as u32),
            _ => Err(DeviceFault::UnmodeledMmioRead { addr }),
        }
    }

    pub fn write_mmio(&mut self, addr: MmioAddr, value: u32) -> Result<(), DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            PI_DRAM_ADDR_REG => {
                self.pi_dram_addr = RdramAddr::from_offset(value);
                Ok(())
            }
            PI_CART_ADDR_REG => {
                self.pi_cart_addr = value;
                Ok(())
            }
            PI_RD_LEN_REG => {
                let len = value
                    .checked_add(1)
                    .ok_or(DeviceFault::PiLengthOverflow { encoded: value })?;
                self.start_pi_dma(PiDmaRequest {
                    direction: DmaDirection::ToRdram,
                    dram_addr: self.pi_dram_addr,
                    cart_addr: self.pi_cart_addr,
                    len,
                })
            }
            PI_WR_LEN_REG => {
                let len = value
                    .checked_add(1)
                    .ok_or(DeviceFault::PiLengthOverflow { encoded: value })?;
                self.start_pi_dma(PiDmaRequest {
                    direction: DmaDirection::FromRdram,
                    dram_addr: self.pi_dram_addr,
                    cart_addr: self.pi_cart_addr,
                    len,
                })
            }
            PI_STATUS_REG if value & !0b11 == 0 => {
                // Public PI_STATUS command bits: bit 0 resets/aborts PI and
                // bit 1 clears the PI interrupt. An aborted event remains in
                // the heap but its token no longer owns `pending_pi`, so it
                // cannot later copy bytes or raise MI.
                if value & 0b1 != 0 {
                    self.pending_pi = None;
                    self.pi_status = 0;
                }
                if value & 0b10 != 0 {
                    self.mi_pending &= !InterruptSource::Pi.bit();
                }
                Ok(())
            }
            PI_DOM1_LAT_REG => {
                self.pi_domain1.latency = value as u8;
                Ok(())
            }
            PI_DOM1_PWD_REG => {
                self.pi_domain1.pulse_width = value as u8;
                Ok(())
            }
            PI_DOM1_PGS_REG => {
                self.pi_domain1.page_size = (value & 0xF) as u8;
                Ok(())
            }
            PI_DOM1_RLS_REG => {
                self.pi_domain1.release = (value & 0x3) as u8;
                Ok(())
            }
            PI_DOM2_LAT_REG => {
                self.pi_domain2.latency = value as u8;
                Ok(())
            }
            PI_DOM2_PWD_REG => {
                self.pi_domain2.pulse_width = value as u8;
                Ok(())
            }
            PI_DOM2_PGS_REG => {
                self.pi_domain2.page_size = (value & 0xF) as u8;
                Ok(())
            }
            PI_DOM2_RLS_REG => {
                self.pi_domain2.release = (value & 0x3) as u8;
                Ok(())
            }
            _ => Err(DeviceFault::UnmodeledMmioWrite { addr, value }),
        }
    }

    /// Advance deterministic device time and fully commit every due event.
    /// Notifications are returned only after their device and MI state is
    /// guest-visible, so the executor can post them before resuming a thread.
    pub fn advance_to(
        &mut self,
        requested: Cycles,
        rdram: &mut Rdram,
    ) -> Result<Vec<DeviceNotification>, DeviceFault> {
        if requested < self.now {
            return Err(DeviceFault::TimeWentBack {
                now: self.now,
                requested,
            });
        }
        let mut notifications = Vec::new();
        while let Some((&key, &event)) = self.events.first_key_value() {
            if key.0 > requested {
                break;
            }
            self.events.remove(&key);
            self.now = key.0;
            match event {
                DeviceEvent::PiComplete { token } => {
                    let Some(pending) = self.pending_pi else {
                        continue;
                    };
                    if pending.token != token {
                        continue;
                    }
                    let request = pending.request;
                    let completion = self.pi_dma.start_dma(
                        rdram,
                        request.direction,
                        request.dram_addr,
                        request.cart_addr,
                        request.len,
                    );
                    self.record(DeviceTraceKind::PiBytesCommitted(request));
                    self.pending_pi = None;
                    self.pi_status &= !PI_STATUS_DMA_BUSY;
                    self.record(DeviceTraceKind::PiBusyCleared);
                    self.mi_pending |= InterruptSource::Pi.bit();
                    self.record(DeviceTraceKind::MiInterruptRaised(InterruptSource::Pi));
                    let notification = DeviceNotification::PiDmaComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
            }
        }
        self.now = requested;
        Ok(notifications)
    }

    fn validate_mmio(&self, addr: MmioAddr) -> Result<(), DeviceFault> {
        if addr.is_word_aligned() {
            Ok(())
        } else {
            Err(DeviceFault::UnalignedMmio { addr })
        }
    }

    fn record(&mut self, kind: DeviceTraceKind) {
        let sequence = self.next_trace_sequence;
        self.next_trace_sequence = self
            .next_trace_sequence
            .checked_add(1)
            .expect("device trace sequence overflow");
        self.trace.push(DeviceTraceEvent {
            at: self.now,
            sequence,
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rom::InMemoryRom;

    #[derive(Clone, Copy)]
    struct TestTiming(Cycles);

    impl PiTimingModel for TestTiming {
        fn completion_latency(&self, _request: PiDmaRequest, _timing: PiDomainTiming) -> Cycles {
            self.0
        }
    }

    fn fabric() -> DeviceFabric<InMemoryRom, TestTiming> {
        let mut rom = vec![0u8; 0x100];
        rom[0x10..0x14].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        DeviceFabric::new(
            PiDma::new(InMemoryRom::new(rom)),
            TestTiming(Cycles::new(12)),
        )
    }

    #[test]
    fn raw_and_shim_pi_starts_share_one_timed_state_machine() {
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            cart_addr: 0x10,
            len: 4,
        };
        let mut shim = fabric();
        let mut raw = fabric();
        let mut shim_rdram = Rdram::new(0x100);
        let mut raw_rdram = Rdram::new(0x100);

        shim.start_pi_dma(request).unwrap();
        raw.write_mmio(PI_DRAM_ADDR_REG, 0x20).unwrap();
        raw.write_mmio(PI_CART_ADDR_REG, 0x10).unwrap();
        raw.write_mmio(PI_RD_LEN_REG, 3).unwrap();
        assert_eq!(raw.snapshot(), shim.snapshot());
        assert_eq!(raw.read_mmio(PI_STATUS_REG).unwrap(), PI_STATUS_DMA_BUSY);

        assert!(raw
            .advance_to(Cycles::new(11), &mut raw_rdram)
            .unwrap()
            .is_empty());
        assert!(shim
            .advance_to(Cycles::new(11), &mut shim_rdram)
            .unwrap()
            .is_empty());
        assert_eq!(raw_rdram.read_w(RdramAddr::from_offset(0x20)), 0);
        assert_eq!(raw.snapshot(), shim.snapshot());

        let raw_notifications = raw.advance_to(Cycles::new(12), &mut raw_rdram).unwrap();
        let shim_notifications = shim.advance_to(Cycles::new(12), &mut shim_rdram).unwrap();
        assert_eq!(raw_notifications, shim_notifications);
        assert_eq!(raw.snapshot(), shim.snapshot());
        assert_eq!(raw.trace(), shim.trace());
        assert_eq!(
            raw_rdram.read_w(RdramAddr::from_offset(0x20)) as u32,
            0xDEAD_BEEF
        );
        assert_eq!(
            raw_rdram.read_w(RdramAddr::from_offset(0x20)),
            shim_rdram.read_w(RdramAddr::from_offset(0x20))
        );
        assert_eq!(raw.read_mmio(PI_STATUS_REG).unwrap(), 0);
        assert_eq!(
            raw.read_mmio(MI_INTR_REG).unwrap(),
            InterruptSource::Pi.bit()
        );
        assert!(raw.interrupt_pending(InterruptSource::Pi));

        let kinds = raw
            .trace()
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                DeviceTraceKind::PiDmaStarted(request),
                DeviceTraceKind::PiBytesCommitted(request),
                DeviceTraceKind::PiBusyCleared,
                DeviceTraceKind::MiInterruptRaised(InterruptSource::Pi),
                DeviceTraceKind::NotificationReady(raw_notifications[0]),
            ]
        );
        assert_eq!(raw.trace()[0].at, Cycles::ZERO);
        assert!(raw.trace()[1..]
            .iter()
            .all(|event| event.at == Cycles::new(12)));
        assert_eq!(
            raw.trace()
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );

        raw.set_interrupt_mask(InterruptSource::Pi, true);
        assert!(raw.cpu_interrupt_pending());
        raw.write_mmio(PI_STATUS_REG, 0b10).unwrap();
        assert!(!raw.interrupt_pending(InterruptSource::Pi));
        assert!(!raw.cpu_interrupt_pending());
    }

    #[test]
    fn pi_channel_serializes_requests_and_time_never_moves_backward() {
        let mut fabric = fabric();
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            cart_addr: 0x10,
            len: 4,
        };
        fabric.start_pi_dma(request).unwrap();
        assert_eq!(fabric.start_pi_dma(request), Err(DeviceFault::PiBusy));

        let mut rdram = Rdram::new(0x100);
        fabric.advance_to(Cycles::new(12), &mut rdram).unwrap();
        assert_eq!(
            fabric.advance_to(Cycles::new(11), &mut rdram),
            Err(DeviceFault::TimeWentBack {
                now: Cycles::new(12),
                requested: Cycles::new(11),
            })
        );
    }

    #[test]
    fn unknown_or_unaligned_registers_fail_loudly() {
        let mut fabric = fabric();
        assert_eq!(
            fabric.read_mmio(MmioAddr::new(0xA460_0001)),
            Err(DeviceFault::UnalignedMmio {
                addr: MmioAddr::new(0xA460_0001)
            })
        );
        assert_eq!(
            fabric.write_mmio(MmioAddr::new(0xA460_0034), 7),
            Err(DeviceFault::UnmodeledMmioWrite {
                addr: MmioAddr::new(0xA460_0034),
                value: 7,
            })
        );
    }

    #[test]
    fn pi_domain_registers_are_the_timing_models_typed_input() {
        let mut fabric = fabric();
        fabric.write_mmio(PI_DOM2_LAT_REG, 0x105).unwrap();
        fabric.write_mmio(PI_DOM2_PWD_REG, 0x20C).unwrap();
        fabric.write_mmio(PI_DOM2_PGS_REG, 0x1D).unwrap();
        fabric.write_mmio(PI_DOM2_RLS_REG, 0x6).unwrap();

        assert_eq!(
            fabric.pi_domain_timing(PiDomain::Domain2),
            PiDomainTiming {
                latency: 0x05,
                pulse_width: 0x0C,
                page_size: 0x0D,
                release: 0x02,
            }
        );
        assert_eq!(fabric.read_mmio(PI_DOM2_LAT_REG).unwrap(), 0x05);
        assert_eq!(fabric.read_mmio(PI_DOM2_PWD_REG).unwrap(), 0x0C);
        assert_eq!(fabric.read_mmio(PI_DOM2_PGS_REG).unwrap(), 0x0D);
        assert_eq!(fabric.read_mmio(PI_DOM2_RLS_REG).unwrap(), 0x02);

        assert_eq!(
            PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0),
                cart_addr: 0x0800_0000,
                len: 2,
            }
            .domain(),
            PiDomain::Domain2
        );
    }

    #[test]
    fn pi_reset_cancels_the_owned_completion_event() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric
            .start_pi_dma(PiDmaRequest {
                direction: DmaDirection::ToRdram,
                dram_addr: RdramAddr::from_offset(0x20),
                cart_addr: 0x10,
                len: 4,
            })
            .unwrap();
        fabric.write_mmio(PI_STATUS_REG, 0b1).unwrap();

        assert_eq!(fabric.read_mmio(PI_STATUS_REG).unwrap(), 0);
        assert!(fabric
            .advance_to(Cycles::new(12), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(rdram.read_w(RdramAddr::from_offset(0x20)), 0);
        assert!(!fabric.interrupt_pending(InterruptSource::Pi));
    }
}
