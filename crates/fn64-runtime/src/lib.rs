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

pub mod executor;
pub mod mesgqueue;
pub mod mmio;
pub mod overlay;
pub mod peripherals;
pub mod rdram;
pub mod rom;
pub mod rsp;
pub mod save;
pub mod si;
pub mod thread;
pub mod timer;
pub mod trace;
pub mod vi;

pub use executor::{Executor, ExternalEvent, RecvMesgOutcome, SendMesgOutcome};
pub use mesgqueue::{Mesg, MesgQueue, RecvResult, SendResult};
pub use mmio::{
    is_mmio_offset, AiRegs, MmioSpace, AI_STATUS_BUSY, AI_STATUS_FULL, RDRAM_MMIO_WINDOW_END,
    RDRAM_MMIO_WINDOW_START,
};
pub use overlay::{FuncEntry, Section, SectionIndex, SectionRegistry};
pub use peripherals::Peripherals;
pub use rdram::{Rdram, RdramAddr};
pub use rom::{DmaCompletion, InMemoryRom, PiDma, RomStorage};
pub use rsp::{OsTaskHeader, TaskLog, M_AUDTASK, M_GFXTASK};
pub use save::{FileSaveStorage, InMemorySaveStorage, SaveStorage, SaveType};
pub use si::{ContInput, PifModel, PortState, CONT_ABSENT, CONT_CARD_ON, CONT_TYPE_STANDARD};
pub use thread::{GameThread, Priority, Resume, RunToken, ThreadState, Yield, OS_PRIORITY_IDLE};
pub use timer::{TimerId, TimerWheel};
pub use trace::{
    DmaDirection, QueueOpKind, SwitchReason, TaskKind, ThreadId, TraceEvent, TraceKind, TraceLog,
};
pub use vi::{RetraceSchedule, ViState};
