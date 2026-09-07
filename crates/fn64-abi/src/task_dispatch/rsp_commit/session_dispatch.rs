use super::*;

/// Attempt the T4 production plan/execute/publish routing for one raw-DPC
/// submission. Returns `None` (never partially attempted) when no
/// `RawDpcAbiSession` is registered, so callers fall back to the legacy
/// atomic `process_rdp_commands` path unconditionally -- required for
/// `Rt64Backend` and any other backend that never implements
/// `plan_raw_dpc`/`execute_raw_dpc`/`publish_raw_dpc`.
///
/// `plan_raw_dpc` (`fn64-render-wgpu`'s `WgpuBackend`) already rejects a
/// `FullSync` command or any command outside the admitted TMEM/state/fill
/// subset as a loud `RenderError`.
///
/// Which guest-commit method runs is decided by what the backend itself
/// says it staged, read back through `staged_guest_render_target_writes`:
///
/// - An empty list takes `commit_zero_guest_writes`, which independently
///   re-rejects any guest-visible write with `EffectCountMismatch`. This is
///   every TMEM-only and triangle-only submission.
/// - A nonempty list takes `commit_guest_render_target_writes`, which
///   re-validates every element's access mode/purpose and then, through
///   `GuestCommitEffectReport::try_new`, its count, order, identity, and
///   content digest against the packet's own guest-write journal. A backend
///   that reported a fabricated list is caught there, not trusted here.
///
/// Neither rejection is caught: both `.unwrap_or_else(|error| panic!(...))`
/// through, matching AGENTS.md's loud-trap rule.
///
/// Taking the nonempty branch DOES modify guest RDRAM, through
/// `copy_committed_guest_writes` and only after the commit above returned
/// `Ok`. This supersedes the earlier nonclaim on this function ("taking the
/// nonempty branch modifies no guest RDRAM byte"), which was true until the
/// copyback landed.
///
/// Nonclaim, unchanged: the zero-write branch modifies nothing, and
/// `CompletedWrite` still carries no bytes -- the payload travels through
/// `RenderBackend::committed_guest_render_target_bytes`, a separate method,
/// and is checked against the committed digest before it is written.
/// A submission this backend cannot admit is a hard stop, not a silent
/// fallback to the legacy path: falling back would let a T4-registered
/// session quietly downgrade capture fidelity for exactly the submissions
/// its own admission rules were built to catch.
pub(super) fn try_dispatch_raw_dpc_via_session(
    rdram: *mut u8,
    source: SessionRawDpcSource,
    mut transaction: LiveDpcTransaction,
    ack: DpcAckGuard,
    temporal_guest_reads: Option<(
        &fn64_audio::rsp::runtime::RspDeferredDpcHistory,
        &[CommandReadEpochBoundary],
    )>,
) -> Option<(fn64_render::DpFullSyncStatus, RspRdpObservationKind)> {
    let registered = RAW_DPC_SESSION.with(|cell| cell.borrow().is_some());
    if !registered {
        return None;
    }

    // The live RDRAM allocation is the sole guest-read byte source for both
    // producers -- see `SessionRawDpcSource`'s doc comment -- and also the
    // sole memory-layout proof: XBUS command words are bounded separately
    // (`DmemRange`, the 4 KiB DMEM bank) inside `preflight_raw_dpc_capture`,
    // never through this `memory_layout`.
    let real = unsafe { renderer_rdram_slice(rdram) };
    let memory_layout = fn64_render::ir::PhysicalMemoryLayout::try_new(
        u32::try_from(real.len()).expect("registered RDRAM allocation fits a u32 byte length"),
    )
    .unwrap_or_else(|error| panic!("try_dispatch_raw_dpc_via_session: {error}"));
    // The `cmd_end` interrupt snapshot is a fixed `Clear`. That is exact, not
    // an assumption: the DP interrupt for a raw FullSync is raised inside
    // `DeviceFabric::advance_to`'s `DeviceEvent::Dp` arm, and device
    // advancement cannot run during renderer dispatch, so the line cannot
    // have been raised by this submission at the moment this boundary is
    // built. `transaction_sequence` reuses this exact transaction's own
    // fabric-issued token: real per-submission fabric identity, not a
    // fabricated counter, matching the requirement to preserve the existing
    // fabric token lifecycle through this new path.
    let token = transaction
        .token
        .expect("try_dispatch_raw_dpc_via_session: transaction committed twice");
    let xbus = source.submission.source() == fn64_render::RawDpcSource::XbusDmem;
    let observation_start = source.submission.start();
    let observation_end = source.submission.end();
    let observation_words = source.submission.command_words();
    maybe_dump_session_raw_dpc(&source.submission, &observation_words, real);
    let cmd_end =
        fn64_render::ir::TemporalBoundary::new(token, fn64_render::ir::DpInterruptState::Clear);

    // Reserve half of the FullSync two-phase contract.
    //
    // `fn64-render-ir` requires exactly one `FullSyncBoundary` per decoded
    // `SYNC_FULL` opcode, so a submission carrying one cannot be planned at
    // all unless this producer supplies it. Count the sites structurally
    // (same stride walk, same six-bit masking as the RDRAM inspector) and,
    // when there are any, prove the sole DP completion slot is free through
    // the nonmutating `preflight_dp_full_sync` BEFORE the backend is entered
    // or any guest byte is read -- which is precisely what that function's
    // own doc says it exists for.
    // **This path receives CLOSED streams only.** A completed RSP task's
    // submissions are coalesced by `coalesce_dp_submissions` before they get
    // here, and a CPU raw-MMIO stream reaches this function only once the
    // fabric has assembled a whole command run -- an incomplete tail is
    // parked in the fabric and never dispatched. So `Incomplete` here means
    // an assembler upstream broke its contract, and it stays a loud panic
    // rather than silently stranding a completed transaction.
    let full_sync_sites = match fn64_render::count_raw_rdp_full_sync_sites(&observation_words)
        .unwrap_or_else(|error| panic!("try_dispatch_raw_dpc_via_session: {error}"))
    {
        fn64_render::RawRdpScan::Complete(sites) => sites,
        fn64_render::RawRdpScan::Incomplete {
            command_start,
            bytes_required,
            bytes_available,
            ..
        } => panic!(
            "try_dispatch_raw_dpc_via_session: a dispatched stream ends inside the command at \
             byte {command_start:#x} ({bytes_available} of {bytes_required} bytes present); \
             incomplete tails must be parked by the fabric, never dispatched"
        ),
    };
    let capture = if full_sync_sites == 0 {
        fn64_render::OwnedRawDpcCapture::new(source.submission, memory_layout, token, cmd_end)
    } else {
        // Interleaving closed exactly as `preflight_raw_dpc_completion`
        // closes it on the legacy path: a prior FullSync may still be
        // pending, and observing an occupied slot here rejects before the
        // backend or RDRAM is touched.
        with_host(|host| {
            host.device_fabric
                .preflight_dp_full_sync(fn64_runtime::Cycles::new(1))
        })
        .unwrap_or_else(|error| {
            panic!("try_dispatch_raw_dpc_via_session: DP FullSync completion: {error}")
        });

        // HONESTY BOUNDARY -- read this before changing either state below.
        //
        // `interrupt_before` is `Clear` because it is genuinely observed:
        // device advancement cannot run during dispatch, so nothing this
        // submission did could have raised the line yet.
        //
        // `interrupt_after` is ALSO `Clear`, and that is the honest value,
        // not a placeholder to be "fixed" later by writing `Asserted` here.
        // A successful `preflight_dp_full_sync` is a RESERVATION: it is
        // nonmutating, it schedules no `DeviceEvent::Dp`, and it raises no
        // interrupt. The interrupt for this submission is raised only when
        // `complete_committed_dpc` calls `start_live_dp_full_sync` and the
        // guest later advances devices past the deadline -- strictly after
        // this capture, this plan, this execution, and this publication have
        // all already happened. There is therefore no point in this flow at
        // which an `Asserted` value could be READ, and writing one would
        // fabricate a guest-visible interrupt edge that never occurred.
        //
        // Delivering a truthful `Asserted` needs the post-commit read-
        // observation and coherence work `docs/RENDER-WGPU-PORT-PLAN.md`'s
        // D7 defers to M9. Until then the decoded site is recorded and the
        // observation is not claimed.
        //
        // Sequences: `cmd_end` owns `token`, so each site's pair must be
        // strictly increasing after it and its own interrupt sequence must
        // exceed its site sequence -- `derive_stream`'s
        // `NonMonotonicFullSyncSequence` check.
        let boundaries = (0..full_sync_sites)
            .map(|ordinal| {
                let ordinal = ordinal as u64;
                fn64_render::ir::FullSyncBoundary::new(
                    token + 1 + ordinal * 2,
                    token + 2 + ordinal * 2,
                    fn64_render::ir::DpInterruptState::Clear,
                    fn64_render::ir::DpInterruptState::Clear,
                )
            })
            .collect();
        fn64_render::OwnedRawDpcCapture::with_full_sync_boundaries(
            source.submission,
            memory_layout,
            token,
            cmd_end,
            boundaries,
        )
    };

    let observation = dpc_observation(xbus, observation_start, observation_end, &observation_words);

    crate::session_phase_census::note_submission();
    let planned =
        crate::session_phase_census::timed(crate::session_phase_census::Phase::Plan, || {
            RENDER_BACKEND.with(|backend_cell| {
                RAW_DPC_SESSION.with(|session_cell| {
                    let mut backend = backend_cell.borrow_mut();
                    let backend = backend
                        .as_mut()
                        .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
                    let session = session_cell.borrow();
                    let session = session.as_ref().expect(
                        "try_dispatch_raw_dpc_via_session: session vanished under this borrow",
                    );
                    let request = session.plan_request(capture);
                    backend
                        .backend_mut("plan_raw_dpc")
                        .plan_raw_dpc(request)
                        .unwrap_or_else(|error| panic!("plan_raw_dpc: {error}"))
                })
            })
        });

    let guest_capture = if let Some((history, boundaries)) = temporal_guest_reads {
        TaskGuestReadCaptureArena::new(real, history).capture(planned.guest_read_plan(), boundaries)
    } else {
        fn64_render::ir::DeferredGuestReadCapture::new(
            planned
                .guest_read_plan()
                .reads()
                .iter()
                .map(|read| {
                    let range = read.range();
                    let start = range.start().get() as usize;
                    let end = range.end() as usize;
                    assert!(
                        end <= real.len(),
                        "plan_raw_dpc declared guest read [{start:#x}, {end:#x}) outside \
                     the captured source"
                    );
                    // **Logical order, not raw storage** -- the same byte-lane
                    // authority the committed-write direction below already
                    // observes, applied to the read direction.
                    //
                    // `CapturedGuestRead`'s contract is N64-logical bytes, and the
                    // TMEM load executors index the capture linearly with no lane
                    // mapping of their own. `real` is a bare pointer slice over
                    // ABI storage, where bytes sit under the `^3` map, so a raw
                    // `to_vec()` handed the sampler every 32-bit word
                    // byte-reversed: "adjacent columns swapped AND each halfword
                    // byte-reversed", exactly the symptom this file's own
                    // write-back doc records for the outlier raw copy that was
                    // fixed there.
                    //
                    // Command words survived the raw read by accident -- `^3`
                    // composed with a little-endian host load cancels for an
                    // aligned 32-bit word -- which is why this was invisible in
                    // command decode and fatal only for byte-granular texture
                    // data.
                    //
                    // Measured: with the raw copy, an eight-texel RGBA16 parity
                    // fixture sampled the raw storage halfwords (`0xc107` where
                    // `0xf801` was staged, all eight explained by that one rule)
                    // while RT64 read the identical buffer and returned the key.
                    // With this, both backends are byte-identical to the key.
                    let mut bytes = vec![0; end - start];
                    fn64_runtime::RdramView::from_storage(real).copy_logical_bytes(
                        fn64_runtime::RdramAddr::from_offset(range.start().get()),
                        &mut bytes,
                    );
                    fn64_render::ir::CapturedGuestRead::try_new(*read, bytes)
                        .unwrap_or_else(|error| panic!("CapturedGuestRead::try_new: {error}"))
                })
                .collect(),
        )
    };

    let bound = RAW_DPC_SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let session = session
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: session vanished under this borrow");
        crate::session_phase_census::timed(crate::session_phase_census::Phase::Finalize, || {
            session
                .finalize_and_submit(planned, guest_capture)
                .unwrap_or_else(|error| panic!("finalize_and_submit: {error}"))
        })
    });

    let prepared = RENDER_BACKEND.with(|cell| {
        let mut backend = cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
        crate::session_phase_census::timed(crate::session_phase_census::Phase::Execute, || {
            backend
                .backend_mut("execute_raw_dpc")
                .execute_raw_dpc(bound)
                .unwrap_or_else(|error| panic!("execute_raw_dpc: {error}"))
        })
    });

    // The guest-visible `RenderTarget` writes the backend staged for THIS
    // submission during the `execute_raw_dpc` call just above, read back in
    // its own borrow because `RENDER_BACKEND` and `RAW_DPC_SESSION` are
    // separate `RefCell`s that this function has always borrowed
    // separately. Empty for every TMEM-only and triangle-only submission,
    // which is every submission admitted before FillRectangle was.
    //
    // This list is transport, not authority: whichever commit branch it
    // selects below re-validates it against the packet's own journal and
    // against the backend's already-issued `BackendEffectReport`.
    let staged_writes = RENDER_BACKEND.with(|cell| {
        let mut backend = cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
        backend
            .backend_mut("staged_guest_render_target_writes")
            .staged_guest_render_target_writes(prepared.submission())
    });

    let submission_identity = prepared.submission();
    let commit_writes = staged_writes.clone();
    let committed = RAW_DPC_SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let session = session
            .as_mut()
            .expect("try_dispatch_raw_dpc_via_session: session vanished under this borrow");
        crate::session_phase_census::timed(crate::session_phase_census::Phase::Commit, || {
            if staged_writes.is_empty() {
                session
                    .commit_zero_guest_writes(prepared)
                    .unwrap_or_else(|error| panic!("commit_zero_guest_writes: {error}"))
            } else {
                session
                    .commit_guest_render_target_writes(prepared, staged_writes)
                    .unwrap_or_else(|error| panic!("commit_guest_render_target_writes: {error}"))
            }
        })
    });

    // The RDRAM copyback, and the ONLY place this path writes a guest byte.
    //
    // Strictly after the commit above, never speculatively: reaching this
    // line means `commit_guest_render_target_writes` already re-validated
    // every element's access mode/purpose and then, through
    // `GuestCommitEffectReport::try_new`, its count, order, identity, and
    // content digest against the packet's own guest-write journal. The
    // journal -- not the backend -- is therefore the authority for which
    // ranges may be written, and a backend that reported a fabricated list
    // panicked above rather than reaching here.
    //
    // Supersedes the T-17 nonclaim ("nothing in the FillRectangle admission
    // chain writes guest RDRAM"), deliberately and with its test replaced by
    // ones asserting the new behavior --
    // `tests::raw_dpc_session_integration`'s
    // `an_admitted_whole_target_fill_writes_its_image_into_guest_rdram`,
    // `an_admitted_partial_width_fill_writes_only_its_own_disjoint_rows`,
    // and `an_admitted_odd_origin_fill_writes_target_relative_columns_into_guest_rdram`.
    // `a_rejected_guest_commit_leaves_guest_rdram_untouched` pins the
    // after-the-commit ordering this `if` depends on, and
    // `a_tmem_only_submission_writes_no_guest_target_byte` pins the gate.
    if !commit_writes.is_empty() {
        copy_committed_guest_writes(real, submission_identity, &commit_writes);
    }

    // Mirrors the legacy path's own `transaction.validate_atomic_completion()`
    // call (see `dispatch_dpc_submission`'s `Rdram` arm and
    // `dispatch_captured_raw_rdp`): the compatibility acknowledgment this
    // transaction opened at `LiveDpcTransaction::new` must be driven to
    // `Complete` before `with_ready_commit` will accept it -- required by
    // `with_ready_commit`'s own precondition assertion, independent of
    // which path (legacy or T4 session) produced the completed backend
    // result.
    transaction.validate_atomic_completion(ack);

    // `with_ready_commit` hands the live `ReadyDpcFabricCommit` to this
    // closure INSIDE its one `with_host` borrow (see its own doc comment);
    // `seal_publication`/`publish_raw_dpc` run here, not after, so the fabric
    // token's prepare -> seal -> publish sequence stays exactly as ordered
    // as the legacy path's own prepare-then-commit, just carrying a capsule
    // through the middle instead of committing immediately.
    let outcome = transaction.with_ready_commit(|ready| {
        RAW_DPC_SESSION.with(|session_cell| {
            let mut session = session_cell.borrow_mut();
            let session = session
                .as_mut()
                .expect("try_dispatch_raw_dpc_via_session: session vanished under this borrow");
            let capsule = session
                .seal_publication(committed, ready)
                .unwrap_or_else(|error| panic!("seal_publication: {error}"));
            RENDER_BACKEND.with(|backend_cell| {
                let mut backend = backend_cell.borrow_mut();
                let backend = backend
                    .as_mut()
                    .expect("try_dispatch_raw_dpc_via_session: no render backend registered");
                backend
                    .backend_mut("publish_raw_dpc")
                    .publish_raw_dpc(capsule)
            })
        })
    });
    let _ = outcome;

    record_rsp_rdp_observations(vec![observation.clone()]);
    record_rdp_renderer_publication_v1();
    // Commit half of the FullSync two-phase contract.
    //
    // `DpFullSyncStatus` keeps its exact existing meaning here -- "the
    // backend reached the opcode" -- which is why no fourth variant was
    // added: this enum is consumed by sticky-OR in five places
    // (`rsp_commit.rs`'s two loops and `advance_one`, `raw_dpc_batch.rs`'s
    // `aggregate_full_sync`, and the reference backend's `imp.rs`), and any
    // new variant would read as "no interrupt" in every `!= Reached` test.
    //
    // Reporting `Reached` routes this submission into the caller's sticky-OR
    // and, eventually, `complete_committed_dpc`'s `start_live_dp_full_sync`
    // -- the mutating commit half that actually schedules the DP event. That
    // is the same commit the legacy path performs for the same command
    // stream; the T4 path no longer silently swallows it.
    //
    // Nonclaim: `Reached` means the opcode was walked and the slot was
    // reserved. It does NOT mean the guest observed a DP interrupt. That
    // claim lives only in a `FullSyncBoundary` whose `interrupt_after` is
    // `Asserted`, and this path supplies `Clear` -- see the honesty boundary
    // comment where the boundaries are built.
    let full_sync = if full_sync_sites == 0 {
        fn64_render::DpFullSyncStatus::NotReached
    } else {
        fn64_render::DpFullSyncStatus::Reached
    };
    Some((full_sync, observation))
}

