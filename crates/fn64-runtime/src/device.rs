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

use crate::mmio::{AI_STATUS_BUSY, AI_STATUS_ENABLED, AI_STATUS_FULL};
use crate::rdram::RdramAddr;
use crate::rom::{DmaCompletion, DmaMemory, PiDma, PiDmaError, RomStorage};
use crate::rsp::{RspMemAddr, RspMemory, RspMemoryBank, RspMemoryError, RSP_MEMORY_BANK_SIZE};
use crate::trace::DmaDirection;
use crate::tv::{TvType, CPU_CLOCK_HZ};

pub const PI_STATUS_DMA_BUSY: u32 = 1;
pub const PI_STATUS_IO_BUSY: u32 = 1 << 1;
pub const PI_STATUS_ERROR: u32 = 1 << 2;

pub const DPC_STATUS_XBUS_DMEM_DMA: u32 = 1;
pub const DPC_STATUS_FREEZE: u32 = 1 << 1;
pub const DPC_STATUS_FLUSH: u32 = 1 << 2;
pub const DPC_STATUS_CMD_BUSY: u32 = 1 << 6;
pub const DPC_STATUS_DMA_BUSY: u32 = 1 << 8;
pub const DPC_STATUS_END_VALID: u32 = 1 << 9;
pub const DPC_STATUS_START_VALID: u32 = 1 << 10;

const AI_DRAM_ADDR_MASK: u32 = 0x00ff_fff8;
const AI_LEN_MASK: u32 = 0x0003_fff8;
const AI_DRAM_DOMAIN_END: u32 = 0x0100_0000;
const AI_DACRATE_MASK: u32 = 0x0000_3fff;
const AI_BITRATE_MASK: u32 = 0x0000_000f;
const DPC_ADDR_MASK: u32 = 0x00ff_fff8;

const MI_INTR_REG: MmioAddr = MmioAddr::new(0xA430_0008);
const MI_INTR_MASK_REG: MmioAddr = MmioAddr::new(0xA430_000C);
const DPC_START_REG: MmioAddr = MmioAddr::new(0xA410_0000);
const DPC_END_REG: MmioAddr = MmioAddr::new(0xA410_0004);
const DPC_CURRENT_REG: MmioAddr = MmioAddr::new(0xA410_0008);
const DPC_STATUS_REG: MmioAddr = MmioAddr::new(0xA410_000C);
const VI_STATUS_REG: MmioAddr = MmioAddr::new(0xA440_0000);
const VI_ORIGIN_REG: MmioAddr = MmioAddr::new(0xA440_0004);
const VI_INTR_REG: MmioAddr = MmioAddr::new(0xA440_000C);
const VI_CURRENT_REG: MmioAddr = MmioAddr::new(0xA440_0010);
const VI_V_SYNC_REG: MmioAddr = MmioAddr::new(0xA440_0018);
const VI_H_SYNC_REG: MmioAddr = MmioAddr::new(0xA440_001C);
const VI_Y_SCALE_REG: MmioAddr = MmioAddr::new(0xA440_0034);
const AI_DRAM_ADDR_REG: MmioAddr = MmioAddr::new(0xA450_0000);
const AI_LEN_REG: MmioAddr = MmioAddr::new(0xA450_0004);
const AI_CONTROL_REG: MmioAddr = MmioAddr::new(0xA450_0008);
const AI_STATUS_REG: MmioAddr = MmioAddr::new(0xA450_000C);
const AI_DACRATE_REG: MmioAddr = MmioAddr::new(0xA450_0010);
const AI_BITRATE_REG: MmioAddr = MmioAddr::new(0xA450_0014);
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

/// Physical command source selected when a DPC END write is accepted.
/// XBUS reads the RSP's 4 KiB DMEM bank; ordinary submissions read the
/// 24-bit RDRAM physical domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcSubmissionSource {
    Rdram,
    Dmem,
}

/// One renderer transaction owned by the device fabric until explicitly
/// committed or cancelled. The token prevents a stale renderer result from
/// advancing the CURRENT register of a later submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcSubmission {
    pub token: u64,
    pub source: DpcSubmissionSource,
    pub start: u32,
    pub end: u32,
}

/// Host work made necessary by a successfully latched MMIO write.
///
/// The device mutation has already happened when this value is returned. A
/// production caller must perform the named host action before allowing the
/// guest to retire another instruction. In particular, a DPC request remains
/// pending until its exact token is committed or cancelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use = "MMIO write effects must be handled before guest execution resumes"]
pub enum DeviceMmioWriteEffect {
    None,
    AiFrequencyChanged { sample_rate_hz: u32 },
    AiDmaStarted(AiDmaRequest),
    DpcSubmissionRequested(DpcSubmission),
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

/// Architecturally observable completions produced by one RSP task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RcpTaskCompletionPlan {
    SpOnly,
    SpThenDpFullSync,
}

impl RcpTaskCompletionPlan {
    const fn reaches_dp_full_sync(self) -> bool {
        matches!(self, Self::SpThenDpFullSync)
    }
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

    /// Canonical policy bytes for deterministic fixed-cycle evidence.
    /// Implementors must bind every field that can change a future
    /// completion deadline; the release encoder adds its own length and
    /// domain separation.
    fn evidence_bytes(&self) -> Vec<u8>;
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

    fn evidence_bytes(&self) -> Vec<u8> {
        let mut bytes = b"fn64.pi-timing.fixed.v1\0".to_vec();
        bytes.extend_from_slice(&self.0.get().to_be_bytes());
        bytes
    }
}

