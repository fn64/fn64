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
#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
use std::num::NonZeroUsize;

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
use sha2::{Digest, Sha256};

use crate::decode;
use crate::execution::{
    BankId, BlockExit, BlockRun, CpuException, CpuFault, CpuFaultKind, ExecutionKey,
    GeneratedBankRunner, GuestPc, InstructionBudget, InstructionWordIdentity,
    ProgramArtifactIdentity,
};
#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
use crate::interp::run_instruction_unit;
#[cfg(feature = "dev-interpreter")]
use crate::interp::UnsupportedOp;
use crate::runtime::{DataAccessError, Rdram, RecompContext, TlbFaultKind};

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

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
const DYNAMIC_MAPPED_UNIT_IDENTITY_DOMAIN_V1: &[u8] = b"fn64.dynamic-mapped-unit.identity.v1\0";

/// Full identity of one execution-local unit snapshotted from live RDRAM.
/// The digest, rather than its [`BankId`] projection, remains collision
/// authority.
#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DynamicMappedUnitIdentityV1([u8; 32]);

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
impl DynamicMappedUnitIdentityV1 {
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

/// Result of snapshotting and executing one straight instruction or one
/// indivisible branch/delay pair from live physical RDRAM.
#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicMappedRunV1 {
    pub entry: ExecutionKey,
    pub identity: DynamicMappedUnitIdentityV1,
    pub instructions: Vec<InstructionWordIdentity>,
    pub newly_admitted: bool,
    pub run: BlockRun,
}

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DynamicMappedErrorV1 {
    ZeroSemanticsIdentity,
    CatalogCapacityExceeded {
        capacity: usize,
    },
    Fetch {
        fault: CpuFault,
        attempted_instructions: u32,
    },
    BankCollision {
        bank: BankId,
        identity: DynamicMappedUnitIdentityV1,
        registered_identity: Option<DynamicMappedUnitIdentityV1>,
        reserved_by_static_program: bool,
    },
}

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
impl fmt::Display for DynamicMappedErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ZeroSemanticsIdentity => {
                write!(formatter, "dynamic mapped execution has zero semantic identity")
            }
            Self::CatalogCapacityExceeded { capacity } => write!(
                formatter,
                "dynamic mapped identity catalog reached its bounded capacity of {capacity} units"
            ),
            Self::Fetch {
                fault,
                attempted_instructions,
            } => write!(
                formatter,
                "{fault} after {attempted_instructions} attempted instruction fetch(es)"
            ),
            Self::BankCollision {
                bank,
                reserved_by_static_program,
                ..
            } => write!(
                formatter,
                "dynamic mapped identity collides at {bank} (reserved_static={reserved_by_static_program})"
            ),
        }
    }
}

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
impl std::error::Error for DynamicMappedErrorV1 {}

/// Execution-local catalog for exact live instruction units.
///
/// Every activation snapshots the next complete unit again. Consequently a
/// committed write cannot leave a stale later instruction resident: straight
/// instructions end the unit, while a control instruction and its already-
/// fetched delay slot are architecturally indivisible. The catalog retains
/// only full identity-to-bank bindings for A→B→A reuse; it does not mutate the
/// immutable static program or claim an executable virtual range.
#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
pub struct DynamicMappedUnitCatalogV1 {
    semantics_identity: [u8; 32],
    identities_by_bank: BTreeMap<BankId, DynamicMappedUnitIdentityV1>,
    capacity: NonZeroUsize,
}

/// Prevent a self-modifying workload from growing host memory without bound.
/// Saturation is loud so a caller cannot mistake a partial run for closure.
const DYNAMIC_MAPPED_UNIT_CATALOG_CAPACITY: usize = 131_072;

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
impl DynamicMappedUnitCatalogV1 {
    /// Construct a catalog bound to the exact mapped-execution implementation
    /// linked into this process.
    pub fn new_linked() -> Self {
        let receipt = crate::dynamic_mapped_execution_build_receipt_v1();
        assert!(
            receipt.available(),
            "dynamic mapped execution capability is not linked"
        );
        Self::new(receipt.source_sha256())
            .expect("implementation-issued dynamic semantics identity is nonzero")
    }

