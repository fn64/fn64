//! Candidate-only evidence for exact transform-wrapper invocations.
//!
//! The evaluator constructs a fresh CPU/RDRAM machine, admits only a narrow
//! dependency-audited integer subset, and requires the completed destination
//! bytes to equal a re-derived [`EvaluatedImageReceiptV1`] output exactly. The
//! resulting certificates prove only one invocation, or one declared ordered
//! one-call-per-stream sequence with shared mutable memory, from committed
//! inputs. They do not promote a fact, prove general transform semantics, or
//! establish runtime placement, reachability, or release authority.

use crate::banks::materialize_rom_range_bounded;
use crate::facts::{evaluated_image_receipt_sha256_v1, EvaluatedImageReceiptV1, FactDb};
use crate::materialized_image::{rederive_materialized_image_v1, MaterializedImageLimitsV1};
use crate::NormalizedRom;
use fn64_recomp_rs::{
    decode, dynamic_mapped_execution_build_receipt_v1, set_read_observer, set_write_observer,
    BankId, BlockExit, DynamicMappedUnitCatalogV1, ExecutionKey, GuestPc, GuestReadEvent,
    GuestWriteEvent, Instruction, InstructionBudget, Rdram, RecompContext, WriterChannel,
    RDRAM_LEN,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::BTreeSet;

pub const TRANSFORM_INVOCATION_CERTIFICATE_SCHEMA_V1: &str =
    "fn64.transform-invocation-certificate.v1";
pub const TRANSFORM_INVOCATION_SEQUENCE_CERTIFICATE_SCHEMA_V1: &str =
    "fn64.transform-invocation-sequence-certificate.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalRangeV1 {
    pub start: u32,
    pub len: u32,
}

impl PhysicalRangeV1 {
    fn end(self) -> Option<u32> {
        self.start.checked_add(self.len)
    }

    fn contains(self, start: u32, len: u32) -> bool {
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        self.len != 0 && start >= self.start && self.end().is_some_and(|own_end| end <= own_end)
    }

    fn intersects(self, other: Self) -> bool {
        match (self.end(), other.end()) {
            (Some(a_end), Some(b_end)) => self.start < b_end && other.start < a_end,
            _ => true,
        }
    }
}

struct ReadAuthorityV1 {
    bytes: Vec<bool>,
}

impl ReadAuthorityV1 {
    fn from_ranges(ranges: &[PhysicalRangeV1]) -> Self {
        let mut authority = Self {
            bytes: vec![false; RDRAM_LEN],
        };
        for range in ranges {
            authority.mark(*range);
        }
        authority
    }

    fn allows(&self, start: u32, len: u32) -> bool {
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        if len == 0 || end as usize > self.bytes.len() {
            return false;
        }
        (start as usize..end as usize).all(|offset| self.bytes[offset])
    }

    fn mark(&mut self, range: PhysicalRangeV1) {
        let end = range
            .end()
            .expect("validated read-authority range end must fit in u32");
        self.bytes[range.start as usize..end as usize].fill(true);
    }

    fn clear(&mut self, range: PhysicalRangeV1) {
        let end = range
            .end()
            .expect("validated read-authority range end must fit in u32");
        self.bytes[range.start as usize..end as usize].fill(false);
    }
}

#[derive(Clone, Debug)]
pub struct KnownTransformCodeImageV1<'a> {
    pub virtual_start: u32,
    pub physical_start: u32,
    pub bytes: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct CommittedMemoryRangeV1<'a> {
    pub role: &'a str,
    pub physical_start: u32,
    pub bytes: &'a [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GprSeedV1 {
    pub register: u8,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransformInvocationLimitsV1 {
    pub max_units: u32,
    pub max_instructions: u32,
    pub materialized_image: MaterializedImageLimitsV1,
}

impl Default for TransformInvocationLimitsV1 {
    fn default() -> Self {
        Self {
            max_units: 100_000,
            max_instructions: 200_000,
            materialized_image: MaterializedImageLimitsV1::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TransformInvocationRequestV1<'a> {
    pub entry_pc: u32,
    pub return_pc: u32,
    pub a0: u32,
    pub a1: u32,
    pub additional_gpr_seeds: &'a [GprSeedV1],
    pub expected_output: ExpectedEvaluatedOutputV1,
    pub code: KnownTransformCodeImageV1<'a>,
    pub source_physical_start: u32,
    pub output_physical_start: u32,
    pub committed_memory: &'a [CommittedMemoryRangeV1<'a>],
    pub additional_allowed_writes: &'a [PhysicalRangeV1],
}

/// One call boundary in an ordered transform sequence. Each step must select
/// the receipt stream with the same ordinal as its position in the sequence;
/// the evaluator rejects subsets and reorderings rather than silently
/// certifying something weaker than the aggregate.
#[derive(Clone, Debug)]
pub struct TransformInvocationStepRequestV1<'a> {
    pub entry_pc: u32,
    pub return_pc: u32,
    pub a0: u32,
    pub a1: u32,
    pub additional_gpr_seeds: &'a [GprSeedV1],
    pub expected_output: ExpectedEvaluatedOutputV1,
    pub expected_mutable_memory_after: &'a [CommittedMemoryRangeV1<'a>],
}

