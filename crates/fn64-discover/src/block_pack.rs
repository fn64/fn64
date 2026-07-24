//! Versioned, content-bound Recompiler Pack for function-independent blocks.
//!
//! The portable pack contains identities, geometry, terminators, and hashes,
//! never ROM words. Materialization re-verifies the normalized ROM and each
//! block digest before exposing instruction words to a code generator.

use crate::block_proof::{BlockAssessment, ReachableCodeBlock};
use crate::cfg::WordClass;
use crate::snapshot::ProgramSnapshotV1;
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

fn default_rom_space() -> crate::facts::RomAddressSpace {
    crate::facts::RomAddressSpace::Physical
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackedBlockV1 {
    pub start_va: u32,
    pub end_va: u32,
    /// Address space `rom_start`/`rom_end` are expressed in. Absent in V1
    /// packs, which were physical-only.
    #[serde(default = "default_rom_space")]
    pub rom_space: crate::facts::RomAddressSpace,
    pub rom_start: u32,
    pub rom_end: u32,
    pub bytes_sha256: String,
    pub terminator: crate::cfg::BlockTerminator,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockPackError {
    UnsupportedSchema {
        expected: u32,
        actual: u32,
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

pub fn emit_block_pack_v1(
    snapshot: &ProgramSnapshotV1,
    rom: &NormalizedRom,
) -> Result<BlockPackV1, BlockPackError> {
    if snapshot.normalized_rom_sha256 != rom.sha256 {
        return Err(BlockPackError::RomIdentityMismatch);
    }
    let mut banks = Vec::with_capacity(snapshot.banks.len());
    let mut bank_ids = BTreeSet::new();
    for bank_snapshot in &snapshot.banks {
        let bank = &bank_snapshot.input.bank;
        let mut proven: Vec<&ReachableCodeBlock> = bank_snapshot
            .block_proof
            .assessments
            .iter()
            .filter_map(|assessment| match assessment {
                BlockAssessment::Proven { block } => Some(block),
                BlockAssessment::Candidate { .. } => None,
            })
            .collect();
        proven.sort_by_key(|block| block.start_va);
        if proven.is_empty() {
            return Err(BlockPackError::NoProvenBlocks { bank: bank.clone() });
        }
        let geometry =
            complete_severed_delay_slots(&proven, &bank_snapshot.closure.cfg.word_class, rom);
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
            // Resolve the block's backing bytes in its own address space: a
            // physically-resident block slices the image, a VROM (DMA-loaded)
            // block resolves through its one proven file-table record. The
            // digest below is over the resolved bytes either way, so a pack
            // stays byte-bound regardless of how the bank reaches RDRAM.
            let resolved = crate::banks::materialize_rom_range(
                rom,
                &snapshot.facts,
                block.rom_space,
                geom.rom_start,
                geom.rom_end,
            )
            .map_err(|_| BlockPackError::RomRangeOutsideImage {
                bank: bank.clone(),
                rom_start: geom.rom_start,
                rom_end: geom.rom_end,
            })?;
            let bytes = resolved.bytes.as_slice();
            blocks.push(PackedBlockV1 {
                start_va: geom.start_va,
                end_va: geom.end_va,
                rom_space: block.rom_space,
                rom_start: geom.rom_start,
                rom_end: geom.rom_end,
                bytes_sha256: sha256_hex(bytes),
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
        schema_version: BLOCK_PACK_SCHEMA_V2,
        normalized_rom_sha256: snapshot.normalized_rom_sha256.clone(),
        banks,
    })
}

/// Materialize a pack whose blocks are all physically resident. A VROM-backed
/// block returns [`BlockPackError::VromRequiresFacts`]; use
/// [`materialize_block_pack_with_facts`] for packs that carry one.
pub fn materialize_block_pack(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
) -> Result<Vec<MaterializedPackedBank>, BlockPackError> {
    materialize_block_pack_with_facts(pack, rom, None)
}

/// Materialize a pack, resolving each block in its own address space. `facts`
/// supplies the proven file-table records a VROM (DMA-loaded, possibly
/// compressed) block needs; physical blocks never consult it.
pub fn materialize_block_pack_with_facts(
    pack: &BlockPackV1,
    rom: &NormalizedRom,
    facts: Option<&crate::facts::FactDb>,
) -> Result<Vec<MaterializedPackedBank>, BlockPackError> {
    if pack.schema_version != BLOCK_PACK_SCHEMA_V1
        && pack.schema_version != BLOCK_PACK_SCHEMA_V2
    {
        return Err(BlockPackError::UnsupportedSchema {
            expected: BLOCK_PACK_SCHEMA_V2,
            actual: pack.schema_version,
        });
    }
    if pack.normalized_rom_sha256 != rom.sha256 {
        return Err(BlockPackError::RomIdentityMismatch);
    }
    let mut output = Vec::with_capacity(pack.banks.len());
    let mut bank_ids = BTreeSet::new();
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
            let resolved;
            let bytes = match (block.rom_space, facts) {
                (crate::facts::RomAddressSpace::Physical, _) => rom
                    .bytes
                    .get(block.rom_start as usize..block.rom_end as usize),
                (crate::facts::RomAddressSpace::Virtual, Some(facts)) => {
                    resolved = crate::banks::materialize_rom_range(
                        rom,
                        facts,
                        crate::facts::RomAddressSpace::Virtual,
                        block.rom_start,
                        block.rom_end,
                    )
                    .map_err(|_| BlockPackError::RomRangeOutsideImage {
                        bank: bank.bank.clone(),
                        rom_start: block.rom_start,
                        rom_end: block.rom_end,
                    })?;
                    Some(resolved.bytes.as_slice())
                }
                (crate::facts::RomAddressSpace::Virtual, None) => {
                    return Err(BlockPackError::VromRequiresFacts {
                        bank: bank.bank.clone(),
                        start_va: block.start_va,
                    });
                }
            }
                .ok_or(BlockPackError::RomRangeOutsideImage {
                    bank: bank.bank.clone(),
                    rom_start: block.rom_start,
                    rom_end: block.rom_end,
                })?;
            if sha256_hex(bytes) != block.bytes_sha256 {
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
            .map(|block| fn64_recomp_rs::BankBlockInput {
                vram: block.start_va,
                words: &block.words,
            })
            .collect::<Vec<_>>();
        source.push_str(&fn64_recomp_rs::emit_sparse_bank_runner_function(
            &fn64_recomp_rs::SparseBankInput {
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
    let blocks: Vec<fn64_recomp_rs::BankBlockInput<'_>> = bank
        .blocks
        .iter()
        .map(|block| fn64_recomp_rs::BankBlockInput {
            vram: block.start_va,
            words: &block.words,
        })
        .collect();
    fn64_recomp_rs::emit_sparse_bank_runner(&fn64_recomp_rs::SparseBankInput {
        name,
        bank: fn64_recomp_rs::BankId::new(bank.bank_id),
        blocks: &blocks,
    })
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
            || !block.rom_start.is_multiple_of(4)
            || !block.rom_end.is_multiple_of(4)
            || block.end_va <= block.start_va
            || block.rom_end.checked_sub(block.rom_start)
                != block.end_va.checked_sub(block.start_va)
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

/// A proven block's emitted VA/ROM extents after delay-slot completion. Byte
/// length is preserved between the VA and ROM views.
#[derive(Clone, Copy)]
struct CompletedGeometry {
    start_va: u32,
    end_va: u32,
    rom_start: u32,
    rom_end: u32,
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
fn complete_severed_delay_slots(
    blocks: &[&ReachableCodeBlock],
    word_class: &BTreeMap<u32, WordClass>,
    rom: &NormalizedRom,
) -> Vec<CompletedGeometry> {
    let admitted: BTreeSet<u32> = blocks
        .iter()
        .flat_map(|block| (block.start_va..block.end_va).step_by(4))
        .collect();
    blocks
        .iter()
        .map(|block| {
            let mut geom = CompletedGeometry {
                start_va: block.start_va,
                end_va: block.end_va,
                rom_start: block.rom_start,
                rom_end: block.rom_end,
            };
            let delay_slot_va = block.end_va;
            let last_word_control = block
                .rom_end
                .checked_sub(4)
                .and_then(|off| rom.bytes.get(off as usize..off as usize + 4))
                .map(|bytes| {
                    fn64_recomp_rs::decode(u32::from_be_bytes(bytes.try_into().unwrap()))
                        .has_delay_slot()
                })
                .unwrap_or(false);
            if last_word_control
                && word_class.get(&delay_slot_va) == Some(&WordClass::ProvenCode)
                && !admitted.contains(&delay_slot_va)
            {
                if let (Some(end_va), Some(rom_end)) =
                    (geom.end_va.checked_add(4), geom.rom_end.checked_add(4))
                {
                    geom.end_va = end_va;
                    geom.rom_end = rom_end;
                }
            }
            geom
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
            || block.rom_end.checked_sub(block.rom_start)
                != block.end_va.checked_sub(block.start_va)
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
    use crate::facts::RomAddressSpace;
    use fn64_recomp_rs::{BankId, CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget};
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

    fn reachable_block(
        start_va: u32,
        end_va: u32,
        terminator: BlockTerminator,
    ) -> ReachableCodeBlock {
        ReachableCodeBlock {
            bank: "boot".into(),
            start_va,
            end_va,
            owner_root: ENTRY_PC,
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_BASE + (start_va - ENTRY_PC),
            rom_end: ROM_BASE + (end_va - ENTRY_PC),
            terminator,
        }
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
        let geometry = complete_severed_delay_slots(&[&control], &word_class, &rom);
        assert_eq!(geometry[0].end_va, ENTRY_PC + 0x0c);
        assert_eq!(geometry[0].rom_end, ROM_BASE + 0x0c);
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
        let geometry = complete_severed_delay_slots(&[&control, &next], &word_class, &rom);
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
        let geometry = complete_severed_delay_slots(&[&control], &word_class, &rom);
        assert_eq!(geometry[0].end_va, ENTRY_PC + 0x0c);
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
                rom_space: crate::facts::RomAddressSpace::Physical,
                rom_start,
                rom_end: rom_start + 4,
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
        wrong_schema.schema_version = BLOCK_PACK_SCHEMA_V2 + 1;
        assert!(matches!(
            materialize_block_pack(&wrong_schema, &rom),
            Err(BlockPackError::UnsupportedSchema { .. })
        ));

        let mut malformed = pack;
        malformed.banks[0].blocks[0].end_va += 4;
        assert!(matches!(
            materialize_block_pack(&malformed, &rom),
            Err(BlockPackError::InvalidGeometry { .. })
        ));

        let (mut trailing_bytes, rom) = synthetic_pack();
        trailing_bytes.banks[0].blocks[0].rom_end += 1;
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