    pub fn new(semantics_identity: [u8; 32]) -> Result<Self, DynamicMappedErrorV1> {
        Self::new_with_capacity(
            semantics_identity,
            NonZeroUsize::new(DYNAMIC_MAPPED_UNIT_CATALOG_CAPACITY)
                .expect("dynamic catalog capacity is nonzero"),
        )
    }

    pub fn new_with_capacity(
        semantics_identity: [u8; 32],
        capacity: NonZeroUsize,
    ) -> Result<Self, DynamicMappedErrorV1> {
        if semantics_identity == [0; 32] {
            return Err(DynamicMappedErrorV1::ZeroSemanticsIdentity);
        }
        Ok(Self {
            semantics_identity,
            identities_by_bank: BTreeMap::new(),
            capacity,
        })
    }

    /// Snapshot and execute one exact live unit after the caller has reached a
    /// quiescent outer dispatch boundary. `reserved_by_static_program` must
    /// report every bank owned by the immutable static/precompiled install.
    pub fn activate_and_run(
        &mut self,
        attempted_entry: ExecutionKey,
        budget: InstructionBudget,
        ctx: &mut RecompContext,
        mem: &mut Rdram<'_>,
        mut reserved_by_static_program: impl FnMut(BankId) -> bool,
    ) -> Result<DynamicMappedRunV1, DynamicMappedErrorV1> {
        let primary = snapshot_live_instruction(
            ctx,
            mem,
            attempted_entry.bank,
            InstructionFetchSite::primary(attempted_entry.pc),
        )?;
        let mut fetched = vec![primary];
        if decode(primary.word).has_delay_slot() {
            // Fetch the complete pair before either instruction has an effect.
            fetched.push(snapshot_live_instruction(
                ctx,
                mem,
                attempted_entry.bank,
                InstructionFetchSite::delay_slot(attempted_entry.pc),
            )?);
        }

        let identity = dynamic_mapped_unit_identity(self.semantics_identity, &fetched);
        let (bank, newly_admitted) =
            self.admit_identity(identity, &mut reserved_by_static_program)?;

        let words = fetched
            .iter()
            .map(|instruction| instruction.word)
            .collect::<Vec<_>>();
        let instructions = fetched
            .iter()
            .map(|instruction| InstructionWordIdentity::new(bank, instruction.physical_address))
            .collect();
        let entry = ExecutionKey::new(bank, attempted_entry.pc);
        let run = run_instruction_unit(bank, attempted_entry.pc, &words, budget, ctx, mem)
            .unwrap_or_else(|unsupported| {
                BlockRun::new(BlockExit::Fault(unsupported.into_cpu_fault()), 0)
            });
        Ok(DynamicMappedRunV1 {
            entry,
            identity,
            instructions,
            newly_admitted,
            run,
        })
    }

    fn admit_identity(
        &mut self,
        identity: DynamicMappedUnitIdentityV1,
        reserved_by_static_program: &mut impl FnMut(BankId) -> bool,
    ) -> Result<(BankId, bool), DynamicMappedErrorV1> {
        let bank = BankId::new(u64::from_be_bytes(identity.0[..8].try_into().unwrap()));
        let registered_identity = self.identities_by_bank.get(&bank).copied();
        let reserved = reserved_by_static_program(bank);
        if reserved || registered_identity.is_some_and(|known| known != identity) {
            return Err(DynamicMappedErrorV1::BankCollision {
                bank,
                identity,
                registered_identity,
                reserved_by_static_program: reserved,
            });
        }
        if registered_identity.is_none() && self.identities_by_bank.len() >= self.capacity.get() {
            return Err(DynamicMappedErrorV1::CatalogCapacityExceeded {
                capacity: self.capacity.get(),
            });
        }
        let newly_admitted = registered_identity.is_none();
        self.identities_by_bank.insert(bank, identity);
        Ok((bank, newly_admitted))
    }

