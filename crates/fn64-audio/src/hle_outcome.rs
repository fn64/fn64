//! Pointer-free task outcomes for exact audio HLE/LLE comparison.
//!
//! This module deliberately models effects rather than execution policy. Both
//! lanes must produce one value from the same task-entry snapshot; callers may
//! commit an outcome only after [`compare_audio_task_outcomes`] returns `Ok`.

use core::num::NonZeroU64;
use fn64_runtime::rdram::DEFAULT_RDRAM_SIZE;
use sha2::{Digest, Sha256};

pub const RSP_BANK_BYTES: usize = 0x1000;

/// A SHA-256 value whose fixed size cannot be confused with arbitrary bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }

    pub fn hash(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }
}

/// Exact content identity required before an audio command HLE is selectable.
///
/// The IMEM digest covers the complete task-entry bank, not merely the declared
/// ucode text length. The data identity is length-bound so two differently
/// sized images with the same prefix cannot alias one catalog entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioMicrocodeIdentity {
    pub imem_sha256: Sha256Digest,
    pub ucode_data_bytes: u32,
    pub ucode_data_sha256: Sha256Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioMicrocodeIdentityError {
    UcodeDataLengthExceedsU32 { byte_len: usize },
}

impl AudioMicrocodeIdentity {
    pub fn from_task_entry(
        imem: &[u8; RSP_BANK_BYTES],
        ucode_data: &[u8],
    ) -> Result<Self, AudioMicrocodeIdentityError> {
        let ucode_data_bytes = u32::try_from(ucode_data.len()).map_err(|_| {
            AudioMicrocodeIdentityError::UcodeDataLengthExceedsU32 {
                byte_len: ucode_data.len(),
            }
        })?;
        Ok(Self {
            imem_sha256: Sha256Digest::hash(imem),
            ucode_data_bytes,
            ucode_data_sha256: Sha256Digest::hash(ucode_data),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AudioHleFamily {
    StandardAbi,
    CompactAbi,
}

/// Exact microcode and HLE implementation selected by catalog admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AudioHleSelection {
    pub microcode: AudioMicrocodeIdentity,
    pub family: AudioHleFamily,
    pub implementation_revision: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioTaskTerminalReason {
    Broke,
    StepLimit,
    UnsupportedInstruction,
    ImemOverrun,
    UnhandledJumpTarget,
    PendingOverlaySwap,
    UnhandledResumeTarget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RdramRangeError {
    Empty,
    AddressOverflow {
        start: u32,
        byte_len: u32,
    },
    OutOfBounds {
        start: u32,
        byte_len: u32,
        rdram_bytes: u32,
    },
}

/// A nonempty half-open guest RDRAM byte range with a proven `u32` end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RdramByteRange {
    start: u32,
    byte_len: u32,
}

impl RdramByteRange {
    pub fn new(start: u32, byte_len: u32) -> Result<Self, RdramRangeError> {
        if byte_len == 0 {
            return Err(RdramRangeError::Empty);
        }
        let end = start
            .checked_add(byte_len)
            .ok_or(RdramRangeError::AddressOverflow { start, byte_len })?;
        let rdram_bytes =
            u32::try_from(DEFAULT_RDRAM_SIZE).expect("physical RDRAM size must fit u32");
        if end > rdram_bytes {
            return Err(RdramRangeError::OutOfBounds {
                start,
                byte_len,
                rdram_bytes,
            });
        }
        Ok(Self { start, byte_len })
    }

    pub const fn start(self) -> u32 {
        self.start
    }

    pub const fn byte_len(self) -> u32 {
        self.byte_len
    }

    pub const fn end(self) -> u32 {
        // Construction proves this addition cannot overflow.
        self.start + self.byte_len
    }

    pub const fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end() <= self.end()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RdramPatchError {
    Range(RdramRangeError),
    ByteLengthExceedsU32 { byte_len: usize },
}

/// Final bytes written to one contiguous RDRAM range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RdramPatch {
    range: RdramByteRange,
    bytes: Vec<u8>,
}

impl RdramPatch {
    pub fn new(start: u32, bytes: Vec<u8>) -> Result<Self, RdramPatchError> {
        let byte_len =
            u32::try_from(bytes.len()).map_err(|_| RdramPatchError::ByteLengthExceedsU32 {
                byte_len: bytes.len(),
            })?;
        let range = RdramByteRange::new(start, byte_len).map_err(RdramPatchError::Range)?;
        Ok(Self { range, bytes })
    }

    pub const fn range(&self) -> RdramByteRange {
        self.range
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonicalRdramError {
    Overlap {
        previous: RdramByteRange,
        next: RdramByteRange,
    },
}

/// Sorted, disjoint RDRAM patches with adjacent patches merged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalRdramPatches(Vec<RdramPatch>);

impl CanonicalRdramPatches {
    pub fn new(mut patches: Vec<RdramPatch>) -> Result<Self, CanonicalRdramError> {
        patches.sort_unstable_by_key(|patch| patch.range.start());
        let mut canonical: Vec<RdramPatch> = Vec::with_capacity(patches.len());
        for patch in patches {
            let Some(previous) = canonical.last_mut() else {
                canonical.push(patch);
                continue;
            };
            if patch.range.start() < previous.range.end() {
                return Err(CanonicalRdramError::Overlap {
                    previous: previous.range,
                    next: patch.range,
                });
            }
            if patch.range.start() == previous.range.end() {
                previous.bytes.extend_from_slice(&patch.bytes);
                previous.range.byte_len += patch.range.byte_len;
            } else {
                canonical.push(patch);
            }
        }
        Ok(Self(canonical))
    }

    pub fn as_slice(&self) -> &[RdramPatch] {
        &self.0
    }

    fn covers(&self, range: RdramByteRange) -> bool {
        self.0.iter().any(|patch| patch.range.contains(range))
    }
}

/// Sorted, disjoint RDRAM ranges with adjacent ranges merged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CanonicalRdramRanges(Vec<RdramByteRange>);

impl CanonicalRdramRanges {
    pub fn new(mut ranges: Vec<RdramByteRange>) -> Result<Self, CanonicalRdramError> {
        ranges.sort_unstable_by_key(|range| range.start());
        let mut canonical: Vec<RdramByteRange> = Vec::with_capacity(ranges.len());
        for range in ranges {
            let Some(previous) = canonical.last_mut() else {
                canonical.push(range);
                continue;
            };
            if range.start() < previous.end() {
                return Err(CanonicalRdramError::Overlap {
                    previous: *previous,
                    next: range,
                });
            }
            if range.start() == previous.end() {
                previous.byte_len += range.byte_len;
            } else {
                canonical.push(range);
            }
        }
        Ok(Self(canonical))
    }

    pub fn as_slice(&self) -> &[RdramByteRange] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RspDmaRegisterState {
    pub mem_address: u32,
    pub dram_address: u32,
    pub read_length: u32,
    pub write_length: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RspDpcRegisterState {
    pub start: u32,
    pub end: u32,
    pub current: u32,
    pub status: u32,
    pub clock: u32,
    pub command_busy: u32,
    pub pipe_busy: u32,
    pub tmem_busy: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcSubmissionSource {
    Rdram,
    Dmem,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcSubmissionError {
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
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
        upper_bound: u32,
    },
    RdramCommandWordCount {
        expected: usize,
        actual: usize,
    },
    DmemPayloadLength {
        expected: usize,
        actual: usize,
    },
}

/// Stable content identity derived from one deferred DPC submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DpcSubmissionIdentity {
    pub source: DpcSubmissionSource,
    pub start: u32,
    pub end: u32,
    pub command_sha256: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DeferredDpcCommands {
    /// Canonical host-independent command words for an RDRAM-backed range.
    Rdram(Vec<u32>),
    /// Logical big-endian bytes captured at the XBUS CMD_END boundary.
    Dmem(Vec<u8>),
}

/// One ordered DPC effect retained until an accepted lane is committed.
///
/// RDRAM-backed submissions retain canonical command words, while XBUS
/// submissions retain the exact DMEM payload that existed when CMD_END was
/// accepted. XBUS words and every comparison identity are derived from that
/// payload, so no second owned representation can disagree with it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredDpcSubmission {
    source: DpcSubmissionSource,
    start: u32,
    end: u32,
    commands: DeferredDpcCommands,
}

impl DeferredDpcSubmission {
    const ALIGNMENT: u32 = 8;
    const RDRAM_ADDRESS_BYTES: u32 = 0x0100_0000;

    fn validate_range(
        source: DpcSubmissionSource,
        start: u32,
        end: u32,
    ) -> Result<usize, DpcSubmissionError> {
        if start >= end {
            return Err(DpcSubmissionError::EmptyOrReversedRange { start, end });
        }
        if !start.is_multiple_of(Self::ALIGNMENT) || !end.is_multiple_of(Self::ALIGNMENT) {
            return Err(DpcSubmissionError::UnalignedRange {
                start,
                end,
                alignment: Self::ALIGNMENT,
            });
        }
        let upper_bound = match source {
            DpcSubmissionSource::Rdram => Self::RDRAM_ADDRESS_BYTES,
            DpcSubmissionSource::Dmem => RSP_BANK_BYTES as u32,
        };
        if end > upper_bound {
            return Err(DpcSubmissionError::SourceRangeOutOfBounds {
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
        command_words: Vec<u32>,
    ) -> Result<Self, DpcSubmissionError> {
        let byte_len = Self::validate_range(DpcSubmissionSource::Rdram, start, end)?;
        let expected = byte_len / core::mem::size_of::<u32>();
        if command_words.len() != expected {
            return Err(DpcSubmissionError::RdramCommandWordCount {
                expected,
                actual: command_words.len(),
            });
        }
        Ok(Self {
            source: DpcSubmissionSource::Rdram,
            start,
            end,
            commands: DeferredDpcCommands::Rdram(command_words),
        })
    }

    pub fn from_dmem_payload(
        start: u32,
        end: u32,
        payload: Vec<u8>,
    ) -> Result<Self, DpcSubmissionError> {
        let expected = Self::validate_range(DpcSubmissionSource::Dmem, start, end)?;
        if payload.len() != expected {
            return Err(DpcSubmissionError::DmemPayloadLength {
                expected,
                actual: payload.len(),
            });
        }
        Ok(Self {
            source: DpcSubmissionSource::Dmem,
            start,
            end,
            commands: DeferredDpcCommands::Dmem(payload),
        })
    }

    pub const fn source(&self) -> DpcSubmissionSource {
        self.source
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn end(&self) -> u32 {
        self.end
    }

    /// Exact logical XBUS bytes captured at submission time.
    pub fn xbus_payload(&self) -> Option<&[u8]> {
        match &self.commands {
            DeferredDpcCommands::Rdram(_) => None,
            DeferredDpcCommands::Dmem(payload) => Some(payload),
        }
    }

    /// Canonical host-independent command words for deferred rendering.
    pub fn command_words(&self) -> Vec<u32> {
        match &self.commands {
            DeferredDpcCommands::Rdram(words) => words.clone(),
            DeferredDpcCommands::Dmem(payload) => payload
                .chunks_exact(core::mem::size_of::<u32>())
                .map(|word| u32::from_be_bytes(word.try_into().expect("four XBUS bytes")))
                .collect(),
        }
    }

    pub fn identity(&self) -> DpcSubmissionIdentity {
        let mut hasher = Sha256::new();
        match &self.commands {
            DeferredDpcCommands::Rdram(words) => {
                for word in words {
                    hasher.update(word.to_be_bytes());
                }
            }
            DeferredDpcCommands::Dmem(payload) => hasher.update(payload),
        }
        DpcSubmissionIdentity {
            source: self.source,
            start: self.start,
            end: self.end,
            command_sha256: Sha256Digest::new(hasher.finalize().into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RspVisibleStateError {
    InvalidPc(u32),
}

/// Guest-visible RSP state at the task's terminal boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RspVisibleState {
    pub dmem: [u8; RSP_BANK_BYTES],
    pub imem: [u8; RSP_BANK_BYTES],
    pub imem_generation: u64,
    sp_pc: u32,
    pub sp_status: u32,
    pub sp_semaphore: bool,
    pub dma: RspDmaRegisterState,
    pub dpc: RspDpcRegisterState,
    pub dpc_submissions: Vec<DeferredDpcSubmission>,
}

impl RspVisibleState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dmem: [u8; RSP_BANK_BYTES],
        imem: [u8; RSP_BANK_BYTES],
        imem_generation: u64,
        sp_pc: u32,
        sp_status: u32,
        sp_semaphore: bool,
        dma: RspDmaRegisterState,
        dpc: RspDpcRegisterState,
        dpc_submissions: Vec<DeferredDpcSubmission>,
    ) -> Result<Self, RspVisibleStateError> {
        if sp_pc > 0x0ffc || !sp_pc.is_multiple_of(4) {
            return Err(RspVisibleStateError::InvalidPc(sp_pc));
        }
        Ok(Self {
            dmem,
            imem,
            imem_generation,
            sp_pc,
            sp_status,
            sp_semaphore,
            dma,
            dpc,
            dpc_submissions,
        })
    }

    pub const fn sp_pc(&self) -> u32 {
        self.sp_pc
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioTaskOutcomeError {
    PcmRangeNotWritten { range: RdramByteRange },
}

/// Complete task effect before any mutation is committed to the live runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioTaskOutcome {
    selection: AudioHleSelection,
    terminal: AudioTaskTerminalReason,
    rdram_patches: CanonicalRdramPatches,
    pcm_ranges: CanonicalRdramRanges,
    rsp: RspVisibleState,
    completion_steps: NonZeroU64,
}

impl AudioTaskOutcome {
    pub fn new(
        selection: AudioHleSelection,
        terminal: AudioTaskTerminalReason,
        rdram_patches: CanonicalRdramPatches,
        pcm_ranges: CanonicalRdramRanges,
        rsp: RspVisibleState,
        completion_steps: NonZeroU64,
    ) -> Result<Self, AudioTaskOutcomeError> {
        for &range in pcm_ranges.as_slice() {
            if !rdram_patches.covers(range) {
                return Err(AudioTaskOutcomeError::PcmRangeNotWritten { range });
            }
        }
        Ok(Self {
            selection,
            terminal,
            rdram_patches,
            pcm_ranges,
            rsp,
            completion_steps,
        })
    }

    pub const fn selection(&self) -> AudioHleSelection {
        self.selection
    }

    pub const fn terminal(&self) -> AudioTaskTerminalReason {
        self.terminal
    }

    pub const fn rdram_patches(&self) -> &CanonicalRdramPatches {
        &self.rdram_patches
    }

    pub const fn pcm_ranges(&self) -> &CanonicalRdramRanges {
        &self.pcm_ranges
    }

    pub const fn rsp(&self) -> &RspVisibleState {
        &self.rsp
    }

    pub const fn completion_steps(&self) -> NonZeroU64 {
        self.completion_steps
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DmaRegister {
    MemAddress,
    DramAddress,
    ReadLength,
    WriteLength,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpcRegister {
    Start,
    End,
    Current,
    Status,
    Clock,
    CommandBusy,
    PipeBusy,
    TmemBusy,
}

/// The first difference in the canonical outcome comparison order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AudioTaskOutcomeMismatch {
    MicrocodeImemIdentity {
        reference: Sha256Digest,
        candidate: Sha256Digest,
    },
    MicrocodeDataLength {
        reference: u32,
        candidate: u32,
    },
    MicrocodeDataIdentity {
        reference: Sha256Digest,
        candidate: Sha256Digest,
    },
    HleFamily {
        reference: AudioHleFamily,
        candidate: AudioHleFamily,
    },
    HleImplementationRevision {
        reference: u32,
        candidate: u32,
    },
    TerminalReason {
        reference: AudioTaskTerminalReason,
        candidate: AudioTaskTerminalReason,
    },
    RdramPatchRange {
        index: usize,
        reference: Option<RdramByteRange>,
        candidate: Option<RdramByteRange>,
    },
    RdramPatchByte {
        address: u32,
        reference: u8,
        candidate: u8,
    },
    PcmRange {
        index: usize,
        reference: Option<RdramByteRange>,
        candidate: Option<RdramByteRange>,
    },
    DmemByte {
        offset: u16,
        reference: u8,
        candidate: u8,
    },
    ImemByte {
        offset: u16,
        reference: u8,
        candidate: u8,
    },
    ImemGeneration {
        reference: u64,
        candidate: u64,
    },
    SpPc {
        reference: u32,
        candidate: u32,
    },
    SpStatus {
        reference: u32,
        candidate: u32,
    },
    SpSemaphore {
        reference: bool,
        candidate: bool,
    },
    DmaRegister {
        register: DmaRegister,
        reference: u32,
        candidate: u32,
    },
    DpcRegister {
        register: DpcRegister,
        reference: u32,
        candidate: u32,
    },
    DpcSubmissionCount {
        reference: usize,
        candidate: usize,
    },
    DpcSubmission {
        index: usize,
        reference: DpcSubmissionIdentity,
        candidate: DpcSubmissionIdentity,
    },
    CompletionSteps {
        reference: NonZeroU64,
        candidate: NonZeroU64,
    },
}

/// Compare two task effects in a stable, first-divergence order.
pub fn compare_audio_task_outcomes(
    reference: &AudioTaskOutcome,
    candidate: &AudioTaskOutcome,
) -> Result<(), AudioTaskOutcomeMismatch> {
    if reference.selection.microcode.imem_sha256 != candidate.selection.microcode.imem_sha256 {
        return Err(AudioTaskOutcomeMismatch::MicrocodeImemIdentity {
            reference: reference.selection.microcode.imem_sha256,
            candidate: candidate.selection.microcode.imem_sha256,
        });
    }
    if reference.selection.microcode.ucode_data_bytes
        != candidate.selection.microcode.ucode_data_bytes
    {
        return Err(AudioTaskOutcomeMismatch::MicrocodeDataLength {
            reference: reference.selection.microcode.ucode_data_bytes,
            candidate: candidate.selection.microcode.ucode_data_bytes,
        });
    }
    if reference.selection.microcode.ucode_data_sha256
        != candidate.selection.microcode.ucode_data_sha256
    {
        return Err(AudioTaskOutcomeMismatch::MicrocodeDataIdentity {
            reference: reference.selection.microcode.ucode_data_sha256,
            candidate: candidate.selection.microcode.ucode_data_sha256,
        });
    }
    if reference.selection.family != candidate.selection.family {
        return Err(AudioTaskOutcomeMismatch::HleFamily {
            reference: reference.selection.family,
            candidate: candidate.selection.family,
        });
    }
    if reference.selection.implementation_revision != candidate.selection.implementation_revision {
        return Err(AudioTaskOutcomeMismatch::HleImplementationRevision {
            reference: reference.selection.implementation_revision,
            candidate: candidate.selection.implementation_revision,
        });
    }
    if reference.terminal != candidate.terminal {
        return Err(AudioTaskOutcomeMismatch::TerminalReason {
            reference: reference.terminal,
            candidate: candidate.terminal,
        });
    }

    let reference_patches = reference.rdram_patches.as_slice();
    let candidate_patches = candidate.rdram_patches.as_slice();
    for index in 0..reference_patches.len().max(candidate_patches.len()) {
        let reference_patch = reference_patches.get(index);
        let candidate_patch = candidate_patches.get(index);
        if reference_patch.map(RdramPatch::range) != candidate_patch.map(RdramPatch::range) {
            return Err(AudioTaskOutcomeMismatch::RdramPatchRange {
                index,
                reference: reference_patch.map(RdramPatch::range),
                candidate: candidate_patch.map(RdramPatch::range),
            });
        }
        if let (Some(reference_patch), Some(candidate_patch)) = (reference_patch, candidate_patch) {
            if let Some(offset) = reference_patch
                .bytes()
                .iter()
                .zip(candidate_patch.bytes())
                .position(|(reference, candidate)| reference != candidate)
            {
                return Err(AudioTaskOutcomeMismatch::RdramPatchByte {
                    address: reference_patch.range.start() + offset as u32,
                    reference: reference_patch.bytes[offset],
                    candidate: candidate_patch.bytes[offset],
                });
            }
        }
    }

    let reference_pcm = reference.pcm_ranges.as_slice();
    let candidate_pcm = candidate.pcm_ranges.as_slice();
    for index in 0..reference_pcm.len().max(candidate_pcm.len()) {
        if reference_pcm.get(index) != candidate_pcm.get(index) {
            return Err(AudioTaskOutcomeMismatch::PcmRange {
                index,
                reference: reference_pcm.get(index).copied(),
                candidate: candidate_pcm.get(index).copied(),
            });
        }
    }

    if let Some(offset) = reference
        .rsp
        .dmem
        .iter()
        .zip(candidate.rsp.dmem.iter())
        .position(|(reference, candidate)| reference != candidate)
    {
        return Err(AudioTaskOutcomeMismatch::DmemByte {
            offset: offset as u16,
            reference: reference.rsp.dmem[offset],
            candidate: candidate.rsp.dmem[offset],
        });
    }
    if let Some(offset) = reference
        .rsp
        .imem
        .iter()
        .zip(candidate.rsp.imem.iter())
        .position(|(reference, candidate)| reference != candidate)
    {
        return Err(AudioTaskOutcomeMismatch::ImemByte {
            offset: offset as u16,
            reference: reference.rsp.imem[offset],
            candidate: candidate.rsp.imem[offset],
        });
    }
    if reference.rsp.imem_generation != candidate.rsp.imem_generation {
        return Err(AudioTaskOutcomeMismatch::ImemGeneration {
            reference: reference.rsp.imem_generation,
            candidate: candidate.rsp.imem_generation,
        });
    }
    if reference.rsp.sp_pc != candidate.rsp.sp_pc {
        return Err(AudioTaskOutcomeMismatch::SpPc {
            reference: reference.rsp.sp_pc,
            candidate: candidate.rsp.sp_pc,
        });
    }
    if reference.rsp.sp_status != candidate.rsp.sp_status {
        return Err(AudioTaskOutcomeMismatch::SpStatus {
            reference: reference.rsp.sp_status,
            candidate: candidate.rsp.sp_status,
        });
    }
    if reference.rsp.sp_semaphore != candidate.rsp.sp_semaphore {
        return Err(AudioTaskOutcomeMismatch::SpSemaphore {
            reference: reference.rsp.sp_semaphore,
            candidate: candidate.rsp.sp_semaphore,
        });
    }
    for (register, reference_value, candidate_value) in [
        (
            DmaRegister::MemAddress,
            reference.rsp.dma.mem_address,
            candidate.rsp.dma.mem_address,
        ),
        (
            DmaRegister::DramAddress,
            reference.rsp.dma.dram_address,
            candidate.rsp.dma.dram_address,
        ),
        (
            DmaRegister::ReadLength,
            reference.rsp.dma.read_length,
            candidate.rsp.dma.read_length,
        ),
        (
            DmaRegister::WriteLength,
            reference.rsp.dma.write_length,
            candidate.rsp.dma.write_length,
        ),
    ] {
        if reference_value != candidate_value {
            return Err(AudioTaskOutcomeMismatch::DmaRegister {
                register,
                reference: reference_value,
                candidate: candidate_value,
            });
        }
    }

    for (register, reference_value, candidate_value) in [
        (
            DpcRegister::Start,
            reference.rsp.dpc.start,
            candidate.rsp.dpc.start,
        ),
        (
            DpcRegister::End,
            reference.rsp.dpc.end,
            candidate.rsp.dpc.end,
        ),
        (
            DpcRegister::Current,
            reference.rsp.dpc.current,
            candidate.rsp.dpc.current,
        ),
        (
            DpcRegister::Status,
            reference.rsp.dpc.status,
            candidate.rsp.dpc.status,
        ),
        (
            DpcRegister::Clock,
            reference.rsp.dpc.clock,
            candidate.rsp.dpc.clock,
        ),
        (
            DpcRegister::CommandBusy,
            reference.rsp.dpc.command_busy,
            candidate.rsp.dpc.command_busy,
        ),
        (
            DpcRegister::PipeBusy,
            reference.rsp.dpc.pipe_busy,
            candidate.rsp.dpc.pipe_busy,
        ),
        (
            DpcRegister::TmemBusy,
            reference.rsp.dpc.tmem_busy,
            candidate.rsp.dpc.tmem_busy,
        ),
    ] {
        if reference_value != candidate_value {
            return Err(AudioTaskOutcomeMismatch::DpcRegister {
                register,
                reference: reference_value,
                candidate: candidate_value,
            });
        }
    }

    if reference.rsp.dpc_submissions.len() != candidate.rsp.dpc_submissions.len() {
        return Err(AudioTaskOutcomeMismatch::DpcSubmissionCount {
            reference: reference.rsp.dpc_submissions.len(),
            candidate: candidate.rsp.dpc_submissions.len(),
        });
    }
    if let Some((index, (reference_submission, candidate_submission))) = reference
        .rsp
        .dpc_submissions
        .iter()
        .zip(candidate.rsp.dpc_submissions.iter())
        .enumerate()
        .find(|(_, (reference, candidate))| reference.identity() != candidate.identity())
    {
        return Err(AudioTaskOutcomeMismatch::DpcSubmission {
            index,
            reference: reference_submission.identity(),
            candidate: candidate_submission.identity(),
        });
    }
    if reference.completion_steps != candidate.completion_steps {
        return Err(AudioTaskOutcomeMismatch::CompletionSteps {
            reference: reference.completion_steps,
            candidate: candidate.completion_steps,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> Sha256Digest {
        Sha256Digest::new([byte; 32])
    }

    fn patches(start: u32, bytes: &[u8]) -> CanonicalRdramPatches {
        CanonicalRdramPatches::new(vec![RdramPatch::new(start, bytes.to_vec()).unwrap()]).unwrap()
    }

    fn ranges(start: u32, byte_len: u32) -> CanonicalRdramRanges {
        CanonicalRdramRanges::new(vec![RdramByteRange::new(start, byte_len).unwrap()]).unwrap()
    }

    fn dmem_submission(start: u32, payload: &[u8]) -> DeferredDpcSubmission {
        DeferredDpcSubmission::from_dmem_payload(
            start,
            start + u32::try_from(payload.len()).unwrap(),
            payload.to_vec(),
        )
        .unwrap()
    }

    fn state() -> RspVisibleState {
        RspVisibleState::new(
            [0x11; RSP_BANK_BYTES],
            [0x22; RSP_BANK_BYTES],
            7,
            0x108,
            0x203,
            true,
            RspDmaRegisterState {
                mem_address: 0x80,
                dram_address: 0x1234,
                read_length: 0x3f,
                write_length: 0x1f,
            },
            RspDpcRegisterState {
                start: 0x20,
                end: 0x28,
                current: 0x28,
                status: 0x80,
                clock: 91,
                command_busy: 0,
                pipe_busy: 0,
                tmem_busy: 0,
            },
            vec![dmem_submission(
                0x20,
                &[0xd0, 0xd1, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7],
            )],
        )
        .unwrap()
    }

    fn outcome() -> AudioTaskOutcome {
        AudioTaskOutcome::new(
            AudioHleSelection {
                microcode: AudioMicrocodeIdentity {
                    imem_sha256: digest(1),
                    ucode_data_bytes: 0x40,
                    ucode_data_sha256: digest(2),
                },
                family: AudioHleFamily::StandardAbi,
                implementation_revision: 7,
            },
            AudioTaskTerminalReason::Broke,
            patches(0x100, &[1, 2, 3, 4]),
            ranges(0x100, 4),
            state(),
            NonZeroU64::new(91).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn exact_outcomes_compare_equal() {
        let reference = outcome();
        assert_eq!(compare_audio_task_outcomes(&reference, &reference), Ok(()));
    }

    #[test]
    fn microcode_identity_covers_full_imem_and_binds_data_length() {
        let mut imem = [0u8; RSP_BANK_BYTES];
        let identity = AudioMicrocodeIdentity::from_task_entry(&imem, &[1, 2, 3, 4]).unwrap();
        imem[RSP_BANK_BYTES - 1] = 1;
        let changed_imem = AudioMicrocodeIdentity::from_task_entry(&imem, &[1, 2, 3, 4]).unwrap();
        let changed_data_len =
            AudioMicrocodeIdentity::from_task_entry(&imem, &[1, 2, 3, 4, 0]).unwrap();

        assert_ne!(identity.imem_sha256, changed_imem.imem_sha256);
        assert_eq!(identity.ucode_data_bytes, 4);
        assert_eq!(changed_data_len.ucode_data_bytes, 5);
        assert_ne!(
            changed_imem.ucode_data_sha256,
            changed_data_len.ucode_data_sha256
        );
    }

    #[test]
    fn canonical_patches_sort_and_merge_adjacent_ranges() {
        let canonical = CanonicalRdramPatches::new(vec![
            RdramPatch::new(0x104, vec![5, 6]).unwrap(),
            RdramPatch::new(0x100, vec![1, 2, 3, 4]).unwrap(),
        ])
        .unwrap();
        assert_eq!(canonical.as_slice().len(), 1);
        assert_eq!(
            canonical.as_slice()[0].range(),
            RdramByteRange::new(0x100, 6).unwrap()
        );
        assert_eq!(canonical.as_slice()[0].bytes(), &[1, 2, 3, 4, 5, 6]);
        assert!(matches!(
            CanonicalRdramPatches::new(vec![
                RdramPatch::new(0x100, vec![0; 8]).unwrap(),
                RdramPatch::new(0x104, vec![0; 8]).unwrap(),
            ]),
            Err(CanonicalRdramError::Overlap { .. })
        ));
    }

    #[test]
    fn canonical_ranges_sort_and_merge_but_reject_overlap() {
        let canonical = CanonicalRdramRanges::new(vec![
            RdramByteRange::new(0x104, 4).unwrap(),
            RdramByteRange::new(0x100, 4).unwrap(),
        ])
        .unwrap();
        assert_eq!(
            canonical.as_slice(),
            &[RdramByteRange::new(0x100, 8).unwrap()]
        );
        assert!(matches!(
            CanonicalRdramRanges::new(vec![
                RdramByteRange::new(0x100, 8).unwrap(),
                RdramByteRange::new(0x104, 8).unwrap(),
            ]),
            Err(CanonicalRdramError::Overlap { .. })
        ));
    }

    #[test]
    fn ranges_reject_empty_and_overflow_and_pcm_must_be_written() {
        assert_eq!(RdramByteRange::new(1, 0), Err(RdramRangeError::Empty));
        assert!(matches!(
            RdramPatch::new(1, Vec::new()),
            Err(RdramPatchError::Range(RdramRangeError::Empty))
        ));
        assert!(matches!(
            RdramByteRange::new(u32::MAX, 2),
            Err(RdramRangeError::AddressOverflow { .. })
        ));
        assert_eq!(
            RdramByteRange::new(DEFAULT_RDRAM_SIZE as u32 - 1, 2),
            Err(RdramRangeError::OutOfBounds {
                start: DEFAULT_RDRAM_SIZE as u32 - 1,
                byte_len: 2,
                rdram_bytes: DEFAULT_RDRAM_SIZE as u32,
            })
        );
        assert!(matches!(
            AudioTaskOutcome::new(
                outcome().selection(),
                AudioTaskTerminalReason::Broke,
                patches(0x100, &[0; 4]),
                ranges(0x200, 4),
                state(),
                NonZeroU64::new(1).unwrap(),
            ),
            Err(AudioTaskOutcomeError::PcmRangeNotWritten { .. })
        ));
    }

    #[test]
    fn identity_and_terminal_mismatches_are_named() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.selection.microcode.imem_sha256 = digest(9);
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::MicrocodeImemIdentity { .. })
        ));
        candidate = reference.clone();
        candidate.selection.implementation_revision += 1;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::HleImplementationRevision { .. })
        ));
        candidate = reference.clone();
        candidate.terminal = AudioTaskTerminalReason::StepLimit;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::TerminalReason { .. })
        ));
    }

    #[test]
    fn rdram_range_byte_and_pcm_mismatches_are_named() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.rdram_patches = patches(0x104, &[1, 2, 3, 4]);
        candidate.pcm_ranges = ranges(0x104, 4);
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::RdramPatchRange { index: 0, .. })
        ));
        candidate = reference.clone();
        candidate.rdram_patches = patches(0x100, &[1, 2, 9, 4]);
        assert_eq!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::RdramPatchByte {
                address: 0x102,
                reference: 3,
                candidate: 9,
            })
        );
        candidate = reference.clone();
        candidate.pcm_ranges = ranges(0x100, 2);
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::PcmRange { index: 0, .. })
        ));
    }

    #[test]
    fn dmem_imem_and_generation_mismatches_are_named() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.rsp.dmem[7] = 0x44;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::DmemByte { offset: 7, .. })
        ));
        candidate = reference.clone();
        candidate.rsp.imem[9] = 0x55;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::ImemByte { offset: 9, .. })
        ));
        candidate = reference.clone();
        candidate.rsp.imem_generation += 1;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::ImemGeneration { .. })
        ));
    }

    #[test]
    fn sp_visible_register_mismatches_are_named() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.rsp.sp_pc = 0x10c;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::SpPc { .. })
        ));
        candidate = reference.clone();
        candidate.rsp.sp_status ^= 1;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::SpStatus { .. })
        ));
        candidate = reference.clone();
        candidate.rsp.sp_semaphore = false;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::SpSemaphore { .. })
        ));
    }

    #[test]
    fn dma_mismatch_names_the_first_register() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.rsp.dma.read_length += 1;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::DmaRegister {
                register: DmaRegister::ReadLength,
                ..
            })
        ));
    }

    #[test]
    fn dpc_count_and_identity_mismatches_are_named() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.rsp.dpc.status ^= 1;
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::DpcRegister {
                register: DpcRegister::Status,
                ..
            })
        ));
        candidate = reference.clone();
        candidate.rsp.dpc_submissions.clear();
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::DpcSubmissionCount { .. })
        ));
        candidate = reference.clone();
        candidate.rsp.dpc_submissions[0] =
            dmem_submission(0x20, &[0xd0, 0xd1, 0xd2, 0xdd, 0xd4, 0xd5, 0xd6, 0xd7]);
        let mismatch = compare_audio_task_outcomes(&reference, &candidate);
        assert!(matches!(
            mismatch,
            Err(AudioTaskOutcomeMismatch::DpcSubmission { index: 0, .. })
        ));
        if let Err(AudioTaskOutcomeMismatch::DpcSubmission {
            reference,
            candidate,
            ..
        }) = mismatch
        {
            assert_eq!(reference.source, candidate.source);
            assert_eq!(
                (reference.start, reference.end),
                (candidate.start, candidate.end)
            );
            assert_ne!(reference.command_sha256, candidate.command_sha256);
        }
    }

    #[test]
    fn deferred_dpc_submissions_validate_source_specific_content() {
        let rdram =
            DeferredDpcSubmission::from_rdram_words(0x100, 0x108, vec![0x1122_3344, 0xaabb_ccdd])
                .unwrap();
        assert_eq!(rdram.source(), DpcSubmissionSource::Rdram);
        assert_eq!((rdram.start(), rdram.end()), (0x100, 0x108));
        assert_eq!(rdram.xbus_payload(), None);
        assert_eq!(rdram.command_words(), [0x1122_3344, 0xaabb_ccdd]);
        assert_eq!(
            rdram.identity().command_sha256,
            Sha256Digest::hash(&[0x11, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0xdd])
        );

        let payload = vec![0x11, 0x22, 0x33, 0x44, 0xaa, 0xbb, 0xcc, 0xdd];
        let dmem = DeferredDpcSubmission::from_dmem_payload(0x20, 0x28, payload.clone()).unwrap();
        assert_eq!(dmem.source(), DpcSubmissionSource::Dmem);
        assert_eq!(dmem.xbus_payload(), Some(payload.as_slice()));
        assert_eq!(dmem.command_words(), [0x1122_3344, 0xaabb_ccdd]);
        assert_eq!(dmem.identity().command_sha256, Sha256Digest::hash(&payload));

        assert!(matches!(
            DeferredDpcSubmission::from_rdram_words(0x104, 0x108, vec![0]),
            Err(DpcSubmissionError::UnalignedRange { alignment: 8, .. })
        ));
        assert!(matches!(
            DeferredDpcSubmission::from_rdram_words(0x00ff_fff8, 0x0100_0008, vec![0; 4]),
            Err(DpcSubmissionError::SourceRangeOutOfBounds {
                source: DpcSubmissionSource::Rdram,
                upper_bound: 0x0100_0000,
                ..
            })
        ));
        assert_eq!(
            DeferredDpcSubmission::from_rdram_words(0x100, 0x108, vec![0]),
            Err(DpcSubmissionError::RdramCommandWordCount {
                expected: 2,
                actual: 1,
            })
        );
        assert!(matches!(
            DeferredDpcSubmission::from_dmem_payload(0x0ff8, 0x1008, vec![0; 16]),
            Err(DpcSubmissionError::SourceRangeOutOfBounds {
                source: DpcSubmissionSource::Dmem,
                upper_bound: 0x1000,
                ..
            })
        ));
        assert_eq!(
            DeferredDpcSubmission::from_dmem_payload(0x20, 0x28, vec![0; 7]),
            Err(DpcSubmissionError::DmemPayloadLength {
                expected: 8,
                actual: 7,
            })
        );
    }

    #[test]
    fn dpc_comparison_preserves_submission_order_and_rdram_word_identity() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.rsp.dpc_submissions = vec![
            dmem_submission(0x28, &[8, 9, 10, 11, 12, 13, 14, 15]),
            reference.rsp.dpc_submissions[0].clone(),
        ];
        let mut ordered_reference = reference.clone();
        ordered_reference
            .rsp
            .dpc_submissions
            .push(dmem_submission(0x28, &[8, 9, 10, 11, 12, 13, 14, 15]));
        assert!(matches!(
            compare_audio_task_outcomes(&ordered_reference, &candidate),
            Err(AudioTaskOutcomeMismatch::DpcSubmission { index: 0, .. })
        ));

        let mut rdram_reference = reference.clone();
        rdram_reference.rsp.dpc_submissions = vec![DeferredDpcSubmission::from_rdram_words(
            0x100,
            0x108,
            vec![0x1122_3344, 0x5566_7788],
        )
        .unwrap()];
        let mut rdram_candidate = rdram_reference.clone();
        rdram_candidate.rsp.dpc_submissions[0] =
            DeferredDpcSubmission::from_rdram_words(0x100, 0x108, vec![0x1122_3344, 0x5566_7789])
                .unwrap();
        assert!(matches!(
            compare_audio_task_outcomes(&rdram_reference, &rdram_candidate),
            Err(AudioTaskOutcomeMismatch::DpcSubmission { index: 0, .. })
        ));
    }

    #[test]
    fn completion_step_mismatch_is_named_last() {
        let reference = outcome();
        let mut candidate = reference.clone();
        candidate.completion_steps = NonZeroU64::new(92).unwrap();
        assert!(matches!(
            compare_audio_task_outcomes(&reference, &candidate),
            Err(AudioTaskOutcomeMismatch::CompletionSteps { .. })
        ));
    }

    #[test]
    fn rsp_pc_and_dpc_ranges_are_validated() {
        assert!(matches!(
            RspVisibleState::new(
                [0; RSP_BANK_BYTES],
                [0; RSP_BANK_BYTES],
                0,
                2,
                0,
                false,
                RspDmaRegisterState::default(),
                RspDpcRegisterState::default(),
                vec![],
            ),
            Err(RspVisibleStateError::InvalidPc(2))
        ));
        assert!(matches!(
            DeferredDpcSubmission::from_rdram_words(8, 8, vec![]),
            Err(DpcSubmissionError::EmptyOrReversedRange { .. })
        ));
    }
}
