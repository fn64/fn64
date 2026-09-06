//! The raw-DPC production seam: plan, execute, and publish.
//!
//! Split out of the former monolithic `RenderBackend` (see the parent
//! module). Every method here carries a loud default, so a backend that does
//! no raw-DPC work writes `impl RawDpcBackend for MyBackend {}` and inherits
//! the same named refusals it inherited before the split.

use super::super::*;

/// Raw-DPC command execution and the production plan/execute/publish seam.
///
/// A backend that implements none of this still implements the trait (every
/// method is defaulted); the defaults refuse by name rather than silently
/// reporting success.
pub trait RawDpcBackend {
    /// Execute a CPU/RSP-produced raw RDP command range selected through the
    /// DPC start/end registers. `output_addr` is the physical VI framebuffer
    /// selected at this submission boundary, under the same contract as
    /// `process_task`; it must not be inferred from backend call history.
    /// Backends that do not implement raw command execution return a named
    /// error; the default must never pretend the range rendered successfully.
    ///
    /// `wait_for_completion`: when `false`, a backend MAY return before the
    /// submitted work is complete, as long as it becomes complete no later
    /// than this backend's next call with `wait_for_completion = true` (or
    /// any other call that reads GPU-completed state, e.g. present).
    /// Callers must always pass `true` for the last submission before
    /// anything downstream needs the finished frame. A backend that has no
    /// concept of asynchronous completion may ignore the flag and always
    /// wait -- that is always correct, just not always fast.
    fn process_rdp_commands(
        &mut self,
        _rdram: &mut [u8],
        start: u32,
        end: u32,
        _output_addr: u32,
        _wait_for_completion: bool,
    ) -> Result<FrameStatus, RenderError> {
        let reason =
            format!("raw RDP command execution [{start:#010x}, {end:#010x}) is unsupported");
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Render,
            "render.raw-rdp.default-backend",
            &reason,
            None,
            fn64_runtime::UnsupportedDisposition::ReturnedError,
        );
        Err(RenderError::Backend {
            backend: "render",
            reason,
        })
    }

    /// Whether this backend can commit separately scheduled raw-DPC chunks.
    /// Existing backends remain atomic and retain their historical call path.
    fn raw_dpc_progression(&self) -> RawDpcProgression {
        RawDpcProgression::Atomic
    }

    /// Execute one externally scheduled raw-DPC quantum.
    ///
    /// The default is a loud rejection. An acknowledged implementation must
    /// leave its private continuation unchanged on `Err`; memory is supplied
    /// as an ABI-owned shadow. Once backend entry occurs, either an `Err` or a
    /// malformed `Ok` poisons that orchestration transaction and is never
    /// retried. The ABI publishes a successful memory image only after
    /// validating transaction, quantum, cursor, identified FullSync evidence,
    /// and `Continue`/`Complete` against the remaining schedule.
    fn process_rdp_command_chunk(
        &mut self,
        _rdram: &mut [u8],
        quantum: RawDpcQuantum,
        _step: RawDpcStep,
    ) -> Result<RawDpcChunkAck, RenderError> {
        Err(RenderError::Backend {
            backend: "raw-dpc-chunk",
            reason: format!(
                "registered atomic backend cannot acknowledge DPC transaction {} quantum {}",
                quantum.request.transaction.get(),
                quantum.request.quantum.get()
            ),
        })
    }

    /// Availability of the explicitly non-certifying staged-RDRAM diagnostic.
    /// `DiagnosticOnly` is never authority to publish guest or device state.
    fn raw_dpc_batch_capability(&self) -> RawDpcBatchCapability {
        RawDpcBatchCapability::Unsupported
    }

    /// Consume a completely preflighted batch for render-only diagnostics.
    ///
    /// The default is a loud capability failure. Implementations must never
    /// loop over `process_rdp_commands` unless it owns a complete backend-state
    /// snapshot: an error after an earlier stream group would otherwise expose
    /// a partial diagnostic result. This seam does not represent `CMD_END`
    /// timing, interrupt ordering, or intermediate memory visibility.
    fn process_raw_dpc_batch(
        &mut self,
        _rdram: &mut [u8],
        _batch: PreflightedRawDpcBatch,
        _output_addr: u32,
    ) -> Result<RawDpcBatchOutcome, RenderError> {
        Err(RenderError::Backend {
            backend: "raw-dpc-batch",
            reason: "registered backend does not implement diagnostic raw-DPC batches".to_string(),
        })
    }

    // --- Production raw-DPC seam (v11 interface freeze) -------------------
    //
    // The four original object-safe raw-DPC methods remain the ordinary
    // one-submission seam. The task-batch methods below add an explicitly
    // capability-gated transport without changing their semantics.
    // `publish_raw_dpc`'s signature has no `Result`
    // (`-> CommittedRawDpcOutcome`, not `Result<_, RenderError>`), unlike its
    // three siblings, so its default body cannot report "unsupported" the
    // way theirs do. It panics instead -- deliberately, not as a workaround:
    // by the time any caller holds a `ReadyRawDpcCommitCapsule` to hand this
    // method, `raw_dpc_ir_capability`/`plan_raw_dpc`/`execute_raw_dpc` have
    // already had to succeed against a real, capable backend (only a capable
    // backend's `execute_raw_dpc` can produce the `BackendPreparedRawDpc` a
    // capsule is eventually sealed from), so this default is unreachable in
    // practice for a correctly gated caller and exists only so the many
    // existing `RenderBackend` implementors across the workspace unrelated
    // to raw-DPC production (test mocks, other backends) do not have to
    // implement a fourth production method just to keep compiling. A
    // conforming raw-DPC-capable backend instead stores a
    // `render_ir::RawDpcCoordinator<P>` (over its own physical state type
    // `P`) and overrides this as exactly
    // `self.coordinator.prepare_publication(publication).commit()` -- see
    // `RawDpcCoordinator::prepare_publication` and `ReadyPublication::commit`
    // for the exact validate-then-consume contract; there is no bare
    // `ReadyRawDpcCommitCapsule` method that reaches `Published` on its own.
    //
    // **Dispatch boundary (B3).** The call *into* `plan_raw_dpc`/
    // `execute_raw_dpc`/`publish_raw_dpc` through `dyn RenderBackend` is
    // itself one dynamic dispatch -- unavoidably, since these are
    // trait-object methods. That entry call is the *only* dynamic dispatch
    // in the raw-DPC production path. Nothing on the `fn64-render` side of
    // the boundary -- `RawDpcAbiSession`'s methods (including
    // `seal_publication`), `RawDpcBackendAuthority::begin_plan`,
    // `ExactRawDpcPlanWriter::finish`,
    // `BoundSubmittedRawDpc::into_backend_prepared`,
    // `RawDpcCoordinator::prepare_publication`, or `ReadyPublication::commit`'s
    // fixed consuming publish body -- performs a further vtable call,
    // `Box<dyn _>` invocation, or trait-object method resolution. A
    // conforming backend's own `execute_raw_dpc`/`publish_raw_dpc` bodies are
    // expected to hold the same property: exactly one dispatch to enter,
    // then monomorphic Rust from there through the terminal state
    // transition.
    //
    // **Dependency-direction reentrancy guarantee.** `fn64-render` and
    // `fn64-render-wgpu` do not, and per this seam's design must never,
    // depend on `fn64-abi`. `fn64-abi`'s live-host access (`with_host`,
    // `with_executor`) is reached only through a `thread_local!`
    // `RefCell`-backed gateway private to `fn64-abi`; that crate has already
    // hit a real "already borrowed" panic from nested reentry through an
    // analogous gateway (`with_executor`'s own doc comment), so this is a
    // proven hazard class, not a theoretical one. Backend code on this side
    // of the boundary cannot name `with_host` even if it wanted to, because
    // no crate-graph edge exists from `fn64-render`/`fn64-render-wgpu` to
    // `fn64-abi`. This is a **load-bearing invariant of this design**, not a
    // hygiene preference: if that dependency direction were ever reversed or
    // an edge added, backend code lent a `ReadyDpcFabricCommit<'_>` "inside
    // the existing `with_host` borrow" (T2 §43-45) could transitively
    // re-enter `with_host` and panic. Any future change that adds
    // `fn64-abi` as a dependency of `fn64-render`/`fn64-render-wgpu` must
    // re-verify this reentrancy property explicitly; it does not hold for
    // free once that edge exists.

    /// What this backend can honestly claim about the production raw-DPC
    /// seam (`docs/RENDER-WGPU-PORT-PLAN.md`'s TMEM-only vertical slice). The
    /// default reports `Unsupported`; only a real transactional backend may
    /// report a wider capability, and only after landing the typestates that
    /// back it.
    fn raw_dpc_ir_capability(&self) -> RawDpcIrCapability {
        RawDpcIrCapability::Unsupported
    }

    /// Whether this backend implements the production task-batch methods.
    fn raw_dpc_task_batch_capability(&self) -> RawDpcTaskBatchCapability {
        RawDpcTaskBatchCapability::Unsupported
    }

    /// Finish one [`RawDpcPlanRequest`] into the neutral, sealed
    /// [`PlannedRawDpcSubmission`] described by card v10 section 3: decode every
    /// command through the one real decoder into
    /// [`ExactRawDpcPlanWriter`](ir::ExactRawDpcPlanWriter)-pushed neutral
    /// semantics, reject FullSync, guest-visible writes, and
    /// unsupported/raster/YUV/TLUT commands, and construct the exact
    /// resource journal and deferred guest-read plan.
    ///
    /// The default is a loud, named rejection -- never a silent `NeedsLle` or
    /// a dropped command. A conforming backend overrides this once its
    /// private provisional decoder can push into an `ExactRawDpcPlanWriter`
    /// obtained from `RawDpcBackendAuthority::begin_plan`, using the
    /// backend's own paired authority -- received at concrete construction/
    /// registration time (per v11 §"non-negotiable shape"), not through this
    /// trait. `fn64-render` intentionally exposes no object-safe method to
    /// install that authority: the pairing is a one-time, backend-concrete
    /// construction fact, not a per-call production operation.
    fn plan_raw_dpc(
        &mut self,
        request: RawDpcPlanRequest,
    ) -> Result<PlannedRawDpcSubmission, RenderError> {
        let _ = request;
        Err(RenderError::Backend {
            backend: "render/raw-dpc-plan",
            reason: "registered backend does not implement production raw-DPC planning".to_string(),
        })
    }

    /// Plan an ordered task's captures while retaining the exact pre-delta
    /// state associated with each member. The default rejects the complete
    /// vector; it never falls back to independently planned packets because
    /// that would lose the batch's state-binding guarantee.
    fn plan_raw_dpc_task_batch(
        &mut self,
        requests: Vec<RawDpcPlanRequest>,
    ) -> Result<Vec<PlannedRawDpcSubmission>, RenderError> {
        drop(requests);
        Err(RenderError::Backend {
            backend: "render/raw-dpc-task-batch-plan",
            reason: "registered backend does not implement production raw-DPC task-batch planning"
                .to_string(),
        })
    }

    /// Execute one sealed, bound raw-DPC submission's declared TMEM loads and
    /// advance it into [`BackendPreparedRawDpc`], retaining every GPU/
    /// physical readiness fact backend-side. The default is a loud rejection
    /// that leaves `bound` unusable to the caller (it is consumed either way,
    /// so its armed retirement still records exactly one `Rejected` on this
    /// path's implicit drop).
    fn execute_raw_dpc(
        &mut self,
        bound: BoundSubmittedRawDpc,
    ) -> Result<BackendPreparedRawDpc, RenderError> {
        let _ = bound;
        Err(RenderError::Backend {
            backend: "render/raw-dpc-execute",
            reason: "registered backend does not implement raw-DPC execution".to_string(),
        })
    }

    /// Execute every bound member of one ordered task against private TMEM
    /// and color-target successor chains. Returned preparations retain their
    /// original order and are still committed and published individually.
    fn execute_raw_dpc_task_batch(
        &mut self,
        bounds: Vec<BoundSubmittedRawDpc>,
    ) -> Result<Vec<BackendPreparedRawDpc>, RenderError> {
        drop(bounds);
        Err(RenderError::Backend {
            backend: "render/raw-dpc-task-batch-execute",
            reason: "registered backend does not implement production raw-DPC task-batch execution"
                .to_string(),
        })
    }

    /// Consume the concrete execution split for the immediately preceding
    /// successful raw-DPC task batch. Compatibility backends report no
    /// authority rather than being guessed from their API or configuration.
    fn take_raw_dpc_task_batch_execution_mechanism(
        &mut self,
    ) -> Option<RawDpcTaskBatchExecutionMechanism> {
        None
    }

    /// The guest-visible `RenderTarget` writes this backend staged for
    /// `submission` during its own [`Self::execute_raw_dpc`] call, in exact
    /// journal order. Empty for every submission this backend staged no
    /// color-target write for -- which is every TMEM-only and triangle-only
    /// submission, and every submission at all for a backend that admits no
    /// fill.
    ///
    /// The caller hands this list straight back to
    /// [`render_ir::RawDpcAbiSession::commit_guest_render_target_writes`],
    /// which re-validates it against the packet's own journal and against
    /// the backend's already-issued `BackendEffectReport`. This method is
    /// therefore a *transport*, not an authority: a backend that returned a
    /// fabricated list would be caught by that constructor, not trusted here.
    ///
    /// Returning an empty list for a submission this backend did stage a
    /// write for is also safe-by-loudness rather than silently wrong: the
    /// caller then takes the zero-write branch, which fails against the
    /// packet's own nonempty guest-write journal with `EffectCountMismatch`.
    ///
    /// Object-safe: takes and returns only owned concrete types.
    fn staged_guest_render_target_writes(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Vec<fn64_render_ir::CompletedWrite> {
        let _ = submission;
        Vec::new()
    }

    /// The exact bytes behind each `CompletedWrite`
    /// [`Self::staged_guest_render_target_writes`] reported for `submission`,
    /// in the identical order, for a caller that has ALREADY committed that
    /// list. This is the RDRAM-copyback transport, and it is deliberately a
    /// second method rather than bytes added to `CompletedWrite`: that type
    /// is `Copy`, shared across every backend, and its premise is that a
    /// backend proves *what* it wrote without shipping the bytes through the
    /// verification path. Widening it would break that premise everywhere.
    ///
    /// Element `i` must be exactly `writes[i].byte_count()` bytes whose
    /// [`ir_effect_content_digest`] equals `writes[i].content()`. The caller
    /// re-derives that digest before copying anything, so a backend returning
    /// the wrong bytes for a correctly-committed range is caught loudly at
    /// the copy site rather than silently corrupting guest memory. The digest
    /// in the committed write is the authority; these bytes are the payload
    /// it already vouched for.
    ///
    /// An empty list means this backend has no bytes for `submission` --
    /// either it staged none, or the token has already been consumed. A
    /// caller that committed a nonempty write list and then receives an empty
    /// byte list must treat that as a defect, not as "nothing to copy".
    ///
    /// Payload ownership is shared and immutable so a backend that already
    /// sealed sparse publication fragments can hand those same bytes across
    /// this seam without materializing a second copy. The caller still owns
    /// its returned handles and still revalidates every payload independently.
    ///
    /// Object-safe: takes and returns only owned concrete types.
    fn committed_guest_render_target_bytes(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Vec<Arc<[u8]>> {
        let _ = submission;
        Vec::new()
    }

    /// Consume the exact device-order color target and physical coverage for
    /// the immediately preceding raw-DPC publication.
    ///
    /// This diagnostic seam is deliberately submission-keyed and consuming:
    /// the caller invokes it synchronously after publishing the same
    /// `submission`, before any later render operation. The interface does
    /// not promise retention across intervening backend work. Backends
    /// without complete physical coverage refuse by name rather than
    /// reconstructing it from visible color bytes.
    fn take_raw_dpc_visual_target_snapshot(
        &mut self,
        submission: fn64_render_ir::SubmissionIdentity,
    ) -> Result<RawDpcVisualTargetSnapshotV1, RawDpcVisualTargetSnapshotRefusal> {
        let reason =
            format!("raw-DPC visual target snapshot for submission {submission:?} is unsupported");
        fn64_runtime::record_unsupported_event(
            fn64_runtime::UnsupportedSubsystem::Render,
            "render.raw-dpc.visual-target-snapshot",
            &reason,
            None,
            fn64_runtime::UnsupportedDisposition::ReturnedError,
        );
        Err(RawDpcVisualTargetSnapshotRefusal::Unsupported)
    }

    /// Jointly publish `publication`'s fabric commit, this backend's own
    /// already-prepared physical state, and the `Published` terminal
    /// outcome. The default panics -- see the module-level comment above for
    /// why that is the deliberate, unreachable-in-practice backstop for
    /// backends that never override this. A conforming raw-DPC-capable
    /// backend stores a [`render_ir::RawDpcCoordinator`] (parameterized over
    /// its own physical state type) and implements this as exactly
    /// `self.coordinator.prepare_publication(publication).commit()` --
    /// `prepare_publication` performs every queue/submission/ready-slot
    /// check `publication` needs (see its own doc comment), and the returned
    /// [`render_ir::ReadyPublication`]'s `commit` is the fixed, straight-line,
    /// no-`Result`, no-callback body that actually flips the coordinator's
    /// active physical slot, commits the fabric transition, and writes
    /// `Published`.
    fn publish_raw_dpc(
        &mut self,
        publication: render_ir::ReadyRawDpcCommitCapsule<'_>,
    ) -> render_ir::CommittedRawDpcOutcome {
        drop(publication);
        panic!(
            "registered backend does not implement raw-DPC publication -- \
             this method should be unreachable unless execute_raw_dpc \
             already (incorrectly) succeeded for a non-raw-DPC-capable backend"
        );
    }
}
