use core::fmt;

use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendEffectReport, CompletedWrite, GpuCompleteTicket, OperationId,
    QueueIdentity, RawCommandStream, RawStreamIdentity, RawTimelineEvent, RdramResource,
    ResourceAccess, ResourceRegion, SubmissionIdentity, SubmittedTicket, ValidationError,
    WorkloadAdmission, WorkloadIdentity,
};

pub const FILL_FIXTURE_WIDTH: u32 = 2;
pub const FILL_FIXTURE_HEIGHT: u32 = 2;
pub const FILL_FIXTURE_BYTES: u32 = FILL_FIXTURE_WIDTH * FILL_FIXTURE_HEIGHT * 4;
pub const FILL_FIXTURE_TEST_COLOR: [u8; 4] = [0x21, 0x3c, 0x4d, 0x59];
pub const FILL_FIXTURE_TEST_OUTPUT: [u8; FILL_FIXTURE_BYTES as usize] = [
    0x21, 0x3c, 0x4d, 0x59, 0x21, 0x3c, 0x4d, 0x59, 0x21, 0x3c, 0x4d, 0x59, 0x21, 0x3c, 0x4d, 0x59,
];

const SET_COLOR_IMAGE: u8 = 0x3f;
const SET_FILL_COLOR: u8 = 0x37;
const FILL_RECTANGLE: u8 = 0x36;
const FULL_SYNC: u8 = 0x29;
const SET_COLOR_IMAGE_WORD: u32 =
    (SET_COLOR_IMAGE as u32) << 24 | 3 << 19 | (FILL_FIXTURE_WIDTH - 1);
const FILL_RECTANGLE_WORD: u32 = (FILL_RECTANGLE as u32) << 24
    | ((FILL_FIXTURE_WIDTH - 1) * 4) << 12
    | ((FILL_FIXTURE_HEIGHT - 1) * 4);
const FULL_SYNC_WORD: u32 = (FULL_SYNC as u32) << 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeCompletionIdentity {
    semantic_queue: QueueIdentity,
    semantic_submission: SubmissionIdentity,
    semantic_ordinal: u64,
    native_ordinal: u64,
}

impl NativeCompletionIdentity {
    pub const fn semantic_queue(self) -> QueueIdentity {
        self.semantic_queue
    }

    pub const fn semantic_submission(self) -> SubmissionIdentity {
        self.semantic_submission
    }

    pub const fn semantic_ordinal(self) -> u64 {
        self.semantic_ordinal
    }

