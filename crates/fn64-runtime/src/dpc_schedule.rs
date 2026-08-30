//! Typed, opt-in scheduling mechanics for future measured DPC execution.
//!
//! This module deliberately does not define an N64 timing policy. A caller
//! supplies every synthetic or hardware-derived quantum boundary explicitly;
//! the state machine only enforces ownership, ordering, and acknowledgment.

use std::{collections::VecDeque, sync::Arc};

use crate::{Cycles, DpcSubmission, DpcSubmissionSource, EmulatedInstant};

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

/// One externally settled stage of a scheduled DPC command range.
///
/// Command ingestion and guest-visible effects are distinct authorities. A
/// backend may prepare both, but neither cursor moves until the exact stage's
/// acknowledgment is accepted by [`DpcTwoStageExecution`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DpcExternalWorkStage {
    CommandIngested,
    EffectsVisible,
}

/// One caller-ordered barrier in an explicit two-stage schedule.
///
/// Vector order is the tie-breaker for barriers at the same instant. This
/// module validates that order but does not derive it or assign hardware
/// timing authority to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcTwoStageBarrierPlan {
    pub at: EmulatedInstant,
    pub transaction_stage: DpcExternalWorkStage,
    pub quantum: DpcQuantumId,
    pub start: DpcCursor,
    pub end: DpcCursor,
}

#[derive(Debug)]
struct DpcTwoStageReceiptIdentity {
    authority: Arc<()>,
    ordinal: u64,
    transaction: DpcTransactionId,
    quantum: DpcQuantumId,
    start: DpcCursor,
    end: DpcCursor,
}

/// Move-only authority to settle one exact command-ingestion barrier.
///
/// Private fields make [`DpcTwoStageExecution::advance_to`] the only minting
/// route. Accessors expose work identity without exposing a constructor.
#[derive(Debug)]
pub struct DpcCommandIngestedReceipt(DpcTwoStageReceiptIdentity);

impl DpcCommandIngestedReceipt {
    pub const fn transaction(&self) -> DpcTransactionId {
        self.0.transaction
    }

    pub const fn quantum(&self) -> DpcQuantumId {
        self.0.quantum
    }

    pub const fn start(&self) -> DpcCursor {
        self.0.start
    }

    pub const fn end(&self) -> DpcCursor {
        self.0.end
    }
}

/// Move-only authority to settle one exact effects-visible barrier.
///
/// This receipt has a distinct type from command ingestion, so a caller
/// cannot relabel a stage before returning it.
#[derive(Debug)]
pub struct DpcEffectsVisibleReceipt(DpcTwoStageReceiptIdentity);

impl DpcEffectsVisibleReceipt {
    pub const fn transaction(&self) -> DpcTransactionId {
        self.0.transaction
    }

    pub const fn quantum(&self) -> DpcQuantumId {
        self.0.quantum
    }

    pub const fn start(&self) -> DpcCursor {
        self.0.start
    }

    pub const fn end(&self) -> DpcCursor {
        self.0.end
    }
}

/// Stage-specific external-work ownership minted at one due barrier.
#[derive(Debug)]
pub enum DpcTwoStageWorkReceipt {
    CommandIngested(DpcCommandIngestedReceipt),
    EffectsVisible(DpcEffectsVisibleReceipt),
}

impl DpcTwoStageWorkReceipt {
    pub const fn transaction(&self) -> DpcTransactionId {
        match self {
            Self::CommandIngested(receipt) => receipt.transaction(),
            Self::EffectsVisible(receipt) => receipt.transaction(),
        }
    }

    pub const fn quantum(&self) -> DpcQuantumId {
        match self {
            Self::CommandIngested(receipt) => receipt.quantum(),
            Self::EffectsVisible(receipt) => receipt.quantum(),
        }
    }

    pub const fn start(&self) -> DpcCursor {
        match self {
            Self::CommandIngested(receipt) => receipt.start(),
            Self::EffectsVisible(receipt) => receipt.start(),
        }
    }

    pub const fn end(&self) -> DpcCursor {
        match self {
            Self::CommandIngested(receipt) => receipt.end(),
            Self::EffectsVisible(receipt) => receipt.end(),
        }
    }

    pub const fn stage(&self) -> DpcExternalWorkStage {
        match self {
            Self::CommandIngested(_) => DpcExternalWorkStage::CommandIngested,
            Self::EffectsVisible(_) => DpcExternalWorkStage::EffectsVisible,
        }
    }