/// Initial bytes which remain shared and writable across every declared
/// invocation. Their pre/post commitments make pointer-cell evolution part of
/// the sequence identity instead of an inferred property of adjacent calls.
#[derive(Clone, Debug)]
pub struct SharedMutableMemoryRangeV1<'a> {
    pub role: &'a str,
    pub physical_start: u32,
    pub initial_bytes: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct TransformInvocationSequenceRequestV1<'a> {
    pub steps: &'a [TransformInvocationStepRequestV1<'a>],
    pub code: KnownTransformCodeImageV1<'a>,
    pub source_physical_start: u32,
    pub output_physical_start: u32,
    pub committed_memory: &'a [CommittedMemoryRangeV1<'a>],
    pub shared_mutable_memory: &'a [SharedMutableMemoryRangeV1<'a>],
    pub additional_allowed_writes: &'a [PhysicalRangeV1],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExpectedEvaluatedOutputV1 {
    Aggregate,
    Stream { ordinal: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentCommitmentV1 {
    pub role: String,
    pub physical_start: u32,
    pub len: u32,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformMemoryEventV1 {
    Read { physical_offset: u32, len: u32 },
    Write { physical_offset: u32, len: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformUnitTranscriptV1 {
    pub entry_pc: u32,
    pub identity_sha256: String,
    pub instruction_physical_addresses: Vec<u32>,
    pub words: Vec<u32>,
    pub retired_instructions: u32,
    pub exit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Replay evidence for one implementation-issued evaluation, not an authority
/// capability. Deserializing this record cannot recreate the opaque
/// [`TransformInvocationEvaluationV1`] returned by the evaluator.
pub struct TransformInvocationCertificateV1 {
    pub schema: String,
    pub evaluated_image_receipt_sha256: String,
    pub dynamic_semantics_schema: String,
    pub dynamic_semantics_sha256: String,
    pub dynamic_semantics_available: bool,
    pub dynamic_semantics_general_dev_interpreter: bool,
    pub code_virtual_start: u32,
    pub code: ContentCommitmentV1,
    pub initial_a0: u32,
    pub initial_a1: u32,
    pub additional_gpr_seeds: Vec<GprSeedV1>,
    pub return_pc: u32,
    pub expected_output: ExpectedEvaluatedOutputV1,
    pub expected_output_start: u32,
    pub expected_output_end: u32,
    pub committed_memory: Vec<ContentCommitmentV1>,
    pub allowed_writes: Vec<PhysicalRangeV1>,
    pub max_units: u32,
    pub max_instructions: u32,
    pub units: Vec<TransformUnitTranscriptV1>,
    pub memory_events: Vec<TransformMemoryEventV1>,
    pub retired_instructions: u32,
    pub output: ContentCommitmentV1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformInvocationSequenceStepCertificateV1 {
    pub ordinal: u32,
    pub entry_pc: u32,
    pub initial_a0: u32,
    pub initial_a1: u32,
    pub additional_gpr_seeds: Vec<GprSeedV1>,
    pub return_pc: u32,
    pub expected_output: ExpectedEvaluatedOutputV1,
    pub expected_output_start: u32,
    pub expected_output_end: u32,
    pub mutable_memory_before: Vec<ContentCommitmentV1>,
    pub mutable_memory_after: Vec<ContentCommitmentV1>,
    pub allowed_writes: Vec<PhysicalRangeV1>,
    pub units: Vec<TransformUnitTranscriptV1>,
    pub memory_events: Vec<TransformMemoryEventV1>,
    pub retired_instructions: u32,
    pub output: ContentCommitmentV1,
}

/// Replay evidence for a complete ordered stream sequence in one shared fresh
/// RDRAM machine. Like the single-invocation certificate, this is not an
/// authority capability and does not prove that boot selects the calls.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformInvocationSequenceCertificateV1 {
    pub schema: String,
    pub evaluated_image_receipt_sha256: String,
    pub dynamic_semantics_schema: String,
    pub dynamic_semantics_sha256: String,
    pub dynamic_semantics_available: bool,
    pub dynamic_semantics_general_dev_interpreter: bool,
    pub code_virtual_start: u32,
    pub code: ContentCommitmentV1,
    pub committed_memory: Vec<ContentCommitmentV1>,
    pub initial_mutable_memory: Vec<ContentCommitmentV1>,
    pub max_units: u32,
    pub max_instructions: u32,
    pub steps: Vec<TransformInvocationSequenceStepCertificateV1>,
    pub executed_units: u32,
    pub retired_instructions: u32,
    pub output: ContentCommitmentV1,
}

pub fn transform_invocation_sequence_certificate_sha256_v1(
    certificate: &TransformInvocationSequenceCertificateV1,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.transform-invocation-sequence-certificate.identity.v1\0");
    hasher.update(
        serde_json::to_vec(certificate)
            .expect("transform invocation sequence certificate serializes"),
    );
    format!("{:x}", hasher.finalize())
}

pub fn transform_invocation_certificate_sha256_v1(
    certificate: &TransformInvocationCertificateV1,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"fn64.transform-invocation-certificate.identity.v1\0");
    hasher.update(
        serde_json::to_vec(certificate).expect("transform invocation certificate serializes"),
    );
    format!("{:x}", hasher.finalize())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformInvocationEvaluationV1 {
    certificate: TransformInvocationCertificateV1,
    output: Vec<u8>,
}

impl TransformInvocationEvaluationV1 {
    pub fn certificate(&self) -> &TransformInvocationCertificateV1 {
        &self.certificate
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformInvocationSequenceEvaluationV1 {
    certificate: TransformInvocationSequenceCertificateV1,
    output: Vec<u8>,
}

impl TransformInvocationSequenceEvaluationV1 {
    pub fn certificate(&self) -> &TransformInvocationSequenceCertificateV1 {
        &self.certificate
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransformInvocationErrorV1 {
    InvalidInput(&'static str),
    Materialization(String),
    DynamicExecution(String),
    UnitLimitExceeded,
    InstructionLimitExceeded,
    CodeEscape {
        pc: u32,
    },
    CodeWrite {
        physical_offset: u32,
        len: u32,
    },
    ReadOutsideCommitted {
        physical_offset: u32,
        len: u32,
    },
    WriteOutsideAllowed {
        physical_offset: u32,
        len: u32,
    },
    NonCpuWrite {
        channel: WriterChannel,
    },
    UnsupportedInstruction {
        pc: u32,
        instruction: String,
    },
    UnseededRegisterRead {
        pc: u32,
        register: u8,
    },
    MemoryEventMismatch {
        pc: u32,
    },
    RejectedExit(String),
    OutputNotFullyWritten {
        first_unwritten_physical_offset: u32,
    },
    MutableMemoryMismatch {
        ordinal: u32,
        role: String,
    },
    OutputMismatch,
}

thread_local! {
    static ACTIVE_JOURNAL: RefCell<Option<Vec<ObservedEvent>>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug)]
enum ObservedEvent {
    Read(GuestReadEvent),
    Write(GuestWriteEvent),
}

fn observe_read(event: GuestReadEvent) {
    ACTIVE_JOURNAL.with(|journal| {
        journal
            .borrow_mut()
            .as_mut()
            .expect("transform read observer called outside evaluation")
            .push(ObservedEvent::Read(event));
    });
}

fn observe_write(event: GuestWriteEvent) {
    ACTIVE_JOURNAL.with(|journal| {
        journal
            .borrow_mut()
            .as_mut()
            .expect("transform write observer called outside evaluation")
            .push(ObservedEvent::Write(event));
    });
}

struct ObserverGuard;

impl ObserverGuard {
    fn install() -> Self {
        ACTIVE_JOURNAL.with(|journal| {
            assert!(journal.borrow().is_none(), "nested transform evaluation");
            *journal.borrow_mut() = Some(Vec::new());
        });
        set_read_observer(Some(observe_read));
        set_write_observer(Some(observe_write));
        Self
    }
}

impl Drop for ObserverGuard {
    fn drop(&mut self) {
        set_read_observer(None);
        set_write_observer(None);
        ACTIVE_JOURNAL.with(|journal| *journal.borrow_mut() = None);
    }
}

fn take_observed_events() -> Vec<ObservedEvent> {
    ACTIVE_JOURNAL.with(|journal| {
        std::mem::take(
            journal
                .borrow_mut()
                .as_mut()
                .expect("active transform journal exists"),
        )
    })
}

struct InvocationSeedV1<'a> {
    entry_pc: u32,
    return_pc: u32,
    a0: u32,
    a1: u32,
    additional_gpr_seeds: &'a [GprSeedV1],
}

struct InvocationRunV1 {
    units: Vec<TransformUnitTranscriptV1>,
    memory_events: Vec<TransformMemoryEventV1>,
    retired_instructions: u32,
}

#[allow(clippy::too_many_arguments)]
fn run_declared_invocation_v1(
    seed: InvocationSeedV1<'_>,
    code: &KnownTransformCodeImageV1<'_>,
    code_range: PhysicalRangeV1,
    catalog: &mut DynamicMappedUnitCatalogV1,
    mem: &mut Rdram<'_>,
    authority: &mut ReadAuthorityV1,
    allowed_writes: &[PhysicalRangeV1],
    max_units: u32,
    max_instructions: u32,
) -> Result<InvocationRunV1, TransformInvocationErrorV1> {
    let mut context = RecompContext::new();
    context.set_r32(4, seed.a0 as i32);
    context.set_r32(5, seed.a1 as i32);
    context.set_r32(31, seed.return_pc as i32);
    context.set_thread_return_pc(Some(seed.return_pc));
    let mut defined = BTreeSet::from([0u8, 4, 5, 31]);
    for register_seed in seed.additional_gpr_seeds {
        context.set_r(register_seed.register, register_seed.value);
        defined.insert(register_seed.register);
    }

    let mut pc = seed.entry_pc;
    let mut units = Vec::new();
    let mut transcript_events = Vec::new();
    let mut retired_total = 0u32;
    loop {
        if units.len() >= max_units as usize {
            return Err(TransformInvocationErrorV1::UnitLimitExceeded);
        }
        validate_code_pc(code, pc)?;
        let primary = decode(word_at_pc(code, pc)?);
        let likely_delay_executes = match primary {
            Instruction::Beql { rs, rt, .. } => context.r(rs) == context.r(rt),
            Instruction::Bnel { rs, rt, .. } => context.r(rs) != context.r(rt),
            Instruction::Blezl { rs, .. } => context.r_s64(rs) <= 0,
            Instruction::Bgtzl { rs, .. } => context.r_s64(rs) > 0,
            _ => true,
        };
        let run = catalog
            .activate_and_run(
                ExecutionKey::new(BankId::new(0), GuestPc::new(pc)),
                InstructionBudget::new(2).unwrap(),
                &mut context,
                mem,
                |_| false,
            )
            .map_err(|error| TransformInvocationErrorV1::DynamicExecution(error.to_string()))?;
        retired_total = retired_total
            .checked_add(run.run.instructions)
            .ok_or(TransformInvocationErrorV1::InstructionLimitExceeded)?;
        if retired_total > max_instructions {
            return Err(TransformInvocationErrorV1::InstructionLimitExceeded);
        }
        let observed = take_observed_events();
        let words = words_for_run(code, &run.instructions)?;
        audit_unit(
            pc,
            &words,
            run.run.instructions,
            &observed,
            &mut defined,
            authority,
            allowed_writes,
            code_range,
            &mut transcript_events,
            likely_delay_executes,
        )?;
        let (exit_name, next) = classify_exit(run.run.exit)?;
        units.push(TransformUnitTranscriptV1 {
            entry_pc: pc,
            identity_sha256: hex(&run.identity.bytes()),
            instruction_physical_addresses: run
                .instructions
                .iter()
                .map(|instruction| instruction.physical_address)
                .collect(),
            words,
            retired_instructions: run.run.instructions,
            exit: exit_name,
        });
        match next {
            Some(next_pc) => {
                validate_code_pc(code, next_pc)?;
                pc = next_pc;
            }
            None => break,
        }
    }
    Ok(InvocationRunV1 {
        units,
        memory_events: transcript_events,
        retired_instructions: retired_total,
    })
}

pub fn certify_transform_wrapper_invocation_v1(
    rom: &NormalizedRom,
    facts: &FactDb,
    expected: &EvaluatedImageReceiptV1,
    request: &TransformInvocationRequestV1<'_>,
    limits: TransformInvocationLimitsV1,
) -> Result<TransformInvocationEvaluationV1, TransformInvocationErrorV1> {
    std::thread::scope(|scope| {
        match scope
            .spawn(|| {
                certify_transform_wrapper_invocation_isolated_v1(
                    rom, facts, expected, request, limits,
                )
            })
            .join()
        {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

fn certify_transform_wrapper_invocation_isolated_v1(
    rom: &NormalizedRom,
    facts: &FactDb,
    expected: &EvaluatedImageReceiptV1,
    request: &TransformInvocationRequestV1<'_>,
    limits: TransformInvocationLimitsV1,
) -> Result<TransformInvocationEvaluationV1, TransformInvocationErrorV1> {
    validate_limits_and_layout(request, limits)?;
    let evaluation =
        rederive_materialized_image_v1(rom, facts, expected, limits.materialized_image)
            .map_err(|error| TransformInvocationErrorV1::Materialization(error.to_string()))?;
    let (expected_output_start, expected_output_end) = match request.expected_output {
        ExpectedEvaluatedOutputV1::Aggregate => (0, expected.output_len),
        ExpectedEvaluatedOutputV1::Stream { ordinal } => {
            let stream = expected.streams.get(ordinal as usize).ok_or(
                TransformInvocationErrorV1::InvalidInput(
                    "expected output stream ordinal is outside the receipt",
                ),
            )?;
            (stream.output_range.start, stream.output_range.end)
        }
    };
    let expected_output = evaluation
        .bytes()
        .get(expected_output_start as usize..expected_output_end as usize)
        .ok_or(TransformInvocationErrorV1::InvalidInput(
            "expected output range is outside the evaluated image",
        ))?;
    let expected_receipt_sha256 = evaluated_image_receipt_sha256_v1(expected);
    let output_initial = output_poison(
        &expected_receipt_sha256,
        expected_output_start,
        expected_output_end,
        request.output_physical_start,
        expected_output.len(),
    );
    let source = materialize_rom_range_bounded(
        rom,
        facts,
        expected.source.rom_space,
        expected.source.rom_start,
        expected.source.rom_end,
        limits.materialized_image.max_decoded_vrom_file_bytes,
    )
    .map_err(TransformInvocationErrorV1::Materialization)?;
    if sha256(&source.bytes) != expected.source_sha256 {
        return Err(TransformInvocationErrorV1::Materialization(
            "materialized source digest disagrees with evaluated receipt".to_owned(),
        ));
    }

    let code_range = range_of(request.code.physical_start, request.code.bytes)?;
    let source_range = range_of(request.source_physical_start, &source.bytes)?;
    let output_range = range_of(request.output_physical_start, &output_initial)?;
    let mut initial_ranges = vec![code_range, source_range, output_range];
    for range in request.committed_memory {
        initial_ranges.push(range_of(range.physical_start, range.bytes)?);
    }
    reject_overlaps(&initial_ranges)?;

    let mut allowed_writes = vec![output_range];
    allowed_writes.extend_from_slice(request.additional_allowed_writes);
    for range in &allowed_writes {
        validate_range(*range)?;
        if range.intersects(range_of(request.code.physical_start, request.code.bytes)?) {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "allowed write range intersects code image",
            ));
        }
    }
    allowed_writes.sort_unstable_by_key(|range| (range.start, range.len));
    allowed_writes.dedup();

    let mut backing = vec![0u8; RDRAM_LEN];
    let mut mem = Rdram::new(&mut backing);
    install_bytes(&mut mem, request.code.physical_start, request.code.bytes);
    install_bytes(&mut mem, request.source_physical_start, &source.bytes);
    install_bytes(&mut mem, request.output_physical_start, &output_initial);
    for range in request.committed_memory {
        install_bytes(&mut mem, range.physical_start, range.bytes);
    }

    let _observers = ObserverGuard::install();
    let semantics = dynamic_mapped_execution_build_receipt_v1();
    if !semantics.available() {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "dynamic mapped execution capability is unavailable",
        ));
    }
    let mut catalog = DynamicMappedUnitCatalogV1::new_linked();
    let mut committed = vec![code_range, source_range];
    committed.extend(
        request
            .committed_memory
            .iter()
            .map(|range| range_of(range.physical_start, range.bytes))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut authority = ReadAuthorityV1::from_ranges(&committed);
    let run = run_declared_invocation_v1(
        InvocationSeedV1 {
            entry_pc: request.entry_pc,
            return_pc: request.return_pc,
            a0: request.a0,
            a1: request.a1,
            additional_gpr_seeds: request.additional_gpr_seeds,
        },
        &request.code,
        code_range,
        &mut catalog,
        &mut mem,
        &mut authority,
        &allowed_writes,
        limits.max_units,
        limits.max_instructions,
    )?;

    let output = mem.copy_physical_bytes(
        request.output_physical_start,
        u32::try_from(output_initial.len())
            .map_err(|_| TransformInvocationErrorV1::InvalidInput("output length exceeds u32"))?,
    );
    require_full_output_write_coverage(output_range, &run.memory_events)?;
    if output != expected_output {
        return Err(TransformInvocationErrorV1::OutputMismatch);
    }

    let code_commitment = commitment("code", request.code.physical_start, request.code.bytes)?;
    let mut commitments = vec![
        commitment(
            "evaluated_source",
            request.source_physical_start,
            &source.bytes,
        )?,
        commitment(
            "output_initial_poison",
            request.output_physical_start,
            &output_initial,
        )?,
    ];
    commitments.extend(
        request
            .committed_memory
            .iter()
            .map(|range| commitment(range.role, range.physical_start, range.bytes))
            .collect::<Result<Vec<_>, _>>()?,
    );
    commitments.sort_unstable_by(|left, right| {
        (
            left.physical_start,
            left.len,
            left.role.as_str(),
            left.sha256.as_str(),
        )
            .cmp(&(
                right.physical_start,
                right.len,
                right.role.as_str(),
                right.sha256.as_str(),
            ))
    });
    let certificate = TransformInvocationCertificateV1 {
        schema: TRANSFORM_INVOCATION_CERTIFICATE_SCHEMA_V1.to_owned(),
        evaluated_image_receipt_sha256: expected_receipt_sha256,
        dynamic_semantics_schema: semantics.schema().to_owned(),
        dynamic_semantics_sha256: hex(&semantics.source_sha256()),
        dynamic_semantics_available: semantics.available(),
        dynamic_semantics_general_dev_interpreter: semantics.general_dev_interpreter(),
        code_virtual_start: request.code.virtual_start,
        code: code_commitment,
        initial_a0: request.a0,
        initial_a1: request.a1,
        additional_gpr_seeds: request.additional_gpr_seeds.to_vec(),
        return_pc: request.return_pc,
        expected_output: request.expected_output,
        expected_output_start,
        expected_output_end,
        committed_memory: commitments,
        allowed_writes,
        max_units: limits.max_units,
        max_instructions: limits.max_instructions,
        units: run.units,
        memory_events: run.memory_events,
        retired_instructions: run.retired_instructions,
        output: commitment("output_final", request.output_physical_start, &output)?,
    };
    Ok(TransformInvocationEvaluationV1 {
        certificate,
        output,
    })
}

pub fn certify_transform_invocation_sequence_v1(
    rom: &NormalizedRom,
    facts: &FactDb,
    expected: &EvaluatedImageReceiptV1,
    request: &TransformInvocationSequenceRequestV1<'_>,
    limits: TransformInvocationLimitsV1,
) -> Result<TransformInvocationSequenceEvaluationV1, TransformInvocationErrorV1> {
    std::thread::scope(|scope| {
        match scope
            .spawn(|| {
                certify_transform_invocation_sequence_isolated_v1(
                    rom, facts, expected, request, limits,
                )
            })
            .join()
        {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}

fn certify_transform_invocation_sequence_isolated_v1(
    rom: &NormalizedRom,
    facts: &FactDb,
    expected: &EvaluatedImageReceiptV1,
    request: &TransformInvocationSequenceRequestV1<'_>,
    limits: TransformInvocationLimitsV1,
) -> Result<TransformInvocationSequenceEvaluationV1, TransformInvocationErrorV1> {
    validate_sequence_request(request, expected, limits)?;
    let evaluation =
        rederive_materialized_image_v1(rom, facts, expected, limits.materialized_image)
            .map_err(|error| TransformInvocationErrorV1::Materialization(error.to_string()))?;
    let expected_receipt_sha256 = evaluated_image_receipt_sha256_v1(expected);
    let output_initial = output_poison(
        &expected_receipt_sha256,
        0,
        expected.output_len,
        request.output_physical_start,
        evaluation.bytes().len(),
    );
    let source = materialize_rom_range_bounded(
        rom,
        facts,
        expected.source.rom_space,
        expected.source.rom_start,
        expected.source.rom_end,
        limits.materialized_image.max_decoded_vrom_file_bytes,
    )
    .map_err(TransformInvocationErrorV1::Materialization)?;
    if sha256(&source.bytes) != expected.source_sha256 {
        return Err(TransformInvocationErrorV1::Materialization(
            "materialized source digest disagrees with evaluated receipt".to_owned(),
        ));
    }

    let code_range = range_of(request.code.physical_start, request.code.bytes)?;
    let source_range = range_of(request.source_physical_start, &source.bytes)?;
    let output_range = range_of(request.output_physical_start, &output_initial)?;
    let immutable_ranges = request
        .committed_memory
        .iter()
        .map(|range| range_of(range.physical_start, range.bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let mutable_ranges = request
        .shared_mutable_memory
        .iter()
        .map(|range| range_of(range.physical_start, range.initial_bytes))
        .collect::<Result<Vec<_>, _>>()?;
    let mut initial_ranges = vec![code_range, source_range, output_range];
    initial_ranges.extend(immutable_ranges.iter().copied());
    initial_ranges.extend(mutable_ranges.iter().copied());
    reject_overlaps(&initial_ranges)?;

    reject_overlaps(request.additional_allowed_writes)?;
    for range in request.additional_allowed_writes {
        if initial_ranges
            .iter()
            .any(|initial| range.intersects(*initial))
        {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "additional allowed write range intersects committed, mutable, code, source, or output memory",
            ));
        }
    }

    let mut backing = vec![0u8; RDRAM_LEN];
    let mut mem = Rdram::new(&mut backing);
    install_bytes(&mut mem, request.code.physical_start, request.code.bytes);
    install_bytes(&mut mem, request.source_physical_start, &source.bytes);
    install_bytes(&mut mem, request.output_physical_start, &output_initial);
    for range in request.committed_memory {
        install_bytes(&mut mem, range.physical_start, range.bytes);
    }
    for range in request.shared_mutable_memory {
        install_bytes(&mut mem, range.physical_start, range.initial_bytes);
    }

    let _observers = ObserverGuard::install();
    let semantics = dynamic_mapped_execution_build_receipt_v1();
    if !semantics.available() {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "dynamic mapped execution capability is unavailable",
        ));
    }
    let mut catalog = DynamicMappedUnitCatalogV1::new_linked();
    let mut committed = vec![code_range, source_range];
    committed.extend(immutable_ranges);
    committed.extend(mutable_ranges.iter().copied());
    let mut authority = ReadAuthorityV1::from_ranges(&committed);

    let mut initial_commitments = vec![
        commitment(
            "evaluated_source",
            request.source_physical_start,
            &source.bytes,
        )?,
        commitment(
            "output_initial_poison",
            request.output_physical_start,
            &output_initial,
        )?,
    ];
    initial_commitments.extend(
        request
            .committed_memory
            .iter()
            .map(|range| commitment(range.role, range.physical_start, range.bytes))
            .collect::<Result<Vec<_>, _>>()?,
    );
    sort_commitments(&mut initial_commitments);
    let initial_mutable_memory = mutable_commitments(&mem, request.shared_mutable_memory)?;

    let mut total_units = 0u32;
    let mut total_instructions = 0u32;
    let mut step_certificates = Vec::with_capacity(request.steps.len());
    for (index, step) in request.steps.iter().enumerate() {
        let ordinal = u32::try_from(index)
            .map_err(|_| TransformInvocationErrorV1::InvalidInput("too many sequence steps"))?;
        let stream = &expected.streams[index];
        let selected_start = request
            .output_physical_start
            .checked_add(stream.output_range.start)
            .ok_or(TransformInvocationErrorV1::InvalidInput(
                "selected output range overflows RDRAM",
            ))?;
        let selected_len = stream
            .output_range
            .end
            .checked_sub(stream.output_range.start)
            .ok_or(TransformInvocationErrorV1::InvalidInput(
                "selected output range is reversed",
            ))?;
        let selected_range = PhysicalRangeV1 {
            start: selected_start,
            len: selected_len,
        };
        validate_range(selected_range)?;

        let mut allowed_writes = vec![selected_range];
        allowed_writes.extend(mutable_ranges.iter().copied());
        allowed_writes.extend_from_slice(request.additional_allowed_writes);
        allowed_writes.sort_unstable_by_key(|range| (range.start, range.len));
        allowed_writes.dedup();

        let mutable_memory_before = mutable_commitments(&mem, request.shared_mutable_memory)?;
        let remaining_units = limits
            .max_units
            .checked_sub(total_units)
            .filter(|remaining| *remaining != 0)
            .ok_or(TransformInvocationErrorV1::UnitLimitExceeded)?;
        let remaining_instructions = limits
            .max_instructions
            .checked_sub(total_instructions)
            .filter(|remaining| *remaining != 0)
            .ok_or(TransformInvocationErrorV1::InstructionLimitExceeded)?;
        let run = run_declared_invocation_v1(
            InvocationSeedV1 {
                entry_pc: step.entry_pc,
                return_pc: step.return_pc,
                a0: step.a0,
                a1: step.a1,
                additional_gpr_seeds: step.additional_gpr_seeds,
            },
            &request.code,
            code_range,
            &mut catalog,
            &mut mem,
            &mut authority,
            &allowed_writes,
            remaining_units,
            remaining_instructions,
        )?;
        total_units = total_units
            .checked_add(
                u32::try_from(run.units.len())
                    .map_err(|_| TransformInvocationErrorV1::UnitLimitExceeded)?,
            )
            .ok_or(TransformInvocationErrorV1::UnitLimitExceeded)?;
        total_instructions = total_instructions
            .checked_add(run.retired_instructions)
            .ok_or(TransformInvocationErrorV1::InstructionLimitExceeded)?;
        require_full_output_write_coverage(selected_range, &run.memory_events)?;
        let step_output = mem.copy_physical_bytes(selected_start, selected_len);
        let expected_step = evaluation
            .bytes()
            .get(stream.output_range.start as usize..stream.output_range.end as usize)
            .ok_or(TransformInvocationErrorV1::InvalidInput(
                "selected output range is outside the evaluated image",
            ))?;
        if step_output != expected_step {
            return Err(TransformInvocationErrorV1::OutputMismatch);
        }
        for expected_mutable in step.expected_mutable_memory_after {
            let len = u32::try_from(expected_mutable.bytes.len()).map_err(|_| {
                TransformInvocationErrorV1::InvalidInput("range length exceeds u32")
            })?;
            if mem.copy_physical_bytes(expected_mutable.physical_start, len)
                != expected_mutable.bytes
            {
                return Err(TransformInvocationErrorV1::MutableMemoryMismatch {
                    ordinal,
                    role: expected_mutable.role.to_owned(),
                });
            }
        }
        let mutable_memory_after = mutable_commitments(&mem, request.shared_mutable_memory)?;
        step_certificates.push(TransformInvocationSequenceStepCertificateV1 {
            ordinal,
            entry_pc: step.entry_pc,
            initial_a0: step.a0,
            initial_a1: step.a1,
            additional_gpr_seeds: step.additional_gpr_seeds.to_vec(),
            return_pc: step.return_pc,
            expected_output: step.expected_output,
            expected_output_start: stream.output_range.start,
            expected_output_end: stream.output_range.end,
            mutable_memory_before,
            mutable_memory_after,
            allowed_writes,
            units: run.units,
            memory_events: run.memory_events,
            retired_instructions: run.retired_instructions,
            output: commitment("step_output_final", selected_start, &step_output)?,
        });
        // Additional scratch is step-local dependency state. Bytes may remain
        // in the fresh machine, but no later invocation may read them until it
        // has overwritten them itself; cross-call state must use the explicit
        // shared-mutable seam and its before/after commitments.
        for scratch in request.additional_allowed_writes {
            authority.clear(*scratch);
        }
    }

    let output = mem.copy_physical_bytes(request.output_physical_start, expected.output_len);
    if output != evaluation.bytes() {
        return Err(TransformInvocationErrorV1::OutputMismatch);
    }
    let certificate = TransformInvocationSequenceCertificateV1 {
        schema: TRANSFORM_INVOCATION_SEQUENCE_CERTIFICATE_SCHEMA_V1.to_owned(),
        evaluated_image_receipt_sha256: expected_receipt_sha256,
        dynamic_semantics_schema: semantics.schema().to_owned(),
        dynamic_semantics_sha256: hex(&semantics.source_sha256()),
        dynamic_semantics_available: semantics.available(),
        dynamic_semantics_general_dev_interpreter: semantics.general_dev_interpreter(),
        code_virtual_start: request.code.virtual_start,
        code: commitment("code", request.code.physical_start, request.code.bytes)?,
        committed_memory: initial_commitments,
        initial_mutable_memory,
        max_units: limits.max_units,
        max_instructions: limits.max_instructions,
        steps: step_certificates,
        executed_units: total_units,
        retired_instructions: total_instructions,
        output: commitment("output_final", request.output_physical_start, &output)?,
    };
    Ok(TransformInvocationSequenceEvaluationV1 {
        certificate,
        output,
    })
}

fn validate_sequence_request(
    request: &TransformInvocationSequenceRequestV1<'_>,
    expected: &EvaluatedImageReceiptV1,
    limits: TransformInvocationLimitsV1,
) -> Result<(), TransformInvocationErrorV1> {
    if limits.max_units == 0 || limits.max_instructions == 0 {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "execution limits must be nonzero",
        ));
    }
    if request.steps.is_empty() || request.steps.len() != expected.streams.len() {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "sequence must contain exactly one step per evaluated stream",
        ));
    }
    if request.code.bytes.is_empty() || !request.code.bytes.len().is_multiple_of(4) {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "code image must be nonempty and word aligned",
        ));
    }
    if request.code.virtual_start & 3 != 0 {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "code address must be word aligned",
        ));
    }
    let physical = direct_physical(request.code.virtual_start).ok_or(
        TransformInvocationErrorV1::InvalidInput("code must use a direct KSEG address"),
    )?;
    if physical != request.code.physical_start {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "code virtual and physical starts disagree",
        ));
    }
    validate_range(range_of(request.code.physical_start, request.code.bytes)?)?;

    for (index, step) in request.steps.iter().enumerate() {
        if step.entry_pc & 3 != 0 {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "sequence entry address must be word aligned",
            ));
        }
        validate_additional_gpr_seeds(step.additional_gpr_seeds)?;
        let ordinal = u32::try_from(index)
            .map_err(|_| TransformInvocationErrorV1::InvalidInput("too many sequence steps"))?;
        if step.expected_output != (ExpectedEvaluatedOutputV1::Stream { ordinal }) {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "sequence steps must select every stream in ordinal order",
            ));
        }
        if step.expected_mutable_memory_after.len() != request.shared_mutable_memory.len() {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "each sequence step must declare every shared mutable post-state",
            ));
        }
        for (declared, shared) in step
            .expected_mutable_memory_after
            .iter()
            .zip(request.shared_mutable_memory)
        {
            if declared.role != shared.role
                || declared.physical_start != shared.physical_start
                || declared.bytes.len() != shared.initial_bytes.len()
            {
                return Err(TransformInvocationErrorV1::InvalidInput(
                    "shared mutable post-state geometry must match the initial declaration",
                ));
            }
        }
    }
    let mut mutable_roles = BTreeSet::new();
    for range in request.shared_mutable_memory {
        if range.role.trim().is_empty() || !mutable_roles.insert(range.role) {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "shared mutable memory roles must be nonempty and unique",
            ));
        }
    }
    Ok(())
}