    pub fn identity_for_bank(&self, bank: BankId) -> Option<DynamicMappedUnitIdentityV1> {
        self.identities_by_bank.get(&bank).copied()
    }

    pub fn admitted_len(&self) -> usize {
        self.identities_by_bank.len()
    }

    pub const fn capacity(&self) -> usize {
        self.capacity.get()
    }
}

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
#[derive(Clone, Copy)]
struct LiveFetchedInstruction {
    physical_address: u32,
    word: u32,
}

fn translate_instruction_site(
    ctx: &RecompContext,
    bank: BankId,
    site: InstructionFetchSite,
) -> Result<(GuestPc, u32), CpuFault> {
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
                bad_vaddr: Some(u64::from(pc.get())),
                coprocessor: None,
            },
        });
    }
    let translated = ctx
        .translate_instruction_address(u64::from(pc.get()))
        .map_err(|error| CpuFault {
            at,
            kind: match error {
                DataAccessError::AddressError { vaddr, .. } => CpuFaultKind::Exception {
                    exception: CpuException::AddressErrorLoad,
                    epc: site.epc(),
                    branch_delay: site.branch_delay(),
                    instruction_code: 0,
                    bad_vaddr: Some(vaddr),
                    coprocessor: None,
                },
                DataAccessError::Tlb(fault) => CpuFaultKind::Exception {
                    exception: match (fault.kind, fault.extended) {
                        (TlbFaultKind::Refill, true) => CpuException::XTlbRefillLoad,
                        (TlbFaultKind::Refill, false) => CpuException::TlbRefillLoad,
                        (TlbFaultKind::Invalid, _) => CpuException::TlbInvalidLoad,
                        (TlbFaultKind::Modified, _) => {
                            unreachable!("instruction fetch cannot raise TLB Modified")
                        }
                    },
                    epc: site.epc(),
                    branch_delay: site.branch_delay(),
                    instruction_code: 0,
                    bad_vaddr: Some(fault.vaddr),
                    coprocessor: None,
                },
                DataAccessError::Unbacked { .. } => {
                    unreachable!("instruction translation does not inspect host backing")
                }
            },
        })?;
    Ok((pc, translated.get()))
}

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
fn snapshot_live_instruction(
    ctx: &RecompContext,
    mem: &Rdram<'_>,
    fault_bank: BankId,
    site: InstructionFetchSite,
) -> Result<LiveFetchedInstruction, DynamicMappedErrorV1> {
    let (virtual_pc, physical_address) = translate_instruction_site(ctx, fault_bank, site)
        .map_err(|fault| DynamicMappedErrorV1::Fetch {
            fault,
            attempted_instructions: attempted_fetches(fault, site.branch_delay()),
        })?;
    let mut bytes = [0u8; 4];
    for (offset, byte) in bytes.iter_mut().enumerate() {
        let physical =
            physical_address
                .checked_add(offset as u32)
                .ok_or(DynamicMappedErrorV1::Fetch {
                    fault: CpuFault {
                        at: ExecutionKey::new(fault_bank, virtual_pc),
                        kind: CpuFaultKind::UnmappedPhysicalInstruction { physical_address },
                    },
                    attempted_instructions: 0,
                })?;
        *byte = mem
            .try_load_physical_bu(physical)
            .ok_or(DynamicMappedErrorV1::Fetch {
                fault: CpuFault {
                    at: ExecutionKey::new(fault_bank, virtual_pc),
                    kind: CpuFaultKind::UnmappedPhysicalInstruction { physical_address },
                },
                attempted_instructions: 0,
            })?;
    }
    Ok(LiveFetchedInstruction {
        physical_address,
        word: u32::from_be_bytes(bytes),
    })
}

