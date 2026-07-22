//! Canonical 32-bit instruction fetch with separate virtual control-flow and
//! physical admitted-word identities.
//!
//! The VR4300 applies branch, jump/link, EPC, and Cause.BD rules to virtual
//! PCs. Executable admission, however, names the physical word selected by the
//! current TLB/direct-segment translation and its immutable [`BankId`]
//! generation. Keeping those values in distinct types prevents a mapped alias
//! from changing architectural control flow and prevents a virtual lookup from
//! executing stale bytes after a remap.

use std::collections::BTreeMap;
use std::fmt;

use crate::decode;
use crate::execution::{
    BankId, BlockExit, BlockRun, CpuException, CpuFault, CpuFaultKind, ExecutionKey,
    GeneratedBankRunner, GuestPc, InstructionBudget, InstructionWordIdentity,
    ProgramArtifactIdentity,
};
use crate::interp::{run_instruction_unit, UnsupportedOp};
use crate::runtime::{Rdram, RecompContext, TlbFaultKind};

/// One immutable contiguous physical code span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalCodeSpan {
    bank: BankId,
    start: u32,
    words: Vec<u32>,
}

impl PhysicalCodeSpan {
    pub fn new(bank: BankId, start: u32, words: Vec<u32>) -> Result<Self, PhysicalCodeError> {
        if start & 3 != 0 {
            return Err(PhysicalCodeError::UnalignedStart { bank, start });
        }
        if words.is_empty() {
            return Err(PhysicalCodeError::Empty { bank });
        }
        let len = u32::try_from(words.len())
            .ok()
            .and_then(|words| words.checked_mul(4))
            .ok_or(PhysicalCodeError::AddressOverflow { bank, start })?;
        start
            .checked_add(len)
            .ok_or(PhysicalCodeError::AddressOverflow { bank, start })?;
        Ok(Self { bank, start, words })
    }

    pub const fn bank(&self) -> BankId {
        self.bank
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub fn end(&self) -> u32 {
        self.start + self.words.len() as u32 * 4
    }

    fn resolve(&self, physical_address: u32) -> Option<u32> {
        let offset = physical_address.checked_sub(self.start)?;
        if offset & 3 != 0 {
            return None;
        }
        self.words.get((offset / 4) as usize).copied()
    }
}

/// One immutable physical executable image/generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalCodeBank {
    id: BankId,
    spans: Vec<PhysicalCodeSpan>,
}

impl PhysicalCodeBank {
    pub fn new(id: BankId, start: u32, words: Vec<u32>) -> Result<Self, PhysicalCodeError> {
        Self::from_spans(id, vec![PhysicalCodeSpan::new(id, start, words)?])
    }

    pub fn from_spans(
        id: BankId,
        mut spans: Vec<PhysicalCodeSpan>,
    ) -> Result<Self, PhysicalCodeError> {
        if spans.is_empty() {
            return Err(PhysicalCodeError::Empty { bank: id });
        }
        for span in &spans {
            if span.bank != id {
                return Err(PhysicalCodeError::SpanBankMismatch {
                    bank: id,
                    span_bank: span.bank,
                    start: span.start,
                });
            }
        }
        spans.sort_by_key(PhysicalCodeSpan::start);
        for pair in spans.windows(2) {
            if pair[1].start < pair[0].end() {
                return Err(PhysicalCodeError::OverlappingSpans {
                    bank: id,
                    left_end: pair[0].end(),
                    right_start: pair[1].start,
                });
            }
        }
        Ok(Self { id, spans })
    }

    pub const fn id(&self) -> BankId {
        self.id
    }

    fn resolve(&self, physical_address: u32) -> Option<u32> {
        let candidate = self
            .spans
            .partition_point(|span| span.start <= physical_address)
            .checked_sub(1)?;
        self.spans[candidate].resolve(physical_address)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhysicalCodeError {
    Empty {
        bank: BankId,
    },
    UnalignedStart {
        bank: BankId,
        start: u32,
    },
    AddressOverflow {
        bank: BankId,
        start: u32,
    },
    SpanBankMismatch {
        bank: BankId,
        span_bank: BankId,
        start: u32,
    },
    OverlappingSpans {
        bank: BankId,
        left_end: u32,
        right_start: u32,
    },
    DuplicateId {
        bank: BankId,
    },
}

impl fmt::Display for PhysicalCodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Empty { bank } => write!(f, "{bank} has no physical executable words"),
            Self::UnalignedStart { bank, start } => {
                write!(f, "{bank} has unaligned physical code start {start:#010X}")
            }
            Self::AddressOverflow { bank, start } => write!(
                f,
                "{bank} physical code starting at {start:#010X} exceeds the address space"
            ),
            Self::SpanBankMismatch {
                bank,
                span_bank,
                start,
            } => write!(
                f,
                "{bank} cannot own physical span from {span_bank} at {start:#010X}"
            ),
            Self::OverlappingSpans {
                bank,
                left_end,
                right_start,
            } => write!(
                f,
                "{bank} has overlapping physical spans at {left_end:#010X} and {right_start:#010X}"
            ),
            Self::DuplicateId { bank } => {
                write!(f, "physical code generation {bank} is registered")
            }
        }
    }
}