fn mutable_commitments(
    mem: &Rdram<'_>,
    ranges: &[SharedMutableMemoryRangeV1<'_>],
) -> Result<Vec<ContentCommitmentV1>, TransformInvocationErrorV1> {
    let mut commitments = ranges
        .iter()
        .map(|range| {
            let len = u32::try_from(range.initial_bytes.len()).map_err(|_| {
                TransformInvocationErrorV1::InvalidInput("range length exceeds u32")
            })?;
            let bytes = mem.copy_physical_bytes(range.physical_start, len);
            commitment(range.role, range.physical_start, &bytes)
        })
        .collect::<Result<Vec<_>, _>>()?;
    sort_commitments(&mut commitments);
    Ok(commitments)
}

fn sort_commitments(commitments: &mut [ContentCommitmentV1]) {
    commitments.sort_unstable_by(|left, right| {
        (
            left.physical_start,
            left.len,
            left.role.as_str(),
            left.sha256.as_str(),
        )
            .cmp(&(
                right.physical_start,
                right.len,
                right.role.as_str(),
                right.sha256.as_str(),
            ))
    });
}

fn require_full_output_write_coverage(
    output: PhysicalRangeV1,
    events: &[TransformMemoryEventV1],
) -> Result<(), TransformInvocationErrorV1> {
    let output_end = output
        .end()
        .expect("validated output range has a non-overflowing end");
    let mut writes = events
        .iter()
        .filter_map(|event| match *event {
            TransformMemoryEventV1::Write {
                physical_offset,
                len,
            } => {
                let start = physical_offset.max(output.start);
                let end = physical_offset
                    .checked_add(len)
                    .unwrap_or(u32::MAX)
                    .min(output_end);
                (start < end).then_some((start, end))
            }
            TransformMemoryEventV1::Read { .. } => None,
        })
        .collect::<Vec<_>>();
    writes.sort_unstable();

    let mut covered_end = output.start;
    for (start, end) in writes {
        if start > covered_end {
            break;
        }
        covered_end = covered_end.max(end);
        if covered_end == output_end {
            return Ok(());
        }
    }
    Err(TransformInvocationErrorV1::OutputNotFullyWritten {
        first_unwritten_physical_offset: covered_end,
    })
}

fn validate_limits_and_layout(
    request: &TransformInvocationRequestV1<'_>,
    limits: TransformInvocationLimitsV1,
) -> Result<(), TransformInvocationErrorV1> {
    if limits.max_units == 0 || limits.max_instructions == 0 {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "execution limits must be nonzero",
        ));
    }
    if request.code.bytes.is_empty() || !request.code.bytes.len().is_multiple_of(4) {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "code image must be nonempty and word aligned",
        ));
    }
    if request.code.virtual_start & 3 != 0 || request.entry_pc & 3 != 0 {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "code and entry addresses must be word aligned",
        ));
    }
    validate_additional_gpr_seeds(request.additional_gpr_seeds)?;
    let physical = direct_physical(request.code.virtual_start).ok_or(
        TransformInvocationErrorV1::InvalidInput("code must use a direct KSEG address"),
    )?;
    if physical != request.code.physical_start {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "code virtual and physical starts disagree",
        ));
    }
    validate_range(range_of(request.code.physical_start, request.code.bytes)?)?;
    Ok(())
}

