use super::*;

/// The sole right to drive one [`LiveDpcTransaction`]'s atomic acknowledgment
/// from `AwaitingAck` to `Complete`.
///
/// [`LiveDpcTransaction::new`] mints exactly one of these per transaction and
/// [`LiveDpcTransaction::validate_atomic_completion`] consumes it. The type is
/// deliberately move-only -- no `Clone`, no `Copy`, no public constructor --
/// so "this transaction's acknowledgment has already been validated" is a
/// state the type system refuses to represent rather than one a runtime
/// assertion catches after the fact.
///
/// It is `pub` only so that `compile_fail` doctests can name it; there is no
/// public way to obtain one, and `LiveDpcTransaction` itself stays
/// `pub(crate)`.
///
/// Validating a transaction twice does not compile, because the guard the
/// first call consumed cannot be produced again. The guard is move-only, so
/// handing it to a second consumer is a use-after-move:
///
/// ```compile_fail
/// use fn64_abi::DpcAckGuard;
/// # fn guard() -> DpcAckGuard { unimplemented!() }
/// fn validate(_: DpcAckGuard) {}
/// let ack = guard();
/// validate(ack);
/// validate(ack);
/// ```
///
/// The same guard also cannot be duplicated to fake a second validation,
/// because it implements neither `Clone` nor `Copy`:
///
/// ```compile_fail
/// use fn64_abi::DpcAckGuard;
/// # fn guard() -> DpcAckGuard { unimplemented!() }
/// let ack = guard();
/// let duplicate = ack.clone();
/// # drop((ack, duplicate));
/// ```
#[derive(Debug)]
#[must_use = "a DpcAckGuard that is never consumed leaves its transaction's \
              atomic acknowledgment unvalidated"]
pub struct DpcAckGuard {
    transaction: fn64_runtime::DpcTransactionId,
}

impl DpcAckGuard {
    /// Mint the single guard for `transaction`. Private to this module: the
    /// only caller is [`LiveDpcTransaction::new`].
    fn new(transaction: fn64_runtime::DpcTransactionId) -> Self {
        Self { transaction }
    }

    /// The transaction this guard authorizes. Used to reject a guard handed to
    /// the wrong transaction.
    pub(crate) const fn transaction(&self) -> fn64_runtime::DpcTransactionId {
        self.transaction
    }
}

/// Own one fabric-issued DPC transaction across renderer execution. A backend
/// panic unwinds through this guard and cancels the exact token, so a rejected
/// range cannot remain busy or later advance CURRENT as if it had rendered.
pub(crate) struct LiveDpcTransaction {
    pub(crate) token: Option<u64>,
    pub(crate) acknowledgment: Option<fn64_runtime::DpcScheduledExecution>,
}

impl LiveDpcTransaction {
    /// Open a transaction and mint its single [`DpcAckGuard`].
    ///
    /// The guard is the only way to reach `validate_atomic_completion`, so a
    /// transaction whose acknowledgment has already been validated cannot be
    /// validated again: the guard was consumed and cannot be reconstructed.
    pub(crate) fn new(submission: fn64_runtime::DpcSubmission) -> (Self, DpcAckGuard) {
        with_host(|host| {
            assert_eq!(
                host.device_fabric.pending_dpc_submission(),
                Some(submission),
                "renderer received DPC transaction which the device fabric does not own"
            );
        });
        // Install cancellation ownership before any shared-ack construction.
        // If an admitted fabric range cannot form the compatibility quantum,
        // unwinding this guard restores the exact pre-admission DPC state.
        // The exact-token assertion stays first so a bad caller cannot make
        // this guard cancel some other transaction while unwinding.
        let mut transaction = Self {
            token: Some(submission.token),
            acknowledgment: None,
        };
        let source = submission.source;
        let start = fn64_runtime::DpcCursor::new(source, submission.start)
            .unwrap_or_else(|error| panic!("fabric admitted invalid DPC start cursor: {error:?}"));
        let end = fn64_runtime::DpcCursor::new(source, submission.end)
            .unwrap_or_else(|error| panic!("fabric admitted invalid DPC end cursor: {error:?}"));
        // Phase B deliberately assigns no device-time meaning to this one
        // compatibility quantum. Zero is an internal acknowledgment sentinel;
        // production still performs one synchronous atomic backend call.
        let sentinel = fn64_runtime::Cycles::new(0);
        let mut acknowledgment = fn64_runtime::DpcScheduledExecution::new(
            submission,
            sentinel,
            vec![fn64_runtime::DpcQuantumPlan {
                at: sentinel,
                id: fn64_runtime::DpcQuantumId::new(1),
                start,
                end,
            }],
        )
        .unwrap_or_else(|error| {
            panic!("fabric DPC transaction cannot form an atomic ack: {error:?}")
        });
        let fn64_runtime::DpcAdvance::Blocked { at, action } = acknowledgment
            .advance_to(sentinel)
            .unwrap_or_else(|error| panic!("arming atomic DPC acknowledgment: {error:?}"))
        else {
            panic!("atomic DPC acknowledgment passed its sole external-work barrier")
        };
        assert_eq!(at, sentinel);
        assert_eq!(action.transaction, acknowledgment.transaction());
        assert_eq!(action.start, start);
        assert_eq!(action.end, end);
        let guard = DpcAckGuard::new(acknowledgment.transaction());
        transaction.acknowledgment = Some(acknowledgment);
        (transaction, guard)
    }

