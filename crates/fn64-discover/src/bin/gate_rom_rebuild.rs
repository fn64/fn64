//! Phase-8 whole-ROM byte-exact rebuild over automatic discovery.
//!
//! `gate_asm_roundtrip` proves the assembly round-trip for exact owners in
//! one hand-bounded OoT bank. This gate composes the same proof into the
//! Phase-8 end artifact for ANY ROM: every proven bank is materialized, every
//! proven code region is emitted as GNU `as` text, assembled and linked at
//! its VA, and byte-compared against the bank's own bytes; verified
//! physically-backed regions are then written into a copy of the ROM whose
//! digest must equal the original's.
//!
//! Two region granularities carry two different claims:
//!
//! * **Exact owners** round-trip as functions (`emit_function`) — the
//!   decompile claim, attached only where Phase 5 proved a boundary.
//! * **Code runs** — maximal contiguous spans of CFG-proven blocks —
//!   round-trip with no ownership claim (`emit_code_region`). They prove the
//!   code *classification*, not boundaries.
//!
//! Every ROM byte lands in exactly one reported class; unclaimed bytes are
//! opaque, and stay original in the rebuild. A truncated or partial run is a
//! frontier, never silently a pass: the gate fails loud on any difference.
//!
//! The oracle is the byte. No answer key, dump, or symbol file is read.
//!
//! Environment:
//! * `FN64_DISCOVER_ROM` — the ROM to prove (required).
//! * `FN64_REBUILD_OUT` — optional path for the rebuilt image.
//! * `FN64_REBUILD_DUMP_ASM_DIR` — optional directory retaining every
//!   emitted `.s` so the mnemonic-vs-`.word` composition of a "pass" can be
//!   audited independently of this gate's own claims.
//!
//! Requires `mips-linux-gnu-{as,ld,objcopy}` (macOS: `brew install
//! mips-linux-gnu-binutils` or a cross-binutils build providing these
//! triple-prefixed tools on PATH).

use fn64_discover::asm_emit::{branch_target, emit_code_region, emit_function, AsmWord};
use fn64_discover::cfg::BlockTerminator;
use fn64_discover::facts::BankBackingV1;
use fn64_discover::owner_proof::{ExactFunctionOwner, OwnerAssessment};
use fn64_discover::snapshot::{
    compose_materialized_banks_validated_v2_with_limits, MultiBankCompositionLimits,
};
use fn64_discover::snapshot_inputs::{
    prepare_snapshot_banks_with_limits, PrepareSnapshotBanksLimits,
};
use fn64_discover::{required_env_path, RomAddressSpace};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MAX_ROM_BYTES: u64 = 128 * 1024 * 1024;
const HEADER_IPL3_END: u64 = 0x1000;

/// One emitted-and-assembled proven region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionKind {
    /// An exact Phase-5 owner: the function-boundary (decompile) claim.
    Function,
    /// A maximal contiguous span of proven CFG blocks: the code
    /// classification claim, no boundary asserted.
    Run,
}

#[derive(Debug)]
enum Difference {
    Bytes {
        pc: u32,
        original: Option<u32>,
        assembled: Option<u32>,
        original_len: usize,
        assembled_len: usize,
    },
    Tool {
        stage: &'static str,
        detail: String,
    },
}