impl std::error::Error for PhysicalCodeError {}

/// Registry indexed only by immutable generation and physical word address.
#[derive(Clone, Debug, Default)]
pub struct PhysicalCodeCatalog {
    banks: BTreeMap<BankId, PhysicalCodeBank>,
}

/// Pointer-independent image of one admitted physical code span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalCodeSpanEvidenceSnapshot {
    pub physical_start: u32,
    pub words: Vec<u32>,
}

/// Pointer-independent physical code generation retained by a block program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhysicalCodeBankEvidenceSnapshot {
    pub id: BankId,
    pub spans: Vec<PhysicalCodeSpanEvidenceSnapshot>,
}

/// One translated AOT unit and the exact fetch words/artifact identities it binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappedAotEvidenceSnapshot {
    pub entry: ExecutionKey,
    pub instructions: Vec<InstructionWordIdentity>,
    pub expected_words: Vec<u32>,
    pub runner_artifact_identity: ProgramArtifactIdentity,
}

impl PhysicalCodeCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, bank: PhysicalCodeBank) -> Result<(), PhysicalCodeError> {
        let id = bank.id;
        if self.banks.contains_key(&id) {
            return Err(PhysicalCodeError::DuplicateId { bank: id });
        }
        self.banks.insert(id, bank);
        Ok(())
    }

    pub fn word(&self, identity: InstructionWordIdentity) -> Option<u32> {
        self.banks
            .get(&identity.bank)?
            .resolve(identity.physical_address)
    }

    pub fn contains_bank(&self, bank: BankId) -> bool {
        self.banks.contains_key(&bank)
    }

    pub fn bank(&self, bank: BankId) -> Option<&PhysicalCodeBank> {
        self.banks.get(&bank)
    }

    pub(crate) fn unregister(&mut self, bank: BankId) -> Option<PhysicalCodeBank> {
        self.banks.remove(&bank)
    }

    pub fn is_empty(&self) -> bool {
        self.banks.is_empty()
    }

    pub fn evidence_snapshot(&self) -> Vec<PhysicalCodeBankEvidenceSnapshot> {
        self.banks
            .values()
            .map(|bank| PhysicalCodeBankEvidenceSnapshot {
                id: bank.id,
                spans: bank
                    .spans
                    .iter()
                    .map(|span| PhysicalCodeSpanEvidenceSnapshot {
                        physical_start: span.start,
                        words: span.words.clone(),
                    })
                    .collect(),
            })
            .collect()
    }
}

/// Precise architectural location of an instruction fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstructionFetchSite {
    Primary { pc: GuestPc },
    DelaySlot { branch_pc: GuestPc, pc: GuestPc },
}

impl InstructionFetchSite {
    pub const fn primary(pc: GuestPc) -> Self {
        Self::Primary { pc }
    }

    pub const fn delay_slot(branch_pc: GuestPc) -> Self {
        Self::DelaySlot {
            branch_pc,
            pc: GuestPc::new(branch_pc.get().wrapping_add(4)),
        }
    }

    pub const fn pc(self) -> GuestPc {
        match self {
            Self::Primary { pc } | Self::DelaySlot { pc, .. } => pc,
        }
    }

    pub const fn epc(self) -> GuestPc {
        match self {
            Self::Primary { pc } => pc,
            Self::DelaySlot { branch_pc, .. } => branch_pc,
        }
    }

    pub const fn branch_delay(self) -> bool {
        matches!(self, Self::DelaySlot { .. })
    }
}

/// One word fetched by PA while retaining its architectural VA separately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FetchedInstruction {
    pub virtual_pc: GuestPc,
    pub identity: InstructionWordIdentity,
    pub word: u32,
}

