//! Deterministic RCP device clock and shared MI interrupt fabric.
//!
//! Both raw KSEG1 register access and libultra shims must drive the same state
//! machine. PI, AI, SI, VI, SP DMA, and HLE SP/DP completion all enter one ordered
//! guest-cycle heap; each completion commits device state and the shared MI
//! source before emitting an OS-facing notification.
//!
//! Provenance: the public libultra `osPiRawStartDma` manual defines the raw PI
//! service and its single-transfer restriction; N64 Programming Manual
//! Chapter 27, "EPI Manager / Description of Handler", defines the two PI
//! domains and their latency/page/pulse/release parameters; the public
//! `rcp.h` register definitions provide register addresses and field widths.
//! Programming Manual Chapter 30 defines `VI_CURRENT` as the sampled
//! half-line, mode/framebuffer changes as V-blank-latched state, and the
//! VI-manager notification boundary. Those sources do not give every exact
//! device completion-cycle formula, so timing policies stay explicit. SP
//! memory, register, alignment, double-buffering, and transfer-shape semantics
//! come from the public SGI *Nintendo 64 RSP Programmer's Guide*, chapter 4,
//! tables 4-1 through 4-7 and the "DMA" section. Its documented 6-12-cycle
//! setup range depends on contention this runtime does not yet model; the
//! deterministic policy here uses eight setup cycles plus one cycle per
//! 64-bit beat and does not claim bus-cycle accuracy.

use std::collections::BTreeMap;
use std::fmt;

use crate::mmio::{AI_STATUS_BUSY, AI_STATUS_FULL};
use crate::rdram::RdramAddr;
use crate::rom::{DmaCompletion, DmaMemory, PiDma, PiDmaError, RomStorage};
use crate::rsp::{RspMemAddr, RspMemory, RspMemoryError, RSP_MEMORY_BANK_SIZE};
use crate::trace::DmaDirection;
use crate::tv::{TvType, CPU_CLOCK_HZ};

pub const PI_STATUS_DMA_BUSY: u32 = 1;
pub const PI_STATUS_IO_BUSY: u32 = 1 << 1;
pub const PI_STATUS_ERROR: u32 = 1 << 2;

const MI_INTR_REG: MmioAddr = MmioAddr::new(0xA430_0008);
const MI_INTR_MASK_REG: MmioAddr = MmioAddr::new(0xA430_000C);
const VI_STATUS_REG: MmioAddr = MmioAddr::new(0xA440_0000);
const VI_ORIGIN_REG: MmioAddr = MmioAddr::new(0xA440_0004);
const VI_INTR_REG: MmioAddr = MmioAddr::new(0xA440_000C);
const VI_CURRENT_REG: MmioAddr = MmioAddr::new(0xA440_0010);
const VI_V_SYNC_REG: MmioAddr = MmioAddr::new(0xA440_0018);
const VI_H_SYNC_REG: MmioAddr = MmioAddr::new(0xA440_001C);
const VI_Y_SCALE_REG: MmioAddr = MmioAddr::new(0xA440_0034);
const SI_DRAM_ADDR_REG: MmioAddr = MmioAddr::new(0xA480_0000);
const SI_PIF_ADDR_RD64B_REG: MmioAddr = MmioAddr::new(0xA480_0004);
const SI_PIF_ADDR_WR64B_REG: MmioAddr = MmioAddr::new(0xA480_0010);
const SI_STATUS_REG: MmioAddr = MmioAddr::new(0xA480_0018);
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
const SP_DMEM_START: u32 = 0xA400_0000;
const SP_IMEM_END: u32 = 0xA400_2000;
const SP_MEM_ADDR_REG: MmioAddr = MmioAddr::new(0xA404_0000);
const SP_DRAM_ADDR_REG: MmioAddr = MmioAddr::new(0xA404_0004);
const SP_RD_LEN_REG: MmioAddr = MmioAddr::new(0xA404_0008);
const SP_WR_LEN_REG: MmioAddr = MmioAddr::new(0xA404_000C);
const SP_STATUS_REG: MmioAddr = MmioAddr::new(0xA404_0010);
const SP_DMA_FULL_REG: MmioAddr = MmioAddr::new(0xA404_0014);
const SP_DMA_BUSY_REG: MmioAddr = MmioAddr::new(0xA404_0018);
const SP_SEMAPHORE_REG: MmioAddr = MmioAddr::new(0xA404_001C);
const SP_PC_REG: MmioAddr = MmioAddr::new(0xA408_0000);

pub const SP_STATUS_HALT: u32 = 1;
pub const SP_STATUS_BROKE: u32 = 1 << 1;
pub const SP_STATUS_DMA_BUSY: u32 = 1 << 2;
pub const SP_STATUS_DMA_FULL: u32 = 1 << 3;
pub const SP_STATUS_SINGLE_STEP: u32 = 1 << 5;
pub const SP_STATUS_INTERRUPT_ON_BREAK: u32 = 1 << 6;
pub const SP_STATUS_SIGNAL_0: u32 = 1 << 7;
pub const SP_STATUS_SIGNAL_1: u32 = 1 << 8;
/// Public RSP task protocol alias: SIG0 is the CPU's asynchronous yield request.
pub const SP_STATUS_YIELD: u32 = SP_STATUS_SIGNAL_0;
/// Public RSP task protocol alias: SIG1 says the microcode saved resumable state.
pub const SP_STATUS_YIELDED: u32 = SP_STATUS_SIGNAL_1;

/// SP status-register commands from the public `rcp.h` register contract.
pub const SP_CLR_YIELD: u32 = 1 << 9;
pub const SP_SET_YIELD: u32 = 1 << 10;
pub const SP_CLR_YIELDED: u32 = 1 << 11;
pub const SP_SET_YIELDED: u32 = 1 << 12;

fn apply_device_clear_set_pair(
    state: &mut u32,
    command: u32,
    clear_command_bit: u32,
    set_command_bit: u32,
    state_mask: u32,
) {
    if command & (1 << clear_command_bit) != 0 {
        *state &= !state_mask;
    }
    if command & (1 << set_command_bit) != 0 {
        *state |= state_mask;
    }
}

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