/// OS-facing work produced after a device event is fully committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceNotification {
    PiDmaComplete(DmaCompletion),
    AiDmaComplete(AiDmaRequest),
    SiDmaComplete(SiDmaRequest),
    ViRetrace { at: Cycles },
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
    UnalignedMmio {
        addr: MmioAddr,
    },
    UnmodeledMmioRead {
        addr: MmioAddr,
    },
    UnmodeledMmioWrite {
        addr: MmioAddr,
        value: u32,
    },
    PiBusy,
    AiFull,
    AiControlWhileBusy {
        current: u32,
        requested: u32,
    },
    AiDacrateWhileBusy {
        current: u32,
        requested: u32,
    },
    AiBitrateWhileBusy {
        current: u32,
        requested: u32,
    },
    AiSampleRateMismatch {
        request: u32,
        register: u32,
    },
    InvalidAiDramAddress {
        address: u32,
    },
    InvalidAiDmaLength {
        len: u32,
    },
    AiDmaRangeOverflow {
        address: u32,
        len: u32,
    },
    ZeroLengthAiDma,
    ZeroAiSampleRate,
    AiClockUnconfigured,
    ZeroViInterval,
    SiBusy,
    SpBusy,
    SpNotRunning,
    SpDmaFull,
    SpDmaMemory(RspMemoryError),
    SpDmaDramRangeOverflow {
        request: SpDmaRequest,
    },
    InvalidSpSemaphoreWrite {
        value: u32,
    },
    SpTaskNotHalted,
    InvalidSpTaskBootSize {
        size: u32,
    },
    DpBusy,
    InvalidDpcRange {
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
    },
    NoPendingDpcSubmission,
    StaleDpcSubmission {
        pending_token: u64,
        received_token: u64,
    },
    ZeroLengthPiDma,
    PiLengthOverflow {
        encoded: u32,
    },
    PiTransfer(PiDmaError),
    DeadlineOverflow,
    TimeWentBack {
        now: Cycles,
        requested: Cycles,
    },
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
            Self::AiControlWhileBusy { current, requested } => write!(
                f,
                "AI_CONTROL transition {current:#x}->{requested:#x} while the AI FIFO is active has no admitted hardware behavior"
            ),
            Self::AiDacrateWhileBusy { current, requested } => write!(
                f,
                "AI_DACRATE transition {current:#x}->{requested:#x} while the AI FIFO is active has no admitted hardware behavior"
            ),
            Self::AiBitrateWhileBusy { current, requested } => write!(
                f,
                "AI_BITRATE transition {current:#x}->{requested:#x} while the AI FIFO is active has no admitted hardware behavior"
            ),
            Self::AiSampleRateMismatch { request, register } => write!(
                f,
                "AI DMA sample-rate metadata {request} Hz does not match the public DAC rate {register} Hz"
            ),
            Self::InvalidAiDramAddress { address } => write!(
                f,
                "AI DMA DRAM address must fit the aligned public 24-bit field, got {address:#010X}"
            ),
            Self::InvalidAiDmaLength { len } => write!(
                f,
                "AI DMA length must fit the public 18-bit field with its low three bits clear, got {len:#010X}"
            ),
            Self::AiDmaRangeOverflow { address, len } => write!(
                f,
                "AI DMA range [{address:#010X}, +{len:#010X}) exceeds the 24-bit physical domain"
            ),
            Self::ZeroLengthAiDma => write!(f, "AI DMA length must be nonzero"),
            Self::ZeroAiSampleRate => write!(f, "AI DMA sample rate must be nonzero"),
            Self::AiClockUnconfigured => write!(
                f,
                "AI DAC rate requires an IPL-selected television clock before guest execution"
            ),
            Self::ZeroViInterval => write!(f, "VI field interval must be nonzero"),
            Self::SiBusy => write!(f, "SI DMA start while the SI channel is busy"),
            Self::SpBusy => write!(f, "RSP task start while SP is busy"),
            Self::SpNotRunning => write!(f, "RSP task completion without an in-flight task"),
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
            Self::InvalidDpcRange { source, start, end } => write!(
                f,
                "invalid {source:?} DPC command range [{start:#010X}, {end:#010X})"
            ),
            Self::NoPendingDpcSubmission => {
                write!(f, "DPC transaction completion without a pending submission")
            }
            Self::StaleDpcSubmission {
                pending_token,
                received_token,
            } => write!(
                f,
                "DPC transaction token {received_token} does not own pending token {pending_token}"
            ),
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
struct DpcRegisters {
    start: u32,
    end: u32,
    current: u32,
    status: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingDpc {
    submission: DpcSubmission,
    rollback: DpcRegisters,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduledDeviceEventKind {
    Pi,
    Ai,
    Si,
    SpDma,
    Vi,
    Sp,
    Dp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduledDeviceEventSnapshot {
    pub at: Cycles,
    pub sequence: u64,
    pub token: u64,
    pub kind: ScheduledDeviceEventKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingPiSnapshot {
    pub token: u64,
    pub request: PiDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingAiSnapshot {
    pub token: u64,
    pub request: AiDmaRequest,
    pub started_at: Cycles,
    pub deadline: Cycles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingDpcSnapshot {
    pub submission: DpcSubmission,
    pub rollback_start: u32,
    pub rollback_end: u32,
    pub rollback_current: u32,
    pub rollback_status: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSiSnapshot {
    pub token: u64,
    pub request: SiDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingSpDmaSnapshot {
    pub token: u64,
    pub request: SpDmaRequest,
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
    pub ai_dram_addr: RdramAddr,
    pub ai_control: u32,
    pub ai_dacrate: u32,
    pub ai_bitrate: u32,
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
    pub dpc_start: u32,
    pub dpc_end: u32,
    pub dpc_current: u32,
    pub dpc_status: u32,
    pub pending_dpc: Option<DpcSubmission>,
    pub mi_pending: u32,
    pub mi_mask: u32,
    pub pi_domain1: PiDomainTiming,
    pub pi_domain2: PiDomainTiming,
}

/// Release-only, future-state-complete view of the modeled device fabric.
///
/// [`DeviceSnapshot`] intentionally remains the compact guest-register view
/// used by ordinary runtime code. This evidence view additionally binds all
/// modeled memory, queues, policy, and scheduled ordering that can make the
/// same later input produce a different device result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEvidenceSnapshot {
    pub guest: DeviceSnapshot,
    pub pi_timing_policy: Vec<u8>,
    pub pending_pi: Option<PendingPiSnapshot>,
    pub current_ai: Option<PendingAiSnapshot>,
    pub queued_ai: Option<AiDmaRequest>,
    pub pending_dpc: Option<PendingDpcSnapshot>,
    pub pending_si: Option<PendingSiSnapshot>,
    pub si_dma_error: bool,
    pub si_latency: Cycles,
    pub pif_ram: [u8; 64],
    pub rsp_dmem: [u8; RSP_MEMORY_BANK_SIZE],
    pub rsp_imem: [u8; RSP_MEMORY_BANK_SIZE],
    pub sp_rd_len: u32,
    pub sp_wr_len: u32,
    pub sp_pc: u32,
    pub sp_semaphore: bool,
    pub active_sp_dma: Option<PendingSpDmaSnapshot>,
    pub queued_sp_dma: Option<SpDmaRequest>,
    pub sp_dma_setup_cycles: Cycles,
    pub vi_registers: [u32; 14],
    pub vi_epoch: Cycles,
    pub pending_vi_token: Option<u64>,
    pub pending_sp_token: Option<u64>,
    pub pending_dp_token: Option<u64>,
    pub scheduled_events: Vec<ScheduledDeviceEventSnapshot>,
    pub next_event_sequence: u64,
    pub save_bytes: Option<Vec<u8>>,
    pub pending_eeprom_write: Option<crate::rom::PendingEepromWriteSnapshot>,
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
    ai_dram_addr: RdramAddr,
    ai_control: u32,
    ai_dacrate: u32,
    ai_bitrate: u32,
    current_ai: Option<PendingAi>,
    queued_ai: Option<AiDmaRequest>,
    dpc: DpcRegisters,
    pending_dpc: Option<PendingDpc>,
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
            ai_dram_addr: RdramAddr::from_offset(0),
            ai_control: 0,
            ai_dacrate: 0,
            ai_bitrate: 0,
            current_ai: None,
            queued_ai: None,
            dpc: DpcRegisters {
                start: 0,
                end: 0,
                current: 0,
                status: 0,
            },
            pending_dpc: None,
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

    /// Immutable access to the PI storage engine's typed observation history.
    /// Mutating save protocols continue to use [`Self::pi_dma_mut`].
    pub fn pi_dma(&self) -> &PiDma<R> {
        &self.pi_dma
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
            ai_dram_addr: self.ai_dram_addr,
            ai_control: self.ai_control,
            ai_dacrate: self.ai_dacrate,
            ai_bitrate: self.ai_bitrate,
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
            dp_busy: self.pending_dp.is_some() || self.pending_dpc.is_some(),
            dpc_start: self.dpc.start,
            dpc_end: self.dpc.end,
            dpc_current: self.dpc.current,
            dpc_status: self.dpc.status,
            pending_dpc: self.pending_dpc.map(|pending| pending.submission),
            mi_pending: self.mi_pending,
            mi_mask: self.mi_mask,
            pi_domain1: self.pi_domain1,
            pi_domain2: self.pi_domain2,
        }
    }

    pub fn evidence_snapshot(&mut self) -> DeviceEvidenceSnapshot {
        let pi_timing_policy = self.pi_timing.evidence_bytes();
        assert!(
            !pi_timing_policy.is_empty(),
            "PiTimingModel::evidence_bytes must identify every future-affecting timing policy"
        );
        let scheduled_events = self
            .events
            .iter()
            .map(|(&(at, sequence), event)| {
                let (token, kind) = match *event {
                    DeviceEvent::Pi { token } => (token, ScheduledDeviceEventKind::Pi),
                    DeviceEvent::Ai { token } => (token, ScheduledDeviceEventKind::Ai),
                    DeviceEvent::Si { token } => (token, ScheduledDeviceEventKind::Si),
                    DeviceEvent::SpDma { token } => (token, ScheduledDeviceEventKind::SpDma),
                    DeviceEvent::Vi { token } => (token, ScheduledDeviceEventKind::Vi),
                    DeviceEvent::Sp { token } => (token, ScheduledDeviceEventKind::Sp),
                    DeviceEvent::Dp { token } => (token, ScheduledDeviceEventKind::Dp),
                };
                ScheduledDeviceEventSnapshot {
                    at,
                    sequence,
                    token,
                    kind,
                }
            })
            .collect();
        let pending_eeprom_write = self.pi_dma.pending_eeprom_write_snapshot();
        let save_bytes = self.pi_dma.save_snapshot_bytes();
        DeviceEvidenceSnapshot {
            guest: self.snapshot(),
            pi_timing_policy,
            pending_pi: self.pending_pi.map(|pending| PendingPiSnapshot {
                token: pending.token,
                request: pending.request,
            }),
            current_ai: self.current_ai.map(|pending| PendingAiSnapshot {
                token: pending.token,
                request: pending.request,
                started_at: pending.started_at,
                deadline: pending.deadline,
            }),
            queued_ai: self.queued_ai,
            pending_dpc: self.pending_dpc.map(|pending| PendingDpcSnapshot {
                submission: pending.submission,
                rollback_start: pending.rollback.start,
                rollback_end: pending.rollback.end,
                rollback_current: pending.rollback.current,
                rollback_status: pending.rollback.status,
            }),
            pending_si: self.pending_si.map(|pending| PendingSiSnapshot {
                token: pending.token,
                request: pending.request,
            }),
            si_dma_error: self.si_dma_error,
            si_latency: self.si_latency,
            pif_ram: self.pif_ram,
            rsp_dmem: *self.rsp_memory.bank(RspMemoryBank::Dmem),
            rsp_imem: *self.rsp_memory.bank(RspMemoryBank::Imem),
            sp_rd_len: self.sp_rd_len,
            sp_wr_len: self.sp_wr_len,
            sp_pc: self.sp_pc,
            sp_semaphore: self.sp_semaphore,
            active_sp_dma: self.active_sp_dma.map(|pending| PendingSpDmaSnapshot {
                token: pending.token,
                request: pending.request,
            }),
            queued_sp_dma: self.queued_sp_dma,
            sp_dma_setup_cycles: self.sp_dma_setup_cycles,
            vi_registers: self.vi_registers,
            vi_epoch: self.vi_epoch,
            pending_vi_token: self.pending_vi,
            pending_sp_token: self.pending_sp,
            pending_dp_token: self.pending_dp,
            scheduled_events,
            next_event_sequence: self.next_event_sequence,
            save_bytes,
            pending_eeprom_write,
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

    /// Direct CPU word load from the 64-byte PIF RAM window
    /// (`0x1FC007C0..0x1FC00800`). Real hardware exposes PIF RAM to uncached
    /// CPU loads as well as SI DMA; AKI-era hand-rolled joybus code and
    /// boot-handshake polls read it directly (e.g. the terminate-boot status
    /// word at 0x1FC007FC).
    pub fn pif_ram_cpu_read_w(&self, offset: usize) -> u32 {
        let offset = offset & !3;
        u32::from_be_bytes(self.pif_ram[offset..offset + 4].try_into().unwrap())
    }

    /// Direct CPU word store into PIF RAM. Bytes only -- the PIF command
    /// interpreter is injected by the ABI layer and runs on the `DramToPif`
    /// DMA completion path, which is how joybus command buffers arrive.
    /// ponytail: a CPU store to the final command byte does not run the
    /// interpreter yet; wire the injected executor through here if a title's
    /// hand-rolled code ever issues commands by direct store.
    pub fn pif_ram_cpu_write_w(&mut self, offset: usize, value: u32) {
        let offset = offset & !3;
        self.pif_ram[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    /// Stage one complete Controller Manager command image in the physical
    /// PIF RAM owned by this fabric. The caller must first acquire the SI
    /// engine with a typed controller request; otherwise a failed overlap
    /// could overwrite the command belonging to the live transfer.
    pub fn stage_controller_pif_command(&mut self, command: [u8; 64]) {
        assert!(
            matches!(
                self.pending_si_request(),
                Some(SiDmaRequest {
                    kind: SiDmaKind::ControllerQuery | SiDmaKind::ControllerRead,
                    ..
                })
            ),
            "controller PIF command staged without an accepted Controller Manager SI request"
        );
        self.pif_ram = command;
    }

    /// Exact physical PIF RAM image. Controller Manager getters decode only
    /// this completed device-owned transaction, never a second live sample.
    pub const fn pif_ram(&self) -> &[u8; 64] {
        &self.pif_ram
    }

    pub const fn ai_status(&self) -> u32 {
        let mut status = 0;
        if self.ai_control & 1 != 0 {
            status |= AI_STATUS_ENABLED;
        }
        if self.current_ai.is_some() {
            status |= AI_STATUS_BUSY;
        }
        if self.queued_ai.is_some() {
            status |= AI_STATUS_FULL;
        }
        status
    }

    pub const fn ai_dram_addr(&self) -> RdramAddr {
        self.ai_dram_addr
    }

    pub const fn ai_control(&self) -> u32 {
        self.ai_control
    }

    pub const fn ai_dacrate(&self) -> u32 {
        self.ai_dacrate
    }

    pub const fn ai_bitrate(&self) -> u32 {
        self.ai_bitrate
    }

    /// True sample rate selected by the latched DAC period and the IPL-owned
    /// television clock. Production may not guess NTSC when boot has not
    /// established that clock authority.
    pub fn ai_sample_rate_hz(&self) -> Result<u32, DeviceFault> {
        let tv_type = self.tv_type.ok_or(DeviceFault::AiClockUnconfigured)?;
        Ok(tv_type.vi_clock_hz() / (self.ai_dacrate + 1))
    }

    /// Guest-visible bytes remaining in the active DMA. The device fabric is
    /// advanced at every translated checkpoint, so this interpolation is a
    /// deterministic function of guest time and never host callback jitter.
    pub fn ai_length(&self) -> u32 {
        let Some(current) = self.current_ai else {
            return 0;
        };
        if self.ai_control & 1 == 0 {
            return current.request.len;
        }
        let duration = current.deadline.get() - current.started_at.get();
        let remaining_cycles = current.deadline.get().saturating_sub(self.now.get());
        let remaining = (u128::from(current.request.len) * u128::from(remaining_cycles))
            .div_ceil(u128::from(duration));
        let remaining = remaining.div_ceil(8) * 8;
        u32::try_from(remaining).expect("AI remaining length exceeds u32")
    }

    pub const fn pending_dpc_submission(&self) -> Option<DpcSubmission> {
        match self.pending_dpc {
            Some(pending) => Some(pending.submission),
            None => None,
        }
    }

    fn validate_dpc_range(
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
    ) -> Result<(), DeviceFault> {
        let upper_bound = match source {
            DpcSubmissionSource::Rdram => 0x0100_0000,
            DpcSubmissionSource::Dmem => RSP_MEMORY_BANK_SIZE as u32,
        };
        if !start.is_multiple_of(8) || !end.is_multiple_of(8) || start >= end || end > upper_bound {
            return Err(DeviceFault::InvalidDpcRange { source, start, end });
        }
        Ok(())
    }

    fn begin_dpc_submission(
        &mut self,
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
        rollback: DpcRegisters,
    ) -> Result<DpcSubmission, DeviceFault> {
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        Self::validate_dpc_range(source, start, end)?;
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let submission = DpcSubmission {
            token,
            source,
            start,
            end,
        };
        self.dpc.status &= !DPC_STATUS_START_VALID;
        self.dpc.status |= DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY;
        self.pending_dpc = Some(PendingDpc {
            submission,
            rollback,
        });
        Ok(submission)
    }

    /// Begin one renderer transaction through the same state used by raw
    /// START/END MMIO. The range is not architecturally consumed until the
    /// renderer returns and the caller commits this exact token.
    pub fn request_dpc_submission(
        &mut self,
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
    ) -> Result<DpcSubmission, DeviceFault> {
        if self.pending_dpc.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        Self::validate_dpc_range(source, start, end)?;
        let rollback = self.dpc;
        self.dpc.start = start;
        self.dpc.end = end;
        self.dpc.current = start;
        match source {
            DpcSubmissionSource::Rdram => self.dpc.status &= !DPC_STATUS_XBUS_DMEM_DMA,
            DpcSubmissionSource::Dmem => self.dpc.status |= DPC_STATUS_XBUS_DMEM_DMA,
        }
        self.begin_dpc_submission(source, start, end, rollback)
    }

    /// Commit renderer acceptance. CURRENT advances only here, after the
    /// selected backend has consumed the submitted bytes.
    pub fn commit_dpc_submission(&mut self, token: u64) -> Result<(), DeviceFault> {
        let pending = self
            .pending_dpc
            .ok_or(DeviceFault::NoPendingDpcSubmission)?;
        if pending.submission.token != token {
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: pending.submission.token,
                received_token: token,
            });
        }
        self.dpc.current = pending.submission.end;
        self.dpc.status &= !(DPC_STATUS_END_VALID | DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY);
        self.pending_dpc = None;
        Ok(())
    }

    /// Roll back every register mutation made while accepting a renderer
    /// transaction. This closes the interleaving where a backend rejection
    /// could otherwise consume START_VALID or advance a range that never ran.
    pub fn cancel_dpc_submission(&mut self, token: u64) -> Result<(), DeviceFault> {
        let pending = self
            .pending_dpc
            .ok_or(DeviceFault::NoPendingDpcSubmission)?;
        if pending.submission.token != token {
            return Err(DeviceFault::StaleDpcSubmission {
                pending_token: pending.submission.token,
                received_token: token,
            });
        }
        self.dpc = pending.rollback;
        self.pending_dpc = None;
        Ok(())
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

    /// The framebuffer line width in pixels, latched from `OSViMode.common.width`
    /// into VI_WIDTH (`vi_registers[2]`, a 12-bit field). `None` before the
    /// first `osViSetMode` (no mode latched), so a presenter can fall back to a
    /// default rather than a bogus zero-stride. This is the origin's line
    /// stride the CPU/RSP write into — the correct stride for reading the
    /// framebuffer, as distinct from the displayed x-scale.
    pub fn vi_width(&self) -> Option<u32> {
        let width = self.vi_registers[2] & 0x0fff;
        (width != 0).then_some(width)
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

    /// Exact pending VI interrupt deadline. Hosts use this rather than adding
    /// a cached interval to an older host tick: instruction checkpoints may
    /// advance the shared clock between quiescent field pumps, and VI timing
    /// register writes may reschedule the next interrupt.
    pub fn next_vi_deadline(&self) -> Option<Cycles> {
        let pending = self.pending_vi?;
        self.events
            .iter()
            .find_map(|(&(at, _), event)| match event {
                DeviceEvent::Vi { token } if *token == pending => Some(at),
                _ => None,
            })
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
    /// Timing uses the exact public `VI_CLOCK / (DACRATE + 1)` rational and
    /// four bytes per stereo 16-bit frame; the one final ceiling prevents a
    /// nonempty buffer from completing early without feeding the truncated
    /// integer ABI playback rate back into the device clock.
    pub fn start_ai_dma(&mut self, request: AiDmaRequest) -> Result<(), DeviceFault> {
        let address = request.dram_addr.offset();
        if address & !AI_DRAM_ADDR_MASK != 0 {
            return Err(DeviceFault::InvalidAiDramAddress { address });
        }
        if request.len == 0 {
            return Err(DeviceFault::ZeroLengthAiDma);
        }
        if request.len & !AI_LEN_MASK != 0 {
            return Err(DeviceFault::InvalidAiDmaLength { len: request.len });
        }
        if address
            .checked_add(request.len)
            .is_none_or(|end| end > AI_DRAM_DOMAIN_END)
        {
            return Err(DeviceFault::AiDmaRangeOverflow {
                address,
                len: request.len,
            });
        }
        if request.sample_rate_hz == 0 {
            return Err(DeviceFault::ZeroAiSampleRate);
        }
        let register_rate = self.ai_sample_rate_hz()?;
        if request.sample_rate_hz != register_rate {
            return Err(DeviceFault::AiSampleRateMismatch {
                request: request.sample_rate_hz,
                register: register_rate,
            });
        }
        if self.current_ai.is_some() && self.queued_ai.is_some() {
            return Err(DeviceFault::AiFull);
        }
        if let Some(current) = self.current_ai {
            if current.deadline != current.started_at {
                self.prepare_ai_dma(request, current.deadline)?;
            }
            self.ai_dram_addr = request.dram_addr;
            self.queued_ai = Some(request);
        } else {
            if self.ai_control & 1 != 0 {
                let prepared = self.prepare_ai_dma(request, self.now)?;
                self.ai_dram_addr = request.dram_addr;
                self.commit_ai_dma(prepared);
            } else {
                // AI_LEN fills the FIFO even while CONTROL disables the DAC.
                // The zero-duration marker owns the current FIFO slot without
                // scheduling a completion; the 0->1 CONTROL transition below
                // replaces it with a timed transfer at that exact guest cycle.
                self.current_ai = Some(PendingAi {
                    token: self.next_event_sequence,
                    request,
                    started_at: self.now,
                    deadline: self.now,
                });
                self.ai_dram_addr = request.dram_addr;
            }
        }
        Ok(())
    }

    fn prepare_ai_dma(
        &self,
        request: AiDmaRequest,
        started_at: Cycles,
    ) -> Result<PendingAi, DeviceFault> {
        const BYTES_PER_STEREO_FRAME: u128 = 4;
        let tv_type = self.tv_type.ok_or(DeviceFault::AiClockUnconfigured)?;
        let frames = u128::from(request.len) / BYTES_PER_STEREO_FRAME;
        let duration = (frames * u128::from(CPU_CLOCK_HZ) * u128::from(self.ai_dacrate + 1))
            .div_ceil(u128::from(tv_type.vi_clock_hz()));
        let duration = u64::try_from(duration.max(1)).map_err(|_| DeviceFault::DeadlineOverflow)?;
        let deadline = started_at
            .checked_add(Cycles::new(duration))
            .ok_or(DeviceFault::DeadlineOverflow)?;
        let token = self.next_event_sequence;
        self.next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        Ok(PendingAi {
            token,
            request,
            started_at,
            deadline,
        })
    }

    fn commit_ai_dma(&mut self, pending: PendingAi) {
        self.next_event_sequence = pending
            .token
            .checked_add(1)
            .expect("AI admission preflight proved the event sequence increment");
        self.current_ai = Some(pending);
        self.events.insert(
            (pending.deadline, pending.token),
            DeviceEvent::Ai {
                token: pending.token,
            },
        );
        self.record(DeviceTraceKind::AiDmaStarted(pending.request));
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
        let boot_size = header
            .ucode_boot_size
            .checked_add(7)
            .map(|size| size & !7)
            .filter(|size| *size != 0 && *size as usize <= RSP_MEMORY_BANK_SIZE)
            .ok_or(DeviceFault::InvalidSpTaskBootSize {
                size: header.ucode_boot_size,
            })? as usize;
        // OSTask pointers may be physical or direct-mapped KSEG0/KSEG1.
        // Both reduce to the public 29-bit physical bus address this way.
        let boot_addr = (header.ucode_boot & 0x1fff_ffff) & !7;
        let boot = rdram.dma_read_bytes_flat(boot_addr as usize, boot_size);
        self.admit_sp_task_with_boot_image(rdram, task_addr, header, &boot)
    }

    /// Variant of [`Self::admit_sp_task`] for a host whose CPU cache and
    /// physical DRAM share one backing allocation. `boot` is the CPU-visible
    /// rspboot text selected by the OS loader, while `rdram` remains the
    /// physical image used for the task header and all device-visible data.
    pub fn admit_sp_task_with_boot_image<M: DmaMemory + ?Sized>(
        &mut self,
        rdram: &M,
        task_addr: RdramAddr,
        header: crate::rsp::OsTaskHeader,
        boot: &[u8],
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
        assert_eq!(
            boot.len(),
            boot_size,
            "osSpTaskLoad cached rspboot image has {} bytes; aligned task size requires {boot_size}",
            boot.len()
        );
        let task_bytes = rdram.dma_read_bytes_flat(task_addr.offset() as usize, 64);
        self.rsp_memory
            .write_bytes(RspMemAddr::from_register(0x0fc0), &task_bytes)
            .map_err(DeviceFault::SpDmaMemory)?;

        let boot_addr = (header.ucode_boot & 0x1fff_ffff) & !7;
        self.rsp_memory
            .write_bytes(RspMemAddr::from_register(0x1000), boot)
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
    pub fn start_rcp_task(&mut self, plan: RcpTaskCompletionPlan) -> Result<(), DeviceFault> {
        self.start_rcp_task_with_latency(plan, Cycles::new(1))
    }

    /// Schedule completion after a measured amount of synchronous RSP work.
    /// The caller has already executed that work while the guest is suspended;
    /// this delay controls only when its architectural interrupt is observable.
    pub fn start_rcp_task_with_latency(
        &mut self,
        plan: RcpTaskCompletionPlan,
        sp_latency: Cycles,
    ) -> Result<(), DeviceFault> {
        if self.pending_sp.is_some() {
            return Err(DeviceFault::SpBusy);
        }
        if plan.reaches_dp_full_sync() && self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        self.begin_rcp_task()?;
        self.finish_rcp_task(plan, sp_latency)
    }

    /// Mark an asynchronously chunked RSP task as running without fabricating
    /// a completion deadline. The retained token becomes schedulable exactly
    /// once through [`Self::finish_rcp_task`].
    pub fn begin_rcp_task(&mut self) -> Result<(), DeviceFault> {
        if self.pending_sp.is_some() {
            return Err(DeviceFault::SpBusy);
        }
        let sp_token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.pending_sp = Some(sp_token);
        self.sp_status &= !(SP_STATUS_HALT | SP_STATUS_BROKE);
        Ok(())
    }

    /// Schedule the sole completion of work previously admitted by
    /// [`Self::begin_rcp_task`].
    pub fn finish_rcp_task(
        &mut self,
        plan: RcpTaskCompletionPlan,
        sp_latency: Cycles,
    ) -> Result<(), DeviceFault> {
        let needs_dp = plan.reaches_dp_full_sync();
        let sp_token = self.pending_sp.ok_or(DeviceFault::SpNotRunning)?;
        if self
            .events
            .values()
            .any(|event| matches!(event, DeviceEvent::Sp { token } if *token == sp_token))
        {
            return Err(DeviceFault::SpBusy);
        }
        if needs_dp && self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        let sp_deadline = self
            .now
            .checked_add(sp_latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
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

    /// Prove that one raw FullSync can reserve the sole DP completion slot.
    /// This is nonmutating so a renderer can be rejected before it observes
    /// or changes guest memory.
    pub fn preflight_dp_full_sync(&self, latency: Cycles) -> Result<(), DeviceFault> {
        assert!(latency.get() > 0, "DP FullSync latency must be nonzero");
        // Interleaving closed here: CPU thread A may submit a second raw DPC
        // FullSync before thread B services the first DP event. The synchronous
        // renderer path calls this before backend entry, and the single-owner
        // dispatcher cannot advance devices until it either commits or
        // unwinds, so the checked slot/deadline/token remain available.
        if self.pending_dp.is_some() {
            return Err(DeviceFault::DpBusy);
        }
        self.now
            .checked_add(latency)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        self.next_event_sequence
            .checked_add(1)
            .ok_or(DeviceFault::DeadlineOverflow)?;
        Ok(())
    }

    /// Schedule the DP interrupt generated by a raw CPU/RSP DPC range that
    /// reached FullSync without starting a new SP task.
    pub fn start_dp_full_sync(&mut self, latency: Cycles) -> Result<(), DeviceFault> {
        self.preflight_dp_full_sync(latency)?;
        let deadline = self
            .now
            .checked_add(latency)
            .expect("DP FullSync deadline was preflighted");
        let token = self.next_event_sequence;
        self.next_event_sequence = self
            .next_event_sequence
            .checked_add(1)
            .expect("DP FullSync event token was preflighted");
        self.pending_dp = Some(token);
        self.events
            .insert((deadline, token), DeviceEvent::Dp { token });
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
            DPC_START_REG => Ok(self.dpc.start),
            DPC_END_REG => Ok(self.dpc.end),
            DPC_CURRENT_REG => Ok(self.dpc.current),
            DPC_STATUS_REG => Ok(self.dpc.status),
            MI_INTR_REG => Ok(self.mi_pending),
            MI_INTR_MASK_REG => Ok(self.mi_mask),
            VI_CURRENT_REG => Ok(self.vi_current()),
            addr if (VI_STATUS_REG.get()..=VI_Y_SCALE_REG.get()).contains(&addr.get()) => {
                let index = ((addr.get() - VI_STATUS_REG.get()) / 4) as usize;
                Ok(self.vi_registers[index])
            }
            AI_DRAM_ADDR_REG => Ok(self.ai_dram_addr.offset()),
            AI_LEN_REG => Ok(self.ai_length()),
            AI_CONTROL_REG => Ok(self.ai_control),
            AI_STATUS_REG => Ok(self.ai_status()),
            AI_DACRATE_REG => Ok(self.ai_dacrate),
            AI_BITRATE_REG => Ok(self.ai_bitrate),
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

    pub fn write_mmio(
        &mut self,
        addr: MmioAddr,
        value: u32,
    ) -> Result<DeviceMmioWriteEffect, DeviceFault> {
        self.validate_mmio(addr)?;
        match addr {
            AI_DRAM_ADDR_REG => {
                self.ai_dram_addr = RdramAddr::from_offset(value & AI_DRAM_ADDR_MASK);
                return Ok(DeviceMmioWriteEffect::None);
            }
            AI_LEN_REG => {
                let request = AiDmaRequest {
                    dram_addr: self.ai_dram_addr,
                    len: value & AI_LEN_MASK,
                    sample_rate_hz: self.ai_sample_rate_hz()?,
                };
                self.start_ai_dma(request)?;
                return Ok(DeviceMmioWriteEffect::AiDmaStarted(request));
            }
            AI_CONTROL_REG => {
                let requested = value & 1;
                if self.ai_control == 1
                    && requested == 0
                    && (self.current_ai.is_some() || self.queued_ai.is_some())
                {
                    return Err(DeviceFault::AiControlWhileBusy {
                        current: self.ai_control,
                        requested,
                    });
                }
                let prepared = if requested == 1 {
                    self.current_ai
                        .filter(|pending| pending.deadline == pending.started_at)
                        .map(|dormant| self.prepare_ai_dma(dormant.request, self.now))
                        .transpose()?
                } else {
                    None
                };
                self.ai_control = requested;
                if let Some(prepared) = prepared {
                    self.commit_ai_dma(prepared);
                }
                return Ok(DeviceMmioWriteEffect::None);
            }
            AI_STATUS_REG => {
                self.clear_interrupt(InterruptSource::Ai);
                return Ok(DeviceMmioWriteEffect::None);
            }
            AI_DACRATE_REG => {
                let dacrate = value & AI_DACRATE_MASK;
                if self.current_ai.is_some() || self.queued_ai.is_some() {
                    return Err(DeviceFault::AiDacrateWhileBusy {
                        current: self.ai_dacrate,
                        requested: dacrate,
                    });
                }
                let tv_type = self.tv_type.ok_or(DeviceFault::AiClockUnconfigured)?;
                self.ai_dacrate = dacrate;
                return Ok(DeviceMmioWriteEffect::AiFrequencyChanged {
                    sample_rate_hz: tv_type.vi_clock_hz() / (dacrate + 1),
                });
            }
            AI_BITRATE_REG => {
                let bitrate = value & AI_BITRATE_MASK;
                if self.current_ai.is_some() || self.queued_ai.is_some() {
                    return Err(DeviceFault::AiBitrateWhileBusy {
                        current: self.ai_bitrate,
                        requested: bitrate,
                    });
                }
                self.ai_bitrate = bitrate;
                return Ok(DeviceMmioWriteEffect::None);
            }
            DPC_START_REG => {
                if self.pending_dpc.is_some() {
                    return Err(DeviceFault::DpBusy);
                }
                if self.dpc.status & DPC_STATUS_START_VALID == 0 {
                    self.dpc.start = value & DPC_ADDR_MASK;
                    self.dpc.status |= DPC_STATUS_START_VALID;
                }
                return Ok(DeviceMmioWriteEffect::None);
            }
            DPC_END_REG => {
                if self.pending_dpc.is_some() {
                    return Err(DeviceFault::DpBusy);
                }
                let rollback = self.dpc;
                let end = value & DPC_ADDR_MASK;
                let start = if self.dpc.status & DPC_STATUS_START_VALID != 0 {
                    self.dpc.start
                } else {
                    self.dpc.current
                };
                if start == end {
                    self.dpc.end = end;
                    self.dpc.current = start;
                    self.dpc.status &= !DPC_STATUS_START_VALID;
                    return Ok(DeviceMmioWriteEffect::None);
                }
                let source = if self.dpc.status & DPC_STATUS_XBUS_DMEM_DMA != 0 {
                    DpcSubmissionSource::Dmem
                } else {
                    DpcSubmissionSource::Rdram
                };
                Self::validate_dpc_range(source, start, end)?;
                self.dpc.end = end;
                self.dpc.current = start;
                let submission = self.begin_dpc_submission(source, start, end, rollback)?;
                return Ok(DeviceMmioWriteEffect::DpcSubmissionRequested(submission));
            }
            DPC_CURRENT_REG => {
                return Err(DeviceFault::UnmodeledMmioWrite { addr, value });
            }
            DPC_STATUS_REG => {
                apply_device_clear_set_pair(
                    &mut self.dpc.status,
                    value,
                    0,
                    1,
                    DPC_STATUS_XBUS_DMEM_DMA,
                );
                apply_device_clear_set_pair(&mut self.dpc.status, value, 2, 3, DPC_STATUS_FREEZE);
                apply_device_clear_set_pair(&mut self.dpc.status, value, 4, 5, DPC_STATUS_FLUSH);
                return Ok(DeviceMmioWriteEffect::None);
            }
            _ => {}
        }
        self.write_mmio_without_effect(addr, value)?;
        Ok(DeviceMmioWriteEffect::None)
    }

    fn write_mmio_without_effect(&mut self, addr: MmioAddr, value: u32) -> Result<(), DeviceFault> {
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
            let prepared_ai_promotion = match event {
                DeviceEvent::Ai { token }
                    if self
                        .current_ai
                        .is_some_and(|current| current.token == token) =>
                {
                    self.queued_ai
                        .map(|next| self.prepare_ai_dma(next, key.0))
                        .transpose()?
                }
                _ => None,
            };
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
                    self.pi_dma.record_sram_dma_commit(self.now, completion);
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
                    let full_before_completion = self.queued_ai.is_some();
                    self.current_ai = None;
                    self.record(DeviceTraceKind::AiDmaComplete(current.request));
                    if self.queued_ai.take().is_some() {
                        self.commit_ai_dma(prepared_ai_promotion.expect(
                            "queued AI promotion was preflighted before event-state mutation",
                        ));
                    }
                    // Public rcp.h defines FIFO FULL transitioning 1 -> 0 as
                    // an AI interrupt edge. Other silicon assertion causes
                    // and the sub-cycle phase remain unclaimed.
                    if full_before_completion {
                        self.raise_interrupt(InterruptSource::Ai);
                        let notification = DeviceNotification::AiDmaComplete(current.request);
                        notifications.push(notification);
                        self.record(DeviceTraceKind::NotificationReady(notification));
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
                        SiDmaKind::PifToDram => {
                            {
                                static PROBE: std::sync::OnceLock<bool> =
                                    std::sync::OnceLock::new();
                                if *PROBE
                                    .get_or_init(|| std::env::var_os("FN64_BOOT_PROBE").is_some())
                                {
                                    eprintln!(
                                        "[boot-probe] PifToDram response: {:02x?}",
                                        self.pif_ram
                                    );
                                }
                            }
                            rdram
                                .dma_write_bytes(request.dram_addr.offset() as usize, &self.pif_ram)
                        }
                        SiDmaKind::ControllerQuery | SiDmaKind::ControllerRead => {
                            execute_pif(self.now, &mut self.pif_ram, &mut self.pi_dma);
                        }
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
                    let notification = DeviceNotification::ViRetrace { at: self.now };
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
#[allow(unused_must_use)]
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

        fn evidence_bytes(&self) -> Vec<u8> {
            let mut bytes = b"fn64.pi-timing.test.v1\0".to_vec();
            bytes.extend_from_slice(&self.0.get().to_be_bytes());
            bytes
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
    fn raw_ai_registers_derive_one_fifo_request_from_the_authoritative_tv_clock() {
        let mut fabric = fabric();
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 151),
            Err(DeviceFault::AiClockUnconfigured),
            "raw AI programming must not guess an NTSC clock before IPL configuration"
        );
        assert_eq!(fabric.ai_dacrate(), 0);

        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        let sample_rate_hz = TvType::Ntsc.vi_clock_hz() / 152;
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 151).unwrap(),
            DeviceMmioWriteEffect::AiFrequencyChanged { sample_rate_hz }
        );
        assert_eq!(
            fabric.write_mmio(AI_DRAM_ADDR_REG, 0x01ff_123f).unwrap(),
            DeviceMmioWriteEffect::None
        );
        fabric.write_mmio(AI_CONTROL_REG, u32::MAX).unwrap();
        fabric.write_mmio(AI_BITRATE_REG, 0x25).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x00ff_1238),
            len: 0x80,
            sample_rate_hz,
        };
        assert_eq!(
            fabric.write_mmio(AI_LEN_REG, 0x87).unwrap(),
            DeviceMmioWriteEffect::AiDmaStarted(request)
        );
        assert_eq!(fabric.read_mmio(AI_DRAM_ADDR_REG).unwrap(), 0x00ff_1238);
        assert_eq!(fabric.read_mmio(AI_CONTROL_REG).unwrap(), 1);
        assert_eq!(fabric.read_mmio(AI_DACRATE_REG).unwrap(), 151);
        assert_eq!(fabric.read_mmio(AI_BITRATE_REG).unwrap(), 5);
        assert_eq!(fabric.read_mmio(AI_LEN_REG).unwrap(), 0x80);
        assert_eq!(
            fabric.read_mmio(AI_STATUS_REG).unwrap(),
            AI_STATUS_ENABLED | AI_STATUS_BUSY
        );

        fabric.raise_interrupt(InterruptSource::Ai);
        fabric.write_mmio(AI_STATUS_REG, u32::MAX).unwrap();
        assert!(!fabric.interrupt_pending(InterruptSource::Ai));
        assert_eq!(fabric.pending_dpc_submission(), None);
    }

    #[test]
    fn typed_ai_requests_reject_unrepresentable_register_values_without_mutation() {
        let cases = [
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x1001),
                    len: 8,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDramAddress { address: 0x1001 },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x0100_0000),
                    len: 8,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDramAddress {
                    address: 0x0100_0000,
                },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x1000),
                    len: 1,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDmaLength { len: 1 },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x1000),
                    len: 0x0004_0000,
                    sample_rate_hz: 1,
                },
                DeviceFault::InvalidAiDmaLength { len: 0x0004_0000 },
            ),
            (
                AiDmaRequest {
                    dram_addr: RdramAddr::from_offset(0x00ff_fff8),
                    len: 16,
                    sample_rate_hz: 1,
                },
                DeviceFault::AiDmaRangeOverflow {
                    address: 0x00ff_fff8,
                    len: 16,
                },
            ),
        ];

        for (request, expected) in cases {
            let mut fabric = fabric();
            let before = fabric.evidence_snapshot();
            assert_eq!(fabric.start_ai_dma(request), Err(expected));
            assert_eq!(fabric.evidence_snapshot(), before);
        }
    }

    #[test]
    fn typed_ai_requests_accept_exact_register_domain_boundaries() {
        for request in [
            AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x00ff_fff8),
                len: 8,
                sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
            },
            AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0),
                len: AI_LEN_MASK,
                sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
            },
        ] {
            let mut fabric = fabric();
            fabric.configure_tv_type(TvType::Ntsc).unwrap();
            fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
            fabric.start_ai_dma(request).unwrap();
            assert_eq!(fabric.current_ai.unwrap().request, request);
        }
    }

    #[test]
    fn raw_ai_len_write_canonicalizes_before_typed_admission() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        fabric.write_mmio(AI_DRAM_ADDR_REG, 0x1007).unwrap();
        let before = fabric.evidence_snapshot();

        assert_eq!(
            fabric.write_mmio(AI_LEN_REG, 1),
            Err(DeviceFault::ZeroLengthAiDma)
        );
        assert_eq!(fabric.evidence_snapshot(), before);
        assert!(matches!(
            fabric.write_mmio(AI_LEN_REG, 9),
            Ok(DeviceMmioWriteEffect::AiDmaStarted(AiDmaRequest {
                dram_addr,
                len: 8,
                ..
            })) if dram_addr == RdramAddr::from_offset(0x1000)
        ));
    }

    #[test]
    fn ai_exact_rational_deadlines_match_public_region_clocks() {
        for (tv_type, dacrate, expected_rate, expected_deadline) in [
            (TvType::Ntsc, 1_520, 32_006, 93_732),
            (TvType::Pal, 1_551, 31_995, 93_765),
            (TvType::Mpal, 1_519, 31_992, 93_773),
        ] {
            let mut fabric = fabric();
            fabric.configure_tv_type(tv_type).unwrap();
            assert_eq!(
                fabric.write_mmio(AI_DACRATE_REG, dacrate).unwrap(),
                DeviceMmioWriteEffect::AiFrequencyChanged {
                    sample_rate_hz: expected_rate,
                }
            );
            fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
            fabric.write_mmio(AI_DRAM_ADDR_REG, 0x1000).unwrap();
            fabric.write_mmio(AI_LEN_REG, 0x80).unwrap();

            let deadline = fabric.current_ai.unwrap().deadline;
            assert_eq!(deadline, Cycles::new(expected_deadline), "{tv_type:?}");
            let mut rdram = Rdram::new(0);
            fabric
                .advance_to(Cycles::new(expected_deadline - 1), &mut rdram)
                .unwrap();
            assert_ne!(fabric.ai_status() & AI_STATUS_BUSY, 0, "{tv_type:?}");
            assert!(fabric.ai_length() > 0, "{tv_type:?}");
            fabric.advance_to(deadline, &mut rdram).unwrap();
            assert_eq!(fabric.ai_status() & AI_STATUS_BUSY, 0, "{tv_type:?}");
            assert_eq!(fabric.ai_length(), 0, "{tv_type:?}");
            assert!(!fabric.interrupt_pending(InterruptSource::Ai));
        }
    }

    #[test]
    fn ai_exact_rational_max_length_boundary_does_not_use_truncated_rate() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_DACRATE_REG, 1_520).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        fabric.write_mmio(AI_DRAM_ADDR_REG, 0).unwrap();
        fabric.write_mmio(AI_LEN_REG, AI_LEN_MASK).unwrap();