/// Translate and fetch through one immutable physical-code catalog.
pub fn fetch_instruction(
    catalog: &PhysicalCodeCatalog,
    ctx: &RecompContext,
    bank: BankId,
    site: InstructionFetchSite,
) -> Result<FetchedInstruction, CpuFault> {
    let pc = site.pc();
    let at = ExecutionKey::new(bank, pc);
    if !pc.is_instruction_aligned() {
        return Err(CpuFault {
            at,
            kind: CpuFaultKind::Exception {
                exception: CpuException::AddressErrorLoad,
                epc: site.epc(),
                branch_delay: site.branch_delay(),
                instruction_code: 0,
                bad_vaddr: Some(pc.get()),
                coprocessor: None,
            },
        });
    }
    let translated = ctx
        .translate_instruction_address(u64::from(pc.get()))
        .map_err(|fault| CpuFault {
            at,
            kind: CpuFaultKind::Exception {
                exception: match fault.kind {
                    TlbFaultKind::Refill => CpuException::TlbRefillLoad,
                    TlbFaultKind::Invalid => CpuException::TlbInvalidLoad,
                    TlbFaultKind::Modified => {
                        unreachable!("instruction fetch cannot raise TLB Modified")
                    }
                },
                epc: site.epc(),
                branch_delay: site.branch_delay(),
                instruction_code: 0,
                bad_vaddr: Some(fault.vaddr),
                coprocessor: None,
            },
        })?;
    let identity = InstructionWordIdentity::new(bank, translated.get());
    let word = catalog.word(identity).ok_or(CpuFault {
        at,
        kind: CpuFaultKind::UnmappedPhysicalInstruction {
            physical_address: translated.get(),
        },
    })?;
    Ok(FetchedInstruction {
        virtual_pc: pc,
        identity,
        word,
    })
}

fn attempted_fetches(fault: CpuFault, delay_slot: bool) -> u32 {
    if matches!(fault.kind, CpuFaultKind::Exception { .. }) {
        if delay_slot {
            2
        } else {
            1
        }
    } else {
        0
    }
}

/// One complete straight or branch/delay unit admitted through mapped fetch.
///
/// Construction is private to [`admit_mapped_unit`], so owning dispatchers may
/// treat possession of this value as proof that every instruction the unit can
/// execute was fetched successfully before recording an entered destination.
pub(crate) struct AdmittedMappedUnit {
    bank: BankId,
    entry: GuestPc,
    words: Vec<u32>,
}

pub(crate) fn admit_mapped_unit(
    catalog: &PhysicalCodeCatalog,
    bank: BankId,
    entry: GuestPc,
    ctx: &RecompContext,
) -> Result<AdmittedMappedUnit, BlockRun> {
    let primary = fetch_instruction(catalog, ctx, bank, InstructionFetchSite::primary(entry))
        .map_err(|fault| BlockRun::new(BlockExit::Fault(fault), attempted_fetches(fault, false)))?;
    let mut words = vec![primary.word];
    if decode(primary.word).has_delay_slot() {
        let delay = fetch_instruction(catalog, ctx, bank, InstructionFetchSite::delay_slot(entry))
            .map_err(|fault| {
                BlockRun::new(BlockExit::Fault(fault), attempted_fetches(fault, true))
            })?;
        words.push(delay.word);
    }
    Ok(AdmittedMappedUnit { bank, entry, words })
}