    fn into_parts(self) -> (DpcExternalWorkStage, DpcTwoStageReceiptIdentity) {
        match self {
            Self::CommandIngested(DpcCommandIngestedReceipt(identity)) => {
                (DpcExternalWorkStage::CommandIngested, identity)
            }
            Self::EffectsVisible(DpcEffectsVisibleReceipt(identity)) => {
                (DpcExternalWorkStage::EffectsVisible, identity)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcTwoStagePhase {
    Scheduled,
    AwaitingCommandIngested,
    AwaitingEffectsVisible,
    Complete,
    Poisoned,
}

#[derive(Debug)]
pub enum DpcTwoStageAdvance {
    Reached {
        at: EmulatedInstant,
    },
    Blocked {
        at: EmulatedInstant,
        receipt: DpcTwoStageWorkReceipt,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcTwoStageScheduleError {
    EmptyOrReversedSubmission {
        start: u32,
        end: u32,
    },
    InvalidCursor {
        source: DpcSubmissionSource,
        address: u32,
        limit: u32,
    },
    SourceMismatch {
        expected: DpcSubmissionSource,
        received: DpcSubmissionSource,
    },
    EmptyOrReversedBarrier {
        quantum: DpcQuantumId,
        transaction_stage: DpcExternalWorkStage,
        start: u32,
        end: u32,
    },
    NonMonotonicBarrier {
        previous: EmulatedInstant,
        received: EmulatedInstant,
    },
    NonContiguousStage {
        quantum: DpcQuantumId,
        transaction_stage: DpcExternalWorkStage,
        expected_start: u32,
        received_start: u32,
    },
    BarrierBeyondSubmission {
        quantum: DpcQuantumId,
        transaction_stage: DpcExternalWorkStage,
        submission_end: u32,
        barrier_end: u32,
    },
    DuplicateStage {
        quantum: DpcQuantumId,
        transaction_stage: DpcExternalWorkStage,
    },
    EffectsBeforeCommand(DpcQuantumId),
    EffectsRangeMismatch {
        quantum: DpcQuantumId,
        command_start: DpcCursor,
        command_end: DpcCursor,
        effects_start: DpcCursor,
        effects_end: DpcCursor,
    },
    IncompleteStage {
        transaction_stage: DpcExternalWorkStage,
        submission_end: u32,
        scheduled_end: u32,
    },
    MissingEffectsStage(DpcQuantumId),
    TimeWentBack {
        now: EmulatedInstant,
        requested: EmulatedInstant,
    },
    ReceiptOutstanding,
    NoBarrierAwaitingReceipt,
    ReceiptAuthorityMismatch,
    ReceiptOrdinalMismatch {
        expected: u64,
        received: u64,
    },
    ReceiptTransactionMismatch {
        expected: DpcTransactionId,
        received: DpcTransactionId,
    },
    ReceiptQuantumMismatch {
        expected: DpcQuantumId,
        received: DpcQuantumId,
    },
    ReceiptStageMismatch {
        expected: DpcExternalWorkStage,
        received: DpcExternalWorkStage,
    },
    ReceiptCursorMismatch {
        expected: DpcCursor,
        received: DpcCursor,
    },
    Poisoned,
}

/// Runtime-owned validation and settlement for an explicit two-stage DPC
/// schedule. It contains no deadline derivation or production admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DpcTwoStagePendingBarrier {
    plan: DpcTwoStageBarrierPlan,
    receipt_ordinal: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DpcTwoStageInternalPhase {
    Scheduled,
    AwaitingReceipt(DpcTwoStagePendingBarrier),
    Complete,
    Poisoned,
}

#[derive(Debug)]
pub struct DpcTwoStageExecution {
    authority: Arc<()>,
    transaction: DpcTransactionId,
    now: EmulatedInstant,
    ingested_through: DpcCursor,
    visible_through: DpcCursor,
    end: DpcCursor,
    phase: DpcTwoStageInternalPhase,
    next_receipt_ordinal: u64,
    remaining: VecDeque<DpcTwoStageBarrierPlan>,
}

impl DpcTwoStageExecution {
    pub fn new(
        submission: DpcSubmission,
        admitted_at: EmulatedInstant,
        plans: Vec<DpcTwoStageBarrierPlan>,
    ) -> Result<Self, DpcTwoStageScheduleError> {
        let checked_cursor = |address| {
            DpcCursor::new(submission.source, address).map_err(|error| match error {
                DpcScheduleError::InvalidCursor {
                    source,
                    address,
                    limit,
                } => DpcTwoStageScheduleError::InvalidCursor {
                    source,
                    address,
                    limit,
                },
                _ => unreachable!("DpcCursor::new returns only InvalidCursor"),
            })
        };
        let start = checked_cursor(submission.start)?;
        let end = checked_cursor(submission.end)?;
        if start.address >= end.address {
            return Err(DpcTwoStageScheduleError::EmptyOrReversedSubmission {
                start: start.address,
                end: end.address,
            });
        }

        let mut ingested_end = start.address;
        let mut visible_end = start.address;
        let mut previous_at = admitted_at;
        let mut commands = Vec::with_capacity(plans.len() / 2);
        let mut seen = Vec::with_capacity(plans.len());
        for plan in &plans {
            for cursor in [plan.start, plan.end] {
                if cursor.source != submission.source {
                    return Err(DpcTwoStageScheduleError::SourceMismatch {
                        expected: submission.source,
                        received: cursor.source,
                    });
                }
            }
            if plan.start.address >= plan.end.address {
                return Err(DpcTwoStageScheduleError::EmptyOrReversedBarrier {
                    quantum: plan.quantum,
                    transaction_stage: plan.transaction_stage,
                    start: plan.start.address,
                    end: plan.end.address,
                });
            }
            if plan.at < previous_at {
                return Err(DpcTwoStageScheduleError::NonMonotonicBarrier {
                    previous: previous_at,
                    received: plan.at,
                });
            }
            if seen.contains(&(plan.quantum, plan.transaction_stage)) {
                return Err(DpcTwoStageScheduleError::DuplicateStage {
                    quantum: plan.quantum,
                    transaction_stage: plan.transaction_stage,
                });
            }
            seen.push((plan.quantum, plan.transaction_stage));

            let expected_start = match plan.transaction_stage {
                DpcExternalWorkStage::CommandIngested => ingested_end,
                DpcExternalWorkStage::EffectsVisible => visible_end,
            };
            if plan.start.address != expected_start {
                return Err(DpcTwoStageScheduleError::NonContiguousStage {
                    quantum: plan.quantum,
                    transaction_stage: plan.transaction_stage,
                    expected_start,
                    received_start: plan.start.address,
                });
            }
            if plan.end.address > end.address {
                return Err(DpcTwoStageScheduleError::BarrierBeyondSubmission {
                    quantum: plan.quantum,
                    transaction_stage: plan.transaction_stage,
                    submission_end: end.address,
                    barrier_end: plan.end.address,
                });
            }

            match plan.transaction_stage {
                DpcExternalWorkStage::CommandIngested => {
                    commands.push((plan.quantum, plan.start, plan.end));
                    ingested_end = plan.end.address;
                }
                DpcExternalWorkStage::EffectsVisible => {
                    let Some((_, command_start, command_end)) = commands
                        .iter()
                        .find(|(quantum, _, _)| *quantum == plan.quantum)
                        .copied()
                    else {
                        return Err(DpcTwoStageScheduleError::EffectsBeforeCommand(plan.quantum));
                    };
                    if plan.start != command_start || plan.end != command_end {
                        return Err(DpcTwoStageScheduleError::EffectsRangeMismatch {
                            quantum: plan.quantum,
                            command_start,
                            command_end,
                            effects_start: plan.start,
                            effects_end: plan.end,
                        });
                    }
                    visible_end = plan.end.address;
                }
            }
            previous_at = plan.at;
        }

        for (quantum, _, _) in &commands {
            if !seen.contains(&(*quantum, DpcExternalWorkStage::EffectsVisible)) {
                return Err(DpcTwoStageScheduleError::MissingEffectsStage(*quantum));
            }
        }
        for (transaction_stage, scheduled_end) in [
            (DpcExternalWorkStage::CommandIngested, ingested_end),
            (DpcExternalWorkStage::EffectsVisible, visible_end),
        ] {
            if scheduled_end != end.address {
                return Err(DpcTwoStageScheduleError::IncompleteStage {
                    transaction_stage,
                    submission_end: end.address,
                    scheduled_end,
                });
            }
        }

        Ok(Self {
            authority: Arc::new(()),
            transaction: DpcTransactionId::from_submission(submission),
            now: admitted_at,
            ingested_through: start,
            visible_through: start,
            end,
            phase: DpcTwoStageInternalPhase::Scheduled,
            next_receipt_ordinal: 1,
            remaining: plans.into(),
        })
    }

    pub const fn transaction(&self) -> DpcTransactionId {
        self.transaction
    }

    pub const fn now(&self) -> EmulatedInstant {
        self.now
    }

    pub const fn ingested_through(&self) -> DpcCursor {
        self.ingested_through
    }

    pub const fn visible_through(&self) -> DpcCursor {
        self.visible_through
    }

    pub const fn phase(&self) -> DpcTwoStagePhase {
        match self.phase {
            DpcTwoStageInternalPhase::Scheduled => DpcTwoStagePhase::Scheduled,
            DpcTwoStageInternalPhase::AwaitingReceipt(pending) => match pending
                .plan
                .transaction_stage
            {
                DpcExternalWorkStage::CommandIngested => DpcTwoStagePhase::AwaitingCommandIngested,
                DpcExternalWorkStage::EffectsVisible => DpcTwoStagePhase::AwaitingEffectsVisible,
            },
            DpcTwoStageInternalPhase::Complete => DpcTwoStagePhase::Complete,
            DpcTwoStageInternalPhase::Poisoned => DpcTwoStagePhase::Poisoned,
        }
    }

    pub fn advance_to(
        &mut self,
        requested: EmulatedInstant,
    ) -> Result<DpcTwoStageAdvance, DpcTwoStageScheduleError> {
        if requested < self.now {
            return Err(DpcTwoStageScheduleError::TimeWentBack {
                now: self.now,
                requested,
            });
        }
        match self.phase {
            DpcTwoStageInternalPhase::AwaitingReceipt(_) => {
                return Err(DpcTwoStageScheduleError::ReceiptOutstanding);
            }
            DpcTwoStageInternalPhase::Poisoned => {
                return Err(DpcTwoStageScheduleError::Poisoned);
            }
            DpcTwoStageInternalPhase::Complete => {
                self.now = requested;
                return Ok(DpcTwoStageAdvance::Reached { at: requested });
            }
            DpcTwoStageInternalPhase::Scheduled => {}
        }
        if let Some(plan) = self.remaining.front().copied() {
            if plan.at <= requested {
                self.now = plan.at;
                let receipt_ordinal = self.next_receipt_ordinal;
                self.next_receipt_ordinal = self
                    .next_receipt_ordinal
                    .checked_add(1)
                    .expect("two-stage DPC receipt ordinal overflow");
                let identity = DpcTwoStageReceiptIdentity {
                    authority: Arc::clone(&self.authority),
                    ordinal: receipt_ordinal,
                    transaction: self.transaction,
                    quantum: plan.quantum,
                    start: plan.start,
                    end: plan.end,
                };
                let receipt = match plan.transaction_stage {
                    DpcExternalWorkStage::CommandIngested => {
                        DpcTwoStageWorkReceipt::CommandIngested(DpcCommandIngestedReceipt(identity))
                    }
                    DpcExternalWorkStage::EffectsVisible => {
                        DpcTwoStageWorkReceipt::EffectsVisible(DpcEffectsVisibleReceipt(identity))
                    }
                };
                self.phase = DpcTwoStageInternalPhase::AwaitingReceipt(DpcTwoStagePendingBarrier {
                    plan,
                    receipt_ordinal,
                });
                return Ok(DpcTwoStageAdvance::Blocked {
                    at: self.now,
                    receipt,
                });
            }
        }
        self.now = requested;
        Ok(DpcTwoStageAdvance::Reached { at: requested })
    }

    pub fn commit(
        &mut self,
        receipt: DpcTwoStageWorkReceipt,
    ) -> Result<(), DpcTwoStageScheduleError> {
        let (stage, identity) = receipt.into_parts();
        let expected = self.validate_receipt(stage, &identity)?;
        let consumed = self
            .remaining
            .pop_front()
            .expect("awaiting two-stage DPC action must own a barrier entry");
        assert_eq!(consumed, expected.plan);
        match stage {
            DpcExternalWorkStage::CommandIngested => {
                self.ingested_through = identity.end;
            }
            DpcExternalWorkStage::EffectsVisible => {
                self.visible_through = identity.end;
            }
        }
        self.phase = if self.remaining.is_empty() {
            assert_eq!(self.ingested_through, self.end);
            assert_eq!(self.visible_through, self.end);
            DpcTwoStageInternalPhase::Complete
        } else {
            DpcTwoStageInternalPhase::Scheduled
        };
        Ok(())
    }

    /// Reject exact external work before it becomes authoritative. Unlike
    /// [`Self::commit`], this path accepts no cursor or disposition value a
    /// caller could mislabel as committed.
    pub fn fail(
        &mut self,
        receipt: DpcTwoStageWorkReceipt,
    ) -> Result<(), DpcTwoStageScheduleError> {
        let (stage, identity) = receipt.into_parts();
        self.validate_receipt(stage, &identity)?;
        self.phase = DpcTwoStageInternalPhase::Poisoned;
        Ok(())
    }

    fn validate_receipt(
        &self,
        received_stage: DpcExternalWorkStage,
        receipt: &DpcTwoStageReceiptIdentity,
    ) -> Result<DpcTwoStagePendingBarrier, DpcTwoStageScheduleError> {
        let DpcTwoStageInternalPhase::AwaitingReceipt(expected) = self.phase else {
            return Err(match self.phase {
                DpcTwoStageInternalPhase::Poisoned => DpcTwoStageScheduleError::Poisoned,
                _ => DpcTwoStageScheduleError::NoBarrierAwaitingReceipt,
            });
        };
        if !Arc::ptr_eq(&receipt.authority, &self.authority) {
            return Err(DpcTwoStageScheduleError::ReceiptAuthorityMismatch);
        }
        if receipt.ordinal != expected.receipt_ordinal {
            return Err(DpcTwoStageScheduleError::ReceiptOrdinalMismatch {
                expected: expected.receipt_ordinal,
                received: receipt.ordinal,
            });
        }
        if receipt.transaction != self.transaction {
            return Err(DpcTwoStageScheduleError::ReceiptTransactionMismatch {
                expected: self.transaction,
                received: receipt.transaction,
            });
        }
        if receipt.quantum != expected.plan.quantum {
            return Err(DpcTwoStageScheduleError::ReceiptQuantumMismatch {
                expected: expected.plan.quantum,
                received: receipt.quantum,
            });
        }
        if received_stage != expected.plan.transaction_stage {
            return Err(DpcTwoStageScheduleError::ReceiptStageMismatch {
                expected: expected.plan.transaction_stage,
                received: received_stage,
            });
        }
        for (expected_cursor, received_cursor) in [
            (expected.plan.start, receipt.start),
            (expected.plan.end, receipt.end),
        ] {
            if received_cursor != expected_cursor {
                return Err(DpcTwoStageScheduleError::ReceiptCursorMismatch {
                    expected: expected_cursor,
                    received: received_cursor,
                });
            }
        }
        Ok(expected)
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

    fn staged_barrier(
        at: u64,
        quantum: u64,
        transaction_stage: DpcExternalWorkStage,
        start: u32,
        end: u32,
    ) -> DpcTwoStageBarrierPlan {
        DpcTwoStageBarrierPlan {
            at: EmulatedInstant::new(at),
            transaction_stage,
            quantum: DpcQuantumId::new(quantum),
            start: cursor(start),
            end: cursor(end),
        }
    }

    fn two_stage_execution() -> DpcTwoStageExecution {
        DpcTwoStageExecution::new(
            DpcSubmission {
                token: 73,
                source: DpcSubmissionSource::Rdram,
                start: 0x100,
                end: 0x118,
            },
            EmulatedInstant::new(5),
            vec![
                staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x100, 0x108),
                staged_barrier(7, 2, DpcExternalWorkStage::CommandIngested, 0x108, 0x118),
                staged_barrier(7, 1, DpcExternalWorkStage::EffectsVisible, 0x100, 0x108),
                staged_barrier(7, 2, DpcExternalWorkStage::EffectsVisible, 0x108, 0x118),
            ],
        )
        .unwrap()
    }

    fn due_receipt(
        execution: &mut DpcTwoStageExecution,
        requested: u64,
    ) -> (EmulatedInstant, DpcTwoStageWorkReceipt) {
        let DpcTwoStageAdvance::Blocked { at, receipt } = execution
            .advance_to(EmulatedInstant::new(requested))
            .unwrap()
        else {
            panic!("two-stage barrier was not due");
        };
        (at, receipt)
    }

    fn duplicate_receipt(receipt: &DpcTwoStageWorkReceipt) -> DpcTwoStageWorkReceipt {
        let copy_identity = |identity: &DpcTwoStageReceiptIdentity| DpcTwoStageReceiptIdentity {
            authority: Arc::clone(&identity.authority),
            ordinal: identity.ordinal,
            transaction: identity.transaction,
            quantum: identity.quantum,
            start: identity.start,
            end: identity.end,
        };
        match receipt {
            DpcTwoStageWorkReceipt::CommandIngested(DpcCommandIngestedReceipt(identity)) => {
                DpcTwoStageWorkReceipt::CommandIngested(DpcCommandIngestedReceipt(copy_identity(
                    identity,
                )))
            }
            DpcTwoStageWorkReceipt::EffectsVisible(DpcEffectsVisibleReceipt(identity)) => {
                DpcTwoStageWorkReceipt::EffectsVisible(DpcEffectsVisibleReceipt(copy_identity(
                    identity,
                )))
            }
        }
    }

    fn receipt_identity_mut(
        receipt: &mut DpcTwoStageWorkReceipt,
    ) -> &mut DpcTwoStageReceiptIdentity {
        match receipt {
            DpcTwoStageWorkReceipt::CommandIngested(DpcCommandIngestedReceipt(identity)) => {
                identity
            }
            DpcTwoStageWorkReceipt::EffectsVisible(DpcEffectsVisibleReceipt(identity)) => identity,
        }
    }

    #[test]
    fn stage_receipts_are_move_only_and_privately_stage_typed() {
        static_assertions::assert_not_impl_any!(DpcCommandIngestedReceipt: Clone, Copy);
        static_assertions::assert_not_impl_any!(DpcEffectsVisibleReceipt: Clone, Copy);
        static_assertions::assert_not_impl_any!(DpcTwoStageWorkReceipt: Clone, Copy);
        static_assertions::assert_not_impl_any!(DpcTwoStageExecution: Clone, Copy);

        let mut execution = two_stage_execution();
        let (_, receipt) = due_receipt(&mut execution, 7);
        assert!(matches!(
            receipt,
            DpcTwoStageWorkReceipt::CommandIngested(_)
        ));
    }

    #[test]
    fn two_stage_same_cycle_barriers_retain_explicit_input_order() {
        let mut execution = two_stage_execution();
        let expected = [
            (DpcQuantumId::new(1), DpcExternalWorkStage::CommandIngested),
            (DpcQuantumId::new(2), DpcExternalWorkStage::CommandIngested),
            (DpcQuantumId::new(1), DpcExternalWorkStage::EffectsVisible),
            (DpcQuantumId::new(2), DpcExternalWorkStage::EffectsVisible),
        ];

        for (index, (quantum, stage)) in expected.into_iter().enumerate() {
            let (at, receipt) = due_receipt(&mut execution, 20);
            assert_eq!(at, EmulatedInstant::new(7), "barrier {index}");
            assert_eq!(receipt.quantum(), quantum);
            assert_eq!(receipt.stage(), stage);
            assert!(matches!(
                execution.advance_to(EmulatedInstant::new(20)),
                Err(DpcTwoStageScheduleError::ReceiptOutstanding)
            ));
            execution.commit(receipt).unwrap();
        }

        assert_eq!(execution.ingested_through(), cursor(0x118));
        assert_eq!(execution.visible_through(), cursor(0x118));
        assert_eq!(execution.phase(), DpcTwoStagePhase::Complete);
        assert!(matches!(
            execution.advance_to(EmulatedInstant::new(20)),
            Ok(DpcTwoStageAdvance::Reached { at }) if at == EmulatedInstant::new(20)
        ));
    }

    #[test]
    fn distinct_cycle_barriers_preserve_caller_order_with_ingestion_overlap() {
        let submission = DpcSubmission {
            token: 81,
            source: DpcSubmissionSource::Rdram,
            start: 0x100,
            end: 0x110,
        };
        let mut execution = DpcTwoStageExecution::new(
            submission,
            EmulatedInstant::new(5),
            vec![
                staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x100, 0x108),
                staged_barrier(8, 2, DpcExternalWorkStage::CommandIngested, 0x108, 0x110),
                staged_barrier(11, 1, DpcExternalWorkStage::EffectsVisible, 0x100, 0x108),
                staged_barrier(13, 2, DpcExternalWorkStage::EffectsVisible, 0x108, 0x110),
            ],
        )
        .unwrap();
        let expected = [
            (7, 1, DpcExternalWorkStage::CommandIngested),
            (8, 2, DpcExternalWorkStage::CommandIngested),
            (11, 1, DpcExternalWorkStage::EffectsVisible),
            (13, 2, DpcExternalWorkStage::EffectsVisible),
        ];
        for (at, quantum, stage) in expected {
            let (received_at, receipt) = due_receipt(&mut execution, 20);
            assert_eq!(received_at, EmulatedInstant::new(at));
            assert_eq!(receipt.quantum(), DpcQuantumId::new(quantum));
            assert_eq!(receipt.stage(), stage);
            execution.commit(receipt).unwrap();
        }
    }

    #[test]
    fn failure_at_first_ingestion_poison_preserves_both_origin_cursors() {
        let mut execution = two_stage_execution();
        let (_, receipt) = due_receipt(&mut execution, 7);
        execution.fail(receipt).unwrap();
        assert_eq!(execution.phase(), DpcTwoStagePhase::Poisoned);
        assert_eq!(execution.ingested_through(), cursor(0x100));
        assert_eq!(execution.visible_through(), cursor(0x100));
        assert!(matches!(
            execution.advance_to(EmulatedInstant::new(7)),
            Err(DpcTwoStageScheduleError::Poisoned)
        ));
    }

    #[test]
    fn failure_after_visible_prefix_preserves_only_committed_prefix() {
        let mut execution = two_stage_execution();
        for _ in 0..3 {
            let (_, receipt) = due_receipt(&mut execution, 7);
            execution.commit(receipt).unwrap();
        }
        assert_eq!(execution.ingested_through(), cursor(0x118));
        assert_eq!(execution.visible_through(), cursor(0x108));

        let (_, receipt) = due_receipt(&mut execution, 7);
        execution.fail(receipt).unwrap();
        assert_eq!(execution.phase(), DpcTwoStagePhase::Poisoned);
        assert_eq!(execution.ingested_through(), cursor(0x118));
        assert_eq!(execution.visible_through(), cursor(0x108));
    }

    #[test]
    fn wrong_and_stale_receipts_cannot_mutate_the_awaiting_owner() {
        let mut execution = two_stage_execution();
        let (_, receipt) = due_receipt(&mut execution, 7);
        let original_ingested = execution.ingested_through();
        let original_visible = execution.visible_through();

        let mut wrong_transaction = duplicate_receipt(&receipt);
        receipt_identity_mut(&mut wrong_transaction).transaction =
            DpcTransactionId(receipt.transaction().get() + 1);
        assert!(matches!(
            execution.commit(wrong_transaction),
            Err(DpcTwoStageScheduleError::ReceiptTransactionMismatch { .. })
        ));

        let mut wrong_quantum = duplicate_receipt(&receipt);
        receipt_identity_mut(&mut wrong_quantum).quantum =
            DpcQuantumId::new(receipt.quantum().get() + 1);
        assert!(matches!(
            execution.commit(wrong_quantum),
            Err(DpcTwoStageScheduleError::ReceiptQuantumMismatch { .. })
        ));

        let mut wrong_cursor = duplicate_receipt(&receipt);
        receipt_identity_mut(&mut wrong_cursor).end = cursor(0x110);
        assert!(matches!(
            execution.commit(wrong_cursor),
            Err(DpcTwoStageScheduleError::ReceiptCursorMismatch { .. })
        ));

        let duplicate = duplicate_receipt(&receipt);
        let (_, foreign) = due_receipt(&mut two_stage_execution(), 7);
        assert!(matches!(
            execution.commit(foreign),
            Err(DpcTwoStageScheduleError::ReceiptAuthorityMismatch)
        ));
        assert_eq!(execution.ingested_through(), original_ingested);
        assert_eq!(execution.visible_through(), original_visible);
        execution.commit(receipt).unwrap();

        let (_, next) = due_receipt(&mut execution, 7);
        assert!(matches!(
            execution.commit(duplicate),
            Err(DpcTwoStageScheduleError::ReceiptOrdinalMismatch { .. })
        ));
        execution.commit(next).unwrap();
    }

    #[test]
    fn invalid_failure_receipts_leave_the_valid_owner_awaiting_and_unmodified() {
        let mut execution = two_stage_execution();
        let (_, receipt) = due_receipt(&mut execution, 7);
        let stale = duplicate_receipt(&receipt);

        let (_, foreign) = due_receipt(&mut two_stage_execution(), 7);
        assert!(matches!(
            execution.fail(foreign),
            Err(DpcTwoStageScheduleError::ReceiptAuthorityMismatch)
        ));
        let mut wrong_transaction = duplicate_receipt(&receipt);
        receipt_identity_mut(&mut wrong_transaction).transaction =
            DpcTransactionId(receipt.transaction().get() + 1);
        assert!(matches!(
            execution.fail(wrong_transaction),
            Err(DpcTwoStageScheduleError::ReceiptTransactionMismatch { .. })
        ));
        assert_eq!(execution.phase(), DpcTwoStagePhase::AwaitingCommandIngested);
        assert_eq!(execution.ingested_through(), cursor(0x100));
        assert_eq!(execution.visible_through(), cursor(0x100));
        execution.commit(receipt).unwrap();

        let (_, next) = due_receipt(&mut execution, 7);
        assert!(matches!(
            execution.fail(stale),
            Err(DpcTwoStageScheduleError::ReceiptOrdinalMismatch { .. })
        ));
        assert_eq!(execution.phase(), DpcTwoStagePhase::AwaitingCommandIngested);
        assert_eq!(execution.ingested_through(), cursor(0x108));
        assert_eq!(execution.visible_through(), cursor(0x100));
        execution.commit(next).unwrap();
    }

    #[test]
    fn stage_relabel_duplicate_and_post_complete_receipts_are_rejected() {
        let mut execution = two_stage_execution();
        let (_, receipt) = due_receipt(&mut execution, 7);
        let identity = match duplicate_receipt(&receipt) {
            DpcTwoStageWorkReceipt::CommandIngested(DpcCommandIngestedReceipt(identity)) => {
                identity
            }
            DpcTwoStageWorkReceipt::EffectsVisible(_) => panic!("first barrier changed stage"),
        };
        let relabeled = DpcTwoStageWorkReceipt::EffectsVisible(DpcEffectsVisibleReceipt(identity));
        assert!(matches!(
            execution.commit(relabeled),
            Err(DpcTwoStageScheduleError::ReceiptStageMismatch { .. })
        ));
        execution.commit(receipt).unwrap();

        let mut final_duplicate = None;
        while execution.phase() != DpcTwoStagePhase::Complete {
            let (_, receipt) = due_receipt(&mut execution, 7);
            final_duplicate = Some(duplicate_receipt(&receipt));
            execution.commit(receipt).unwrap();
        }
        assert!(matches!(
            execution.commit(final_duplicate.unwrap()),
            Err(DpcTwoStageScheduleError::NoBarrierAwaitingReceipt)
        ));
    }

    #[test]
    fn two_stage_constructor_rejects_source_domain_gap_duplicate_and_range_errors() {
        let submission = DpcSubmission {
            token: 9,
            source: DpcSubmissionSource::Rdram,
            start: 0x100,
            end: 0x110,
        };
        let dmem_cursor = DpcCursor::new(DpcSubmissionSource::Dmem, 0x100).unwrap();
        let wrong_source = DpcTwoStageBarrierPlan {
            at: EmulatedInstant::new(7),
            transaction_stage: DpcExternalWorkStage::CommandIngested,
            quantum: DpcQuantumId::new(1),
            start: dmem_cursor,
            end: DpcCursor::new(DpcSubmissionSource::Dmem, 0x108).unwrap(),
        };
        assert!(matches!(
            DpcTwoStageExecution::new(submission, EmulatedInstant::new(5), vec![wrong_source]),
            Err(DpcTwoStageScheduleError::SourceMismatch { .. })
        ));
        assert!(matches!(
            DpcTwoStageExecution::new(
                DpcSubmission {
                    end: 0x0100_0008,
                    ..submission
                },
                EmulatedInstant::new(5),
                vec![]
            ),
            Err(DpcTwoStageScheduleError::InvalidCursor { .. })
        ));

        let gap = staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x108, 0x110);
        assert!(matches!(
            DpcTwoStageExecution::new(submission, EmulatedInstant::new(5), vec![gap]),
            Err(DpcTwoStageScheduleError::NonContiguousStage { .. })
        ));
        let command = staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x100, 0x110);
        assert!(matches!(
            DpcTwoStageExecution::new(submission, EmulatedInstant::new(5), vec![command, command]),
            Err(DpcTwoStageScheduleError::DuplicateStage { .. })
        ));
        let short_effects =
            staged_barrier(7, 1, DpcExternalWorkStage::EffectsVisible, 0x100, 0x108);
        assert!(matches!(
            DpcTwoStageExecution::new(
                submission,
                EmulatedInstant::new(5),
                vec![command, short_effects]
            ),
            Err(DpcTwoStageScheduleError::EffectsRangeMismatch { .. })
        ));
        let beyond = staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x100, 0x118);
        assert!(matches!(
            DpcTwoStageExecution::new(submission, EmulatedInstant::new(5), vec![beyond]),
            Err(DpcTwoStageScheduleError::BarrierBeyondSubmission { .. })
        ));
        let short_command =
            staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x100, 0x108);
        assert!(matches!(
            DpcTwoStageExecution::new(
                submission,
                EmulatedInstant::new(5),
                vec![short_command, short_effects]
            ),
            Err(DpcTwoStageScheduleError::IncompleteStage { .. })
        ));
    }

    #[test]
    fn two_stage_constructor_rejects_order_time_and_pair_errors_and_accepts_dmem() {
        let submission = DpcSubmission {
            token: 10,
            source: DpcSubmissionSource::Rdram,
            start: 0x100,
            end: 0x108,
        };
        let command = staged_barrier(7, 1, DpcExternalWorkStage::CommandIngested, 0x100, 0x108);
        let effects = staged_barrier(7, 1, DpcExternalWorkStage::EffectsVisible, 0x100, 0x108);
        assert!(matches!(
            DpcTwoStageExecution::new(submission, EmulatedInstant::new(5), vec![effects, command]),
            Err(DpcTwoStageScheduleError::EffectsBeforeCommand(_))
        ));
        assert!(matches!(
            DpcTwoStageExecution::new(submission, EmulatedInstant::new(5), vec![command]),
            Err(DpcTwoStageScheduleError::MissingEffectsStage(_))
        ));
        assert!(matches!(
            DpcTwoStageExecution::new(
                submission,
                EmulatedInstant::new(5),
                vec![
                    DpcTwoStageBarrierPlan {
                        at: EmulatedInstant::new(8),
                        ..command
                    },
                    effects,
                ]
            ),
            Err(DpcTwoStageScheduleError::NonMonotonicBarrier { .. })
        ));

        let dmem = |stage| DpcTwoStageBarrierPlan {
            at: EmulatedInstant::new(7),
            transaction_stage: stage,
            quantum: DpcQuantumId::new(1),
            start: DpcCursor::new(DpcSubmissionSource::Dmem, 0).unwrap(),
            end: DpcCursor::new(DpcSubmissionSource::Dmem, 8).unwrap(),
        };
        assert!(DpcTwoStageExecution::new(
            DpcSubmission {
                token: 11,
                source: DpcSubmissionSource::Dmem,
                start: 0,
                end: 8,
            },
            EmulatedInstant::new(5),
            vec![
                dmem(DpcExternalWorkStage::CommandIngested),
                dmem(DpcExternalWorkStage::EffectsVisible),
            ],
        )
        .is_ok());

        let mut execution = two_stage_execution();
        assert!(matches!(
            execution.advance_to(EmulatedInstant::new(6)),
            Ok(DpcTwoStageAdvance::Reached { at }) if at == EmulatedInstant::new(6)
        ));
        assert!(matches!(
            execution.advance_to(EmulatedInstant::new(5)),
            Err(DpcTwoStageScheduleError::TimeWentBack { now, requested })
                if now == EmulatedInstant::new(6) && requested == EmulatedInstant::new(5)
        ));
    }
}
