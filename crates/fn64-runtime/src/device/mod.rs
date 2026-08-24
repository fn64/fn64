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
use crate::rom::{DmaCompletion, DmaMemory, PiDeviceAddress, PiDma, PiDmaError, RomStorage};
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

// DPC STATUS counter-clear commands (bits 6..9 of a STATUS write). Each clears
// exactly one of the four performance counters without touching STATUS mode
// bits or the other counters.
pub const DPC_STATUS_CLEAR_TMEM_COUNTER_COMMAND: u32 = 1 << 6;
pub const DPC_STATUS_CLEAR_PIPE_COUNTER_COMMAND: u32 = 1 << 7;
pub const DPC_STATUS_CLEAR_CMD_COUNTER_COMMAND: u32 = 1 << 8;
pub const DPC_STATUS_CLEAR_CLOCK_COUNTER_COMMAND: u32 = 1 << 9;

const AI_DRAM_ADDR_MASK: u32 = 0x00ff_fff8;
const AI_LEN_MASK: u32 = 0x0003_fff8;
const AI_DRAM_DOMAIN_END: u32 = 0x0100_0000;
const AI_DACRATE_MASK: u32 = 0x0000_3fff;
const AI_BITRATE_MASK: u32 = 0x0000_000f;
const DPC_ADDR_MASK: u32 = 0x00ff_fff8;
const DPC_COUNTER_MASK: u32 = 0x00ff_ffff;

const MI_INTR_REG: MmioAddr = MmioAddr::new(0xA430_0008);
const MI_INTR_MASK_REG: MmioAddr = MmioAddr::new(0xA430_000C);
const DPC_START_REG: MmioAddr = MmioAddr::new(0xA410_0000);
const DPC_END_REG: MmioAddr = MmioAddr::new(0xA410_0004);
const DPC_CURRENT_REG: MmioAddr = MmioAddr::new(0xA410_0008);
const DPC_STATUS_REG: MmioAddr = MmioAddr::new(0xA410_000C);
const DPC_CLOCK_REG: MmioAddr = MmioAddr::new(0xA410_0010);
const DPC_BUFBUSY_REG: MmioAddr = MmioAddr::new(0xA410_0014);
const DPC_PIPEBUSY_REG: MmioAddr = MmioAddr::new(0xA410_0018);
const DPC_TMEM_REG: MmioAddr = MmioAddr::new(0xA410_001C);
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
const PI_DOM1_ADDR2_START: u32 = 0x1000_0000;
const PI_DOM1_ADDR2_END: u32 = 0x1FC0_0000;
const PI_DOM2_ADDR2_START: u32 = 0x0800_0000;
const PI_DOM2_ADDR2_END: u32 = 0x1000_0000;
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

/// Apply the three DPC STATUS mode clear/set command pairs (xbus, freeze,
/// flush) to a status word. Shared between the live STATUS write and the
/// pending-submission rollback mirror so an interleaved mode command survives
/// renderer cancellation.
fn apply_dpc_status_mode_commands(status: &mut u32, command: u32) {
    apply_device_clear_set_pair(status, command, 0, 1, DPC_STATUS_XBUS_DMEM_DMA);
    apply_device_clear_set_pair(status, command, 2, 3, DPC_STATUS_FREEZE);
    apply_device_clear_set_pair(status, command, 4, 5, DPC_STATUS_FLUSH);
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
    pub device: PiDeviceAddress,
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

/// Move-only reservation of an ordered RSP task's future DPC submissions.
/// Reserving allocates globally ordered fabric tokens but does not touch DPC
/// registers or make any renderer transaction pending. Members can only be
/// activated from the front through [`DeviceFabric::activate_reserved_dpc_submission`].
#[derive(Debug)]
pub struct ReservedDpcSubmissionBatch {
    submissions: Box<[DpcSubmission]>,
    next: usize,
}

impl ReservedDpcSubmissionBatch {
    /// All reserved identities in activation order. This read-only view lets
    /// a renderer bind its plans to the exact future fabric tokens.
    pub fn submissions(&self) -> &[DpcSubmission] {
        &self.submissions
    }

    /// Number of members not yet activated.
    pub fn remaining(&self) -> usize {
        self.submissions.len() - self.next
    }
}

/// A known-width RDP command parked until a later END exposes the remainder.
///
/// The DPC accepts END extensions in 8-byte increments, so a multiword command
/// straddles several END writes; hardware stalls CURRENT at that command's
/// start rather than decoding a truncated stream. This is the raw CPU MMIO
/// counterpart of the coalescing `fn64-abi`'s `coalesce_dp_submissions`
/// already performs for RSP-produced streams.
///
/// `retained_words` spans `command_start..exposed_end` and is **captured, not
/// a range to reread**: XBUS DMEM can change between END writes, so the bytes
/// admitted with the first END are the bytes that must be decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StalledDpc {
    pub source: DpcSubmissionSource,
    pub command_start: u32,
    pub exposed_end: u32,
    pub bytes_required: u32,
    pub retained_words: Vec<u32>,
}

