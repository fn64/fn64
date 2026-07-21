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
pub mod vi;
pub mod voice;

pub use device::{
    AiDmaRequest, Cycles, DeviceFabric, DeviceFault, DeviceNotification, DeviceSnapshot,
    DeviceTraceEvent, DeviceTraceKind, FixedPiTiming, InterruptSource, MmioAddr, PiDmaRequest,
    PiDomain, PiDomainTiming, PiTimingModel, RcpTaskCompletion, SiDmaKind, SiDmaRequest,
    SpDmaDirection, SpDmaRequest, PI_STATUS_DMA_BUSY, PI_STATUS_ERROR, PI_STATUS_IO_BUSY,
    SP_CLR_YIELD, SP_CLR_YIELDED, SP_SET_YIELD, SP_SET_YIELDED, SP_STATUS_BROKE,
    SP_STATUS_DMA_BUSY, SP_STATUS_DMA_FULL, SP_STATUS_HALT, SP_STATUS_INTERRUPT_ON_BREAK,
    SP_STATUS_SIGNAL_0, SP_STATUS_SIGNAL_1, SP_STATUS_SINGLE_STEP, SP_STATUS_YIELD,
    SP_STATUS_YIELDED,
};
pub use executor::{Executor, ExternalEvent, RecvMesgOutcome, SendMesgOutcome};
pub use mesgqueue::{Mesg, MesgQueue, RecvResult, SendResult};
pub use mmio::{
    is_mmio_offset, AiRegs, MmioSpace, AI_STATUS_BUSY, AI_STATUS_FULL, RDRAM_MMIO_WINDOW_END,
    RDRAM_MMIO_WINDOW_START,
};
pub use overlay::{FuncEntry, Section, SectionIndex, SectionRegistry};
pub use peripherals::Peripherals;
pub use pfs::{ControllerPak, PfsError, PfsKey, PfsState};
pub use rdram::{Rdram, RdramAddr, RdramPtr, RdramView, RdramViewMut};
pub use rom::{DmaCompletion, DmaMemory, InMemoryRom, PiDma, PiDmaError, RomStorage};
pub use rsp::{
    OsTaskHeader, RspMemAddr, RspMemory, RspMemoryBank, RspMemoryError, TaskLog, M_AUDTASK,
    M_GFXTASK, OS_TASK_YIELDED, RSP_MEMORY_BANK_SIZE,
};
pub use save::{
    EepromError, EepromKind, EepromStatus, FileSaveStorage, InMemorySaveStorage, SaveStorage,
    SaveType, EEPROM_WRITE_CYCLES,
};
pub use si::{
    ContInput, PifModel, PortState, RumbleError, CONT_ABSENT, CONT_CARD_ON, CONT_TYPE_STANDARD,
};
pub use thread::{GameThread, Priority, Resume, RunToken, ThreadState, Yield, OS_PRIORITY_IDLE};
pub use timer::{TimerId, TimerWheel};
pub use trace::{
    DmaDirection, QueueOpKind, SwitchReason, TaskKind, ThreadId, TraceEvent, TraceKind, TraceLog,
};
pub use transfer_pak::{TransferPak, TransferPakError, TransferPakStatus, TRANSFER_PAK_BLOCK_SIZE};
pub use tv::{TvType, CPU_CLOCK_HZ};
pub use vi::{RetraceSchedule, ViState};
pub use voice::{VoiceData, VoiceError, VoiceUnit};
