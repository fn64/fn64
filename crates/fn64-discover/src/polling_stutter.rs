//! Candidate-only validation of an exact MMIO busy-poll stutter cycle.
//!
//! This validator proves a deliberately narrow quotient: from one committed
//! machine state, the first busy observation reaches a stable loop-head state,
//! every later busy observation repeats that state modulo unobservable Count
//! and Random phase, and a ready observation reaches the declared join.  It
//! preserves the immediate-ready and delayed-ready join states separately;
//! downstream validation must prove that both lead to the same claimed result.
//! The returned opaque value is not executable-image, placement, PI-event, or
//! release authority, and its serializable certificate cannot recreate it.

use fn64_recomp_rs::{
    decode, dynamic_mapped_execution_build_receipt_v1, BankId, BlockExit,
    DynamicMappedUnitCatalogV1, ExecutionKey, GuestPc, Instruction, InstructionBudget, MemoryPort,
    MmioOutcome, MmioPort, Rdram, RecompContext, RecompContextEvidenceSnapshotV1, RDRAM_LEN,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MMIO_POLLING_STUTTER_CERTIFICATE_SCHEMA_V1: &str =
    "fn64.mmio-polling-stutter-certificate.v1";

#[derive(Clone, Debug)]
pub struct PollingCodeSpanV1<'a> {
    pub virtual_start: u32,
    pub physical_start: u32,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct MmioPollingStutterRequestV1<'a> {
    pub code: Vec<PollingCodeSpanV1<'a>>,
    pub initial_context: &'a RecompContext,
    /// Raw host backing before the device commit, in the exact layout
    /// accepted by [`Rdram::new`].
    pub initial_rdram: &'a [u8],
    /// Raw host backing after the device commit. The ready paths execute
    /// against this backing; a later corridor validator must authenticate
    /// the transition from `initial_rdram` to this state.
    pub ready_rdram: &'a [u8],
    pub cycle_head_pc: u32,
    pub join_pc: u32,
    pub status_vaddr: u64,
    pub busy_status_word: u32,
    pub ready_status_word: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmioPollingStutterLimitsV1 {
    pub max_units_per_path: u32,
    pub max_instructions_per_path: u32,
}

impl Default for MmioPollingStutterLimitsV1 {
    fn default() -> Self {
        Self {
            max_units_per_path: 64,
            max_instructions_per_path: 128,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingPathCertificateV1 {
    pub units: u32,
    pub retired_instructions: u32,
    pub status_reads: u32,
    pub transcript_sha256: String,
    pub normalized_end_state_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingCodeCommitmentV1 {
    pub virtual_start: u32,
    pub physical_start: u32,
    pub len: u32,
    pub sha256: String,
}

/// Clock dimensions deliberately quotiented from recurrence equality.
/// A consumer must prove these fields unobservable through its final claim;
/// this validator neither advances the production Count clock nor admits
/// pending interrupts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingClockObligationV1 {
    pub quotiented_context_fields: Vec<String>,
    pub production_count_not_modeled: bool,
    pub interrupts_not_admitted: bool,
    pub downstream_must_prove_unobservable: bool,
}

impl PollingClockObligationV1 {
    fn required() -> Self {
        Self {
            quotiented_context_fields: vec![
                "cop0_count".to_owned(),
                "cop0_random_phase".to_owned(),
            ],
            production_count_not_modeled: true,
            interrupts_not_admitted: true,
            downstream_must_prove_unobservable: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmioPollingStutterCertificateV1 {
    pub schema: String,
    pub dynamic_semantics_schema: String,
    pub dynamic_semantics_sha256: String,
    pub code: Vec<PollingCodeCommitmentV1>,
    /// SHA-256 of all physical RDRAM bytes in guest address order, independent
    /// of the host backing's native-endian byte swizzle.
    pub initial_rdram_sha256: String,
    /// Canonical guest-order commitment to the post-device-commit backing used
    /// by every ready path.
    pub ready_rdram_sha256: String,
    pub initial_state_sha256: String,
    pub cycle_head_pc: u32,
    pub join_pc: u32,
    pub status_vaddr: u64,
    pub busy_status_word: u32,
    pub ready_status_word: u32,
    pub max_units_per_path: u32,
    pub max_instructions_per_path: u32,
    pub first_busy: PollingPathCertificateV1,
    pub steady_busy: PollingPathCertificateV1,
    pub repeated_steady_busy: PollingPathCertificateV1,
    pub immediate_ready: PollingPathCertificateV1,
    pub delayed_ready: PollingPathCertificateV1,
    pub repeated_delayed_ready: PollingPathCertificateV1,
    pub clock_obligation: PollingClockObligationV1,
}

/// Opaque validation issued only by [`validate_mmio_polling_stutter_v1`].
pub struct MmioPollingStutterValidationV1 {
    certificate: MmioPollingStutterCertificateV1,
    initial_context: RecompContext,
    immediate_join_context: RecompContext,
    delayed_join_context: RecompContext,
}

impl MmioPollingStutterValidationV1 {
    pub fn certificate(&self) -> &MmioPollingStutterCertificateV1 {
        &self.certificate
    }

    pub fn initial_context(&self) -> &RecompContext {
        &self.initial_context
    }

    /// State at the join when the device was ready before the first poll.
    pub fn immediate_join_representative(&self) -> &RecompContext {
        &self.immediate_join_context
    }

    /// State at the join after one or more inductively-equivalent busy polls.
    /// One representative only. Count and Random phase for arbitrary repeat
    /// counts remain the certificate's explicit downstream obligation.
    pub fn delayed_join_representative(&self) -> &RecompContext {
        &self.delayed_join_context
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MmioPollingStutterErrorV1 {
    EmptyCode,
    UnalignedCode,
    CodeRangeOverflow,
    InvalidDirectCodeMapping,
    OverlappingCodeSpans,
    InvalidMachinePoint {
        field: &'static str,
        pc: u32,
    },
    InvalidStatusAddress,
    EqualStatusWords,
    InvalidRdramLength {
        field: &'static str,
        actual: usize,
    },
    CodeBackingMismatch,
    PathLimitZero,
    PathLimitHit {
        path: &'static str,
    },
    UnsupportedInstruction {
        pc: u32,
        instruction: Instruction,
    },
    InstructionOutsideCode {
        pc: u32,
    },
    InstructionIdentityMismatch {
        pc: u32,
    },
    InstructionCountMismatch {
        pc: u32,
        expected: usize,
        actual: usize,
    },
    DynamicExecution {
        path: &'static str,
        detail: String,
    },
    UnexpectedExit {
        path: &'static str,
        detail: String,
    },
    UnexpectedStop {
        path: &'static str,
        expected_pc: u32,
        actual_pc: u32,
    },
    StatusReadCount {
        path: &'static str,
        actual: u32,
    },
    RdramChanged {
        path: &'static str,
    },
    BusyCycleNotRecurrent,
    BusyTranscriptNotRecurrent,
    DelayedReadyNotRecurrent,
    DelayedReadyTranscriptNotRecurrent,
}

#[derive(Clone, Copy)]
enum StatusMode {
    Busy,
    Ready,
}

impl StatusMode {
    fn word(self, request: &MmioPollingStutterRequestV1<'_>) -> u32 {
        match self {
            Self::Busy => request.busy_status_word,
            Self::Ready => request.ready_status_word,
        }
    }
}

struct ExactStatusPort {
    address: u64,
    word: u32,
    reads: u32,
}

impl MmioPort for ExactStatusPort {
    fn read_w(&mut self, vaddr: u64) -> MmioOutcome<u32> {
        if vaddr == self.address {
            self.reads = self.reads.saturating_add(1);
            MmioOutcome::Handled(self.word)
        } else {
            MmioOutcome::Fault { addr: vaddr }
        }
    }

    fn write_w(&mut self, vaddr: u64, _value: u32) -> MmioOutcome<()> {
        MmioOutcome::Fault { addr: vaddr }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UnitEvidence {
    entry_pc: u32,
    identity: [u8; 32],
    physical_addresses: Vec<u32>,
    retired_instructions: u32,
    exit_tag: u8,
    next_pc: u32,
}

struct PathResult {
    context: RecompContext,
    certificate: PollingPathCertificateV1,
    units: Vec<UnitEvidence>,
}

pub fn validate_mmio_polling_stutter_v1(
    request: &MmioPollingStutterRequestV1<'_>,
    limits: MmioPollingStutterLimitsV1,
) -> Result<MmioPollingStutterValidationV1, MmioPollingStutterErrorV1> {
    validate_request(request, limits)?;

    let first_busy = run_path(
        "first_busy",
        request,
        request.initial_context,
        request.initial_rdram,
        StatusMode::Busy,
        request.cycle_head_pc,
        limits,
    )?;
    let steady_busy = run_path(
        "steady_busy",
        request,
        &first_busy.context,
        request.initial_rdram,
        StatusMode::Busy,
        request.cycle_head_pc,
        limits,
    )?;
    let repeated_steady_busy = run_path(
        "repeated_steady_busy",
        request,
        &steady_busy.context,
        request.initial_rdram,
        StatusMode::Busy,
        request.cycle_head_pc,
        limits,
    )?;

    if normalized_snapshot(&first_busy.context) != normalized_snapshot(&steady_busy.context)
        || normalized_snapshot(&steady_busy.context)
            != normalized_snapshot(&repeated_steady_busy.context)
    {
        return Err(MmioPollingStutterErrorV1::BusyCycleNotRecurrent);
    }
    if steady_busy.units != repeated_steady_busy.units {
        return Err(MmioPollingStutterErrorV1::BusyTranscriptNotRecurrent);
    }

    let immediate_ready = run_path(
        "immediate_ready",
        request,
        request.initial_context,
        request.ready_rdram,
        StatusMode::Ready,
        request.join_pc,
        limits,
    )?;
    let delayed_ready = run_path(
        "delayed_ready",
        request,
        &first_busy.context,
        request.ready_rdram,
        StatusMode::Ready,
        request.join_pc,
        limits,
    )?;
    let repeated_delayed_ready = run_path(
        "repeated_delayed_ready",
        request,
        &steady_busy.context,
        request.ready_rdram,
        StatusMode::Ready,
        request.join_pc,
        limits,
    )?;

    if normalized_snapshot(&delayed_ready.context)
        != normalized_snapshot(&repeated_delayed_ready.context)
    {
        return Err(MmioPollingStutterErrorV1::DelayedReadyNotRecurrent);
    }
    if delayed_ready.units != repeated_delayed_ready.units {
        return Err(MmioPollingStutterErrorV1::DelayedReadyTranscriptNotRecurrent);
    }

    let semantics = dynamic_mapped_execution_build_receipt_v1();
    let code = request
        .code
        .iter()
        .map(|span| {
            Ok(PollingCodeCommitmentV1 {
                virtual_start: span.virtual_start,
                physical_start: span.physical_start,
                len: u32::try_from(span.bytes.len())
                    .map_err(|_| MmioPollingStutterErrorV1::CodeRangeOverflow)?,
                sha256: sha256(span.bytes),
            })
        })
        .collect::<Result<Vec<_>, MmioPollingStutterErrorV1>>()?;
    let certificate = MmioPollingStutterCertificateV1 {
        schema: MMIO_POLLING_STUTTER_CERTIFICATE_SCHEMA_V1.to_owned(),
        dynamic_semantics_schema: semantics.schema().to_owned(),
        dynamic_semantics_sha256: hex(semantics.source_sha256()),
        code,
        initial_rdram_sha256: canonical_rdram_sha256(request.initial_rdram),
        ready_rdram_sha256: canonical_rdram_sha256(request.ready_rdram),
        initial_state_sha256: hash_full_state(request.initial_context, request.cycle_head_pc),
        cycle_head_pc: request.cycle_head_pc,
        join_pc: request.join_pc,
        status_vaddr: request.status_vaddr,
        busy_status_word: request.busy_status_word,
        ready_status_word: request.ready_status_word,
        max_units_per_path: limits.max_units_per_path,
        max_instructions_per_path: limits.max_instructions_per_path,
        first_busy: first_busy.certificate,
        steady_busy: steady_busy.certificate,
        repeated_steady_busy: repeated_steady_busy.certificate,
        immediate_ready: immediate_ready.certificate,
        delayed_ready: delayed_ready.certificate,
        repeated_delayed_ready: repeated_delayed_ready.certificate,
        clock_obligation: PollingClockObligationV1::required(),
    };
    Ok(MmioPollingStutterValidationV1 {
        certificate,
        initial_context: request.initial_context.clone(),
        immediate_join_context: immediate_ready.context,
        delayed_join_context: delayed_ready.context,
    })
}

fn validate_request(
    request: &MmioPollingStutterRequestV1<'_>,
    limits: MmioPollingStutterLimitsV1,
) -> Result<(), MmioPollingStutterErrorV1> {
    if request.code.is_empty() {
        return Err(MmioPollingStutterErrorV1::EmptyCode);
    }
    let mut virtual_ranges = Vec::new();
    let mut physical_ranges = Vec::new();
    for span in &request.code {
        if span.bytes.is_empty()
            || !span.virtual_start.is_multiple_of(4)
            || !span.physical_start.is_multiple_of(4)
            || !span.bytes.len().is_multiple_of(4)
        {
            return Err(MmioPollingStutterErrorV1::UnalignedCode);
        }
        let len = u32::try_from(span.bytes.len())
            .map_err(|_| MmioPollingStutterErrorV1::CodeRangeOverflow)?;
        let virtual_end = span
            .virtual_start
            .checked_add(len)
            .ok_or(MmioPollingStutterErrorV1::CodeRangeOverflow)?;
        let physical_end = span
            .physical_start
            .checked_add(len)
            .filter(|end| *end <= RDRAM_LEN as u32)
            .ok_or(MmioPollingStutterErrorV1::CodeRangeOverflow)?;
        if !(0x8000_0000..0xc000_0000).contains(&span.virtual_start)
            || virtual_end > 0xc000_0000
            || span.virtual_start & 0x1fff_ffff != span.physical_start
        {
            return Err(MmioPollingStutterErrorV1::InvalidDirectCodeMapping);
        }
        virtual_ranges.push((span.virtual_start, virtual_end));
        physical_ranges.push((span.physical_start, physical_end));
    }
    virtual_ranges.sort_unstable();
    physical_ranges.sort_unstable();
    if virtual_ranges.windows(2).any(|pair| pair[0].1 > pair[1].0)
        || physical_ranges.windows(2).any(|pair| pair[0].1 > pair[1].0)
    {
        return Err(MmioPollingStutterErrorV1::OverlappingCodeSpans);
    }
    if !request.cycle_head_pc.is_multiple_of(4)
        || code_location(request, request.cycle_head_pc).is_none()
    {
        return Err(MmioPollingStutterErrorV1::InvalidMachinePoint {
            field: "cycle_head_pc",
            pc: request.cycle_head_pc,
        });
    }
    if !request.join_pc.is_multiple_of(4)
        || !(0x8000_0000..0xc000_0000).contains(&request.join_pc)
        || request.join_pc & 0x1fff_ffff >= RDRAM_LEN as u32
    {
        return Err(MmioPollingStutterErrorV1::InvalidMachinePoint {
            field: "join_pc",
            pc: request.join_pc,
        });
    }
    let low = request.status_vaddr as u32;
    let upper = request.status_vaddr >> 32;
    if !request.status_vaddr.is_multiple_of(4)
        || upper != u64::from(u32::MAX)
        || !(0xa000_0000..0xc000_0000).contains(&low)
        || low & 0x1fff_ffff < RDRAM_LEN as u32
    {
        return Err(MmioPollingStutterErrorV1::InvalidStatusAddress);
    }
    if request.busy_status_word == request.ready_status_word {
        return Err(MmioPollingStutterErrorV1::EqualStatusWords);
    }
    for (field, backing) in [
        ("initial_rdram", request.initial_rdram),
        ("ready_rdram", request.ready_rdram),
    ] {
        if backing.len() != RDRAM_LEN {
            return Err(MmioPollingStutterErrorV1::InvalidRdramLength {
                field,
                actual: backing.len(),
            });
        }
    }
    if limits.max_units_per_path == 0 || limits.max_instructions_per_path == 0 {
        return Err(MmioPollingStutterErrorV1::PathLimitZero);
    }
    for source in [request.initial_rdram, request.ready_rdram] {
        let mut backing = source.to_vec();
        let mem = Rdram::new(&mut backing);
        for span in &request.code {
            let len = u32::try_from(span.bytes.len()).expect("validated code span length fits u32");
            if mem.copy_physical_bytes(span.physical_start, len) != span.bytes {
                return Err(MmioPollingStutterErrorV1::CodeBackingMismatch);
            }
        }
    }
    Ok(())
}

fn run_path(
    path: &'static str,
    request: &MmioPollingStutterRequestV1<'_>,
    initial_context: &RecompContext,
    path_rdram: &[u8],
    mode: StatusMode,
    stop_pc: u32,
    limits: MmioPollingStutterLimitsV1,
) -> Result<PathResult, MmioPollingStutterErrorV1> {
    let mut context = initial_context.clone();
    let mut backing = path_rdram.to_vec();
    let mut mem = Rdram::new(&mut backing);
    let mut port = ExactStatusPort {
        address: request.status_vaddr,
        word: mode.word(request),
        reads: 0,
    };
    let mut memory_port = MemoryPort::mmio_only(&mut port);
    let mut catalog = DynamicMappedUnitCatalogV1::new_linked();
    let budget = InstructionBudget::new(InstructionBudget::CONTROL_TRANSFER_INSTRUCTIONS)
        .expect("control-transfer budget is nonzero");
    let mut key = ExecutionKey::new(BankId::new(0), GuestPc::new(request.cycle_head_pc));
    let mut units = Vec::new();
    let mut retired = 0u32;

    while key.pc.get() != stop_pc || units.is_empty() {
        if units.len() >= limits.max_units_per_path as usize {
            return Err(MmioPollingStutterErrorV1::PathLimitHit { path });
        }
        let expected_instruction_count = preflight_unit(request, key.pc.get())?;
        let entered_pc = key.pc.get();
        let run = catalog
            .activate_and_run_with_memory_port(
                key,
                budget,
                &mut context,
                &mut mem,
                &mut memory_port,
                |_| false,
            )
            .map_err(|error| MmioPollingStutterErrorV1::DynamicExecution {
                path,
                detail: error.to_string(),
            })?;
        let next_retired = retired
            .checked_add(run.run.instructions)
            .filter(|total| *total <= limits.max_instructions_per_path)
            .ok_or(MmioPollingStutterErrorV1::PathLimitHit { path })?;
        if run.run.instructions == 0 {
            return Err(MmioPollingStutterErrorV1::UnexpectedExit {
                path,
                detail: format!("zero retired instructions at 0x{entered_pc:08x}"),
            });
        }
        retired = next_retired;
        verify_instruction_identities(
            request,
            entered_pc,
            expected_instruction_count,
            &run.instructions,
        )?;
        let (exit_tag, next) = next_key(path, run.run.exit)?;
        ensure_code_or_stop(request, path, next.pc.get(), stop_pc)?;
        units.push(UnitEvidence {
            entry_pc: entered_pc,
            identity: run.identity.bytes(),
            physical_addresses: run
                .instructions
                .iter()
                .map(|identity| identity.physical_address)
                .collect(),
            retired_instructions: run.run.instructions,
            exit_tag,
            next_pc: next.pc.get(),
        });
        key = next;
    }

    if key.pc.get() != stop_pc {
        return Err(MmioPollingStutterErrorV1::UnexpectedStop {
            path,
            expected_pc: stop_pc,
            actual_pc: key.pc.get(),
        });
    }
    drop(memory_port);
    if port.reads != 1 {
        return Err(MmioPollingStutterErrorV1::StatusReadCount {
            path,
            actual: port.reads,
        });
    }
    drop(mem);
    if backing != path_rdram {
        return Err(MmioPollingStutterErrorV1::RdramChanged { path });
    }
    let certificate = PollingPathCertificateV1 {
        units: u32::try_from(units.len()).expect("path unit bound fits u32"),
        retired_instructions: retired,
        status_reads: port.reads,
        transcript_sha256: hash_transcript(&units),
        normalized_end_state_sha256: hash_normalized_state(&context, stop_pc),
    };
    Ok(PathResult {
        context,
        certificate,
        units,
    })
}

fn preflight_unit(
    request: &MmioPollingStutterRequestV1<'_>,
    pc: u32,
) -> Result<usize, MmioPollingStutterErrorV1> {
    let word = code_word(request, pc)?;
    let instruction = decode(word);
    if !allowed_instruction(instruction) {
        return Err(MmioPollingStutterErrorV1::UnsupportedInstruction { pc, instruction });
    }
    if instruction.has_delay_slot() {
        let delay_pc = pc
            .checked_add(4)
            .ok_or(MmioPollingStutterErrorV1::InstructionOutsideCode { pc })?;
        let delay = decode(code_word(request, delay_pc)?);
        if !allowed_instruction(delay) {
            return Err(MmioPollingStutterErrorV1::UnsupportedInstruction {
                pc: delay_pc,
                instruction: delay,
            });
        }
        Ok(2)
    } else {
        Ok(1)
    }
}

fn allowed_instruction(instruction: Instruction) -> bool {
    matches!(
        instruction,
        Instruction::Nop
            | Instruction::Lw { .. }
            | Instruction::Lui { .. }
            | Instruction::Andi { .. }
            | Instruction::Addiu { .. }
            | Instruction::Addu { .. }
            | Instruction::Or { .. }
            | Instruction::Jal { .. }
            | Instruction::Jr { .. }
            | Instruction::Beq { .. }
            | Instruction::Bne { .. }
    )
}

fn next_key(
    path: &'static str,
    exit: BlockExit,
) -> Result<(u8, ExecutionKey), MmioPollingStutterErrorV1> {
    match exit {
        BlockExit::Transfer(next) => Ok((1, next)),
        BlockExit::ResolveTransfer {
            source_bank,
            target_pc,
        } => Ok((2, ExecutionKey::new(source_bank, target_pc))),
        BlockExit::ResolveCall {
            source_bank,
            target_pc,
            ..
        } => Ok((3, ExecutionKey::new(source_bank, target_pc))),
        other => Err(MmioPollingStutterErrorV1::UnexpectedExit {
            path,
            detail: format!("{other:?}"),
        }),
    }
}

fn ensure_code_or_stop(
    request: &MmioPollingStutterRequestV1<'_>,
    path: &'static str,
    pc: u32,
    stop_pc: u32,
) -> Result<(), MmioPollingStutterErrorV1> {
    if pc == stop_pc || code_location(request, pc).is_some() {
        Ok(())
    } else {
        Err(MmioPollingStutterErrorV1::UnexpectedExit {
            path,
            detail: format!("code escape to 0x{pc:08x}"),
        })
    }
}

fn verify_instruction_identities(
    request: &MmioPollingStutterRequestV1<'_>,
    entry_pc: u32,
    expected_count: usize,
    identities: &[fn64_recomp_rs::InstructionWordIdentity],
) -> Result<(), MmioPollingStutterErrorV1> {
    if identities.len() != expected_count {
        return Err(MmioPollingStutterErrorV1::InstructionCountMismatch {
            pc: entry_pc,
            expected: expected_count,
            actual: identities.len(),
        });
    }
    for (index, identity) in identities.iter().enumerate() {
        let pc = entry_pc
            .checked_add(u32::try_from(index).unwrap().saturating_mul(4))
            .ok_or(MmioPollingStutterErrorV1::InstructionOutsideCode { pc: entry_pc })?;
        let (span, offset) = code_location(request, pc)
            .ok_or(MmioPollingStutterErrorV1::InstructionOutsideCode { pc })?;
        let expected = span
            .physical_start
            .checked_add(offset)
            .ok_or(MmioPollingStutterErrorV1::CodeRangeOverflow)?;
        if identity.physical_address != expected {
            return Err(MmioPollingStutterErrorV1::InstructionIdentityMismatch { pc });
        }
    }
    Ok(())
}

fn code_location<'a>(
    request: &'a MmioPollingStutterRequestV1<'a>,
    pc: u32,
) -> Option<(&'a PollingCodeSpanV1<'a>, u32)> {
    request.code.iter().find_map(|span| {
        let offset = pc.checked_sub(span.virtual_start)?;
        let len = u32::try_from(span.bytes.len()).ok()?;
        (offset < len && offset.is_multiple_of(4)).then_some((span, offset))
    })
}

fn code_word(
    request: &MmioPollingStutterRequestV1<'_>,
    pc: u32,
) -> Result<u32, MmioPollingStutterErrorV1> {
    let (span, offset) = code_location(request, pc)
        .ok_or(MmioPollingStutterErrorV1::InstructionOutsideCode { pc })?;
    let offset = offset as usize;
    let bytes = span
        .bytes
        .get(offset..offset + 4)
        .ok_or(MmioPollingStutterErrorV1::InstructionOutsideCode { pc })?;
    Ok(u32::from_be_bytes(bytes.try_into().unwrap()))
}

fn normalized_snapshot(context: &RecompContext) -> RecompContextEvidenceSnapshotV1 {
    let mut snapshot = context.evidence_snapshot_v1();
    snapshot.cop0_count = 0;
    snapshot.cop0_random_phase = 0;
    snapshot
}

fn hash_normalized_state(context: &RecompContext, pc: u32) -> String {
    let snapshot = normalized_snapshot(context);
    hash_state_snapshot(&snapshot, pc, b"normalized")
}

fn hash_full_state(context: &RecompContext, pc: u32) -> String {
    hash_state_snapshot(&context.evidence_snapshot_v1(), pc, b"full")
}

fn hash_state_snapshot(
    snapshot: &RecompContextEvidenceSnapshotV1,
    pc: u32,
    projection: &[u8],
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"fn64:mmio-polling-stutter-state:v1\0");
    digest.update((projection.len() as u64).to_be_bytes());
    digest.update(projection);
    digest.update(pc.to_be_bytes());
    for value in snapshot.gprs {
        digest.update(value.to_be_bytes());
    }
    digest.update(snapshot.hi.to_be_bytes());
    digest.update(snapshot.lo.to_be_bytes());
    for value in snapshot.physical_fgrs {
        digest.update(value.to_be_bytes());
    }
    digest.update([u8::from(snapshot.fpu_cond)]);
    digest.update(snapshot.fcsr.to_be_bytes());
    update_option_reservation(&mut digest, snapshot.ll_reservation);
    digest.update(snapshot.cop0_count.to_be_bytes());
    digest.update(snapshot.cop0_compare.to_be_bytes());
    update_option_u32(&mut digest, snapshot.cop0_count_write);
    update_option_u32(&mut digest, snapshot.cop0_compare_write);
    digest.update([u8::from(snapshot.cop0_cond)]);
    digest.update(snapshot.cop0_status.to_be_bytes());
    digest.update(snapshot.cop0_cause.to_be_bytes());
    digest.update(snapshot.cop0_epc.to_be_bytes());
    digest.update(snapshot.cop0_error_epc.to_be_bytes());
    digest.update(snapshot.cop0_badvaddr.to_be_bytes());
    digest.update(snapshot.cop0_context.to_be_bytes());
    digest.update(snapshot.cop0_xcontext.to_be_bytes());
    digest.update(snapshot.cop0_index.to_be_bytes());
    for entry in snapshot.tlb_entries {
        digest.update(entry.page_mask.to_be_bytes());
        digest.update(entry.entry_hi.to_be_bytes());
        digest.update(entry.entry_lo0.to_be_bytes());
        digest.update(entry.entry_lo1.to_be_bytes());
    }
    digest.update(snapshot.cop0_entry_lo0.to_be_bytes());
    digest.update(snapshot.cop0_entry_lo1.to_be_bytes());
    digest.update(snapshot.cop0_page_mask.to_be_bytes());
    digest.update(snapshot.cop0_wired.to_be_bytes());
    digest.update(snapshot.cop0_entry_hi.to_be_bytes());
    digest.update(snapshot.cop0_random_phase.to_be_bytes());
    digest.update(snapshot.cop0_watch_lo.to_be_bytes());
    digest.update(snapshot.cop0_watch_hi.to_be_bytes());
    digest.update(snapshot.os_interrupt_mask.to_be_bytes());
    update_option_u32(&mut digest, snapshot.thread_return_pc);
    hex(digest.finalize().into())
}

fn update_option_u32(digest: &mut Sha256, value: Option<u32>) {
    match value {
        Some(value) => {
            digest.update([1]);
            digest.update(value.to_be_bytes());
        }
        None => digest.update([0]),
    }
}

fn update_option_reservation(digest: &mut Sha256, value: Option<(u64, u8)>) {
    match value {
        Some((address, width)) => {
            digest.update([1]);
            digest.update(address.to_be_bytes());
            digest.update([width]);
        }
        None => digest.update([0]),
    }
}

fn hash_transcript(units: &[UnitEvidence]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"fn64:mmio-polling-stutter-transcript:v1\0");
    digest.update((units.len() as u64).to_be_bytes());
    for unit in units {
        digest.update(unit.entry_pc.to_be_bytes());
        digest.update(unit.identity);
        digest.update((unit.physical_addresses.len() as u64).to_be_bytes());
        for address in &unit.physical_addresses {
            digest.update(address.to_be_bytes());
        }
        digest.update(unit.retired_instructions.to_be_bytes());
        digest.update([unit.exit_tag]);
        digest.update(unit.next_pc.to_be_bytes());
    }
    hex(digest.finalize().into())
}

fn sha256(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes).into())
}

fn canonical_rdram_sha256(raw_backing: &[u8]) -> String {
    let mut backing = raw_backing.to_vec();
    let mem = Rdram::new(&mut backing);
    sha256(&mem.copy_physical_bytes(0, RDRAM_LEN as u32))
}

fn hex(bytes: [u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE_VA: u32 = 0x8000_0400;
    const CODE_PA: u32 = 0x0000_0400;
    const HEAD: u32 = 0x8000_04ac;
    const JOIN: u32 = 0x8000_04c0;
    const HELPER: u32 = 0x8000_20e0;
    const STATUS: u64 = 0xffff_ffff_a460_0010;

    struct Fixture {
        code: Vec<u8>,
        backing: Vec<u8>,
        context: RecompContext,
    }

    impl Fixture {
        fn new() -> Self {
            let mut code = vec![0u8; 0x1cf0];
            put_word(&mut code, HEAD, 0x0c00_0838); // jal helper
            put_word(&mut code, HEAD + 4, 0x0000_0000); // nop
            put_word(&mut code, HEAD + 8, 0x3048_0001); // andi t0,v0,1
            put_word(&mut code, HEAD + 12, 0x1500_fffc); // bne t0,zero,head
            put_word(&mut code, HEAD + 16, 0x0000_0000); // nop
            put_word(&mut code, HELPER, 0x3c0e_a460); // lui t6,0xa460
            put_word(&mut code, HELPER + 4, 0x03e0_0008); // jr ra
            put_word(&mut code, HELPER + 8, 0x8dc2_0010); // lw v0,0x10(t6)
            let mut backing = vec![0u8; RDRAM_LEN];
            for (index, byte) in code.iter().copied().enumerate() {
                backing[(CODE_PA as usize + index) ^ 3] = byte;
            }
            Self {
                code,
                backing,
                context: RecompContext::new(),
            }
        }

        fn request(&self) -> MmioPollingStutterRequestV1<'_> {
            let loop_offset = (HEAD - CODE_VA) as usize;
            let helper_offset = (HELPER - CODE_VA) as usize;
            MmioPollingStutterRequestV1 {
                code: vec![
                    PollingCodeSpanV1 {
                        virtual_start: HEAD,
                        physical_start: CODE_PA + (HEAD - CODE_VA),
                        bytes: &self.code[loop_offset..loop_offset + 0x14],
                    },
                    PollingCodeSpanV1 {
                        virtual_start: HELPER,
                        physical_start: CODE_PA + (HELPER - CODE_VA),
                        bytes: &self.code[helper_offset..helper_offset + 0x0c],
                    },
                ],
                initial_context: &self.context,
                initial_rdram: &self.backing,
                ready_rdram: &self.backing,
                cycle_head_pc: HEAD,
                join_pc: JOIN,
                status_vaddr: STATUS,
                busy_status_word: 1,
                ready_status_word: 0,
            }
        }

        fn replace(&mut self, pc: u32, word: u32) {
            put_word(&mut self.code, pc, word);
            let offset = (pc - CODE_VA) as usize;
            for index in 0..4 {
                self.backing[(CODE_PA as usize + offset + index) ^ 3] = self.code[offset + index];
            }
        }
    }

    fn put_word(code: &mut [u8], pc: u32, word: u32) {
        let offset = (pc - CODE_VA) as usize;
        code[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
    }

    #[test]
    fn exact_busy_cycle_is_inductive_and_preserves_two_ready_classes() {
        let fixture = Fixture::new();
        let validation = validate_mmio_polling_stutter_v1(
            &fixture.request(),
            MmioPollingStutterLimitsV1::default(),
        )
        .unwrap();
        let certificate = validation.certificate();
        assert_eq!(certificate.first_busy.units, 5);
        assert_eq!(certificate.first_busy.retired_instructions, 8);
        assert_eq!(
            certificate.steady_busy.transcript_sha256,
            certificate.repeated_steady_busy.transcript_sha256
        );
        assert_eq!(
            certificate.delayed_ready.normalized_end_state_sha256,
            certificate
                .repeated_delayed_ready
                .normalized_end_state_sha256
        );
        assert_eq!(validation.immediate_join_representative().r_u32(8), 0);
        assert_eq!(validation.delayed_join_representative().r_u32(8), 0);
        assert_eq!(
            certificate.clock_obligation.quotiented_context_fields,
            ["cop0_count", "cop0_random_phase"]
        );
        assert!(
            certificate
                .clock_obligation
                .downstream_must_prove_unobservable
        );
    }

    #[test]
    fn store_is_rejected_before_execution() {
        let mut fixture = Fixture::new();
        fixture.replace(HEAD + 8, 0xafa0_0000);
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::UnsupportedInstruction { .. })
        ));
    }

    #[test]
    fn store_in_control_delay_slot_is_rejected_before_execution() {
        let mut fixture = Fixture::new();
        fixture.replace(HEAD + 4, 0xafa0_0000);
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::UnsupportedInstruction { pc, .. })
                if pc == HEAD + 4
        ));
    }

    #[test]
    fn wrong_mmio_address_is_loud() {
        let mut fixture = Fixture::new();
        fixture.replace(HELPER + 8, 0x8dc2_0014);
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::UnexpectedExit { .. })
        ));
    }

    #[test]
    fn ordinary_rdram_load_cannot_fall_through_the_exact_port() {
        let mut fixture = Fixture::new();
        fixture.replace(HELPER, 0x3c0e_8000); // lui t6,0x8000
        fixture.replace(HELPER + 8, 0x8dc2_0000); // lw v0,0(t6)
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::UnexpectedExit { .. })
        ));
    }

    #[test]
    fn backed_kseg1_alias_cannot_be_declared_as_status_mmio() {
        let fixture = Fixture::new();
        let mut request = fixture.request();
        request.status_vaddr = 0xffff_ffff_a000_0010;
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&request, Default::default()),
            Err(MmioPollingStutterErrorV1::InvalidStatusAddress)
        ));
    }

    #[test]
    fn ready_paths_use_and_commit_the_post_device_backing() {
        let fixture = Fixture::new();
        let mut ready = fixture.backing.clone();
        ready[0x0010_0000usize ^ 3] = 0x5a;
        let mut request = fixture.request();
        request.ready_rdram = &ready;
        let validation = validate_mmio_polling_stutter_v1(&request, Default::default()).unwrap();
        assert_ne!(
            validation.certificate().initial_rdram_sha256,
            validation.certificate().ready_rdram_sha256
        );
    }

    #[test]
    fn ready_backing_cannot_change_committed_polling_code() {
        let fixture = Fixture::new();
        let mut ready = fixture.backing.clone();
        ready[((CODE_PA + (HEAD - CODE_VA)) as usize) ^ 3] ^= 1;
        let mut request = fixture.request();
        request.ready_rdram = &ready;
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&request, Default::default()),
            Err(MmioPollingStutterErrorV1::CodeBackingMismatch)
        ));
    }

    #[test]
    fn overlapping_virtual_or_physical_code_spans_are_rejected() {
        let fixture = Fixture::new();
        let mut virtual_overlap = fixture.request();
        virtual_overlap.code[1].virtual_start = HEAD + 4;
        virtual_overlap.code[1].physical_start = CODE_PA + (HEAD + 4 - CODE_VA);
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&virtual_overlap, Default::default()),
            Err(MmioPollingStutterErrorV1::OverlappingCodeSpans)
        ));

        let mut physical_alias = fixture.request();
        physical_alias.code[1].physical_start = physical_alias.code[0].physical_start;
        physical_alias.code[1].virtual_start = 0xa000_04ac;
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&physical_alias, Default::default()),
            Err(MmioPollingStutterErrorV1::OverlappingCodeSpans)
        ));
    }

    #[test]
    fn cop0_count_read_is_rejected_before_execution() {
        let mut fixture = Fixture::new();
        fixture.replace(HEAD + 8, 0x4008_4800);
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::UnsupportedInstruction { .. })
        ));
    }

    #[test]
    fn non_recurrent_integer_state_is_rejected() {
        let mut fixture = Fixture::new();
        fixture.replace(HEAD + 4, 0x2610_0001); // addiu s0,s0,1
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::BusyCycleNotRecurrent)
        ));
    }

    #[test]
    fn call_outside_committed_code_is_rejected() {
        let mut fixture = Fixture::new();
        fixture.replace(HEAD, 0x0c00_2140); // jal 0x80008500
        assert!(matches!(
            validate_mmio_polling_stutter_v1(&fixture.request(), Default::default()),
            Err(MmioPollingStutterErrorV1::UnexpectedExit { .. })
        ));
    }

    #[test]
    fn initial_cpu_state_is_bound_even_when_the_loop_overwrites_scratch() {
        let fixture_a = Fixture::new();
        let mut fixture_b = Fixture::new();
        fixture_b.context.cop0_status = 1;
        let a = validate_mmio_polling_stutter_v1(&fixture_a.request(), Default::default()).unwrap();
        let b = validate_mmio_polling_stutter_v1(&fixture_b.request(), Default::default()).unwrap();
        assert_ne!(
            a.certificate().initial_state_sha256,
            b.certificate().initial_state_sha256
        );
        assert_eq!(a.initial_context().cop0_status, 0);
        assert_eq!(b.initial_context().cop0_status, 1);
    }

    #[test]
    fn serialized_certificate_contains_commitments_not_code_words() {
        let fixture = Fixture::new();
        let validation = validate_mmio_polling_stutter_v1(
            &fixture.request(),
            MmioPollingStutterLimitsV1::default(),
        )
        .unwrap();
        let value = serde_json::to_value(validation.certificate()).unwrap();
        let object = value.as_object().unwrap();
        assert!(object.contains_key("code"));
        assert!(!object.contains_key("words"));
        assert!(!object.contains_key("bytes"));
        for span in object["code"].as_array().unwrap() {
            let span = span.as_object().unwrap();
            assert!(span.contains_key("sha256"));
            assert!(!span.contains_key("words"));
            assert!(!span.contains_key("bytes"));
        }
    }
}