/// A verified region's physical-ROM placement, retained for the rebuild.
struct VerifiedInterval {
    rom_start: usize,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct BankTally {
    functions_attempted: usize,
    functions_exact: usize,
    runs_attempted: usize,
    runs_exact: usize,
    run_bytes: u64,
    /// Words retained numerically (`.word`) rather than as mnemonics:
    /// out-of-region branches, non-canonical encodings, embedded table data
    /// inside proven blocks. Byte-exact either way; reported, never hidden.
    raw_words: u64,
    differences: Vec<(RegionKind, u32, Difference)>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_rom_rebuild: FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    require_tool("mips-linux-gnu-as")?;
    require_tool("mips-linux-gnu-ld")?;
    require_tool("mips-linux-gnu-objcopy")?;

    let rom_path = required_env_path("FN64_DISCOVER_ROM", "a .z64 ROM to rebuild")?;
    let metadata =
        std::fs::metadata(&rom_path).map_err(|error| format!("reading ROM metadata: {error}"))?;
    if metadata.len() > MAX_ROM_BYTES {
        return Err(format!(
            "ROM input is {} bytes, exceeding the {MAX_ROM_BYTES}-byte limit",
            metadata.len()
        ));
    }
    let rom_bytes = std::fs::read(&rom_path).map_err(|error| format!("reading ROM: {error}"))?;
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .map_err(|error| format!("automatic discovery rejected the ROM: {error:?}"))?;
    let prepared = prepare_snapshot_banks_with_limits(
        &discovery.rom,
        &discovery.facts,
        PrepareSnapshotBanksLimits::default(),
    )
    .map_err(|error| format!("preparing snapshot banks: {error}"))?;
    let inputs = prepared.materialized_inputs();
    let composed = compose_materialized_banks_validated_v2_with_limits(
        &discovery.rom,
        &discovery.facts,
        &inputs,
        MultiBankCompositionLimits::default(),
    )
    .map_err(|error| format!("composing snapshot banks: {error}"))?;

    let temp = TempDir::create()?;
    let mut assembly_digest = Sha256::new();
    let mut verified: Vec<VerifiedInterval> = Vec::new();
    let mut tallies: BTreeMap<String, BankTally> = BTreeMap::new();
    let mut physical_code = IntervalUnion::default();
    let mut materialized_code_bytes = 0u64;
    let mut physical_bank_bytes = IntervalUnion::default();
    let mut region_index = 0usize;

    for snapshot in composed.snapshots() {
        let [bank_snapshot] = snapshot.banks.as_slice() else {
            return Err("a composed snapshot did not contain exactly one bank".into());
        };
        let matching: Vec<_> = prepared
            .banks()
            .iter()
            .filter(|bank| {
                bank.bank == bank_snapshot.input.bank
                    && bank.va_start == bank_snapshot.input.va_start
                    && bank.va_end == bank_snapshot.input.va_end
            })
            .collect();
        let [bank] = matching.as_slice() else {
            return Err("a composed snapshot did not match exactly one prepared bank".into());
        };
        // Only a physically ROM-affine bank places bytes at cartridge
        // offsets. Virtual (VROM) affine spans and materialized outputs are
        // round-tripped against their own bytes but never mapped back onto
        // the ROM image: their source bytes stay original by construction.
        let physical_base = match &bank.backing {
            BankBackingV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start,
                rom_end,
            } => {
                physical_bank_bytes.insert(*rom_start as u64, *rom_end as u64);
                Some(*rom_start)
            }
            _ => None,
        };
        let tally = tallies.entry(bank.bank.clone()).or_default();

        let mut owners: Vec<ExactFunctionOwner> = bank_snapshot
            .owner_proof
            .assessments
            .iter()
            .filter_map(|assessment| match assessment {
                OwnerAssessment::Proven { owner } => Some(owner.clone()),
                OwnerAssessment::Candidate { .. } | OwnerAssessment::Ambiguous { .. } => None,
            })
            .collect();
        owners.sort_by_key(|owner| owner.entry.pc);

        for owner in &owners {
            tally.functions_attempted += 1;
            let Some(original) = bank_slice(bank, owner.entry.pc, owner.va_end) else {
                tally.differences.push((
                    RegionKind::Function,
                    owner.entry.pc,
                    Difference::Tool {
                        stage: "input",
                        detail: "owner extent leaves its bank's retained bytes".to_owned(),
                    },
                ));
                continue;
            };
            let words: Vec<AsmWord> = original
                .chunks_exact(4)
                .map(|bytes| AsmWord::decode(u32::from_be_bytes(bytes.try_into().unwrap())))
                .collect();
            let assembly = match emit_function(owner, &words, &owners) {
                Ok(assembly) => assembly,
                Err(error) => {
                    tally.differences.push((
                        RegionKind::Function,
                        owner.entry.pc,
                        Difference::Tool {
                            stage: "emit",
                            detail: error.to_string(),
                        },
                    ));
                    continue;
                }
            };
            assembly_digest.update(owner.entry.pc.to_be_bytes());
            assembly_digest.update(assembly.as_bytes());
            match assemble(
                &temp.path,
                region_index,
                owner.entry.pc,
                original.len(),
                &assembly,
            ) {
                Ok(assembled) if assembled == original => {
                    tally.functions_exact += 1;
                    record_verified(
                        &mut verified,
                        &mut physical_code,
                        &mut materialized_code_bytes,
                        physical_base,
                        bank.va_start,
                        owner.entry.pc,
                        assembled,
                        // Function extents lie inside runs; only runs feed
                        // the classification union to avoid double counting.
                        false,
                    );
                }
                Ok(assembled) => tally.differences.push((
                    RegionKind::Function,
                    owner.entry.pc,
                    first_difference(owner.entry.pc, original, &assembled),
                )),
                Err(difference) => {
                    tally
                        .differences
                        .push((RegionKind::Function, owner.entry.pc, difference));
                }
            }
            region_index += 1;
        }

        for (run_start, run_end) in proven_code_runs(bank_snapshot) {
            tally.runs_attempted += 1;
            let Some(original) = bank_slice(bank, run_start, run_end) else {
                tally.differences.push((
                    RegionKind::Run,
                    run_start,
                    Difference::Tool {
                        stage: "input",
                        detail: "code run leaves its bank's retained bytes".to_owned(),
                    },
                ));
                continue;
            };
            // A branch whose target leaves the run cannot assemble as a
            // mnemonic: GNU `as` resolves absolute branch operands against
            // section-relative addresses. Its word is retained numerically —
            // the byte is the claim, and the byte round-trips exactly.
            let mut words: Vec<AsmWord> = original
                .chunks_exact(4)
                .enumerate()
                .map(|(index, bytes)| {
                    let word = u32::from_be_bytes(bytes.try_into().unwrap());
                    let decoded = AsmWord::decode(word);
                    let pc = run_start + index as u32 * 4;
                    let AsmWord::Instruction {
                        decoded: instruction,
                        ..
                    } = decoded
                    else {
                        return decoded;
                    };
                    // GNU `as` refuses `jalr` with identical source and
                    // destination registers; a word carrying that encoding
                    // inside a proven block is table data, retained raw.
                    if let fn64_recomp_rs::Instruction::Jalr { rd, rs } = instruction {
                        if rd == rs {
                            return AsmWord::raw(word);
                        }
                    }
                    // FPU conditional moves are MIPS IV; `as -mips3` (the
                    // VR4300's ISA) cannot express them, so their words are
                    // retained numerically wherever they appear.
                    if matches!(
                        instruction,
                        fn64_recomp_rs::Instruction::MovzS { .. }
                            | fn64_recomp_rs::Instruction::MovnS { .. }
                            | fn64_recomp_rs::Instruction::MovzD { .. }
                            | fn64_recomp_rs::Instruction::MovnD { .. }
                            | fn64_recomp_rs::Instruction::MovcfS { .. }
                            | fn64_recomp_rs::Instruction::MovcfD { .. }
                    ) {
                        return AsmWord::raw(word);
                    }
                    // A branch-and-link whose source is the link register
                    // itself clobbers its own comparison operand; gas
                    // refuses the mnemonic outright.
                    if matches!(
                        instruction,
                        fn64_recomp_rs::Instruction::Bltzal { rs: 31, .. }
                            | fn64_recomp_rs::Instruction::Bgezal { rs: 31, .. }
                            | fn64_recomp_rs::Instruction::Bltzall { rs: 31, .. }
                            | fn64_recomp_rs::Instruction::Bgezall { rs: 31, .. }
                    ) {
                        return AsmWord::raw(word);
                    }
                    match branch_target(pc, instruction) {
                        Some(target) if target < run_start || target >= run_end => {
                            AsmWord::raw(word)
                        }
                        _ => decoded,
                    }
                })
                .collect();

            // Self-repair: a word whose emission is not encoding-faithful
            // (non-canonical fields, embedded table data inside a proven
            // block) reassembles to different bytes. The differing word is
            // demoted to a numeric literal and the region retried; the
            // demotion count is reported, never hidden. The budget bounds
            // pathological regions.
            let mut outcome = None;
            for _attempt in 0..64 {
                let assembly = match emit_code_region(&bank.bank, run_start, &words) {
                    Ok(assembly) => assembly,
                    Err(error) => {
                        outcome = Some(Err(Difference::Tool {
                            stage: "emit",
                            detail: error.to_string(),
                        }));
                        break;
                    }
                };
                match assemble(
                    &temp.path,
                    region_index,
                    run_start,
                    original.len(),
                    &assembly,
                ) {
                    Ok(assembled) if assembled == original => {
                        assembly_digest.update(run_start.to_be_bytes());
                        assembly_digest.update(assembly.as_bytes());
                        if let Ok(dir) = std::env::var("FN64_REBUILD_DUMP_ASM_DIR") {
                            let _ = std::fs::create_dir_all(&dir);
                            let _ = std::fs::write(
                                Path::new(&dir).join(format!("{}_{run_start:08x}.s", bank.bank)),
                                &assembly,
                            );
                        }
                        outcome = Some(Ok(assembled));
                        break;
                    }
                    Ok(assembled) => {
                        // Demote every unfaithfully-emitted word this round.
                        // A raw word emits as its own bytes, so a mismatch
                        // at an already-raw index is a real emitter defect.
                        let mut demoted = 0usize;
                        let mut defect = false;
                        for index in word_mismatches(original, &assembled) {
                            if matches!(words[index], AsmWord::Raw { .. }) {
                                defect = true;
                                break;
                            }
                            words[index] = AsmWord::raw(words[index].word());
                            demoted += 1;
                        }
                        if defect || demoted == 0 {
                            outcome = Some(Err(first_difference(run_start, original, &assembled)));
                            break;
                        }
                        tally.raw_words += demoted as u64;
                    }
                    Err(Difference::Tool { stage, detail })
                        if stage == "assemble" && detail.contains("branch address range") =>
                    {
                        // A relaxation the pre-pass missed (e.g. a COP
                        // branch form): demote every remaining symbolic
                        // branch mentioned nowhere and retry once via the
                        // generic path below is impossible to target — so
                        // demote the first still-symbolic branch word.
                        let Some(index) = words.iter().position(|word| {
                            matches!(
                                word,
                                AsmWord::Instruction { decoded, .. }
                                    if branch_target(0, *decoded).is_some()
                            )
                        }) else {
                            outcome = Some(Err(Difference::Tool { stage, detail }));
                            break;
                        };
                        words[index] = AsmWord::raw(words[index].word());
                        tally.raw_words += 1;
                    }
                    Err(difference) => {
                        outcome = Some(Err(difference));
                        break;
                    }
                }
            }
            match outcome {
                Some(Ok(assembled)) => {
                    tally.runs_exact += 1;
                    tally.run_bytes += assembled.len() as u64;
                    record_verified(
                        &mut verified,
                        &mut physical_code,
                        &mut materialized_code_bytes,
                        physical_base,
                        bank.va_start,
                        run_start,
                        assembled,
                        true,
                    );
                }
                Some(Err(difference)) => {
                    tally
                        .differences
                        .push((RegionKind::Run, run_start, difference));
                }
                None => {
                    tally.differences.push((
                        RegionKind::Run,
                        run_start,
                        Difference::Tool {
                            stage: "repair",
                            detail: "region did not converge within the demotion budget".to_owned(),
                        },
                    ));
                }
            }
            region_index += 1;
        }
    }

