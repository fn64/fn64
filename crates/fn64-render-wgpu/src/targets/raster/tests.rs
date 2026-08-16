use fn64_render_ir::{
    AccessMode, AccessPurpose, DecodedTicket, DpInterruptState, DramCommandChunk,
    DramCommandStream, FullSyncBoundary, GuestCommitAuthority, OperationId, RawCommandStream,
    RdramResource, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
    TemporalBoundary, TicketAuthoritySet, WorkloadAdmission, WorkloadPacket,
};
#[cfg(feature = "host-gpu-tests")]
use fn64_render_ir::{CompletedWrite, GuestCommitEffectReport};

use super::*;
#[cfg(feature = "host-gpu-tests")]
use crate::NATIVE_FILL_N64RECOMP_STORAGE_RGBA16;
use crate::{
    decode_raw_dpc, prepare_native_fill, NativeDurableState, RdpState, NATIVE_FILL_COMMAND_END,
    NATIVE_FILL_COMMAND_START, NATIVE_FILL_COMMAND_WORDS, NATIVE_FILL_TARGET_END,
    NATIVE_FILL_TARGET_START, NATIVE_FILL_TRANSACTION_SEQUENCE,
};

fn lifecycle() -> (
    crate::DecodedRawDpc,
    BackendCompletionAuthority,
    GuestCommitAuthority,
) {
    let layout = PhysicalMemoryLayout::try_new(NATIVE_FILL_RDRAM_BYTES).unwrap();
    let command_range = layout
        .range(NATIVE_FILL_COMMAND_START, NATIVE_FILL_COMMAND_END)
        .unwrap();
    let stream = RawCommandStream::Dram(
        DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            command_range,
            NATIVE_FILL_COMMAND_WORDS.to_vec(),
            TemporalBoundary::new(1, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                2,
                3,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )],
        )
        .unwrap()])
        .unwrap(),
    );
    let target_range = layout
        .range(NATIVE_FILL_TARGET_START, NATIVE_FILL_TARGET_END)
        .unwrap();
    let journal = ResourceJournal::try_new(
        ResourceJournalLimits::try_new(2, NATIVE_FILL_RDRAM_BYTES).unwrap(),
        vec![
            ResourceAccess::try_new(
                OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap(),
            ResourceAccess::try_new(
                OperationId::new(1),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    range: target_range,
                },
            )
            .unwrap(),
        ],
    )
    .unwrap();
    let packet = WorkloadPacket::try_new(
        layout,
        WorkloadAdmission::RawDpc {
            transaction_sequence: NATIVE_FILL_TRANSACTION_SEQUENCE,
        },
        vec![stream],
        journal,
    )
    .unwrap();
    let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
    let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
    (
        decode_raw_dpc(submitted, &RdpState::default()).unwrap(),
        backend,
        guest,
    )
}

fn target_candidate(
    prepared: &PreparedNativeFill<'_>,
    registry: &ColorTargetRegistry,
) -> (CandidateColorTarget, ExactRowPlan, RasterCompletionBinding) {
    let target = prepared.target();
    let extent = ColorTargetExtent::try_new(target.width(), target.height()).unwrap();
    let key =
        ColorTargetKey::try_new(target.range().start(), extent, ColorTargetFormat::Rgba16).unwrap();
    let candidate = registry.begin_candidate(key).unwrap();
    let plan = candidate
        .plan_rows(TargetRectangle::try_new(0, 0, 4, 2).unwrap())
        .unwrap();
    let binding = RasterCompletionBinding {
        frame: prepared.binding(),
        key,
        generation: candidate.generation(),
        range: key.range(),
        native_ordinal: 9,
    };
    (candidate, plan, binding)
}

fn exact_readback() -> Vec<u8> {
    [
        NATIVE_FILL_DEVICE_RGBA16.as_slice(),
        NATIVE_FILL_POST_VI_BGRA8.as_slice(),
    ]
    .concat()
}

