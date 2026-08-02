//! Versioned, content-bound Recompiler Pack for function-independent blocks.
//!
//! The portable pack contains identities, geometry, terminators, and hashes,
//! never ROM words. Materialization re-verifies the normalized ROM, any
//! evaluator receipt, and each block digest before exposing instruction words
//! to a code generator.

use crate::block_proof::{BlockAssessment, InstalledCodeBlock, ReachableCodeBlock};
use crate::cfg::BlockTerminator;
use crate::cfg::WordClass;
use crate::facts::{BankBackingSpanV1, FactDb, RomAddressSpace};
use crate::materialized_image::{
    materialize_backing_span_v1, MaterializedBackingFactsRequirementV1,
    MaterializedBackingSpanCacheV1, MaterializedBackingSpanErrorV1, MaterializedImageErrorV1,
    MaterializedImageLimitsV1,
};
use crate::snapshot::{
    ProgramSnapshotV1, ValidatedComposedSnapshotsV2, PROGRAM_SNAPSHOT_SCHEMA_V6,
};
use crate::NormalizedRom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

pub const BLOCK_PACK_SCHEMA_V1: u32 = 1;
/// V2 adds a per-block `rom_space`, so a pack can carry VROM (DMA-loaded,
/// possibly compressed) blocks alongside physically-resident ones. A V1 pack
/// deserializes with `rom_space = Physical`, which is exactly what it meant,
/// so existing packs stay readable and keep their meaning.
pub const BLOCK_PACK_SCHEMA_V2: u32 = 2;
/// V3 replaces flat ROM coordinates with a tagged affine-ROM or evaluated
/// output span. No decoded bytes enter the portable wire.
pub const BLOCK_PACK_SCHEMA_V3: u32 = 3;

