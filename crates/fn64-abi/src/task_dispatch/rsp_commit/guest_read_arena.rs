use super::*;

/// One captured raw-DPC source ready for the T4 plan/execute/publish
/// production seam: the exact original command words and their exact
/// source range/kind. Preserves the real capture source and ranges --
/// unlike the legacy `dispatch_captured_raw_rdp` staging path, this never
/// manufactures a synthetic RDRAM suffix.
///
/// Every admitted TMEM load's *source* bytes are declared as
/// `RdramResource::Buffer` regardless of `submission.source()`
/// (`crate::raw_dpc::production_adapter`'s push loop, mirroring real RDP
/// hardware: XBUS changes only where the RDP *command words* come from --
/// DMEM vs. DRAM -- never where `LoadBlock`/`LoadTile`/`LoadTLUT` read their
/// texel data, which is always the RDP's 24-bit physical RDRAM address
/// space). So there is exactly one guest-read byte source, live RDRAM, for
/// both producers; no separate DMEM byte source is needed or correct here.
pub(super) struct SessionRawDpcSource {
    pub(super) submission: fn64_render::OwnedRawDpcSubmission,
}

pub(super) fn build_task_batch_capture(
    real: &[u8],
    source: SessionRawDpcSource,
    token: u64,
) -> (
    fn64_render::OwnedRawDpcCapture,
    RspRdpObservationKind,
    usize,
) {
    let memory_layout = fn64_render::ir::PhysicalMemoryLayout::try_new(
        u32::try_from(real.len()).expect("registered RDRAM allocation fits a u32 byte length"),
    )
    .unwrap_or_else(|error| panic!("build_task_batch_capture: {error}"));
    let xbus = source.submission.source() == fn64_render::RawDpcSource::XbusDmem;
    let start = source.submission.start();
    let end = source.submission.end();
    let words = source.submission.command_words();
    maybe_dump_session_raw_dpc(&source.submission, &words, real);
    let cmd_end =
        fn64_render::ir::TemporalBoundary::new(token, fn64_render::ir::DpInterruptState::Clear);
    let full_sync_sites = fn64_render::count_raw_rdp_full_sync_sites(&words)
        .unwrap_or_else(|error| panic!("build_task_batch_capture: {error}"))
        .complete()
        .unwrap_or_else(|| {
            panic!("build_task_batch_capture received a command stream with an incomplete tail")
        });
    let capture = if full_sync_sites == 0 {
        fn64_render::OwnedRawDpcCapture::new(source.submission, memory_layout, token, cmd_end)
    } else {
        with_host(|host| {
            host.device_fabric
                .preflight_dp_full_sync(fn64_runtime::Cycles::new(1))
        })
        .unwrap_or_else(|error| panic!("task-batch DP FullSync completion: {error}"));
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
    (
        capture,
        dpc_observation(xbus, start, end, &words),
        full_sync_sites,
    )
}

pub(super) fn capture_task_batch_guest_reads(
    planned: &fn64_render::PlannedRawDpcSubmission,
    real: &[u8],
    history: &fn64_audio::rsp::runtime::RspDeferredDpcHistory,
    boundaries: &[CommandReadEpochBoundary],
) -> fn64_render::ir::DeferredGuestReadCapture {
    fn64_render::ir::DeferredGuestReadCapture::new(
        planned
            .guest_read_plan()
            .reads()
            .iter()
            .map(|read| {
                let range = read.range();
                let epoch = resolve_guest_read_epoch(*read, boundaries, history.current_epoch());
                let bytes = if epoch == history.current_epoch() {
                    fn64_runtime::RdramView::from_storage(real).read_logical_bytes(
                        fn64_runtime::RdramAddr::from_offset(range.start().get()),
                        range.len(),
                    )
                } else {
                    read_historical_logical_bytes(history, epoch, real, range)
                };
                fn64_render::ir::CapturedGuestRead::try_new(*read, bytes)
                    .unwrap_or_else(|error| panic!("CapturedGuestRead::try_new: {error}"))
            })
            .collect(),
    )
}

fn resolve_guest_read_epoch(
    read: fn64_render::ir::DeferredGuestRead,
    boundaries: &[CommandReadEpochBoundary],
    current_epoch: fn64_audio::rsp::runtime::RspRdramReadEpoch,
) -> fn64_audio::rsp::runtime::RspRdramReadEpoch {
    match read.moment() {
        fn64_render::ir::GuestReadMoment::PacketSnapshot => current_epoch,
        fn64_render::ir::GuestReadMoment::CommandCompletion(moment) => {
            assert_eq!(
                moment.stream_index(),
                0,
                "one coalesced RSP DPC run must produce exactly one raw-DPC stream"
            );
            boundaries
                .iter()
                .find(|boundary| {
                    boundary.command_end_byte_offset >= moment.command_end_byte_offset()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "raw-DPC command ending at stream byte {} lies beyond the final RSP DPC END boundary {}",
                        moment.command_end_byte_offset(),
                        boundaries
                            .last()
                            .map_or(0, |boundary| boundary.command_end_byte_offset)
                    )
                })
                .read_epoch
        }
    }
}