fn validate_additional_gpr_seeds(seeds: &[GprSeedV1]) -> Result<(), TransformInvocationErrorV1> {
    let mut seeded = BTreeSet::new();
    let mut previous_seed = None;
    for seed in seeds {
        if seed.register >= 32 || matches!(seed.register, 0 | 4 | 5 | 31) {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "additional GPR seed uses an invalid or dedicated register",
            ));
        }
        if !seeded.insert(seed.register) {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "additional GPR seeds contain a duplicate register",
            ));
        }
        if previous_seed.is_some_and(|previous| seed.register <= previous) {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "additional GPR seeds must be in strictly increasing register order",
            ));
        }
        previous_seed = Some(seed.register);
    }
    Ok(())
}

fn validate_range(range: PhysicalRangeV1) -> Result<(), TransformInvocationErrorV1> {
    if range.len == 0 || range.end().is_none_or(|end| end > RDRAM_LEN as u32) {
        return Err(TransformInvocationErrorV1::InvalidInput(
            "physical range is empty, overflowing, or outside RDRAM",
        ));
    }
    Ok(())
}

fn reject_overlaps(ranges: &[PhysicalRangeV1]) -> Result<(), TransformInvocationErrorV1> {
    for (index, range) in ranges.iter().enumerate() {
        validate_range(*range)?;
        if ranges[index + 1..]
            .iter()
            .any(|other| range.intersects(*other))
        {
            return Err(TransformInvocationErrorV1::InvalidInput(
                "committed memory ranges overlap",
            ));
        }
    }
    Ok(())
}

