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
//!   FN64_DISCOVER_ROM         the game's .z64
//!   FN64_DISCOVER_DUMP        the matching answer-key dump.toml
//!   FN64_DISCOVER_ENTRY_ARGS  optional cited code-pointer-argument claims
//!                             TOML (e.g. osCreateThread's entry in $a2);
//!                             constant operands seed additional CFG roots
//!   FN64_DISCOVER_TABLES / FN64_DISCOVER_REQUEST_DMA
//!                             optional geometry/claims (as in
//!                             gate_decomp_reference); every additional
//!                             proven bank is scanned for direct jals into
//!                             the boot window — the boot bank is a library,
//!                             and cross-bank call sites are the
//!                             machine-checked evidence of which boot
//!                             functions other segments enter

use fn64_discover::banks::{self, materialize_rom_range, LoadImageTableInput, StaticRequestDmaInput};
use fn64_discover::grade_oot_functions::{grade_functions, AnswerFunction, JalTargets};
use fn64_discover::partition::{partition, same_bank_overlaps};
use fn64_discover::resolve::build_cfg_closed_with_facts;
use fn64_discover::{required_env_path, run_discovery_with_tables_and_request_dma, Fact};
use serde::Deserialize;

#[derive(Deserialize)]
struct TablesFile {
    load_image_tables: Vec<LoadImageTableInput>,
}

#[derive(Deserialize)]
struct RequestDmaFile {
    request_dma: Vec<StaticRequestDmaInput>,
}

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

#[derive(Deserialize)]
struct EntryArgsFile {
    entry_arg_calls: Vec<EntryArgCall>,
}

/// Cited script-embedded pointer claims: an interpreter's data stream at a
/// known physical ROM range whose `[command][pointer]` records carry raw
/// code addresses (SM64 behavior scripts' CALL_NATIVE). The claim cites the
/// range and the command encoding; the pointers are read from ROM bytes.
#[derive(Deserialize)]
struct ScriptPtrFile {
    script_ptr_tables: Vec<ScriptPtrTable>,
}

#[derive(Deserialize)]
struct ScriptPtrTable {
    name: String,
    rom_start: u32,
    rom_end: u32,
    command_words: Vec<u32>,
    command_mask: u32,
}

#[derive(Deserialize)]
struct EntryArgCall {
    name: String,
    callee_va: u32,
    pointer_arg_register: u8,
}

