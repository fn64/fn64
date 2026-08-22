//! Typed, opt-in scheduling mechanics for future measured DPC execution.
//!
//! This module deliberately does not define an N64 timing policy. A caller
//! supplies every synthetic or hardware-derived quantum boundary explicitly;
//! the state machine only enforces ownership, ordering, and acknowledgment.

use std::collections::VecDeque;

use crate::{Cycles, DpcSubmission, DpcSubmissionSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DpcTransactionId(u64);

impl DpcTransactionId {
    pub const fn from_submission(submission: DpcSubmission) -> Self {
        Self(submission.token)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DpcQuantumId(u64);

impl DpcQuantumId {
    pub const fn new(value: u64) -> Self {
        assert!(value != 0, "DPC quantum id must be nonzero");
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcCursor {
    source: DpcSubmissionSource,
    address: u32,
}

impl DpcCursor {
    pub fn new(source: DpcSubmissionSource, address: u32) -> Result<Self, DpcScheduleError> {
        let limit = source_limit(source);
        if !address.is_multiple_of(8) || address > limit {
            return Err(DpcScheduleError::InvalidCursor {
                source,
                address,
                limit,
            });
        }
        Ok(Self { source, address })
    }

    pub const fn source(self) -> DpcSubmissionSource {
        self.source
    }

    pub const fn address(self) -> u32 {
        self.address
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcQuantumPlan {
    pub at: Cycles,
    pub id: DpcQuantumId,
    pub start: DpcCursor,
    pub end: DpcCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcBackendQuantumRequest {
    pub transaction: DpcTransactionId,
    pub quantum: DpcQuantumId,
    pub start: DpcCursor,
    pub end: DpcCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcBackendQuantumAck {
    pub transaction: DpcTransactionId,
    pub quantum: DpcQuantumId,
    pub committed_through: DpcCursor,
    pub status: DpcBackendQuantumStatus,
}

/// Renderer disposition stripped of its backend-private continuation token.
/// The schedule owns whether another quantum exists, so this value is checked
/// together with the identity/cursor acknowledgment before either is committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcBackendQuantumStatus {
    Continue,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcScheduledPhase {
    Scheduled,
    AwaitingAck(DpcBackendQuantumRequest),
    Complete,
    Poisoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcAdvance {
    Reached {
        at: Cycles,
    },
    Blocked {
        at: Cycles,
        action: DpcBackendQuantumRequest,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcScheduleError {
    EmptyOrReversedSubmission {
        start: u32,
        end: u32,
    },
    InvalidCursor {
        source: DpcSubmissionSource,
        address: u32,
        limit: u32,
    },
    EmptyOrReversedQuantum {
        quantum: DpcQuantumId,
        start: u32,
        end: u32,
    },
    SourceMismatch {
        expected: DpcSubmissionSource,
        received: DpcSubmissionSource,
    },
    NonContiguousQuantum {
        quantum: DpcQuantumId,
        expected_start: u32,
        received_start: u32,
    },
    QuantumBeyondSubmission {
        quantum: DpcQuantumId,
        submission_end: u32,
        quantum_end: u32,
    },
    NonMonotonicDeadline {
        previous: Cycles,
        received: Cycles,
    },
    DuplicateQuantum(DpcQuantumId),
    IncompleteSchedule {
        submission_end: u32,
        scheduled_end: u32,
    },
    TimeWentBack {
        now: Cycles,
        requested: Cycles,
    },
    NoQuantumAwaitingAck,
    AckTransactionMismatch {
        expected: DpcTransactionId,
        received: DpcTransactionId,
    },
    AckQuantumMismatch {
        expected: DpcQuantumId,
        received: DpcQuantumId,
    },
    AckCursorMismatch {
        expected: DpcCursor,
        received: DpcCursor,
    },
    EarlyComplete {
        quantum: DpcQuantumId,
    },
    FinalContinue {
        quantum: DpcQuantumId,
    },
    Poisoned,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DpcScheduledExecution {
    transaction: DpcTransactionId,
    now: Cycles,
    cursor: DpcCursor,
    end: DpcCursor,
    phase: DpcScheduledPhase,
    remaining: VecDeque<DpcQuantumPlan>,
}

impl DpcScheduledExecution {
    /// Validate an explicit schedule without assigning it hardware authority.
    /// Each quantum must cover the submission exactly once and in order.
    pub fn new(
        submission: DpcSubmission,
        admitted_at: Cycles,
        plans: Vec<DpcQuantumPlan>,
    ) -> Result<Self, DpcScheduleError> {
        let start = DpcCursor::new(submission.source, submission.start)?;
        let end = DpcCursor::new(submission.source, submission.end)?;
        if start.address >= end.address {
            return Err(DpcScheduleError::EmptyOrReversedSubmission {
                start: start.address,
                end: end.address,
            });
        }
        let mut expected_start = start.address;
        let mut previous_at = admitted_at;
        let mut ids = Vec::with_capacity(plans.len());
        for plan in &plans {
            if plan.start.source != submission.source {
                return Err(DpcScheduleError::SourceMismatch {
                    expected: submission.source,
                    received: plan.start.source,
                });
            }
            if plan.end.source != submission.source {
                return Err(DpcScheduleError::SourceMismatch {
                    expected: submission.source,
                    received: plan.end.source,
                });
            }
            if plan.start.address >= plan.end.address {
                return Err(DpcScheduleError::EmptyOrReversedQuantum {
                    quantum: plan.id,
                    start: plan.start.address,
                    end: plan.end.address,
                });
            }
            if plan.start.address != expected_start {
                return Err(DpcScheduleError::NonContiguousQuantum {
                    quantum: plan.id,
                    expected_start,
                    received_start: plan.start.address,
                });
            }
            if plan.end.address > end.address {
                return Err(DpcScheduleError::QuantumBeyondSubmission {
                    quantum: plan.id,
                    submission_end: end.address,
                    quantum_end: plan.end.address,
                });
            }
            if plan.at < previous_at {
                return Err(DpcScheduleError::NonMonotonicDeadline {
                    previous: previous_at,
                    received: plan.at,
                });
            }
            if ids.contains(&plan.id) {
                return Err(DpcScheduleError::DuplicateQuantum(plan.id));
            }
            ids.push(plan.id);
            expected_start = plan.end.address;
            previous_at = plan.at;
        }
        if expected_start != end.address {
            return Err(DpcScheduleError::IncompleteSchedule {
                submission_end: end.address,
                scheduled_end: expected_start,
            });
        }
        Ok(Self {
            transaction: DpcTransactionId::from_submission(submission),
            now: admitted_at,
            cursor: start,
            end,
            phase: DpcScheduledPhase::Scheduled,
            remaining: plans.into(),
        })
    }

    pub const fn transaction(&self) -> DpcTransactionId {
        self.transaction
    }

    pub const fn now(&self) -> Cycles {
        self.now
    }

    pub const fn cursor(&self) -> DpcCursor {
        self.cursor
    }

    pub const fn phase(&self) -> DpcScheduledPhase {
        self.phase
    }

    /// Stop at the first due external-work boundary. Calling this again before
    /// acknowledgment returns the same action and cannot pass the barrier.
    pub fn advance_to(&mut self, requested: Cycles) -> Result<DpcAdvance, DpcScheduleError> {
        if requested < self.now {
            return Err(DpcScheduleError::TimeWentBack {
                now: self.now,
                requested,
            });
        }
        match self.phase {
            DpcScheduledPhase::AwaitingAck(action) => {
                return Ok(DpcAdvance::Blocked {
                    at: self.now,
                    action,
                });
            }
            DpcScheduledPhase::Poisoned => return Err(DpcScheduleError::Poisoned),
            DpcScheduledPhase::Complete => {
                self.now = requested;
                return Ok(DpcAdvance::Reached { at: requested });
            }
            DpcScheduledPhase::Scheduled => {}
        }
        if let Some(plan) = self.remaining.front().copied() {
            if plan.at <= requested {
                self.now = plan.at;
                let action = DpcBackendQuantumRequest {
                    transaction: self.transaction,
                    quantum: plan.id,
                    start: plan.start,
                    end: plan.end,
                };
                self.phase = DpcScheduledPhase::AwaitingAck(action);
                return Ok(DpcAdvance::Blocked {
                    at: self.now,
                    action,
                });
            }
        }
        self.now = requested;
        Ok(DpcAdvance::Reached { at: requested })
    }

    pub fn acknowledge(&mut self, ack: DpcBackendQuantumAck) -> Result<(), DpcScheduleError> {
        let DpcScheduledPhase::AwaitingAck(expected) = self.phase else {
            return Err(match self.phase {
                DpcScheduledPhase::Poisoned => DpcScheduleError::Poisoned,
                _ => DpcScheduleError::NoQuantumAwaitingAck,
            });
        };
        if ack.transaction != expected.transaction {
            return Err(DpcScheduleError::AckTransactionMismatch {
                expected: expected.transaction,
                received: ack.transaction,
            });
        }
        if ack.quantum != expected.quantum {
            return Err(DpcScheduleError::AckQuantumMismatch {
                expected: expected.quantum,
                received: ack.quantum,
            });
        }
        if ack.committed_through != expected.end {
            return Err(DpcScheduleError::AckCursorMismatch {
                expected: expected.end,
                received: ack.committed_through,
            });
        }
        let final_quantum = expected.end == self.end;
        match (final_quantum, ack.status) {
            (false, DpcBackendQuantumStatus::Complete) => {
                return Err(DpcScheduleError::EarlyComplete {
                    quantum: expected.quantum,
                });
            }
            (true, DpcBackendQuantumStatus::Continue) => {
                return Err(DpcScheduleError::FinalContinue {
                    quantum: expected.quantum,
                });
            }
            (false, DpcBackendQuantumStatus::Continue)
            | (true, DpcBackendQuantumStatus::Complete) => {}
        }
        let consumed = self
            .remaining
            .pop_front()
            .expect("awaiting DPC action must own a schedule entry");
        assert_eq!(consumed.id, expected.quantum);
        self.cursor = ack.committed_through;
        self.phase = if self.cursor == self.end {
            DpcScheduledPhase::Complete
        } else {
            DpcScheduledPhase::Scheduled
        };
        Ok(())
    }

    pub fn poison(&mut self) {
        self.phase = DpcScheduledPhase::Poisoned;
    }
}

const fn source_limit(source: DpcSubmissionSource) -> u32 {
    match source {
        DpcSubmissionSource::Rdram => 0x0100_0000,
        DpcSubmissionSource::Dmem => crate::RSP_MEMORY_BANK_SIZE as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor(address: u32) -> DpcCursor {
        DpcCursor::new(DpcSubmissionSource::Rdram, address).unwrap()
    }

    fn execution() -> DpcScheduledExecution {
        DpcScheduledExecution::new(
            DpcSubmission {
                token: 41,
                source: DpcSubmissionSource::Rdram,
                start: 0x100,
                end: 0x118,
            },
            Cycles::new(5),
            vec![
                DpcQuantumPlan {
                    at: Cycles::new(7),
                    id: DpcQuantumId::new(1),
                    start: cursor(0x100),
                    end: cursor(0x108),
                },
                DpcQuantumPlan {
                    at: Cycles::new(7),
                    id: DpcQuantumId::new(2),
                    start: cursor(0x108),
                    end: cursor(0x118),
                },
            ],
        )
        .unwrap()
    }

    #[test]
    fn action_barrier_requires_exact_ack_before_same_cycle_progress() {
        let mut execution = execution();
        let DpcAdvance::Blocked { at, action } = execution.advance_to(Cycles::new(20)).unwrap()
        else {
            panic!("first synthetic quantum was not due");
        };
        assert_eq!(at, Cycles::new(7));
        assert_eq!(
            execution.advance_to(Cycles::new(20)).unwrap(),
            DpcAdvance::Blocked { at, action }
        );
        assert_eq!(execution.cursor(), cursor(0x100));

        let wrong = DpcBackendQuantumAck {
            transaction: action.transaction,
            quantum: action.quantum,
            committed_through: cursor(0x110),
            status: DpcBackendQuantumStatus::Continue,
        };
        assert!(matches!(
            execution.acknowledge(wrong),
            Err(DpcScheduleError::AckCursorMismatch { .. })
        ));
        assert_eq!(execution.phase(), DpcScheduledPhase::AwaitingAck(action));

        execution
            .acknowledge(DpcBackendQuantumAck {
                transaction: action.transaction,
                quantum: action.quantum,
                committed_through: action.end,
                status: DpcBackendQuantumStatus::Continue,
            })
            .unwrap();
        let DpcAdvance::Blocked { at, action } = execution.advance_to(Cycles::new(20)).unwrap()
        else {
            panic!("second same-cycle quantum did not retain its barrier");
        };
        assert_eq!(at, Cycles::new(7));
        execution
            .acknowledge(DpcBackendQuantumAck {
                transaction: action.transaction,
                quantum: action.quantum,
                committed_through: action.end,
                status: DpcBackendQuantumStatus::Complete,
            })
            .unwrap();
        assert_eq!(execution.cursor(), cursor(0x118));
        assert_eq!(execution.phase(), DpcScheduledPhase::Complete);
        assert_eq!(
            execution.advance_to(Cycles::new(20)).unwrap(),
            DpcAdvance::Reached {
                at: Cycles::new(20)
            }
        );
    }

    #[test]
    fn schedule_validation_rejects_gaps_and_wrong_domains() {
        let submission = DpcSubmission {
            token: 1,
            source: DpcSubmissionSource::Rdram,
            start: 0x100,
            end: 0x110,
        };
        let gap = DpcQuantumPlan {
            at: Cycles::new(1),
            id: DpcQuantumId::new(1),
            start: cursor(0x108),
            end: cursor(0x110),
        };
        assert!(matches!(
            DpcScheduledExecution::new(submission, Cycles::new(0), vec![gap]),
            Err(DpcScheduleError::NonContiguousQuantum { .. })
        ));
        assert!(matches!(
            DpcCursor::new(DpcSubmissionSource::Dmem, 0x1008),
            Err(DpcScheduleError::InvalidCursor { .. })
        ));
        assert!(matches!(
            DpcScheduledExecution::new(submission, Cycles::new(0), vec![]),
            Err(DpcScheduleError::IncompleteSchedule {
                submission_end: 0x110,
                scheduled_end: 0x100,
            })
        ));
    }

    #[test]
    fn predeadline_progress_time_rollback_and_poison_are_explicit() {
        let mut execution = execution();
        assert_eq!(
            execution.advance_to(Cycles::new(6)).unwrap(),
            DpcAdvance::Reached { at: Cycles::new(6) }
        );
        assert_eq!(execution.cursor(), cursor(0x100));
        assert_eq!(
            execution.advance_to(Cycles::new(5)),
            Err(DpcScheduleError::TimeWentBack {
                now: Cycles::new(6),
                requested: Cycles::new(5),
            })
        );
        let DpcAdvance::Blocked { action, .. } = execution.advance_to(Cycles::new(7)).unwrap()
        else {
            panic!("first quantum must become due at its exact deadline")
        };
        execution.poison();
        assert_eq!(execution.phase(), DpcScheduledPhase::Poisoned);
        assert_eq!(
            execution.acknowledge(DpcBackendQuantumAck {
                transaction: action.transaction,
                quantum: action.quantum,
                committed_through: action.end,
                status: DpcBackendQuantumStatus::Continue,
            }),
            Err(DpcScheduleError::Poisoned)
        );
        assert_eq!(
            execution.advance_to(Cycles::new(7)),
            Err(DpcScheduleError::Poisoned)
        );
        assert_eq!(execution.cursor(), cursor(0x100));
    }

    #[test]
    fn completion_status_is_validated_before_schedule_mutation() {
        let mut execution = execution();
        let DpcAdvance::Blocked { action, .. } = execution.advance_to(Cycles::new(7)).unwrap()
        else {
            panic!("first quantum must be due")
        };
        assert_eq!(
            execution.acknowledge(DpcBackendQuantumAck {
                transaction: action.transaction,
                quantum: action.quantum,
                committed_through: action.end,
                status: DpcBackendQuantumStatus::Complete,
            }),
            Err(DpcScheduleError::EarlyComplete {
                quantum: action.quantum,
            })
        );
        assert_eq!(execution.phase(), DpcScheduledPhase::AwaitingAck(action));
        assert_eq!(execution.cursor(), cursor(0x100));

        execution
            .acknowledge(DpcBackendQuantumAck {
                transaction: action.transaction,
                quantum: action.quantum,
                committed_through: action.end,
                status: DpcBackendQuantumStatus::Continue,
            })
            .unwrap();
        let DpcAdvance::Blocked { action, .. } = execution.advance_to(Cycles::new(7)).unwrap()
        else {
            panic!("final quantum must be due")
        };
        assert_eq!(
            execution.acknowledge(DpcBackendQuantumAck {
                transaction: action.transaction,
                quantum: action.quantum,
                committed_through: action.end,
                status: DpcBackendQuantumStatus::Continue,
            }),
            Err(DpcScheduleError::FinalContinue {
                quantum: action.quantum,
            })
        );
        assert_eq!(execution.phase(), DpcScheduledPhase::AwaitingAck(action));
        assert_eq!(execution.cursor(), cursor(0x108));
    }

    #[test]
    fn stale_ack_does_not_mutate_the_awaiting_owner() {
        let mut execution = execution();
        let DpcAdvance::Blocked { action, .. } = execution.advance_to(Cycles::new(7)).unwrap()
        else {
            panic!("synthetic quantum was not due");
        };
        let stale = DpcBackendQuantumAck {
            transaction: DpcTransactionId(action.transaction.get() + 1),
            quantum: action.quantum,
            committed_through: action.end,
            status: DpcBackendQuantumStatus::Continue,
        };
        assert!(matches!(
            execution.acknowledge(stale),
            Err(DpcScheduleError::AckTransactionMismatch { .. })
        ));
        assert_eq!(execution.cursor(), cursor(0x100));
        assert_eq!(execution.phase(), DpcScheduledPhase::AwaitingAck(action));

        let wrong_quantum = DpcBackendQuantumAck {
            transaction: action.transaction,
            quantum: DpcQuantumId::new(action.quantum.get() + 1),
            committed_through: action.end,
            status: DpcBackendQuantumStatus::Continue,
        };
        assert!(matches!(
            execution.acknowledge(wrong_quantum),
            Err(DpcScheduleError::AckQuantumMismatch { .. })
        ));
        assert_eq!(execution.cursor(), cursor(0x100));

        let accepted = DpcBackendQuantumAck {
            transaction: action.transaction,
            quantum: action.quantum,
            committed_through: action.end,
            status: DpcBackendQuantumStatus::Continue,
        };
        execution.acknowledge(accepted).unwrap();
        assert_eq!(
            execution.acknowledge(accepted),
            Err(DpcScheduleError::NoQuantumAwaitingAck)
        );
    }
}