fn range_of(start: u32, bytes: &[u8]) -> Result<PhysicalRangeV1, TransformInvocationErrorV1> {
    Ok(PhysicalRangeV1 {
        start,
        len: u32::try_from(bytes.len())
            .map_err(|_| TransformInvocationErrorV1::InvalidInput("range length exceeds u32"))?,
    })
}

fn install_bytes(mem: &mut Rdram<'_>, start: u32, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().copied().enumerate() {
        mem.store_b(
            0xffff_ffff_8000_0000 | u64::from(start + offset as u32),
            byte,
        );
    }
}

fn validate_code_pc(
    code: &KnownTransformCodeImageV1<'_>,
    pc: u32,
) -> Result<(), TransformInvocationErrorV1> {
    let virtual_end = code
        .virtual_start
        .checked_add(code.bytes.len() as u32)
        .ok_or(TransformInvocationErrorV1::CodeEscape { pc })?;
    if pc < code.virtual_start || pc >= virtual_end || pc & 3 != 0 {
        return Err(TransformInvocationErrorV1::CodeEscape { pc });
    }
    let physical = direct_physical(pc).ok_or(TransformInvocationErrorV1::CodeEscape { pc })?;
    let expected = code.physical_start + (pc - code.virtual_start);
    if physical != expected {
        return Err(TransformInvocationErrorV1::CodeEscape { pc });
    }
    Ok(())
}

fn words_for_run(
    code: &KnownTransformCodeImageV1<'_>,
    instructions: &[fn64_recomp_rs::InstructionWordIdentity],
) -> Result<Vec<u32>, TransformInvocationErrorV1> {
    instructions
        .iter()
        .map(|instruction| {
            let relative = instruction
                .physical_address
                .checked_sub(code.physical_start)
                .ok_or(TransformInvocationErrorV1::CodeEscape {
                    pc: instruction.physical_address,
                })? as usize;
            let bytes = code.bytes.get(relative..relative + 4).ok_or(
                TransformInvocationErrorV1::CodeEscape {
                    pc: instruction.physical_address,
                },
            )?;
            Ok(u32::from_be_bytes(bytes.try_into().unwrap()))
        })
        .collect()
}

fn word_at_pc(
    code: &KnownTransformCodeImageV1<'_>,
    pc: u32,
) -> Result<u32, TransformInvocationErrorV1> {
    let relative = pc
        .checked_sub(code.virtual_start)
        .ok_or(TransformInvocationErrorV1::CodeEscape { pc })? as usize;
    let bytes = code
        .bytes
        .get(relative..relative + 4)
        .ok_or(TransformInvocationErrorV1::CodeEscape { pc })?;
    Ok(u32::from_be_bytes(bytes.try_into().unwrap()))
}

#[allow(clippy::too_many_arguments)]
fn audit_unit(
    pc: u32,
    words: &[u32],
    retired: u32,
    observed: &[ObservedEvent],
    defined: &mut BTreeSet<u8>,
    authority: &mut ReadAuthorityV1,
    allowed_writes: &[PhysicalRangeV1],
    code: PhysicalRangeV1,
    transcript: &mut Vec<TransformMemoryEventV1>,
    likely_delay_executes: bool,
) -> Result<(), TransformInvocationErrorV1> {
    let mut executed_words = usize::try_from(retired)
        .map_err(|_| TransformInvocationErrorV1::InstructionLimitExceeded)?;
    if words.len() == 2 && decode(words[0]).is_branch_likely() && !likely_delay_executes {
        executed_words = 1;
    }
    if executed_words == 0 || executed_words > words.len() {
        return Err(TransformInvocationErrorV1::MemoryEventMismatch { pc });
    }
    let mut events = observed.iter();
    for (index, word) in words.iter().copied().take(executed_words).enumerate() {
        audit_instruction(
            pc.wrapping_add(index as u32 * 4),
            decode(word),
            &mut events,
            defined,
            authority,
            allowed_writes,
            code,
            transcript,
        )?;
    }
    if events.next().is_some() {
        return Err(TransformInvocationErrorV1::MemoryEventMismatch { pc });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn audit_instruction<'a>(
    pc: u32,
    instruction: Instruction,
    events: &mut impl Iterator<Item = &'a ObservedEvent>,
    defined: &mut BTreeSet<u8>,
    authority: &mut ReadAuthorityV1,
    allowed_writes: &[PhysicalRangeV1],
    code: PhysicalRangeV1,
    transcript: &mut Vec<TransformMemoryEventV1>,
) -> Result<(), TransformInvocationErrorV1> {
    use Instruction::*;
    let require = |register: u8| {
        if defined.contains(&register) {
            Ok(())
        } else {
            Err(TransformInvocationErrorV1::UnseededRegisterRead { pc, register })
        }
    };
    let define = |register: u8, defined: &mut BTreeSet<u8>| {
        if register != 0 {
            defined.insert(register);
        }
    };
    match instruction {
        Nop => {}
        Lb { rt, base, .. }
        | Lbu { rt, base, .. }
        | Lh { rt, base, .. }
        | Lhu { rt, base, .. }
        | Lw { rt, base, .. }
        | Lwu { rt, base, .. }
        | Ld { rt, base, .. } => {
            require(base)?;
            consume_read(pc, events, authority, transcript)?;
            define(rt, defined);
        }
        Lwl { rt, base, .. }
        | Lwr { rt, base, .. }
        | Ldl { rt, base, .. }
        | Ldr { rt, base, .. } => {
            require(base)?;
            require(rt)?;
            consume_read(pc, events, authority, transcript)?;
            define(rt, defined);
        }
        Sb { rt, base, .. } | Sh { rt, base, .. } | Sw { rt, base, .. } | Sd { rt, base, .. } => {
            require(base)?;
            require(rt)?;
            consume_write(pc, events, authority, allowed_writes, code, transcript)?;
        }
        Swl { .. } | Swr { .. } | Sdl { .. } | Sdr { .. } => {
            return Err(TransformInvocationErrorV1::UnsupportedInstruction {
                pc,
                instruction: format!("{instruction:?}"),
            });
        }
        Addiu { rt, rs, .. }
        | Daddiu { rt, rs, .. }
        | Andi { rt, rs, .. }
        | Ori { rt, rs, .. }
        | Xori { rt, rs, .. }
        | Slti { rt, rs, .. }
        | Sltiu { rt, rs, .. } => {
            require(rs)?;
            define(rt, defined);
        }
        Lui { rt, .. } => define(rt, defined),
        Addu { rd, rs, rt }
        | Subu { rd, rs, rt }
        | And { rd, rs, rt }
        | Or { rd, rs, rt }
        | Xor { rd, rs, rt }
        | Nor { rd, rs, rt }
        | Slt { rd, rs, rt }
        | Sltu { rd, rs, rt }
        | Daddu { rd, rs, rt }
        | Dsubu { rd, rs, rt } => {
            require(rs)?;
            require(rt)?;
            define(rd, defined);
        }
        Sll { rd, rt, .. }
        | Srl { rd, rt, .. }
        | Sra { rd, rt, .. }
        | Dsll { rd, rt, .. }
        | Dsrl { rd, rt, .. }
        | Dsra { rd, rt, .. }
        | Dsll32 { rd, rt, .. }
        | Dsrl32 { rd, rt, .. }
        | Dsra32 { rd, rt, .. } => {
            require(rt)?;
            define(rd, defined);
        }
        Sllv { rd, rt, rs }
        | Srlv { rd, rt, rs }
        | Srav { rd, rt, rs }
        | Dsllv { rd, rt, rs }
        | Dsrlv { rd, rt, rs }
        | Dsrav { rd, rt, rs } => {
            require(rs)?;
            require(rt)?;
            define(rd, defined);
        }
        Beq { rs, rt, .. } | Bne { rs, rt, .. } | Beql { rs, rt, .. } | Bnel { rs, rt, .. } => {
            require(rs)?;
            require(rt)?;
        }
        Blez { rs, .. } | Bgtz { rs, .. } | Blezl { rs, .. } | Bgtzl { rs, .. } => require(rs)?,
        J { .. } => {}
        Jal { .. } => define(31, defined),
        Jr { rs } => require(rs)?,
        Jalr { rd, rs } => {
            require(rs)?;
            define(rd, defined);
        }
        _ => {
            return Err(TransformInvocationErrorV1::UnsupportedInstruction {
                pc,
                instruction: format!("{instruction:?}"),
            });
        }
    }
    Ok(())
}

fn consume_read<'a>(
    pc: u32,
    events: &mut impl Iterator<Item = &'a ObservedEvent>,
    authority: &ReadAuthorityV1,
    transcript: &mut Vec<TransformMemoryEventV1>,
) -> Result<(), TransformInvocationErrorV1> {
    let Some(ObservedEvent::Read(event)) = events.next() else {
        return Err(TransformInvocationErrorV1::MemoryEventMismatch { pc });
    };
    if !authority.allows(event.physical_offset, event.len) {
        return Err(TransformInvocationErrorV1::ReadOutsideCommitted {
            physical_offset: event.physical_offset,
            len: event.len,
        });
    }
    transcript.push(TransformMemoryEventV1::Read {
        physical_offset: event.physical_offset,
        len: event.len,
    });
    Ok(())
}

