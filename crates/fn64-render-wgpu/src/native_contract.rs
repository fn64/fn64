//! Linear ownership contract for the first native-renderer vertical slice.
//!
//! This module freezes evidence and ownership only. Target allocation, raster,
//! guest-memory mutation, VI execution, capture, and shaders belong to later
//! implementation lanes.

// The two crate-private transitions are intentionally reserved for the target,
// raster, and VI implementation lanes which follow this contract-only slice.
#![allow(dead_code)]

use core::fmt;

use fn64_render_ir::{
    AccessMode, AccessPurpose, BackendCompletionAuthority, BackendEffectReport, CompletedWrite,
    DpInterruptState, GpuCompleteTicket, GuestCommittedTicket, PhysicalRange, QueueIdentity,
    RawCommandStream, RawTimelineEvent, RdramResource, ResourceAccess, ResourceRegion,
    SubmissionIdentity, ValidationError, WorkloadAdmission, WorkloadIdentity,
};

use crate::raw_dpc::{DecodedRawDpcParts, RawDpcDecodeOrigin, RawDpcResourcePlan};
use crate::{
    CycleType, DecodedRawDpc, DecodedRawDpcCommand, ImageFormat, PixelSize, RawDpcCommandKind,
    RdpState, RdpStateDelta, StagedRdpState,
};

pub const NATIVE_FILL_FIXTURE_SCHEMA: &str = "fn64.render-wgpu.native-fill.v1";
pub const NATIVE_FILL_RDRAM_BYTES: u32 = 8 * 1024 * 1024;
pub const NATIVE_FILL_COMMAND_START: u32 = 0x100;
pub const NATIVE_FILL_COMMAND_END: u32 = 0x128;
pub const NATIVE_FILL_TARGET_START: u32 = 0x400;
pub const NATIVE_FILL_TARGET_END: u32 = 0x410;
pub const NATIVE_FILL_WIDTH: u32 = 4;
pub const NATIVE_FILL_HEIGHT: u32 = 2;
pub const NATIVE_FILL_TRANSACTION_SEQUENCE: u64 = 7;

pub const NATIVE_FILL_COMMAND_WORDS: [u32; 10] = [
    0xef30_0000,
    0,
    0xff10_0003,
    NATIVE_FILL_TARGET_START,
    0xf700_0000,
    0xf801_f801,
    0xf600_c004,
    0,
    0xe900_0000,
    0,
];

/// Logical/device-order RGBA5551 pixels: red is the big-endian halfword
/// `f8 01` before the N64Recomp ABI backing-store lane transform.
pub const NATIVE_FILL_DEVICE_RGBA16: [u8; 16] = [
    0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01,
];
/// Bytes staged for the ABI transaction's flat copy into N64Recomp's
/// native-word RDRAM backing allocation. This is storage order, not logical
/// N64/device byte order.
pub const NATIVE_FILL_N64RECOMP_STORAGE_RGBA16: [u8; 16] = [
    0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8, 0x01, 0xf8,
];
pub const NATIVE_FILL_NATIVE_RGBA8: [u8; 32] = [
    0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff,
    0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff,
];
pub const NATIVE_FILL_POST_VI_BGRA8: [u8; 32] = [
    0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0,
    0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff,
];

pub const NATIVE_FILL_N64RECOMP_STORAGE_RGBA16_SHA256: &str =
    "007d65aa7365956d4ae38da6ee8849b14b7a5d88658adfb49df757255249f248";
pub const NATIVE_FILL_NATIVE_RGBA8_SHA256: &str =
    "5ed2cb747cf2014feda8638a6894704f15eb46c867ce7bae38d0447556f80549";
pub const NATIVE_FILL_POST_VI_BGRA8_SHA256: &str =
    "f9d2bc2ea8345a97d8a514eae7f50c165175355a80ca805309429d83748f7ee2";
pub const NATIVE_FILL_WORKLOAD_SHA256: &str =
    "08dc8fbed0143100b556b7b8bce27a31b78ff5e7bb1f0c914e29963275eb22d0";
pub const NATIVE_FILL_STREAM_SHA256: &str =
    "057b789d4989fe90faf753f8f6802db8aa64b94249dadffdda8e3a70ff4753d1";
pub const NATIVE_FILL_JOURNAL_SHA256: &str =
    "1206767d7c857d57832d88bb557a450d0e8f3fb331669e827316b676db83bc50";

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceRgba16Bytes(Box<[u8]>);

impl DeviceRgba16Bytes {
    pub(crate) fn from_device_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }

    pub fn device_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct N64RecompRdramStorageBytes(Box<[u8]>);