/// One stereo 16-bit AI DMA buffer. `sample_rate_hz` is the true rate
/// returned by libultra after quantizing the programmed DAC rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiDmaRequest {
    pub dram_addr: RdramAddr,
    pub len: u32,
    pub sample_rate_hz: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiDmaKind {
    DramToPif,
    PifToDram,
    ControllerQuery,
    ControllerRead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiDmaRequest {
    pub kind: SiDmaKind,
    pub dram_addr: RdramAddr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpDmaDirection {
    RdramToRsp,
    RspToRdram,
}

/// One fully latched SP DMA request. Length/count retain the hardware's
/// encoded-minus-one representation only at the register boundary; these
/// accessors expose the actual aligned transfer shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpDmaRequest {
    pub direction: SpDmaDirection,
    pub mem_addr: RspMemAddr,
    pub dram_addr: RdramAddr,
    pub encoded_len: u32,
}

impl SpDmaRequest {
    pub const fn line_len(self) -> usize {
        ((self.encoded_len & 0x0ff8) + 8) as usize
    }

    pub const fn line_count(self) -> usize {
        (((self.encoded_len >> 12) & 0xff) + 1) as usize
    }

    pub const fn skip(self) -> usize {
        ((self.encoded_len >> 20) & 0x0fff) as usize
    }

    pub const fn total_bytes(self) -> usize {
        self.line_len() * self.line_count()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RcpTaskCompletion {
    Sp,
    Dp,
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

/// Explicit deterministic PI policy for hosts that do not yet install a
/// hardware-derived domain timing model. It never consults wall time and it
/// does not claim cycle-accuracy: every transfer completes after the supplied
/// fixed number of guest cycles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedPiTiming(pub Cycles);

impl PiTimingModel for FixedPiTiming {
    fn completion_latency(&self, _request: PiDmaRequest, _timing: PiDomainTiming) -> Cycles {
        self.0
    }
}

/// OS-facing work produced after a device event is fully committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceNotification {
    PiDmaComplete(DmaCompletion),
    AiDmaComplete(AiDmaRequest),
    SiDmaComplete(SiDmaRequest),
    ViRetrace,
    RcpTaskComplete(RcpTaskCompletion),
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
    AiDmaStarted(AiDmaRequest),
    AiDmaComplete(AiDmaRequest),
    SiDmaStarted(SiDmaRequest),
    SiBytesCommitted(SiDmaRequest),
    SiBusyCleared,
    SpDmaStarted(SpDmaRequest),
    SpDmaQueued(SpDmaRequest),
    SpDmaBytesCommitted(SpDmaRequest),
    SpDmaBusyCleared,
    SpTaskAdmitted {
        task_addr: RdramAddr,
        header: crate::rsp::OsTaskHeader,
    },
    ViInterrupt,
    RcpTaskStarted {
        needs_dp: bool,
    },
    RcpTaskComplete(RcpTaskCompletion),
    MiInterruptRaised(InterruptSource),
    MiInterruptCleared(InterruptSource),
    NotificationReady(DeviceNotification),
}

/// Typed failure at the raw/shim device boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceFault {
    UnalignedMmio { addr: MmioAddr },
    UnmodeledMmioRead { addr: MmioAddr },
    UnmodeledMmioWrite { addr: MmioAddr, value: u32 },
    PiBusy,
    AiFull,
    ZeroLengthAiDma,
    ZeroAiSampleRate,
    ZeroViInterval,
    SiBusy,
    SpBusy,
    SpDmaFull,
    SpDmaMemory(RspMemoryError),
    SpDmaDramRangeOverflow { request: SpDmaRequest },
    InvalidSpSemaphoreWrite { value: u32 },
    SpTaskNotHalted,
    InvalidSpTaskBootSize { size: u32 },
    DpBusy,
    ZeroLengthPiDma,
    PiLengthOverflow { encoded: u32 },
    PiTransfer(PiDmaError),
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
            Self::AiFull => write!(f, "AI DMA start while both FIFO slots are occupied"),
            Self::ZeroLengthAiDma => write!(f, "AI DMA length must be nonzero"),
            Self::ZeroAiSampleRate => write!(f, "AI DMA sample rate must be nonzero"),
            Self::ZeroViInterval => write!(f, "VI field interval must be nonzero"),
            Self::SiBusy => write!(f, "SI DMA start while the SI channel is busy"),
            Self::SpBusy => write!(f, "RSP task start while SP is busy"),
            Self::SpDmaFull => write!(f, "SP DMA start while active and pending slots are full"),
            Self::SpDmaMemory(error) => write!(f, "SP DMA rejected: {error}"),
            Self::SpDmaDramRangeOverflow { request } => write!(
                f,
                "SP DMA DRAM addressing overflows the 24-bit physical domain: {request:?}"
            ),
            Self::InvalidSpSemaphoreWrite { value } => write!(
                f,
                "SP semaphore release requires a zero write, got {value:#010X}"
            ),
            Self::SpTaskNotHalted => write!(f, "SP task load while the RSP is not halted"),
            Self::InvalidSpTaskBootSize { size } => write!(
                f,
                "SP task boot microcode size {size:#x} does not fit the 4 KiB IMEM bank"
            ),
            Self::DpBusy => write!(f, "graphics task start while DP is busy"),
            Self::ZeroLengthPiDma => write!(f, "PI DMA length must be nonzero"),
            Self::PiLengthOverflow { encoded } => {
                write!(f, "PI encoded DMA length {encoded:#010X} overflows")
            }
            Self::PiTransfer(error) => write!(f, "PI transfer rejected: {error}"),
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
struct PendingAi {
    token: u64,
    request: AiDmaRequest,
    started_at: Cycles,
    deadline: Cycles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingSi {
    token: u64,
    request: SiDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingSpDma {
    token: u64,
    request: SpDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceEvent {
    Pi { token: u64 },
    Ai { token: u64 },
    Si { token: u64 },
    SpDma { token: u64 },
    Vi { token: u64 },
    Sp { token: u64 },
    Dp { token: u64 },
}

/// Guest-visible PI/MI snapshot used by deterministic traces and tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub now: Cycles,
    pub pi_dram_addr: RdramAddr,
    pub pi_cart_addr: u32,
    pub pi_status: u32,
    pub ai_status: u32,
    pub ai_length: u32,
    pub si_dram_addr: RdramAddr,
    pub si_status: u32,
    pub vi_current: u32,
    pub vi_intr: u32,
    pub vi_v_sync: u32,
    pub tv_type: Option<TvType>,
    pub vi_field_interval: Option<Cycles>,
    pub sp_busy: bool,
    pub sp_status: u32,
    pub sp_mem_addr: RspMemAddr,
    pub sp_dram_addr: RdramAddr,
    pub sp_imem_generation: u64,
    pub dp_busy: bool,
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
    current_ai: Option<PendingAi>,
    queued_ai: Option<AiDmaRequest>,
    si_dram_addr: RdramAddr,
    si_dma_error: bool,
    pending_si: Option<PendingSi>,
    si_latency: Cycles,
    pif_ram: [u8; 64],
    rsp_memory: RspMemory,
    sp_mem_addr: RspMemAddr,
    sp_dram_addr: RdramAddr,
    sp_rd_len: u32,
    sp_wr_len: u32,
    sp_status: u32,
    sp_pc: u32,
    sp_semaphore: bool,
    active_sp_dma: Option<PendingSpDma>,
    queued_sp_dma: Option<SpDmaRequest>,
    sp_dma_setup_cycles: Cycles,
    vi_registers: [u32; 14],
    tv_type: Option<TvType>,
    vi_field_interval: Option<Cycles>,
    vi_epoch: Cycles,
    pending_vi: Option<u64>,
    pending_sp: Option<u64>,
    pending_dp: Option<u64>,
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
            current_ai: None,
            queued_ai: None,
            si_dram_addr: RdramAddr::from_offset(0),
            si_dma_error: false,
            pending_si: None,
            si_latency: Cycles::new(1),
            pif_ram: [0; 64],
            rsp_memory: RspMemory::new(),
            sp_mem_addr: RspMemAddr::default(),
            sp_dram_addr: RdramAddr::from_offset(0),
            sp_rd_len: 0,
            sp_wr_len: 0,
            sp_status: SP_STATUS_HALT,
            sp_pc: 0,
            sp_semaphore: false,
            active_sp_dma: None,
            queued_sp_dma: None,
            sp_dma_setup_cycles: Cycles::new(8),
            vi_registers: [0; 14],
            tv_type: None,
            vi_field_interval: None,
            vi_epoch: Cycles::ZERO,
            pending_vi: None,
            pending_sp: None,
            pending_dp: None,
            events: BTreeMap::new(),
            next_event_sequence: 0,
            trace: Vec::new(),
            next_trace_sequence: 0,
        }
    }

    pub const fn now(&self) -> Cycles {
        self.now
    }

    /// Mutable access to the one PI storage engine for synchronous save-chip
    /// protocols and host configuration. Timed transfers still enter through
    /// [`Self::start_pi_dma`] or [`Self::write_mmio`].
    pub fn pi_dma_mut(&mut self) -> &mut PiDma<R> {
        &mut self.pi_dma
    }

    pub const fn pending_pi_request(&self) -> Option<PiDmaRequest> {
        match self.pending_pi {
            Some(pending) => Some(pending.request),
            None => None,
        }
    }

    pub const fn pending_si_request(&self) -> Option<SiDmaRequest> {
        match self.pending_si {
            Some(pending) => Some(pending.request),
            None => None,
        }
    }

    pub fn snapshot(&self) -> DeviceSnapshot {
        DeviceSnapshot {
            now: self.now,
            pi_dram_addr: self.pi_dram_addr,
            pi_cart_addr: self.pi_cart_addr,
            pi_status: self.pi_status,
            ai_status: self.ai_status(),
            ai_length: self.ai_length(),
            si_dram_addr: self.si_dram_addr,
            si_status: self.si_status(),
            vi_current: self.vi_current(),
            vi_intr: self.vi_registers[3],
            vi_v_sync: self.vi_registers[6],
            tv_type: self.tv_type,
            vi_field_interval: self.vi_field_interval,
            sp_busy: self.pending_sp.is_some(),
            sp_status: self.sp_status(),
            sp_mem_addr: self.sp_mem_addr,
            sp_dram_addr: self.sp_dram_addr,
            sp_imem_generation: self.rsp_memory.imem_generation(),
            dp_busy: self.pending_dp.is_some(),
            mi_pending: self.mi_pending,
            mi_mask: self.mi_mask,
            pi_domain1: self.pi_domain1,
            pi_domain2: self.pi_domain2,
        }
    }

    pub const fn rsp_memory(&self) -> &RspMemory {
        &self.rsp_memory
    }

    /// Mutable access for the one RSP execution engine owned by the host.
    /// Device DMA and the interpreter are never advanced concurrently.
    pub fn rsp_memory_mut(&mut self) -> &mut RspMemory {
        &mut self.rsp_memory
    }

    pub const fn sp_status(&self) -> u32 {
        let mut status = self.sp_status;
        if self.active_sp_dma.is_some() {
            status |= SP_STATUS_DMA_BUSY;
        }
        if self.queued_sp_dma.is_some() {
            status |= SP_STATUS_DMA_FULL;
        }
        status
    }

    pub const fn sp_dma_busy(&self) -> bool {
        self.active_sp_dma.is_some() || self.queued_sp_dma.is_some()
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

    pub fn raise_interrupt(&mut self, source: InterruptSource) {
        if self.mi_pending & source.bit() == 0 {
            self.mi_pending |= source.bit();
            self.record(DeviceTraceKind::MiInterruptRaised(source));
        }
    }

    pub fn clear_interrupt(&mut self, source: InterruptSource) {
        if self.mi_pending & source.bit() != 0 {
            self.mi_pending &= !source.bit();
            self.record(DeviceTraceKind::MiInterruptCleared(source));
        }
    }

    pub fn cpu_interrupt_pending(&self) -> bool {
        self.mi_pending & self.mi_mask != 0
    }

    pub const fn ai_status(&self) -> u32 {
        let mut status = 0;
        if self.current_ai.is_some() {
            status |= AI_STATUS_BUSY;
        }
        if self.queued_ai.is_some() {
            status |= AI_STATUS_FULL;
        }
        status
    }

    /// Guest-visible bytes remaining in the active DMA. The device fabric is
    /// advanced at every translated checkpoint, so this interpolation is a
    /// deterministic function of guest time and never host callback jitter.
    pub fn ai_length(&self) -> u32 {
        let Some(current) = self.current_ai else {
            return 0;
        };
        let duration = current.deadline.get() - current.started_at.get();
        let remaining_cycles = current.deadline.get().saturating_sub(self.now.get());
        let remaining = (u128::from(current.request.len) * u128::from(remaining_cycles))
            .div_ceil(u128::from(duration));
        u32::try_from(remaining).expect("AI remaining length exceeds u32")
    }

    pub const fn si_status(&self) -> u32 {
        let mut status = 0;
        if self.pending_si.is_some() {
            status |= 1;
        }
        if self.si_dma_error {
            status |= 1 << 3;
        }
        if self.mi_pending & InterruptSource::Si.bit() != 0 {
            status |= 1 << 12;
        }
        status
    }

    /// Current VI field selected by `VI_CURRENT` bit zero. Public `rcp.h` and
    /// the `osViGetCurrentField` manual define it as zero in non-interlaced
    /// mode and alternating zero/one for interlaced fields.
    pub fn vi_field(&self) -> u32 {
        const VI_STATUS_SERRATE: u32 = 1 << 6;
        if self.vi_registers[0] & VI_STATUS_SERRATE == 0 {
            return 0;
        }
        self.vi_field_interval.map_or(0, |interval| {
            ((self.now.get().saturating_sub(self.vi_epoch.get()) / interval.get()) & 1) as u32
        })
    }

    pub const fn tv_type(&self) -> Option<TvType> {
        self.tv_type
    }

    pub const fn vi_field_interval(&self) -> Option<Cycles> {
        self.vi_field_interval
    }

    /// Current sampled VI half-line. The public VI manual defines V_CURRENT
    /// as an even sequence `0,2,...` in non-interlaced mode and alternating
    /// even/odd sequences in interlaced mode. The caller-supplied field
    /// interval supplies the deterministic time base while VI_V_SYNC supplies
    /// the field size. Before either is configured the hardware-facing value
    /// remains zero.
    pub fn vi_current(&self) -> u32 {
        let Some(interval) = self.vi_field_interval else {
            return 0;
        };
        let total = self.vi_registers[6] & 0x3ff;
        if total == 0 {
            return 0;
        }
        let elapsed = self.now.get().saturating_sub(self.vi_epoch.get());
        let phase = elapsed % interval.get();
        let field = self.vi_field();
        let lines_in_field = (total + 1 - field) / 2;
        if lines_in_field == 0 {
            return field;
        }
        let line = u32::try_from(
            (u128::from(phase) * u128::from(lines_in_field)) / u128::from(interval.get()),
        )
        .expect("VI line exceeds u32");
        line * 2 + field
    }

    /// Install an explicit field-duration override for compatibility tests or
    /// embedders without IPL state. This clears the typed television standard;
    /// production boot should call [`Self::configure_tv_type`] instead.
    pub fn arm_vi(&mut self, interval: Cycles) -> Result<(), DeviceFault> {
        if interval.get() == 0 {
            return Err(DeviceFault::ZeroViInterval);
        }
        self.tv_type = None;
        self.vi_field_interval = Some(interval);
        self.vi_epoch = self.now;
        self.reschedule_vi_interrupt()
    }

    /// Select the IPL television standard and arm VI from its public clock.
    /// Before a mode supplies H_SYNC/V_SYNC, the public nominal 60/50 Hz rate
    /// is used. Register writes replace that bootstrap interval with the
    /// programmed mode-derived duration.
    pub fn configure_tv_type(&mut self, tv_type: TvType) -> Result<Cycles, DeviceFault> {
        self.tv_type = Some(tv_type);
        self.refresh_vi_interval_from_standard()?;
        Ok(self
            .vi_field_interval
            .expect("configured television standard must arm VI"))
    }

    fn refresh_vi_interval_from_standard(&mut self) -> Result<(), DeviceFault> {
        let Some(tv_type) = self.tv_type else {
            return self.reschedule_vi_interrupt();
        };
        let interval = tv_type
            .programmed_field_cycles(self.vi_registers[7], self.vi_registers[6])
            .unwrap_or_else(|| tv_type.nominal_field_cycles());
        self.vi_field_interval = Some(Cycles::new(interval));
        self.vi_epoch = self.now;
        self.reschedule_vi_interrupt()
    }

    fn vi_interrupt_offset(&self, interval: Cycles) -> Cycles {
        let total = self.vi_registers[6] & 0x3ff;
        if total == 0 {
            return interval;
        }
        let target = (self.vi_registers[3] & 0x3ff).min(total.saturating_sub(1));
        if target == 0 {
            return interval;
        }
        let offset = (u128::from(interval.get()) * u128::from(target)).div_ceil(u128::from(total));
        Cycles::new(
            u64::try_from(offset)
                .expect("VI interrupt offset exceeds u64")
                .max(1),
        )
    }

    fn reschedule_vi_interrupt(&mut self) -> Result<(), DeviceFault> {
        let Some(interval) = self.vi_field_interval else {
            return Ok(());
        };
        let offset = self.vi_interrupt_offset(interval).get();
        let elapsed = self.now.get().saturating_sub(self.vi_epoch.get());
        let field = elapsed / interval.get();
        let mut deadline = self
            .vi_epoch
            .get()
            .checked_add(
                field
                    .checked_mul(interval.get())
                    .ok_or(DeviceFault::DeadlineOverflow)?,
            )
            .and_then(|base| base.checked_add(offset))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        if deadline <= self.now.get() {
            deadline = deadline
                .checked_add(interval.get())
                .ok_or(DeviceFault::DeadlineOverflow)?;
        }
        if let Some(stale_token) = self.pending_vi.take() {
            self.events.retain(
                |_, event| !matches!(event, DeviceEvent::Vi { token } if *token == stale_token),
            );
        }
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.pending_vi = Some(token);
        self.events
            .insert((Cycles::new(deadline), token), DeviceEvent::Vi { token });
        Ok(())
    }

    pub fn set_si_latency(&mut self, latency: Cycles) {
        assert!(latency.get() > 0, "SI latency must be nonzero");
        self.si_latency = latency;
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
            .insert((deadline, token), DeviceEvent::Pi { token });
        self.record(DeviceTraceKind::PiDmaStarted(request));
        Ok(())
    }

    /// Enqueue one AI buffer in the hardware's current/next two-slot FIFO.
    /// Timing uses the N64 CPU clock and four bytes per stereo 16-bit frame;
    /// the explicit ceiling prevents a nonempty buffer from completing early.
    pub fn start_ai_dma(&mut self, request: AiDmaRequest) -> Result<(), DeviceFault> {
        if request.len == 0 {
            return Err(DeviceFault::ZeroLengthAiDma);
        }
        if request.sample_rate_hz == 0 {
            return Err(DeviceFault::ZeroAiSampleRate);
        }
        if self.current_ai.is_none() {
            self.begin_ai_dma(request, self.now)?;
        } else if self.queued_ai.is_none() {
            self.queued_ai = Some(request);
        } else {
            return Err(DeviceFault::AiFull);
        }
        Ok(())
    }

    fn begin_ai_dma(
        &mut self,
        request: AiDmaRequest,
        started_at: Cycles,
    ) -> Result<(), DeviceFault> {
        const BYTES_PER_STEREO_FRAME: u128 = 4;
        let frames = u128::from(request.len).div_ceil(BYTES_PER_STEREO_FRAME);
        let duration =
            (frames * u128::from(CPU_CLOCK_HZ)).div_ceil(u128::from(request.sample_rate_hz));
        let duration = u64::try_from(duration.max(1)).map_err(|_| DeviceFault::DeadlineOverflow)?;
        let deadline = started_at
            .checked_add(Cycles::new(duration))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.current_ai = Some(PendingAi {
            token,
            request,
            started_at,
            deadline,
        });
        self.events
            .insert((deadline, token), DeviceEvent::Ai { token });
        self.record(DeviceTraceKind::AiDmaStarted(request));
        Ok(())
    }

    pub fn start_si_dma(&mut self, request: SiDmaRequest) -> Result<(), DeviceFault> {
        if self.pending_si.is_some() {
            self.si_dma_error = true;
            return Err(DeviceFault::SiBusy);
        }
        let deadline = self
            .now
            .checked_add(self.si_latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.si_dram_addr = request.dram_addr;
        self.pending_si = Some(PendingSi { token, request });
        self.events
            .insert((deadline, token), DeviceEvent::Si { token });
        self.record(DeviceTraceKind::SiDmaStarted(request));
        Ok(())
    }

    fn validate_sp_dma(request: SpDmaRequest) -> Result<(), DeviceFault> {
        let total = request.total_bytes();
        let remaining = RSP_MEMORY_BANK_SIZE - request.mem_addr.offset();
        if total > remaining {
            return Err(DeviceFault::SpDmaMemory(RspMemoryError::CrossesBank {
                addr: request.mem_addr,
                len: total,
            }));
        }
        let row_stride = request
            .line_len()
            .checked_add(request.skip())
            .ok_or(DeviceFault::SpDmaDramRangeOverflow { request })?;
        let last_row = request
            .line_count()
            .saturating_sub(1)
            .checked_mul(row_stride)
            .ok_or(DeviceFault::SpDmaDramRangeOverflow { request })?;
        let end = (request.dram_addr.offset() as usize)
            .checked_add(last_row)
            .and_then(|start| start.checked_add(request.line_len()))
            .ok_or(DeviceFault::SpDmaDramRangeOverflow { request })?;
        if end > 0x0100_0000 {
            return Err(DeviceFault::SpDmaDramRangeOverflow { request });
        }
        Ok(())
    }

    fn begin_sp_dma(&mut self, request: SpDmaRequest) -> Result<(), DeviceFault> {
        let transfer_cycles =
            u64::try_from(request.total_bytes() / 8).map_err(|_| DeviceFault::DeadlineOverflow)?;
        let latency = self
            .sp_dma_setup_cycles
            .checked_add(Cycles::new(transfer_cycles))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let deadline = self
            .now
            .checked_add(latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.active_sp_dma = Some(PendingSpDma { token, request });
        self.events
            .insert((deadline, token), DeviceEvent::SpDma { token });
        self.record(DeviceTraceKind::SpDmaStarted(request));
        Ok(())
    }

    fn start_sp_dma(&mut self, request: SpDmaRequest) -> Result<(), DeviceFault> {
        Self::validate_sp_dma(request)?;
        if self.active_sp_dma.is_none() {
            self.begin_sp_dma(request)
        } else if self.queued_sp_dma.is_none() {
            self.queued_sp_dma = Some(request);
            self.record(DeviceTraceKind::SpDmaQueued(request));
            Ok(())
        } else {
            Err(DeviceFault::SpDmaFull)
        }
    }

    /// Apply the SP status command register's documented clear/set pairs.
    pub fn write_sp_status(&mut self, command: u32) {
        if command & (1 << 0) != 0 {
            self.sp_status &= !SP_STATUS_HALT;
        }
        if command & (1 << 1) != 0 {
            self.sp_status |= SP_STATUS_HALT;
        }
        if command & (1 << 2) != 0 {
            self.sp_status &= !SP_STATUS_BROKE;
        }
        if command & (1 << 3) != 0 {
            self.clear_interrupt(InterruptSource::Sp);
        }
        if command & (1 << 4) != 0 {
            self.raise_interrupt(InterruptSource::Sp);
        }
        apply_device_clear_set_pair(&mut self.sp_status, command, 5, 6, SP_STATUS_SINGLE_STEP);
        apply_device_clear_set_pair(
            &mut self.sp_status,
            command,
            7,
            8,
            SP_STATUS_INTERRUPT_ON_BREAK,
        );
        for signal in 0..8 {
            apply_device_clear_set_pair(
                &mut self.sp_status,
                command,
                9 + signal * 2,
                10 + signal * 2,
                1 << (7 + signal),
            );
        }
    }

    pub fn set_sp_pc(&mut self, pc: u32) {
        self.sp_pc = pc & 0x0ffc;
    }

    pub const fn sp_pc(&self) -> u32 {
        self.sp_pc
    }

    /// Commit architectural state produced by a synchronous RSP execution.
    /// DMA BUSY/FULL are derived from the fabric's queues and cannot be
    /// overwritten by an interpreter snapshot.
    pub fn commit_rsp_execution_state(&mut self, pc: u32, status: u32) {
        self.set_sp_pc(pc);
        self.sp_status = status & !(SP_STATUS_DMA_BUSY | SP_STATUS_DMA_FULL);
    }

    /// Complete the CPU-side `osSpTaskLoad` admission sequence at its shim
    /// return boundary. The public RSP guide's "Starting RSP Tasks" algorithm
    /// requires the 64-byte `OSTask` at DMEM `0xfc0`, rspboot at IMEM `0`, and
    /// PC `0`. Raw SP DMA remains independently timed; this helper represents
    /// the two DMA-and-poll loops as already complete when the synchronous OS
    /// function returns.
    pub fn admit_sp_task<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &M,
        task_addr: RdramAddr,
        header: crate::rsp::OsTaskHeader,
    ) -> Result<(), DeviceFault> {
        if self.sp_status() & SP_STATUS_HALT == 0 || self.pending_sp.is_some() {
            return Err(DeviceFault::SpTaskNotHalted);
        }
        if self.active_sp_dma.is_some() || self.queued_sp_dma.is_some() {
            return Err(DeviceFault::SpDmaFull);
        }
        let boot_size = header
            .ucode_boot_size
            .checked_add(7)
            .map(|size| size & !7)
            .filter(|size| *size != 0 && *size as usize <= RSP_MEMORY_BANK_SIZE)
            .ok_or(DeviceFault::InvalidSpTaskBootSize {
                size: header.ucode_boot_size,
            })? as usize;
        let task_bytes = rdram.dma_read_bytes_flat(task_addr.offset() as usize, 64);
        self.rsp_memory
            .write_bytes(RspMemAddr::from_register(0x0fc0), &task_bytes)
            .map_err(DeviceFault::SpDmaMemory)?;

        // OSTask pointers may be physical or direct-mapped KSEG0/KSEG1.
        // Both reduce to the public 29-bit physical bus address this way.
        let boot_addr = (header.ucode_boot & 0x1fff_ffff) & !7;
        let boot = rdram.dma_read_bytes_flat(boot_addr as usize, boot_size);
        self.rsp_memory
            .write_bytes(RspMemAddr::from_register(0x1000), &boot)
            .map_err(DeviceFault::SpDmaMemory)?;
        self.sp_mem_addr = RspMemAddr::from_register(0x1000);
        self.sp_dram_addr = RdramAddr::from_offset(boot_addr);
        self.sp_pc = 0;
        self.record(DeviceTraceKind::SpTaskAdmitted { task_addr, header });
        Ok(())
    }

    /// Schedule the externally visible completion of work already executed by
    /// the HLE task backend. SP completes one deterministic guest cycle after
    /// the kick; graphics DP completion follows one cycle later, preserving
    /// the architectural SP-before-DP ordering without claiming RDP timing.
    pub fn start_rcp_task(&mut self, needs_dp: bool) -> Result<(), DeviceFault> {
        self.start_rcp_task_with_latency(needs_dp, Cycles::new(1))
    }

    /// Schedule completion after a measured amount of synchronous RSP work.
    /// The caller has already executed that work while the guest is suspended;
    /// this delay controls only when its architectural interrupt is observable.
    pub fn start_rcp_task_with_latency(
        &mut self,
        needs_dp: bool,
        sp_latency: Cycles,
    ) -> Result<(), DeviceFault> {
        if self.pending_sp.is_some() {
            return Err(DeviceFault::SpBusy);
        }
        if needs_dp && self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        let sp_deadline = self
            .now
            .checked_add(sp_latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let sp_token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.pending_sp = Some(sp_token);
        self.sp_status &= !(SP_STATUS_HALT | SP_STATUS_BROKE);
        self.events
            .insert((sp_deadline, sp_token), DeviceEvent::Sp { token: sp_token });
        if needs_dp {
            let dp_deadline = self
                .now
                .checked_add(
                    sp_latency
                        .checked_add(Cycles::new(1))
                        .ok_or(DeviceFault::DeadlineOverflow)?,
                )
                .ok_or(DeviceFault::DeadlineOverflow)?;
            let dp_token = self.next_event_sequence;
            self.next_event_sequence = self
                .next_event_sequence
                .checked_add(1)
                .ok_or(DeviceFault::DeadlineOverflow)?;
            self.pending_dp = Some(dp_token);
            self.events
                .insert((dp_deadline, dp_token), DeviceEvent::Dp { token: dp_token });
        }
        self.record(DeviceTraceKind::RcpTaskStarted { needs_dp });
        Ok(())
    }

    pub fn read_mmio(&mut self, addr: MmioAddr) -> Result<u32, DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            addr if (SP_DMEM_START..SP_IMEM_END).contains(&addr.get()) => self
                .rsp_memory
                .read_word(RspMemAddr::from_register(addr.get() - SP_DMEM_START))
                .map_err(DeviceFault::SpDmaMemory),
            SP_MEM_ADDR_REG => Ok(self.sp_mem_addr.get() as u32),
            SP_DRAM_ADDR_REG => Ok(self.sp_dram_addr.offset()),
            SP_RD_LEN_REG => Ok(self.sp_rd_len),
            SP_WR_LEN_REG => Ok(self.sp_wr_len),
            SP_STATUS_REG => Ok(self.sp_status()),
            SP_DMA_FULL_REG => Ok(u32::from(self.queued_sp_dma.is_some())),
            SP_DMA_BUSY_REG => Ok(u32::from(self.active_sp_dma.is_some())),
            SP_SEMAPHORE_REG => {
                let previous = u32::from(self.sp_semaphore);
                self.sp_semaphore = true;
                Ok(previous)
            }
            SP_PC_REG => Ok(self.sp_pc),
            MI_INTR_REG => Ok(self.mi_pending),
            MI_INTR_MASK_REG => Ok(self.mi_mask),
            VI_CURRENT_REG => Ok(self.vi_current()),
            addr if (VI_STATUS_REG.get()..=VI_Y_SCALE_REG.get()).contains(&addr.get()) => {
                let index = ((addr.get() - VI_STATUS_REG.get()) / 4) as usize;
                Ok(self.vi_registers[index])
            }
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
            SI_DRAM_ADDR_REG => Ok(self.si_dram_addr.offset()),
            SI_STATUS_REG => Ok(self.si_status()),
            _ => Err(DeviceFault::UnmodeledMmioRead { addr }),
        }
    }

    pub fn write_mmio(&mut self, addr: MmioAddr, value: u32) -> Result<(), DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            addr if (SP_DMEM_START..SP_IMEM_END).contains(&addr.get()) => self
                .rsp_memory
                .write_word(RspMemAddr::from_register(addr.get() - SP_DMEM_START), value)
                .map_err(DeviceFault::SpDmaMemory),
            SP_MEM_ADDR_REG => {
                self.sp_mem_addr = RspMemAddr::from_register(value);
                Ok(())
            }
            SP_DRAM_ADDR_REG => {
                self.sp_dram_addr = RdramAddr::from_offset(value & 0x00ff_ffff);
                Ok(())
            }
            SP_RD_LEN_REG => {
                self.sp_rd_len = value;
                self.start_sp_dma(SpDmaRequest {
                    direction: SpDmaDirection::RdramToRsp,
                    mem_addr: self.sp_mem_addr.dma_aligned(),
                    dram_addr: RdramAddr::from_offset(self.sp_dram_addr.offset() & !7),
                    encoded_len: value,
                })
            }
            SP_WR_LEN_REG => {
                self.sp_wr_len = value;
                self.start_sp_dma(SpDmaRequest {
                    direction: SpDmaDirection::RspToRdram,
                    mem_addr: self.sp_mem_addr.dma_aligned(),
                    dram_addr: RdramAddr::from_offset(self.sp_dram_addr.offset() & !7),
                    encoded_len: value,
                })
            }
            SP_STATUS_REG => {
                self.write_sp_status(value);
                Ok(())
            }
            SP_SEMAPHORE_REG if value == 0 => {
                self.sp_semaphore = false;
                Ok(())
            }
            SP_SEMAPHORE_REG => Err(DeviceFault::InvalidSpSemaphoreWrite { value }),
            SP_PC_REG => {
                self.set_sp_pc(value);
                Ok(())
            }
            MI_INTR_MASK_REG if value & !0x0FFF == 0 => {
                // MI_INTR_MASK is a command register, not a replacement
                // value. Public N64 hardware documentation assigns one
                // clear/set pair to each MI source, in MI_INTR bit order.
                // Apply clear before set so a malformed command containing
                // both leaves the source enabled, matching the paired
                // clear-then-set register behavior.
                for (index, source) in [
                    InterruptSource::Sp,
                    InterruptSource::Si,
                    InterruptSource::Ai,
                    InterruptSource::Vi,
                    InterruptSource::Pi,
                    InterruptSource::Dp,
                ]
                .into_iter()
                .enumerate()
                {
                    let clear = 1 << (index * 2);
                    let set = 1 << (index * 2 + 1);
                    if value & clear != 0 {
                        self.mi_mask &= !source.bit();
                    }
                    if value & set != 0 {
                        self.mi_mask |= source.bit();
                    }
                }
                Ok(())
            }
            VI_CURRENT_REG => {
                // VI_CURRENT is read-only as a counter. Any write is the
                // documented acknowledgement for the level-sensitive VI
                // source; it must not replace the sampled line value.
                self.clear_interrupt(InterruptSource::Vi);
                Ok(())
            }
            addr if (VI_STATUS_REG.get()..=VI_Y_SCALE_REG.get()).contains(&addr.get()) => {
                let index = ((addr.get() - VI_STATUS_REG.get()) / 4) as usize;
                self.vi_registers[index] = match addr {
                    VI_STATUS_REG => value & 0x1ffff,
                    VI_ORIGIN_REG => value & 0x00ff_ffff,
                    VI_INTR_REG | VI_V_SYNC_REG => value & 0x3ff,
                    _ => value,
                };
                if matches!(addr, VI_V_SYNC_REG | VI_H_SYNC_REG) {
                    if self.tv_type.is_some() {
                        self.refresh_vi_interval_from_standard()?;
                    } else if addr == VI_V_SYNC_REG {
                        self.reschedule_vi_interrupt()?;
                    }
                } else if addr == VI_INTR_REG {
                    self.reschedule_vi_interrupt()?;
                }
                Ok(())
            }
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
                    self.clear_interrupt(InterruptSource::Pi);
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
            SI_DRAM_ADDR_REG => {
                self.si_dram_addr = RdramAddr::from_offset(value & 0x00FF_FFFF);
                Ok(())
            }
            SI_PIF_ADDR_RD64B_REG => self.start_si_dma(SiDmaRequest {
                kind: SiDmaKind::PifToDram,
                dram_addr: self.si_dram_addr,
            }),
            SI_PIF_ADDR_WR64B_REG => self.start_si_dma(SiDmaRequest {
                kind: SiDmaKind::DramToPif,
                dram_addr: self.si_dram_addr,
            }),
            SI_STATUS_REG => {
                self.clear_interrupt(InterruptSource::Si);
                Ok(())
            }
            _ => Err(DeviceFault::UnmodeledMmioWrite { addr, value }),
        }
    }

    /// Advance deterministic device time and fully commit every due event.
    /// Notifications are returned only after their device and MI state is
    /// guest-visible, so the executor can post them before resuming a thread.
    pub fn advance_to<M: DmaMemory + ?Sized>(
        &mut self,
        requested: Cycles,
        rdram: &mut M,
    ) -> Result<Vec<DeviceNotification>, DeviceFault> {
        self.advance_to_with_pif(requested, rdram, |_, _, _| {
            panic!("SI DRAM-to-PIF completion requires a PIF command handler")
        })
    }

    pub fn advance_to_with_pif<M: DmaMemory + ?Sized>(
        &mut self,
        requested: Cycles,
        rdram: &mut M,
        mut execute_pif: impl FnMut(Cycles, &mut [u8; 64], &mut PiDma<R>),
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
            self.pi_dma.advance_eeprom_to(self.now);
            match event {
                DeviceEvent::Pi { token } => {
                    let Some(pending) = self.pending_pi else {
                        continue;
                    };
                    if pending.token != token {
                        continue;
                    }
                    let request = pending.request;
                    let completion = self
                        .pi_dma
                        .try_start_dma(
                            rdram,
                            request.direction,
                            request.dram_addr,
                            request.cart_addr,
                            request.len,
                        )
                        .map_err(DeviceFault::PiTransfer)?;
                    self.record(DeviceTraceKind::PiBytesCommitted(request));
                    self.pending_pi = None;
                    self.pi_status &= !PI_STATUS_DMA_BUSY;
                    self.record(DeviceTraceKind::PiBusyCleared);
                    self.raise_interrupt(InterruptSource::Pi);
                    let notification = DeviceNotification::PiDmaComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
                DeviceEvent::Ai { token } => {
                    let Some(current) = self.current_ai else {
                        continue;
                    };
                    if current.token != token {
                        continue;
                    }
                    self.current_ai = None;
                    self.record(DeviceTraceKind::AiDmaComplete(current.request));
                    self.raise_interrupt(InterruptSource::Ai);
                    let notification = DeviceNotification::AiDmaComplete(current.request);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                    if let Some(next) = self.queued_ai.take() {
                        self.begin_ai_dma(next, key.0)?;
                    }
                }
                DeviceEvent::Si { token } => {
                    let Some(pending) = self.pending_si else {
                        continue;
                    };
                    if pending.token != token {
                        continue;
                    }
                    let request = pending.request;
                    match request.kind {
                        SiDmaKind::DramToPif => {
                            let bytes =
                                rdram.dma_read_bytes_flat(request.dram_addr.offset() as usize, 64);
                            self.pif_ram.copy_from_slice(&bytes);
                            execute_pif(self.now, &mut self.pif_ram, &mut self.pi_dma);
                        }
                        SiDmaKind::PifToDram => rdram
                            .dma_write_bytes(request.dram_addr.offset() as usize, &self.pif_ram),
                        SiDmaKind::ControllerQuery | SiDmaKind::ControllerRead => {}
                    }
                    self.record(DeviceTraceKind::SiBytesCommitted(request));
                    self.pending_si = None;
                    self.record(DeviceTraceKind::SiBusyCleared);
                    self.raise_interrupt(InterruptSource::Si);
                    let notification = DeviceNotification::SiDmaComplete(request);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
                DeviceEvent::SpDma { token } => {
                    let Some(active) = self.active_sp_dma else {
                        continue;
                    };
                    if active.token != token {
                        continue;
                    }
                    let request = active.request;
                    let line_len = request.line_len();
                    let row_stride = line_len + request.skip();
                    match request.direction {
                        SpDmaDirection::RdramToRsp => {
                            let mut bytes = Vec::with_capacity(request.total_bytes());
                            for row in 0..request.line_count() {
                                let offset = request.dram_addr.offset() as usize + row * row_stride;
                                bytes.extend(rdram.dma_read_bytes_flat(offset, line_len));
                            }
                            self.rsp_memory
                                .write_bytes(request.mem_addr, &bytes)
                                .map_err(DeviceFault::SpDmaMemory)?;
                        }
                        SpDmaDirection::RspToRdram => {
                            let bytes = self
                                .rsp_memory
                                .read_bytes(request.mem_addr, request.total_bytes())
                                .map_err(DeviceFault::SpDmaMemory)?;
                            for (row, line) in bytes.chunks_exact(line_len).enumerate() {
                                let offset = request.dram_addr.offset() as usize + row * row_stride;
                                rdram.dma_write_bytes(offset, line);
                            }
                        }
                    }
                    self.record(DeviceTraceKind::SpDmaBytesCommitted(request));
                    self.active_sp_dma = None;
                    if let Some(next) = self.queued_sp_dma.take() {
                        // The public guide requires a pending request to begin
                        // before DMA_BUSY clears. Starting it in this same
                        // ordered event transition makes that intermediate
                        // false-busy state unobservable to the guest.
                        self.begin_sp_dma(next)?;
                    } else {
                        self.record(DeviceTraceKind::SpDmaBusyCleared);
                    }
                }
                DeviceEvent::Vi { token } => {
                    if self.pending_vi != Some(token) {
                        continue;
                    }
                    self.pending_vi = None;
                    self.record(DeviceTraceKind::ViInterrupt);
                    self.raise_interrupt(InterruptSource::Vi);
                    let notification = DeviceNotification::ViRetrace;
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                    self.reschedule_vi_interrupt()?;
                }
                DeviceEvent::Sp { token } => {
                    if self.pending_sp != Some(token) {
                        continue;
                    }
                    self.pending_sp = None;
                    self.sp_status |= SP_STATUS_HALT | SP_STATUS_BROKE;
                    let completion = RcpTaskCompletion::Sp;
                    self.record(DeviceTraceKind::RcpTaskComplete(completion));
                    self.raise_interrupt(InterruptSource::Sp);
                    let notification = DeviceNotification::RcpTaskComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
                DeviceEvent::Dp { token } => {
                    if self.pending_dp != Some(token) {
                        continue;
                    }
                    self.pending_dp = None;
                    let completion = RcpTaskCompletion::Dp;
                    self.record(DeviceTraceKind::RcpTaskComplete(completion));
                    self.raise_interrupt(InterruptSource::Dp);
                    let notification = DeviceNotification::RcpTaskComplete(completion);
                    notifications.push(notification);
                    self.record(DeviceTraceKind::NotificationReady(notification));
                }
            }
        }
        self.now = requested;
        self.pi_dma.advance_eeprom_to(self.now);
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
    use crate::rdram::Rdram;
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
    fn raw_mi_mask_commands_drive_the_cpu_interrupt_gate() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        let request = PiDmaRequest {
            direction: DmaDirection::ToRdram,
            dram_addr: RdramAddr::from_offset(0x20),
            cart_addr: 0x10,
            len: 4,
        };
        fabric.start_pi_dma(request).unwrap();
        fabric.advance_to(Cycles::new(12), &mut rdram).unwrap();

        assert!(!fabric.cpu_interrupt_pending());
        fabric.write_mmio(MI_INTR_MASK_REG, 1 << 9).unwrap();
        assert_eq!(
            fabric.read_mmio(MI_INTR_MASK_REG).unwrap(),
            InterruptSource::Pi.bit()
        );
        assert!(fabric.cpu_interrupt_pending());

        fabric.write_mmio(MI_INTR_MASK_REG, 1 << 8).unwrap();
        assert_eq!(fabric.read_mmio(MI_INTR_MASK_REG).unwrap(), 0);
        assert!(!fabric.cpu_interrupt_pending());
    }

    #[test]
    fn every_rcp_source_uses_the_same_level_sensitive_mi_gate() {
        let mut fabric = fabric();
        for source in [
            InterruptSource::Sp,
            InterruptSource::Si,
            InterruptSource::Ai,
            InterruptSource::Vi,
            InterruptSource::Pi,
            InterruptSource::Dp,
        ] {
            fabric.set_interrupt_mask(source, true);
            fabric.raise_interrupt(source);
            fabric.raise_interrupt(source);
            assert!(fabric.interrupt_pending(source));
            assert!(fabric.cpu_interrupt_pending());
            fabric.clear_interrupt(source);
            assert!(!fabric.interrupt_pending(source));
            fabric.set_interrupt_mask(source, false);
        }
        assert_eq!(
            fabric
                .trace()
                .iter()
                .filter(|event| matches!(event.kind, DeviceTraceKind::MiInterruptRaised(_)))
                .count(),
            6
        );
        assert_eq!(
            fabric
                .trace()
                .iter()
                .filter(|event| matches!(event.kind, DeviceTraceKind::MiInterruptCleared(_)))
                .count(),
            6
        );
    }

    #[test]
    fn ai_fifo_drains_on_guest_cycles_and_raises_one_shared_mi_source() {
        let mut fabric = fabric();
        let first = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 400,
            sample_rate_hz: 1_000_000,
        };
        let second = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x2000),
            ..first
        };
        fabric.start_ai_dma(first).unwrap();
        fabric.start_ai_dma(second).unwrap();
        assert_eq!(fabric.ai_status(), AI_STATUS_BUSY | AI_STATUS_FULL);
        assert_eq!(fabric.ai_length(), 400);
        assert_eq!(fabric.start_ai_dma(first), Err(DeviceFault::AiFull));

        let mut rdram = Rdram::new(0x100);
        assert!(fabric
            .advance_to(Cycles::new(9_374), &mut rdram)
            .unwrap()
            .is_empty());
        assert!(fabric.ai_length() > 0);
        let first_done = fabric.advance_to(Cycles::new(9_375), &mut rdram).unwrap();
        assert_eq!(first_done, vec![DeviceNotification::AiDmaComplete(first)]);
        assert_eq!(fabric.ai_status(), AI_STATUS_BUSY);
        assert_eq!(fabric.ai_length(), 400);
        assert!(fabric.interrupt_pending(InterruptSource::Ai));

        fabric.clear_interrupt(InterruptSource::Ai);
        let second_done = fabric.advance_to(Cycles::new(18_750), &mut rdram).unwrap();
        assert_eq!(second_done, vec![DeviceNotification::AiDmaComplete(second)]);
        assert_eq!(fabric.ai_status(), 0);
        assert_eq!(fabric.ai_length(), 0);
        assert!(fabric.interrupt_pending(InterruptSource::Ai));
    }

    #[test]
    fn device_clock_commits_eeprom_without_requiring_another_si_command() {
        use crate::save::{InMemorySaveStorage, SaveType, EEPROM_WRITE_CYCLES};

        let mut fabric = fabric();
        fabric
            .pi_dma_mut()
            .set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        let data = [0x3C; crate::save::EEPROM_BLOCK_SIZE];
        let deadline = fabric
            .pi_dma_mut()
            .start_eeprom_write(Cycles::ZERO, 5, data)
            .unwrap();
        assert_eq!(deadline, EEPROM_WRITE_CYCLES);

        let mut rdram = Rdram::new(0);
        fabric
            .advance_to(Cycles::new(deadline.get() - 1), &mut rdram)
            .unwrap();
        assert!(
            fabric
                .pi_dma_mut()
                .eeprom_status(Cycles::new(deadline.get() - 1))
                .unwrap()
                .busy
        );
        fabric.advance_to(deadline, &mut rdram).unwrap();
        assert_eq!(
            fabric.pi_dma_mut().eeprom_read_block(deadline, 5).unwrap(),
            data
        );
    }

    #[test]
    fn si_write_execute_read_uses_one_timed_pif_ram_and_mi_latch() {
        let mut fabric = fabric();
        fabric.set_si_latency(Cycles::new(5));
        let mut rdram = Rdram::new(0x200);
        rdram.dma_write_bytes(0x40, &[1, 3, 0xFF, 0]);

        fabric.write_mmio(SI_DRAM_ADDR_REG, 0x40).unwrap();
        fabric.write_mmio(SI_PIF_ADDR_WR64B_REG, 0).unwrap();
        assert_eq!(fabric.si_status() & 1, 1);
        assert!(fabric
            .advance_to_with_pif(Cycles::new(4), &mut rdram, |_, _, _| unreachable!())
            .unwrap()
            .is_empty());
        let write_done = fabric
            .advance_to_with_pif(Cycles::new(5), &mut rdram, |_, pif, _| {
                assert_eq!(&pif[..4], &[1, 3, 0xFF, 0]);
                pif[3..6].copy_from_slice(&[0x05, 0, 0]);
            })
            .unwrap();
        assert_eq!(
            write_done,
            vec![DeviceNotification::SiDmaComplete(SiDmaRequest {
                kind: SiDmaKind::DramToPif,
                dram_addr: RdramAddr::from_offset(0x40),
            })]
        );
        assert_eq!(fabric.si_status(), 1 << 12);
        fabric.write_mmio(SI_STATUS_REG, 0).unwrap();

        fabric.write_mmio(SI_DRAM_ADDR_REG, 0x80).unwrap();
        fabric.write_mmio(SI_PIF_ADDR_RD64B_REG, 0).unwrap();
        fabric
            .advance_to_with_pif(Cycles::new(10), &mut rdram, |_, _, _| unreachable!())
            .unwrap();
        assert_eq!(rdram.dma_read_bytes_flat(0x83, 3), vec![0x05, 0, 0]);
        assert!(fabric.interrupt_pending(InterruptSource::Si));
    }

    #[test]
    fn sp_rectangular_dma_is_aligned_timed_and_replaces_imem_once() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        rdram.dma_write_bytes(0x20, &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17]);
        rdram.dma_write_bytes(0x30, &[0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27]);

        fabric.write_mmio(SP_MEM_ADDR_REG, 0x1003).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x23).unwrap();
        let encoded = (8 << 20) | (1 << 12);
        fabric.write_mmio(SP_RD_LEN_REG, encoded).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 1);
        assert_eq!(
            fabric.read_mmio(SP_STATUS_REG).unwrap() & SP_STATUS_DMA_BUSY,
            SP_STATUS_DMA_BUSY
        );
        assert_eq!(fabric.snapshot().sp_imem_generation, 0);

        assert!(fabric
            .advance_to(Cycles::new(9), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0x1000), 16)
                .unwrap(),
            [0; 16]
        );

        assert!(fabric
            .advance_to(Cycles::new(10), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0x1000), 16)
                .unwrap(),
            [
                0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25,
                0x26, 0x27,
            ]
        );
        assert_eq!(fabric.snapshot().sp_imem_generation, 1);
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 0);
    }

    #[test]
    fn sp_dma_pending_slot_starts_before_busy_can_clear() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        rdram.dma_write_bytes(0x20, &[1; 8]);
        rdram.dma_write_bytes(0x30, &[2; 8]);

        fabric.write_mmio(SP_MEM_ADDR_REG, 0).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x20).unwrap();
        fabric.write_mmio(SP_RD_LEN_REG, 7).unwrap();
        fabric.write_mmio(SP_MEM_ADDR_REG, 8).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x30).unwrap();
        fabric.write_mmio(SP_RD_LEN_REG, 7).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_FULL_REG).unwrap(), 1);
        assert_eq!(
            fabric.write_mmio(SP_RD_LEN_REG, 7),
            Err(DeviceFault::SpDmaFull)
        );

        fabric.advance_to(Cycles::new(9), &mut rdram).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 1);
        assert_eq!(fabric.read_mmio(SP_DMA_FULL_REG).unwrap(), 0);
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0), 16)
                .unwrap(),
            [1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0]
        );

        fabric.advance_to(Cycles::new(18), &mut rdram).unwrap();
        assert_eq!(fabric.read_mmio(SP_DMA_BUSY_REG).unwrap(), 0);
        assert_eq!(
            fabric
                .rsp_memory()
                .read_bytes(RspMemAddr::from_register(0), 16)
                .unwrap(),
            [1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2]
        );
        assert_eq!(
            fabric
                .trace()
                .iter()
                .filter(|event| matches!(event.kind, DeviceTraceKind::SpDmaBusyCleared))
                .count(),
            1
        );
    }

    #[test]
    fn cpu_sp_memory_pc_status_semaphore_and_write_dma_share_one_state() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);

        fabric
            .write_mmio(MmioAddr::new(0xA400_0040), 0xDEAD_BEEF)
            .unwrap();
        assert_eq!(
            fabric.read_mmio(MmioAddr::new(0xA400_0040)).unwrap(),
            0xDEAD_BEEF
        );
        fabric.write_mmio(SP_PC_REG, 0x1ABC).unwrap();
        assert_eq!(fabric.read_mmio(SP_PC_REG).unwrap(), 0x0ABC);
        assert_eq!(fabric.read_mmio(SP_SEMAPHORE_REG).unwrap(), 0);
        assert_eq!(fabric.read_mmio(SP_SEMAPHORE_REG).unwrap(), 1);
        fabric.write_mmio(SP_SEMAPHORE_REG, 0).unwrap();
        assert_eq!(fabric.read_mmio(SP_SEMAPHORE_REG).unwrap(), 0);

        fabric
            .write_mmio(SP_STATUS_REG, (1 << 0) | SP_SET_YIELD)
            .unwrap();
        assert_eq!(fabric.read_mmio(SP_STATUS_REG).unwrap(), SP_STATUS_YIELD);
        fabric.write_mmio(SP_MEM_ADDR_REG, 0x40).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0x80).unwrap();
        fabric.write_mmio(SP_WR_LEN_REG, 7).unwrap();
        fabric.advance_to(Cycles::new(9), &mut rdram).unwrap();
        assert_eq!(
            rdram.dma_read_bytes_flat(0x80, 8),
            [0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0]
        );
    }

    #[test]
    fn sp_dma_crossing_a_memory_bank_is_a_named_fault() {
        let mut fabric = fabric();
        fabric.write_mmio(SP_MEM_ADDR_REG, 0x0ff8).unwrap();
        fabric.write_mmio(SP_DRAM_ADDR_REG, 0).unwrap();
        let request = SpDmaRequest {
            direction: SpDmaDirection::RdramToRsp,
            mem_addr: RspMemAddr::from_register(0x0ff8),
            dram_addr: RdramAddr::from_offset(0),
            encoded_len: 15,
        };
        assert_eq!(
            fabric.write_mmio(SP_RD_LEN_REG, 15),
            Err(DeviceFault::SpDmaMemory(RspMemoryError::CrossesBank {
                addr: request.mem_addr,
                len: request.total_bytes(),
            }))
        );
    }

    #[test]
    fn graphics_task_completes_sp_then_dp_on_distinct_guest_cycles() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric.start_rcp_task(true).unwrap();
        assert!(fabric.snapshot().sp_busy);
        assert!(fabric.snapshot().dp_busy);
        assert!(fabric
            .advance_to(Cycles::new(0), &mut rdram)
            .unwrap()
            .is_empty());

        let sp = fabric.advance_to(Cycles::new(1), &mut rdram).unwrap();
        assert_eq!(
            sp,
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Sp)]
        );
        assert!(!fabric.snapshot().sp_busy);
        assert!(fabric.snapshot().dp_busy);
        assert!(fabric.interrupt_pending(InterruptSource::Sp));
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));

        let dp = fabric.advance_to(Cycles::new(2), &mut rdram).unwrap();
        assert_eq!(
            dp,
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
        assert!(!fabric.snapshot().dp_busy);
        assert!(fabric.interrupt_pending(InterruptSource::Dp));
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

    #[test]
    fn vi_half_line_interrupt_latches_mi_before_notification_and_ack_preserves_line() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0);
        fabric.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        fabric.write_mmio(VI_INTR_REG, 100).unwrap();
        fabric.arm_vi(Cycles::new(1_000)).unwrap();

        assert!(fabric
            .advance_to(Cycles::new(190), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(fabric.read_mmio(VI_CURRENT_REG).unwrap(), 98);

        let notifications = fabric.advance_to(Cycles::new(191), &mut rdram).unwrap();
        assert_eq!(notifications, vec![DeviceNotification::ViRetrace]);
        assert_eq!(fabric.read_mmio(VI_CURRENT_REG).unwrap(), 100);
        assert!(fabric.interrupt_pending(InterruptSource::Vi));

        let tail = &fabric.trace()[fabric.trace().len() - 3..];
        assert_eq!(tail[0].kind, DeviceTraceKind::ViInterrupt);
        assert_eq!(
            tail[1].kind,
            DeviceTraceKind::MiInterruptRaised(InterruptSource::Vi)
        );
        assert_eq!(
            tail[2].kind,
            DeviceTraceKind::NotificationReady(DeviceNotification::ViRetrace)
        );

        fabric.write_mmio(VI_CURRENT_REG, u32::MAX).unwrap();
        assert!(!fabric.interrupt_pending(InterruptSource::Vi));
        assert_eq!(fabric.read_mmio(VI_CURRENT_REG).unwrap(), 100);
    }

    #[test]
    fn television_standard_bootstraps_nominal_vi_then_registers_derive_the_field() {
        let mut fabric = fabric();
        assert_eq!(
            fabric.configure_tv_type(TvType::Pal).unwrap(),
            Cycles::new(1_875_000)
        );
        assert_eq!(fabric.tv_type(), Some(TvType::Pal));

        fabric.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        assert_eq!(
            fabric.vi_field_interval(),
            Some(Cycles::new(1_875_000)),
            "one zero timing register retains the nominal bootstrap"
        );
        fabric.write_mmio(VI_H_SYNC_REG, 3_093).unwrap();
        assert_eq!(
            fabric.vi_field_interval(),
            Some(Cycles::new(
                TvType::Pal.programmed_field_cycles(3_093, 525).unwrap()
            ))
        );
        assert_eq!(fabric.snapshot().tv_type, Some(TvType::Pal));
    }

    #[test]
    fn vi_current_and_field_follow_progressive_and_interlaced_half_line_sequences() {
        let mut progressive = fabric();
        let mut rdram = Rdram::new(0);
        progressive.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        progressive.arm_vi(Cycles::new(1_000)).unwrap();

        progressive
            .advance_to(Cycles::new(999), &mut rdram)
            .unwrap();
        assert_eq!(progressive.vi_field(), 0);
        assert_eq!(progressive.vi_current() & 1, 0);
        progressive
            .advance_to(Cycles::new(1_000), &mut rdram)
            .unwrap();
        assert_eq!(progressive.vi_field(), 0);
        assert_eq!(progressive.vi_current(), 0);

        let mut interlaced = fabric();
        interlaced.write_mmio(VI_STATUS_REG, 1 << 6).unwrap();
        interlaced.write_mmio(VI_V_SYNC_REG, 525).unwrap();
        interlaced.arm_vi(Cycles::new(1_000)).unwrap();

        interlaced.advance_to(Cycles::new(999), &mut rdram).unwrap();
        assert_eq!(interlaced.vi_field(), 0);
        assert_eq!(interlaced.vi_current() & 1, 0);
        interlaced
            .advance_to(Cycles::new(1_000), &mut rdram)
            .unwrap();
        assert_eq!(interlaced.vi_field(), 1);
        assert_eq!(interlaced.vi_current(), 1);
        interlaced
            .advance_to(Cycles::new(1_999), &mut rdram)
            .unwrap();
        assert_eq!(interlaced.vi_field(), 1);
        assert_eq!(interlaced.vi_current() & 1, 1);
        interlaced
            .advance_to(Cycles::new(2_000), &mut rdram)
            .unwrap();
        assert_eq!(interlaced.vi_field(), 0);
        assert_eq!(interlaced.vi_current(), 0);
    }

    #[test]
    fn vi_raw_register_file_masks_documented_fields_and_reschedules_interrupt() {
        let mut fabric = fabric();
        fabric.write_mmio(VI_STATUS_REG, 0xFFFF_FFFF).unwrap();
        fabric.write_mmio(VI_ORIGIN_REG, 0xFFFF_FFFF).unwrap();
        fabric.write_mmio(VI_V_SYNC_REG, 0xFFFF_FFFF).unwrap();
        fabric.write_mmio(VI_INTR_REG, 0xFFFF_FFFF).unwrap();
        assert_eq!(fabric.read_mmio(VI_STATUS_REG).unwrap(), 0x1FFFF);
        assert_eq!(fabric.read_mmio(VI_ORIGIN_REG).unwrap(), 0x00FF_FFFF);
        assert_eq!(fabric.read_mmio(VI_V_SYNC_REG).unwrap(), 0x3FF);
        assert_eq!(fabric.read_mmio(VI_INTR_REG).unwrap(), 0x3FF);

        fabric.arm_vi(Cycles::new(1_000)).unwrap();
        let old_deadline = fabric.next_deadline().unwrap();
        fabric.write_mmio(VI_INTR_REG, 1).unwrap();
        let new_deadline = fabric.next_deadline().unwrap();
        assert!(new_deadline < old_deadline);
        assert_eq!(new_deadline, Cycles::new(1));
    }
}