fn consume_write<'a>(
    pc: u32,
    events: &mut impl Iterator<Item = &'a ObservedEvent>,
    authority: &mut ReadAuthorityV1,
    allowed_writes: &[PhysicalRangeV1],
    code: PhysicalRangeV1,
    transcript: &mut Vec<TransformMemoryEventV1>,
) -> Result<(), TransformInvocationErrorV1> {
    let Some(ObservedEvent::Write(event)) = events.next() else {
        return Err(TransformInvocationErrorV1::MemoryEventMismatch { pc });
    };
    if event.channel() != WriterChannel::CpuInstructionStore {
        return Err(TransformInvocationErrorV1::NonCpuWrite {
            channel: event.channel(),
        });
    }
    let (physical_offset, len) = event.range();
    let written = PhysicalRangeV1 {
        start: physical_offset,
        len,
    };
    if code.intersects(written) {
        return Err(TransformInvocationErrorV1::CodeWrite {
            physical_offset,
            len,
        });
    }
    if !allowed_writes
        .iter()
        .any(|range| range.contains(physical_offset, len))
    {
        return Err(TransformInvocationErrorV1::WriteOutsideAllowed {
            physical_offset,
            len,
        });
    }
    authority.mark(written);
    transcript.push(TransformMemoryEventV1::Write {
        physical_offset,
        len,
    });
    Ok(())
}

fn classify_exit(exit: BlockExit) -> Result<(String, Option<u32>), TransformInvocationErrorV1> {
    match exit {
        BlockExit::Transfer(next) => Ok(("transfer".to_owned(), Some(next.pc.get()))),
        BlockExit::ResolveTransfer { target_pc, .. } => {
            Ok(("resolve_transfer".to_owned(), Some(target_pc.get())))
        }
        BlockExit::ResolveCall { target_pc, .. } => {
            Ok(("resolve_call".to_owned(), Some(target_pc.get())))
        }
        BlockExit::ThreadReturn => Ok(("thread_return".to_owned(), None)),
        other => Err(TransformInvocationErrorV1::RejectedExit(format!(
            "{other:?}"
        ))),
    }
}

fn direct_physical(vaddr: u32) -> Option<u32> {
    (0x8000_0000..0xc000_0000)
        .contains(&vaddr)
        .then_some(vaddr & 0x1fff_ffff)
}