    pub const fn native_ordinal(self) -> u64 {
        self.native_ordinal
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct StagedWgpuEffect {
    access: ResourceAccess,
    bytes: Box<[u8]>,
}

impl StagedWgpuEffect {
    fn try_new(access: ResourceAccess, bytes: Vec<u8>) -> Result<Self, WgpuRenderError> {
        if !access.mode().writes() {
            return Err(WgpuRenderError::UndeclaredEffect {
                workload: None,
                reason: "staged effect names a read-only access",
            });
        }
        let actual = u32::try_from(bytes.len()).map_err(|_| WgpuRenderError::UndeclaredEffect {
            workload: None,
            reason: "staged effect byte length exceeds u32",
        })?;
        let expected = access.region().declared_bytes();
        if actual != expected {
            return Err(WgpuRenderError::EffectByteCount { expected, actual });
        }
        Ok(Self {
            access,
            bytes: bytes.into_boxed_slice(),
        })
    }

    pub const fn access(&self) -> ResourceAccess {
        self.access
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn completed_write(&self) -> CompletedWrite {
        CompletedWrite::try_from_bytes(self.access, &self.bytes)
            .expect("staged wgpu effect construction proves a writable exact-length access")
    }
}

#[derive(Debug)]
pub struct WgpuBackendCompletion {
    ticket: GpuCompleteTicket,
    staged_effects: Box<[StagedWgpuEffect]>,
    native: NativeCompletionIdentity,
}

impl WgpuBackendCompletion {
    pub const fn ticket(&self) -> &GpuCompleteTicket {
        &self.ticket
    }

    pub fn staged_effects(&self) -> &[StagedWgpuEffect] {
        &self.staged_effects
    }

    pub const fn native_completion(&self) -> NativeCompletionIdentity {
        self.native
    }

    pub fn into_parts(
        self,
    ) -> (
        GpuCompleteTicket,
        Box<[StagedWgpuEffect]>,
        NativeCompletionIdentity,
    ) {
        (self.ticket, self.staged_effects, self.native)
    }
}

#[derive(Debug)]
pub enum WgpuRenderError {
    RequestAdapter(String),
    RequestDevice(String),
    PipelinePrewarm(String),
    DevicePoisoned {
        count: usize,
        first: Option<String>,
    },
    UnsupportedAdmission {
        workload: WorkloadIdentity,
    },
    UnsupportedStreamCount {
        workload: WorkloadIdentity,
        actual: usize,
    },
    UnsupportedStream {
        workload: WorkloadIdentity,
        index: usize,
        kind: fn64_render_ir::RawStreamKind,
    },
    MalformedFixtureCommand {
        workload: WorkloadIdentity,
        stream: RawStreamIdentity,
        word: usize,
        expected: u32,
        actual: u32,
    },
    MissingFullSync {
        workload: WorkloadIdentity,
        stream: RawStreamIdentity,
    },
    InvalidFullSync {
        workload: WorkloadIdentity,
        stream: RawStreamIdentity,
        reason: &'static str,
    },
    UndeclaredEffect {
        workload: Option<WorkloadIdentity>,
        reason: &'static str,
    },
    EffectByteCount {
        expected: u32,
        actual: u32,
    },
    NativeSubmissionOrdinalExhausted,
    ExactSubmissionWait(String),
    CompletionCallbackNotObserved,
    Readback(String),
    OutputMismatch {
        expected: Box<[u8]>,
        actual: Box<[u8]>,
    },
    CompletionBindingMismatch {
        field: &'static str,
    },
    EarlyCompletion {
        missing: &'static str,
    },
    Ir(ValidationError),
}

impl fmt::Display for WgpuRenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestAdapter(reason) => {
                write!(formatter, "wgpu adapter request failed: {reason}")
            }
            Self::RequestDevice(reason) => write!(formatter, "wgpu device request failed: {reason}"),
            Self::PipelinePrewarm(reason) => {
                write!(formatter, "wgpu fill fixture pipeline prewarm failed: {reason}")
            }
            Self::DevicePoisoned { count, first } => write!(
                formatter,
                "wgpu device recorded {count} uncaptured errors; first={first:?}"
            ),
            Self::UnsupportedAdmission { workload } => {
                write!(formatter, "workload {workload} is not admitted as raw DPC")
            }
            Self::UnsupportedStreamCount { workload, actual } => write!(
                formatter,
                "workload {workload} has {actual} streams; the M3.1 fixture requires exactly one"
            ),
            Self::UnsupportedStream {
                workload,
                index,
                kind,
            } => write!(
                formatter,
                "workload {workload} stream {index} is {kind:?}; the M3.1 fixture requires DRAM"
            ),
            Self::MalformedFixtureCommand {
                workload,
                stream,
                word,
                expected,
                actual,
            } => write!(
                formatter,
                "workload {workload} stream {stream} word {word} is {actual:#010x}; expected {expected:#010x} for the exact M3.1 fixture"
            ),
            Self::MissingFullSync { workload, stream } => {
                write!(formatter, "workload {workload} stream {stream} has no FullSync")
            }
            Self::InvalidFullSync {
                workload,
                stream,
                reason,
            } => write!(
                formatter,
                "workload {workload} stream {stream} has invalid FullSync evidence: {reason}"
            ),
            Self::UndeclaredEffect { workload, reason } => {
                write!(formatter, "workload {workload:?} has unsupported effects: {reason}")
            }
            Self::EffectByteCount { expected, actual } => write!(
                formatter,
                "wgpu staged effect has {actual} bytes; journal declares {expected}"
            ),
            Self::NativeSubmissionOrdinalExhausted => {
                formatter.write_str("wgpu native submission ordinal exhausted")
            }
            Self::ExactSubmissionWait(reason) => {
                write!(formatter, "exact wgpu submission wait failed: {reason}")
            }
            Self::CompletionCallbackNotObserved => formatter.write_str(
                "wgpu completion callback was not observable after exact submission wait",
            ),
            Self::Readback(reason) => write!(formatter, "wgpu readback failed: {reason}"),
            Self::OutputMismatch { expected, actual } => write!(
                formatter,
                "wgpu fill output mismatch: expected {} bytes, observed {} bytes",
                expected.len(),
                actual.len()
            ),
            Self::CompletionBindingMismatch { field } => {
                write!(formatter, "wgpu completion belongs to a different {field}")
            }
            Self::EarlyCompletion { missing } => {
                write!(formatter, "wgpu completion attempted before {missing}")
            }
            Self::Ir(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for WgpuRenderError {}

impl From<ValidationError> for WgpuRenderError {
    fn from(error: ValidationError) -> Self {
        Self::Ir(error)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FillFixture {
    pub(crate) fill_rgba: [u8; 4],
    pub(crate) effect: ResourceAccess,
}

pub(crate) fn decode_fill_fixture(
    submitted: &SubmittedTicket,
) -> Result<FillFixture, WgpuRenderError> {
    let packet = submitted.packet();
    let workload = packet.identity();
    if !matches!(packet.admission(), WorkloadAdmission::RawDpc { .. }) {
        return Err(WgpuRenderError::UnsupportedAdmission { workload });
    }
    if packet.streams().len() != 1 {
        return Err(WgpuRenderError::UnsupportedStreamCount {
            workload,
            actual: packet.streams().len(),
        });
    }
    let RawCommandStream::Dram(stream) = &packet.streams()[0] else {
        return Err(WgpuRenderError::UnsupportedStream {
            workload,
            index: 0,
            kind: packet.streams()[0].kind(),
        });
    };
    let stream_identity = stream.identity();
    if stream.chunks().len() != 1 {
        return Err(WgpuRenderError::MalformedFixtureCommand {
            workload,
            stream: stream_identity,
            word: 0,
            expected: 1,
            actual: stream.chunks().len() as u32,
        });
    }
    let words = stream.chunks()[0].words();
    if words.len() != 8 {
        return Err(WgpuRenderError::MalformedFixtureCommand {
            workload,
            stream: stream_identity,
            word: words.len(),
            expected: 8,
            actual: words.len() as u32,
        });
    }
    for (index, expected) in [
        SET_COLOR_IMAGE_WORD,
        0,
        (SET_FILL_COLOR as u32) << 24,
        words[3],
        FILL_RECTANGLE_WORD,
        0,
        FULL_SYNC_WORD,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        if words[index] != expected {
            return Err(WgpuRenderError::MalformedFixtureCommand {
                workload,
                stream: stream_identity,
                word: index,
                expected,
                actual: words[index],
            });
        }
    }

    let full_syncs = stream.full_sync_occurrences();
    if full_syncs.is_empty() {
        return Err(WgpuRenderError::MissingFullSync {
            workload,
            stream: stream_identity,
        });
    }
    if full_syncs.len() != 1 {
        return Err(WgpuRenderError::InvalidFullSync {
            workload,
            stream: stream_identity,
            reason: "the M3.1 fixture requires exactly one FullSync",
        });
    }
    let sync = full_syncs[0];
    if sync.ordinal != 0
        || sync.stream_byte_offset != 24
        || sync.source_address != stream.chunks()[0].range().start().get() + 24
        || sync.chunk_index != 0
        || sync.chunk_byte_offset != 24
        || sync.sequence != 2
        || sync.interrupt_sequence != 3
        || sync.interrupt_before != fn64_render_ir::DpInterruptState::Clear
        || sync.interrupt_after != fn64_render_ir::DpInterruptState::Asserted
    {
        return Err(WgpuRenderError::InvalidFullSync {
            workload,
            stream: stream_identity,
            reason: "offset, chunk, or DP interrupt transition differs from the exact fixture",
        });
    }
    let timeline = stream.timeline();
    let [RawTimelineEvent::CmdEnd(cmd_end), RawTimelineEvent::FullSync(timeline_sync), RawTimelineEvent::DpInterrupt(dp)] =
        timeline.as_slice()
    else {
        return Err(WgpuRenderError::InvalidFullSync {
            workload,
            stream: stream_identity,
            reason: "timeline is not exactly CMD_END then FullSync then DP interrupt",
        });
    };
    if cmd_end.chunk_index != 0
        || cmd_end.sequence != 1
        || cmd_end.source_address != stream.chunks()[0].range().end()
        || cmd_end.interrupt != fn64_render_ir::DpInterruptState::Clear
        || timeline_sync != &sync
        || dp.full_sync_ordinal != 0
        || dp.sequence != 3
        || dp.before != fn64_render_ir::DpInterruptState::Clear
        || dp.after != fn64_render_ir::DpInterruptState::Asserted
    {
        return Err(WgpuRenderError::InvalidFullSync {
            workload,
            stream: stream_identity,
            reason: "CMD_END, FullSync, and DP observation identities or order differ from the exact fixture",
        });
    }

    let command_access = ResourceAccess::try_new(
        OperationId::new(0),
        AccessMode::Read,
        AccessPurpose::CommandDecode,
        ResourceRegion::Rdram {
            resource: RdramResource::RawCommands,
            range: stream.chunks()[0].range(),
        },
    )?;
    let effect = ResourceAccess::try_new(
        OperationId::new(1),
        AccessMode::Write,
        AccessPurpose::RenderTarget,
        ResourceRegion::Rdram {
            resource: RdramResource::ColorFramebuffer,
            range: packet.memory_layout().range(0, FILL_FIXTURE_BYTES)?,
        },
    )?;
    if packet.journal().accesses() != [command_access, effect] {
        return Err(WgpuRenderError::UndeclaredEffect {
            workload: Some(workload),
            reason: "the M3.1 fixture requires exactly ordered operation 0 command read and operation 1 framebuffer write accesses",
        });
    }

    Ok(FillFixture {
        fill_rgba: words[3].to_be_bytes(),
        effect,
    })
}

#[derive(Clone, Copy)]
pub(crate) struct CompletionBinding {
    queue: QueueIdentity,
    submission: SubmissionIdentity,
    semantic_ordinal: u64,
    native_ordinal: u64,
}

impl CompletionBinding {
    pub(crate) const fn from_submitted(submitted: &SubmittedTicket, native_ordinal: u64) -> Self {
        Self {
            queue: submitted.queue(),
            submission: submitted.identity(),
            semantic_ordinal: submitted.ordinal(),
            native_ordinal,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CompletionObservation {
    pub(crate) binding: CompletionBinding,
    pub(crate) exact_wait_complete: bool,
    pub(crate) callback_observed: bool,
    pub(crate) readback_complete: bool,
}

fn validate_completion(
    expected: CompletionBinding,
    actual: CompletionObservation,
) -> Result<NativeCompletionIdentity, WgpuRenderError> {
    for (matches, field) in [
        (expected.queue == actual.binding.queue, "semantic queue"),
        (
            expected.submission == actual.binding.submission,
            "semantic submission",
        ),
        (
            expected.semantic_ordinal == actual.binding.semantic_ordinal,
            "semantic submission ordinal",
        ),
        (
            expected.native_ordinal == actual.binding.native_ordinal,
            "native submission ordinal",
        ),
    ] {
        if !matches {
            return Err(WgpuRenderError::CompletionBindingMismatch { field });
        }
    }
    for (complete, missing) in [
        (actual.exact_wait_complete, "the exact indexed GPU wait"),
        (
            actual.callback_observed,
            "the exact submission completion callback",
        ),
        (actual.readback_complete, "the bounded GPU readback"),
    ] {
        if !complete {
            return Err(WgpuRenderError::EarlyCompletion { missing });
        }
    }
    Ok(NativeCompletionIdentity {
        semantic_queue: expected.queue,
        semantic_submission: expected.submission,
        semantic_ordinal: expected.semantic_ordinal,
        native_ordinal: expected.native_ordinal,
    })
}

pub(crate) fn finalize_completion(
    authority: &mut fn64_render_ir::BackendCompletionAuthority,
    submitted: SubmittedTicket,
    fixture: FillFixture,
    binding: CompletionBinding,
    observation: CompletionObservation,
    output: Vec<u8>,
) -> Result<WgpuBackendCompletion, WgpuRenderError> {
    let native = validate_completion(binding, observation)?;
    let expected = expected_output(fixture.fill_rgba);
    if output != expected {
        return Err(WgpuRenderError::OutputMismatch {
            expected: expected.into_boxed_slice(),
            actual: output.into_boxed_slice(),
        });
    }
    let staged = StagedWgpuEffect::try_new(fixture.effect, expected)?;
    let report = BackendEffectReport::try_new(submitted.packet(), vec![staged.completed_write()])?;
    let receipt = authority.issue(&submitted, report)?;
    let ticket = submitted.gpu_complete(receipt)?;
    Ok(WgpuBackendCompletion {
        ticket,
        staged_effects: vec![staged].into_boxed_slice(),
        native,
    })
}

pub(crate) fn expected_output(fill_rgba: [u8; 4]) -> Vec<u8> {
    fill_rgba.repeat((FILL_FIXTURE_WIDTH * FILL_FIXTURE_HEIGHT) as usize)
}

#[cfg(test)]
pub(crate) mod tests_support {
    use fn64_render_ir::{
        AccessMode, AccessPurpose, DecodedTicket, DpInterruptState, DramCommandChunk,
        DramCommandStream, FullSyncBoundary, OperationId, PhysicalMemoryLayout, RawCommandStream,
        RdramResource, ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion,
        TemporalBoundary, WorkloadAdmission, WorkloadPacket,
    };

    use super::{
        FILL_FIXTURE_BYTES, FILL_RECTANGLE_WORD, FULL_SYNC_WORD, SET_COLOR_IMAGE_WORD,
        SET_FILL_COLOR,
    };

    pub(crate) fn packet(
        color: [u8; 4],
        words_override: Option<Vec<u32>>,
        include_full_sync_observation: bool,
        include_effect: bool,
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let command_range = layout.range(0x100, 0x120).unwrap();
        let words = words_override.unwrap_or_else(|| {
            vec![
                SET_COLOR_IMAGE_WORD,
                0,
                (SET_FILL_COLOR as u32) << 24,
                u32::from_be_bytes(color),
                FILL_RECTANGLE_WORD,
                0,
                FULL_SYNC_WORD,
                0,
            ]
        });
        let full_syncs = if include_full_sync_observation {
            vec![FullSyncBoundary::new(
                2,
                3,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )]
        } else {
            Vec::new()
        };
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                words,
                TemporalBoundary::new(1, DpInterruptState::Clear),
                full_syncs,
            )
            .unwrap()])
            .unwrap(),
        );
        let mut accesses = vec![ResourceAccess::try_new(
            OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: command_range,
            },
        )
        .unwrap()];
        if include_effect {
            accesses.push(
                ResourceAccess::try_new(
                    OperationId::new(1),
                    AccessMode::Write,
                    AccessPurpose::RenderTarget,
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        range: layout.range(0, FILL_FIXTURE_BYTES).unwrap(),
                    },
                )
                .unwrap(),
            );
        }
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(4, 0x100).unwrap(), accesses)
                .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence: 7,
            },
            vec![stream],
            journal,
        )
        .unwrap()
    }

