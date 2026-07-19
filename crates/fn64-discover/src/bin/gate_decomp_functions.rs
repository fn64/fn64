//! Function-boundary grade for any decomp answer key's resident boot bank.
//!
//! Game-agnostic sibling of gate_b2's per-game boot grades: run Phase 4/5/6
//! (delay-slot-aware CFG + owner partitioning + bounded HI/LO resolution)
//! over the ROM's proven boot bank, seeded with ONLY the header entrypoint,
//! then grade owner boundaries against the answer-key dump's boot-bank
//! functions. Same posture as gate_b2: `wrong == 0` is required; `open`
//! (found fewer than exist) is an honest, allowed gap.
//!
//! Answer-key selection is by ROM window, not VA window: a dump section
//! belongs to the boot grade only when its `rom` range lies inside the boot
//! copy's proven ROM extent. (VA selection would wrongly sweep in segments
//! that share the boot copy's VA window but are loaded separately — MM's
//! `code` segment does exactly that.) The scan's code end comes from the
//! answer key's own maximum function end — a cited external bound, used to
//! grade output only, exactly like gate_b2's OoT code-end constant.
//!
//! Env:
//!   FN64_DISCOVER_ROM    the game's .z64
//!   FN64_DISCOVER_DUMP   the matching answer-key dump.toml

use fn64_discover::banks;
use fn64_discover::grade_oot_functions::{grade_functions, AnswerFunction, JalTargets};
use fn64_discover::partition::{partition, same_bank_overlaps};
use fn64_discover::resolve::build_cfg_closed_with_facts;
use fn64_discover::{required_env_path, run_discovery, Fact};
use serde::Deserialize;

#[derive(Deserialize)]
struct Dump {
    #[serde(rename = "section")]
    sections: Vec<Section>,
}

#[derive(Deserialize)]
struct Section {
    rom: u32,
    #[serde(default)]
    functions: Vec<Function>,
}

#[derive(Deserialize)]
struct Function {
    name: String,
    vram: u32,
    size: u32,
}

fn main() {
    let rom_path =
        required_env_path("FN64_DISCOVER_ROM", "the game's .z64").unwrap_or_else(|error| {
            eprintln!("gate_decomp_functions: {error}");
            std::process::exit(1);
        });
    let dump_path = required_env_path("FN64_DISCOVER_DUMP", "the answer-key dump.toml")
        .unwrap_or_else(|error| {
            eprintln!("gate_decomp_functions: {error}");
            std::process::exit(1);
        });
    let rom_bytes =
        std::fs::read(&rom_path).unwrap_or_else(|error| panic!("reading {rom_path}: {error}"));
    let dump_text = std::fs::read_to_string(&dump_path).expect("reading answer-key dump");
    let dump: Dump = toml::from_str(&dump_text).expect("parsing answer-key dump");

    let (rom, db) = run_discovery(&rom_bytes, None).expect("ROM-only discovery");
    let boot = db
        .facts()
        .iter()
        .find_map(|fact| match fact {
            Fact::RomMapping {
                bank,
                rom_start,
                rom_end,
                va_start,
                ..
            } if bank == banks::BOOT_BANK => Some((*rom_start, *rom_end, *va_start)),
            _ => None,
        })
        .expect("boot bank not proven");
    let (boot_rom_start, boot_rom_end, boot_va_start) = boot;

    // Cited answer-key facts about the boot image: which functions live in
    // it (by ROM containment) and where its code region ends.
    let mut answer: Vec<AnswerFunction> = Vec::new();
    let mut code_end = boot_va_start;
    for section in &dump.sections {
        if section.rom < boot_rom_start || section.rom >= boot_rom_end {
            continue;
        }
        for function in &section.functions {
            code_end = code_end.max(function.vram.saturating_add(function.size));
            answer.push(AnswerFunction {
                name: function.name.clone(),
                va_start: function.vram,
            });
        }
    }
    answer.sort_by_key(|function| function.va_start);
    assert!(
        !answer.is_empty(),
        "no answer-key sections inside the boot copy ROM window"
    );

    let code_len = (code_end - boot_va_start) as usize;
    let bank_bytes =
        &rom.bytes[boot_rom_start as usize..boot_rom_start as usize + code_len];
    let entrypoint = rom.header.entry_point;
    println!(
        "boot bank: rom=0x{boot_rom_start:x}..0x{boot_rom_end:x} va_start=0x{boot_va_start:x} \
         answer_code_end=0x{code_end:x} entrypoint=0x{entrypoint:x} answer_functions={}",
        answer.len()
    );

    let (cfg, resolved) =
        build_cfg_closed_with_facts(&db, banks::BOOT_BANK, bank_bytes, boot_va_start, &[entrypoint]);
    let part = partition(&cfg);
    let overlaps = same_bank_overlaps(&part, &cfg);
    println!(
        "CFG: {} blocks, {} direct calls, {} indirect sites, {} bounded targets resolved",
        cfg.blocks.len(),
        cfg.direct_calls.len(),
        cfg.indirect_sites.len(),
        resolved.len()
    );
    println!(
        "Partition: {} owners, {} ambiguous, {} unowned, {} same-bank overlaps",
        part.owners.len(),
        part.ambiguous.len(),
        part.unowned.len(),
        overlaps.len()
    );
    assert!(
        overlaps.is_empty(),
        "same-bank owner overlaps must be zero, got {overlaps:?}"
    );

    let jal_targets: JalTargets = cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    let report = grade_functions(&part.owners, &answer, code_end, &jal_targets);
    println!(
        "function-boundary grade: total={} matched_exact={} matched_coarse={} \
         interior_entries={} open={} wrong={}",
        report.total,
        report.matched_exact,
        report.matched_coarse,
        report.interior_entries,
        report.open,
        report.wrong
    );
    println!(
        "  matched%={:.1} open%={:.1}",
        report.matched_fraction() * 100.0,
        (report.open as f64 / report.total as f64) * 100.0
    );
    if report.wrong != 0 {
        eprintln!("gate_decomp_functions FAILED: wrong={}", report.wrong);
        std::process::exit(1);
    }
    println!("decomp function grade PASSED (wrong=0)");
}