fn exact_observation(binding: RasterCompletionBinding) -> RasterCompletionObservation {
    RasterCompletionObservation {
        binding,
        exact_wait_complete: true,
        callback_observed: true,
        readback_complete: true,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadbackFailure {
    Poll,
    CallbackTimeout,
    MapResult,
    GetRange,
}

struct InjectedReadback {
    reusable: std::cell::Cell<bool>,
    failure: std::cell::Cell<Option<ReadbackFailure>>,
    unmaps: std::cell::Cell<usize>,
}

impl InjectedReadback {
    fn new() -> Self {
        Self {
            reusable: std::cell::Cell::new(true),
            failure: std::cell::Cell::new(None),
            unmaps: std::cell::Cell::new(0),
        }
    }

    fn arm(&self, failure: Option<ReadbackFailure>) {
        assert!(self.reusable.replace(false), "prior map was not cleaned up");
        self.failure.set(failure);
    }
}

impl MappedReadback for InjectedReadback {
    type Range = Box<[u8]>;

    fn wait_for_mapping(&self) -> Result<(), NativeRasterError> {
        assert!(!self.reusable.get());
        if self.failure.get() == Some(ReadbackFailure::Poll) {
            return Err(NativeRasterError::Readback("injected poll failure".into()));
        }
        Ok(())
    }

    fn observe_mapping(&self) -> Result<(), NativeRasterError> {
        match self.failure.get() {
            Some(ReadbackFailure::CallbackTimeout) => {
                Err(NativeRasterError::Readback("injected timeout".into()))
            }
            Some(ReadbackFailure::MapResult) => {
                Err(NativeRasterError::Readback("injected map result".into()))
            }
            _ => Ok(()),
        }
    }

    fn mapped_range(&self) -> Result<Self::Range, NativeRasterError> {
        if self.failure.get() == Some(ReadbackFailure::GetRange) {
            return Err(NativeRasterError::Readback(
                "injected mapped-range failure".into(),
            ));
        }
        Ok(vec![0x5a; READBACK_BYTES as usize].into_boxed_slice())
    }

    fn unmap(&self) {
        assert!(!self.reusable.replace(true), "map cleanup ran twice");
        self.unmaps.set(self.unmaps.get() + 1);
    }
}

#[cfg(feature = "host-gpu-tests")]
fn guest_commit(
    authority: &mut GuestCommitAuthority,
    ticket: fn64_render_ir::GpuCompleteTicket,
    bytes: &[u8],
) -> fn64_render_ir::GuestCommittedTicket {
    let writes = ticket
        .backend_writes()
        .iter()
        .map(|write| CompletedWrite::try_from_bytes(write.access(), bytes).unwrap())
        .collect();
    let effects = GuestCommitEffectReport::try_new(&ticket, writes).unwrap();
    let receipt = authority.issue(&ticket, effects).unwrap();
    ticket.commit_guest(receipt).unwrap()
}

#[test]
fn exact_completion_binds_every_target_identity_and_observation() {
    let (decoded, _, _) = lifecycle();
    let mut durable = NativeDurableState::default();
    let prepared = prepare_native_fill(decoded, &mut durable).unwrap();
    let registry = ColorTargetRegistry::try_new(
        PhysicalMemoryLayout::try_new(NATIVE_FILL_RDRAM_BYTES).unwrap(),
        1,
    )
    .unwrap();
    let (_, _, binding) = target_candidate(&prepared, &registry);

    for corruption in 0..7 {
        let mut observation = exact_observation(binding);
        match corruption {
            0 => observation.binding.native_ordinal += 1,
            1 => observation.binding.generation = TargetGeneration(2),
            2 => {
                observation.binding.range = observation
                    .binding
                    .key
                    .address()
                    .layout()
                    .range(0x408, 0x410)
                    .unwrap()
            }
            3 => observation.exact_wait_complete = false,
            4 => observation.callback_observed = false,
            5 => observation.readback_complete = false,
            6 => {
                let other_layout = PhysicalMemoryLayout::try_new(4 * 1024 * 1024).unwrap();
                observation.binding.key = ColorTargetKey::try_new(
                    other_layout.address(0x400).unwrap(),
                    ColorTargetExtent::try_new(4, 2).unwrap(),
                    ColorTargetFormat::Rgba16,
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
        let error = finish_exact_completion(binding, observation, exact_readback()).unwrap_err();
        if corruption < 3 || corruption == 6 {
            assert!(matches!(
                error,
                NativeRasterError::CompletionBindingMismatch { .. }
            ));
        } else {
            assert!(matches!(error, NativeRasterError::EarlyCompletion { .. }));
        }
    }
    assert_eq!(durable, NativeDurableState::default());
}

#[test]
fn hostile_readback_lengths_and_each_output_domain_fail_loudly() {
    let (decoded, _, _) = lifecycle();
    let mut durable = NativeDurableState::default();
    let prepared = prepare_native_fill(decoded, &mut durable).unwrap();
    let registry = ColorTargetRegistry::try_new(
        PhysicalMemoryLayout::try_new(NATIVE_FILL_RDRAM_BYTES).unwrap(),
        1,
    )
    .unwrap();
    let (_, _, binding) = target_candidate(&prepared, &registry);

    for length in [0, READBACK_BYTES as usize - 1, READBACK_BYTES as usize + 1] {
        assert!(matches!(
            finish_exact_completion(binding, exact_observation(binding), vec![0; length]),
            Err(NativeRasterError::OutputLength { .. })
        ));
    }

    for offset in [0, DEVICE_RGBA16_BYTES as usize] {
        let mut bytes = exact_readback();
        bytes[offset] ^= 1;
        let completion =
            finish_exact_completion(binding, exact_observation(binding), bytes).unwrap();
        let (candidate, plan, _) = target_candidate(&prepared, &registry);
        assert!(matches!(
            completion.into_outputs(
                candidate,
                plan,
                validate_exact_presentation(M3dPresentationSpec::exact_fixture()).unwrap()
            ),
            Err(NativeRasterError::OutputMismatch { .. })
        ));
    }
    assert_eq!(durable, NativeDurableState::default());
}

#[test]
fn every_post_map_failure_unmaps_before_the_same_readback_is_reused() {
    let readback = InjectedReadback::new();
    for (ordinal, failure) in [
        ReadbackFailure::Poll,
        ReadbackFailure::CallbackTimeout,
        ReadbackFailure::MapResult,
        ReadbackFailure::GetRange,
    ]
    .into_iter()
    .enumerate()
    {
        readback.arm(Some(failure));
        assert!(matches!(
            finish_mapped_readback(&readback),
            Err(NativeRasterError::Readback(_))
        ));
        assert!(readback.reusable.get());
        assert_eq!(readback.unmaps.get(), ordinal * 2 + 1);

        readback.arm(None);
        assert_eq!(
            finish_mapped_readback(&readback).unwrap(),
            vec![0x5a; READBACK_BYTES as usize]
        );
        assert!(readback.reusable.get());
        assert_eq!(readback.unmaps.get(), ordinal * 2 + 2);
    }
}

#[test]
fn dropping_prevalidated_publication_changes_neither_registry_nor_durable_state() {
    let (decoded, _, _) = lifecycle();
    let mut durable = NativeDurableState::default();
    let prepared = prepare_native_fill(decoded, &mut durable).unwrap();
    let mut registry = ColorTargetRegistry::try_new(
        PhysicalMemoryLayout::try_new(NATIVE_FILL_RDRAM_BYTES).unwrap(),
        1,
    )
    .unwrap();
    let (candidate, plan, binding) = target_candidate(&prepared, &registry);
    let completion =
        finish_exact_completion(binding, exact_observation(binding), exact_readback()).unwrap();
    let (initialized, _) = completion
        .into_outputs(
            candidate,
            plan,
            validate_exact_presentation(M3dPresentationSpec::exact_fixture()).unwrap(),
        )
        .unwrap();
    let publication = registry.prepare_publication(initialized).unwrap();
    drop(publication);
    assert!(registry.residents().is_empty());
    drop(prepared);
    assert_eq!(durable, NativeDurableState::default());
}

#[cfg(feature = "host-gpu-tests")]
#[test]
fn required_host_executes_exact_native_fill_through_guest_and_target_commit() {
    let (decoded, backend, mut guest) = lifecycle();
    let mut durable = NativeDurableState::default();
    let prepared = prepare_native_fill(decoded, &mut durable).unwrap();
    let requested =
        block_on(UninitializedNativeRaster::new(HeadlessBackend::AnyNative, backend).request())
            .unwrap();
    let mut renderer = match requested {
        NativeRasterDeviceOutcome::Ready(renderer) => renderer,
        NativeRasterDeviceOutcome::NoAdapter(no_adapter) => panic!(
            "required M3.3c GPU evidence unavailable: typed no-adapter for {:?}",
            no_adapter.requested()
        ),
    };
    let pending = renderer
        .submit_native_fill(prepared)
        .unwrap()
        .complete()
        .unwrap();
    assert_eq!(
        pending.guest_writeback_storage().n64recomp_storage_bytes(),
        NATIVE_FILL_N64RECOMP_STORAGE_RGBA16
    );
    let committed = pending
        .commit_guest::<fn64_render_ir::ValidationError>(|ticket, bytes| {
            Ok(guest_commit(
                &mut guest,
                ticket,
                bytes.n64recomp_storage_bytes(),
            ))
        })
        .unwrap();
    assert_eq!(committed.native_frame().durable_state().generation(), 1);
    assert_eq!(
        committed.resident_target().generation(),
        TargetGeneration::FIRST
    );
    assert_eq!(
        committed.resident_target().device_bytes().device_bytes(),
        NATIVE_FILL_DEVICE_RGBA16
    );
    assert_eq!(
        committed.native_frame().post_vi_bgra8(),
        NATIVE_FILL_POST_VI_BGRA8
    );
    drop(committed);
    assert_eq!(renderer.resident_targets().len(), 1);
    assert_eq!(durable.generation(), 1);
}

#[cfg(feature = "host-gpu-tests")]
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};

    struct ThreadWake(std::thread::Thread);

    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}