    /// Validate the compatibility backend's sole atomic completion before
    /// publishing its shadow memory. This carries no timing authority.
    ///
    /// Consumes this transaction's [`DpcAckGuard`]. Because `new` mints
    /// exactly one guard and there is no other way to construct one, a second
    /// validation of the same transaction is a compile error rather than a
    /// runtime assertion -- which is what retired the former
    /// "lost its acknowledgment owner before validation" panic's
    /// *already-validated* trigger.
    ///
    /// # Panics
    ///
    /// Panics if `guard` was minted for a different transaction, or if the
    /// acknowledgment is not awaiting its acknowledgment -- in practice, if it
    /// has been poisoned (`DpcScheduledExecution::poison`).
    ///
    /// **No production path reaches the poisoned arm today.** The only
    /// non-test `poison()` callers in this file are methods on
    /// `ScheduledRawDpcTransaction`, which is `#[cfg(test)]` and poisons its
    /// own separate `execution` field, never a `LiveDpcTransaction`'s
    /// acknowledgment; `dispatch_a`'s regression reaches this arm by poisoning
    /// the acknowledgment field directly. The arm is kept anyway because
    /// `poison` is a live `pub` API on a `fn64-runtime` type that a future
    /// production backend-rejection path can call, and the phase is not
    /// excluded by the type system -- so the choice is a loud named panic or a
    /// silent pass-through that would validate un-acknowledged work.
    pub(crate) fn validate_atomic_completion(&mut self, guard: DpcAckGuard) {
        let acknowledgment = self
            .acknowledgment
            .as_mut()
            .expect("atomic DPC transaction has no acknowledgment owner");
        assert_eq!(
            guard.transaction(),
            acknowledgment.transaction(),
            "DpcAckGuard was minted for a different DPC transaction"
        );
        let fn64_runtime::DpcScheduledPhase::AwaitingAck(request) = acknowledgment.phase() else {
            panic!("atomic DPC transaction is not awaiting its acknowledgment before validation")
        };
        acknowledgment
            .acknowledge(fn64_runtime::DpcBackendQuantumAck {
                transaction: request.transaction,
                quantum: request.quantum,
                committed_through: request.end,
                status: fn64_runtime::DpcBackendQuantumStatus::Complete,
            })
            .unwrap_or_else(|error| panic!("validating atomic DPC acknowledgment: {error:?}"));
        assert_eq!(
            acknowledgment.phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC acknowledgment did not consume its sole quantum"
        );
    }

    pub(crate) fn commit(mut self) {
        let token = *self
            .token
            .as_ref()
            .expect("DPC transaction committed twice");
        assert_eq!(
            self.acknowledgment
                .as_ref()
                .expect("atomic DPC transaction has no acknowledgment owner")
                .phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC transaction committed before acknowledgment validation"
        );
        with_host(|host| host.device_fabric.commit_dpc_submission(token))
            .unwrap_or_else(|error| panic!("committing rendered DPC transaction: {error}"));
        self.token.take();
    }