/// Copy one already-committed submission's guest render-target writes into
/// live RDRAM, and nothing else.
///
/// Called only from `try_dispatch_raw_dpc_via_session`, and only after
/// `commit_guest_render_target_writes` returned `Ok`. `writes` is that exact
/// committed list, so every range here has already been validated against
/// the packet's own guest-write journal by
/// `GuestCommitEffectReport::try_new`.
///
/// **The copy is self-checking.** Each write's committed `ContentDigest` is
/// re-derived from the bytes the backend hands over, in the same
/// `ir_effect_content_digest` domain, and a mismatch panics BEFORE any byte
/// is written. A backend whose byte transport disagrees with the digest it
/// already committed is a defect that must be loud, not one that silently
/// scribbles a wrong rectangle into guest memory. The digest is the
/// authority; the bytes are the payload it vouched for.
///
/// **Exactly the committed ranges, no more.** Each `CompletedWrite` is
/// copied at its own `ResourceRegion::Rdram` range and nowhere else. A
/// partial-width fill declares N *disjoint* per-row ranges strided by the
/// color image's width (`fn64-render-wgpu`'s `raw_dpc::plan_fill` collapses
/// to a single range only when the rectangle spans the full image width), so
/// this loop writes N separate spans and never the gaps between them.
/// Collapsing them into one span would claim far more bytes than the fill
/// wrote.
///
/// **Byte-lane mapping: the payload is LOGICAL, the storage is PHYSICAL.**
/// The backend hands over guest-order bytes -- `targets/fill.rs`'s
/// `write_pixel` emits `packed.to_be_bytes()`, big-endian as the RDP writes
/// them -- while this crate's RDRAM allocation is N64Recomp native-word
/// storage, where a logical byte at offset `o` lives at `o ^ 3`
/// (`crates/fn64-runtime/src/rdram.rs`'s module doc, transcribed from
/// `recomp.h`'s `MEM_B`/`MEM_H`). So the copy goes through
/// `RdramViewMut::write_logical_bytes`, which owns that one mapping.
///
/// This was a `copy_from_slice` into the raw allocation and that was WRONG,
/// measured not argued: the VI reads the same memory through
/// `PhysicalRdramRead::read_u16`'s `^2` lane XOR, so a raw-copied fill
/// presented with adjacent columns swapped AND each halfword byte-reversed.
/// The lane-mapped convention is the established one -- the reference
/// backend's own RDP writeback uses `view.write_u16`
/// (`crates/fn64-render-reference/src/backend/framebuffer_io.rs:188`) and
/// `vi_scanout.rs`'s "Byte-lane authority" section names it as the single
/// authority -- and the raw copy here was the outlier. The two legacy
/// copybacks in this file stay raw and are NOT the same case: they round-trip
/// `real` through a whole-RDRAM `image`, so their bytes are already physical.
///
/// **Byte granularity, not halfword.** `write_logical_bytes` maps one byte at
/// a time (`^3`), so it is correct for an arbitrary `CompletedWrite` range
/// with no alignment or even-length precondition. A `write_u16` loop would
/// need both and a committed range guarantees neither -- it is a byte range
/// whose seam is byte-typed, even though a fill's rows happen to be RGBA16.
///
/// **What the digest covers: the PAYLOAD, not the memory image.** The
/// `ir_effect_content_digest` re-check below hashes the backend's logical
/// bytes exactly as handed over, before any lane mapping. That is the right
/// domain and not an oversight: the digest is the backend's own commitment
/// about the content it rendered, and the backend has no opinion about host
/// storage layout. Hashing the post-mapping image would compare a value the
/// backend never computed, and would make the self-check pass or fail on
/// this crate's storage convention rather than on byte transport integrity.
///
/// Writes go through `track_rdp_renderer_mutation` for the same reason the
/// legacy `dispatch_captured_raw_rdp` path does: a guest-visible renderer
/// write must reach the write-barrier journal, not bypass it. The tracker is
/// handed the WHOLE `real` allocation, not the destination subslice: it
/// snapshots and diffs watched ranges by absolute physical offset
/// (`recompiled/snapshots.rs`'s `track_catalog_nested_mutation` reads through
/// `RdramView::read_u8(RdramAddr::from_offset(physical))`), so a subslice
/// would have made every watched offset name the wrong byte.
struct ValidatedGuestCopyback<'a> {
    addr: fn64_runtime::RdramAddr,
    bytes: &'a [u8],
}

