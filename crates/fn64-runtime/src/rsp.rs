//! RSP task-header capture + counting.
//!
//! ## Provenance
//!
//! `OSTask_t`'s field shape (`type`/`flags`/`ucode`/`ucode_data`/
//! `dram_stack`/`output_buff`/`data_ptr` etc) is the public libultra manual's
//! documented `OSTask` structure (RSP task-submission ABI, `os_task.h`);
//! `type` values `M_GFXTASK = 1`/`M_AUDTASK = 2` are the public libultra
//! manual's documented task-type constants. No GPL runtime RSP-dispatch
//! implementation was read -- `docs/DESIGN.md` section 1 explicitly flags
//! the gfx/audio task HANDOFF signature (which C function receives it) as
//! an open question for `fn64-rt64`; this module only models the ONE piece
//! of that boundary this milestone's evidence supports: recording what the
//! task header said, and (per the task's explicit scope) really invoking
//! the translated audio ucode function for `M_AUDTASK`, while acknowledging
//! (not executing) `M_GFXTASK`.
//!
//! ## Why this lives in `fn64-runtime`, not `fn64-rt64`
//!
//! `fn64-rt64` is reserved for actual RT64 (C++) interop (`docs/DESIGN.md`
//! section 1, reason 1: "the ONLY crate... permitted to contain C++"). Task
//! *counting*/*header capture* has no C++ dependency at all -- it's a pure
//! bookkeeping structure the executor's trace already wants a
//! `TaskKind`/`ucode` shape for (`trace.rs`'s `TaskSubmit`). Keeping it here
//! means `fn64-runtime` stays the single source of truth for "what tasks
//! were submitted," queryable by tests with no RT64/audio-ucode dependency
//! at all; only the actual audio-ucode FUNCTION POINTER CALL (which requires
//! linking the out-of-tree translated C) lives in the boot harness, per
//! `README.md`'s "no game content ships in this repo" rule -- this crate
//! only defines the callback SHAPE, never a real ucode body.

use crate::trace::TaskKind;

/// Public libultra manual's `OSTask_t` field shape (RSP task-submission
/// ABI). Only the fields this milestone's task-header logging/audio-call
/// path needs are modeled; `output_buff_size`/`yield_data_ptr`/
/// `yield_data_size` are declared in the real struct but unused by any
/// call site this milestone reaches, so omitted rather than guessed (per
/// `AGENTS.md`'s "don't model the shape speculatively").
#[derive(Copy, Clone, Debug, Default)]
pub struct OsTaskHeader {
    pub task_type: u32,
    pub flags: u32,
    pub ucode_boot: u32,
    pub ucode_boot_size: u32,
    pub ucode: u32,
    pub ucode_size: u32,
    pub ucode_data: u32,
    pub ucode_data_size: u32,
    pub dram_stack: u32,
    pub dram_stack_size: u32,
    pub output_buff: u32,
    pub data_ptr: u32,
    pub data_size: u32,
}

/// Public libultra manual's documented `OSTask.t.type` constants.
pub const M_GFXTASK: u32 = 1;
pub const M_AUDTASK: u32 = 2;

impl OsTaskHeader {
    pub fn kind(&self) -> Option<TaskKind> {
        match self.task_type {
            M_GFXTASK => Some(TaskKind::Graphics),
            M_AUDTASK => Some(TaskKind::Audio),
            _ => None,
        }
    }
}

/// Host-side counters/log for every RSP task this run submitted -- the
/// task's explicit "record the task header in the trace + count them"
/// requirement, kept separate from the shared `TraceLog` (which records the
/// lighter-weight `TaskSubmit{task_kind, ucode}` event for the A/B
/// comparator) so a harness/test can inspect full headers, not just the
/// trace-log summary.
#[derive(Default)]
pub struct TaskLog {
    submissions: Vec<OsTaskHeader>,
    gfx_count: u64,
    audio_count: u64,
}

impl TaskLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, header: OsTaskHeader) {
        match header.kind() {
            Some(TaskKind::Graphics) => self.gfx_count += 1,
            Some(TaskKind::Audio) => self.audio_count += 1,
            None => {}
        }
        self.submissions.push(header);
    }

    pub fn gfx_count(&self) -> u64 {
        self.gfx_count
    }

    pub fn audio_count(&self) -> u64 {
        self.audio_count
    }

    pub fn submissions(&self) -> &[OsTaskHeader] {
        &self.submissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_gfx_and_audio_separately() {
        let mut log = TaskLog::new();
        log.record(OsTaskHeader {
            task_type: M_GFXTASK,
            ..Default::default()
        });
        log.record(OsTaskHeader {
            task_type: M_AUDTASK,
            ..Default::default()
        });
        log.record(OsTaskHeader {
            task_type: M_AUDTASK,
            ..Default::default()
        });
        assert_eq!(log.gfx_count(), 1);
        assert_eq!(log.audio_count(), 2);
        assert_eq!(log.submissions().len(), 3);
    }

    #[test]
    fn unknown_task_type_is_recorded_but_not_counted() {
        let mut log = TaskLog::new();
        log.record(OsTaskHeader {
            task_type: 99,
            ..Default::default()
        });
        assert_eq!(log.gfx_count(), 0);
        assert_eq!(log.audio_count(), 0);
        assert_eq!(log.submissions().len(), 1);
    }
}