fn commitment(
    role: &str,
    physical_start: u32,
    bytes: &[u8],
) -> Result<ContentCommitmentV1, TransformInvocationErrorV1> {
    Ok(ContentCommitmentV1 {
        role: role.to_owned(),
        physical_start,
        len: u32::try_from(bytes.len())
            .map_err(|_| TransformInvocationErrorV1::InvalidInput("content length exceeds u32"))?,
        sha256: sha256(bytes),
    })
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn output_poison(
    receipt_sha256: &str,
    output_start: u32,
    output_end: u32,
    physical_start: u32,
    len: usize,
) -> Vec<u8> {
    let mut seed = Sha256::new();
    seed.update(b"fn64.transform-invocation.output-poison.v1\0");
    seed.update(receipt_sha256.as_bytes());
    seed.update(output_start.to_be_bytes());
    seed.update(output_end.to_be_bytes());
    seed.update(physical_start.to_be_bytes());
    let seed: [u8; 32] = seed.finalize().into();
    seed.into_iter().cycle().take(len).collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{MaterializationEvaluatorV1, MaterializedImageSourceV1, RomAddressSpace};
    use crate::materialized_image::evaluate_materialized_image_v1;
    use crate::normalize;

    const CODE_PA: u32 = 0x1000;
    const CODE_VA: u32 = 0x8000_1000;
    const SOURCE_PA: u32 = 0x2000;
    const SOURCE_VA: u32 = 0x8000_2000;
    const OUTPUT_PA: u32 = 0x3000;
    const OUTPUT_VA: u32 = 0x8000_3000;
    const RETURN_PC: u32 = 0xffff_fffc;

    fn i(op: u32, rs: u8, rt: u8, imm: i16) -> u32 {
        (op << 26) | (u32::from(rs) << 21) | (u32::from(rt) << 16) | u32::from(imm as u16)
    }

    fn r(rs: u8, rt: u8, rd: u8, funct: u32) -> u32 {
        (u32::from(rs) << 21) | (u32::from(rt) << 16) | (u32::from(rd) << 11) | funct
    }

    fn copy_wrapper(payload_len: usize) -> Vec<u8> {
        [
            i(0x09, 0, 9, payload_len as i16), // addiu t1,zero,len
            i(0x24, 4, 8, 0),                  // lbu t0,0(a0)
            i(0x28, 5, 8, 0),                  // sb t0,0(a1)
            i(0x09, 4, 4, 1),                  // addiu a0,a0,1
            i(0x09, 5, 5, 1),                  // addiu a1,a1,1
            i(0x09, 9, 9, -1),                 // addiu t1,t1,-1
            i(0x05, 9, 0, -6),                 // bne t1,zero,loop
            0,                                 // nop
            r(31, 0, 0, 0x08),                 // jr ra
            0,                                 // nop
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect()
    }

    fn pointer_cell_copy_wrapper() -> Vec<u8> {
        [
            i(0x23, 4, 8, 0),  // lw t0,0(a0): encoded-source cursor
            i(0x23, 5, 9, 0),  // lw t1,0(a1): output cursor
            r(8, 7, 8, 0x21),  // addu t0,t0,a3: skip stream header
            i(0x24, 8, 10, 0), // lbu t2,0(t0)
            i(0x28, 9, 10, 0), // sb t2,0(t1)
            i(0x09, 8, 8, 1),  // addiu t0,t0,1
            i(0x09, 9, 9, 1),  // addiu t1,t1,1
            i(0x09, 6, 6, -1), // addiu a2,a2,-1
            i(0x05, 6, 0, -6), // bne a2,zero,copy
            0,                 // nop
            i(0x2b, 4, 8, 0),  // sw t0,0(a0)
            i(0x2b, 5, 9, 0),  // sw t1,0(a1)
            r(31, 0, 0, 0x08), // jr ra
            0,                 // nop
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect()
    }

    fn prefixed_copy_wrapper(prefix: &[u32], payload_len: usize) -> Vec<u8> {
        prefix
            .iter()
            .copied()
            .flat_map(u32::to_be_bytes)
            .chain(copy_wrapper(payload_len))
            .collect()
    }

    fn stored_stream(payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u16;
        let mut bytes = vec![0x11, 0x72];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.push(0x01); // final stored raw-DEFLATE block
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&(!len).to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn fixture() -> (NormalizedRom, EvaluatedImageReceiptV1, Vec<u8>, Vec<u8>) {
        let payload = b"exact transformed payload".to_vec();
        let source = stored_stream(&payload);
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let source_spec = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: rom_offset as u32,
            rom_end: rom_offset as u32 + source.len() as u32,
            cursor: 0,
        };
        let evaluator =
            MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 };
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &source_spec,
            &evaluator,
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        (rom, evaluation.receipt().clone(), source, payload)
    }

    fn request<'a>(code: &'a [u8], _payload: &'a [u8]) -> TransformInvocationRequestV1<'a> {
        TransformInvocationRequestV1 {
            entry_pc: CODE_VA,
            return_pc: RETURN_PC,
            a0: SOURCE_VA + 11,
            a1: OUTPUT_VA,
            additional_gpr_seeds: &[],
            expected_output: ExpectedEvaluatedOutputV1::Aggregate,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            additional_allowed_writes: &[],
        }
    }

    #[test]
    fn exact_stored_deflate_payload_copy_binds_receipt_and_transcript() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();

        assert_eq!(result.output(), payload);
        assert_eq!(
            result.certificate().evaluated_image_receipt_sha256,
            evaluated_image_receipt_sha256_v1(&receipt)
        );
        assert!(result
            .certificate()
            .memory_events
            .iter()
            .any(|event| matches!(event, TransformMemoryEventV1::Read { .. })));
        assert!(result
            .certificate()
            .memory_events
            .iter()
            .any(|event| matches!(event, TransformMemoryEventV1::Write { .. })));
        assert_eq!(
            transform_invocation_certificate_sha256_v1(result.certificate()),
            "826e55db18f6a3f8f9f6bdaee8aa4b275efea75429e58009521c379485847cad"
        );
        let repeated = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();
        assert_eq!(repeated.certificate(), result.certificate());
    }

    #[test]
    fn selected_second_stream_certifies_one_invocation_against_aggregate_receipt() {
        let first_payload = b"first stream".to_vec();
        let second_payload = b"second stream selected".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let source_spec = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: rom_offset as u32,
            rom_end: rom_offset as u32 + source.len() as u32,
            cursor: 0,
        };
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &source_spec,
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let code = copy_wrapper(second_payload.len());
        let zeros = vec![0; second_payload.len()];
        let mut request = request(&code, &zeros);
        request.a0 = SOURCE_VA + first_source.len() as u32 + 11;
        request.expected_output = ExpectedEvaluatedOutputV1::Stream { ordinal: 1 };

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();

        assert_eq!(result.output(), second_payload);
        assert_eq!(
            result.certificate().expected_output,
            ExpectedEvaluatedOutputV1::Stream { ordinal: 1 }
        );
        assert_eq!(
            (
                result.certificate().expected_output_start,
                result.certificate().expected_output_end,
            ),
            (
                first_payload.len() as u32,
                (first_payload.len() + second_payload.len()) as u32,
            )
        );
    }

    #[test]
    fn ordered_sequence_binds_shared_pointer_evolution_and_aggregate_output() {
        const SOURCE_CELL_PA: u32 = 0x4000;
        const OUTPUT_CELL_PA: u32 = 0x4004;
        const SOURCE_CELL_VA: u32 = 0x8000_4000;
        const OUTPUT_CELL_VA: u32 = 0x8000_4004;

        let first_payload = b"first ordered stream".to_vec();
        let second_payload = b"second ordered stream with a different length".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let expected_output = [first_payload.as_slice(), second_payload.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: rom_offset as u32,
                rom_end: rom_offset as u32 + source.len() as u32,
                cursor: 0,
            },
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let code = pointer_cell_copy_wrapper();
        let first_seeds = [
            GprSeedV1 {
                register: 6,
                value: first_payload.len() as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let second_seeds = [
            GprSeedV1 {
                register: 6,
                value: second_payload.len() as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let first_source_after = (SOURCE_VA + first_source.len() as u32).to_be_bytes();
        let first_output_after = (OUTPUT_VA + first_payload.len() as u32).to_be_bytes();
        let final_source_after = (SOURCE_VA + source.len() as u32).to_be_bytes();
        let final_output_after = (OUTPUT_VA + expected_output.len() as u32).to_be_bytes();
        let first_expected_mutable = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                bytes: &first_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                bytes: &first_output_after,
            },
        ];
        let second_expected_mutable = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                bytes: &final_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                bytes: &final_output_after,
            },
        ];
        let steps = [
            TransformInvocationStepRequestV1 {
                entry_pc: CODE_VA,
                return_pc: RETURN_PC,
                a0: SOURCE_CELL_VA,
                a1: OUTPUT_CELL_VA,
                additional_gpr_seeds: &first_seeds,
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
                expected_mutable_memory_after: &first_expected_mutable,
            },
            TransformInvocationStepRequestV1 {
                entry_pc: CODE_VA,
                return_pc: RETURN_PC,
                a0: SOURCE_CELL_VA,
                a1: OUTPUT_CELL_VA,
                additional_gpr_seeds: &second_seeds,
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                expected_mutable_memory_after: &second_expected_mutable,
            },
        ];
        let source_cell_initial = SOURCE_VA.to_be_bytes();
        let output_cell_initial = OUTPUT_VA.to_be_bytes();
        let mutable = [
            SharedMutableMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                initial_bytes: &source_cell_initial,
            },
            SharedMutableMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                initial_bytes: &output_cell_initial,
            },
        ];
        let request = TransformInvocationSequenceRequestV1 {
            steps: &steps,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: &code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            shared_mutable_memory: &mutable,
            additional_allowed_writes: &[],
        };

        let result = certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();

        assert_eq!(result.output(), expected_output);
        assert_eq!(result.certificate().steps.len(), 2);
        assert_eq!(
            result.certificate().executed_units,
            result
                .certificate()
                .steps
                .iter()
                .map(|step| step.units.len() as u32)
                .sum::<u32>()
        );
        assert_eq!(
            result.certificate().retired_instructions,
            result
                .certificate()
                .steps
                .iter()
                .map(|step| step.retired_instructions)
                .sum::<u32>()
        );
        let first_source_end = SOURCE_VA + first_source.len() as u32;
        let final_source_end = SOURCE_VA + source.len() as u32;
        let first_output_end = OUTPUT_VA + first_payload.len() as u32;
        let final_output_end = OUTPUT_VA + expected_output.len() as u32;
        let source_commitment = |value: u32| sha256(&value.to_be_bytes());
        let commitment_for = |commitments: &[ContentCommitmentV1], role: &str| {
            commitments
                .iter()
                .find(|commitment| commitment.role == role)
                .unwrap()
                .sha256
                .clone()
        };
        let first = &result.certificate().steps[0];
        let second = &result.certificate().steps[1];
        assert_eq!(
            commitment_for(&first.mutable_memory_before, "source_cursor"),
            source_commitment(SOURCE_VA)
        );
        assert_eq!(
            commitment_for(&first.mutable_memory_after, "source_cursor"),
            source_commitment(first_source_end)
        );
        assert_eq!(
            first.mutable_memory_after, second.mutable_memory_before,
            "the second call must consume the first call's exact mutable state"
        );
        assert_eq!(
            commitment_for(&second.mutable_memory_after, "source_cursor"),
            source_commitment(final_source_end)
        );
        assert_eq!(
            commitment_for(&first.mutable_memory_after, "output_cursor"),
            source_commitment(first_output_end)
        );
        assert_eq!(
            commitment_for(&second.mutable_memory_after, "output_cursor"),
            source_commitment(final_output_end)
        );
        assert_eq!(
            transform_invocation_sequence_certificate_sha256_v1(result.certificate()),
            "3171969d740169ce789ec740b75d6dddb60eb47ace6fc6a1fbfc711f5e6679a1"
        );
        let repeated = certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();
        assert_eq!(repeated.certificate(), result.certificate());

        let mut exact_unit_bound = TransformInvocationLimitsV1::default();
        exact_unit_bound.max_units = result.certificate().executed_units;
        assert!(certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            exact_unit_bound,
        )
        .is_ok());
        exact_unit_bound.max_units -= 1;
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &request,
                exact_unit_bound,
            ),
            Err(TransformInvocationErrorV1::UnitLimitExceeded)
        );

        let mut bounded = TransformInvocationLimitsV1::default();
        bounded.max_instructions = result.certificate().retired_instructions - 1;
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &request,
                bounded,
            ),
            Err(TransformInvocationErrorV1::InstructionLimitExceeded)
        );

        let wrong_final_output_after = (final_output_end + 1).to_be_bytes();
        let wrong_second_expected = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                bytes: &final_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                bytes: &wrong_final_output_after,
            },
        ];
        let wrong_steps = [
            steps[0].clone(),
            TransformInvocationStepRequestV1 {
                expected_mutable_memory_after: &wrong_second_expected,
                ..steps[1].clone()
            },
        ];
        let wrong_mutable = TransformInvocationSequenceRequestV1 {
            steps: &wrong_steps,
            ..request.clone()
        };
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &wrong_mutable,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::MutableMemoryMismatch {
                ordinal: 1,
                role: "output_cursor".to_owned(),
            })
        );
    }

    #[test]
    fn sequence_rejects_missing_reordered_and_cross_stream_writes() {
        let first_payload = b"first stream".to_vec();
        let second_payload = b"second stream".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: rom_offset as u32,
                rom_end: rom_offset as u32 + source.len() as u32,
                cursor: 0,
            },
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let code = pointer_cell_copy_wrapper();
        let seeds = [
            GprSeedV1 {
                register: 6,
                value: first_payload.len() as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let first_source_after = (SOURCE_VA + first_source.len() as u32).to_be_bytes();
        let first_output_after = (OUTPUT_VA + first_payload.len() as u32).to_be_bytes();
        let first_expected_mutable = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: 0x4000,
                bytes: &first_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: 0x4004,
                bytes: &first_output_after,
            },
        ];
        let first = TransformInvocationStepRequestV1 {
            entry_pc: CODE_VA,
            return_pc: RETURN_PC,
            a0: 0x8000_4000,
            a1: 0x8000_4004,
            additional_gpr_seeds: &seeds,
            expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
            expected_mutable_memory_after: &first_expected_mutable,
        };
        let source_cell = SOURCE_VA.to_be_bytes();
        let output_cell = OUTPUT_VA.to_be_bytes();
        let mutable = [
            SharedMutableMemoryRangeV1 {
                role: "source_cursor",
                physical_start: 0x4000,
                initial_bytes: &source_cell,
            },
            SharedMutableMemoryRangeV1 {
                role: "output_cursor",
                physical_start: 0x4004,
                initial_bytes: &output_cell,
            },
        ];
        let one_step = [first.clone()];
        let missing = TransformInvocationSequenceRequestV1 {
            steps: &one_step,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: &code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            shared_mutable_memory: &mutable,
            additional_allowed_writes: &[],
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &missing,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::InvalidInput(_))
        ));

        let valid_ordinals = [
            first.clone(),
            TransformInvocationStepRequestV1 {
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                ..first.clone()
            },
        ];
        let output_alias = [PhysicalRangeV1 {
            start: OUTPUT_PA,
            len: (first_payload.len() + second_payload.len()) as u32,
        }];
        let aliased_output = TransformInvocationSequenceRequestV1 {
            steps: &valid_ordinals,
            additional_allowed_writes: &output_alias,
            ..missing.clone()
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &aliased_output,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::InvalidInput(_))
        ));

        let reordered_steps = [
            TransformInvocationStepRequestV1 {
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                ..first.clone()
            },
            TransformInvocationStepRequestV1 {
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
                ..first.clone()
            },
        ];
        let reordered = TransformInvocationSequenceRequestV1 {
            steps: &reordered_steps,
            ..missing.clone()
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &reordered,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::InvalidInput(_))
        ));

        let oversized_seeds = [
            GprSeedV1 {
                register: 6,
                value: (first_payload.len() + 1) as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let crossing_steps = [
            TransformInvocationStepRequestV1 {
                additional_gpr_seeds: &oversized_seeds,
                ..first.clone()
            },
            TransformInvocationStepRequestV1 {
                additional_gpr_seeds: &seeds,
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                ..first
            },
        ];
        let crossing = TransformInvocationSequenceRequestV1 {
            steps: &crossing_steps,
            ..missing
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &crossing,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::WriteOutsideAllowed { .. })
        ));
    }

    #[test]
    fn sequence_scratch_must_be_rewritten_before_a_later_step_reads_it() {
        const SCRATCH_PA: u32 = 0x5000;
        const SCRATCH_VA: u64 = 0x8000_5000;

        let first_payload = b"first scratch stream".to_vec();
        let second_payload = b"second scratch stream".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: rom_offset as u32,
                rom_end: rom_offset as u32 + source.len() as u32,
                cursor: 0,
            },
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();

        let first_code = prefixed_copy_wrapper(
            &[i(0x28, 12, 11, 0)], // sb t3,0(t4)
            first_payload.len(),
        );
        let second_read_code = prefixed_copy_wrapper(
            &[i(0x24, 12, 11, 0)], // lbu t3,0(t4)
            second_payload.len(),
        );
        let second_rewrite_code = prefixed_copy_wrapper(
            &[
                i(0x28, 12, 11, 0), // sb t3,0(t4)
                i(0x24, 12, 11, 0), // lbu t3,0(t4)
            ],
            second_payload.len(),
        );
        let second_read_entry = CODE_VA + first_code.len() as u32;
        let second_rewrite_entry = second_read_entry + second_read_code.len() as u32;
        let code = [
            first_code.as_slice(),
            second_read_code.as_slice(),
            second_rewrite_code.as_slice(),
        ]
        .concat();
        let scratch_seeds = [
            GprSeedV1 {
                register: 11,
                value: 0xa5,
            },
            GprSeedV1 {
                register: 12,
                value: SCRATCH_VA,
            },
        ];
        let steps_for = |second_entry| {
            [
                TransformInvocationStepRequestV1 {
                    entry_pc: CODE_VA,
                    return_pc: RETURN_PC,
                    a0: SOURCE_VA + 11,
                    a1: OUTPUT_VA,
                    additional_gpr_seeds: &scratch_seeds,
                    expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
                    expected_mutable_memory_after: &[],
                },
                TransformInvocationStepRequestV1 {
                    entry_pc: second_entry,
                    return_pc: RETURN_PC,
                    a0: SOURCE_VA + first_source.len() as u32 + 11,
                    a1: OUTPUT_VA + first_payload.len() as u32,
                    additional_gpr_seeds: &scratch_seeds,
                    expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                    expected_mutable_memory_after: &[],
                },
            ]
        };
        let scratch = [PhysicalRangeV1 {
            start: SCRATCH_PA,
            len: 1,
        }];
        let make_request = |steps| TransformInvocationSequenceRequestV1 {
            steps,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: &code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            shared_mutable_memory: &[],
            additional_allowed_writes: &scratch,
        };

        let read_before_rewrite = steps_for(second_read_entry);
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &make_request(&read_before_rewrite),
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::ReadOutsideCommitted {
                physical_offset: SCRATCH_PA,
                len: 1,
            })
        );

        let rewrite_before_read = steps_for(second_rewrite_entry);
        assert!(certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &make_request(&rewrite_before_read),
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn unseeded_register_dependency_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[..4].copy_from_slice(&i(0x09, 10, 9, payload.len() as i16).to_be_bytes());
        let zeros = vec![0; payload.len()];
        let unseeded_request = request(&code, &zeros);

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &unseeded_request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::UnseededRegisterRead { register: 10, .. })
        ));

        let seeds = [GprSeedV1 {
            register: 10,
            value: 0,
        }];
        let mut seeded_request = request(&code, &zeros);
        seeded_request.additional_gpr_seeds = &seeds;
        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &seeded_request,
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn read_outside_committed_source_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.a0 = 0x8000_7000;

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::ReadOutsideCommitted { .. })
        ));
    }

    #[test]
    fn adjacent_commitments_cover_one_boundary_spanning_read() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = i(0x23, 10, 8, 0).to_be_bytes().to_vec(); // lw t0,0(t2)
        code.extend(copy_wrapper(payload.len()));
        let left = [1u8, 2];
        let right = [3u8, 4];
        let committed = [
            CommittedMemoryRangeV1 {
                role: "left_half",
                physical_start: 0x7000,
                bytes: &left,
            },
            CommittedMemoryRangeV1 {
                role: "right_half",
                physical_start: 0x7002,
                bytes: &right,
            },
        ];
        let seeds = [GprSeedV1 {
            register: 10,
            value: 0x8000_7000,
        }];
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.committed_memory = &committed;
        request.additional_gpr_seeds = &seeds;

        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn output_mismatch_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[4..8].copy_from_slice(&i(0x09, 0, 8, 0).to_be_bytes());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);

        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::OutputMismatch)
        );
    }

    #[test]
    fn matching_initial_output_without_stores_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let code = [r(31, 0, 0, 0x08), 0]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let request = request(&code, &payload);

        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::OutputNotFullyWritten {
                first_unwritten_physical_offset: OUTPUT_PA,
            })
        );
    }

    #[test]
    fn write_outside_allowed_ranges_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.a1 = 0x8000_7000;

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::WriteOutsideAllowed { .. })
        ));
    }

    #[test]
    fn code_write_is_rejected_before_allowed_range_classification() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.a1 = CODE_VA;

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::CodeWrite { .. })
        ));
    }

    #[test]
    fn partial_unaligned_store_family_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[8..12].copy_from_slice(&i(0x2a, 5, 8, 0).to_be_bytes()); // swl t0,0(a1)
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::UnsupportedInstruction { .. })
        ));
    }

    #[test]
    fn branch_likely_register_dependencies_are_audited() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[6 * 4..7 * 4].copy_from_slice(&i(0x15, 9, 0, -6).to_be_bytes()); // bnel
        let zeros = vec![0; payload.len()];
        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request(&code, &zeros),
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());

        code[6 * 4..7 * 4].copy_from_slice(&i(0x15, 10, 0, -6).to_be_bytes());
        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request(&code, &zeros),
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::UnseededRegisterRead { register: 10, .. })
        ));

        let mut annulled = [
            i(0x15, 0, 0, 1),  // bnel zero,zero,+1: not taken
            i(0x23, 10, 8, 0), // lw t0,0(t2): annulled, t2 is unseeded
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
        annulled.extend(copy_wrapper(payload.len()));
        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request(&annulled, &zeros),
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn code_escape_and_instruction_saturation_are_typed() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut escaped = request(&code, &zeros);
        escaped.entry_pc = CODE_VA + code.len() as u32;
        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &escaped,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::CodeEscape {
                pc: escaped.entry_pc
            })
        );

        let bounded = request(&code, &zeros);
        let mut limits = TransformInvocationLimitsV1::default();
        limits.max_instructions = 1;
        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &bounded,
                limits,
            ),
            Err(TransformInvocationErrorV1::InstructionLimitExceeded)
        );
    }

    fn ambient_read(_: GuestReadEvent) {}

    thread_local! {
        static AMBIENT_WRITES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    fn ambient_write(_: GuestWriteEvent) {
        AMBIENT_WRITES.with(|count| count.set(count.get() + 1));
    }

    #[test]
    fn caller_read_observer_is_isolated_and_restored() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        let previous = set_read_observer(Some(ambient_read));

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        );
        let restored = set_read_observer(previous);

        assert!(result.is_ok());
        assert!(restored.is_some());
    }

    #[test]
    fn caller_write_observer_is_isolated_from_setup_and_execution() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        AMBIENT_WRITES.with(|count| count.set(0));
        let previous = set_write_observer(Some(ambient_write));

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        );
        let restored = set_write_observer(previous);

        assert!(result.is_ok());
        assert_eq!(AMBIENT_WRITES.with(std::cell::Cell::get), 0);
        assert!(restored.is_some());
    }

    fn executable_changed(_: GuestWriteEvent) -> fn64_recomp_rs::GuestWriteBoundary {
        fn64_recomp_rs::GuestWriteBoundary::ExecutableChanged
    }

    #[test]
    fn isolated_run_preserves_caller_writer_session_and_pending_request() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        assert!(
            fn64_recomp_rs::set_guest_write_boundary_observer(Some(executable_changed)).is_none()
        );
        fn64_recomp_rs::notify_cpu_instruction_store(0x4000, 4);
        let token_before = fn64_recomp_rs::guest_write_token(0x4000, 4);

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        );

        assert!(result.is_ok());
        assert_eq!(fn64_recomp_rs::guest_write_token(0x4000, 4), token_before);
        assert!(fn64_recomp_rs::take_executable_write_boundary());
        assert!(fn64_recomp_rs::set_guest_write_boundary_observer(None).is_some());
    }
}