pub(super) fn copy_committed_guest_writes(
    real: &mut [u8],
    submission: fn64_render::ir::SubmissionIdentity,
    writes: &[fn64_render::ir::CompletedWrite],
) {
    let census_started = renderer_copyback_census::started();
    let payloads = RENDER_BACKEND.with(|cell| {
        let mut backend = cell.borrow_mut();
        let backend = backend
            .as_mut()
            .expect("copy_committed_guest_writes: no render backend registered");
        backend
            .backend_mut("committed_guest_render_target_bytes")
            .committed_guest_render_target_bytes(submission)
    });

    assert_eq!(
        payloads.len(),
        writes.len(),
        "the backend committed {} guest render-target write(s) but produced bytes for {} -- \
         a committed write with no bytes behind it is a backend defect, never a reason to \
         copy a partial rectangle",
        writes.len(),
        payloads.len()
    );

    // Convert the host allocation length once at the boundary where the
    // renderer's typed physical layout is matched to this concrete storage.
    // Past this point each prepared value keeps an RdramAddr rather than a
    // host index, so copyback cannot accidentally mix address domains.
    let registered_layout = fn64_render::ir::PhysicalMemoryLayout::try_new(
        u32::try_from(real.len()).expect("registered RDRAM exceeds the RDP address width"),
    )
    .expect("registered RDRAM must be a valid physical memory layout");

    // Every payload is validated against its own committed write BEFORE the
    // first byte is copied, so a mismatch in the last write cannot leave the
    // earlier ones already applied. The collected type is the proof consumed
    // by the mutation transaction below.
    //
    // The digest assertion below is deliberately kept even though deleting
    // it leaves every test's FINAL RDRAM state unchanged -- measured, by
    // mutation, not assumed. Corrupting one halfword in the backend's byte
    // transport (`committed_guest_render_target_bytes`) trips this assertion
    // and no guest byte is written. Delete the assertion and the same
    // corruption reaches guest memory, where it is caught only afterwards by
    // a test's own pixel comparison. The two mutants are equivalent in
    // outcome and NOT equivalent in blast radius: one is a loud trap before
    // the write, the other is silent guest-memory corruption that happens to
    // be observed downstream. AGENTS.md's loud-trap rule decides that
    // tie -- this is the guard, not a redundant check.
    let prepared = writes
        .iter()
        .zip(payloads.iter())
        .enumerate()
        .map(|(index, (write, bytes))| {
            let payload_byte_count = u32::try_from(bytes.len())
                .expect("committed guest-write payload exceeds the RDP address width");
            assert_eq!(
                payload_byte_count,
                write.byte_count(),
                "committed guest write #{index} declares {} byte(s) but its payload is {}",
                write.byte_count(),
                bytes.len()
            );
            assert_eq!(
                fn64_render::ir_effect_content_digest(bytes),
                write.content(),
                "committed guest write #{index}'s payload does not hash to the ContentDigest the \
                 backend already committed for it"
            );
            let fn64_render::ir::ResourceRegion::Rdram { range, .. } = write.access().region()
            else {
                panic!(
                    "a committed guest render-target write must name an RDRAM region; \
                     commit_guest_render_target_writes admitted a write that does not"
                );
            };
            assert_eq!(
                range.layout(),
                registered_layout,
                "committed guest write range [{:#x}, {:#x}) was validated against a different \
                 physical memory layout",
                range.start().get(),
                range.end(),
            );
            assert_eq!(
                range.len(),
                payload_byte_count,
                "committed guest write range [{:#x}, {:#x}) spans {} byte(s) but its \
                 payload is {}",
                range.start().get(),
                range.end(),
                range.len(),
                bytes.len()
            );
            ValidatedGuestCopyback {
                addr: fn64_runtime::RdramAddr::from_offset(range.start().get()),
                bytes,
            }
        })
        .collect::<Vec<_>>();

    if renderer_copyback_batch_enabled() {
        // A committed submission is one writer transaction. Observing its
        // rows separately repeats catalog snapshot/diff work and exposes
        // intermediate row states that no guest instruction can observe.
        track_rdp_renderer_mutation(real, |real| {
            let mut view = fn64_runtime::RdramViewMut::from_storage(real);
            for write in &prepared {
                view.write_logical_bytes(write.addr, write.bytes);
            }
        });
    } else {
        for write in &prepared {
            track_rdp_renderer_mutation(real, |real| {
                fn64_runtime::RdramViewMut::from_storage(real)
                    .write_logical_bytes(write.addr, write.bytes);
            });
        }
    }
    renderer_copyback_census::record(
        census_started,
        prepared.len(),
        prepared.iter().map(|write| write.bytes.len()).sum(),
    );
}