pub(crate) fn run_admitted_mapped_unit(
    unit: AdmittedMappedUnit,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> Result<BlockRun, UnsupportedOp> {
    run_instruction_unit(unit.bank, unit.entry, &unit.words, budget, ctx, mem)
}

/// Execute one dynamically fetched unit in the interpreter lane.
///
/// A unit is one straight instruction or one branch plus its delay slot. The
/// slot VA is translated independently before execution, so adjacent virtual
/// words may come from unrelated physical pages. The temporary virtual view is
/// execution-local; admitted identity remains the physical catalog above.
pub fn run_mapped_bank(
    catalog: &PhysicalCodeCatalog,
    bank: BankId,
    entry: GuestPc,
    budget: InstructionBudget,
    ctx: &mut RecompContext,
    mem: &mut Rdram<'_>,
) -> Result<BlockRun, UnsupportedOp> {
    let unit = match admit_mapped_unit(catalog, bank, entry, ctx) {
        Ok(unit) => unit,
        Err(run) => return Ok(run),
    };
    run_admitted_mapped_unit(unit, budget, ctx, mem)
}

/// One AOT unit bound to exact VA-to-physical identities at translation time.
///
/// The unit shape makes fetch validation structural: one straight word ends at
/// a resolver boundary; a control word includes exactly its independently
/// translated slot. No generated runner can execute a later unvalidated fetch.
pub struct MappedAotBlock {
    bank: BankId,
    entry: GuestPc,
    expected: Vec<FetchedInstruction>,
    runner: GeneratedBankRunner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappedAotError {
    RunnerBankMismatch {
        code_bank: BankId,
        runner_bank: BankId,
    },
    InvalidUnit,
    Fetch(CpuFault),
}

impl MappedAotBlock {
    pub fn new(
        catalog: &PhysicalCodeCatalog,
        ctx: &RecompContext,
        bank: BankId,
        entry: GuestPc,
        compiled_words: &[u32],
        runner: GeneratedBankRunner,
    ) -> Result<Self, MappedAotError> {
        if runner.bank() != bank {
            return Err(MappedAotError::RunnerBankMismatch {
                code_bank: bank,
                runner_bank: runner.bank(),
            });
        }
        let valid_shape = match compiled_words {
            [word] => !decode(*word).has_delay_slot(),
            [word, delay] => decode(*word).has_delay_slot() && !decode(*delay).has_delay_slot(),
            _ => false,
        };
        if !valid_shape {
            return Err(MappedAotError::InvalidUnit);
        }
        let primary = fetch_instruction(catalog, ctx, bank, InstructionFetchSite::primary(entry))
            .map_err(MappedAotError::Fetch)?;
        if primary.word != compiled_words[0] {
            return Err(MappedAotError::InvalidUnit);
        }
        let mut expected = vec![primary];
        if compiled_words.len() == 2 {
            let delay =
                fetch_instruction(catalog, ctx, bank, InstructionFetchSite::delay_slot(entry))
                    .map_err(MappedAotError::Fetch)?;
            if delay.word != compiled_words[1] {
                return Err(MappedAotError::InvalidUnit);
            }
            expected.push(delay);
        }
        Ok(Self {
            bank,
            entry,
            expected,
            runner,
        })
    }

    pub fn identities(&self) -> Vec<InstructionWordIdentity> {
        self.expected.iter().map(|word| word.identity).collect()
    }

    pub const fn bank(&self) -> BankId {
        self.bank
    }

    pub const fn entry(&self) -> GuestPc {
        self.entry
    }

    pub const fn runner_artifact_identity(&self) -> Option<crate::ProgramArtifactIdentity> {
        self.runner.artifact_identity()
    }

    pub fn evidence_snapshot(&self) -> MappedAotEvidenceSnapshot {
        MappedAotEvidenceSnapshot {
            entry: ExecutionKey::new(self.bank, self.entry),
            instructions: self.identities(),
            expected_words: self.expected.iter().map(|word| word.word).collect(),
            runner_artifact_identity: self.runner.artifact_identity().unwrap_or_else(|| {
                panic!(
                    "mapped AOT release evidence requires a stable artifact identity at {}",
                    ExecutionKey::new(self.bank, self.entry)
                )
            }),
        }
    }

    pub fn run(
        &self,
        catalog: &PhysicalCodeCatalog,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        if let Err(run) = self.preflight(catalog, ctx) {
            return run;
        }
        self.run_preflighted(budget, ctx, mem)
    }

    pub(crate) fn preflight(
        &self,
        catalog: &PhysicalCodeCatalog,
        ctx: &RecompContext,
    ) -> Result<(), BlockRun> {
        for (index, expected) in self.expected.iter().copied().enumerate() {
            let site = if index == 0 {
                InstructionFetchSite::primary(self.entry)
            } else {
                InstructionFetchSite::delay_slot(self.entry)
            };
            let actual = match fetch_instruction(catalog, ctx, self.bank, site) {
                Ok(actual) => actual,
                Err(fault) => {
                    return Err(BlockRun::new(
                        BlockExit::Fault(fault),
                        attempted_fetches(fault, index != 0),
                    ))
                }
            };
            if actual.identity != expected.identity || actual.word != expected.word {
                return Err(BlockRun::new(
                    BlockExit::Fault(CpuFault {
                        at: ExecutionKey::new(self.bank, actual.virtual_pc),
                        kind: CpuFaultKind::StaleInstructionIdentity {
                            expected: expected.identity,
                            actual: actual.identity,
                        },
                    }),
                    0,
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn run_preflighted(
        &self,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
    ) -> BlockRun {
        (self.runner.callable())(ExecutionKey::new(self.bank, self.entry), budget, ctx, mem)
    }
}
