//! Backend-neutral renderer-conformance replay and verification types.
//!
//! Replay input is public, while expected observations and effects stay in a
//! separate verifier-private artifact. Only the orchestrator derives row state
//! after the Rust verifier has decoded the authoritative renderer IR.
//!
//! Evaluation authority is executable-private rather than a library API.
//!
//! ```compile_fail
//! use fn64_render_conformance::wire;
//! ```
#![forbid(unsafe_code)]

use core::fmt;
use fn64_render_ir::{
    ContentDigest, RawCommandStream, ValidationError, WorkloadPacket, WorkloadRecord,
};

pub const FIXTURE_SCHEMA: &str = "fn64.render-conformance.semantic-fixture.v2";
pub const RECEIPT_SCHEMA: &str = "fn64.render-conformance.receipt.v5";
pub const RUN_SERIES_SCHEMA: &str = "fn64.render-conformance.run-series.v4";
pub const PROCESS_RESULT_SCHEMA: &str = "fn64.render-conformance.process-result.v3";
pub const REQUIRED_CLEAN_RUNS: usize = 10;
pub const MAX_OBSERVABLE_BYTES: usize = 256 * 1024;
pub const MAX_ROW_ID_BYTES: usize = 96;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContractError {
    InvalidRowId,
    ObservableTooLarge { actual: usize, maximum: usize },
    EmptyFullSyncTimeline,
    ObservableLayerMismatch,
    ObservableMismatch,
    FixtureDigestMismatch,
    Ir(ValidationError),
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "renderer conformance contract rejected: {self:?}"
        )
    }
}

impl std::error::Error for ContractError {}

