//! Function-boundary grade for any decomp answer key's resident boot bank.
//!
//! Game-agnostic sibling of gate_b2's per-game boot grades: run Phase 4/5/6
//! (delay-slot-aware CFG + owner partitioning + bounded HI/LO resolution)
//! over the ROM's proven boot bank, seeded with ONLY the effective entry
//! (the proven boot bank's va_start — equal to the header entrypoint except
//! under a relocating IPL3 like Kirby 64's CIC-6103, where IPL3 jumps to
//! the relocated base),
//! then grade owner boundaries against the answer-key dump's boot-bank
//! functions. Same posture as gate_b2: `wrong == 0` is required; `open`
//! (found fewer than exist) is an honest, allowed gap.
//!
//! Answer-key selection requires AFFINE AGREEMENT with the boot copy: a
//! dump section belongs to the boot grade only when its `rom` lies inside
//! the boot copy's proven ROM extent AND its vram equals
//! `boot_va_start + (rom - boot_rom_start)` — the section genuinely runs
//! where the copy places it. Either half alone over-selects on real
//! corpora: VA containment sweeps in MM's separately-loaded `code`
//! segment, and ROM containment sweeps in Kirby 64's ovl1/ovl2, which are
//! *stored* inside the first MiB but relocated to their overlay slots at
//! runtime. The scan's code end comes from the answer key's own maximum
//! function end (a cited external bound, used to grade output only,
//! exactly like gate_b2's OoT code-end constant), clamped to the proven
//! boot extent so a malformed dump cannot push the scan past the image.
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
    vram: u32,
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
    // it (ROM containment + affine agreement) and where its code ends.
    let boot_va_end = boot_va_start + (boot_rom_end - boot_rom_start);
    let mut answer: Vec<AnswerFunction> = Vec::new();
    let mut code_end = boot_va_start;
    let mut skipped_nonaffine = 0usize;
    for section in &dump.sections {
        if section.rom < boot_rom_start || section.rom >= boot_rom_end {
            continue;
        }
        if section.vram != boot_va_start + (section.rom - boot_rom_start) {
            skipped_nonaffine += 1;
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
    code_end = code_end.min(boot_va_end);
    answer.sort_by_key(|function| function.va_start);
    assert!(
        !answer.is_empty(),
        "no answer-key sections affine with the boot copy"
    );
    if skipped_nonaffine != 0 {
        println!(
            "answer-key sections in boot ROM window but not affine with the \
             boot copy (stored-not-resident, e.g. packed overlays): {skipped_nonaffine}"
        );
    }

    let code_len = (code_end - boot_va_start) as usize;
    let bank_bytes =
        &rom.bytes[boot_rom_start as usize..boot_rom_start as usize + code_len];
    // The proven bank base IS the effective entry: IPL3 jumps to where it
    // copied the image (header entry adjusted by the identified variant's
    // relocation delta) — for 6102/6105-class IPL3s the two coincide.
    let entrypoint = boot_va_start;
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