        let deadline = fabric.current_ai.unwrap().deadline;
        assert_eq!(deadline, Cycles::new(191_955_444));
        assert_ne!(deadline, Cycles::new(191_958_149));
        let mut rdram = Rdram::new(0);
        fabric
            .advance_to(Cycles::new(deadline.get() - 1), &mut rdram)
            .unwrap();
        assert_ne!(fabric.ai_status() & AI_STATUS_BUSY, 0);
        assert!(fabric.ai_length() > 0);
        fabric.advance_to(deadline, &mut rdram).unwrap();
        assert_eq!(fabric.ai_status() & AI_STATUS_BUSY, 0);
        assert_eq!(fabric.ai_length(), 0);
    }

    #[test]
    fn ai_review_contract_rejects_metadata_and_busy_rate_writes_without_mutation() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_DACRATE_REG, 1_520).unwrap();
        fabric.write_mmio(AI_BITRATE_REG, 15).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 0x80,
            sample_rate_hz: 32_000,
        };
        let before_mismatch = fabric.evidence_snapshot();
        assert_eq!(
            fabric.start_ai_dma(request),
            Err(DeviceFault::AiSampleRateMismatch {
                request: 32_000,
                register: 32_006,
            })
        );
        assert_eq!(fabric.evidence_snapshot(), before_mismatch);

        fabric
            .start_ai_dma(AiDmaRequest {
                sample_rate_hz: 32_006,
                ..request
            })
            .unwrap();
        let before = fabric.evidence_snapshot();
        assert_eq!(
            fabric.write_mmio(AI_DACRATE_REG, 1_551),
            Err(DeviceFault::AiDacrateWhileBusy {
                current: 1_520,
                requested: 1_551,
            })
        );
        assert_eq!(fabric.evidence_snapshot(), before);
        assert_eq!(
            fabric.write_mmio(AI_BITRATE_REG, 7),
            Err(DeviceFault::AiBitrateWhileBusy {
                current: 15,
                requested: 7,
            })
        );
        assert_eq!(fabric.evidence_snapshot(), before);
    }

    #[test]
    fn ai_deadline_failures_preserve_active_and_dormant_fifo_state() {
        let mut active = fabric();
        active.configure_tv_type(TvType::Ntsc).unwrap();
        active.events.clear();
        active.write_mmio(AI_CONTROL_REG, 1).unwrap();
        active.now = Cycles::new(u64::MAX - 6);
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 8,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let before_start = active.evidence_snapshot();
        assert_eq!(
            active.start_ai_dma(AiDmaRequest {
                len: 0x80,
                ..request
            }),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(active.evidence_snapshot(), before_start);

        let mut dormant = fabric();
        dormant.configure_tv_type(TvType::Ntsc).unwrap();
        dormant.events.clear();
        dormant.now = Cycles::new(u64::MAX);
        dormant.start_ai_dma(request).unwrap();
        let before_enable = dormant.evidence_snapshot();
        assert_eq!(
            dormant.write_mmio(AI_CONTROL_REG, 1),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(dormant.evidence_snapshot(), before_enable);
    }

    #[test]
    fn ai_promotion_preflights_before_event_mutation() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 8,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        fabric.start_ai_dma(request).unwrap();
        fabric
            .start_ai_dma(AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x2000),
                ..request
            })
            .unwrap();
        let deadline = fabric.current_ai.unwrap().deadline;
        fabric.next_event_sequence = u64::MAX;
        let before = fabric.evidence_snapshot();
        let mut rdram = Rdram::new(0);
        assert_eq!(
            fabric.advance_to(deadline, &mut rdram),
            Err(DeviceFault::DeadlineOverflow)
        );
        assert_eq!(fabric.evidence_snapshot(), before);
    }

    #[test]
    fn raw_dpc_end_is_transactional_and_does_not_replay_after_commit() {
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, 0x103).unwrap();
        let before_end = fabric.snapshot();
        let first = match fabric.write_mmio(DPC_END_REG, 0x147).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => submission,
            other => panic!("DPC END did not request renderer work: {other:?}"),
        };
        assert_eq!(first.source, DpcSubmissionSource::Rdram);
        assert_eq!((first.start, first.end), (0x100, 0x140));
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x100);
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap()
                & (DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY | DPC_STATUS_END_VALID),
            DPC_STATUS_DMA_BUSY | DPC_STATUS_CMD_BUSY | DPC_STATUS_END_VALID
        );
        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x140),
            Err(DeviceFault::DpBusy)
        );
        assert!(matches!(
            fabric.commit_dpc_submission(first.token + 1),
            Err(DeviceFault::StaleDpcSubmission { .. })
        ));

        fabric.cancel_dpc_submission(first.token).unwrap();
        let cancelled = fabric.snapshot();
        assert_eq!(cancelled.dpc_start, before_end.dpc_start);
        assert_eq!(cancelled.dpc_end, before_end.dpc_end);
        assert_eq!(cancelled.dpc_current, before_end.dpc_current);
        assert_eq!(cancelled.dpc_status, before_end.dpc_status);

        let retry = match fabric.write_mmio(DPC_END_REG, 0x140).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => submission,
            other => panic!("cancelled DPC END was not retryable: {other:?}"),
        };
        fabric.commit_dpc_submission(retry.token).unwrap();
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x140);
        assert_eq!(fabric.pending_dpc_submission(), None);

        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x140).unwrap(),
            DeviceMmioWriteEffect::None,
            "repeating the committed END pointer must not replay the range"
        );
        let extension = match fabric.write_mmio(DPC_END_REG, 0x180).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => submission,
            other => panic!("DPC END extension did not request renderer work: {other:?}"),
        };
        assert_eq!((extension.start, extension.end), (0x140, 0x180));
    }

    #[test]
    fn empty_dpc_start_end_pair_sets_the_extension_origin() {
        let mut fabric = fabric();
        fabric.write_mmio(DPC_START_REG, 0x100).unwrap();

        assert_eq!(
            fabric.write_mmio(DPC_END_REG, 0x100).unwrap(),
            DeviceMmioWriteEffect::None
        );
        assert_eq!(fabric.read_mmio(DPC_CURRENT_REG).unwrap(), 0x100);
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap() & DPC_STATUS_START_VALID,
            0
        );

        let extension = match fabric.write_mmio(DPC_END_REG, 0x108).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => submission,
            other => panic!("DPC END extension did not request renderer work: {other:?}"),
        };
        assert_eq!((extension.start, extension.end), (0x100, 0x108));
    }

    #[test]
    fn dpc_status_commands_select_xbus_without_overwriting_transaction_bits() {
        let mut fabric = fabric();
        fabric
            .write_mmio(DPC_STATUS_REG, 0x02 | 0x08 | 0x20)
            .unwrap();
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap(),
            DPC_STATUS_XBUS_DMEM_DMA | DPC_STATUS_FREEZE | DPC_STATUS_FLUSH
        );
        fabric.write_mmio(DPC_START_REG, 0x20).unwrap();
        let submission = match fabric.write_mmio(DPC_END_REG, 0x40).unwrap() {
            DeviceMmioWriteEffect::DpcSubmissionRequested(submission) => submission,
            other => panic!("XBUS END did not request renderer work: {other:?}"),
        };
        assert_eq!(submission.source, DpcSubmissionSource::Dmem);
        assert_ne!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap() & DPC_STATUS_DMA_BUSY,
            0
        );
        fabric
            .write_mmio(DPC_STATUS_REG, 0x01 | 0x04 | 0x10)
            .unwrap();
        assert_eq!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap()
                & (DPC_STATUS_XBUS_DMEM_DMA | DPC_STATUS_FREEZE | DPC_STATUS_FLUSH),
            0
        );
        assert_ne!(
            fabric.read_mmio(DPC_STATUS_REG).unwrap()
                & (DPC_STATUS_DMA_BUSY | DPC_STATUS_END_VALID),
            0,
            "status mode commands cannot consume the renderer transaction"
        );
    }

    #[test]
    fn dpc_source_domains_reject_wrapped_or_out_of_range_command_bytes() {
        let mut fabric = fabric();
        for (source, start, end) in [
            (DpcSubmissionSource::Dmem, 0x0ff8, 0x1008),
            (DpcSubmissionSource::Dmem, 0x0800, 0x0400),
            (DpcSubmissionSource::Rdram, 0x00ff_fff8, 0x0100_0008),
            (DpcSubmissionSource::Rdram, 0x0100, 0x0104),
        ] {
            assert_eq!(
                fabric.request_dpc_submission(source, start, end),
                Err(DeviceFault::InvalidDpcRange { source, start, end })
            );
            assert_eq!(fabric.pending_dpc_submission(), None);
        }
    }

    #[test]
    fn snapshots_distinguish_future_affecting_ai_and_dpc_latches() {
        let mut baseline = fabric();
        let mut ai_changed = fabric();
        let mut dpc_changed = fabric();
        for fabric in [&mut baseline, &mut ai_changed, &mut dpc_changed] {
            fabric.configure_tv_type(TvType::Ntsc).unwrap();
        }
        ai_changed.write_mmio(AI_CONTROL_REG, 1).unwrap();
        dpc_changed.write_mmio(DPC_START_REG, 0x80).unwrap();

        assert_ne!(baseline.snapshot(), ai_changed.snapshot());
        assert_ne!(baseline.snapshot(), dpc_changed.snapshot());
        assert_ne!(baseline.evidence_snapshot(), ai_changed.evidence_snapshot());
        assert_ne!(
            baseline.evidence_snapshot(),
            dpc_changed.evidence_snapshot()
        );
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
    fn release_evidence_distinguishes_pif_state_that_the_compact_snapshot_cannot() {
        let mut left = fabric();
        let mut right = fabric();
        left.pif_ram_cpu_write_w(0, 0x1122_3344);
        right.pif_ram_cpu_write_w(0, 0x5566_7788);

        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());

        let request = SiDmaRequest {
            kind: SiDmaKind::PifToDram,
            dram_addr: RdramAddr::from_offset(0),
        };
        left.start_si_dma(request).unwrap();
        right.start_si_dma(request).unwrap();
        let mut left_rdram = Rdram::new(64);
        let mut right_rdram = Rdram::new(64);
        left.advance_to_with_pif(Cycles::new(1), &mut left_rdram, |_, _, _| {})
            .unwrap();
        right
            .advance_to_with_pif(Cycles::new(1), &mut right_rdram, |_, _, _| {})
            .unwrap();
        assert_ne!(left_rdram.read_bytes(0, 64), right_rdram.read_bytes(0, 64));
    }

    #[test]
    fn release_evidence_binds_rsp_memory_and_queued_ai_identity() {
        let mut left = fabric();
        let mut right = fabric();
        left.write_mmio(MmioAddr::new(SP_DMEM_START), 0x1122_3344)
            .unwrap();
        right
            .write_mmio(MmioAddr::new(SP_DMEM_START), 0x5566_7788)
            .unwrap();
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());

        let current = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x20),
            len: 0x100,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let mut left = fabric();
        let mut right = fabric();
        left.configure_tv_type(TvType::Ntsc).unwrap();
        right.configure_tv_type(TvType::Ntsc).unwrap();
        left.write_mmio(AI_CONTROL_REG, 1).unwrap();
        right.write_mmio(AI_CONTROL_REG, 1).unwrap();
        left.start_ai_dma(current).unwrap();
        right.start_ai_dma(current).unwrap();
        left.start_ai_dma(AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x200),
            ..current
        })
        .unwrap();
        right
            .start_ai_dma(AiDmaRequest {
                dram_addr: RdramAddr::from_offset(0x200),
                len: 0x108,
                ..current
            })
            .unwrap();
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());
    }

    #[test]
    fn release_evidence_binds_save_bytes_and_pending_eeprom_programming() {
        use crate::save::{InMemorySaveStorage, SaveType};

        let mut left = fabric();
        let mut right = fabric();
        left.pi_dma_mut()
            .set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        right
            .pi_dma_mut()
            .set_save(Box::new(InMemorySaveStorage::for_device(
                SaveType::Eeprom4k,
            )));
        left.pi_dma_mut().save_write_from(0, &[0x11; 8]);
        right.pi_dma_mut().save_write_from(0, &[0x22; 8]);
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());

        left.pi_dma_mut().save_write_from(0, &[0x33; 8]);
        right.pi_dma_mut().save_write_from(0, &[0x33; 8]);
        left.pi_dma_mut()
            .start_eeprom_write(Cycles::ZERO, 1, [0x44; 8])
            .unwrap();
        right
            .pi_dma_mut()
            .start_eeprom_write(Cycles::ZERO, 1, [0x55; 8])
            .unwrap();
        assert_eq!(left.snapshot(), right.snapshot());
        assert_ne!(left.evidence_snapshot(), right.evidence_snapshot());
    }

    #[test]
    #[should_panic(expected = "PiTimingModel::evidence_bytes must identify")]
    fn release_evidence_rejects_an_unidentified_pi_timing_policy() {
        struct UnidentifiedTiming;
        impl PiTimingModel for UnidentifiedTiming {
            fn completion_latency(
                &self,
                _request: PiDmaRequest,
                _timing: PiDomainTiming,
            ) -> Cycles {
                Cycles::new(1)
            }

            fn evidence_bytes(&self) -> Vec<u8> {
                Vec::new()
            }
        }

        let mut fabric =
            DeviceFabric::new(PiDma::new(InMemoryRom::new(Vec::new())), UnidentifiedTiming);
        let _ = fabric.evidence_snapshot();
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
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let first = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 400,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let second = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x2000),
            ..first
        };
        fabric.start_ai_dma(first).unwrap();
        fabric.start_ai_dma(second).unwrap();
        assert_eq!(
            fabric.ai_status(),
            AI_STATUS_ENABLED | AI_STATUS_BUSY | AI_STATUS_FULL
        );
        assert_eq!(fabric.ai_length(), 400);
        assert_eq!(fabric.start_ai_dma(first), Err(DeviceFault::AiFull));

        let mut rdram = Rdram::new(0x100);
        assert!(fabric
            .advance_to(Cycles::new(192), &mut rdram)
            .unwrap()
            .is_empty());
        assert!(fabric.ai_length() > 0);
        let first_done = fabric.advance_to(Cycles::new(193), &mut rdram).unwrap();
        assert_eq!(first_done, vec![DeviceNotification::AiDmaComplete(first)]);
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);
        assert_eq!(fabric.ai_length(), 400);
        assert!(fabric.interrupt_pending(InterruptSource::Ai));

        fabric.clear_interrupt(InterruptSource::Ai);
        let second_done = fabric.advance_to(Cycles::new(386), &mut rdram).unwrap();
        assert!(second_done.is_empty());
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED);
        assert_eq!(fabric.ai_length(), 0);
        assert!(!fabric.interrupt_pending(InterruptSource::Ai));
    }

    #[test]
    fn ai_control_gates_drain_without_rejecting_fifo_writes() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        let request = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 0x80,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };

        fabric.write_mmio(AI_DRAM_ADDR_REG, 0x1000).unwrap();
        let ai_events_before = fabric
            .evidence_snapshot()
            .scheduled_events
            .iter()
            .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
            .count();
        assert_eq!(
            fabric.write_mmio(AI_LEN_REG, 0x80).unwrap(),
            DeviceMmioWriteEffect::AiDmaStarted(request)
        );
        assert_eq!(fabric.ai_status(), AI_STATUS_BUSY);
        assert_eq!(fabric.ai_length(), 0x80);
        assert_eq!(
            fabric
                .evidence_snapshot()
                .scheduled_events
                .iter()
                .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
                .count(),
            ai_events_before
        );
        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);
        assert_eq!(
            fabric
                .evidence_snapshot()
                .scheduled_events
                .iter()
                .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
                .count(),
            ai_events_before + 1
        );
        assert_eq!(
            fabric.write_mmio(AI_CONTROL_REG, 0),
            Err(DeviceFault::AiControlWhileBusy {
                current: 1,
                requested: 0,
            })
        );
        assert_eq!(fabric.ai_control(), 1);
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);
    }

    #[test]
    fn ai_disabled_fifo_accepts_two_slots_then_full_edge_interrupts_once() {
        let mut fabric = fabric();
        fabric.configure_tv_type(TvType::Ntsc).unwrap();
        let first = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x1000),
            len: 8,
            sample_rate_hz: TvType::Ntsc.vi_clock_hz(),
        };
        let second = AiDmaRequest {
            dram_addr: RdramAddr::from_offset(0x2000),
            ..first
        };
        fabric.start_ai_dma(first).unwrap();
        fabric.start_ai_dma(second).unwrap();
        assert_eq!(fabric.ai_status(), AI_STATUS_BUSY | AI_STATUS_FULL);
        assert_eq!(fabric.ai_length(), 8);
        assert_eq!(fabric.start_ai_dma(first), Err(DeviceFault::AiFull));
        assert_eq!(
            fabric
                .evidence_snapshot()
                .scheduled_events
                .iter()
                .filter(|event| event.kind == ScheduledDeviceEventKind::Ai)
                .count(),
            0
        );

        fabric.write_mmio(AI_CONTROL_REG, 1).unwrap();
        let first_deadline = fabric.current_ai.unwrap().deadline;
        let mut rdram = Rdram::new(0);
        assert_eq!(
            fabric.advance_to(first_deadline, &mut rdram).unwrap(),
            vec![DeviceNotification::AiDmaComplete(first)]
        );
        assert!(fabric.interrupt_pending(InterruptSource::Ai));
        assert_eq!(fabric.current_ai.unwrap().request, second);
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED | AI_STATUS_BUSY);

        fabric.clear_interrupt(InterruptSource::Ai);
        let second_deadline = fabric.current_ai.unwrap().deadline;
        assert!(fabric
            .advance_to(second_deadline, &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(fabric.ai_status(), AI_STATUS_ENABLED);
        assert!(!fabric.interrupt_pending(InterruptSource::Ai));
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
        fabric
            .start_rcp_task(RcpTaskCompletionPlan::SpThenDpFullSync)
            .unwrap();
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
    fn task_without_dp_full_sync_completes_sp_only() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric
            .start_rcp_task(RcpTaskCompletionPlan::SpOnly)
            .unwrap();
        assert!(fabric.snapshot().sp_busy);
        assert!(!fabric.snapshot().dp_busy);

        assert_eq!(
            fabric.advance_to(Cycles::new(1), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Sp)]
        );
        assert!(!fabric.interrupt_pending(InterruptSource::Dp));
        assert!(fabric
            .advance_to(Cycles::new(2), &mut rdram)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn chunked_rcp_task_is_busy_without_a_fabricated_completion_deadline() {
        let mut fabric = fabric();
        fabric.begin_rcp_task().unwrap();
        assert!(fabric.snapshot().sp_busy);
        assert_eq!(fabric.next_deadline(), None);

        fabric
            .finish_rcp_task(RcpTaskCompletionPlan::SpOnly, Cycles::new(2))
            .unwrap();
        assert_eq!(fabric.next_deadline(), Some(Cycles::new(2)));
        assert_eq!(
            fabric.finish_rcp_task(RcpTaskCompletionPlan::SpOnly, Cycles::new(1)),
            Err(DeviceFault::SpBusy),
            "one in-flight task token may acquire only one completion event"
        );
    }

    #[test]
    fn raw_dp_full_sync_completes_dp_without_starting_sp() {
        let mut fabric = fabric();
        let mut rdram = Rdram::new(0x100);
        fabric.start_dp_full_sync(Cycles::new(3)).unwrap();
        assert!(!fabric.snapshot().sp_busy);
        assert!(fabric.snapshot().dp_busy);
        assert!(fabric
            .advance_to(Cycles::new(2), &mut rdram)
            .unwrap()
            .is_empty());

        assert_eq!(
            fabric.advance_to(Cycles::new(3), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
        assert!(!fabric.interrupt_pending(InterruptSource::Sp));
        assert!(fabric.interrupt_pending(InterruptSource::Dp));
    }

    #[test]
    fn second_raw_dp_full_sync_rejects_without_replacing_pending_completion() {
        let mut fabric = fabric();
        fabric.start_dp_full_sync(Cycles::new(3)).unwrap();
        let before = fabric.evidence_snapshot();

        assert_eq!(
            fabric.start_dp_full_sync(Cycles::new(1)),
            Err(DeviceFault::DpBusy)
        );
        assert_eq!(fabric.evidence_snapshot(), before);

        let mut rdram = Rdram::new(0x100);
        assert!(fabric
            .advance_to(Cycles::new(1), &mut rdram)
            .unwrap()
            .is_empty());
        assert_eq!(
            fabric.advance_to(Cycles::new(3), &mut rdram).unwrap(),
            vec![DeviceNotification::RcpTaskComplete(RcpTaskCompletion::Dp)]
        );
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
        assert_eq!(
            notifications,
            vec![DeviceNotification::ViRetrace {
                at: Cycles::new(191)
            }]
        );
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
            DeviceTraceKind::NotificationReady(DeviceNotification::ViRetrace {
                at: Cycles::new(191)
            })
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
        assert_eq!(fabric.next_vi_deadline(), Some(Cycles::new(1_875_000)));

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
        assert_eq!(fabric.next_vi_deadline(), fabric.vi_field_interval());
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