    // Rebuild: verified physically-backed bytes overwrite a copy of the
    // normalized ROM. Byte equality was proven per region, so the digest must
    // match; performing the writes keeps the claim mechanical rather than
    // inferred.
    let mut rebuilt = discovery.rom.bytes.to_vec();
    for interval in &verified {
        let end = interval.rom_start + interval.bytes.len();
        let Some(target) = rebuilt.get_mut(interval.rom_start..end) else {
            return Err(format!(
                "verified interval [{:#x},{end:#x}) exceeds the normalized ROM",
                interval.rom_start
            ));
        };
        target.copy_from_slice(&interval.bytes);
    }
    let rebuilt_sha256 = format!("{:x}", Sha256::digest(&rebuilt));
    let digest_match = rebuilt_sha256 == discovery.rom.sha256;
    if let Ok(out_path) = std::env::var("FN64_REBUILD_OUT") {
        std::fs::write(&out_path, &rebuilt)
            .map_err(|error| format!("writing rebuilt ROM to {out_path}: {error}"))?;
    }

    let rom_len = discovery.rom.bytes.len() as u64;
    let classes = classify_rom_bytes(rom_len, &mut physical_code)?;
    let bank_bytes = physical_bank_bytes.total();
    let total_differences: usize = tallies.values().map(|tally| tally.differences.len()).sum();