    /// Route this transaction's terminal fabric commit through the
    /// nonmutating-prepare / infallible-consume `ReadyDpcFabricCommit`
    /// typestate (`fn64_runtime::device::fabric_ops`), handing the live
    /// `ReadyDpcFabricCommit` to a caller-supplied closure INSIDE the
    /// `with_host` borrow rather than committing it immediately.
    ///
    /// This is the seam a future T0 capsule-assembly call needs: the v11
    /// migration card's `ReadyRawDpcCommitCapsule` must own the ready fabric
    /// state across its OWN joint physical/fabric publication (device fabric
    /// prepares, wgpu does its fallible physical-readiness work, THEN one
    /// atomic body commits both). A method that prepares and immediately
    /// calls `.commit()` before any capsule exists cannot serve that: there
    /// is nothing left for a capsule to receive. `with_ready_commit` instead
    /// hands the ready value, live, to `f` -- which is where a future caller
    /// builds `ReadyRawDpcCommitCapsule` from it (combined with the
    /// guest-committed wrapper) and either commits or lets it drop-cancel,
    /// all still inside the one `with_host` borrow this fabric requires.
    ///
    /// `f`'s return value `R` is NOT permitted to retain the
    /// `ReadyDpcFabricCommit<'_>` borrow -- `with_host`'s own signature
    /// (`impl FnOnce(&mut HostState) -> R`) already forbids that, so the
    /// compiler enforces it structurally, not by convention.
    ///
    /// **Disarms this transaction's own cancel guard (`self.token`) only
    /// AFTER a `ReadyDpcFabricCommit` has been successfully constructed, not
    /// before.** `LiveDpcTransaction::drop` and `ReadyDpcFabricCommit::drop`
    /// are two independent cancellation paths over the same underlying
    /// fabric state (`LiveDpcTransaction` via `cancel_dpc_submission(token)`
    /// on the fabric as a whole; `ReadyDpcFabricCommit` via direct field
    /// writes to the same `dpc`/`pending_dpc` fields `prepare_dpc_commit`
    /// borrowed). Exactly one of the two must be the live cancellation owner
    /// at every point in this method's body -- disarming too early leaks the
    /// pending fabric transaction; disarming too late double-cancels it:
    ///
    /// - While the acknowledgment-phase check and `prepare_dpc_commit`'s own
    ///   fallible validation are still running, NO `ReadyDpcFabricCommit`
    ///   exists yet, so `LiveDpcTransaction` must remain the armed owner: if
    ///   either fails (an assertion panic, or `prepare_dpc_commit` returning
    ///   `Err`), `LiveDpcTransaction::drop` is what cancels the still-pending
    ///   fabric transaction. `prepare_dpc_commit` itself restores the owned
    ///   `PendingDpc` on any rejection (see its own doc comment), so the
    ///   fabric's `pending_dpc` is exactly as it was when
    ///   `LiveDpcTransaction::drop` runs `cancel_dpc_submission`.
    /// - The token is read (`self.token`), not taken, for this whole
    ///   validate-then-prepare span, so `self` stays armed throughout it.
    /// - Only once `prepare_dpc_commit` has returned `Ok` -- meaning a
    ///   `ReadyDpcFabricCommit` now exists and holds its own independent
    ///   cancellation path -- does this method assign `self.token = None`,
    ///   disarming `LiveDpcTransaction::drop`, immediately before calling
    ///   `f(ready)`. From that point on, `ReadyDpcFabricCommit` is the sole
    ///   cancellation owner: if `f` panics, `ReadyDpcFabricCommit::drop`
    ///   cancels (it unwinds before `LiveDpcTransaction::drop`, which is
    ///   already a no-op by then); `LiveDpcTransaction::drop` cannot also
    ///   fire a second `cancel_dpc_submission` against fabric state
    ///   `ReadyDpcFabricCommit::drop` may have already rolled back or
    ///   cleared, which is what would otherwise panic from inside an unwind
    ///   already in progress and abort the process.
    pub(crate) fn with_ready_commit<R>(
        mut self,
        f: impl FnOnce(fn64_runtime::device::ReadyDpcFabricCommit<'_>) -> R,
    ) -> R {
        let token = self.token.expect("DPC transaction committed twice");
        assert_eq!(
            self.acknowledgment
                .as_ref()
                .expect("atomic DPC transaction has no acknowledgment owner")
                .phase(),
            fn64_runtime::DpcScheduledPhase::Complete,
            "atomic DPC transaction committed before acknowledgment validation"
        );
        // `self.token` is still `Some` here: `LiveDpcTransaction::drop`
        // remains the armed cancellation owner through every fallible step
        // below, up to and including `prepare_dpc_commit` itself.
        with_host(|host| {
            let ready = host
                .device_fabric
                .prepare_dpc_commit(token)
                .unwrap_or_else(|error| panic!("preparing ready DPC fabric commit: {error}"));
            // A `ReadyDpcFabricCommit` now exists and owns its own
            // cancellation path. Disarm `LiveDpcTransaction` here, and not
            // one line earlier, so there is no window where neither guard is
            // armed, and no OBSERVABLE OR FALLIBLE window where both can
            // act: both guards are briefly simultaneously armed across this
            // one nonpanicking assignment (`ready` already exists the moment
            // this comment is reached), but nothing between `ready`'s
            // construction and this line can panic, return `Result`, invoke
            // a callback, or otherwise give either guard's `Drop` a chance to
            // run -- so the two-armed span has no reachable exit that could
            // let both actually cancel.
            self.token = None;
            f(ready)
        })
    }
}

impl Drop for LiveDpcTransaction {
    fn drop(&mut self) {
        let Some(token) = self.token.take() else {
            return;
        };
        with_host(|host| host.device_fabric.cancel_dpc_submission(token))
            .unwrap_or_else(|error| panic!("cancelling rejected DPC transaction: {error}"));
    }
}