impl From<ValidationError> for ContractError {
    fn from(value: ValidationError) -> Self {
        Self::Ir(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RowId(Box<str>);

impl RowId {
    pub fn new(value: impl Into<String>) -> Result<Self, ContractError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_ROW_ID_BYTES
            || !value.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b':' | b'_')
            })
        {
            return Err(ContractError::InvalidRowId);
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum ObservableLayer {
    AdmittedCommandsState = 0,
    FullSyncTimeline = 1,
    TmemBytes = 2,
    ResourceJournalGuestMemoryEffects = 3,
    ShaderParameters = 4,
    FramebufferNative = 5,
    FramebufferHigh = 6,
    Vi = 7,
    PostViPixels = 8,
}

impl ObservableLayer {
    pub const ORDERED: [Self; 9] = [
        Self::AdmittedCommandsState,
        Self::FullSyncTimeline,
        Self::TmemBytes,
        Self::ResourceJournalGuestMemoryEffects,
        Self::ShaderParameters,
        Self::FramebufferNative,
        Self::FramebufferHigh,
        Self::Vi,
        Self::PostViPixels,
    ];

    const fn tag(self) -> u8 {
        self as u8
    }

    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::AdmittedCommandsState => "admitted_commands_state",
            Self::FullSyncTimeline => "full_sync_timeline",
            Self::TmemBytes => "tmem_bytes",
            Self::ResourceJournalGuestMemoryEffects => "resource_journal_guest_memory_effects",
            Self::ShaderParameters => "shader_parameters",
            Self::FramebufferNative => "framebuffer_native",
            Self::FramebufferHigh => "framebuffer_high",
            Self::Vi => "vi",
            Self::PostViPixels => "post_vi_pixels",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observable {
    layer: ObservableLayer,
    bytes: Box<[u8]>,
}

impl Observable {
    pub fn new(layer: ObservableLayer, bytes: Vec<u8>) -> Result<Self, ContractError> {
        if bytes.len() > MAX_OBSERVABLE_BYTES {
            return Err(ContractError::ObservableTooLarge {
                actual: bytes.len(),
                maximum: MAX_OBSERVABLE_BYTES,
            });
        }
        Ok(Self {
            layer,
            bytes: bytes.into_boxed_slice(),
        })
    }

    /// Exact record-bound expected FullSync observation. This is fixture data,
    /// not evidence that any backend produced the timeline.
    pub fn full_sync_timeline(record: &WorkloadRecord) -> Result<Self, ContractError> {
        if !record
            .streams()
            .iter()
            .any(|stream| !stream.full_sync_occurrences().is_empty())
        {
            return Err(ContractError::EmptyFullSyncTimeline);
        }
        Ok(Self {
            layer: ObservableLayer::FullSyncTimeline,
            bytes: record
                .record_identity()
                .as_bytes()
                .to_vec()
                .into_boxed_slice(),
        })
    }

    pub const fn layer(&self) -> ObservableLayer {
        self.layer
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn identity(&self) -> ContentDigest {
        ContentDigest::hash(
            b"fn64.render-conformance.observable.v2\0",
            &[&[self.layer.tag()], &self.bytes],
        )
    }
}

/// A replayable fixture built from the renderer's single authoritative IR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConformanceFixture {
    row_id: RowId,
    record: WorkloadRecord,
    streams: Box<[RawCommandStream]>,
    expected: Observable,
    digest: ContentDigest,
}

impl ConformanceFixture {
    pub fn try_new(
        row_id: RowId,
        packet: WorkloadPacket,
        expected: Observable,
    ) -> Result<Self, ContractError> {
        let record = WorkloadRecord::from_packet(&packet);
        let streams = packet.streams().to_vec().into_boxed_slice();
        let digest = fixture_digest(&row_id, &record, &expected);
        let fixture = Self {
            row_id,
            record,
            streams,
            expected,
            digest,
        };
        fixture.verify()?;
        Ok(fixture)
    }

    pub fn from_packet_full_sync(
        row_id: RowId,
        packet: WorkloadPacket,
    ) -> Result<Self, ContractError> {
        let record = WorkloadRecord::from_packet(&packet);
        let expected = Observable::full_sync_timeline(&record)?;
        Self::try_new(row_id, packet, expected)
    }

    pub fn row_id(&self) -> &RowId {
        &self.row_id
    }

    pub const fn record(&self) -> &WorkloadRecord {
        &self.record
    }

    pub const fn expected(&self) -> &Observable {
        &self.expected
    }

    pub const fn digest(&self) -> ContentDigest {
        self.digest
    }

    pub fn replay_packet(&self) -> Result<WorkloadPacket, ContractError> {
        Ok(self.record.replay(self.streams.to_vec())?)
    }

    pub fn verify(&self) -> Result<(), ContractError> {
        let packet = self.replay_packet()?;
        if packet.identity() != self.record.workload_identity()
            || self.record.record_identity()
                != WorkloadRecord::from_packet(&packet).record_identity()
            || self.digest != fixture_digest(&self.row_id, &self.record, &self.expected)
        {
            return Err(ContractError::FixtureDigestMismatch);
        }
        Ok(())
    }
}

fn fixture_digest(row: &RowId, record: &WorkloadRecord, expected: &Observable) -> ContentDigest {
    let encoded = record.encode();
    ContentDigest::hash(
        b"fn64.render-conformance.fixture.v2\0",
        &[
            row.as_str().as_bytes(),
            &encoded,
            expected.identity().as_ref(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fn64_render_ir::{
        AccessMode, AccessPurpose, DpInterruptState, DramCommandChunk, DramCommandStream,
        FullSyncBoundary, OperationId, PhysicalMemoryLayout, RawCommandStream, RdramResource,
        ResourceAccess, ResourceJournal, ResourceJournalLimits, ResourceRegion, TemporalBoundary,
        WorkloadAdmission,
    };

    fn packet(transaction: u64, guest_write: bool) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(0x2000).unwrap();
        let command_range = layout.range(0x100, 0x108).unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                vec![0xe9_u32 << 24, 0],
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
        if guest_write {
            accesses.push(
                ResourceAccess::try_new(
                    OperationId::new(1),
                    AccessMode::Write,
                    AccessPurpose::RenderTarget,
                    ResourceRegion::Rdram {
                        resource: RdramResource::ColorFramebuffer,
                        range: layout.range(0x400, 0x408).unwrap(),
                    },
                )
                .unwrap(),
            );
        }
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(accesses.len(), 16).unwrap(),
            accesses,
        )
        .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence: transaction,
            },
            vec![stream],
            journal,
        )
        .unwrap()
    }

    fn fixture(guest_write: bool) -> ConformanceFixture {
        ConformanceFixture::from_packet_full_sync(
            RowId::new("base::rdp-command-state-order").unwrap(),
            packet(7, guest_write),
        )
        .unwrap()
    }

    #[test]
    fn fixture_is_exact_render_ir_record_and_replay() {
        let fixture = fixture(false);
        let replay = fixture.replay_packet().unwrap();
        assert_eq!(replay.identity(), fixture.record().workload_identity());
        assert_eq!(
            WorkloadRecord::from_packet(&replay).record_identity(),
            fixture.record().record_identity()
        );
        let changed =
            ConformanceFixture::from_packet_full_sync(fixture.row_id().clone(), packet(8, false))
                .unwrap();
        assert_ne!(changed.digest(), fixture.digest());
    }

    #[test]
    fn observable_bounds_are_explicit() {
        assert_eq!(
            Observable::new(
                ObservableLayer::TmemBytes,
                vec![0; MAX_OBSERVABLE_BYTES + 1]
            ),
            Err(ContractError::ObservableTooLarge {
                actual: MAX_OBSERVABLE_BYTES + 1,
                maximum: MAX_OBSERVABLE_BYTES,
            })
        );
    }
}
