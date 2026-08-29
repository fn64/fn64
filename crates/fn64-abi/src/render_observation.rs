//! Bounded, opt-in host observations of raw-DPC batch execution.
//!
//! These records explain where renderer work overlaps the guest thread. They
//! are not device events and never participate in emulated scheduling: every
//! timestamp is sampled only after the shell explicitly enables observation.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

const MAX_COMPLETED_BATCHES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchExecutionMode {
    Local,
    Worker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchJoinCause {
    ViVisibility,
    LaterGraphics,
    DmemDependency,
    LaterGraphicsAndDmemDependency,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderWorkerSpan {
    pub started_at: Instant,
    pub finished_at: Instant,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderBatchJoinSpan {
    pub cause: RenderBatchJoinCause,
    pub requested_at: Instant,
    pub returned_at: Instant,
}

#[derive(Clone, Debug)]
pub struct RenderBatchObservation {
    pub batch_id: u64,
    pub member_count: usize,
    pub dispatch_cycle: fn64_runtime::EmulatedInstant,
    pub completion_cycle: fn64_runtime::EmulatedInstant,
    pub dispatch_host_at: Instant,
    pub execution_mode: RenderBatchExecutionMode,
    pub worker: Option<RenderWorkerSpan>,
    pub join: Option<RenderBatchJoinSpan>,
    pub staged_writes: Duration,
    pub commit: Duration,
    pub copyback: Duration,
    pub publication: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderBatchIncompleteReason {
    ProcessExitBeforeCompletion,
}

#[derive(Clone, Debug)]
pub struct RenderBatchIncompleteObservation {
    pub batch_id: u64,
    pub member_count: usize,
    pub dispatch_cycle: fn64_runtime::EmulatedInstant,
    pub dispatch_host_at: Instant,
    pub reason: RenderBatchIncompleteReason,
}

#[derive(Debug)]
pub(crate) struct PendingRenderBatchObservation {
    batch_id: u64,
    member_count: usize,
    dispatch_cycle: fn64_runtime::EmulatedInstant,
    dispatch_host_at: Instant,
    worker: Option<RenderWorkerSpan>,
    join: Option<(RenderBatchJoinCause, Instant)>,
    staged_writes: Duration,
    commit: Duration,
    copyback: Duration,
    publication: Duration,
}

pub(crate) struct CompletedRenderBatchObservation {
    pending: PendingRenderBatchObservation,
    completion_cycle: fn64_runtime::EmulatedInstant,
}

thread_local! {
    static CONFIGURED: Cell<bool> = const { Cell::new(false) };
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static NEXT_BATCH_ID: Cell<u64> = const { Cell::new(0) };
    static COMPLETED: RefCell<Vec<RenderBatchObservation>> = const { RefCell::new(Vec::new()) };
}

/// Enable or disable host renderer observations before emulation begins.
///
/// The shell owns this diagnostic gate. It is deliberately an explicit call,
/// rather than another environment lookup inside the worker, so all records
/// share the shell trace's one host epoch and run identity.
pub fn set_render_batch_observation_enabled(enabled: bool) {
    CONFIGURED.with(|configured| {
        assert!(
            !configured.replace(true),
            "render batch observation may be configured only once per emulation thread"
        );
    });
    ENABLED.with(|cell| cell.set(enabled));
    NEXT_BATCH_ID.with(|cell| cell.set(0));
    COMPLETED.with(|cell| {
        assert!(
            cell.borrow().is_empty(),
            "render batch observation enabled with stale completed records"
        );
    });
}

pub fn drain_render_batch_observations(destination: &mut Vec<RenderBatchObservation>) {
    COMPLETED.with(|cell| destination.extend(cell.borrow_mut().drain(..)));
}

pub(crate) fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

pub(crate) fn begin(
    member_count: usize,
    dispatch_cycle: fn64_runtime::EmulatedInstant,
) -> Option<PendingRenderBatchObservation> {
    if !enabled() {
        return None;
    }
    assert!(
        member_count > 0,
        "render observation batch must have a member"
    );
    let batch_id = NEXT_BATCH_ID.with(|cell| {
        let id = cell.get();
        cell.set(
            id.checked_add(1)
                .expect("render observation batch ID overflow"),
        );
        id
    });
    Some(PendingRenderBatchObservation {
        batch_id,
        member_count,
        dispatch_cycle,
        dispatch_host_at: Instant::now(),
        worker: None,
        join: None,
        staged_writes: Duration::ZERO,
        commit: Duration::ZERO,
        copyback: Duration::ZERO,
        publication: Duration::ZERO,
    })
}

impl PendingRenderBatchObservation {
    pub(crate) fn set_worker_span(&mut self, span: Option<RenderWorkerSpan>) {
        assert!(self.worker.is_none(), "render worker span recorded twice");
        self.worker = span;
    }

    pub(crate) fn note_join(&mut self, cause: RenderBatchJoinCause) {
        assert!(self.join.is_none(), "render batch joined twice");
        self.join = Some((cause, Instant::now()));
    }

    pub(crate) fn phase_started(&self) -> Instant {
        Instant::now()
    }

    pub(crate) fn finish_staged_writes(&mut self, started: Instant) {
        add_elapsed(&mut self.staged_writes, started);
    }

    pub(crate) fn finish_commit(&mut self, started: Instant) {
        add_elapsed(&mut self.commit, started);
    }

    pub(crate) fn finish_copyback(&mut self, started: Instant) {
        add_elapsed(&mut self.copyback, started);
    }

    pub(crate) fn finish_publication(&mut self, started: Instant) {
        add_elapsed(&mut self.publication, started);
    }

    pub(crate) fn complete(
        self,
        completion_cycle: fn64_runtime::EmulatedInstant,
    ) -> CompletedRenderBatchObservation {
        CompletedRenderBatchObservation {
            pending: self,
            completion_cycle,
        }
    }

    pub(crate) fn into_incomplete(
        self,
        reason: RenderBatchIncompleteReason,
    ) -> RenderBatchIncompleteObservation {
        RenderBatchIncompleteObservation {
            batch_id: self.batch_id,
            member_count: self.member_count,
            dispatch_cycle: self.dispatch_cycle,
            dispatch_host_at: self.dispatch_host_at,
            reason,
        }
    }
}

impl CompletedRenderBatchObservation {
    pub(crate) fn seal(self, returned_at: Option<Instant>) -> RenderBatchObservation {
        let join = match (self.pending.join, returned_at) {
            (Some((cause, requested_at)), Some(returned_at)) => {
                assert!(
                    returned_at >= requested_at,
                    "render join returned before request"
                );
                Some(RenderBatchJoinSpan {
                    cause,
                    requested_at,
                    returned_at,
                })
            }
            (None, None) => None,
            _ => panic!("render observation join request and return must be complete together"),
        };
        RenderBatchObservation {
            batch_id: self.pending.batch_id,
            member_count: self.pending.member_count,
            dispatch_cycle: self.pending.dispatch_cycle,
            completion_cycle: self.completion_cycle,
            dispatch_host_at: self.pending.dispatch_host_at,
            execution_mode: if self.pending.worker.is_some() {
                RenderBatchExecutionMode::Worker
            } else {
                RenderBatchExecutionMode::Local
            },
            worker: self.pending.worker,
            join,
            staged_writes: self.pending.staged_writes,
            commit: self.pending.commit,
            copyback: self.pending.copyback,
            publication: self.pending.publication,
        }
    }
}

pub(crate) fn record_completed(observation: RenderBatchObservation) {
    COMPLETED.with(|cell| {
        let mut completed = cell.borrow_mut();
        assert!(
            completed.len() < MAX_COMPLETED_BATCHES,
            "render observation exceeded its {MAX_COMPLETED_BATCHES}-batch bound"
        );
        completed.push(observation);
    });
}

fn add_elapsed(total: &mut Duration, started: Instant) {
    *total = total
        .checked_add(started.elapsed())
        .expect("render observation phase duration overflow");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_observation_takes_no_record_and_enabled_record_is_drained() {
        ENABLED.with(|cell| cell.set(false));
        assert!(begin(2, fn64_runtime::EmulatedInstant::new(10)).is_none());
        let mut records = Vec::new();
        drain_render_batch_observations(&mut records);
        assert!(records.is_empty());

        ENABLED.with(|cell| cell.set(true));
        let mut pending = begin(3, fn64_runtime::EmulatedInstant::new(20)).unwrap();
        pending.note_join(RenderBatchJoinCause::ViVisibility);
        let completed = pending.complete(fn64_runtime::EmulatedInstant::new(30));
        record_completed(completed.seal(Some(Instant::now())));
        drain_render_batch_observations(&mut records);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].batch_id, 0);
        assert_eq!(records[0].member_count, 3);
        assert_eq!(records[0].dispatch_cycle.get(), 20);
        assert_eq!(records[0].completion_cycle.get(), 30);
        assert_eq!(
            records[0].join.as_ref().map(|join| join.cause),
            Some(RenderBatchJoinCause::ViVisibility)
        );
        records.clear();
        drain_render_batch_observations(&mut records);
        assert!(records.is_empty());
        ENABLED.with(|cell| cell.set(false));
    }

    fn completed_fixture(batch_id: u64) -> RenderBatchObservation {
        RenderBatchObservation {
            batch_id,
            member_count: 1,
            dispatch_cycle: fn64_runtime::EmulatedInstant::new(batch_id),
            completion_cycle: fn64_runtime::EmulatedInstant::new(batch_id + 1),
            dispatch_host_at: Instant::now(),
            execution_mode: RenderBatchExecutionMode::Local,
            worker: None,
            join: None,
            staged_writes: Duration::ZERO,
            commit: Duration::ZERO,
            copyback: Duration::ZERO,
            publication: Duration::ZERO,
        }
    }

    #[test]
    fn completed_bound_traps_before_growth_and_drain_restores_capacity() {
        COMPLETED.with(|cell| cell.borrow_mut().clear());
        for batch_id in 0..MAX_COMPLETED_BATCHES as u64 {
            record_completed(completed_fixture(batch_id));
        }
        let overflow = std::panic::catch_unwind(|| {
            record_completed(completed_fixture(MAX_COMPLETED_BATCHES as u64));
        });
        assert!(overflow.is_err());

        let mut records = Vec::new();
        drain_render_batch_observations(&mut records);
        assert_eq!(records.len(), MAX_COMPLETED_BATCHES);
        record_completed(completed_fixture(MAX_COMPLETED_BATCHES as u64));
        records.clear();
        drain_render_batch_observations(&mut records);
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn incomplete_observation_retains_dispatch_identity_without_completing_work() {
        ENABLED.with(|cell| cell.set(true));
        NEXT_BATCH_ID.with(|cell| cell.set(7));
        let pending = begin(3, fn64_runtime::EmulatedInstant::new(20)).unwrap();
        let incomplete =
            pending.into_incomplete(RenderBatchIncompleteReason::ProcessExitBeforeCompletion);
        assert_eq!(incomplete.batch_id, 7);
        assert_eq!(incomplete.member_count, 3);
        assert_eq!(incomplete.dispatch_cycle.get(), 20);
        assert_eq!(
            incomplete.reason,
            RenderBatchIncompleteReason::ProcessExitBeforeCompletion
        );
        ENABLED.with(|cell| cell.set(false));
        NEXT_BATCH_ID.with(|cell| cell.set(0));
    }
}
