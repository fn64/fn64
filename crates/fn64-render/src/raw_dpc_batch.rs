//! Owned, preflighted raw-DPC batches.
//!
//! Public RDP command widths come from SGI *RDP Command Summary* Table 11.
//! A batch preserves each DPC submission boundary and source identity while
//! staging a diagnostic command image for a renderer call. The synthetic
//! suffix remains RDP-addressable and replay observes final guest memory, so
//! this transport is intentionally non-certifying.

use sha2::{Digest, Sha256};

use crate::{inspect_raw_rdp_full_sync, DpFullSyncStatus, RenderError};

const DPC_ALIGNMENT: u32 = 8;
const RSP_DMEM_BYTES: u32 = 0x1000;
const RDP_ADDRESS_BYTES: usize = 0x0100_0000;

/// Original memory interface selected for one accepted DPC range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RawDpcSource {
    Rdram,
    XbusDmem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawDpcSubmissionError {
    EmptyOrReversedRange {
        start: u32,
        end: u32,
    },
    UnalignedRange {
        start: u32,
        end: u32,
        alignment: u32,
    },
    SourceRangeOutOfBounds {
        source: RawDpcSource,
        start: u32,
        end: u32,
        upper_bound: u32,
    },
    CommandWordCount {
        expected: usize,
        actual: usize,
    },
    XbusPayloadLength {
        expected: usize,
        actual: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RawDpcCommands {
    Rdram(Box<[u32]>),
    XbusDmem(Box<[u8]>),
}

/// One owned DPC submission captured at the guest's CMD_END boundary.
///
/// RDRAM ranges retain canonical host-independent command words. XBUS ranges
/// retain exact logical big-endian DMEM bytes and derive words from that sole
/// representation, so source bytes and word identity cannot disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedRawDpcSubmission {
    source: RawDpcSource,
    start: u32,
    end: u32,
    commands: RawDpcCommands,
}

impl OwnedRawDpcSubmission {
    fn validate_range(
        source: RawDpcSource,
        start: u32,
        end: u32,
    ) -> Result<usize, RawDpcSubmissionError> {
        if start >= end {
            return Err(RawDpcSubmissionError::EmptyOrReversedRange { start, end });
        }
        if !start.is_multiple_of(DPC_ALIGNMENT) || !end.is_multiple_of(DPC_ALIGNMENT) {
            return Err(RawDpcSubmissionError::UnalignedRange {
                start,
                end,
                alignment: DPC_ALIGNMENT,
            });
        }
        let upper_bound = match source {
            RawDpcSource::Rdram => RDP_ADDRESS_BYTES as u32,
            RawDpcSource::XbusDmem => RSP_DMEM_BYTES,
        };
        if end > upper_bound {
            return Err(RawDpcSubmissionError::SourceRangeOutOfBounds {
                source,
                start,
                end,
                upper_bound,
            });
        }
        Ok((end - start) as usize)
    }

    pub fn from_rdram_words(
        start: u32,
        end: u32,
        words: Vec<u32>,
    ) -> Result<Self, RawDpcSubmissionError> {
        let bytes = Self::validate_range(RawDpcSource::Rdram, start, end)?;
        let expected = bytes / size_of::<u32>();
        if words.len() != expected {
            return Err(RawDpcSubmissionError::CommandWordCount {
                expected,
                actual: words.len(),
            });
        }
        Ok(Self {
            source: RawDpcSource::Rdram,
            start,
            end,
            commands: RawDpcCommands::Rdram(words.into_boxed_slice()),
        })
    }

    pub fn from_xbus_payload(
        start: u32,
        end: u32,
        payload: Vec<u8>,
    ) -> Result<Self, RawDpcSubmissionError> {
        let expected = Self::validate_range(RawDpcSource::XbusDmem, start, end)?;
        if payload.len() != expected {
            return Err(RawDpcSubmissionError::XbusPayloadLength {
                expected,
                actual: payload.len(),
            });
        }
        Ok(Self {
            source: RawDpcSource::XbusDmem,
            start,
            end,
            commands: RawDpcCommands::XbusDmem(payload.into_boxed_slice()),
        })
    }

    pub const fn source(&self) -> RawDpcSource {
        self.source
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn end(&self) -> u32 {
        self.end
    }

    pub fn xbus_payload(&self) -> Option<&[u8]> {
        match &self.commands {
            RawDpcCommands::Rdram(_) => None,
            RawDpcCommands::XbusDmem(payload) => Some(payload),
        }
    }

    pub fn command_words(&self) -> Vec<u32> {
        match &self.commands {
            RawDpcCommands::Rdram(words) => words.to_vec(),
            RawDpcCommands::XbusDmem(payload) => payload
                .chunks_exact(size_of::<u32>())
                .map(|word| u32::from_be_bytes(word.try_into().expect("four XBUS bytes")))
                .collect(),
        }
    }

    pub fn identity(&self) -> RawDpcSubmissionIdentity {
        let mut hasher = Sha256::new();
        match &self.commands {
            RawDpcCommands::Rdram(words) => {
                for word in words {
                    hasher.update(word.to_be_bytes());
                }
            }
            RawDpcCommands::XbusDmem(payload) => hasher.update(payload),
        }
        RawDpcSubmissionIdentity {
            source: self.source,
            start: self.start,
            end: self.end,
            command_sha256: hasher.finalize().into(),
        }
    }
}

/// Stable original-source/content identity for one ordered batch member.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RawDpcSubmissionIdentity {
    pub source: RawDpcSource,
    pub start: u32,
    pub end: u32,
    pub command_sha256: [u8; 32],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawDpcBatchPreflightError {
    Empty,
    RdramAddressSpaceExceeded { rdram_len: usize },
    StagingAddressOverflow,
    StagingAddressSpaceExceeded { staged_end: usize },
    InvalidStreamGroup { group: usize, error: String },
}

/// Ordered owned submissions before transport staging is bound to one RDRAM
/// allocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawDpcBatch(Box<[OwnedRawDpcSubmission]>);

impl RawDpcBatch {
    pub fn new(submissions: Vec<OwnedRawDpcSubmission>) -> Result<Self, RawDpcBatchPreflightError> {
        if submissions.is_empty() {
            return Err(RawDpcBatchPreflightError::Empty);
        }
        Ok(Self(submissions.into_boxed_slice()))
    }

    pub fn submissions(&self) -> &[OwnedRawDpcSubmission] {
        &self.0
    }

    /// Consume the source batch and validate every command boundary before a
    /// backend receives mutable access to either itself or guest memory.
    pub fn preflight(
        self,
        rdram_len: usize,
    ) -> Result<PreflightedRawDpcBatch, RawDpcBatchPreflightError> {
        if rdram_len > RDP_ADDRESS_BYTES {
            return Err(RawDpcBatchPreflightError::RdramAddressSpaceExceeded { rdram_len });
        }
        let staging_start = rdram_len
            .checked_add(
                (DPC_ALIGNMENT as usize - rdram_len % DPC_ALIGNMENT as usize)
                    % DPC_ALIGNMENT as usize,
            )
            .ok_or(RawDpcBatchPreflightError::StagingAddressOverflow)?;
        let staging_bytes = self.0.iter().try_fold(0usize, |total, submission| {
            total
                .checked_add((submission.end - submission.start) as usize)
                .ok_or(RawDpcBatchPreflightError::StagingAddressOverflow)
        })?;
        let staged_end = staging_start
            .checked_add(staging_bytes)
            .ok_or(RawDpcBatchPreflightError::StagingAddressOverflow)?;
        if staged_end > RDP_ADDRESS_BYTES {
            return Err(RawDpcBatchPreflightError::StagingAddressSpaceExceeded { staged_end });
        }

        let mut staged_commands = vec![0u8; staging_bytes];
        let mut cursor = 0usize;
        let mut submission_ends = Vec::with_capacity(self.0.len());
        for submission in self.0.iter() {
            let words = submission.command_words();
            let mut view = fn64_runtime::RdramViewMut::from_storage(&mut staged_commands);
            for word in words {
                view.write_u32(fn64_runtime::RdramAddr::from_offset(cursor as u32), word);
                cursor += size_of::<u32>();
            }
            submission_ends.push(cursor);
        }
        let mut groups = Vec::new();
        let mut first_submission = 0usize;
        let mut group_start = 0usize;
        let mut prior_end = self.0[0].end;
        for index in 1..=self.0.len() {
            // This is a conservative diagnostic transport heuristic, not a
            // reconstruction of silicon START/END state. Without a captured
            // continuation marker, only an explicitly contiguous range from
            // the same source may be concatenated.
            let continues = index < self.0.len()
                && self.0[index].source == self.0[index - 1].source
                && self.0[index].start == prior_end;
            if continues {
                prior_end = self.0[index].end;
                continue;
            }
            let group_end = submission_ends[index - 1];
            let full_sync =
                inspect_raw_rdp_full_sync(&staged_commands, group_start as u32, group_end as u32)
                    .map_err(|error| RawDpcBatchPreflightError::InvalidStreamGroup {
                    group: groups.len(),
                    error: error.to_string(),
                })?;
            groups.push(RawDpcStreamGroup {
                first_submission,
                submission_count: index - first_submission,
                source: self.0[first_submission].source,
                staging_start: staging_start as u32 + group_start as u32,
                staging_end: staging_start as u32 + group_end as u32,
                full_sync,
            });
            if index < self.0.len() {
                first_submission = index;
                group_start = group_end;
                prior_end = self.0[index].end;
            }
        }
        let identities = self.0.iter().map(OwnedRawDpcSubmission::identity).collect();
        Ok(PreflightedRawDpcBatch {
            physical_rdram_len: rdram_len,
            staging_start: staging_start as u32,
            staging_end: staged_end as u32,
            staged_commands: staged_commands.into_boxed_slice(),
            identities,
            groups: groups.into_boxed_slice(),
        })
    }
}

/// One-use proof that all source ranges, content lengths, command widths, and
/// the combined 24-bit staging range were accepted without mutation.
#[derive(Debug, PartialEq, Eq)]
pub struct PreflightedRawDpcBatch {
    physical_rdram_len: usize,
    staging_start: u32,
    staging_end: u32,
    staged_commands: Box<[u8]>,
    identities: Box<[RawDpcSubmissionIdentity]>,
    groups: Box<[RawDpcStreamGroup]>,
}

impl PreflightedRawDpcBatch {
    pub const fn physical_rdram_len(&self) -> usize {
        self.physical_rdram_len
    }

    pub const fn staging_start(&self) -> u32 {
        self.staging_start
    }

    pub const fn staging_end(&self) -> u32 {
        self.staging_end
    }

    pub fn identities(&self) -> &[RawDpcSubmissionIdentity] {
        &self.identities
    }

    pub fn stream_groups(&self) -> &[RawDpcStreamGroup] {
        &self.groups
    }

    pub fn aggregate_full_sync(&self) -> DpFullSyncStatus {
        if self
            .groups
            .iter()
            .any(|group| group.full_sync == DpFullSyncStatus::Reached)
        {
            DpFullSyncStatus::Reached
        } else {
            DpFullSyncStatus::NotReached
        }
    }

    /// Build a diagnostic renderer input image. The physical prefix is copied
    /// exactly, but the synthetic suffix remains visible in the RDP's 24-bit
    /// address space and therefore has no exact-runtime authority.
    pub fn staged_image(&self, rdram: &[u8]) -> Result<Vec<u8>, RenderError> {
        if rdram.len() != self.physical_rdram_len {
            return Err(RenderError::Backend {
                backend: "raw-dpc-batch",
                reason: format!(
                    "preflight is bound to {:#x} RDRAM bytes, but execution supplied {:#x}",
                    self.physical_rdram_len,
                    rdram.len()
                ),
            });
        }
        let mut image = vec![0; self.staging_end as usize];
        image[..rdram.len()].copy_from_slice(rdram);
        image[self.staging_start as usize..self.staging_end as usize]
            .copy_from_slice(&self.staged_commands);
        Ok(image)
    }

    pub fn outcome(&self) -> RawDpcBatchOutcome {
        RawDpcBatchOutcome {
            identities: self.identities.clone(),
            stream_groups: self.groups.clone(),
            full_sync: self.aggregate_full_sync(),
        }
    }
}

/// One conservatively grouped diagnostic command stream within a batch.
///
/// Same-source contiguous ranges are concatenated so a command may cross an
/// observed submission boundary. A source switch or discontinuity starts a
/// group because this type carries no captured silicon continuation marker.
/// The grouping is a transport heuristic, not evidence of a hardware command
/// boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawDpcStreamGroup {
    first_submission: usize,
    submission_count: usize,
    source: RawDpcSource,
    staging_start: u32,
    staging_end: u32,
    full_sync: DpFullSyncStatus,
}

impl RawDpcStreamGroup {
    pub const fn first_submission(self) -> usize {
        self.first_submission
    }

    pub const fn submission_count(self) -> usize {
        self.submission_count
    }

    pub const fn source(self) -> RawDpcSource {
        self.source
    }

    pub const fn staging_start(self) -> u32 {
        self.staging_start
    }

    pub const fn staging_end(self) -> u32 {
        self.staging_end
    }

    pub const fn full_sync(self) -> DpFullSyncStatus {
        self.full_sync
    }
}

/// Availability of the diagnostic staged-RDRAM raw-DPC adapter.
///
/// This adapter is not an exact hardware transaction boundary. Its staging
/// suffix is addressable by RDP commands, and replay starts from the final
/// guest-memory image rather than the memory/device state visible at each
/// original `CMD_END` write. It therefore cannot certify runtime execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RawDpcBatchCapability {
    #[default]
    Unsupported,
    /// A render-only diagnostic approximation is available. Callers must not
    /// use its result to publish exact guest or device state.
    DiagnosticOnly,
}

