//! Content-free shape census for the unresolved-indirect frontier.
//!
//! This module is diagnostic only. It classifies the transfer register and
//! its nearest writer in the same basic block; it does not claim a reaching
//! definition across predecessors and cannot promote discovery authority.

use crate::cfg::{BasicBlock, BlockTerminator};
use crate::owner_proof::{OwnerAssessment, OwnerBlocker, OwnerProofReport};
use crate::resolve::{written_gpr, ClosureResult, IndirectProofState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterFamilyV1 {
    Zero,
    AssemblerTemporary,
    ReturnValue,
    Argument,
    Temporary,
    Saved,
    Kernel,
    GlobalPointer,
    StackPointer,
    FramePointer,
    ReturnAddress,
}

impl RegisterFamilyV1 {
    fn from_register(register: u8) -> Self {
        match register {
            0 => Self::Zero,
            1 => Self::AssemblerTemporary,
            2..=3 => Self::ReturnValue,
            4..=7 => Self::Argument,
            8..=15 | 24..=25 => Self::Temporary,
            16..=23 => Self::Saved,
            26..=27 => Self::Kernel,
            28 => Self::GlobalPointer,
            29 => Self::StackPointer,
            30 => Self::FramePointer,
            31 => Self::ReturnAddress,
            _ => unreachable!("a MIPS GPR index is five bits"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseDefinitionShapeV1 {
    LiveIn,
    Immediate,
    RegisterCopy,
    Add,
    Shift,
    Logical,
    OtherArithmetic,
    Load,
    CoprocessorMove,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LocalDefinitionShapeV1 {
    /// No writer exists between this basic block's start and the transfer.
    LiveIn,
    Load {
        base: RegisterFamilyV1,
        base_definition: BaseDefinitionShapeV1,
    },
    RegisterCopy {
        source: RegisterFamilyV1,
    },
    Immediate,
    Arithmetic,
    CoprocessorMove,
    AtomicResult,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySourceCardinalityV1 {
    None,
    One,
    Multiple,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIndirectShapeCountV1 {
    pub via_call: bool,
    pub transfer_register: RegisterFamilyV1,
    pub local_definition: LocalDefinitionShapeV1,
    pub memory_sources: MemorySourceCardinalityV1,
    pub sites: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticDefinitionShapeV1 {
    LiveIn,
    Load {
        base_definition: BaseDefinitionShapeV1,
    },
    RegisterCopy,
    Immediate,
    Arithmetic,
    CoprocessorMove,
    AtomicResult,
    Other,
}

impl From<LocalDefinitionShapeV1> for SemanticDefinitionShapeV1 {
    fn from(value: LocalDefinitionShapeV1) -> Self {
        match value {
            LocalDefinitionShapeV1::LiveIn => Self::LiveIn,
            LocalDefinitionShapeV1::Load {
                base_definition, ..
            } => Self::Load { base_definition },
            LocalDefinitionShapeV1::RegisterCopy { .. } => Self::RegisterCopy,
            LocalDefinitionShapeV1::Immediate => Self::Immediate,
            LocalDefinitionShapeV1::Arithmetic => Self::Arithmetic,
            LocalDefinitionShapeV1::CoprocessorMove => Self::CoprocessorMove,
            LocalDefinitionShapeV1::AtomicResult => Self::AtomicResult,
            LocalDefinitionShapeV1::Other => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIndirectSemanticShapeCountV1 {
    pub via_call: bool,
    pub local_definition: SemanticDefinitionShapeV1,
    pub sites: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenIndirectMechanismV1 {
    AllOpenSites,
    LoadSites,
    LoadSitesWithRetainedMemorySources,
    LoadSitesWithImmediateBase,
    LoadSitesWithAddBase,
    LiveInSites,
    OtherLocalSites,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIndirectMechanismCounterfactualV1 {
    pub mechanism: OpenIndirectMechanismV1,
    pub sites: u64,
    /// Assessments promoted if, and only if, every unresolved-indirect
    /// blocker they carry is an open site in this mechanism family.
    pub sole_owner_assessments_if_discharged: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenIndirectFrontierV1 {
    pub open_sites: u64,
    /// Register-independent workload distribution. Rank these rows instead of
    /// the allocator-sensitive `shapes` rows when comparing mechanisms.
    pub semantic_shapes: Vec<OpenIndirectSemanticShapeCountV1>,
    pub shapes: Vec<OpenIndirectShapeCountV1>,
    pub mechanism_counterfactuals: Vec<OpenIndirectMechanismCounterfactualV1>,
}

impl OpenIndirectFrontierV1 {
    pub fn merge(&mut self, other: &Self) {
        let mut counts = self.shape_counts();
        for row in &other.shapes {
            *counts
                .entry((
                    row.via_call,
                    row.transfer_register,
                    row.local_definition,
                    row.memory_sources,
                ))
                .or_default() += row.sites;
        }
        self.open_sites += other.open_sites;
        self.shapes = rows_from_counts(counts);
        self.semantic_shapes = semantic_rows_from_shapes(&self.shapes);
        let mut counterfactuals: BTreeMap<_, (u64, u64)> = self
            .mechanism_counterfactuals
            .iter()
            .map(|row| {
                (
                    row.mechanism,
                    (row.sites, row.sole_owner_assessments_if_discharged),
                )
            })
            .collect();
        for row in &other.mechanism_counterfactuals {
            let aggregate = counterfactuals.entry(row.mechanism).or_default();
            aggregate.0 += row.sites;
            aggregate.1 += row.sole_owner_assessments_if_discharged;
        }
        self.mechanism_counterfactuals = counterfactuals
            .into_iter()
            .map(
                |(mechanism, (sites, sole_owner_assessments_if_discharged))| {
                    OpenIndirectMechanismCounterfactualV1 {
                        mechanism,
                        sites,
                        sole_owner_assessments_if_discharged,
                    }
                },
            )
            .collect();
    }

    fn shape_counts(
        &self,
    ) -> BTreeMap<
        (
            bool,
            RegisterFamilyV1,
            LocalDefinitionShapeV1,
            MemorySourceCardinalityV1,
        ),
        u64,
    > {
        let mut counts = BTreeMap::new();
        for row in &self.shapes {
            *counts
                .entry((
                    row.via_call,
                    row.transfer_register,
                    row.local_definition,
                    row.memory_sources,
                ))
                .or_default() += row.sites;
        }
        counts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenIndirectFrontierError {
    DuplicateIndirectBlock,
    OpenSiteMissingIndirectBlock,
    TransferKindMismatch,
    TransferWordUnavailable,
    TransferOpcodeMismatch,
    BlockWordUnavailable,
    OwnerReportBankMismatch,
}

impl std::fmt::Display for OpenIndirectFrontierError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::DuplicateIndirectBlock => "two indirect blocks have the same transfer site",
            Self::OpenSiteMissingIndirectBlock => {
                "an open resolution has no matching indirect CFG block"
            }
            Self::TransferKindMismatch => {
                "an open resolution disagrees with its CFG block's transfer kind"
            }
            Self::TransferWordUnavailable => "an indirect transfer word is outside the bank",
            Self::TransferOpcodeMismatch => "an indirect CFG block does not end in jr/jalr",
            Self::BlockWordUnavailable => "an indirect block word is outside the bank",
            Self::OwnerReportBankMismatch => {
                "the owner-proof report does not belong to the classified CFG bank"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for OpenIndirectFrontierError {}

/// Classify final `Open` indirects by content-free local instruction shape.
///
/// The nearest writer is intentionally limited to the site's own basic block.
/// `LiveIn` therefore includes values defined in predecessors as well as true
/// entry-state values. This census is an opportunity-ranking instrument, not
/// an inter-block data-flow result.
pub fn classify_open_indirects_v1(
    closure: &ClosureResult,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<OpenIndirectFrontierV1, OpenIndirectFrontierError> {
    let site_shapes = classify_site_shapes(closure, bank_bytes, va_start)?;
    Ok(report_from_site_shapes(&site_shapes, None))
}

/// Add owner-promotion counterfactuals to the same content-free shape census.
/// An assessment counts only when it has no non-indirect blockers and every
/// unresolved-indirect blocker it carries is an open site in the named family.
pub fn classify_open_indirects_with_owners_v1(
    closure: &ClosureResult,
    owner_proof: &OwnerProofReport,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<OpenIndirectFrontierV1, OpenIndirectFrontierError> {
    if owner_proof.bank != closure.cfg.bank {
        return Err(OpenIndirectFrontierError::OwnerReportBankMismatch);
    }
    let site_shapes = classify_site_shapes(closure, bank_bytes, va_start)?;
    Ok(report_from_site_shapes(&site_shapes, Some(owner_proof)))
}

type SiteShapeV1 = (
    bool,
    RegisterFamilyV1,
    LocalDefinitionShapeV1,
    MemorySourceCardinalityV1,
);

fn classify_site_shapes(
    closure: &ClosureResult,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<BTreeMap<u32, SiteShapeV1>, OpenIndirectFrontierError> {
    let mut indirect_blocks = BTreeMap::new();
    for block in &closure.cfg.blocks {
        let BlockTerminator::Indirect { via_call } = block.terminator else {
            continue;
        };
        let Some(site_pc) = block.end_va.checked_sub(8) else {
            return Err(OpenIndirectFrontierError::TransferWordUnavailable);
        };
        if indirect_blocks.insert(site_pc, (block, via_call)).is_some() {
            return Err(OpenIndirectFrontierError::DuplicateIndirectBlock);
        }
    }

    let mut site_shapes = BTreeMap::new();
    for resolution in &closure.indirect {
        if resolution.state != IndirectProofState::Open {
            continue;
        }
        let Some((block, via_call)) = indirect_blocks.get(&resolution.site_pc).copied() else {
            return Err(OpenIndirectFrontierError::OpenSiteMissingIndirectBlock);
        };
        if via_call != resolution.via_call {
            return Err(OpenIndirectFrontierError::TransferKindMismatch);
        }
        let transfer_word = read_word(bank_bytes, va_start, resolution.site_pc)
            .ok_or(OpenIndirectFrontierError::TransferWordUnavailable)?;
        let transfer_register = transfer_register(transfer_word, via_call)
            .ok_or(OpenIndirectFrontierError::TransferOpcodeMismatch)?;
        let local_definition = nearest_local_definition(
            block,
            resolution.site_pc,
            transfer_register,
            bank_bytes,
            va_start,
        )?;
        if site_shapes
            .insert(
                resolution.site_pc,
                (
                    via_call,
                    RegisterFamilyV1::from_register(transfer_register),
                    local_definition,
                    match resolution.memory_sources.len() {
                        0 => MemorySourceCardinalityV1::None,
                        1 => MemorySourceCardinalityV1::One,
                        _ => MemorySourceCardinalityV1::Multiple,
                    },
                ),
            )
            .is_some()
        {
            return Err(OpenIndirectFrontierError::DuplicateIndirectBlock);
        }
    }
    Ok(site_shapes)
}

fn report_from_site_shapes(
    site_shapes: &BTreeMap<u32, SiteShapeV1>,
    owner_proof: Option<&OwnerProofReport>,
) -> OpenIndirectFrontierV1 {
    let mut counts = BTreeMap::new();
    for shape in site_shapes.values() {
        *counts.entry(*shape).or_default() += 1;
    }
    let mechanism_counterfactuals = OpenIndirectMechanismV1::ALL
        .into_iter()
        .map(|mechanism| {
            let sites = site_shapes
                .values()
                .filter(|shape| mechanism.matches(shape))
                .count() as u64;
            let sole_owner_assessments_if_discharged = owner_proof.map_or(0, |report| {
                report
                    .assessments
                    .iter()
                    .filter(|assessment| assessment_promotes(assessment, site_shapes, mechanism))
                    .count() as u64
            });
            OpenIndirectMechanismCounterfactualV1 {
                mechanism,
                sites,
                sole_owner_assessments_if_discharged,
            }
        })
        .collect();
    OpenIndirectFrontierV1 {
        open_sites: site_shapes.len() as u64,
        semantic_shapes: semantic_rows_from_shapes(&rows_from_counts(counts.clone())),
        shapes: rows_from_counts(counts),
        mechanism_counterfactuals,
    }
}

impl OpenIndirectMechanismV1 {
    const ALL: [Self; 7] = [
        Self::AllOpenSites,
        Self::LoadSites,
        Self::LoadSitesWithRetainedMemorySources,
        Self::LoadSitesWithImmediateBase,
        Self::LoadSitesWithAddBase,
        Self::LiveInSites,
        Self::OtherLocalSites,
    ];

    fn matches(self, shape: &SiteShapeV1) -> bool {
        let (_, _, local, memory_sources) = *shape;
        match self {
            Self::AllOpenSites => true,
            Self::LoadSites => matches!(local, LocalDefinitionShapeV1::Load { .. }),
            Self::LoadSitesWithRetainedMemorySources => {
                matches!(local, LocalDefinitionShapeV1::Load { .. })
                    && memory_sources != MemorySourceCardinalityV1::None
            }
            Self::LoadSitesWithImmediateBase => matches!(
                local,
                LocalDefinitionShapeV1::Load {
                    base_definition: BaseDefinitionShapeV1::Immediate,
                    ..
                }
            ),
            Self::LoadSitesWithAddBase => matches!(
                local,
                LocalDefinitionShapeV1::Load {
                    base_definition: BaseDefinitionShapeV1::Add,
                    ..
                }
            ),
            Self::LiveInSites => matches!(local, LocalDefinitionShapeV1::LiveIn),
            Self::OtherLocalSites => !matches!(
                local,
                LocalDefinitionShapeV1::LiveIn | LocalDefinitionShapeV1::Load { .. }
            ),
        }
    }
}

fn assessment_promotes(
    assessment: &OwnerAssessment,
    site_shapes: &BTreeMap<u32, SiteShapeV1>,
    mechanism: OpenIndirectMechanismV1,
) -> bool {
    let blockers = match assessment {
        OwnerAssessment::Proven { .. } => return false,
        OwnerAssessment::Candidate { frontier } | OwnerAssessment::Ambiguous { frontier } => {
            &frontier.blockers
        }
    };
    let mut unresolved_sites = Vec::new();
    for blocker in blockers {
        match blocker {
            OwnerBlocker::UnresolvedIndirect { site, .. } => unresolved_sites.push(*site),
            _ => return false,
        }
    }
    !unresolved_sites.is_empty()
        && unresolved_sites.iter().all(|site| {
            site_shapes
                .get(site)
                .is_some_and(|shape| mechanism.matches(shape))
        })
}

fn rows_from_counts(
    counts: BTreeMap<
        (
            bool,
            RegisterFamilyV1,
            LocalDefinitionShapeV1,
            MemorySourceCardinalityV1,
        ),
        u64,
    >,
) -> Vec<OpenIndirectShapeCountV1> {
    counts
        .into_iter()
        .map(
            |((via_call, transfer_register, local_definition, memory_sources), sites)| {
                OpenIndirectShapeCountV1 {
                    via_call,
                    transfer_register,
                    local_definition,
                    memory_sources,
                    sites,
                }
            },
        )
        .collect()
}

fn semantic_rows_from_shapes(
    shapes: &[OpenIndirectShapeCountV1],
) -> Vec<OpenIndirectSemanticShapeCountV1> {
    let mut counts = BTreeMap::new();
    for shape in shapes {
        *counts
            .entry((
                shape.via_call,
                SemanticDefinitionShapeV1::from(shape.local_definition),
            ))
            .or_default() += shape.sites;
    }
    counts
        .into_iter()
        .map(
            |((via_call, local_definition), sites)| OpenIndirectSemanticShapeCountV1 {
                via_call,
                local_definition,
                sites,
            },
        )
        .collect()
}

fn read_word(bank_bytes: &[u8], va_start: u32, pc: u32) -> Option<u32> {
    let offset = pc.checked_sub(va_start)? as usize;
    let bytes = bank_bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn transfer_register(word: u32, via_call: bool) -> Option<u8> {
    if word >> 26 != 0 {
        return None;
    }
    let funct = word & 0x3f;
    let rd = ((word >> 11) & 0x1f) as u8;
    let instruction_via_call = match funct {
        0x08 => false,
        0x09 => rd != 0,
        _ => return None,
    };
    (instruction_via_call == via_call).then_some(((word >> 21) & 0x1f) as u8)
}

fn nearest_local_definition(
    block: &BasicBlock,
    site_pc: u32,
    transfer_register: u8,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<LocalDefinitionShapeV1, OpenIndirectFrontierError> {
    let mut nearest = None;
    let mut pc = block.start_va;
    while pc < site_pc {
        let word = read_word(bank_bytes, va_start, pc)
            .ok_or(OpenIndirectFrontierError::BlockWordUnavailable)?;
        if written_gpr(word) == Some(transfer_register) {
            nearest = Some((pc, word));
        }
        pc = pc
            .checked_add(4)
            .ok_or(OpenIndirectFrontierError::BlockWordUnavailable)?;
    }
    nearest.map_or(Ok(LocalDefinitionShapeV1::LiveIn), |(pc, word)| {
        classify_writer(block, pc, word, bank_bytes, va_start)
    })
}

fn classify_writer(
    block: &BasicBlock,
    writer_pc: u32,
    word: u32,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<LocalDefinitionShapeV1, OpenIndirectFrontierError> {
    let opcode = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let shamt = ((word >> 6) & 0x1f) as u8;
    let funct = word & 0x3f;

    if matches!(opcode, 0x1a | 0x1b | 0x20..=0x27 | 0x30 | 0x34 | 0x37) {
        return Ok(LocalDefinitionShapeV1::Load {
            base: RegisterFamilyV1::from_register(rs),
            base_definition: nearest_base_definition(block, writer_pc, rs, bank_bytes, va_start)?,
        });
    }
    if opcode == 0 {
        let copy_source = match funct {
            0x20 | 0x21 | 0x25 | 0x2c | 0x2d if rt == 0 => Some(rs),
            0x20 | 0x21 | 0x25 | 0x2c | 0x2d if rs == 0 => Some(rt),
            0x00 | 0x38 if shamt == 0 => Some(rt),
            _ => None,
        };
        return Ok(
            copy_source.map_or(LocalDefinitionShapeV1::Arithmetic, |source| {
                LocalDefinitionShapeV1::RegisterCopy {
                    source: RegisterFamilyV1::from_register(source),
                }
            }),
        );
    }
    if matches!(opcode, 0x08..=0x0f | 0x18 | 0x19) {
        return Ok(LocalDefinitionShapeV1::Immediate);
    }
    if matches!(opcode, 0x10..=0x13) && matches!(rs, 0x00..=0x02) {
        return Ok(LocalDefinitionShapeV1::CoprocessorMove);
    }
    if matches!(opcode, 0x38 | 0x3c) {
        return Ok(LocalDefinitionShapeV1::AtomicResult);
    }
    Ok(LocalDefinitionShapeV1::Other)
}

fn nearest_base_definition(
    block: &BasicBlock,
    before_pc: u32,
    base_register: u8,
    bank_bytes: &[u8],
    va_start: u32,
) -> Result<BaseDefinitionShapeV1, OpenIndirectFrontierError> {
    let mut nearest = None;
    let mut pc = block.start_va;
    while pc < before_pc {
        let word = read_word(bank_bytes, va_start, pc)
            .ok_or(OpenIndirectFrontierError::BlockWordUnavailable)?;
        if written_gpr(word) == Some(base_register) {
            nearest = Some(word);
        }
        pc = pc
            .checked_add(4)
            .ok_or(OpenIndirectFrontierError::BlockWordUnavailable)?;
    }
    Ok(nearest.map_or(BaseDefinitionShapeV1::LiveIn, classify_base_writer))
}

fn classify_base_writer(word: u32) -> BaseDefinitionShapeV1 {
    let opcode = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let shamt = ((word >> 6) & 0x1f) as u8;
    let funct = word & 0x3f;
    if matches!(opcode, 0x1a | 0x1b | 0x20..=0x27 | 0x30 | 0x34 | 0x37) {
        BaseDefinitionShapeV1::Load
    } else if opcode == 0
        && (matches!(funct, 0x20 | 0x21 | 0x25 | 0x2c | 0x2d) && (rs == 0 || rt == 0)
            || matches!(funct, 0x00 | 0x38) && shamt == 0)
    {
        BaseDefinitionShapeV1::RegisterCopy
    } else if matches!(opcode, 0x08..=0x0f | 0x18 | 0x19) {
        BaseDefinitionShapeV1::Immediate
    } else if opcode == 0 && matches!(funct, 0x20 | 0x21 | 0x2c | 0x2d) {
        BaseDefinitionShapeV1::Add
    } else if opcode == 0 && matches!(funct, 0x00 | 0x02 | 0x03 | 0x04 | 0x06 | 0x07 | 0x38..=0x3f)
    {
        BaseDefinitionShapeV1::Shift
    } else if opcode == 0 && matches!(funct, 0x24..=0x27 | 0x2a | 0x2b | 0x2e | 0x2f) {
        BaseDefinitionShapeV1::Logical
    } else if opcode == 0 {
        BaseDefinitionShapeV1::OtherArithmetic
    } else if matches!(opcode, 0x10..=0x13) && matches!(rs, 0x00..=0x02) {
        BaseDefinitionShapeV1::CoprocessorMove
    } else {
        BaseDefinitionShapeV1::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::{Cfg, IndirectSite};
    use crate::facts::BankAddr;
    use crate::owner_proof::{IndirectScope, OwnerFrontier};
    use crate::resolve::{IndirectResolution, IndirectResolutionKind};
    use std::collections::BTreeMap;

    fn closure(words: &[u32], via_call: bool) -> (ClosureResult, Vec<u8>) {
        let bytes = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let site_pc = 0x8000_0000 + (words.len() as u32 - 2) * 4;
        let block = BasicBlock {
            start_va: 0x8000_0000,
            end_va: 0x8000_0000 + words.len() as u32 * 4,
            terminator: BlockTerminator::Indirect { via_call },
        };
        let cfg = Cfg {
            bank: "test".into(),
            word_class: BTreeMap::new(),
            blocks: vec![block],
            direct_calls: vec![],
            tail_transfers: vec![],
            indirect_sites: vec![IndirectSite {
                pc: site_pc,
                via_call,
            }],
            plain_delay_entry_aliases: vec![],
            unsupported_delay_entries: vec![],
            proven_roots: vec![],
        };
        let indirect = vec![IndirectResolution {
            site_pc,
            via_call,
            state: IndirectProofState::Open,
            kind: None::<IndirectResolutionKind>,
            targets: vec![],
            memory_sources: vec![],
        }];
        (ClosureResult { cfg, indirect }, bytes)
    }

    #[test]
    fn classifies_stack_load_feeding_computed_jump() {
        let (closure, bytes) = closure(&[0x8fb9_0010, 0x0320_0008, 0], false);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(report.open_sites, 1);
        assert_eq!(
            report.shapes[0],
            OpenIndirectShapeCountV1 {
                via_call: false,
                transfer_register: RegisterFamilyV1::Temporary,
                local_definition: LocalDefinitionShapeV1::Load {
                    base: RegisterFamilyV1::StackPointer,
                    base_definition: BaseDefinitionShapeV1::LiveIn,
                },
                memory_sources: MemorySourceCardinalityV1::None,
                sites: 1,
            }
        );
    }

    #[test]
    fn classifies_live_in_computed_call() {
        let (closure, bytes) = closure(&[0x0320_f809, 0], true);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(
            report.shapes[0].local_definition,
            LocalDefinitionShapeV1::LiveIn
        );
        assert!(report.shapes[0].via_call);
        assert_eq!(
            report.shapes[0].memory_sources,
            MemorySourceCardinalityV1::None
        );
    }

    #[test]
    fn classifies_link_discarding_jalr_as_computed_jump() {
        let jalr_zero_t9 = (25u32 << 21) | 0x09;
        let (closure, bytes) = closure(&[jalr_zero_t9, 0], false);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(report.open_sites, 1);
        assert!(!report.shapes[0].via_call);
        assert_eq!(
            report.shapes[0].transfer_register,
            RegisterFamilyV1::Temporary
        );
        assert_eq!(
            report.shapes[0].local_definition,
            LocalDefinitionShapeV1::LiveIn
        );
    }

    #[test]
    fn classifies_indirect_after_plain_delay_entry_alias() {
        let base = 0x8000_0000;
        let branch_to_delay_word = 0x1000_0000;
        let jalr_zero_t9 = (25u32 << 21) | 0x09;
        let words = [branch_to_delay_word, 0, jalr_zero_t9, 0];
        let bytes: Vec<u8> = words.iter().flat_map(|word| word.to_be_bytes()).collect();
        let closure = crate::resolve::build_cfg_value_set_closed("test", &bytes, base, &[base]);
        assert_eq!(closure.cfg.plain_delay_entry_aliases.len(), 1);
        assert_eq!(closure.indirect[0].site_pc, base + 8);

        let report = classify_open_indirects_v1(&closure, &bytes, base).unwrap();
        assert_eq!(report.open_sites, 1);
        assert!(!report.shapes[0].via_call);
    }

    #[test]
    fn classifies_argument_register_copy() {
        let (closure, bytes) = closure(&[0x0080_c825, 0x0320_0008, 0], false);
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(
            report.shapes[0].local_definition,
            LocalDefinitionShapeV1::RegisterCopy {
                source: RegisterFamilyV1::Argument,
            }
        );
    }

    #[test]
    fn semantic_shapes_do_not_fragment_on_register_allocation() {
        let (first_closure, first_bytes) = closure(&[0x8fb9_0010, 0x0320_0008, 0], false);
        let (second_closure, second_bytes) = closure(&[0x8f98_0010, 0x0300_0008, 0], false);
        let mut report =
            classify_open_indirects_v1(&first_closure, &first_bytes, 0x8000_0000).unwrap();
        report.merge(
            &classify_open_indirects_v1(&second_closure, &second_bytes, 0x8000_0000).unwrap(),
        );

        assert_eq!(report.shapes.len(), 2);
        assert_eq!(
            report.semantic_shapes,
            vec![OpenIndirectSemanticShapeCountV1 {
                via_call: false,
                local_definition: SemanticDefinitionShapeV1::Load {
                    base_definition: BaseDefinitionShapeV1::LiveIn,
                },
                sites: 2,
            }]
        );
    }

    #[test]
    fn classifies_immediate_load_base_and_retained_source() {
        let (mut closure, bytes) = closure(&[0x3c01_8000, 0x8c22_1234, 0x0040_f809, 0], true);
        closure.indirect[0].memory_sources = vec![0x8000_1234];
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(
            report.shapes[0].local_definition,
            LocalDefinitionShapeV1::Load {
                base: RegisterFamilyV1::AssemblerTemporary,
                base_definition: BaseDefinitionShapeV1::Immediate,
            }
        );
        assert_eq!(
            report.shapes[0].memory_sources,
            MemorySourceCardinalityV1::One
        );
    }

    #[test]
    fn bounded_sites_are_not_counted() {
        let (mut closure, bytes) = closure(&[0x0320_0008, 0], false);
        closure.indirect[0].state = IndirectProofState::Bounded;
        let report = classify_open_indirects_v1(&closure, &bytes, 0x8000_0000).unwrap();
        assert_eq!(report.open_sites, 0);
        assert!(report.shapes.is_empty());
    }

    #[test]
    fn counterfactual_requires_every_blocker_to_match_the_family() {
        let (closure, bytes) = closure(&[0x8fb9_0010, 0x0320_0008, 0], false);
        let site = closure.indirect[0].site_pc;
        let mut owner_proof = OwnerProofReport {
            bank: "test".into(),
            assessments: vec![OwnerAssessment::Candidate {
                frontier: OwnerFrontier {
                    entry: BankAddr::new("test", 0x8000_0000),
                    proposed_va_end: Some(0x8000_000c),
                    blockers: vec![OwnerBlocker::UnresolvedIndirect {
                        site,
                        scope: IndirectScope::Bank,
                    }],
                },
            }],
        };
        let report =
            classify_open_indirects_with_owners_v1(&closure, &owner_proof, &bytes, 0x8000_0000)
                .unwrap();
        let all_open = report
            .mechanism_counterfactuals
            .iter()
            .find(|row| row.mechanism == OpenIndirectMechanismV1::AllOpenSites)
            .unwrap();
        let loads = report
            .mechanism_counterfactuals
            .iter()
            .find(|row| row.mechanism == OpenIndirectMechanismV1::LoadSites)
            .unwrap();
        assert_eq!(all_open.sole_owner_assessments_if_discharged, 1);
        assert_eq!(loads.sole_owner_assessments_if_discharged, 1);

        let OwnerAssessment::Candidate { frontier } = &mut owner_proof.assessments[0] else {
            unreachable!()
        };
        frontier.blockers.push(OwnerBlocker::NotProvenExecutable);
        let report =
            classify_open_indirects_with_owners_v1(&closure, &owner_proof, &bytes, 0x8000_0000)
                .unwrap();
        assert!(report
            .mechanism_counterfactuals
            .iter()
            .all(|row| row.sole_owner_assessments_if_discharged == 0));
    }
}