fn read_historical_logical_bytes(
    history: &fn64_audio::rsp::runtime::RspDeferredDpcHistory,
    epoch: fn64_audio::rsp::runtime::RspRdramReadEpoch,
    final_storage: &[u8],
    range: fn64_render::ir::PhysicalRange,
) -> Vec<u8> {
    let logical_start = usize::try_from(range.start().get())
        .expect("guest-read physical start fits the host address space");
    let logical_end =
        usize::try_from(range.end()).expect("guest-read physical end fits the host address space");
    let storage_start = logical_start & !3;
    let storage_end = logical_end
        .checked_add(3)
        .expect("guest-read storage hull overflow")
        & !3;
    let mut storage = vec![0; storage_end - storage_start];
    history
        .copy_storage_at(epoch, final_storage, storage_start, &mut storage)
        .unwrap_or_else(|error| panic!("historical RSP guest read rejected: {error:?}"));
    (logical_start..logical_end)
        .map(|address| {
            let storage_address = address ^ 3;
            storage[storage_address - storage_start]
        })
        .collect()
}

/// Task-scoped immutable payload sharing for exact physical guest-read ranges.
///
/// The guest coroutine remains suspended from planning through finalization,
/// and renderer publication begins only after every capture is complete.
/// Therefore no guest or renderer write can interleave two bindings of the
/// same range in this arena; both descriptors observe the same transaction
/// preimage while retaining their independent operation/access identities.
pub(super) struct TaskGuestReadCaptureArena<'a> {
    view: fn64_runtime::RdramView<'a>,
    final_storage: &'a [u8],
    history: &'a fn64_audio::rsp::runtime::RspDeferredDpcHistory,
    payloads: std::collections::HashMap<
        (
            fn64_audio::rsp::runtime::RspRdramReadEpoch,
            fn64_render::ir::PhysicalRange,
        ),
        fn64_render::ir::CapturedGuestReadPayload,
    >,
}

impl<'a> TaskGuestReadCaptureArena<'a> {
    pub(super) fn new(
        real: &'a [u8],
        history: &'a fn64_audio::rsp::runtime::RspDeferredDpcHistory,
    ) -> Self {
        Self {
            view: fn64_runtime::RdramView::from_storage(real),
            final_storage: real,
            history,
            payloads: std::collections::HashMap::new(),
        }
    }

    pub(super) fn capture(
        &mut self,
        plan: &fn64_render::ir::DeferredGuestReadPlan,
        boundaries: &[CommandReadEpochBoundary],
    ) -> fn64_render::ir::DeferredGuestReadCapture {
        let view = self.view;
        fn64_render::ir::DeferredGuestReadCapture::new(
            plan.reads()
                .iter()
                .map(|read| {
                    let range = read.range();
                    let epoch =
                        resolve_guest_read_epoch(*read, boundaries, self.history.current_epoch());
                    let payload = self.payloads.entry((epoch, range)).or_insert_with(|| {
                        let bytes = if epoch == self.history.current_epoch() {
                            view.read_logical_bytes(
                                fn64_runtime::RdramAddr::from_offset(range.start().get()),
                                range.len(),
                            )
                        } else {
                            read_historical_logical_bytes(
                                self.history,
                                epoch,
                                self.final_storage,
                                range,
                            )
                        };
                        fn64_render::ir::CapturedGuestReadPayload::try_new(*read, bytes)
                            .unwrap_or_else(|error| {
                                panic!("CapturedGuestReadPayload::try_new: {error}")
                            })
                    });
                    fn64_render::ir::CapturedGuestRead::try_from_payload(*read, payload)
                        .unwrap_or_else(|error| {
                            panic!("CapturedGuestRead::try_from_payload: {error}")
                        })
                })
                .collect(),
        )
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