impl N64RecompRdramStorageBytes {
    pub(crate) fn from_n64recomp_storage_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }

    pub fn n64recomp_storage_bytes(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeTargetIdentity {
    range: PhysicalRange,
    format: ImageFormat,
    size: PixelSize,
    width: u32,
    height: u32,
}

impl NativeTargetIdentity {
    pub const fn range(self) -> PhysicalRange {
        self.range
    }

    pub const fn format(self) -> ImageFormat {
        self.format
    }

    pub const fn size(self) -> PixelSize {
        self.size
    }

    pub const fn width(self) -> u32 {
        self.width
    }

    pub const fn height(self) -> u32 {
        self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeFrameBinding {
    workload: WorkloadIdentity,
    queue: QueueIdentity,
    submission: SubmissionIdentity,
    submission_ordinal: u64,
    transaction_sequence: u64,
}

impl NativeFrameBinding {
    pub const fn workload(self) -> WorkloadIdentity {
        self.workload
    }

    pub const fn queue(self) -> QueueIdentity {
        self.queue
    }

    pub const fn submission(self) -> SubmissionIdentity {
        self.submission
    }

    pub const fn submission_ordinal(self) -> u64 {
        self.submission_ordinal
    }

    pub const fn transaction_sequence(self) -> u64 {
        self.transaction_sequence
    }
}

/// Renderer-owned state which becomes durable only after guest-owned commit.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct NativeDurableState {
    rdp: RdpState,
    target: Option<NativeTargetIdentity>,
    generation: u64,
    last_commit: Option<NativeFrameBinding>,
}

impl NativeDurableState {
    pub const fn rdp_state(&self) -> &RdpState {
        &self.rdp
    }

    pub const fn target(&self) -> Option<NativeTargetIdentity> {
        self.target
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn last_commit(&self) -> Option<NativeFrameBinding> {
        self.last_commit
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeContractError {
    UnsupportedFixture {
        workload: WorkloadIdentity,
        reason: &'static str,
    },
    DurableStateMismatch {
        workload: WorkloadIdentity,
    },
    DecodedStateMismatch {
        workload: WorkloadIdentity,
        field: &'static str,
    },
    SuccessorMismatch {
        workload: WorkloadIdentity,
        field: &'static str,
    },
    StateGenerationExhausted,
    GpuCompletionMismatch {
        workload: WorkloadIdentity,
        field: &'static str,
    },
    GuestCommitMismatch {
        workload: WorkloadIdentity,
        field: &'static str,
    },
    Ir(ValidationError),
}

impl fmt::Display for NativeContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFixture { workload, reason } => {
                write!(
                    formatter,
                    "workload {workload} is outside the exact M3.3a fixture: {reason}"
                )
            }
            Self::DurableStateMismatch { workload } => write!(
                formatter,
                "workload {workload} was decoded from a different durable RDP state"
            ),
            Self::DecodedStateMismatch { workload, field } => write!(
                formatter,
                "workload {workload} decoded-state proof differs at {field}"
            ),
            Self::SuccessorMismatch { workload, field } => write!(
                formatter,
                "workload {workload} is not the immediate committed successor for {field}"
            ),
            Self::StateGenerationExhausted => {
                formatter.write_str("native durable-state generation exhausted")
            }
            Self::GpuCompletionMismatch { workload, field } => write!(
                formatter,
                "workload {workload} GPU completion differs at {field}"
            ),
            Self::GuestCommitMismatch { workload, field } => write!(
                formatter,
                "workload {workload} guest commit differs at {field}"
            ),
            Self::Ir(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for NativeContractError {}

impl From<ValidationError> for NativeContractError {
    fn from(error: ValidationError) -> Self {
        Self::Ir(error)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeGuestCommitError<E> {
    Guest(E),
    Contract(NativeContractError),
}

pub struct PreparedNativeFill<'state> {
    durable: &'state mut NativeDurableState,
    submitted: fn64_render_ir::SubmittedTicket,
    commands: Box<[DecodedRawDpcCommand]>,
    state_delta: RdpStateDelta,
    staged_state: StagedRdpState,
    resource_plan: RawDpcResourcePlan,
    binding: NativeFrameBinding,
    target: NativeTargetIdentity,
    next_generation: u64,
}

impl fmt::Debug for PreparedNativeFill<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedNativeFill")
            .field("binding", &self.binding)
            .field("target", &self.target)
            .field("next_generation", &self.next_generation)
            .finish_non_exhaustive()
    }
}

impl<'state> PreparedNativeFill<'state> {
    pub const fn binding(&self) -> NativeFrameBinding {
        self.binding
    }

    pub const fn target(&self) -> NativeTargetIdentity {
        self.target
    }

    pub fn commands(&self) -> &[DecodedRawDpcCommand] {
        &self.commands
    }

    pub const fn resource_plan(&self) -> &RawDpcResourcePlan {
        &self.resource_plan
    }

    pub const fn state_delta(&self) -> &RdpStateDelta {
        &self.state_delta
    }

    pub(crate) fn begin(self) -> InFlightNativeFill<'state> {
        InFlightNativeFill { prepared: self }
    }
}

pub struct InFlightNativeFill<'state> {
    prepared: PreparedNativeFill<'state>,
}

impl fmt::Debug for InFlightNativeFill<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InFlightNativeFill")
            .field("binding", &self.prepared.binding)
            .finish_non_exhaustive()
    }
}

impl<'state> InFlightNativeFill<'state> {
    pub const fn binding(&self) -> NativeFrameBinding {
        self.prepared.binding
    }

    pub(crate) fn complete(
        self,
        backend: &mut BackendCompletionAuthority,
        output: NativeGpuOutput,
    ) -> Result<PendingNativeCommit<'state>, NativeContractError> {
        output.validate(self.prepared.binding.workload)?;
        let write = exact_write_access(&self.prepared)?;
        let completed = CompletedWrite::try_from_bytes(
            write,
            output.n64recomp_storage.n64recomp_storage_bytes(),
        )?;
        let effects =
            BackendEffectReport::try_new(self.prepared.submitted.packet(), vec![completed])?;
        let receipt = backend.issue(&self.prepared.submitted, effects)?;
        let ticket = self.prepared.submitted.gpu_complete(receipt)?;
        let backend_effects = ticket.backend_effect_identity();
        Ok(PendingNativeCommit {
            durable: self.prepared.durable,
            ticket,
            device_rgba16: output.device_rgba16,
            n64recomp_storage: output.n64recomp_storage,
            native_rgba8: output.native_rgba8,
            post_vi_bgra8: output.post_vi_bgra8,
            state_delta: self.prepared.state_delta,
            staged_state: self.prepared.staged_state,
            binding: self.prepared.binding,
            target: self.prepared.target,
            next_generation: self.prepared.next_generation,
            backend_effects,
        })
    }
}

pub(crate) struct NativeGpuOutput {
    device_rgba16: DeviceRgba16Bytes,
    n64recomp_storage: N64RecompRdramStorageBytes,
    native_rgba8: Box<[u8]>,
    post_vi_bgra8: Box<[u8]>,
}

impl NativeGpuOutput {
    pub(crate) fn from_typed_domains(
        device_rgba16: DeviceRgba16Bytes,
        n64recomp_storage: N64RecompRdramStorageBytes,
        native_rgba8: Vec<u8>,
        post_vi_bgra8: Vec<u8>,
    ) -> Self {
        Self {
            device_rgba16,
            n64recomp_storage,
            native_rgba8: native_rgba8.into_boxed_slice(),
            post_vi_bgra8: post_vi_bgra8.into_boxed_slice(),
        }
    }

    fn validate(&self, workload: WorkloadIdentity) -> Result<(), NativeContractError> {
        for (field, actual, expected) in [
            (
                "logical/device RGBA16 bytes",
                self.device_rgba16.device_bytes(),
                &NATIVE_FILL_DEVICE_RGBA16[..],
            ),
            (
                "N64Recomp RDRAM backing-storage bytes",
                self.n64recomp_storage.n64recomp_storage_bytes(),
                &NATIVE_FILL_N64RECOMP_STORAGE_RGBA16[..],
            ),
            (
                "native RGBA8 target bytes",
                &*self.native_rgba8,
                &NATIVE_FILL_NATIVE_RGBA8[..],
            ),
            (
                "post-VI BGRA8 capture bytes",
                &*self.post_vi_bgra8,
                &NATIVE_FILL_POST_VI_BGRA8[..],
            ),
        ] {
            if actual != expected {
                return Err(NativeContractError::GpuCompletionMismatch { workload, field });
            }
        }
        Ok(())
    }
}