    println!("gate_rom_rebuild: Phase-8 whole-ROM byte-exact rebuild");
    println!("  rom_sha256={}", discovery.rom.sha256);
    println!("  internal_name={}", discovery.rom.header.name);
    println!("  banks={}", tallies.len());
    for (bank, tally) in &tallies {
        println!(
            "  bank={bank} functions={}/{} runs={}/{} run_bytes={} raw_words={} differences={}",
            tally.functions_exact,
            tally.functions_attempted,
            tally.runs_exact,
            tally.runs_attempted,
            tally.run_bytes,
            tally.raw_words,
            tally.differences.len(),
        );
        for (kind, entry, difference) in &tally.differences {
            let kind = match kind {
                RegionKind::Function => "function",
                RegionKind::Run => "run",
            };
            match difference {
                Difference::Bytes {
                    pc,
                    original,
                    assembled,
                    original_len,
                    assembled_len,
                } => println!(
                    "    {kind}={entry:#010x} first_diff_pc={pc:#010x} original={} assembled={} lengths={original_len}/{assembled_len}",
                    optional_word(*original),
                    optional_word(*assembled),
                ),
                Difference::Tool { stage, detail } => {
                    println!("    {kind}={entry:#010x} {stage}_error={detail}")
                }
            }
        }
    }
    println!("  rom_bytes={rom_len}");
    println!(
        "  header_ipl3_bytes={} ({:.2}%)",
        classes.header_ipl3,
        percent(classes.header_ipl3, rom_len)
    );
    println!(
        "  physical_bank_bytes={bank_bytes} ({:.2}%)",
        percent(bank_bytes, rom_len)
    );
    println!(
        "  roundtripped_code_bytes={} ({:.2}%)",
        classes.roundtripped_code,
        percent(classes.roundtripped_code, rom_len)
    );
    println!("  materialized_roundtripped_bytes={materialized_code_bytes}");
    println!(
        "  opaque_bytes={} ({:.2}%)",
        classes.opaque,
        percent(classes.opaque, rom_len)
    );
    println!("  differences={total_differences}");
    println!("  assembly_text_sha256={:x}", assembly_digest.finalize());
    println!("  rebuilt_sha256={rebuilt_sha256}");
    println!("  digest_match={digest_match}");