/// Pointer-free architectural registers produced by one synchronous RSP run.
///
/// DPC command transactions are deliberately absent: renderer ownership is
/// committed separately through DeviceFabric::request_dpc_submission and
/// DeviceFabric::commit_dpc_submission. The public RSP Programmer's Guide,
/// chapter 4, defines the SP DMA/status/semaphore registers; the public
/// rcp.h register map defines the eight DPC registers carried here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RspExecutionState {
    pub pc: u32,
    pub sp_status: u32,
    pub sp_semaphore: bool,
    pub sp_dma_mem_addr: RspMemAddr,
    pub sp_dma_dram_addr: RdramAddr,
    pub sp_dma_read_length: u32,
    pub sp_dma_write_length: u32,
    pub dpc_start: u32,
    pub dpc_end: u32,
    pub dpc_current: u32,
    pub dpc_status: u32,
    pub dpc_clock: u32,
    pub dpc_busy: u32,
    pub dpc_pipe_busy: u32,
    pub dpc_tmem_busy: u32,
}

/// Host work made necessary by a successfully latched MMIO write.
///
/// The device mutation has already happened when this value is returned. A
/// production caller must perform the named host action before allowing the
/// guest to retire another instruction. In particular, a DPC request remains
/// pending until its exact token is committed or cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "MMIO write effects must be handled before guest execution resumes"]
pub enum DeviceMmioWriteEffect {
    None,
    AiFrequencyChanged {
        sample_rate_hz: u32,
    },
    AiDmaStarted(AiDmaRequest),
    DpcSubmissionRequested {
        submission: DpcSubmission,
        /// Empty for a new stream; a continuation carries a clone of the
        /// fabric-owned stalled tail so the ABI can prepend it without
        /// rereading memory that may have changed.
        retained_tail: Vec<u32>,
    },
    /// A raw `SP_STATUS` write cleared HALT while the RSP was halted, which on
    /// hardware starts it executing IMEM from `SP_PC`. Reported as an effect so
    /// a host can run the RSP; the device models registers, not execution.
    ///
    /// Only the raw MMIO path produces this. `write_sp_status` is also called
    /// directly by the libultra `__osSpSetStatus` shim, which does not route
    /// through `write_mmio` because the HLE task lane kicks the RSP itself.
    RspStartRequested {
        pc: u32,
    },
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
        match self.device {
            PiDeviceAddress::RomOffset(_) => PiDomain::Domain1,
            PiDeviceAddress::SramOffset(_) => PiDomain::Domain2,
        }
    }
}

/// Decode the physical PI CART latch only when a raw length-register write
/// starts a transfer. The public libultra `osPiRawStartDma` manual defines
/// `OS_READ` as device-to-RDRAM and `OS_WRITE` as RDRAM-to-device; its PI
/// domain documentation keeps the physical bus window separate from the
/// device-relative address. The Unlicense libdragon PI behavior independently
/// programs WR_LEN for cartridge-to-RDRAM transfers and removes the Domain-1
/// Address-2 base before indexing ROM. Those constraints determine this
/// boundary; shim callers already supply a typed device-relative address.
fn decode_raw_pi_device_address(physical: u32) -> Result<PiDeviceAddress, DeviceFault> {
    if (PI_DOM1_ADDR2_START..PI_DOM1_ADDR2_END).contains(&physical) {
        Ok(PiDeviceAddress::RomOffset(physical - PI_DOM1_ADDR2_START))
    } else if (PI_DOM2_ADDR2_START..PI_DOM2_ADDR2_END).contains(&physical) {
        Ok(PiDeviceAddress::SramOffset(physical - PI_DOM2_ADDR2_START))
    } else {
        Err(DeviceFault::InvalidPiCartAddress { physical })
    }
}

fn physical_pi_device_range(device: PiDeviceAddress, len: u32) -> Result<u32, DeviceFault> {
    let (base, span, offset) = match device {
        PiDeviceAddress::RomOffset(offset) => (
            PI_DOM1_ADDR2_START,
            PI_DOM1_ADDR2_END - PI_DOM1_ADDR2_START,
            offset,
        ),
        PiDeviceAddress::SramOffset(offset) => (
            PI_DOM2_ADDR2_START,
            PI_DOM2_ADDR2_END - PI_DOM2_ADDR2_START,
            offset,
        ),
    };
    let end = offset
        .checked_add(len)
        .ok_or(DeviceFault::InvalidPiDeviceRange { device, len })?;
    if offset >= span || end > span {
        return Err(DeviceFault::InvalidPiDeviceRange { device, len });
    }
    Ok(base + offset)
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

/// Constant-space counts of accepted device transitions. These remain active
/// when diagnostic event retention is disabled, so long exploratory runs can
/// report progress without growing an unbounded trace vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceTraceSummary {
    pub events: u64,
    pub pi_dma_started: u64,
    pub si_dma_started: u64,
    pub ai_dma_started: u64,
    pub sp_dma_started: u64,
    pub sp_tasks_admitted: u64,
    pub rcp_tasks_started: u64,
    pub rcp_tasks_completed: u64,
    pub vi_interrupts: u64,
}