pub struct PendingNativeCommit<'state> {
    durable: &'state mut NativeDurableState,
    ticket: GpuCompleteTicket,
    device_rgba16: DeviceRgba16Bytes,
    n64recomp_storage: N64RecompRdramStorageBytes,
    native_rgba8: Box<[u8]>,
    post_vi_bgra8: Box<[u8]>,
    state_delta: RdpStateDelta,
    staged_state: StagedRdpState,
    binding: NativeFrameBinding,
    target: NativeTargetIdentity,
    next_generation: u64,
    backend_effects: fn64_render_ir::EffectIdentity,
}

impl fmt::Debug for PendingNativeCommit<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PendingNativeCommit")
            .field("binding", &self.binding)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl<'state> PendingNativeCommit<'state> {
    pub const fn binding(&self) -> NativeFrameBinding {
        self.binding
    }

    pub const fn guest_writeback_storage(&self) -> &N64RecompRdramStorageBytes {
        &self.n64recomp_storage
    }

    /// Transfers the GPU-complete capability to the guest-memory owner.
    /// Renderer state is published only if that owner returns the exact
    /// `GuestCommittedTicket` issued from the transferred ticket.
    pub fn commit_guest<E>(
        self,
        commit: impl FnOnce(
            GpuCompleteTicket,
            &N64RecompRdramStorageBytes,
        ) -> Result<GuestCommittedTicket, E>,
    ) -> Result<CommittedNativeFrame<'state>, NativeGuestCommitError<E>> {
        let guest =
            commit(self.ticket, &self.n64recomp_storage).map_err(NativeGuestCommitError::Guest)?;
        validate_guest_commit(self.binding, self.backend_effects, &guest)
            .map_err(NativeGuestCommitError::Contract)?;

        let (next_rdp, _, _, _) = self.staged_state.into_parts();
        self.durable.rdp = next_rdp;
        self.durable.target = Some(self.target);
        self.durable.generation = self.next_generation;
        self.durable.last_commit = Some(self.binding);

        Ok(CommittedNativeFrame {
            durable: self.durable,
            guest,
            device_rgba16: self.device_rgba16,
            native_rgba8: self.native_rgba8,
            post_vi_bgra8: self.post_vi_bgra8,
            binding: self.binding,
        })
    }
}

pub struct CommittedNativeFrame<'state> {
    durable: &'state NativeDurableState,
    guest: GuestCommittedTicket,
    device_rgba16: DeviceRgba16Bytes,
    native_rgba8: Box<[u8]>,
    post_vi_bgra8: Box<[u8]>,
    binding: NativeFrameBinding,
}

impl fmt::Debug for CommittedNativeFrame<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommittedNativeFrame")
            .field("binding", &self.binding)
            .field("generation", &self.durable.generation)
            .finish_non_exhaustive()
    }
}

impl CommittedNativeFrame<'_> {
    pub const fn binding(&self) -> NativeFrameBinding {
        self.binding
    }

    pub const fn durable_state(&self) -> &NativeDurableState {
        self.durable
    }

    pub fn native_rgba8(&self) -> &[u8] {
        &self.native_rgba8
    }

    pub const fn device_rgba16(&self) -> &DeviceRgba16Bytes {
        &self.device_rgba16
    }

    pub fn post_vi_bgra8(&self) -> &[u8] {
        &self.post_vi_bgra8
    }

    pub fn into_guest_ticket(self) -> GuestCommittedTicket {
        self.guest
    }
}

pub fn prepare_native_fill<'state>(
    decoded: DecodedRawDpc,
    durable: &'state mut NativeDurableState,
) -> Result<PreparedNativeFill<'state>, NativeContractError> {
    let parts = decoded.into_contract_parts();
    let workload = parts.submitted.packet().identity();
    if parts.origin != RawDpcDecodeOrigin::Durable {
        return Err(NativeContractError::UnsupportedFixture {
            workload,
            reason: "speculative staged-state decode cannot guarantee predecessor rollback",
        });
    }
    if parts.base_state != durable.rdp {
        return Err(NativeContractError::DurableStateMismatch { workload });
    }

    let binding = binding(&parts)?;
    validate_staged_state(&parts, binding)?;
    validate_successor(durable.last_commit, binding)?;
    let target = validate_exact_fixture(&parts)?;
    let next_generation = durable
        .generation
        .checked_add(1)
        .ok_or(NativeContractError::StateGenerationExhausted)?;

    Ok(PreparedNativeFill {
        durable,
        submitted: parts.submitted,
        commands: parts.commands,
        state_delta: parts.state_delta,
        staged_state: parts.staged_state,
        resource_plan: parts.resource_plan,
        binding,
        target,
        next_generation,
    })
}

fn validate_staged_state(
    parts: &DecodedRawDpcParts,
    binding: NativeFrameBinding,
) -> Result<(), NativeContractError> {
    for (matches, field) in [
        (
            parts.staged_state.queue() == binding.queue,
            "queue identity",
        ),
        (
            parts.staged_state.submission_ordinal() == binding.submission_ordinal,
            "submission ordinal",
        ),
        (
            parts.staged_state.transaction_sequence() == binding.transaction_sequence,
            "transaction sequence",
        ),
    ] {
        if !matches {
            return Err(NativeContractError::DecodedStateMismatch {
                workload: binding.workload,
                field,
            });
        }
    }
    let mut expected = parts.base_state.fork_for_decode();
    expected.apply(&parts.state_delta);
    if expected.other_mode() != parts.staged_state.other_mode()
        || expected.color_image() != parts.staged_state.color_image()
        || expected.fill_color() != parts.staged_state.fill_color()
    {
        return Err(NativeContractError::DecodedStateMismatch {
            workload: binding.workload,
            field: "RDP state delta",
        });
    }
    Ok(())
}

fn binding(parts: &DecodedRawDpcParts) -> Result<NativeFrameBinding, NativeContractError> {
    let workload = parts.submitted.packet().identity();
    let WorkloadAdmission::RawDpc {
        transaction_sequence,
    } = parts.submitted.packet().admission()
    else {
        return Err(NativeContractError::UnsupportedFixture {
            workload,
            reason: "admission is not raw DPC",
        });
    };
    Ok(NativeFrameBinding {
        workload,
        queue: parts.submitted.queue(),
        submission: parts.submitted.identity(),
        submission_ordinal: parts.submitted.ordinal(),
        transaction_sequence,
    })
}