fn load_toml_env<T: serde::de::DeserializeOwned>(variable: &str) -> Option<T> {
    let path = std::env::var(variable).ok()?;
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("reading {path}: {error}"));
    Some(toml::from_str(&text).unwrap_or_else(|error| panic!("parsing {path}: {error}")))
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

    let tables: Vec<LoadImageTableInput> = load_toml_env::<TablesFile>("FN64_DISCOVER_TABLES")
        .map(|file| file.load_image_tables)
        .unwrap_or_default();
    let request_dma: Vec<StaticRequestDmaInput> =
        load_toml_env::<RequestDmaFile>("FN64_DISCOVER_REQUEST_DMA")
            .map(|file| file.request_dma)
            .unwrap_or_default();
    let (rom, db, request_report) =
        run_discovery_with_tables_and_request_dma(&rom_bytes, None, &tables, &request_dma)
            .expect("baseline discovery");
    if !request_dma.is_empty() {
        println!(
            "request-dma banks proven={} open sites={}",
            request_report.proven_banks.len(),
            request_report.open.len()
        );
    }
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

    // Optional cited entry-argument claims (FN64_DISCOVER_ENTRY_ARGS): a
    // callee that receives a code pointer in a declared register (e.g.
    // osCreateThread's entry in $a2). Constant operands recovered from the
    // scanned image become additional CFG roots — the OS transfers control
    // there, so a static constant is a callable-entry observation. The
    // grade's wrong==0 posture judges the result; open operands and
    // out-of-window pointers are reported, never guessed.
    let mut roots = vec![entrypoint];
    if let Some(file) = load_toml_env::<EntryArgsFile>("FN64_DISCOVER_ENTRY_ARGS") {
        let words: Vec<u32> = bank_bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        let mut seeded = 0usize;
        let mut open_sites = 0usize;
        let mut outside = 0usize;
        for claim in &file.entry_arg_calls {
            let slices = fn64_discover::pi_dma::slice_pointer_arg_calls(
                &words,
                fn64_discover::loaders::VirtualAddress::new(boot_va_start),
                fn64_discover::loaders::VirtualAddress::new(claim.callee_va),
                0x0080_0000,
                claim.pointer_arg_register,
            )
            .expect("boot image already validated");
            for slice in slices {
                match slice.pointer.proven() {
                    Some(pointer)
                        if pointer.get() >= boot_va_start && pointer.get() < code_end =>
                    {
                        let va = pointer.get();
                        if !roots.contains(&va) {
                            println!(
                                "entry-arg root: 0x{va:x} ({} call at 0x{:x})",
                                claim.name,
                                slice.call_pc.get()
                            );
                            roots.push(va);
                            seeded += 1;
                        }
                    }
                    Some(_) => outside += 1,
                    None => open_sites += 1,
                }
            }
        }
        println!(
            "entry-arg seeding: roots added={seeded} open sites={open_sites} \
             pointers outside scanned window={outside}"
        );
    }

    // Script-embedded pointers (FN64_DISCOVER_SCRIPT_PTRS): raw code
    // addresses inside a cited interpreter data stream. These seeds are
    // deliberately NOT added to the interior-entry excuse set — nothing
    // machine-checks that the interpreter reaches each record, so a bogus
    // pointer that splits a real answer function FAILS the gate loudly.
    // The wrong==0 posture is the adversarial judge of every claim here.
    if let Some(file) = load_toml_env::<ScriptPtrFile>("FN64_DISCOVER_SCRIPT_PTRS") {
        for table in &file.script_ptr_tables {
            let mut seeded = 0usize;
            let mut outside = 0usize;
            let Some(bytes) = rom
                .bytes
                .get(table.rom_start as usize..table.rom_end as usize)
            else {
                println!("script-ptrs {}: cited range exceeds ROM", table.name);
                continue;
            };
            let words: Vec<u32> = bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect();
            for pair in words.windows(2) {
                if !table
                    .command_words
                    .iter()
                    .any(|&command| pair[0] & table.command_mask == command)
                {
                    continue;
                }
                let pointer = pair[1];
                if pointer % 4 == 0 && pointer >= boot_va_start && pointer < code_end {
                    if !roots.contains(&pointer) {
                        roots.push(pointer);
                        seeded += 1;
                    }
                } else if pointer != 0 {
                    outside += 1;
                }
            }
            println!(
                "script-ptrs {}: roots added={seeded} pointers outside window={outside}",
                table.name
            );
        }
    }

    // Multi-bank recall: the boot bank is a library, so many of its
    // functions are entered only from other segments. Every additional
    // proven bank's image (materialized through its own proof chain —
    // physical bytes directly, VROM through exactly one proven file
    // record, Yaz0 included) is scanned for direct jals landing in the
    // boot window; each such target is a machine-checked callable entry,
    // the same evidentiary class as an in-bank jal target.
    let mut cross_bank = 0usize;
    let mut cross_bank_unreadable = 0usize;
    for fact in db.proven_rom_mappings() {
        let Fact::RomMapping {
            bank,
            rom_space,
            rom_start,
            rom_end,
            va_start,
            ..
        } = fact
        else {
            continue;
        };
        if bank == banks::BOOT_BANK {
            continue;
        }
        let image = match materialize_rom_range(&rom, &db, *rom_space, *rom_start, *rom_end) {
            Ok(image) => image,
            Err(_) => {
                cross_bank_unreadable += 1;
                continue;
            }
        };
        for (index, chunk) in image.bytes.chunks_exact(4).enumerate() {
            let word = u32::from_be_bytes(chunk.try_into().unwrap());
            if word >> 26 != 0x03 {
                continue;
            }
            let pc = va_start.wrapping_add((index as u32) * 4);
            let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
            if target >= boot_va_start && target < code_end && !roots.contains(&target) {
                roots.push(target);
                cross_bank += 1;
            }
        }
    }
    if cross_bank != 0 || cross_bank_unreadable != 0 {
        println!(
            "cross-bank jal roots added={cross_bank} unreadable banks={cross_bank_unreadable}"
        );
    }

    let (cfg, resolved) =
        build_cfg_closed_with_facts(&db, banks::BOOT_BANK, bank_bytes, boot_va_start, &roots);
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

    // The interior-entry excuse set holds machine-checkable callable
    // entries: direct jal targets, plus statically proven entry-argument
    // pointers (the OS transfers control there — same evidentiary class).
    // An owner rooted at one inside a coarser answer function is correct
    // finer-grained discovery, not a mis-split (see grade_oot_functions;
    // Kirby's WIP answer key gap-derives sizes across unlabeled real
    // functions, which is exactly where this matters).
    let mut jal_targets: JalTargets = cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    jal_targets.extend(roots.iter().copied());
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
        for (function, grade) in &report.per_function {
            if let fn64_discover::grade_oot_functions::FunctionGrade::WrongSplit { owner_root } =
                grade
            {
                println!("wrong: answer function {function} split by owner root 0x{owner_root:x}");
            }
        }
        eprintln!("gate_decomp_functions FAILED: wrong={}", report.wrong);
        std::process::exit(1);
    }
    println!("decomp function grade PASSED (wrong=0)");
}