fn default_rom_space() -> crate::facts::RomAddressSpace {
    crate::facts::RomAddressSpace::Physical
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackedBlockV1 {
    pub start_va: u32,
    pub end_va: u32,
    pub backing: BankBackingSpanV1,
    pub bytes_sha256: String,
    pub terminator: crate::cfg::BlockTerminator,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedBlockV3Wire {
    start_va: u32,
    end_va: u32,
    backing: BankBackingSpanV1,
    bytes_sha256: String,
    terminator: crate::cfg::BlockTerminator,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PackedBlockLegacyWire {
    start_va: u32,
    end_va: u32,
    #[serde(default = "default_rom_space")]
    rom_space: RomAddressSpace,
    rom_start: u32,
    rom_end: u32,
    bytes_sha256: String,
    terminator: crate::cfg::BlockTerminator,
}

impl<'de> Deserialize<'de> for PackedBlockV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            V3(PackedBlockV3Wire),
            Legacy(PackedBlockLegacyWire),
        }

        Ok(match Wire::deserialize(deserializer)? {
            Wire::V3(wire) => Self {
                start_va: wire.start_va,
                end_va: wire.end_va,
                backing: wire.backing,
                bytes_sha256: wire.bytes_sha256,
                terminator: wire.terminator,
            },
            Wire::Legacy(wire) => Self {
                start_va: wire.start_va,
                end_va: wire.end_va,
                backing: BankBackingSpanV1::RomAffine {
                    rom_space: wire.rom_space,
                    rom_start: wire.rom_start,
                    rom_end: wire.rom_end,
                },
                bytes_sha256: wire.bytes_sha256,
                terminator: wire.terminator,
            },
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedBankV1 {
    pub bank: String,
    pub bank_id: u64,
    pub blocks: Vec<PackedBlockV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockPackV1 {
    pub schema_version: u32,
    pub normalized_rom_sha256: String,
    pub banks: Vec<PackedBankV1>,
}

#[derive(Debug, Clone)]
pub struct MaterializedPackedBlock {
    pub start_va: u32,
    pub words: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct MaterializedPackedBank {
    pub bank: String,
    pub bank_id: u64,
    pub blocks: Vec<MaterializedPackedBlock>,
}

/// Measured effect of adding one exact trace generation's executed words to a
/// materialized sparse bank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedExecutionAugmentReport {
    pub observed_words: usize,
    pub required_delay_slot_words: usize,
    pub newly_admitted_words: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObservedExecutionAugmentError {
    RomIdentityMismatch,
    TraceNotCompleted,
    BankNameMismatch { packed: String, observed: String },
    MappingAddressOverflow,
    ObservedPcOutsideMapping { pc: u32, va_start: u32, va_end: u32 },
    RomWordOutsideImage { pc: u32, rom_offset: u32 },
    ExistingWordMismatch { pc: u32, packed: u32, observed: u32 },
}

impl std::fmt::Display for ObservedExecutionAugmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ObservedExecutionAugmentError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPackError {
    UnsupportedSchema {
        expected: u32,
        actual: u32,
    },
    UnsupportedSnapshotSchema {
        expected: u32,
        actual: u32,
    },
    ValidatedSnapshotIndexOutsideComposition {
        index: usize,
        count: usize,
    },
    LegacySchemaVirtualBacking {
        bank: String,
        start_va: u32,
    },
    LegacySchemaMaterializedBacking {
        schema_version: u32,
        bank: String,
        start_va: u32,
    },
    RomIdentityMismatch,
    NoProvenBlocks {
        bank: String,
    },
    NonPhysicalBacking {
        bank: String,
        start_va: u32,
    },
    InvalidGeometry {
        bank: String,
        start_va: u32,
    },
    OverlappingBlocks {
        bank: String,
        left: u32,
        right: u32,
    },
    DuplicateBankId {
        bank: String,
        bank_id: u64,
    },
    RomRangeOutsideImage {
        bank: String,
        rom_start: u32,
        rom_end: u32,
    },
    BackingSpanLimitExceeded {
        bank: String,
        start_va: u32,
        bytes: usize,
        limit: usize,
    },
    BlockDigestMismatch {
        bank: String,
        start_va: u32,
    },
    /// A VROM-backed block cannot be re-verified without the file-table facts
    /// that resolve it. Callers holding a `FactDb` should use
    /// [`materialize_block_pack_with_facts`].
    VromRequiresFacts {
        bank: String,
        start_va: u32,
    },
    MaterializedRequiresFacts {
        bank: String,
        start_va: u32,
    },
    MissingEvaluatedImageReceipt {
        bank: String,
        start_va: u32,
        receipt_sha256: String,
    },
    AmbiguousEvaluatedImageReceipt {
        bank: String,
        start_va: u32,
        receipt_sha256: String,
        count: usize,
    },
    EvaluatedImageRederivation {
        bank: String,
        start_va: u32,
        receipt_sha256: String,
        error: MaterializedImageErrorV1,
    },
}

/// Explicit host policy needed to turn a portable pack into an executable
/// generated-source artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockProgramSourceConfig {
    pub entry: fn64_recomp_rs::ExecutionKey,
    pub instruction_budget: fn64_recomp_rs::InstructionBudget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockProgramSourceError {
    Pack(BlockPackError),
    InvalidBank {
        bank: String,
        error: fn64_recomp_rs::BankError,
    },
    DuplicateBankId {
        bank: fn64_recomp_rs::BankId,
    },
    EntryFault(fn64_recomp_rs::CpuFault),
    TooManyBanks {
        count: usize,
    },
}

impl std::fmt::Display for BlockProgramSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pack(error) => write!(f, "block pack: {error}"),
            Self::InvalidBank { bank, error } => {
                write!(f, "materialized bank {bank:?} is invalid: {error}")
            }
            Self::DuplicateBankId { bank } => {
                write!(f, "block pack repeats executable identity {bank}")
            }
            Self::EntryFault(fault) => {
                write!(f, "declared block-program entry is not admitted: {fault}")
            }
            Self::TooManyBanks { count } => write!(
                f,
                "block program has {count} banks, exceeding the u32 resolver ambiguity wire"
            ),
        }
    }
}

impl std::error::Error for BlockProgramSourceError {}

impl From<BlockPackError> for BlockProgramSourceError {
    fn from(error: BlockPackError) -> Self {
        Self::Pack(error)
    }
}

impl std::fmt::Display for BlockPackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for BlockPackError {}

/// Emit a diagnostic/interchange pack from an inspectable snapshot.
///
/// This compatibility API does not carry execution authority: callers can
/// construct or deserialize `ProgramSnapshotV1`. Current execution gates must
/// use [`emit_validated_block_pack_v3`], whose opaque composition wrapper can
/// only come from the byte-verifying snapshot pipeline. The historical Rust
/// function name is retained, but a V6 snapshot always emits a V3 pack.
pub fn emit_block_pack_v1(
    snapshot: &ProgramSnapshotV1,
    rom: &NormalizedRom,
) -> Result<BlockPackV1, BlockPackError> {
    emit_block_pack_from_snapshot(snapshot, rom)
}

/// Historical name retained for callers while the pack wire advances to V3.
/// A V6 composition always emits a V3 pack.
pub fn emit_validated_block_pack_v2(
    composition: &ValidatedComposedSnapshotsV2,
    snapshot_index: usize,
    rom: &NormalizedRom,
) -> Result<BlockPackV1, BlockPackError> {
    emit_validated_block_pack_v3(composition, snapshot_index, rom)
}

/// Emit one authoritative V3 pack from an exact member of a move-only,
/// byte-verified composition.
pub fn emit_validated_block_pack_v3(
    composition: &ValidatedComposedSnapshotsV2,
    snapshot_index: usize,
    rom: &NormalizedRom,
) -> Result<BlockPackV1, BlockPackError> {
    let snapshot = composition.snapshot(snapshot_index).ok_or(
        BlockPackError::ValidatedSnapshotIndexOutsideComposition {
            index: snapshot_index,
            count: composition.snapshots().len(),
        },
    )?;
    emit_block_pack_from_snapshot(snapshot, rom)
}

fn emit_block_pack_from_snapshot(
    snapshot: &ProgramSnapshotV1,
    rom: &NormalizedRom,
) -> Result<BlockPackV1, BlockPackError> {
    if snapshot.schema_version != PROGRAM_SNAPSHOT_SCHEMA_V6 {
        return Err(BlockPackError::UnsupportedSnapshotSchema {
            expected: PROGRAM_SNAPSHOT_SCHEMA_V6,
            actual: snapshot.schema_version,
        });
    }
    if snapshot.normalized_rom_sha256 != rom.sha256 {
        return Err(BlockPackError::RomIdentityMismatch);
    }
    let mut banks = Vec::with_capacity(snapshot.banks.len());
    let mut bank_ids = BTreeSet::new();
    let mut materialized_cache = MaterializedBackingSpanCacheV1::default();
    for bank_snapshot in &snapshot.banks {
        let bank = &bank_snapshot.input.bank;
        // Emission admits both claims. A `Proven` block is reachable from an
        // authoritative root; an `Installed` block is proven-installed code
        // whose entry no proven fact names (an overlay image the descriptor
        // table proves is DMA'd in). Both are byte-identical proven code with
        // a sound terminator and unique proven backing, which is everything
        // emission itself requires -- the distinction is what the metadata
        // claims about reachability, and it is preserved in `block_proof`'s
        // own assessments and counters rather than erased here.
        let mut proven: Vec<&ReachableCodeBlock> = Vec::new();
        let mut installed: Vec<&InstalledCodeBlock> = Vec::new();
        for assessment in &bank_snapshot.block_proof.assessments {
            match assessment {
                BlockAssessment::Proven { block } => proven.push(block),
                BlockAssessment::Installed { block, .. } => installed.push(block),
                BlockAssessment::Candidate { .. } => {}
            }
        }
        let mut admitted: Vec<PackBlockView<'_>> = proven
            .iter()
            .map(|block| PackBlockView::from(*block))
            .chain(installed.iter().map(|block| PackBlockView::from(*block)))
            .collect();
        admitted.sort_by_key(|block| block.start_va);
        if admitted.is_empty() {
            return Err(BlockPackError::NoProvenBlocks { bank: bank.clone() });
        }
        let proven = admitted;
        let geometry = complete_severed_delay_slots(
            &proven,
            &bank_snapshot.closure.cfg.word_class,
            rom,
            &snapshot.facts,
            MaterializedImageLimitsV1::default(),
            &mut materialized_cache,
        )?;
        validate_completed_geometry(bank, &geometry)?;
        let bank_id = stable_bank_id(&snapshot.normalized_rom_sha256, bank);
        if !bank_ids.insert(bank_id) {
            return Err(BlockPackError::DuplicateBankId {
                bank: bank.clone(),
                bank_id,
            });
        }
        let mut blocks = Vec::with_capacity(geometry.len());
        for (block, geom) in proven.iter().zip(&geometry) {
            let bytes = materialize_pack_backing_span(
                rom,
                Some(&snapshot.facts),
                bank,
                geom.start_va,
                geom.end_va,
                &geom.backing,
                MaterializedImageLimitsV1::default(),
                &mut materialized_cache,
            )?;
            blocks.push(PackedBlockV1 {
                start_va: geom.start_va,
                end_va: geom.end_va,
                backing: geom.backing.clone(),
                bytes_sha256: sha256_hex(&bytes),
                terminator: block.terminator.clone(),
            });
        }
        banks.push(PackedBankV1 {
            bank: bank.clone(),
            bank_id,
            blocks,
        });
    }
    banks.sort_by(|left, right| left.bank.cmp(&right.bank));
    Ok(BlockPackV1 {
        schema_version: BLOCK_PACK_SCHEMA_V3,
        normalized_rom_sha256: snapshot.normalized_rom_sha256.clone(),
        banks,
    })
}

/// Materialize a pack whose blocks are all physically resident. VROM and
/// evaluated-image spans require the fact database accepted by discovery.
pub fn materialize_block_pack(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
) -> Result<Vec<MaterializedPackedBank>, BlockPackError> {
    materialize_block_pack_with_facts(pack, rom, None)
}

/// Materialize a pack, resolving each block through its tagged backing.
/// `facts` supplies proven file-table records for VROM and the exact evaluated
/// image receipt for materialized output; physical blocks never consult it.
pub fn materialize_block_pack_with_facts(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
    facts: Option<&crate::facts::FactDb>,
) -> Result<Vec<MaterializedPackedBank>, BlockPackError> {
    if !matches!(
        pack.schema_version,
        BLOCK_PACK_SCHEMA_V1 | BLOCK_PACK_SCHEMA_V2 | BLOCK_PACK_SCHEMA_V3
    ) {
        return Err(BlockPackError::UnsupportedSchema {
            expected: BLOCK_PACK_SCHEMA_V3,
            actual: pack.schema_version,
        });
    }
    if pack.normalized_rom_sha256 != rom.sha256 {
        return Err(BlockPackError::RomIdentityMismatch);
    }
    let mut output = Vec::with_capacity(pack.banks.len());
    let mut bank_ids = BTreeSet::new();
    let mut materialized_cache = MaterializedBackingSpanCacheV1::default();
    for bank in &pack.banks {
        if bank.blocks.is_empty() {
            return Err(BlockPackError::NoProvenBlocks {
                bank: bank.bank.clone(),
            });
        }
        validate_packed_geometry(&bank.bank, &bank.blocks)?;
        if !bank_ids.insert(bank.bank_id) {
            return Err(BlockPackError::DuplicateBankId {
                bank: bank.bank.clone(),
                bank_id: bank.bank_id,
            });
        }
        let mut blocks = Vec::with_capacity(bank.blocks.len());
        for block in &bank.blocks {
            validate_schema_backing(pack.schema_version, &bank.bank, block)?;
            let bytes = materialize_pack_backing_span(
                rom,
                facts,
                &bank.bank,
                block.start_va,
                block.end_va,
                &block.backing,
                MaterializedImageLimitsV1::default(),
                &mut materialized_cache,
            )?;
            if sha256_hex(&bytes) != block.bytes_sha256 {
                return Err(BlockPackError::BlockDigestMismatch {
                    bank: bank.bank.clone(),
                    start_va: block.start_va,
                });
            }
            blocks.push(MaterializedPackedBlock {
                start_va: block.start_va,
                words: bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                    .collect(),
            });
        }
        output.push(MaterializedPackedBank {
            bank: bank.bank.clone(),
            bank_id: bank.bank_id,
            blocks,
        });
    }
    Ok(output)
}

fn validate_schema_backing(
    schema_version: u32,
    bank: &str,
    block: &PackedBlockV1,
) -> Result<(), BlockPackError> {
    match (schema_version, &block.backing) {
        (
            BLOCK_PACK_SCHEMA_V1,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Virtual,
                ..
            },
        ) => Err(BlockPackError::LegacySchemaVirtualBacking {
            bank: bank.to_owned(),
            start_va: block.start_va,
        }),
        (BLOCK_PACK_SCHEMA_V1 | BLOCK_PACK_SCHEMA_V2, BankBackingSpanV1::Materialized { .. }) => {
            Err(BlockPackError::LegacySchemaMaterializedBacking {
                schema_version,
                bank: bank.to_owned(),
                start_va: block.start_va,
            })
        }
        (BLOCK_PACK_SCHEMA_V1 | BLOCK_PACK_SCHEMA_V2 | BLOCK_PACK_SCHEMA_V3, _) => Ok(()),
        _ => unreachable!("pack schema was validated before block backing"),
    }
}

fn materialize_pack_backing_span(
    rom: &NormalizedRom,
    facts: Option<&FactDb>,
    bank: &str,
    start_va: u32,
    end_va: u32,
    backing: &BankBackingSpanV1,
    limits: MaterializedImageLimitsV1,
    materialized_cache: &mut MaterializedBackingSpanCacheV1,
) -> Result<Vec<u8>, BlockPackError> {
    materialize_backing_span_v1(
        rom,
        facts,
        bank,
        start_va,
        end_va,
        backing,
        limits,
        materialized_cache,
    )
    .map_err(|error| match error {
        MaterializedBackingSpanErrorV1::InvalidGeometry => BlockPackError::InvalidGeometry {
            bank: bank.to_owned(),
            start_va,
        },
        MaterializedBackingSpanErrorV1::SpanLimitExceeded { bytes, limit } => {
            BlockPackError::BackingSpanLimitExceeded {
                bank: bank.to_owned(),
                start_va,
                bytes,
                limit,
            }
        }
        MaterializedBackingSpanErrorV1::FactsRequired {
            requirement: MaterializedBackingFactsRequirementV1::VirtualRom,
        } => BlockPackError::VromRequiresFacts {
            bank: bank.to_owned(),
            start_va,
        },
        MaterializedBackingSpanErrorV1::FactsRequired {
            requirement: MaterializedBackingFactsRequirementV1::EvaluatedImage,
        } => BlockPackError::MaterializedRequiresFacts {
            bank: bank.to_owned(),
            start_va,
        },
        MaterializedBackingSpanErrorV1::RomMaterialization {
            rom_start, rom_end, ..
        } => BlockPackError::RomRangeOutsideImage {
            bank: bank.to_owned(),
            rom_start,
            rom_end,
        },
        MaterializedBackingSpanErrorV1::MissingEvaluatedImageReceipt { receipt_sha256 } => {
            BlockPackError::MissingEvaluatedImageReceipt {
                bank: bank.to_owned(),
                start_va,
                receipt_sha256,
            }
        }
        MaterializedBackingSpanErrorV1::AmbiguousEvaluatedImageReceipt {
            receipt_sha256,
            count,
        } => BlockPackError::AmbiguousEvaluatedImageReceipt {
            bank: bank.to_owned(),
            start_va,
            receipt_sha256,
            count,
        },
        MaterializedBackingSpanErrorV1::EvaluatedImageRederivation {
            receipt_sha256,
            error,
        } => BlockPackError::EvaluatedImageRederivation {
            bank: bank.to_owned(),
            start_va,
            receipt_sha256,
            error,
        },
    })
}

/// Union the exact instruction words observed in one trace bank generation
/// into an already materialized sparse bank.
///
/// This is scenario AOT coverage, not function-boundary discovery. Every
/// added word is re-read from the normalized ROM through the supplied affine
/// mapping; an observation outside that mapping traps instead of borrowing a
/// numeric VA from another image generation. Existing proven blocks and
/// observed words are canonicalized into disjoint contiguous spans.
pub fn augment_with_observed_execution(
    bank: &mut MaterializedPackedBank,
    report: &crate::trace::IngestReport,
    rom: &NormalizedRom,
    observed_bank: &str,
    activation: u64,
    rom_start: u32,
    va_start: u32,
    byte_len: u32,
) -> Result<ObservedExecutionAugmentReport, ObservedExecutionAugmentError> {
    if report.header.normalized_rom_sha256.as_str() != rom.sha256 {
        return Err(ObservedExecutionAugmentError::RomIdentityMismatch);
    }
    if report.completion != crate::trace::TraceCompletion::Completed {
        return Err(ObservedExecutionAugmentError::TraceNotCompleted);
    }
    if bank.bank != observed_bank {
        return Err(ObservedExecutionAugmentError::BankNameMismatch {
            packed: bank.bank.clone(),
            observed: observed_bank.to_string(),
        });
    }
    let va_end = va_start
        .checked_add(byte_len)
        .ok_or(ObservedExecutionAugmentError::MappingAddressOverflow)?;
    let mut words = BTreeMap::<u32, u32>::new();
    for block in &bank.blocks {
        for (index, word) in block.words.iter().copied().enumerate() {
            words.insert(
                block.start_va
                    + u32::try_from(index).expect("materialized block index fits u32") * 4,
                word,
            );
        }
    }

    let observed = crate::trace::observed_execution_roots(report, observed_bank, activation);
    let mut required = observed.clone();
    let mut required_delay_slot_words = 0usize;
    for pc in observed.iter().copied() {
        if pc < va_start || pc >= va_end {
            return Err(ObservedExecutionAugmentError::ObservedPcOutsideMapping {
                pc,
                va_start,
                va_end,
            });
        }
        let rom_offset = rom_start
            .checked_add(pc - va_start)
            .ok_or(ObservedExecutionAugmentError::MappingAddressOverflow)?;
        let start = usize::try_from(rom_offset).expect("u32 ROM offset fits usize");
        let bytes = rom
            .bytes
            .get(start..start + 4)
            .ok_or(ObservedExecutionAugmentError::RomWordOutsideImage { pc, rom_offset })?;
        let word = u32::from_be_bytes(bytes.try_into().expect("four-byte ROM word"));
        if fn64_recomp_rs::decode(word).has_delay_slot() {
            let delay_pc = pc
                .checked_add(4)
                .ok_or(ObservedExecutionAugmentError::MappingAddressOverflow)?;
            if required.insert(delay_pc) {
                required_delay_slot_words += 1;
            }
        }
    }
    let mut newly_admitted_words = 0usize;
    for pc in required.iter().copied() {
        if pc < va_start || pc >= va_end {
            return Err(ObservedExecutionAugmentError::ObservedPcOutsideMapping {
                pc,
                va_start,
                va_end,
            });
        }
        let rom_offset = rom_start
            .checked_add(pc - va_start)
            .ok_or(ObservedExecutionAugmentError::MappingAddressOverflow)?;
        let start = usize::try_from(rom_offset).expect("u32 ROM offset fits usize");
        let bytes = rom
            .bytes
            .get(start..start + 4)
            .ok_or(ObservedExecutionAugmentError::RomWordOutsideImage { pc, rom_offset })?;
        let observed_word = u32::from_be_bytes(bytes.try_into().expect("four-byte ROM word"));
        match words.insert(pc, observed_word) {
            Some(packed) if packed != observed_word => {
                return Err(ObservedExecutionAugmentError::ExistingWordMismatch {
                    pc,
                    packed,
                    observed: observed_word,
                });
            }
            Some(_) => {}
            None => newly_admitted_words += 1,
        }
    }

    let mut blocks = Vec::new();
    for (pc, word) in words {
        match blocks.last_mut() {
            Some(MaterializedPackedBlock {
                start_va,
                words: block_words,
            }) if start_va.checked_add(
                u32::try_from(block_words.len()).expect("block word count fits u32") * 4,
            ) == Some(pc) =>
            {
                block_words.push(word)
            }
            _ => blocks.push(MaterializedPackedBlock {
                start_va: pc,
                words: vec![word],
            }),
        }
    }
    bank.blocks = blocks;
    Ok(ObservedExecutionAugmentReport {
        observed_words: observed.len(),
        required_delay_slot_words,
        newly_admitted_words,
    })
}

/// Emit one deterministic, standalone Rust source module implementing the
/// typed block-program contract consumed by the public boot harness.
///
/// The pack is re-materialized from the supplied normalized ROM before any
/// instruction enters the source. The caller must choose an admitted
/// bank-qualified entry and a valid instruction budget explicitly. Generated
/// registration binds every runner to the artifact identity supplied later by
/// the compiling host; no identity-free registration path is emitted.
pub fn emit_block_program_source(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
    config: BlockProgramSourceConfig,
) -> Result<String, BlockProgramSourceError> {
    emit_block_program_source_with_facts(pack, rom, None, config)
}

/// As [`emit_block_program_source`], with the file-table facts a VROM-backed
/// pack needs to re-verify its blocks.
pub fn emit_block_program_source_with_facts(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
    facts: Option<&crate::facts::FactDb>,
    config: BlockProgramSourceConfig,
) -> Result<String, BlockProgramSourceError> {
    let mut banks = materialize_block_pack_with_facts(pack, rom, facts)?;
    u32::try_from(banks.len())
        .map_err(|_| BlockProgramSourceError::TooManyBanks { count: banks.len() })?;
    for bank in &mut banks {
        bank.blocks.sort_by_key(|block| block.start_va);
    }
    banks.sort_by_key(|bank| bank.bank_id);

    let mut catalog = fn64_recomp_rs::CodeCatalog::new();
    let mut ids = BTreeSet::new();
    for bank in &banks {
        let id = fn64_recomp_rs::BankId::new(bank.bank_id);
        if !ids.insert(id) {
            return Err(BlockProgramSourceError::DuplicateBankId { bank: id });
        }
        let code =
            materialized_code_bank(bank).map_err(|error| BlockProgramSourceError::InvalidBank {
                bank: bank.bank.clone(),
                error,
            })?;
        catalog
            .register(code)
            .map_err(|error| BlockProgramSourceError::InvalidBank {
                bank: bank.bank.clone(),
                error,
            })?;
    }
    catalog
        .resolve(config.entry)
        .map_err(BlockProgramSourceError::EntryFault)?;

    let mut source = String::new();
    writeln!(
        source,
        "// Generated by fn64-discover from BlockPackV1. No ROM bytes belong in the repository."
    )
    .unwrap();
    writeln!(
        source,
        "pub const FN64_BLOCK_PROGRAM_SOURCE_SCHEMA: u32 = 1;"
    )
    .unwrap();
    writeln!(
        source,
        "pub const FN64_BLOCK_PACK_ROM_SHA256: &str = {:?};",
        pack.normalized_rom_sha256
    )
    .unwrap();
    writeln!(
        source,
        "use fn64_recomp_rs::{{BankId, BlockExit, BlockProgram, BlockRun, CodeBank, CodeSpan, CpuException, CpuFault, CpuFaultKind, ExecutionKey, GeneratedBankRunner, GuestPc, InstructionBudget, ProgramArtifactIdentity, Rdram, RecompContext}};\n"
    )
    .unwrap();

    for bank in &banks {
        let name = format!("run_bank_{:016x}", bank.bank_id);
        let blocks = bank
            .blocks
            .iter()
            .map(|block| fn64_recomp_rs_codegen::BankBlockInput {
                vram: block.start_va,
                words: &block.words,
            })
            .collect::<Vec<_>>();
        source.push_str(&fn64_recomp_rs_codegen::emit_sparse_bank_runner_function(
            &fn64_recomp_rs_codegen::SparseBankInput {
                name: &name,
                bank: fn64_recomp_rs::BankId::new(bank.bank_id),
                blocks: &blocks,
            },
        ));
        source.push('\n');
    }

    emit_source_catalog_helpers(&mut source, &banks, config);
    emit_source_builder(&mut source, &banks);
    Ok(source)
}

/// Feed a re-verified materialized bank into the sparse arbitrary-PC emitter.
///
/// The adapter deliberately preserves the pack's disjoint spans. It never
/// widens them to one bounding interval, so bytes in code/data gaps cannot be
/// decoded or acquire same-bank transfer authority.
pub fn emit_materialized_bank_runner(bank: &MaterializedPackedBank, name: &str) -> String {
    emit_materialized_bank_runner_with_host_calls(bank, name, &[])
}

/// Feed a re-verified materialized bank into the sparse emitter with an exact
/// inventory of statically bound host-call destinations.
///
/// The caller owns semantic proof for every address in `host_calls`; this
/// adapter only preserves that typed boundary in generated control flow.
pub fn emit_materialized_bank_runner_with_host_calls(
    bank: &MaterializedPackedBank,
    name: &str,
    host_calls: &[u32],
) -> String {
    let blocks: Vec<fn64_recomp_rs_codegen::BankBlockInput<'_>> = bank
        .blocks
        .iter()
        .map(|block| fn64_recomp_rs_codegen::BankBlockInput {
            vram: block.start_va,
            words: &block.words,
        })
        .collect();
    fn64_recomp_rs_codegen::emit_sparse_bank_runner_with_host_calls(
        &fn64_recomp_rs_codegen::SparseBankInput {
            name,
            bank: fn64_recomp_rs::BankId::new(bank.bank_id),
            blocks: &blocks,
        },
        host_calls,
    )
}

/// Convert a re-verified pack bank into the runtime's owned sparse catalog
/// type without flattening gaps.
pub fn materialized_code_bank(
    bank: &MaterializedPackedBank,
) -> Result<fn64_recomp_rs::CodeBank, fn64_recomp_rs::BankError> {
    let id = fn64_recomp_rs::BankId::new(bank.bank_id);
    let spans = bank
        .blocks
        .iter()
        .map(|block| {
            fn64_recomp_rs::CodeSpan::new(
                id,
                fn64_recomp_rs::GuestPc::new(block.start_va),
                block.words.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    fn64_recomp_rs::CodeBank::from_spans(id, spans)
}

fn emit_source_catalog_helpers(
    source: &mut String,
    banks: &[MaterializedPackedBank],
    config: BlockProgramSourceConfig,
) {
    writeln!(
        source,
        "const ENTRY_BANK: BankId = BankId::new({:#018X});",
        config.entry.bank.get()
    )
    .unwrap();
    writeln!(
        source,
        "const ENTRY_PC: GuestPc = GuestPc::new({:#010X});",
        config.entry.pc.get()
    )
    .unwrap();
    writeln!(
        source,
        "const INSTRUCTION_BUDGET: u32 = {};",
        config.instruction_budget.get()
    )
    .unwrap();
    writeln!(source, "const BANKS: [BankId; {}] = [", banks.len()).unwrap();
    for bank in banks {
        writeln!(source, "    BankId::new({:#018X}),", bank.bank_id).unwrap();
    }
    writeln!(source, "];\n").unwrap();

    writeln!(
        source,
        "fn bank_admits(bank: BankId, pc: GuestPc) -> bool {{"
    )
    .unwrap();
    writeln!(source, "    let pc = pc.get();").unwrap();
    writeln!(source, "    match bank.get() {{").unwrap();
    for bank in banks {
        write!(source, "        {:#018X} => matches!(pc, ", bank.bank_id).unwrap();
        for (index, block) in bank.blocks.iter().enumerate() {
            if index != 0 {
                source.push_str(" | ");
            }
            let end = block.start_va + block.words.len() as u32 * 4 - 1;
            write!(source, "{:#010X}..={end:#010X}", block.start_va).unwrap();
        }
        writeln!(source, "),").unwrap();
    }
    writeln!(source, "        _ => false,").unwrap();
    writeln!(source, "    }}").unwrap();
    writeln!(source, "}}\n").unwrap();

    writeln!(
        source,
        "fn bank_bounds(bank: BankId) -> Option<(u32, u32)> {{"
    )
    .unwrap();
    writeln!(source, "    match bank.get() {{").unwrap();
    for bank in banks {
        let start = bank
            .blocks
            .first()
            .expect("validated nonempty bank")
            .start_va;
        let last = bank.blocks.last().expect("validated nonempty bank");
        let end = last.start_va + last.words.len() as u32 * 4;
        writeln!(
            source,
            "        {:#018X} => Some(({start:#010X}, {end:#010X})),",
            bank.bank_id
        )
        .unwrap();
    }
    writeln!(source, "        _ => None,").unwrap();
    writeln!(source, "    }}").unwrap();
    writeln!(source, "}}\n").unwrap();

    source.push_str(
        "fn missing_mapping(fault_bank: BankId, target_pc: GuestPc) -> CpuFault {\n\
         \x20   let at = ExecutionKey::new(fault_bank, target_pc);\n\
         \x20   match bank_bounds(fault_bank) {\n\
         \x20       Some((bank_start, bank_end)) => CpuFault {\n\
         \x20           at,\n\
         \x20           kind: CpuFaultKind::UnmappedPc { bank_start, bank_end },\n\
         \x20       },\n\
         \x20       None => CpuFault { at, kind: CpuFaultKind::UnknownBank },\n\
         \x20   }\n\
         }\n\n\
         fn resolve_unique(fault_bank: BankId, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {\n\
         \x20   let mut first = None;\n\
         \x20   let mut second = None;\n\
         \x20   let mut candidate_count = 0u32;\n\
         \x20   for bank in BANKS {\n\
         \x20       if bank_admits(bank, target_pc) {\n\
         \x20           candidate_count += 1;\n\
         \x20           if first.is_none() {\n\
         \x20               first = Some(bank);\n\
         \x20           } else if second.is_none() {\n\
         \x20               second = Some(bank);\n\
         \x20           }\n\
         \x20       }\n\
         \x20   }\n\
         \x20   match candidate_count {\n\
         \x20       0 => Err(missing_mapping(fault_bank, target_pc)),\n\
         \x20       1 => Ok(ExecutionKey::new(first.expect(\"one candidate was counted\"), target_pc)),\n\
         \x20       _ => Err(CpuFault {\n\
         \x20           at: ExecutionKey::new(fault_bank, target_pc),\n\
         \x20           kind: CpuFaultKind::AmbiguousPc {\n\
         \x20               first_candidate: first.expect(\"ambiguous lookup has a first candidate\"),\n\
         \x20               second_candidate: second.expect(\"ambiguous lookup has a second candidate\"),\n\
         \x20               candidate_count,\n\
         \x20           },\n\
         \x20       }),\n\
         \x20   }\n\
         }\n\n\
         pub fn entry() -> ExecutionKey {\n\
         \x20   ExecutionKey::new(ENTRY_BANK, ENTRY_PC)\n\
         }\n\n\
         pub fn entry_lookup(target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {\n\
         \x20   if !target_pc.is_instruction_aligned() {\n\
         \x20       return Err(CpuFault::instruction_address_error(ExecutionKey::new(ENTRY_BANK, target_pc)));\n\
         \x20   }\n\
         \x20   resolve_unique(ENTRY_BANK, target_pc)\n\
         }\n\n\
         pub fn transfer_lookup(source_bank: BankId, target_pc: GuestPc) -> Result<ExecutionKey, CpuFault> {\n\
         \x20   if !target_pc.is_instruction_aligned() {\n\
         \x20       return Err(CpuFault::instruction_address_error(ExecutionKey::new(source_bank, target_pc)));\n\
         \x20   }\n\
         \x20   if bank_admits(source_bank, target_pc) {\n\
         \x20       return Ok(ExecutionKey::new(source_bank, target_pc));\n\
         \x20   }\n\
         \x20   resolve_unique(source_bank, target_pc)\n\
         }\n\n\
         pub fn instruction_budget() -> InstructionBudget {\n\
         \x20   InstructionBudget::new(INSTRUCTION_BUDGET)\n\
         \x20       .expect(\"fn64-discover admitted the generated instruction budget\")\n\
         }\n\n",
    );
}

fn emit_source_builder(source: &mut String, banks: &[MaterializedPackedBank]) {
    source.push_str(
        "pub fn build_block_program(artifact_identity: ProgramArtifactIdentity) -> Result<BlockProgram, String> {\n\
         \x20   let mut program = BlockProgram::new();\n",
    );
    for bank in banks {
        let runner = format!("run_bank_{:016x}", bank.bank_id);
        writeln!(
            source,
            "    let bank = BankId::new({:#018X});",
            bank.bank_id
        )
        .unwrap();
        writeln!(source, "    let spans = vec![").unwrap();
        for block in &bank.blocks {
            writeln!(
                source,
                "        CodeSpan::new(bank, GuestPc::new({:#010X}), vec![",
                block.start_va
            )
            .unwrap();
            for chunk in block.words.chunks(8) {
                source.push_str("            ");
                for word in chunk {
                    write!(source, "{word:#010X}, ").unwrap();
                }
                source.push('\n');
            }
            writeln!(
                source,
                "        ]).map_err(|error| format!(\"construct {{bank}} span: {{error}}\"))?,"
            )
            .unwrap();
        }
        source.push_str(
            "    ];\n\
             \x20   let code = CodeBank::from_spans(bank, spans)\n\
             \x20       .map_err(|error| format!(\"construct {bank}: {error}\"))?;\n",
        );
        writeln!(
            source,
            "    program.register(code, GeneratedBankRunner::new_with_artifact_identity(bank, {runner}, artifact_identity))"
        )
        .unwrap();
        source.push_str("        .map_err(|error| format!(\"register {bank}: {error}\"))?;\n");
    }
    source.push_str("    Ok(program)\n}\n");
}

fn validate_packed_geometry(bank: &str, blocks: &[PackedBlockV1]) -> Result<(), BlockPackError> {
    let mut sorted = blocks.iter().collect::<Vec<_>>();
    sorted.sort_by_key(|block| block.start_va);
    let mut previous_end = None;
    for block in sorted {
        if !block.start_va.is_multiple_of(4)
            || !block.end_va.is_multiple_of(4)
            || block.end_va <= block.start_va
            || !backing_is_word_aligned(&block.backing)
            || backing_len(&block.backing) != block.end_va.checked_sub(block.start_va)
        {
            return Err(BlockPackError::InvalidGeometry {
                bank: bank.into(),
                start_va: block.start_va,
            });
        }
        if let Some(end) = previous_end {
            if block.start_va < end {
                return Err(BlockPackError::OverlappingBlocks {
                    bank: bank.into(),
                    left: end,
                    right: block.start_va,
                });
            }
        }
        previous_end = Some(block.end_va);
    }
    Ok(())
}

/// A proven block's emitted VA/backing extents after delay-slot completion.
/// Byte length is preserved between runtime and typed backing views.
#[derive(Clone)]
struct CompletedGeometry {
    start_va: u32,
    end_va: u32,
    backing: BankBackingSpanV1,
}

/// Realize the "control transfer and its delay slot are one architecturally
/// inseparable unit" invariant (DISCOVER-DESIGN Phase 4) at emission time.
///
/// `canonicalize_blocks` cuts a block at any later-discovered leader inside it,
/// replacing the control terminator with `Fallthrough` and handing the trailing
/// words to the leader's block. When the leader lands *on* a delay slot, the cut
/// can strand the control transfer from a contested leader block that is later
/// dropped. Re-attach exactly that one proven, unadmitted delay-slot word. The
/// proof state and owner geometry remain unchanged; this only regroups already
/// proven code for emission.
/// The block geometry emission consumes, borrowed from either assessment
/// kind.
///
/// `ReachableCodeBlock` additionally carries `authoritative_roots`, which no
/// code on this path reads. Projecting to the shared fields lets an
/// `InstalledCodeBlock` travel the same route without being converted into a
/// `ReachableCodeBlock` -- a conversion that would require inventing
/// reachability roots it does not have.
#[derive(Clone, Copy)]
struct PackBlockView<'a> {
    bank: &'a str,
    start_va: u32,
    end_va: u32,
    backing: &'a BankBackingSpanV1,
    terminator: &'a BlockTerminator,
}

impl<'a> From<&'a ReachableCodeBlock> for PackBlockView<'a> {
    fn from(block: &'a ReachableCodeBlock) -> Self {
        Self {
            bank: &block.bank,
            start_va: block.start_va,
            end_va: block.end_va,
            backing: &block.backing,
            terminator: &block.terminator,
        }
    }
}

impl<'a> From<&'a InstalledCodeBlock> for PackBlockView<'a> {
    fn from(block: &'a InstalledCodeBlock) -> Self {
        Self {
            bank: &block.bank,
            start_va: block.start_va,
            end_va: block.end_va,
            backing: &block.backing,
            terminator: &block.terminator,
        }
    }
}

fn complete_severed_delay_slots(
    blocks: &[PackBlockView<'_>],
    word_class: &BTreeMap<u32, WordClass>,
    rom: &NormalizedRom,
    facts: &FactDb,
    limits: MaterializedImageLimitsV1,
    materialized_cache: &mut MaterializedBackingSpanCacheV1,
) -> Result<Vec<CompletedGeometry>, BlockPackError> {
    let admitted: BTreeSet<u32> = blocks
        .iter()
        .flat_map(|block| (block.start_va..block.end_va).step_by(4))
        .collect();
    blocks
        .iter()
        .map(|block| -> Result<CompletedGeometry, BlockPackError> {
            let mut geom = CompletedGeometry {
                start_va: block.start_va,
                end_va: block.end_va,
                backing: block.backing.clone(),
            };
            let delay_slot_va = block.end_va;
            let last_start_va =
                block
                    .end_va
                    .checked_sub(4)
                    .ok_or_else(|| BlockPackError::InvalidGeometry {
                        bank: block.bank.to_string(),
                        start_va: block.start_va,
                    })?;
            let last_backing = backing_last_word(&block.backing).ok_or_else(|| {
                BlockPackError::InvalidGeometry {
                    bank: block.bank.to_string(),
                    start_va: block.start_va,
                }
            })?;
            let last_bytes = materialize_pack_backing_span(
                rom,
                Some(facts),
                &block.bank,
                last_start_va,
                block.end_va,
                &last_backing,
                limits,
                materialized_cache,
            )?;
            let last_word: [u8; 4] =
                last_bytes
                    .try_into()
                    .map_err(|_| BlockPackError::InvalidGeometry {
                        bank: block.bank.to_string(),
                        start_va: block.start_va,
                    })?;
            let last_word_control =
                fn64_recomp_rs::decode(u32::from_be_bytes(last_word)).has_delay_slot();
            if last_word_control
                && word_class.get(&delay_slot_va) == Some(&WordClass::ProvenCode)
                && !admitted.contains(&delay_slot_va)
            {
                geom.end_va =
                    geom.end_va
                        .checked_add(4)
                        .ok_or_else(|| BlockPackError::InvalidGeometry {
                            bank: block.bank.to_string(),
                            start_va: block.start_va,
                        })?;
                extend_backing_end(&mut geom.backing, 4).ok_or_else(|| {
                    BlockPackError::InvalidGeometry {
                        bank: block.bank.to_string(),
                        start_va: block.start_va,
                    }
                })?;
            }
            Ok(geom)
        })
        .collect()
}

fn validate_completed_geometry(
    bank: &str,
    blocks: &[CompletedGeometry],
) -> Result<(), BlockPackError> {
    let mut previous_end = None;
    for block in blocks {
        if !block.start_va.is_multiple_of(4)
            || !block.end_va.is_multiple_of(4)
            || block.end_va <= block.start_va
            || !backing_is_word_aligned(&block.backing)
            || backing_len(&block.backing) != block.end_va.checked_sub(block.start_va)
        {
            return Err(BlockPackError::InvalidGeometry {
                bank: bank.into(),
                start_va: block.start_va,
            });
        }
        if let Some(end) = previous_end {
            if block.start_va < end {
                return Err(BlockPackError::OverlappingBlocks {
                    bank: bank.into(),
                    left: end,
                    right: block.start_va,
                });
            }
        }
        previous_end = Some(block.end_va);
    }
    Ok(())
}

fn backing_len(backing: &BankBackingSpanV1) -> Option<u32> {
    match backing {
        BankBackingSpanV1::RomAffine {
            rom_start, rom_end, ..
        } => rom_end.checked_sub(*rom_start),
        BankBackingSpanV1::Materialized {
            output_start,
            output_end,
            ..
        } => output_end.checked_sub(*output_start),
    }
}

fn backing_is_word_aligned(backing: &BankBackingSpanV1) -> bool {
    match backing {
        BankBackingSpanV1::RomAffine {
            rom_start, rom_end, ..
        } => rom_start.is_multiple_of(4) && rom_end.is_multiple_of(4),
        BankBackingSpanV1::Materialized {
            output_start,
            output_end,
            ..
        } => output_start.is_multiple_of(4) && output_end.is_multiple_of(4),
    }
}

fn backing_last_word(backing: &BankBackingSpanV1) -> Option<BankBackingSpanV1> {
    match backing {
        BankBackingSpanV1::RomAffine {
            rom_space,
            rom_start,
            rom_end,
        } => Some(BankBackingSpanV1::RomAffine {
            rom_space: *rom_space,
            rom_start: rom_end.checked_sub(4).filter(|start| start >= rom_start)?,
            rom_end: *rom_end,
        }),
        BankBackingSpanV1::Materialized {
            receipt_sha256,
            output_start,
            output_end,
        } => Some(BankBackingSpanV1::Materialized {
            receipt_sha256: receipt_sha256.clone(),
            output_start: output_end
                .checked_sub(4)
                .filter(|start| start >= output_start)?,
            output_end: *output_end,
        }),
    }
}

fn extend_backing_end(backing: &mut BankBackingSpanV1, bytes: u32) -> Option<()> {
    match backing {
        BankBackingSpanV1::RomAffine { rom_end, .. } => *rom_end = rom_end.checked_add(bytes)?,
        BankBackingSpanV1::Materialized { output_end, .. } => {
            *output_end = output_end.checked_add(bytes)?
        }
    }
    Some(())
}

fn stable_bank_id(rom_sha256: &str, bank: &str) -> u64 {
    let digest = Sha256::digest(format!("fn64:block-pack:v1:{rom_sha256}:{bank}").as_bytes());
    u64::from_be_bytes(digest[..8].try_into().unwrap())
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::BlockTerminator;
    use crate::facts::{
        evaluated_image_receipt_sha256_v1, EvaluatedImageReceiptV1, Fact,
        MaterializationEvaluatorV1, MaterializedImageSourceV1, ProofState, RomAddressSpace,
    };
    use crate::materialized_image::evaluate_materialized_image_v1;
    use flate2::{write::DeflateEncoder, Compression};
    use fn64_recomp_rs::{BankId, CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    const BANK_A: u64 = 0x11;
    const BANK_B: u64 = 0x22;
    const ENTRY_PC: u32 = 0x8000_1000;
    const HOLE_PC: u32 = 0x8000_1010;
    const UNIQUE_B_PC: u32 = 0x8000_2000;
    const ROM_BASE: u32 = 0x1000;
    const JR_RA: u32 = 0x03e0_0008;
    const NOP: u32 = 0x0000_0000;

    fn rom_with(words: &[u32]) -> NormalizedRom {
        let bank = words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        let mut bytes = vec![0u8; ROM_BASE as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
        bytes[ROM_BASE as usize..].copy_from_slice(&bank);
        crate::normalize(&bytes).unwrap()
    }

    fn raw_deflate_stream(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut stream = Vec::with_capacity(6 + compressed.len());
        stream.extend_from_slice(&[0x11, 0x72]);
        stream.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        stream.extend_from_slice(&compressed);
        stream
    }

    fn materialized_fixture() -> (NormalizedRom, FactDb, EvaluatedImageReceiptV1, Vec<u8>) {
        let output = [NOP, JR_RA, NOP]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let encoded = raw_deflate_stream(&output);
        let mut rom_bytes = vec![0; (ROM_BASE as usize + encoded.len() + 3) & !3];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
        rom_bytes[ROM_BASE as usize..ROM_BASE as usize + encoded.len()].copy_from_slice(&encoded);
        let rom = crate::normalize(&rom_bytes).unwrap();
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_BASE,
            rom_end: ROM_BASE + encoded.len() as u32,
            cursor: 0,
        };
        let mut facts = FactDb::new();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &facts,
            &source,
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let receipt = evaluation.receipt().clone();
        let image = facts.insert(Fact::EvaluatedImage {
            bank: "boot".into(),
            va_start: ENTRY_PC,
            va_end: ENTRY_PC + output.len() as u32,
            receipt: receipt.clone(),
        });
        facts
            .conclude(
                "bank:boot",
                ProofState::Proven,
                vec![image],
                "test evaluated image",
            )
            .unwrap();
        (rom, facts, receipt, output)
    }

    fn reachable_block(
        start_va: u32,
        end_va: u32,
        terminator: BlockTerminator,
    ) -> ReachableCodeBlock {
        ReachableCodeBlock {
            bank: "boot".into(),
            start_va,
            end_va,
            authoritative_roots: crate::block_proof::AuthoritativeReachabilityRoots::new([
                ENTRY_PC,
            ])
            .unwrap(),
            backing: BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_BASE + (start_va - ENTRY_PC),
                rom_end: ROM_BASE + (end_va - ENTRY_PC),
            },
            terminator,
        }
    }

    #[test]
    fn observed_execution_augments_exact_words_and_required_delay_slot() {
        let jal = 0x0c00_0800;
        let rom = rom_with(&[NOP, jal, NOP, NOP]);
        let digest = crate::trace::NormalizedRomDigest::try_from(rom.sha256.clone()).unwrap();
        let report = crate::trace::IngestReport {
            header: crate::trace::TraceHeader {
                schema_version: crate::trace::TRACE_SCHEMA_VERSION,
                normalized_rom_sha256: digest,
                trace_id: "observed-aot-test".into(),
                producer: "synthetic-test".into(),
            },
            completion: crate::trace::TraceCompletion::Completed,
            final_sequence: 2,
            counts: crate::trace::TraceEventCounts {
                executed_pc: 1,
                ..Default::default()
            },
            observations_with_unknown_bank: 0,
            facts: vec![crate::trace::ObservedTraceFact::ExecutedPc {
                sequence: 1,
                pc: crate::trace::ObservedAddress {
                    address: ENTRY_PC + 4,
                    bank: crate::trace::BankContext::Known {
                        bank: "boot".into(),
                        activation: 0,
                    },
                },
            }],
            exhaustiveness: Vec::new(),
        };
        let mut bank = MaterializedPackedBank {
            bank: "boot".into(),
            bank_id: BANK_A,
            blocks: vec![MaterializedPackedBlock {
                start_va: ENTRY_PC,
                words: vec![NOP],
            }],
        };

        let augmented = augment_with_observed_execution(
            &mut bank, &report, &rom, "boot", 0, ROM_BASE, ENTRY_PC, 16,
        )
        .unwrap();
        assert_eq!(
            augmented,
            ObservedExecutionAugmentReport {
                observed_words: 1,
                required_delay_slot_words: 1,
                newly_admitted_words: 2,
            }
        );
        assert_eq!(bank.blocks.len(), 1);
        assert_eq!(bank.blocks[0].start_va, ENTRY_PC);
        assert_eq!(bank.blocks[0].words, vec![NOP, jal, NOP]);
    }

    #[test]
    fn severed_proven_delay_slot_is_reattached_to_its_control_block() {
        let rom = rom_with(&[NOP, JR_RA, NOP]);
        let control = reachable_block(
            ENTRY_PC,
            ENTRY_PC + 8,
            BlockTerminator::Fallthrough { next: ENTRY_PC + 8 },
        );
        let word_class = BTreeMap::from([
            (ENTRY_PC, WordClass::ProvenCode),
            (ENTRY_PC + 4, WordClass::ProvenCode),
            (ENTRY_PC + 8, WordClass::ProvenCode),
        ]);
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&control)],
            &word_class,
            &rom,
            &crate::facts::FactDb::new(),
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 0x0c);
        assert_eq!(
            geometry[0].backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_BASE,
                rom_end: ROM_BASE + 0x0c,
            }
        );
    }

    #[test]
    fn admitted_delay_slot_is_not_duplicated() {
        let rom = rom_with(&[NOP, JR_RA, NOP, JR_RA, NOP]);
        let control = reachable_block(
            ENTRY_PC,
            ENTRY_PC + 8,
            BlockTerminator::Fallthrough { next: ENTRY_PC + 8 },
        );
        let next = reachable_block(ENTRY_PC + 8, ENTRY_PC + 0x14, BlockTerminator::Return);
        let word_class = (0..5)
            .map(|index| (ENTRY_PC + index * 4, WordClass::ProvenCode))
            .collect();
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&control), PackBlockView::from(&next)],
            &word_class,
            &rom,
            &crate::facts::FactDb::new(),
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 8);
        assert_eq!(geometry[1].start_va, ENTRY_PC + 8);
    }

    #[test]
    fn control_shaped_word_with_unproven_successor_is_not_extended() {
        let rom = rom_with(&[NOP, JR_RA, JR_RA]);
        let control = reachable_block(ENTRY_PC, ENTRY_PC + 0x0c, BlockTerminator::Return);
        let word_class = BTreeMap::from([
            (ENTRY_PC, WordClass::ProvenCode),
            (ENTRY_PC + 4, WordClass::ProvenCode),
            (ENTRY_PC + 8, WordClass::ProvenCode),
        ]);
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&control)],
            &word_class,
            &rom,
            &crate::facts::FactDb::new(),
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 0x0c);
    }

    #[test]
    fn materialized_pack_rederives_receipt_and_extends_tagged_delay_slot() {
        let (rom, facts, receipt, output) = materialized_fixture();
        let receipt_sha256 = evaluated_image_receipt_sha256_v1(&receipt);
        let block = ReachableCodeBlock {
            bank: "boot".into(),
            start_va: ENTRY_PC,
            end_va: ENTRY_PC + 8,
            authoritative_roots: crate::block_proof::AuthoritativeReachabilityRoots::new([
                ENTRY_PC,
            ])
            .unwrap(),
            backing: BankBackingSpanV1::Materialized {
                receipt_sha256: receipt_sha256.clone(),
                output_start: 0,
                output_end: 8,
            },
            terminator: BlockTerminator::Fallthrough { next: ENTRY_PC + 8 },
        };
        let word_class = BTreeMap::from([
            (ENTRY_PC, WordClass::ProvenCode),
            (ENTRY_PC + 4, WordClass::ProvenCode),
            (ENTRY_PC + 8, WordClass::ProvenCode),
        ]);
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&block)],
            &word_class,
            &rom,
            &facts,
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 12);
        assert_eq!(
            geometry[0].backing,
            BankBackingSpanV1::Materialized {
                receipt_sha256: receipt_sha256.clone(),
                output_start: 0,
                output_end: 12,
            }
        );

        let pack = BlockPackV1 {
            schema_version: BLOCK_PACK_SCHEMA_V3,
            normalized_rom_sha256: rom.sha256.clone(),
            banks: vec![PackedBankV1 {
                bank: "boot".into(),
                bank_id: BANK_A,
                blocks: vec![PackedBlockV1 {
                    start_va: ENTRY_PC,
                    end_va: ENTRY_PC + 12,
                    backing: geometry[0].backing.clone(),
                    bytes_sha256: sha256_hex(&output),
                    terminator: block.terminator.clone(),
                }],
            }],
        };
        let materialized = materialize_block_pack_with_facts(&pack, &rom, Some(&facts)).unwrap();
        assert_eq!(materialized[0].blocks[0].words, vec![NOP, JR_RA, NOP]);
        let wire = serde_json::to_string(&pack).unwrap();
        assert!(wire.contains("\"kind\":\"materialized\""));
        assert!(!wire.contains("\"words\""));

        let mut tampered_receipt = receipt;
        tampered_receipt.streams[0].output_sha256 = "00".repeat(32);
        let tampered_digest = evaluated_image_receipt_sha256_v1(&tampered_receipt);
        let mut tampered_facts = FactDb::new();
        let tampered_image = tampered_facts.insert(Fact::EvaluatedImage {
            bank: "boot".into(),
            va_start: ENTRY_PC,
            va_end: ENTRY_PC + 12,
            receipt: tampered_receipt,
        });
        tampered_facts
            .conclude(
                "bank:boot",
                ProofState::Proven,
                vec![tampered_image],
                "test tampered evaluated image",
            )
            .unwrap();
        let mut tampered_pack = pack;
        let BankBackingSpanV1::Materialized { receipt_sha256, .. } =
            &mut tampered_pack.banks[0].blocks[0].backing
        else {
            panic!("test pack is materialized")
        };
        *receipt_sha256 = tampered_digest;
        assert!(matches!(
            materialize_block_pack_with_facts(&tampered_pack, &rom, Some(&tampered_facts),),
            Err(BlockPackError::EvaluatedImageRederivation {
                error: MaterializedImageErrorV1::ReceiptMismatch { .. },
                ..
            })
        ));
    }

    #[test]
    fn legacy_v1_v2_block_wires_deserialize_to_affine_backing() {
        let terminator = serde_json::to_value(BlockTerminator::Return).unwrap();
        let v1 = serde_json::json!({
            "start_va": ENTRY_PC,
            "end_va": ENTRY_PC + 4,
            "rom_start": ROM_BASE,
            "rom_end": ROM_BASE + 4,
            "bytes_sha256": "11".repeat(32),
            "terminator": terminator,
        });
        let v1: PackedBlockV1 = serde_json::from_value(v1).unwrap();
        assert_eq!(
            v1.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_BASE,
                rom_end: ROM_BASE + 4,
            }
        );
        validate_schema_backing(BLOCK_PACK_SCHEMA_V1, "boot", &v1).unwrap();
        validate_packed_geometry("boot", std::slice::from_ref(&v1)).unwrap();

        let v2 = serde_json::json!({
            "start_va": ENTRY_PC,
            "end_va": ENTRY_PC + 4,
            "rom_space": "Virtual",
            "rom_start": 0x2000,
            "rom_end": 0x2004,
            "bytes_sha256": "22".repeat(32),
            "terminator": BlockTerminator::Return,
        });
        let v2: PackedBlockV1 = serde_json::from_value(v2).unwrap();
        assert_eq!(
            v2.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Virtual,
                rom_start: 0x2000,
                rom_end: 0x2004,
            }
        );
        validate_schema_backing(BLOCK_PACK_SCHEMA_V2, "boot", &v2).unwrap();
        validate_packed_geometry("boot", std::slice::from_ref(&v2)).unwrap();
    }

    fn synthetic_pack() -> (BlockPackV1, NormalizedRom) {
        let words = [0x2402_0007u32, 0, 0x2402_0009, 0x2403_0005];
        let mut bytes = vec![0u8; ROM_BASE as usize + words.len() * 4];
        bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
        for (index, word) in words.into_iter().enumerate() {
            let start = ROM_BASE as usize + index * 4;
            bytes[start..start + 4].copy_from_slice(&word.to_be_bytes());
        }
        let rom = crate::normalize(&bytes).unwrap();
        let block = |start_va: u32, word_index: u32| {
            let rom_start = ROM_BASE + word_index * 4;
            PackedBlockV1 {
                start_va,
                end_va: start_va + 4,
                backing: BankBackingSpanV1::RomAffine {
                    rom_space: crate::facts::RomAddressSpace::Physical,
                    rom_start,
                    rom_end: rom_start + 4,
                },
                bytes_sha256: sha256_hex(&rom.bytes[rom_start as usize..rom_start as usize + 4]),
                terminator: crate::cfg::BlockTerminator::Fallthrough { next: start_va + 4 },
            }
        };
        (
            BlockPackV1 {
                schema_version: BLOCK_PACK_SCHEMA_V1,
                normalized_rom_sha256: rom.sha256.clone(),
                banks: vec![
                    PackedBankV1 {
                        bank: "resident".into(),
                        bank_id: BANK_A,
                        blocks: vec![block(ENTRY_PC, 0), block(0x8000_1020, 1)],
                    },
                    PackedBankV1 {
                        bank: "overlay".into(),
                        bank_id: BANK_B,
                        blocks: vec![block(ENTRY_PC, 2), block(UNIQUE_B_PC, 3)],
                    },
                ],
            },
            rom,
        )
    }

    fn source_config() -> BlockProgramSourceConfig {
        BlockProgramSourceConfig {
            entry: ExecutionKey::new(BankId::new(BANK_A), GuestPc::new(ENTRY_PC)),
            instruction_budget: InstructionBudget::new(2).unwrap(),
        }
    }

    #[test]
    fn block_program_source_is_deterministic_sparse_and_identity_bound() {
        let (pack, rom) = synthetic_pack();
        let source = emit_block_program_source(&pack, &rom, source_config()).unwrap();
        let mut reordered = pack.clone();
        reordered.banks.reverse();
        for bank in &mut reordered.banks {
            bank.blocks.reverse();
        }
        assert_eq!(
            source,
            emit_block_program_source(&reordered, &rom, source_config()).unwrap()
        );
        assert_eq!(
            source
                .matches("GeneratedBankRunner::new_with_artifact_identity")
                .count(),
            2
        );
        assert!(!source.contains("GeneratedBankRunner::new("));
        assert!(source.contains("0x80001000..=0x80001003 | 0x80001020..=0x80001023"));
        assert!(!source.contains("0x80001004..=0x8000101F"));
        assert!(source.contains("pub fn entry_lookup(target_pc: GuestPc)"));
        assert!(source.contains("CpuFaultKind::AmbiguousPc"));
    }

    #[test]
    fn block_program_source_rejects_unadmitted_entry_and_malformed_pack() {
        let (pack, rom) = synthetic_pack();
        let config = BlockProgramSourceConfig {
            entry: ExecutionKey::new(BankId::new(BANK_A), GuestPc::new(HOLE_PC)),
            instruction_budget: InstructionBudget::new(2).unwrap(),
        };
        assert!(matches!(
            emit_block_program_source(&pack, &rom, config),
            Err(BlockProgramSourceError::EntryFault(fn64_recomp_rs::CpuFault {
                at,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: ENTRY_PC,
                    bank_end: 0x8000_1024,
                },
            })) if at == config.entry
        ));

        let mut wrong_schema = pack.clone();
        // A version newer than any this build supports, so the check stays
        // meaningful as supported versions are added.
        wrong_schema.schema_version = BLOCK_PACK_SCHEMA_V3 + 1;
        assert!(matches!(
            materialize_block_pack(&wrong_schema, &rom),
            Err(BlockPackError::UnsupportedSchema { .. })
        ));

        let mut false_legacy_virtual = pack.clone();
        let BankBackingSpanV1::RomAffine { rom_space, .. } =
            &mut false_legacy_virtual.banks[0].blocks[0].backing
        else {
            panic!("synthetic V1 block is affine")
        };
        *rom_space = RomAddressSpace::Virtual;
        assert!(matches!(
            materialize_block_pack_with_facts(
                &false_legacy_virtual,
                &rom,
                Some(&crate::facts::FactDb::new()),
            ),
            Err(BlockPackError::LegacySchemaVirtualBacking {
                bank,
                start_va: ENTRY_PC,
            }) if bank == "resident"
        ));

        let mut malformed = pack;
        malformed.banks[0].blocks[0].end_va += 4;
        assert!(matches!(
            materialize_block_pack(&malformed, &rom),
            Err(BlockPackError::InvalidGeometry { .. })
        ));

        let (mut trailing_bytes, rom) = synthetic_pack();
        let BankBackingSpanV1::RomAffine { rom_end, .. } =
            &mut trailing_bytes.banks[0].blocks[0].backing
        else {
            panic!("synthetic V1 block is affine")
        };
        *rom_end += 1;
        assert!(matches!(
            materialize_block_pack(&trailing_bytes, &rom),
            Err(BlockPackError::InvalidGeometry { .. })
        ));
    }

    #[test]
    fn generated_block_program_compiles_executes_and_rejects_ambiguity() {
        let (pack, rom) = synthetic_pack();
        let source = emit_block_program_source(&pack, &rom, source_config()).unwrap();
        let wrapper = format!(
            r#"{source}

fn main() {{
    let artifact = ProgramArtifactIdentity::new([0xA5; 32]);
    let program = build_block_program(artifact).unwrap();
    assert_eq!(entry(), ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({ENTRY_PC})));
    assert_eq!(instruction_budget().get(), 2);

    let evidence = program.evidence_snapshot();
    assert_eq!(evidence.banks.len(), 2);
    assert!(evidence.banks.iter().all(|bank| bank.runner_artifact_identity == artifact));
    assert_eq!(evidence.banks[0].spans.len(), 2);
    assert_eq!(evidence.banks[0].spans[0].vram_start, GuestPc::new({ENTRY_PC}));
    assert_eq!(evidence.banks[0].spans[1].vram_start, GuestPc::new(0x8000_1020));

    assert!(matches!(
        entry_lookup(GuestPc::new({ENTRY_PC})),
        Err(CpuFault {{
            kind: CpuFaultKind::AmbiguousPc {{
                first_candidate,
                second_candidate,
                candidate_count: 2,
            }},
            ..
        }}) if first_candidate == BankId::new({BANK_A}) && second_candidate == BankId::new({BANK_B})
    ));
    assert_eq!(
        transfer_lookup(BankId::new({BANK_A}), GuestPc::new({ENTRY_PC})).unwrap(),
        ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({ENTRY_PC}))
    );
    assert_eq!(
        transfer_lookup(BankId::new({BANK_B}), GuestPc::new({ENTRY_PC})).unwrap(),
        ExecutionKey::new(BankId::new({BANK_B}), GuestPc::new({ENTRY_PC}))
    );
    assert_eq!(
        transfer_lookup(BankId::new({BANK_A}), GuestPc::new({UNIQUE_B_PC})).unwrap(),
        ExecutionKey::new(BankId::new({BANK_B}), GuestPc::new({UNIQUE_B_PC}))
    );
    assert!(matches!(
        entry_lookup(GuestPc::new({HOLE_PC})),
        Err(CpuFault {{
            at,
            kind: CpuFaultKind::UnmappedPc {{
                bank_start: {ENTRY_PC},
                bank_end: 0x8000_1024,
            }},
        }}) if at == ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({HOLE_PC}))
    ));
    assert!(matches!(
        transfer_lookup(BankId::new({BANK_A}), GuestPc::new({HOLE_PC})),
        Err(CpuFault {{
            at,
            kind: CpuFaultKind::UnmappedPc {{
                bank_start: {ENTRY_PC},
                bank_end: 0x8000_1024,
            }},
        }}) if at == ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({HOLE_PC}))
    ));

    let mut backing = vec![0u8; fn64_recomp_rs::RDRAM_LEN];
    let mut mem = Rdram::new(&mut backing);
    let mut ctx = RecompContext::default();
    let run = program.run(entry(), instruction_budget(), &mut ctx, &mut mem);
    assert_eq!(ctx.r_u32(2), 7);
    assert_eq!(run.instructions, 1);
    assert_eq!(
        run.exit,
        BlockExit::ResolveTransfer {{
            source_bank: BankId::new({BANK_A}),
            target_pc: GuestPc::new(0x8000_1004),
        }}
    );
}}
"#
        );
        compile_and_run(&wrapper);
    }

    fn compile_and_run(source: &str) {
        let deps = current_dependency_dir();
        let rlib = current_recomp_rlib(&deps);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "fn64-block-program-source-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&temp).unwrap();
        let source_path = temp.join("main.rs");
        let binary_path = temp.join("generated-block-program");
        std::fs::write(&source_path, source).unwrap();
        let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .arg("--edition=2021")
            .arg(&source_path)
            .arg("--extern")
            .arg(format!("fn64_recomp_rs={}", rlib.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(&binary_path)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "generated source failed to compile:\nstdout:\n{}\nstderr:\n{}\nsource:\n{}",
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr),
            source
        );
        let run = Command::new(&binary_path).output().unwrap();
        assert!(
            run.status.success(),
            "generated source failed to execute:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    fn current_dependency_dir() -> PathBuf {
        let executable = std::env::current_exe().unwrap();
        executable
            .parent()
            .expect("fn64-discover test executable has a dependency directory")
            .to_owned()
    }

    fn current_recomp_rlib(deps: &Path) -> PathBuf {
        std::fs::read_dir(deps)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("libfn64_recomp_rs-") && name.ends_with(".rlib")
                    })
            })
            .max_by_key(|path| {
                path.metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
            })
            .expect("fn64-recomp-rs rlib is beside fn64-discover test executable")
    }
}