#[cfg(any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime"))]
fn dynamic_mapped_unit_identity(
    semantics_identity: [u8; 32],
    fetched: &[LiveFetchedInstruction],
) -> DynamicMappedUnitIdentityV1 {
    let mut hasher = Sha256::new();
    hasher.update(DYNAMIC_MAPPED_UNIT_IDENTITY_DOMAIN_V1);
    hasher.update(semantics_identity);
    hasher.update((fetched.len() as u64).to_be_bytes());
    for instruction in fetched {
        hasher.update(instruction.physical_address.to_be_bytes());
        hasher.update(instruction.word.to_be_bytes());
    }
    DynamicMappedUnitIdentityV1(hasher.finalize().into())
}

/// Translate and fetch through one immutable physical-code catalog.
pub fn fetch_instruction(
    catalog: &PhysicalCodeCatalog,
    ctx: &RecompContext,
    bank: BankId,
    site: InstructionFetchSite,
) -> Result<FetchedInstruction, CpuFault> {
    let (pc, physical_address) = translate_instruction_site(ctx, bank, site)?;
    let at = ExecutionKey::new(bank, pc);
    let identity = InstructionWordIdentity::new(bank, physical_address);
    let word = catalog.word(identity).ok_or(CpuFault {
        at,
        kind: CpuFaultKind::UnmappedPhysicalInstruction { physical_address },
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
#[cfg(feature = "dev-interpreter")]
pub(crate) struct AdmittedMappedUnit {
    bank: BankId,
    entry: GuestPc,
    words: Vec<u32>,
}

#[cfg(feature = "dev-interpreter")]
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

#[cfg(feature = "dev-interpreter")]
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
#[cfg(feature = "dev-interpreter")]
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

#[cfg(all(
    test,
    any(feature = "dev-interpreter", feature = "dynamic-mapped-runtime")
))]
mod dynamic_mapped_tests {
    use super::*;
    use crate::TlbEntryRaw;

    const ATTEMPT_BANK: BankId = BankId::new(0x1234);
    const SEMANTICS: [u8; 32] = [0x5a; 32];

    fn put_word(bytes: &mut [u8], physical: u32, word: u32) {
        for (offset, byte) in word.to_be_bytes().into_iter().enumerate() {
            bytes[(physical as usize + offset) ^ 3] = byte;
        }
    }

    fn entry_lo(physical_page: u32, valid: bool) -> u32 {
        ((physical_page >> 6) & 0x03ff_ffc0) | 1 | ((valid as u32) << 1) | (1 << 2)
    }

    fn map_pair(
        ctx: &mut RecompContext,
        index: usize,
        virtual_pair: u32,
        even_pa: u32,
        odd_pa: u32,
    ) {
        ctx.tlb_entries[index] = TlbEntryRaw {
            page_mask: 0,
            entry_hi: u64::from(virtual_pair & 0xffff_e000),
            entry_lo0: entry_lo(even_pa, true),
            entry_lo1: entry_lo(odd_pa, true),
        };
    }

    #[test]
    fn live_fetch_reuses_exact_a_after_a_b_a() {
        const PC: GuestPc = GuestPc::new(0x8000_0040);
        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x2442_0001); // addiu $v0,$v0,1
        let mut mem = Rdram::new(&mut bytes);
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let budget = InstructionBudget::new(2).unwrap();