impl DeviceTraceSummary {
    fn record(&mut self, kind: DeviceTraceKind) {
        self.events = self
            .events
            .checked_add(1)
            .expect("device event count overflow");
        let counter = match kind {
            DeviceTraceKind::PiDmaStarted(_) => Some(&mut self.pi_dma_started),
            DeviceTraceKind::SiDmaStarted(_) => Some(&mut self.si_dma_started),
            DeviceTraceKind::AiDmaStarted(_) => Some(&mut self.ai_dma_started),
            DeviceTraceKind::SpDmaStarted(_) => Some(&mut self.sp_dma_started),
            DeviceTraceKind::SpTaskAdmitted { .. } => Some(&mut self.sp_tasks_admitted),
            DeviceTraceKind::RcpTaskStarted { .. } => Some(&mut self.rcp_tasks_started),
            DeviceTraceKind::RcpTaskComplete(_) => Some(&mut self.rcp_tasks_completed),
            DeviceTraceKind::ViInterrupt => Some(&mut self.vi_interrupts),
            _ => None,
        };
        if let Some(counter) = counter {
            *counter = counter.checked_add(1).expect("device event count overflow");
        }
    }
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
    /// An END write arrived while a tail was parked, but it does not continue
    /// that stream. A mismatched source, a latched START, or a non-advancing
    /// END are all real stream boundaries -- concatenating across one would
    /// splice unrelated commands, which is exactly what an XBUS ring wrap
    /// looks like.
    InvalidStalledDpcContinuation {
        expected_source: DpcSubmissionSource,
        received_source: DpcSubmissionSource,
        exposed_end: u32,
        received_end: u32,
        start_valid: bool,
    },
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
    InvalidRspExecutionPc {
        pc: u32,
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
    InvalidPiDeviceRange {
        device: PiDeviceAddress,
        len: u32,
    },
    InvalidPiCartAddress {
        physical: u32,
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
            Self::InvalidStalledDpcContinuation {
                expected_source,
                received_source,
                exposed_end,
                received_end,
                start_valid,
            } => write!(
                f,
                "invalid stalled DPC continuation: expected {expected_source:?} bytes after \
                 {exposed_end:#010X}, got {received_source:?} END {received_end:#010X}, \
                 START_VALID={start_valid}"
            ),
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
            Self::InvalidRspExecutionPc { pc } => write!(
                f,
                "synchronous RSP execution PC must be an aligned canonical low-12 address, got {pc:#010X}"
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
            Self::InvalidPiDeviceRange { device, len } => {
                write!(
                    f,
                    "PI device-relative range {device:?} + {len:#x} bytes escapes its physical domain"
                )
            }
            Self::InvalidPiCartAddress { physical } => write!(
                f,
                "PI CART address {physical:#010X} is outside supported Domain-1/2 Address-2 windows"
            ),
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
pub(crate) struct PendingPi {
    token: u64,
    request: PiDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingAi {
    token: u64,
    request: AiDmaRequest,
    started_at: Cycles,
    deadline: Cycles,
}

/// Public DPC performance counters expose a 24-bit modulo domain. The current
/// model imports counter values and honors the STATUS counter-clear commands,
/// but does not fabricate increments.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DpcCounter24(u32);

impl DpcCounter24 {
    const ZERO: Self = Self(0);

    const fn from_register(value: u32) -> Self {
        Self(value & DPC_COUNTER_MASK)
    }

    const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DpcRegisters {
    start: u32,
    end: u32,
    current: u32,
    status: u32,
    clock: DpcCounter24,
    busy: DpcCounter24,
    pipe_busy: DpcCounter24,
    tmem_busy: DpcCounter24,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingDpc {
    submission: DpcSubmission,
    rollback: DpcRegisters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingSi {
    token: u64,
    request: SiDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingSpDma {
    token: u64,
    request: SpDmaRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeviceEvent {
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
    pub dpc_clock: u32,
    pub dpc_busy: u32,
    pub dpc_pipe_busy: u32,
    pub dpc_tmem_busy: u32,
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
mod fabric;
mod fabric_ops;
pub use fabric::*;
pub use fabric_ops::ReadyDpcFabricCommit;

#[cfg(test)]
#[allow(unused_must_use)]
mod tests;
