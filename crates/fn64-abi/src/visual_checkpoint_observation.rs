//! Bounded, explicitly armed raw-DPC visual-checkpoint observations.
//!
//! The arm and queue are thread-local: callers must arm, execute, and drain
//! on the emulation thread that performs task-batch publication. Disarming
//! clears undrained receipts; arming does not. The queue traps at 4,096
//! receipts instead of dropping evidence silently.

use std::cell::{Cell, RefCell};

const MAX_VISUAL_CHECKPOINTS: usize = 4096;

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static COMPLETED: RefCell<Vec<RawDpcVisualCheckpointObservation>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawDpcVisualCheckpointObservationRefusal {
    /// The backend could not expose exact device target state.
    Target(fn64_render::RawDpcVisualTargetSnapshotRefusal),
    /// Supplied target state existed but failed checkpoint readiness.
    Checkpoint(fn64_render::RawDpcVisualCheckpointRefusal),
}

/// One completed task-batch member observation in publication order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawDpcVisualCheckpointObservation {
    /// Canonical identity shared by every member of the same task batch.
    pub task_batch_identity: [u8; 32],
    /// Zero-based member position within that task batch.
    pub member_ordinal: u32,
    /// Exact evidence or the named reason exact evidence was unavailable.
    pub result: Result<
        fn64_render::RawDpcVisualCheckpointEvidenceV1,
        RawDpcVisualCheckpointObservationRefusal,
    >,
}

/// Arm or disarm observation on the calling thread.
///
/// Disarming also clears every undrained receipt. Re-arming an already armed
/// queue retains its receipts, so callers should disarm first when beginning
/// an independent measurement.
pub fn set_raw_dpc_visual_checkpoint_observation_enabled(enabled: bool) {
    ENABLED.with(|cell| cell.set(enabled));
    if !enabled {
        COMPLETED.with(|cell| cell.borrow_mut().clear());
    }
}

/// Append all completed receipts from the calling thread and empty its queue.
pub fn drain_raw_dpc_visual_checkpoint_observations(
    destination: &mut Vec<RawDpcVisualCheckpointObservation>,
) {
    COMPLETED.with(|cell| destination.append(&mut cell.borrow_mut()));
}

pub(crate) fn enabled() -> bool {
    ENABLED.with(Cell::get)
}

pub(crate) fn record(observation: RawDpcVisualCheckpointObservation) {
    COMPLETED.with(|cell| {
        let mut completed = cell.borrow_mut();
        assert!(
            completed.len() < MAX_VISUAL_CHECKPOINTS,
            "raw-DPC visual-checkpoint observation capacity exceeded"
        );
        completed.push(observation);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_explicitly_armed_and_drained_once() {
        set_raw_dpc_visual_checkpoint_observation_enabled(false);
        assert!(!enabled());
        set_raw_dpc_visual_checkpoint_observation_enabled(true);
        let observation = RawDpcVisualCheckpointObservation {
            task_batch_identity: [3; 32],
            member_ordinal: 4,
            result: Err(RawDpcVisualCheckpointObservationRefusal::Target(
                fn64_render::RawDpcVisualTargetSnapshotRefusal::NoPublishedColorTarget,
            )),
        };
        record(observation);
        let mut drained = Vec::new();
        drain_raw_dpc_visual_checkpoint_observations(&mut drained);
        assert_eq!(drained, vec![observation]);
        drain_raw_dpc_visual_checkpoint_observations(&mut drained);
        assert_eq!(drained, vec![observation]);
        set_raw_dpc_visual_checkpoint_observation_enabled(false);
    }
}