    if !digest_match {
        return Err("rebuilt ROM digest does not match the original".to_owned());
    }
    if total_differences != 0 {
        return Err(format!(
            "{total_differences} region(s) failed the byte round-trip"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RomByteClasses {
    header_ipl3: u64,
    roundtripped_code: u64,
    opaque: u64,
}

/// Partition the normalized ROM into the three Phase-8 physical-byte
/// classes. Header/IPL3 is a distinct non-code class, and the opaque class is
/// the checked complement of it and the accepted-code union. This makes it
/// impossible for an accepted code interval to be hidden by the opaque
/// remainder while still reporting a complete ROM-sized partition.
fn classify_rom_bytes(
    rom_len: u64,
    physical_code: &mut IntervalUnion,
) -> Result<RomByteClasses, String> {
    if physical_code
        .intervals
        .iter()
        .any(|&(start, end)| start >= end || end > rom_len)
    {
        return Err("a round-tripped code interval leaves the normalized ROM".to_owned());
    }

    let header_ipl3 = rom_len.min(HEADER_IPL3_END);
    if physical_code.overlaps(0, header_ipl3) {
        return Err("round-tripped code overlaps the header/IPL3 class".to_owned());
    }

    let roundtripped_code = physical_code.total();
    let classified = header_ipl3
        .checked_add(roundtripped_code)
        .ok_or_else(|| "physical-byte classification overflowed".to_owned())?;
    let opaque = rom_len
        .checked_sub(classified)
        .ok_or_else(|| "header/IPL3 and round-tripped code exceed the normalized ROM".to_owned())?;
    if header_ipl3 + roundtripped_code + opaque != rom_len {
        return Err("physical-byte classification does not cover the normalized ROM".to_owned());
    }

    Ok(RomByteClasses {
        header_ipl3,
        roundtripped_code,
        opaque,
    })
}

/// The bank's retained bytes for `[va_start, va_end)`, or `None` when the
/// span leaves the retained image.
fn bank_slice(
    bank: &fn64_discover::snapshot_inputs::PreparedSnapshotBank,
    va_start: u32,
    va_end: u32,
) -> Option<&[u8]> {
    let start = va_start.checked_sub(bank.va_start)? as usize;
    let end = va_end.checked_sub(bank.va_start)? as usize;
    if va_end < va_start || !va_start.is_multiple_of(4) || !(end - start).is_multiple_of(4) {
        return None;
    }
    bank.bytes.get(start..end)
}

/// Maximal contiguous spans of CFG blocks whose terminators prove every word
/// in the extent decoded as code. Blocks ending in `InvalidInstruction`,
/// `MissingDelaySlot`, `RanOffEnd`, or `DataFence` carry words that are not
/// proven code, so their extents are excluded wholesale — conservatively:
/// their decodable prefixes become opaque frontier, never a silent claim.
/// Adjacent and delay-slot-overlapping blocks merge into one span.
fn proven_code_runs(bank_snapshot: &fn64_discover::snapshot::BankSnapshotV1) -> Vec<(u32, u32)> {
    let mut spans: Vec<(u32, u32)> = bank_snapshot
        .closure
        .cfg
        .blocks
        .iter()
        .filter(|block| {
            !matches!(
                block.terminator,
                BlockTerminator::InvalidInstruction { .. }
                    | BlockTerminator::MissingDelaySlot { .. }
                    | BlockTerminator::RanOffEnd
                    | BlockTerminator::DataFence { .. }
            )
        })
        .map(|block| (block.start_va, block.end_va))
        .collect();
    spans.sort_unstable();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for (start, end) in spans {
        match runs.last_mut() {
            Some((_, run_end)) if start <= *run_end => *run_end = (*run_end).max(end),
            _ => runs.push((start, end)),
        }
    }
    runs
}

/// Retain one verified region for the rebuild and the coverage union.
#[allow(clippy::too_many_arguments)]
fn record_verified(
    verified: &mut Vec<VerifiedInterval>,
    physical_code: &mut IntervalUnion,
    materialized_code_bytes: &mut u64,
    physical_base: Option<u32>,
    bank_va_start: u32,
    region_va: u32,
    bytes: Vec<u8>,
    count_coverage: bool,
) {
    match physical_base {
        Some(rom_start) => {
            let offset = rom_start as u64 + (region_va - bank_va_start) as u64;
            if count_coverage {
                physical_code.insert(offset, offset + bytes.len() as u64);
            }
            verified.push(VerifiedInterval {
                rom_start: offset as usize,
                bytes,
            });
        }
        None if count_coverage => *materialized_code_bytes += bytes.len() as u64,
        None => {}
    }
}

/// A union of half-open `u64` intervals; overlaps merge.
#[derive(Default)]
struct IntervalUnion {
    intervals: Vec<(u64, u64)>,
}

impl IntervalUnion {
    fn insert(&mut self, start: u64, end: u64) {
        self.intervals.push((start, end));
    }

    fn total(&mut self) -> u64 {
        self.intervals.sort_unstable();
        let mut total = 0u64;
        let mut cursor = 0u64;
        for &(start, end) in &self.intervals {
            let start = start.max(cursor);
            if end > start {
                total += end - start;
                cursor = end;
            }
        }
        total
    }

    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.intervals
            .iter()
            .any(|&(other_start, other_end)| other_start < end && start < other_end)
    }
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn require_tool(tool: &str) -> Result<(), String> {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|_| ())
        .map_err(|error| format!("{tool} is required: {error}"))
}

fn assemble(
    temp: &Path,
    index: usize,
    entry_pc: u32,
    expected_len: usize,
    assembly: &str,
) -> Result<Vec<u8>, Difference> {
    let source = temp.join(format!("region_{index:05}.s"));
    let object = temp.join(format!("region_{index:05}.o"));
    let linked = temp.join(format!("region_{index:05}.elf"));
    let binary = temp.join(format!("region_{index:05}.bin"));
    let linker_script = temp.join(format!("region_{index:05}.ld"));
    std::fs::write(&source, assembly).map_err(|error| Difference::Tool {
        stage: "write",
        detail: error.to_string(),
    })?;
    std::fs::write(
        &linker_script,
        format!("SECTIONS {{ .text {entry_pc:#x} : SUBALIGN(4) {{ *(.text) }} }}\n"),
    )
    .map_err(|error| Difference::Tool {
        stage: "write",
        detail: error.to_string(),
    })?;

    check_output(
        "assemble",
        Command::new("mips-linux-gnu-as")
            .args(["-EB", "-mips3", "-32", "-G", "0", "-o"])
            .arg(&object)
            .arg(&source)
            .output(),
        temp,
    )?;
    check_output(
        "link",
        Command::new("mips-linux-gnu-ld")
            .args(["-EB", "-m", "elf32btsmip"])
            .arg("-T")
            .arg(&linker_script)
            .arg("-o")
            .arg(&linked)
            .arg(&object)
            .output(),
        temp,
    )?;
    check_output(
        "extract",
        Command::new("mips-linux-gnu-objcopy")
            .args(["-O", "binary", "-j", ".text"])
            .arg(&linked)
            .arg(&binary)
            .output(),
        temp,
    )?;
    let mut bytes = std::fs::read(&binary).map_err(|error| Difference::Tool {
        stage: "extract",
        detail: error.to_string(),
    })?;
    // GNU ld aligns the output `.text` section and may append zero fill. The
    // proven extent selects the region bytes; a short section remains a real
    // length mismatch.
    bytes.truncate(expected_len);
    Ok(bytes)
}

fn check_output(
    stage: &'static str,
    output: std::io::Result<Output>,
    temp: &Path,
) -> Result<(), Difference> {
    let output = output.map_err(|error| Difference::Tool {
        stage,
        detail: error.to_string(),
    })?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr)
        .trim()
        .replace(&temp.to_string_lossy().to_string(), "<tmp>")
        .replace('\n', " | ");
    Err(Difference::Tool { stage, detail })
}

fn first_difference(entry_pc: u32, original: &[u8], assembled: &[u8]) -> Difference {
    let byte = original
        .iter()
        .zip(assembled)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| original.len().min(assembled.len()));
    let word_offset = byte / 4 * 4;
    Difference::Bytes {
        pc: entry_pc + word_offset as u32,
        original: read_word(original, word_offset),
        assembled: read_word(assembled, word_offset),
        original_len: original.len(),
        assembled_len: assembled.len(),
    }
}

/// Word indices where `assembled` differs from `original`.
fn word_mismatches(original: &[u8], assembled: &[u8]) -> Vec<usize> {
    original
        .chunks_exact(4)
        .zip(assembled.chunks_exact(4))
        .enumerate()
        .filter_map(|(index, (left, right))| (left != right).then_some(index))
        .collect()
}

fn read_word(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
}

fn optional_word(word: Option<u32>) -> String {
    word.map_or_else(|| "<missing>".to_owned(), |word| format!("{word:#010x}"))
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn create() -> Result<Self, String> {
        let base = std::env::temp_dir();
        for nonce in 0..1000u32 {
            let path = base.join(format!("fn64-rom-rebuild-{}-{nonce}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("creating temporary directory: {error}")),
            }
        }
        Err("could not allocate a temporary directory".to_owned())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_byte_classes_are_disjoint_and_complete() {
        let mut code = IntervalUnion::default();
        code.insert(0x1000, 0x1100);
        code.insert(0x1080, 0x1200);

        let classes = classify_rom_bytes(0x4000, &mut code).unwrap();

        assert_eq!(classes.header_ipl3, 0x1000);
        assert_eq!(classes.roundtripped_code, 0x200);
        assert_eq!(classes.opaque, 0x2e00);
        assert_eq!(
            classes.header_ipl3 + classes.roundtripped_code + classes.opaque,
            0x4000
        );
    }

    #[test]
    fn physical_byte_classes_reject_code_hidden_under_header_ipl3() {
        let mut code = IntervalUnion::default();
        code.insert(0x0ffc, 0x1004);

        assert_eq!(
            classify_rom_bytes(0x4000, &mut code).unwrap_err(),
            "round-tripped code overlaps the header/IPL3 class"
        );
    }

    #[test]
    fn physical_byte_classes_reject_code_outside_the_rom() {
        let mut code = IntervalUnion::default();
        code.insert(0x3ffc, 0x4004);

        assert_eq!(
            classify_rom_bytes(0x4000, &mut code).unwrap_err(),
            "a round-tripped code interval leaves the normalized ROM"
        );
    }
}