    pub(crate) fn submitted(
        color: [u8; 4],
    ) -> (
        fn64_render_ir::SubmittedTicket,
        fn64_render_ir::BackendCompletionAuthority,
    ) {
        let packet = packet(color, None, true, true);
        let (mut queue, backend, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        (submitted, backend)
    }
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::{
        DecodedTicket, DpInterruptState, DramCommandChunk, DramCommandStream, FullSyncBoundary,
        ResourceJournal, ResourceJournalLimits, TemporalBoundary, ValidationError, WorkloadPacket,
    };

    use super::tests_support::{packet, submitted};
    use super::*;

    fn packet_with_accesses(base: WorkloadPacket, accesses: Vec<ResourceAccess>) -> WorkloadPacket {
        let journal =
            ResourceJournal::try_new(ResourceJournalLimits::try_new(8, 0x1000).unwrap(), accesses)
                .unwrap();
        WorkloadPacket::try_new(
            base.memory_layout(),
            base.admission(),
            base.streams().to_vec(),
            journal,
        )
        .unwrap()
    }

    fn submit_packet(packet: WorkloadPacket) -> SubmittedTicket {
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        queue.submit(DecodedTicket::new(packet)).unwrap()
    }

    #[test]
    fn exact_fixture_decodes_fill_and_declared_effect() {
        let (submitted, _) = submitted(FILL_FIXTURE_TEST_COLOR);
        let fixture = decode_fill_fixture(&submitted).unwrap();
        assert_eq!(fixture.fill_rgba, FILL_FIXTURE_TEST_COLOR);
        assert_eq!(fixture.effect.region().declared_bytes(), FILL_FIXTURE_BYTES);
        assert_eq!(expected_output(fixture.fill_rgba), FILL_FIXTURE_TEST_OUTPUT);
    }

    #[test]
    fn missing_full_sync_is_loud() {
        let mut words = vec![
            SET_COLOR_IMAGE_WORD,
            0,
            (SET_FILL_COLOR as u32) << 24,
            0x0102_0304,
            FILL_RECTANGLE_WORD,
            0,
            0x2600_0000,
            0,
        ];
        let malformed_packet = packet([1, 2, 3, 4], Some(words.clone()), false, true);
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let submitted = queue.submit(DecodedTicket::new(malformed_packet)).unwrap();
        assert!(matches!(
            decode_fill_fixture(&submitted),
            Err(WgpuRenderError::MalformedFixtureCommand { word: 6, .. })
        ));

        words[6] = FULL_SYNC_WORD;
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let chunk = DramCommandChunk::try_new(
            layout.range(0x100, 0x120).unwrap(),
            words,
            TemporalBoundary::new(1, DpInterruptState::Clear),
            Vec::new(),
        )
        .unwrap();
        let error = DramCommandStream::try_new(vec![chunk]).unwrap_err();
        assert!(matches!(
            error,
            ValidationError::MissingFullSyncObservation {
                chunk_index: 0,
                occurrence: 0
            }
        ));
    }

    #[test]
    fn malformed_fixture_state_is_loud() {
        let mut words = vec![
            SET_COLOR_IMAGE_WORD,
            0,
            (SET_FILL_COLOR as u32) << 24,
            0x0102_0304,
            FILL_RECTANGLE_WORD,
            0,
            FULL_SYNC_WORD,
            0,
        ];
        words[4] ^= 4;
        let packet = packet([1, 2, 3, 4], Some(words), true, true);
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        assert!(matches!(
            decode_fill_fixture(&submitted),
            Err(WgpuRenderError::MalformedFixtureCommand { word: 4, .. })
        ));
    }

    #[test]
    fn undeclared_output_is_loud() {
        let packet = packet([1, 2, 3, 4], None, true, false);
        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
        assert!(matches!(
            decode_fill_fixture(&submitted),
            Err(WgpuRenderError::UndeclaredEffect { .. })
        ));
    }

    #[test]
    fn extra_valid_read_access_is_loud() {
        let base = packet([1, 2, 3, 4], None, true, true);
        let mut accesses = base.journal().accesses().to_vec();
        accesses.push(
            ResourceAccess::try_new(
                OperationId::new(2),
                AccessMode::Read,
                AccessPurpose::UploadSource,
                ResourceRegion::Rdram {
                    resource: RdramResource::Buffer,
                    range: base.memory_layout().range(0x200, 0x204).unwrap(),
                },
            )
            .unwrap(),
        );
        let submitted = submit_packet(packet_with_accesses(base, accesses));
        assert!(matches!(
            decode_fill_fixture(&submitted),
            Err(WgpuRenderError::UndeclaredEffect { .. })
        ));
    }

    #[test]
    fn reversed_journal_access_order_is_loud() {
        let base = packet([1, 2, 3, 4], None, true, true);
        let original = base.journal().accesses();
        let reversed = vec![
            ResourceAccess::try_new(
                OperationId::new(0),
                original[1].mode(),
                original[1].purpose(),
                original[1].region(),
            )
            .unwrap(),
            ResourceAccess::try_new(
                OperationId::new(1),
                original[0].mode(),
                original[0].purpose(),
                original[0].region(),
            )
            .unwrap(),
        ];
        let submitted = submit_packet(packet_with_accesses(base, reversed));
        assert!(matches!(
            decode_fill_fixture(&submitted),
            Err(WgpuRenderError::UndeclaredEffect { .. })
        ));
    }

    #[test]
    fn changed_journal_operation_id_is_loud() {
        let base = packet([1, 2, 3, 4], None, true, true);
        let original = base.journal().accesses();
        let changed = vec![
            original[0],
            ResourceAccess::try_new(
                OperationId::new(2),
                original[1].mode(),
                original[1].purpose(),
                original[1].region(),
            )
            .unwrap(),
        ];
        let submitted = submit_packet(packet_with_accesses(base, changed));
        assert!(matches!(
            decode_fill_fixture(&submitted),
            Err(WgpuRenderError::UndeclaredEffect { .. })
        ));
    }

    #[test]
    fn unknown_opcode_is_rejected_by_gpu_independent_admission() {
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let error = DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            layout.range(0x100, 0x108).unwrap(),
            vec![0x1000_0000, 0],
            TemporalBoundary::new(1, DpInterruptState::Clear),
            Vec::new(),
        )
        .unwrap()])
        .unwrap_err();
        assert!(matches!(error, ValidationError::UnknownRdpOpcode { .. }));
    }

    #[test]
    fn extra_or_mismatched_full_sync_evidence_is_rejected_before_backend() {
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let error = DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            layout.range(0x100, 0x108).unwrap(),
            vec![FULL_SYNC_WORD, 0],
            TemporalBoundary::new(1, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                2,
                3,
                DpInterruptState::Asserted,
                DpInterruptState::Asserted,
            )],
        )
        .unwrap()])
        .unwrap_err();
        assert_eq!(error, ValidationError::DiscontinuousDpInterruptObservation);
    }

    #[test]
    fn reordered_cmd_end_full_sync_dp_evidence_is_rejected_before_backend() {
        let layout = fn64_render_ir::PhysicalMemoryLayout::try_new(0x1000).unwrap();
        let error = DramCommandStream::try_new(vec![DramCommandChunk::try_new(
            layout.range(0x100, 0x108).unwrap(),
            vec![FULL_SYNC_WORD, 0],
            TemporalBoundary::new(2, DpInterruptState::Clear),
            vec![FullSyncBoundary::new(
                1,
                3,
                DpInterruptState::Clear,
                DpInterruptState::Asserted,
            )],
        )
        .unwrap()])
        .unwrap_err();
        assert!(matches!(
            error,
            ValidationError::NonMonotonicFullSyncSequence {
                prior: 2,
                full_sync: 1,
                interrupt: 3
            }
        ));
    }

    #[test]
    fn early_completion_and_wrong_bindings_are_rejected() {
        let (first_submission, _) = submitted([1, 2, 3, 4]);
        let expected = CompletionBinding::from_submitted(&first_submission, 9);
        let early = CompletionObservation {
            binding: expected,
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: false,
        };
        assert!(matches!(
            validate_completion(expected, early),
            Err(WgpuRenderError::EarlyCompletion { .. })
        ));

        let (other_queue_submission, _) = submitted([1, 2, 3, 4]);
        let wrong_queue = CompletionObservation {
            binding: CompletionBinding::from_submitted(&other_queue_submission, 9),
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: true,
        };
        assert!(matches!(
            validate_completion(expected, wrong_queue),
            Err(WgpuRenderError::CompletionBindingMismatch {
                field: "semantic queue"
            })
        ));

        let (mut queue, _, _) = fn64_render_ir::TicketAuthoritySet::try_new()
            .unwrap()
            .into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet([1, 2, 3, 4], None, true, true)))
            .unwrap();
        let second = queue
            .submit(DecodedTicket::new(packet([1, 2, 3, 4], None, true, true)))
            .unwrap();
        let same_queue_expected = CompletionBinding::from_submitted(&first, 9);
        let wrong_semantic_submission = CompletionObservation {
            binding: CompletionBinding::from_submitted(&second, 9),
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: true,
        };
        assert!(matches!(
            validate_completion(same_queue_expected, wrong_semantic_submission),
            Err(WgpuRenderError::CompletionBindingMismatch {
                field: "semantic submission"
            })
        ));

        let wrong_native = CompletionObservation {
            binding: CompletionBinding {
                native_ordinal: 10,
                ..expected
            },
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: true,
        };
        assert!(matches!(
            validate_completion(expected, wrong_native),
            Err(WgpuRenderError::CompletionBindingMismatch {
                field: "native submission ordinal"
            })
        ));
    }

    #[test]
    fn finalization_requires_exact_output_before_ir_receipt() {
        let (mismatched_submission, mut authority) = submitted([0x21, 0x3c, 0x4d, 0x59]);
        let fixture = decode_fill_fixture(&mismatched_submission).unwrap();
        let binding = CompletionBinding::from_submitted(&mismatched_submission, 0);
        let observation = CompletionObservation {
            binding,
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: true,
        };
        let mismatch = finalize_completion(
            &mut authority,
            mismatched_submission,
            fixture,
            binding,
            observation,
            vec![0; FILL_FIXTURE_BYTES as usize],
        )
        .unwrap_err();
        assert!(matches!(mismatch, WgpuRenderError::OutputMismatch { .. }));

        let (submitted, mut authority) = submitted([0x21, 0x3c, 0x4d, 0x59]);
        let fixture = decode_fill_fixture(&submitted).unwrap();
        let binding = CompletionBinding::from_submitted(&submitted, 1);
        let observation = CompletionObservation {
            binding,
            exact_wait_complete: true,
            callback_observed: true,
            readback_complete: true,
        };
        let completion = finalize_completion(
            &mut authority,
            submitted,
            fixture,
            binding,
            observation,
            expected_output(fixture.fill_rgba),
        )
        .unwrap();
        assert_eq!(
            completion.staged_effects()[0].bytes(),
            [0x21, 0x3c, 0x4d, 0x59].repeat(4)
        );
        assert_eq!(completion.native_completion().native_ordinal(), 1);
    }

    #[test]
    fn wgpu_effect_identity_satisfies_m1_2_guest_staging_shape() {
        use fn64_render::{IrGuestMemoryPreimage, IrRawDpcBackendCompletion, StagedIrRdramWrite};

        let (submitted, mut authority) = submitted(FILL_FIXTURE_TEST_COLOR);
        let preimage = IrGuestMemoryPreimage::try_capture(
            submitted.queue(),
            0,
            submitted.identity(),
            submitted.ordinal(),
            &[0; 0x1000],
        )
        .unwrap();
        let fixture = decode_fill_fixture(&submitted).unwrap();
        let binding = CompletionBinding::from_submitted(&submitted, 0);
        let completion = finalize_completion(
            &mut authority,
            submitted,
            fixture,
            binding,
            CompletionObservation {
                binding,
                exact_wait_complete: true,
                callback_observed: true,
                readback_complete: true,
            },
            FILL_FIXTURE_TEST_OUTPUT.to_vec(),
        )
        .unwrap();
        let (ticket, wgpu_effects, _) = completion.into_parts();
        let staged = wgpu_effects
            .iter()
            .map(|effect| {
                let m1_2 =
                    StagedIrRdramWrite::try_new(effect.access(), effect.bytes().to_vec()).unwrap();
                assert_eq!(effect.completed_write(), m1_2.completed_write());
                m1_2
            })
            .collect();
        let bridged = IrRawDpcBackendCompletion::try_new(ticket, preimage, staged).unwrap();
        assert_eq!(bridged.staged_guest_writes().len(), 1);
        assert_eq!(
            bridged.staged_guest_writes()[0].completed_write().content(),
            fn64_render_ir::effect_content_digest(&FILL_FIXTURE_TEST_OUTPUT)
        );
    }
}