        let mut ctx = RecompContext::new();
        let a = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, PC),
                budget,
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert!(a.newly_admitted);
        assert_eq!(ctx.r(2), 1);

        ctx.set_r(2, 0);
        let same_a = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, PC),
                budget,
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(same_a.identity, a.identity);
        assert_eq!(same_a.entry.bank, a.entry.bank);
        assert!(!same_a.newly_admitted);

        mem.store_w(0xffff_ffff_8000_0040, 0x2442_0007);
        ctx.set_r(2, 0);
        let b = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, PC),
                budget,
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_ne!(b.identity, a.identity);
        assert_ne!(b.entry.bank, a.entry.bank);
        assert_eq!(ctx.r(2), 7);

        mem.store_w(0xffff_ffff_8000_0040, 0x2442_0001);
        ctx.set_r(2, 0);
        let restored = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, PC),
                budget,
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(restored.identity, a.identity);
        assert_eq!(restored.entry.bank, a.entry.bank);
        assert!(!restored.newly_admitted);
        assert_eq!(ctx.r(2), 1);
    }

    #[test]
    fn aliases_share_physical_identity_but_keep_virtual_entries() {
        let mut bytes = vec![0; 0x101000];
        put_word(&mut bytes, 0x0010_0000, 0x2442_0001);
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        map_pair(&mut ctx, 0, 0x0040_0000, 0x0010_0000, 0x0010_1000);
        map_pair(&mut ctx, 1, 0x0080_0000, 0x0010_0000, 0x0010_1000);
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let budget = InstructionBudget::new(2).unwrap();

        let first = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x0040_0000)),
                budget,
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        let second = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x0080_0000)),
                budget,
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(first.identity, second.identity);
        assert_eq!(first.entry.bank, second.entry.bank);
        assert_ne!(first.entry.pc, second.entry.pc);
        assert_eq!(first.instructions, second.instructions);
    }

    #[test]
    fn cross_page_delay_slot_is_snapshotted_before_execution() {
        const BRANCH_PC: GuestPc = GuestPc::new(0x0040_0ffc);
        let mut bytes = vec![0; 0x301000];
        put_word(&mut bytes, 0x0010_0ffc, 0x1000_0001); // beq zero,zero,+1
        put_word(&mut bytes, 0x0030_0000, 0x2442_0005); // addiu v0,v0,5
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        map_pair(&mut ctx, 0, 0x0040_0000, 0x0010_0000, 0x0030_0000);
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let run = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, BRANCH_PC),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(run.instructions[0].physical_address, 0x0010_0ffc);
        assert_eq!(run.instructions[1].physical_address, 0x0030_0000);
        assert_eq!(ctx.r(2), 5);

        let mut invalid_ctx = RecompContext::new();
        map_pair(&mut invalid_ctx, 0, 0x0040_0000, 0x0010_0000, 0x0030_0000);
        invalid_ctx.tlb_entries[0].entry_lo1 &= !(1 << 1);
        let mut invalid_catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let error = invalid_catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, BRANCH_PC),
                InstructionBudget::new(2).unwrap(),
                &mut invalid_ctx,
                &mut mem,
                |_| false,
            )
            .unwrap_err();
        let DynamicMappedErrorV1::Fetch {
            fault,
            attempted_instructions,
        } = error
        else {
            panic!("expected delay-slot fetch fault")
        };
        assert_eq!(attempted_instructions, 2);
        let CpuFaultKind::Exception {
            epc,
            branch_delay,
            bad_vaddr,
            ..
        } = fault.kind
        else {
            panic!("expected architectural delay-slot fault")
        };
        assert_eq!(epc, BRANCH_PC);
        assert!(branch_delay);
        assert_eq!(bad_vaddr, Some(0x0040_1000));
        assert_eq!(invalid_ctx.r(2), 0);
        assert_eq!(invalid_catalog.admitted_len(), 0);
    }

    fn target_code_boundary(
        event: crate::runtime::GuestWriteEvent,
    ) -> crate::runtime::GuestWriteBoundary {
        let (start, len) = event.range();
        if start < 0x50 && start.saturating_add(len) > 0x4c {
            crate::runtime::GuestWriteBoundary::ExecutableChanged
        } else {
            crate::runtime::GuestWriteBoundary::Continue
        }
    }

    #[test]
    fn delay_store_returns_before_target_and_next_activation_reads_new_word() {
        const BRANCH_PC: GuestPc = GuestPc::new(0x8000_0040);
        const TARGET_PC: GuestPc = GuestPc::new(0x8000_004c);
        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x1000_0002); // beq zero,zero,+2
        put_word(&mut bytes, 0x44, 0xac88_0000); // sw t0,0(a0)
        put_word(&mut bytes, 0x4c, 0x2442_0001); // stale addiu v0,v0,1
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        ctx.set_r(4, 0xffff_ffff_8000_004c);
        ctx.set_r(8, 0x2442_0007); // replacement addiu v0,v0,7
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let previous =
            crate::runtime::set_guest_write_boundary_observer(Some(target_code_boundary));

        let writer = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, BRANCH_PC),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(writer.run.instructions, 2);
        assert_eq!(
            writer.run.exit,
            BlockExit::ExecutableWrite {
                source_bank: writer.entry.bank,
                resume: ExecutionKey::new(writer.entry.bank, TARGET_PC),
            }
        );
        assert_eq!(
            ctx.r(2),
            0,
            "stale target did not execute in the writer unit"
        );

        let target = catalog
            .activate_and_run(
                ExecutionKey::new(writer.entry.bank, TARGET_PC),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_ne!(target.identity, writer.identity);
        assert_eq!(ctx.r(2), 7);
        crate::runtime::set_guest_write_boundary_observer(previous);
    }

    #[test]
    fn preadmission_faults_and_static_bank_collisions_are_loud() {
        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x2442_0001);
        let mut mem = Rdram::new(&mut bytes);
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let mut ctx = RecompContext::new();
        let misaligned = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0042)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap_err();
        assert!(matches!(
            misaligned,
            DynamicMappedErrorV1::Fetch {
                fault: CpuFault {
                    kind: CpuFaultKind::Exception {
                        exception: CpuException::AddressErrorLoad,
                        ..
                    },
                    ..
                },
                attempted_instructions: 1,
            }
        ));
        assert_eq!(catalog.admitted_len(), 0);

        let unbacked = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0100)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap_err();
        assert!(matches!(
            unbacked,
            DynamicMappedErrorV1::Fetch {
                fault: CpuFault {
                    kind: CpuFaultKind::UnmappedPhysicalInstruction {
                        physical_address: 0x100
                    },
                    ..
                },
                attempted_instructions: 0,
            }
        ));
        assert_eq!(catalog.admitted_len(), 0);

        let first = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        let mut colliding_catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let collision = colliding_catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |bank| bank == first.entry.bank,
            )
            .unwrap_err();
        assert!(matches!(
            collision,
            DynamicMappedErrorV1::BankCollision {
                bank,
                reserved_by_static_program: true,
                ..
            } if bank == first.entry.bank
        ));
        assert_eq!(colliding_catalog.admitted_len(), 0);
    }

    #[test]
    fn dynamic_identity_catalog_capacity_is_loud_and_does_not_grow() {
        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x2442_0001);
        put_word(&mut bytes, 0x44, 0x2463_0001);
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let mut catalog =
            DynamicMappedUnitCatalogV1::new_with_capacity(SEMANTICS, NonZeroUsize::new(1).unwrap())
                .unwrap();

        catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        let error = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0044)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap_err();

        assert_eq!(
            error,
            DynamicMappedErrorV1::CatalogCapacityExceeded { capacity: 1 }
        );
        assert_eq!(catalog.admitted_len(), 1);
        assert_eq!(catalog.capacity(), 1);
    }

    #[test]
    fn exact_dynamic_jal_retains_call_and_resume_semantics() {
        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x0c00_0020); // jal 0x80000080
        put_word(&mut bytes, 0x44, 0); // delay nop
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();

        let run = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();

        assert_eq!(ctx.r_u32(31), 0x8000_0048);
        assert_eq!(run.run.instructions, 2);
        assert_eq!(
            run.run.exit,
            BlockExit::ResolveCall {
                source_bank: run.entry.bank,
                target_pc: GuestPc::new(0x8000_0080),
                resume: ExecutionKey::new(run.entry.bank, GuestPc::new(0x8000_0048)),
            }
        );
    }

    #[test]
    fn exact_dynamic_in_unit_jal_and_jalr_still_require_call_resolution() {
        const PC: GuestPc = GuestPc::new(0x8000_0040);
        const RESUME: GuestPc = GuestPc::new(0x8000_0048);
        for (word, source_register) in [
            (0x0c00_0010, None),      // jal 0x80000040
            (0x0100_f809, Some(8u8)), // jalr ra,t0
        ] {
            let mut bytes = vec![0; 0x100];
            put_word(&mut bytes, 0x40, word);
            put_word(&mut bytes, 0x44, 0); // delay nop
            let mut mem = Rdram::new(&mut bytes);
            let mut ctx = RecompContext::new();
            if source_register.is_some() {
                ctx.set_r32(8, PC.get() as i32);
            }
            let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();

            let run = catalog
                .activate_and_run(
                    ExecutionKey::new(ATTEMPT_BANK, PC),
                    InstructionBudget::new(2).unwrap(),
                    &mut ctx,
                    &mut mem,
                    |_| false,
                )
                .unwrap();

            assert_eq!(ctx.r_u32(31), RESUME.get());
            assert_eq!(run.run.instructions, 2);
            assert_eq!(
                run.run.exit,
                BlockExit::ResolveCall {
                    source_bank: run.entry.bank,
                    target_pc: PC,
                    resume: ExecutionKey::new(run.entry.bank, RESUME),
                }
            );
            assert_eq!(
                ctx.indirect_transfer_observations().len(),
                usize::from(source_register.is_some())
            );
        }
    }

    #[test]
    fn exact_dynamic_jalr_retains_call_and_jr_retains_thread_return() {
        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x0100_f809); // jalr ra,t0
        put_word(&mut bytes, 0x44, 0); // delay nop
        put_word(&mut bytes, 0x50, 0x03e0_0008); // jr ra
        put_word(&mut bytes, 0x54, 0); // delay nop
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        ctx.set_r32(8, 0x8000_0080u32 as i32);
        ctx.hi = 0x0123_4567_89ab_cdef;
        ctx.lo = 0xfedc_ba98_7654_3210;
        ctx.cop0_status = 0x1000_0001;
        ctx.cop0_cause = 0x2000_00b2;
        ctx.cop0_epc = 0x3000_00c4;
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();

        let call = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(ctx.r_u32(31), 0x8000_0048);
        assert_eq!(
            call.run.exit,
            BlockExit::ResolveCall {
                source_bank: call.entry.bank,
                target_pc: GuestPc::new(0x8000_0080),
                resume: ExecutionKey::new(call.entry.bank, GuestPc::new(0x8000_0048)),
            }
        );
        let mut call_gprs = [0; 32];
        call_gprs[8] = 0xffff_ffff_8000_0080;
        call_gprs[31] = 0xffff_ffff_8000_0048;
        assert_eq!(
            ctx.indirect_transfer_observations(),
            &[crate::runtime::IndirectTransferObservation {
                source_bank: call.entry.bank.get(),
                source_pc: 0x8000_0040,
                source_register: 8,
                target_pc: 0x8000_0080,
                link_pc: Some(0x8000_0048),
                gprs: call_gprs,
                hi: 0x0123_4567_89ab_cdef,
                lo: 0xfedc_ba98_7654_3210,
                cop0_status: 0x1000_0001,
                cop0_cause: 0x2000_00b2,
                cop0_epc: 0x3000_00c4,
            }]
        );

        const SENTINEL: u32 = 0xffff_fffc;
        ctx.set_r32(31, SENTINEL as i32);
        ctx.set_thread_return_pc(Some(SENTINEL));
        let returned = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0050)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();
        assert_eq!(returned.run.exit, BlockExit::ThreadReturn);
        assert_eq!(returned.run.instructions, 2);
        let mut return_gprs = call_gprs;
        return_gprs[31] = 0xffff_ffff_ffff_fffc;
        assert_eq!(
            ctx.indirect_transfer_observations(),
            &[
                crate::runtime::IndirectTransferObservation {
                    source_bank: call.entry.bank.get(),
                    source_pc: 0x8000_0040,
                    source_register: 8,
                    target_pc: 0x8000_0080,
                    link_pc: Some(0x8000_0048),
                    gprs: call_gprs,
                    hi: 0x0123_4567_89ab_cdef,
                    lo: 0xfedc_ba98_7654_3210,
                    cop0_status: 0x1000_0001,
                    cop0_cause: 0x2000_00b2,
                    cop0_epc: 0x3000_00c4,
                },
                crate::runtime::IndirectTransferObservation {
                    source_bank: returned.entry.bank.get(),
                    source_pc: 0x8000_0050,
                    source_register: 31,
                    target_pc: SENTINEL,
                    link_pc: None,
                    gprs: return_gprs,
                    hi: 0x0123_4567_89ab_cdef,
                    lo: 0xfedc_ba98_7654_3210,
                    cop0_status: 0x1000_0001,
                    cop0_cause: 0x2000_00b2,
                    cop0_epc: 0x3000_00c4,
                },
            ]
        );
    }

    #[test]
    fn equal_bank_projection_with_different_full_digest_is_rejected() {
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let mut first_bytes = [0x11; 32];
        let mut second_bytes = first_bytes;
        first_bytes[31] = 0x22;
        second_bytes[31] = 0x33;
        let first = DynamicMappedUnitIdentityV1(first_bytes);
        let second = DynamicMappedUnitIdentityV1(second_bytes);
        let (bank, newly_admitted) = catalog.admit_identity(first, &mut |_| false).unwrap();
        assert!(newly_admitted);
        let error = catalog.admit_identity(second, &mut |_| false).unwrap_err();
        assert!(matches!(
            error,
            DynamicMappedErrorV1::BankCollision {
                bank: collided,
                identity,
                registered_identity: Some(registered),
                reserved_by_static_program: false,
            } if collided == bank && identity == second && registered == first
        ));
        assert_eq!(catalog.admitted_len(), 1);
        assert_eq!(catalog.identity_for_bank(bank), Some(first));
    }

    #[test]
    fn exact_dynamic_unit_uses_the_canonical_rdram_mmio_hooks() {
        fn read_mmio(vaddr: u64) -> Option<u32> {
            (vaddr == 0xffff_ffff_a460_0010).then_some(0x1122_3344)
        }

        let mut bytes = vec![0; 0x100];
        put_word(&mut bytes, 0x40, 0x8c82_0000); // lw v0,0(a0)
        let mut mem = Rdram::new(&mut bytes);
        let mut ctx = RecompContext::new();
        ctx.set_r(4, 0xffff_ffff_a460_0010);
        let mut catalog = DynamicMappedUnitCatalogV1::new(SEMANTICS).unwrap();
        let previous = crate::runtime::set_mmio_hooks(Some(read_mmio), None);

        let run = catalog
            .activate_and_run(
                ExecutionKey::new(ATTEMPT_BANK, GuestPc::new(0x8000_0040)),
                InstructionBudget::new(2).unwrap(),
                &mut ctx,
                &mut mem,
                |_| false,
            )
            .unwrap();

        crate::runtime::set_mmio_hooks(previous.0, previous.1);
        assert_eq!(run.run.instructions, 1);
        assert_eq!(ctx.r_u32(2), 0x1122_3344);
    }
}