fn validate_successor(
    prior: Option<NativeFrameBinding>,
    candidate: NativeFrameBinding,
) -> Result<(), NativeContractError> {
    let Some(prior) = prior else {
        return Ok(());
    };
    if prior.queue != candidate.queue {
        return Err(NativeContractError::SuccessorMismatch {
            workload: candidate.workload,
            field: "queue identity",
        });
    }
    if prior.submission_ordinal.checked_add(1) != Some(candidate.submission_ordinal) {
        return Err(NativeContractError::SuccessorMismatch {
            workload: candidate.workload,
            field: "submission ordinal",
        });
    }
    if prior.transaction_sequence.checked_add(1) != Some(candidate.transaction_sequence) {
        return Err(NativeContractError::SuccessorMismatch {
            workload: candidate.workload,
            field: "transaction sequence",
        });
    }
    Ok(())
}

fn validate_exact_fixture(
    parts: &DecodedRawDpcParts,
) -> Result<NativeTargetIdentity, NativeContractError> {
    let packet = parts.submitted.packet();
    let workload = packet.identity();
    let reject = |reason| NativeContractError::UnsupportedFixture { workload, reason };

    if packet.memory_layout().bytes() != NATIVE_FILL_RDRAM_BYTES {
        return Err(reject("installed RDRAM size differs"));
    }
    if packet.admission()
        != (WorkloadAdmission::RawDpc {
            transaction_sequence: NATIVE_FILL_TRANSACTION_SEQUENCE,
        })
    {
        return Err(reject("transaction sequence differs"));
    }
    let [RawCommandStream::Dram(stream)] = packet.streams() else {
        return Err(reject("stream set is not one DRAM stream"));
    };
    let [chunk] = stream.chunks() else {
        return Err(reject("DRAM stream is not one chunk"));
    };
    if chunk.range().start().get() != NATIVE_FILL_COMMAND_START
        || chunk.range().end() != NATIVE_FILL_COMMAND_END
        || chunk.words() != NATIVE_FILL_COMMAND_WORDS
    {
        return Err(reject("command range or words differ"));
    }
    let timeline = stream.timeline();
    let [RawTimelineEvent::CmdEnd(cmd_end), RawTimelineEvent::FullSync(sync), RawTimelineEvent::DpInterrupt(interrupt)] =
        timeline.as_slice()
    else {
        return Err(reject(
            "timeline is not exactly CMD_END, FullSync, DP interrupt",
        ));
    };
    if cmd_end.chunk_index != 0
        || cmd_end.sequence != 1
        || cmd_end.source_address != NATIVE_FILL_COMMAND_END
        || cmd_end.interrupt != DpInterruptState::Clear
        || sync.ordinal != 0
        || sync.chunk_index != 0
        || sync.chunk_byte_offset != 32
        || sync.sequence != 2
        || sync.interrupt_sequence != 3
        || sync.stream_byte_offset != 32
        || sync.source_address != NATIVE_FILL_COMMAND_START + 32
        || sync.interrupt_before != DpInterruptState::Clear
        || sync.interrupt_after != DpInterruptState::Asserted
        || interrupt.full_sync_ordinal != 0
        || interrupt.sequence != 3
        || interrupt.before != DpInterruptState::Clear
        || interrupt.after != DpInterruptState::Asserted
    {
        return Err(reject("timeline identities differ"));
    }

    let layout = packet.memory_layout();
    let command_range = layout.range(NATIVE_FILL_COMMAND_START, NATIVE_FILL_COMMAND_END)?;
    let target_range = layout.range(NATIVE_FILL_TARGET_START, NATIVE_FILL_TARGET_END)?;
    let expected_accesses = [
        ResourceAccess::try_new(
            fn64_render_ir::OperationId::new(0),
            AccessMode::Read,
            AccessPurpose::CommandDecode,
            ResourceRegion::Rdram {
                resource: RdramResource::RawCommands,
                range: command_range,
            },
        )?,
        ResourceAccess::try_new(
            fn64_render_ir::OperationId::new(1),
            AccessMode::Write,
            AccessPurpose::RenderTarget,
            ResourceRegion::Rdram {
                resource: RdramResource::ColorFramebuffer,
                range: target_range,
            },
        )?,
    ];
    if packet.journal().accesses() != expected_accesses
        || parts.resource_plan.accesses() != expected_accesses
    {
        return Err(reject("resource identities differ"));
    }

    let command_kinds = parts
        .commands
        .iter()
        .map(|command| command.kind())
        .collect::<Vec<_>>();
    let [other_mode, color_image, fill_color, rectangle, RawDpcCommandKind::FullSync(_)] =
        command_kinds.as_slice()
    else {
        return Err(reject("decoded command kinds differ"));
    };
    let RawDpcCommandKind::SetOtherMode(other_mode) = other_mode else {
        return Err(reject("first command is not SetOtherMode"));
    };
    let RawDpcCommandKind::SetColorImage(color_image) = color_image else {
        return Err(reject("second command is not SetColorImage"));
    };
    let RawDpcCommandKind::SetFillColor(fill_color) = fill_color else {
        return Err(reject("third command is not SetFillColor"));
    };
    let RawDpcCommandKind::FillRectangle(rectangle) = rectangle else {
        return Err(reject("fourth command is not FillRectangle"));
    };
    if other_mode.high() != 0x0030_0000
        || other_mode.low() != 0
        || other_mode.cycle_type() != CycleType::Fill
        || color_image.format() != ImageFormat::Rgba
        || color_image.size() != PixelSize::Bits16
        || color_image.width() != NATIVE_FILL_WIDTH
        || color_image.address().get() != NATIVE_FILL_TARGET_START
        || fill_color.value() != 0xf801_f801
        || rectangle.upper_left_x() != 0
        || rectangle.upper_left_y() != 0
        || rectangle.lower_right_x() != 12
        || rectangle.lower_right_y() != 4
    {
        return Err(reject("decoded state or fill geometry differs"));
    }

    Ok(NativeTargetIdentity {
        range: target_range,
        format: ImageFormat::Rgba,
        size: PixelSize::Bits16,
        width: NATIVE_FILL_WIDTH,
        height: NATIVE_FILL_HEIGHT,
    })
}

