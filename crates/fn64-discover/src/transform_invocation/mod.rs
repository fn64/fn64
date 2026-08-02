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
mod tests;