/// Diagnostic accepted members and aggregate FullSync observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawDpcBatchOutcome {
    pub identities: Box<[RawDpcSubmissionIdentity]>,
    pub stream_groups: Box<[RawDpcStreamGroup]>,
    /// FullSync evidence for the combined ordered FIFO. Submission boundaries
    /// are not command boundaries: one multiword command may straddle them.
    pub full_sync: DpFullSyncStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(opcode: u8) -> Vec<u32> {
        vec![u32::from(opcode) << 24, 0]
    }

    #[test]
    fn preflight_preserves_mixed_source_order_identity_and_full_sync() {
        let rdram = OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, words(0xe6)).unwrap();
        let xbus = OwnedRawDpcSubmission::from_xbus_payload(
            0x20,
            0x28,
            [0xe900_0000u32, 0]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect(),
        )
        .unwrap();
        let expected = [rdram.identity(), xbus.identity()];
        let batch = RawDpcBatch::new(vec![rdram, xbus])
            .unwrap()
            .preflight(0x101)
            .unwrap();

        assert_eq!(batch.staging_start(), 0x108);
        assert_eq!(batch.staging_end(), 0x118);
        assert_eq!(batch.identities(), expected);
        assert_eq!(batch.aggregate_full_sync(), DpFullSyncStatus::Reached);
        assert_eq!(batch.outcome().identities.as_ref(), expected);
    }

    #[test]
    fn xbus_words_are_derived_from_the_exact_owned_payload() {
        let payload = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let submission = OwnedRawDpcSubmission::from_xbus_payload(0, 8, payload.clone()).unwrap();
        assert_eq!(submission.xbus_payload(), Some(payload.as_slice()));
        assert_eq!(submission.command_words(), [0x1122_3344, 0x5566_7788]);
    }

    #[test]
    fn malformed_combined_stream_rejects_the_whole_preflight() {
        let first = OwnedRawDpcSubmission::from_rdram_words(0, 8, words(0xe6)).unwrap();
        let invalid = OwnedRawDpcSubmission::from_rdram_words(8, 16, words(0x7f)).unwrap();
        let error = RawDpcBatch::new(vec![first, invalid])
            .unwrap()
            .preflight(0x100)
            .unwrap_err();
        assert!(matches!(
            error,
            RawDpcBatchPreflightError::InvalidStreamGroup { group: 0, .. }
        ));
    }

    #[test]
    fn command_may_straddle_contiguous_same_source_submissions() {
        let first =
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, vec![0xe400_0000, 0]).unwrap();
        let second =
            OwnedRawDpcSubmission::from_rdram_words(0x108, 0x110, vec![0x1122_3344, 0x5566_7788])
                .unwrap();

        let batch = RawDpcBatch::new(vec![first, second])
            .unwrap()
            .preflight(0x200)
            .unwrap();

        assert_eq!(batch.identities().len(), 2);
        assert_eq!(batch.stream_groups().len(), 1);
        assert_eq!(batch.stream_groups()[0].submission_count(), 2);
        assert_eq!(batch.aggregate_full_sync(), DpFullSyncStatus::NotReached);
        let image = batch.staged_image(&vec![0; 0x200]).unwrap();
        assert_eq!(
            inspect_raw_rdp_full_sync(&image, batch.staging_start(), batch.staging_end()).unwrap(),
            DpFullSyncStatus::NotReached
        );
    }

    #[test]
    fn command_may_straddle_contiguous_xbus_submissions() {
        let first = OwnedRawDpcSubmission::from_xbus_payload(
            0x100,
            0x108,
            [0xe400_0000u32, 0]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect(),
        )
        .unwrap();
        let second = OwnedRawDpcSubmission::from_xbus_payload(
            0x108,
            0x110,
            [0x1122_3344u32, 0x5566_7788]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect(),
        )
        .unwrap();

        let batch = RawDpcBatch::new(vec![first, second])
            .unwrap()
            .preflight(0x200)
            .unwrap();

        assert_eq!(batch.stream_groups().len(), 1);
        assert_eq!(batch.stream_groups()[0].source(), RawDpcSource::XbusDmem);
        assert_eq!(batch.stream_groups()[0].submission_count(), 2);
    }

    #[test]
    fn source_transition_and_any_discontinuity_start_new_groups() {
        let submissions = vec![
            OwnedRawDpcSubmission::from_rdram_words(0x100, 0x108, words(0xe6)).unwrap(),
            OwnedRawDpcSubmission::from_rdram_words(0x200, 0x208, words(0xe7)).unwrap(),
            OwnedRawDpcSubmission::from_xbus_payload(
                0x20,
                0x28,
                words(0xe8).into_iter().flat_map(u32::to_be_bytes).collect(),
            )
            .unwrap(),
            OwnedRawDpcSubmission::from_xbus_payload(
                0x80,
                0x88,
                words(0xe9).into_iter().flat_map(u32::to_be_bytes).collect(),
            )
            .unwrap(),
        ];
        let batch = RawDpcBatch::new(submissions)
            .unwrap()
            .preflight(0x400)
            .unwrap();
        assert_eq!(batch.stream_groups().len(), 4);
        assert_eq!(batch.stream_groups()[0].submission_count(), 1);
        assert_eq!(batch.stream_groups()[1].submission_count(), 1);
        assert_eq!(batch.stream_groups()[2].submission_count(), 1);
        assert_eq!(batch.stream_groups()[3].submission_count(), 1);
        assert_eq!(
            batch.stream_groups()[3].full_sync(),
            DpFullSyncStatus::Reached
        );
    }

    #[test]
    fn staging_keeps_physical_prefix_and_uses_native_word_storage() {
        let submission =
            OwnedRawDpcSubmission::from_rdram_words(0x300, 0x308, words(0xe9)).unwrap();
        let batch = RawDpcBatch::new(vec![submission])
            .unwrap()
            .preflight(8)
            .unwrap();
        let physical = [0xa5; 8];
        let image = batch.staged_image(&physical).unwrap();
        assert_eq!(&image[..8], &physical);
        assert_eq!(
            inspect_raw_rdp_full_sync(&image, batch.staging_start(), batch.staging_end()).unwrap(),
            DpFullSyncStatus::Reached
        );
    }

    #[test]
    fn empty_and_overfull_batches_reject_before_staging() {
        assert_eq!(
            RawDpcBatch::new(Vec::new()).unwrap_err(),
            RawDpcBatchPreflightError::Empty
        );
        let submission = OwnedRawDpcSubmission::from_rdram_words(0, 8, words(0xe9)).unwrap();
        assert!(matches!(
            RawDpcBatch::new(vec![submission])
                .unwrap()
                .preflight(RDP_ADDRESS_BYTES),
            Err(RawDpcBatchPreflightError::StagingAddressSpaceExceeded { .. })
        ));
    }
}