fn exact_write_access(
    prepared: &PreparedNativeFill<'_>,
) -> Result<ResourceAccess, NativeContractError> {
    let [_, write] = prepared.resource_plan.accesses() else {
        return Err(NativeContractError::GpuCompletionMismatch {
            workload: prepared.binding.workload,
            field: "resource-plan effect count",
        });
    };
    Ok(*write)
}

fn validate_guest_commit(
    binding: NativeFrameBinding,
    backend_effects: fn64_render_ir::EffectIdentity,
    guest: &GuestCommittedTicket,
) -> Result<(), NativeContractError> {
    for (matches, field) in [
        (
            guest.packet().identity() == binding.workload,
            "workload identity",
        ),
        (guest.queue() == binding.queue, "queue identity"),
        (
            guest.ordinal() == binding.submission_ordinal,
            "submission ordinal",
        ),
        (
            guest.submission() == binding.submission,
            "submission identity",
        ),
        (
            guest.backend_effect_identity() == backend_effects,
            "backend effect identity",
        ),
    ] {
        if !matches {
            return Err(NativeContractError::GuestCommitMismatch {
                workload: binding.workload,
                field,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fn64_render_ir::{
        DecodedTicket, DramCommandChunk, DramCommandStream, FullSyncBoundary, GuestCommitAuthority,
        GuestCommitEffectReport, PhysicalMemoryLayout, RawCommandStream, ResourceJournal,
        ResourceJournalLimits, TemporalBoundary, TicketAuthoritySet, WorkloadPacket,
    };
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::{decode_raw_dpc, RawDpcDecodeError};

    fn packet_at(transaction_sequence: u64, words: Vec<u32>, target_end: u32) -> WorkloadPacket {
        packet_variant(
            NATIVE_FILL_RDRAM_BYTES,
            transaction_sequence,
            words,
            target_end,
            (1, 2, 3),
            false,
        )
    }

    fn packet_variant(
        layout_bytes: u32,
        transaction_sequence: u64,
        words: Vec<u32>,
        target_end: u32,
        timeline_sequences: (u64, u64, u64),
        extra_journal_access: bool,
    ) -> WorkloadPacket {
        let layout = PhysicalMemoryLayout::try_new(layout_bytes).unwrap();
        let command_range = layout
            .range(NATIVE_FILL_COMMAND_START, NATIVE_FILL_COMMAND_END)
            .unwrap();
        let stream = RawCommandStream::Dram(
            DramCommandStream::try_new(vec![DramCommandChunk::try_new(
                command_range,
                words,
                TemporalBoundary::new(timeline_sequences.0, DpInterruptState::Clear),
                vec![FullSyncBoundary::new(
                    timeline_sequences.1,
                    timeline_sequences.2,
                    DpInterruptState::Clear,
                    DpInterruptState::Asserted,
                )],
            )
            .unwrap()])
            .unwrap(),
        );
        let mut accesses = vec![
            ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(0),
                AccessMode::Read,
                AccessPurpose::CommandDecode,
                ResourceRegion::Rdram {
                    resource: RdramResource::RawCommands,
                    range: command_range,
                },
            )
            .unwrap(),
            ResourceAccess::try_new(
                fn64_render_ir::OperationId::new(1),
                AccessMode::Write,
                AccessPurpose::RenderTarget,
                ResourceRegion::Rdram {
                    resource: RdramResource::ColorFramebuffer,
                    range: layout.range(NATIVE_FILL_TARGET_START, target_end).unwrap(),
                },
            )
            .unwrap(),
        ];
        if extra_journal_access {
            accesses.push(
                ResourceAccess::try_new(
                    fn64_render_ir::OperationId::new(2),
                    AccessMode::Read,
                    AccessPurpose::UploadSource,
                    ResourceRegion::Rdram {
                        resource: RdramResource::Buffer,
                        range: layout.range(0x800, 0x804).unwrap(),
                    },
                )
                .unwrap(),
            );
        }
        let journal = ResourceJournal::try_new(
            ResourceJournalLimits::try_new(3, NATIVE_FILL_RDRAM_BYTES).unwrap(),
            accesses,
        )
        .unwrap();
        WorkloadPacket::try_new(
            layout,
            WorkloadAdmission::RawDpc {
                transaction_sequence,
            },
            vec![stream],
            journal,
        )
        .unwrap()
    }

    fn packet(words: Vec<u32>, target_end: u32) -> WorkloadPacket {
        packet_at(NATIVE_FILL_TRANSACTION_SEQUENCE, words, target_end)
    }

    fn lifecycle() -> (
        DecodedRawDpc,
        BackendCompletionAuthority,
        GuestCommitAuthority,
    ) {
        let (mut queue, backend, guest) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue
            .submit(DecodedTicket::new(packet(
                NATIVE_FILL_COMMAND_WORDS.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        (decoded, backend, guest)
    }

    fn exact_output() -> NativeGpuOutput {
        NativeGpuOutput::from_typed_domains(
            DeviceRgba16Bytes::from_device_bytes(NATIVE_FILL_DEVICE_RGBA16.to_vec()),
            N64RecompRdramStorageBytes::from_n64recomp_storage_bytes(
                NATIVE_FILL_N64RECOMP_STORAGE_RGBA16.to_vec(),
            ),
            NATIVE_FILL_NATIVE_RGBA8.to_vec(),
            NATIVE_FILL_POST_VI_BGRA8.to_vec(),
        )
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn assert_sentinel_unchanged(state: &NativeDurableState, target: NativeTargetIdentity) {
        assert_eq!(state.rdp_state(), &RdpState::default());
        assert_eq!(state.target(), Some(target));
        assert_eq!(state.generation(), 41);
        assert_eq!(state.last_commit(), None);
    }

    fn sentinel_state(decoded: &DecodedRawDpc) -> NativeDurableState {
        let image = decoded.staged_state().color_image().unwrap();
        let layout = decoded.submitted().packet().memory_layout();
        NativeDurableState {
            rdp: RdpState::default(),
            target: Some(NativeTargetIdentity {
                range: layout
                    .range(NATIVE_FILL_TARGET_START, NATIVE_FILL_TARGET_END)
                    .unwrap(),
                format: image.format(),
                size: image.size(),
                width: image.width(),
                height: NATIVE_FILL_HEIGHT,
            }),
            generation: 41,
            last_commit: None,
        }
    }

    fn guest_commit(
        authority: &mut GuestCommitAuthority,
        ticket: GpuCompleteTicket,
        bytes: &[u8],
    ) -> GuestCommittedTicket {
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
    fn exact_fixture_reaches_guest_owned_commit_then_publishes() {
        let (decoded, mut backend, mut guest) = lifecycle();
        assert_eq!(
            NATIVE_FILL_WORKLOAD_SHA256,
            "08dc8fbed0143100b556b7b8bce27a31b78ff5e7bb1f0c914e29963275eb22d0"
        );
        assert_eq!(
            decoded.submitted().packet().identity().to_string(),
            "08dc8fbed0143100b556b7b8bce27a31b78ff5e7bb1f0c914e29963275eb22d0"
        );
        assert_eq!(
            NATIVE_FILL_STREAM_SHA256,
            "057b789d4989fe90faf753f8f6802db8aa64b94249dadffdda8e3a70ff4753d1"
        );
        assert_eq!(
            decoded.submitted().packet().streams()[0]
                .identity()
                .to_string(),
            "057b789d4989fe90faf753f8f6802db8aa64b94249dadffdda8e3a70ff4753d1"
        );
        assert_eq!(
            NATIVE_FILL_JOURNAL_SHA256,
            "1206767d7c857d57832d88bb557a450d0e8f3fb331669e827316b676db83bc50"
        );
        assert_eq!(
            decoded
                .submitted()
                .packet()
                .journal()
                .identity()
                .to_string(),
            "1206767d7c857d57832d88bb557a450d0e8f3fb331669e827316b676db83bc50"
        );
        assert_eq!(
            NATIVE_FILL_N64RECOMP_STORAGE_RGBA16_SHA256,
            "007d65aa7365956d4ae38da6ee8849b14b7a5d88658adfb49df757255249f248"
        );
        assert_eq!(
            sha256(&NATIVE_FILL_N64RECOMP_STORAGE_RGBA16),
            "007d65aa7365956d4ae38da6ee8849b14b7a5d88658adfb49df757255249f248"
        );
        assert_eq!(
            NATIVE_FILL_NATIVE_RGBA8_SHA256,
            "5ed2cb747cf2014feda8638a6894704f15eb46c867ce7bae38d0447556f80549"
        );
        assert_eq!(
            sha256(&NATIVE_FILL_NATIVE_RGBA8),
            "5ed2cb747cf2014feda8638a6894704f15eb46c867ce7bae38d0447556f80549"
        );
        assert_eq!(
            NATIVE_FILL_POST_VI_BGRA8_SHA256,
            "f9d2bc2ea8345a97d8a514eae7f50c165175355a80ca805309429d83748f7ee2"
        );
        assert_eq!(
            sha256(&NATIVE_FILL_POST_VI_BGRA8),
            "f9d2bc2ea8345a97d8a514eae7f50c165175355a80ca805309429d83748f7ee2"
        );
        let mut durable = NativeDurableState::default();
        let prepared = prepare_native_fill(decoded, &mut durable).unwrap();
        assert_eq!(prepared.commands().len(), 5);
        assert_eq!(
            prepared.target().range().start().get(),
            NATIVE_FILL_TARGET_START
        );
        let pending = prepared
            .begin()
            .complete(&mut backend, exact_output())
            .unwrap();
        let committed = pending
            .commit_guest::<ValidationError>(|ticket, bytes| {
                assert_eq!(
                    bytes.n64recomp_storage_bytes(),
                    NATIVE_FILL_N64RECOMP_STORAGE_RGBA16
                );
                Ok(guest_commit(
                    &mut guest,
                    ticket,
                    bytes.n64recomp_storage_bytes(),
                ))
            })
            .unwrap();
        assert_eq!(committed.durable_state().generation(), 1);
        let target = committed.durable_state().target().unwrap();
        assert_eq!(target.range().start().get(), NATIVE_FILL_TARGET_START);
        assert_eq!(target.range().end(), NATIVE_FILL_TARGET_END);
        assert_eq!(target.format(), ImageFormat::Rgba);
        assert_eq!(target.size(), PixelSize::Bits16);
        assert_eq!((target.width(), target.height()), (4, 2));
        assert_eq!(
            committed.device_rgba16().device_bytes(),
            NATIVE_FILL_DEVICE_RGBA16
        );
        assert_eq!(committed.native_rgba8(), NATIVE_FILL_NATIVE_RGBA8);
        assert_eq!(committed.post_vi_bgra8(), NATIVE_FILL_POST_VI_BGRA8);
        drop(committed);
        assert_eq!(
            durable.rdp_state().fill_color().unwrap().value(),
            0xf801_f801
        );
    }

    #[test]
    fn abi_backing_storage_decodes_to_the_frozen_logical_device_pixels() {
        let mut storage = vec![0u8; fn64_runtime::rdram::DEFAULT_RDRAM_SIZE];
        storage[NATIVE_FILL_TARGET_START as usize..NATIVE_FILL_TARGET_END as usize]
            .copy_from_slice(&NATIVE_FILL_N64RECOMP_STORAGE_RGBA16);

        let view = fn64_runtime::RdramView::from_storage(&storage);
        let mut logical = [0u8; NATIVE_FILL_DEVICE_RGBA16.len()];
        view.copy_logical_bytes(
            fn64_runtime::RdramAddr::from_offset(NATIVE_FILL_TARGET_START),
            &mut logical,
        );
        assert_eq!(logical, NATIVE_FILL_DEVICE_RGBA16);
        assert_eq!(
            sha256(&logical),
            "a72ddbb473553ad1c66b20762411485e170c27126a244154e2e143703e06ae2c"
        );
        for pixel in 0..NATIVE_FILL_WIDTH * NATIVE_FILL_HEIGHT {
            assert_eq!(
                view.read_u16(fn64_runtime::RdramAddr::from_offset(
                    NATIVE_FILL_TARGET_START + pixel * 2
                )),
                0xf801,
                "logical RGBA5551 pixel {pixel}"
            );
        }
    }

    #[test]
    fn preparation_rejects_a_decode_from_another_durable_state() {
        let (first, _, _) = lifecycle();
        let mut state = NativeDurableState::default();
        let mut changed = NATIVE_FILL_COMMAND_WORDS;
        changed[5] = 0x1234_5678;
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue
            .submit(DecodedTicket::new(packet(
                changed.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let staged = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        let delta = staged.state_delta().clone();
        state.rdp.apply(&delta);
        assert!(matches!(
            prepare_native_fill(first, &mut state),
            Err(NativeContractError::DurableStateMismatch { .. })
        ));
    }

    #[test]
    fn hostile_output_table_is_loud_and_preserves_prior_rdp_and_target() {
        for (corrupt, expected_field) in [
            (0, "logical/device RGBA16 bytes"),
            (1, "N64Recomp RDRAM backing-storage bytes"),
            (2, "native RGBA8 target bytes"),
            (3, "post-VI BGRA8 capture bytes"),
        ] {
            let (decoded, mut backend, _) = lifecycle();
            let mut durable = sentinel_state(&decoded);
            let target = durable.target().unwrap();
            let mut device = NATIVE_FILL_DEVICE_RGBA16.to_vec();
            let mut storage = NATIVE_FILL_N64RECOMP_STORAGE_RGBA16.to_vec();
            let mut native = NATIVE_FILL_NATIVE_RGBA8.to_vec();
            let mut post_vi = NATIVE_FILL_POST_VI_BGRA8.to_vec();
            [&mut device, &mut storage, &mut native, &mut post_vi][corrupt][0] ^= 1;
            let error = prepare_native_fill(decoded, &mut durable)
                .unwrap()
                .begin()
                .complete(
                    &mut backend,
                    NativeGpuOutput::from_typed_domains(
                        DeviceRgba16Bytes::from_device_bytes(device),
                        N64RecompRdramStorageBytes::from_n64recomp_storage_bytes(storage),
                        native,
                        post_vi,
                    ),
                )
                .unwrap_err();
            assert!(matches!(
                error,
                NativeContractError::GpuCompletionMismatch { field, .. }
                    if field == expected_field
            ));
            assert_sentinel_unchanged(&durable, target);
        }
    }

    #[test]
    fn dropping_each_precommit_typestate_preserves_prior_rdp_and_target() {
        let (decoded, _, _) = lifecycle();
        let mut durable = sentinel_state(&decoded);
        let target = durable.target().unwrap();
        drop(prepare_native_fill(decoded, &mut durable).unwrap());
        assert_sentinel_unchanged(&durable, target);

        let (decoded, _, _) = lifecycle();
        let mut durable = sentinel_state(&decoded);
        let target = durable.target().unwrap();
        drop(prepare_native_fill(decoded, &mut durable).unwrap().begin());
        assert_sentinel_unchanged(&durable, target);

        let (decoded, mut backend, _) = lifecycle();
        let mut durable = sentinel_state(&decoded);
        let target = durable.target().unwrap();
        drop(
            prepare_native_fill(decoded, &mut durable)
                .unwrap()
                .begin()
                .complete(&mut backend, exact_output())
                .unwrap(),
        );
        assert_sentinel_unchanged(&durable, target);
    }

    #[test]
    fn wrong_backend_capability_is_loud_and_preserves_prior_state() {
        let (decoded, _, _) = lifecycle();
        let (_, mut wrong_backend, _) = lifecycle();
        let mut durable = sentinel_state(&decoded);
        let target = durable.target().unwrap();
        let error = prepare_native_fill(decoded, &mut durable)
            .unwrap()
            .begin()
            .complete(&mut wrong_backend, exact_output())
            .unwrap_err();
        assert!(matches!(
            error,
            NativeContractError::Ir(ValidationError::ReceiptAuthorityMismatch)
        ));
        assert_sentinel_unchanged(&durable, target);
    }

    #[test]
    fn guest_owner_error_preserves_durable_state() {
        let (decoded, mut backend, _) = lifecycle();
        let mut durable = NativeDurableState::default();
        let pending = prepare_native_fill(decoded, &mut durable)
            .unwrap()
            .begin()
            .complete(&mut backend, exact_output())
            .unwrap();
        let error = pending
            .commit_guest(|_ticket, _bytes| Err::<GuestCommittedTicket, _>("guest write failed"))
            .unwrap_err();
        assert_eq!(error, NativeGuestCommitError::Guest("guest write failed"));
        assert_eq!(durable, NativeDurableState::default());
    }

    #[test]
    fn wrong_guest_ticket_is_hostile_and_cannot_publish_renderer_state() {
        let (decoded, mut backend, _) = lifecycle();
        let (other_decoded, mut other_backend, mut other_guest) = lifecycle();
        let mut other_state = NativeDurableState::default();
        let other_pending = prepare_native_fill(other_decoded, &mut other_state)
            .unwrap()
            .begin()
            .complete(&mut other_backend, exact_output())
            .unwrap();
        let other_ticket = other_pending
            .commit_guest::<ValidationError>(|ticket, bytes| {
                Ok(guest_commit(
                    &mut other_guest,
                    ticket,
                    bytes.n64recomp_storage_bytes(),
                ))
            })
            .unwrap()
            .into_guest_ticket();

        let mut durable = NativeDurableState::default();
        let pending = prepare_native_fill(decoded, &mut durable)
            .unwrap()
            .begin()
            .complete(&mut backend, exact_output())
            .unwrap();
        let error = pending
            .commit_guest::<ValidationError>(|ticket, _| {
                drop(ticket);
                Ok(other_ticket)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            NativeGuestCommitError::Contract(NativeContractError::GuestCommitMismatch {
                field: "queue identity",
                ..
            })
        ));
        assert_eq!(durable, NativeDurableState::default());
    }

    #[test]
    fn same_queue_wrong_submission_is_loud_and_preserves_prior_state() {
        let (mut queue, mut backend, mut guest) =
            TicketAuthoritySet::try_new().unwrap().into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet(
                NATIVE_FILL_COMMAND_WORDS.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let second = queue
            .submit(DecodedTicket::new(packet(
                NATIVE_FILL_COMMAND_WORDS.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let first = decode_raw_dpc(first, &RdpState::default()).unwrap();
        let second = decode_raw_dpc(second, &RdpState::default()).unwrap();

        let mut durable = sentinel_state(&first);
        let target = durable.target().unwrap();
        let pending = prepare_native_fill(first, &mut durable)
            .unwrap()
            .begin()
            .complete(&mut backend, exact_output())
            .unwrap();
        let mut other_state = NativeDurableState::default();
        let other_ticket = prepare_native_fill(second, &mut other_state)
            .unwrap()
            .begin()
            .complete(&mut backend, exact_output())
            .unwrap()
            .commit_guest::<ValidationError>(|ticket, storage| {
                Ok(guest_commit(
                    &mut guest,
                    ticket,
                    storage.n64recomp_storage_bytes(),
                ))
            })
            .unwrap()
            .into_guest_ticket();

        let error = pending
            .commit_guest::<ValidationError>(|ticket, _| {
                drop(ticket);
                Ok(other_ticket)
            })
            .unwrap_err();
        assert!(matches!(
            error,
            NativeGuestCommitError::Contract(NativeContractError::GuestCommitMismatch {
                field: "submission ordinal",
                ..
            })
        ));
        assert_sentinel_unchanged(&durable, target);
    }

    #[test]
    fn same_submission_wrong_effect_identity_is_loud_and_preserves_prior_state() {
        let (decoded, mut backend, mut guest) = lifecycle();
        let mut durable = sentinel_state(&decoded);
        let target = durable.target().unwrap();
        let mut pending = prepare_native_fill(decoded, &mut durable)
            .unwrap()
            .begin()
            .complete(&mut backend, exact_output())
            .unwrap();
        let access = pending.ticket.backend_writes()[0].access();
        let wrong_write = CompletedWrite::try_from_bytes(
            access,
            &[0u8; NATIVE_FILL_N64RECOMP_STORAGE_RGBA16.len()],
        )
        .unwrap();
        pending.backend_effects =
            BackendEffectReport::try_new(pending.ticket.packet(), vec![wrong_write])
                .unwrap()
                .identity();

        let error = pending
            .commit_guest::<ValidationError>(|ticket, storage| {
                Ok(guest_commit(
                    &mut guest,
                    ticket,
                    storage.n64recomp_storage_bytes(),
                ))
            })
            .unwrap_err();
        assert!(matches!(
            error,
            NativeGuestCommitError::Contract(NativeContractError::GuestCommitMismatch {
                field: "backend effect identity",
                ..
            })
        ));
        assert_sentinel_unchanged(&durable, target);
    }

    #[test]
    fn byte_different_command_fixture_is_rejected_without_publication() {
        let mut words = NATIVE_FILL_COMMAND_WORDS;
        words[5] ^= 1;
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let submitted = queue
            .submit(DecodedTicket::new(packet(
                words.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let decoded = decode_raw_dpc(submitted, &RdpState::default()).unwrap();
        let mut durable = NativeDurableState::default();
        assert!(matches!(
            prepare_native_fill(decoded, &mut durable),
            Err(NativeContractError::UnsupportedFixture {
                reason: "command range or words differ",
                ..
            })
        ));
        assert_eq!(durable, NativeDurableState::default());
    }

    #[test]
    fn hostile_fixture_identity_table_is_loud_and_preserves_prior_state() {
        for case in ["timeline", "journal", "layout", "sequence"] {
            let packet = match case {
                "timeline" => packet_variant(
                    NATIVE_FILL_RDRAM_BYTES,
                    NATIVE_FILL_TRANSACTION_SEQUENCE,
                    NATIVE_FILL_COMMAND_WORDS.to_vec(),
                    NATIVE_FILL_TARGET_END,
                    (10, 11, 12),
                    false,
                ),
                "journal" => packet_variant(
                    NATIVE_FILL_RDRAM_BYTES,
                    NATIVE_FILL_TRANSACTION_SEQUENCE,
                    NATIVE_FILL_COMMAND_WORDS.to_vec(),
                    NATIVE_FILL_TARGET_END,
                    (1, 2, 3),
                    true,
                ),
                "layout" => packet_variant(
                    4 * 1024 * 1024,
                    NATIVE_FILL_TRANSACTION_SEQUENCE,
                    NATIVE_FILL_COMMAND_WORDS.to_vec(),
                    NATIVE_FILL_TARGET_END,
                    (1, 2, 3),
                    false,
                ),
                "sequence" => packet_variant(
                    NATIVE_FILL_RDRAM_BYTES,
                    NATIVE_FILL_TRANSACTION_SEQUENCE + 1,
                    NATIVE_FILL_COMMAND_WORDS.to_vec(),
                    NATIVE_FILL_TARGET_END,
                    (1, 2, 3),
                    false,
                ),
                _ => unreachable!(),
            };
            let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
            let submitted = queue.submit(DecodedTicket::new(packet)).unwrap();
            let (seed, _, _) = lifecycle();
            let mut durable = sentinel_state(&seed);
            let target = durable.target().unwrap();
            match decode_raw_dpc(submitted, durable.rdp_state()) {
                Err(RawDpcDecodeError::JournalMismatch { .. }) if case == "journal" => {}
                Err(error) => panic!("{case}: unexpected decode error: {error}"),
                Ok(decoded) => {
                    let error = prepare_native_fill(decoded, &mut durable).unwrap_err();
                    assert!(matches!(
                        (case, error),
                        (
                            "timeline",
                            NativeContractError::UnsupportedFixture {
                                reason: "timeline identities differ",
                                ..
                            }
                        ) | (
                            "layout",
                            NativeContractError::UnsupportedFixture {
                                reason: "installed RDRAM size differs",
                                ..
                            }
                        ) | (
                            "sequence",
                            NativeContractError::UnsupportedFixture {
                                reason: "transaction sequence differs",
                                ..
                            }
                        )
                    ));
                }
            }
            assert_sentinel_unchanged(&durable, target);
        }
    }

    #[test]
    fn speculative_staged_decode_is_rejected_even_if_rdp_values_match_durable_state() {
        let (mut queue, _, _) = TicketAuthoritySet::try_new().unwrap().into_roles();
        let first = queue
            .submit(DecodedTicket::new(packet_at(
                NATIVE_FILL_TRANSACTION_SEQUENCE - 1,
                NATIVE_FILL_COMMAND_WORDS.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let second = queue
            .submit(DecodedTicket::new(packet_at(
                NATIVE_FILL_TRANSACTION_SEQUENCE,
                NATIVE_FILL_COMMAND_WORDS.to_vec(),
                NATIVE_FILL_TARGET_END,
            )))
            .unwrap();
        let first = decode_raw_dpc(first, &RdpState::default()).unwrap();
        let mut durable = NativeDurableState::default();
        durable.rdp.apply(first.state_delta());
        let speculative = crate::decode_raw_dpc_after(second, first.into_staged_state()).unwrap();
        assert!(matches!(
            prepare_native_fill(speculative, &mut durable),
            Err(NativeContractError::UnsupportedFixture {
                reason: "speculative staged-state decode cannot guarantee predecessor rollback",
                ..
            })
        ));
        assert_eq!(durable.generation(), 0);
        assert_eq!(durable.target(), None);
    }
}
