//! Phase-8 assembly-text round-trip over fn64's exact OoT boot owners.
//!
//! The ROM is the byte oracle. No dump, disassembly, or answer-key symbol is
//! loaded: owner geometry comes from `ProgramSnapshotV1`, instruction typing
//! comes from the shared decoder, and the emitted text is assembled/linked at
//! each owner's proven VA before `.text` bytes are compared with its proven
//! physical-ROM interval.

use fn64_discover::asm_emit::{emit_function, AsmWord};
use fn64_discover::banks;
use fn64_discover::owner_proof::{ExactFunctionOwner, OwnerAssessment};
use fn64_discover::snapshot::{compose_materialized_bank_v1, MaterializedBankInput};
use fn64_discover::{required_env_path, run_discovery, Fact, RomAddressSpace};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

// The existing OoT owner-proof gate bounds the materialized boot text at this
// repo-recorded load-image endpoint. It is not used as a function boundary:
// every attempted extent below must be carried by `ExactFunctionOwner`.
const OOT_BOOT_TEXT_END: u32 = 0x8000_6230;

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

fn main() {
    if let Err(error) = run() {
        eprintln!("gate_asm_roundtrip: FAILED: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    require_tool("mips-linux-gnu-as")?;
    require_tool("mips-linux-gnu-ld")?;
    require_tool("mips-linux-gnu-objcopy")?;

    let rom_path = required_env_path("FN64_DISCOVER_OOT_ROM", "an OoT NTSC 1.0 .z64")?;
    let rom_bytes = std::fs::read(&rom_path).map_err(|error| format!("reading ROM: {error}"))?;
    let (rom, facts) =
        run_discovery(&rom_bytes, None).map_err(|error| format!("discovering ROM: {error}"))?;
    let mapping = facts
        .proven_rom_mappings()
        .into_iter()
        .find(|fact| matches!(fact, Fact::RomMapping { bank, .. } if bank == banks::BOOT_BANK))
        .ok_or_else(|| "boot bank has no proven physical mapping".to_owned())?;
    let (rom_start, va_start) = match mapping {
        Fact::RomMapping {
            rom_start,
            va_start,
            ..
        } => (*rom_start, *va_start),
        _ => unreachable!(),
    };
    let code_len = OOT_BOOT_TEXT_END
        .checked_sub(va_start)
        .ok_or_else(|| "boot text end precedes proven mapping".to_owned())?
        as usize;
    let bank_bytes = rom
        .bytes
        .get(rom_start as usize..rom_start as usize + code_len)
        .ok_or_else(|| "boot text interval exceeds normalized ROM".to_owned())?;
    let snapshot = compose_materialized_bank_v1(
        &rom,
        &facts,
        MaterializedBankInput {
            bank: banks::BOOT_BANK,
            va_start,
            bytes: bank_bytes,
            seed_roots: std::slice::from_ref(&rom.header.entry_point),
        },
    )
    .map_err(|error| format!("composing proof snapshot: {error}"))?;
    let mut owners: Vec<ExactFunctionOwner> = snapshot.banks[0]
        .owner_proof
        .assessments
        .iter()
        .filter_map(|assessment| match assessment {
            OwnerAssessment::Proven { owner } => Some(owner.clone()),
            OwnerAssessment::Candidate { .. } | OwnerAssessment::Ambiguous { .. } => None,
        })
        .collect();
    owners.sort_by_key(|owner| owner.entry.pc);
    if owners.is_empty() {
        return Err("owner proof admitted zero exact functions".to_owned());
    }

    let temp = TempDir::create()?;
    let mut exact = 0usize;
    let mut differences = Vec::new();
    let mut digest = Sha256::new();

    for (index, owner) in owners.iter().enumerate() {
        let fn64_discover::facts::BankBackingSpanV1::RomAffine {
            rom_space: RomAddressSpace::Physical,
            rom_start,
            rom_end,
        } = &owner.backing
        else {
            differences.push((
                owner.entry.pc,
                Difference::Tool {
                    stage: "input",
                    detail: "exact owner is not physically ROM-backed".to_owned(),
                },
            ));
            continue;
        };
        let original = rom
            .bytes
            .get(*rom_start as usize..*rom_end as usize)
            .ok_or_else(|| {
                format!(
                    "owner {:#010x} ROM interval is out of bounds",
                    owner.entry.pc
                )
            })?;
        if original.len() != owner.byte_len() as usize || !original.len().is_multiple_of(4) {
            return Err(format!(
                "owner {:#010x} has inconsistent VA/ROM extents",
                owner.entry.pc
            ));
        }
        let words: Vec<AsmWord> = original
            .chunks_exact(4)
            .map(|bytes| AsmWord::decode(u32::from_be_bytes(bytes.try_into().unwrap())))
            .collect();
        let assembly = emit_function(owner, &words, &owners)
            .map_err(|error| format!("emitting owner {:#010x}: {error}", owner.entry.pc))?;
        digest.update(owner.entry.pc.to_be_bytes());
        digest.update(assembly.as_bytes());

        match assemble(&temp.path, index, owner.entry.pc, original.len(), &assembly) {
            Ok(assembled) if assembled == original => exact += 1,
            Ok(assembled) => differences.push((
                owner.entry.pc,
                first_difference(owner.entry.pc, original, &assembled),
            )),
            Err(difference) => differences.push((owner.entry.pc, difference)),
        }
    }

    println!("gate_asm_roundtrip: Phase-8 exact-owner assembly verification");
    println!("  rom_sha256={}", rom.sha256);
    println!(
        "  bank={} va=[{va_start:#010x},{OOT_BOOT_TEXT_END:#010x})",
        banks::BOOT_BANK
    );
    println!("  functions_attempted={}", owners.len());
    println!("  exact_byte_matches={exact}");
    println!("  differences={}", differences.len());
    for (entry, difference) in &differences {
        match difference {
            Difference::Bytes {
                pc,
                original,
                assembled,
                original_len,
                assembled_len,
            } => println!(
                "    entry={entry:#010x} first_diff_pc={pc:#010x} original={} assembled={} lengths={original_len}/{assembled_len}",
                optional_word(*original),
                optional_word(*assembled),
            ),
            Difference::Tool { stage, detail } => {
                println!("    entry={entry:#010x} {stage}_error={detail}")
            }
        }
    }
    println!("  assembly_text_sha256={:x}", digest.finalize());
    Ok(())
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
    let source = temp.join(format!("function_{index:03}.s"));
    let object = temp.join(format!("function_{index:03}.o"));
    let linked = temp.join(format!("function_{index:03}.elf"));
    let binary = temp.join(format!("function_{index:03}.bin"));
    let linker_script = temp.join(format!("function_{index:03}.ld"));
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
    // GNU ld aligns the output `.text` section and may append zero fill after
    // the function symbol. The proven owner extent selects the function bytes
    // from that section; a short section remains a real length mismatch.
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
            let path = base.join(format!("fn64-asm-roundtrip-{}-{nonce}", std::process::id()));
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
