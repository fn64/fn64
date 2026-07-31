//! fn64-runtime: the core, pure-Rust half of fn64.
//!
//! See `docs/DESIGN.md` (workspace root) sections 1-3 for the architecture
//! this crate implements: the rdram ownership model, the `RdramAddr`
//! translation newtype, and `OSMesgQueue` semantics. This crate has zero
//! knowledge of `fn64-abi`'s extern "C" surface or `fn64-rt64`'s C++
//! interop — it is deliberately the independently-testable core.
//!
//! Design provenance for every non-obvious semantic choice below is cited
//! inline; see `docs/DESIGN.md` section 6 for the full provenance table.

pub mod device;
pub mod dpc_schedule;
pub mod executor;
pub mod mesgqueue;
pub mod mmio;
pub mod overlay;
pub mod peripherals;
pub mod pfs;
pub mod rdram;
pub mod rom;
pub mod rsp;
pub mod save;
pub mod si;
pub mod thread;
pub mod timer;
pub mod trace;
pub mod transfer_pak;
pub mod tv;
pub mod unsupported;
pub mod vi;
pub mod voice;

pub use device::{
    AiDmaRequest, Cycles, DeviceEvidenceSnapshot, DeviceFabric, DeviceFault, DeviceMmioWriteEffect,
    DeviceNotification, DeviceSnapshot, DeviceTraceEvent, DeviceTraceKind, DeviceTraceSummary,
    DpcSubmission, DpcSubmissionSource, FixedPiTiming, InterruptSource, MmioAddr,
    PendingAiSnapshot, PendingDpcSnapshot, PendingPiSnapshot, PendingSiSnapshot,
    PendingSpDmaSnapshot, PiDmaRequest, PiDomain, PiDomainTiming, PiTimingModel, RcpTaskCompletion,
    RcpTaskCompletionPlan, RspExecutionState, ScheduledDeviceEventKind,
    ScheduledDeviceEventSnapshot, SiDmaKind, SiDmaRequest, SpDmaDirection, SpDmaRequest,
    DPC_STATUS_CLEAR_CLOCK_COUNTER_COMMAND, DPC_STATUS_CLEAR_CMD_COUNTER_COMMAND,
    DPC_STATUS_CLEAR_PIPE_COUNTER_COMMAND, DPC_STATUS_CLEAR_TMEM_COUNTER_COMMAND,
    DPC_STATUS_CMD_BUSY, DPC_STATUS_DMA_BUSY, DPC_STATUS_END_VALID, DPC_STATUS_FLUSH,
    DPC_STATUS_FREEZE, DPC_STATUS_START_VALID, DPC_STATUS_XBUS_DMEM_DMA, PI_STATUS_DMA_BUSY,
    PI_STATUS_ERROR, PI_STATUS_IO_BUSY, SP_CLR_YIELD, SP_CLR_YIELDED, SP_SET_YIELD, SP_SET_YIELDED,
    SP_STATUS_BROKE, SP_STATUS_DMA_BUSY, SP_STATUS_DMA_FULL, SP_STATUS_HALT,
    SP_STATUS_INTERRUPT_ON_BREAK, SP_STATUS_SIGNAL_0, SP_STATUS_SIGNAL_1, SP_STATUS_SINGLE_STEP,
    SP_STATUS_YIELD, SP_STATUS_YIELDED,
};
pub use dpc_schedule::{
    DpcAdvance, DpcBackendQuantumAck, DpcBackendQuantumRequest, DpcBackendQuantumStatus, DpcCursor,
    DpcQuantumId, DpcQuantumPlan, DpcScheduleError, DpcScheduledExecution, DpcScheduledPhase,
    DpcTransactionId,
};
pub use executor::{
    EventRegistrationEvidenceSnapshot, Executor, ExecutorControlEvidenceSnapshot,
    ExecutorControlInvariantError, ExecutorQueueEvidenceSnapshot, ExecutorRunningEvidenceSnapshot,
    ExternalEvent, PendingResumeEvidenceSnapshot, ProcessExitSummary,
    RdramRegistrationEvidenceSnapshot, RecvMesgOutcome, SendMesgOutcome, ThreadEvidenceSnapshot,
};
pub use mesgqueue::{
    BlockedReceiverEvidenceSnapshot, BlockedSenderEvidenceSnapshot, Mesg, MesgQueue,
    MesgQueueActivity, MesgQueueEvidenceSnapshot, RecvResult, SendPlacement, SendResult,
    WaiterPriority,
};
pub use mmio::{
    is_mmio_offset, AiRegs, MmioSpace, AI_STATUS_BUSY, AI_STATUS_ENABLED, AI_STATUS_FULL,
    RDRAM_MMIO_WINDOW_END, RDRAM_MMIO_WINDOW_START, RDRAM_RCP_MMIO_END,
};
pub use overlay::{
    FuncEntry, FuncEntryEvidenceSnapshot, Section, SectionEvidenceSnapshot, SectionIndex,
    SectionLoadEvidenceSnapshot, SectionRegistry, SectionRegistryEvidenceSnapshot,
    StaticMirrorEvidenceSnapshot, StaticStorageEndEvidenceSnapshot,
};
pub use peripherals::{
    ControllerOperationDevice, ControllerOperationEvent, ControllerOperationKind, Peripherals,
    PeripheralsEvidenceSnapshot,
};
pub use pfs::{
    ControllerPak, ControllerPakBankCount, ControllerPakEvidenceSnapshot, PfsError, PfsKey,
    PfsNoteEvidenceSnapshot, PfsState,
};
pub use rdram::{
    with_physical_rdram_read, PhysicalRdramRead, Rdram, RdramAddr, RdramPtr, RdramView,
    RdramViewMut,
};
pub use rom::{
    DmaCompletion, DmaMemory, DmaWriterChannel, InMemoryRom, PendingEepromWriteSnapshot, PiDma,
    PiDmaError, ProcessDmaMemory, RomStorage,
};
pub use rsp::{
    OsTaskHeader, RspMemAddr, RspMemory, RspMemoryBank, RspMemoryError, TaskLog, M_AUDTASK,
    M_GFXTASK, OS_TASK_YIELDED, RSP_MEMORY_BANK_SIZE,
};
pub use save::{
    load_mbc3_battery_sidecar, store_mbc3_battery_sidecar, EepromError, EepromKind, EepromStatus,
    FileSaveStorage, InMemorySaveStorage, Mbc3BatteryFileError, SaveOperationEvent,
    SaveOperationKind, SaveStorage, SaveType, EEPROM_WRITE_CYCLES,
};
pub use si::{
    ContInput, PifEvidenceSnapshot, PifModel, PortState, RumbleError, CONT_ABSENT, CONT_CARD_ON,
    CONT_TYPE_STANDARD,
};
pub use thread::{GameThread, Priority, Resume, RunToken, ThreadState, Yield, OS_PRIORITY_IDLE};
pub use timer::{TimerEvidenceSnapshot, TimerId, TimerWheel, TimerWheelEvidenceSnapshot};
pub use trace::{
    DmaDirection, QueueOpKind, SwitchReason, TaskKind, ThreadId, TraceEvent, TraceKind, TraceLog,
};
pub use transfer_pak::{
    GameBoyCartridgeEvidenceSnapshot, GameBoyMapperEvidenceSnapshot, HostUnixNanos,
    Mbc3BatteryMetadata, Mbc3BatteryMetadataError, Mbc3BatteryRestore, TransferPak,
    TransferPakError, TransferPakEvidenceSnapshot, TransferPakStatus, MBC3_BATTERY_METADATA_LEN,
    TRANSFER_PAK_BLOCK_SIZE,
};
pub use tv::{TvType, CPU_CLOCK_HZ};
pub use unsupported::{
    arm_unsupported_events, arm_unsupported_events_with_run_identity,
    complete_unsupported_observation, copy_unsupported_events, record_unsupported_event,
    unsupported_events_armed, unsupported_journal_error, UnsupportedDisposition, UnsupportedEvent,
    UnsupportedSubsystem, UNSUPPORTED_INSTRUMENTATION_SCHEMA, UNSUPPORTED_INSTRUMENTATION_SHA256,
};
pub use vi::{
    PendingViFade, RetraceSchedule, RetraceScheduleEvidenceSnapshot, ViEvidenceSnapshot, ViState,
};
pub use voice::{VoiceData, VoiceError, VoiceEvidenceSnapshot, VoiceUnit};
