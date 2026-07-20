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
//!   FN64_DISCOVER_SIG_DONOR_ROM / FN64_DISCOVER_SIG_DONOR_DUMP
//!                             optional donor pair for the signature lane
//!                             (see sig_scan): relocation-masked full-body
//!                             signatures from a DIFFERENT ROM's answer key
//!                             matched into this ROM's boot window; matches
//!                             seed unexcused roots, judged by wrong==0.
//!                             ';'-lists zipped pairwise for multiple donors
//!   FN64_DISCOVER_SIG_DONOR_TABLES / FN64_DISCOVER_SIG_DONOR_REQUEST_DMA
//!                             optional ';'-lists parallel to the donor
//!                             lists (empty segment = none): the donor's own
//!                             geometry/claims, so its compressed banks
//!                             materialize and contribute signatures
//!   FN64_DISCOVER_ADJUDICATED_ENTRIES
//!                             optional cited byte-adjudicated interior
//!                             entries (WIP answer keys); grading excuses
//!                             only, never discovery input
//!   FN64_DISCOVER_JUMP_TABLES optional cited jump-table claims for open
//!                             `jr` sites the bounded resolver cannot prove
//!                             (load-derived index): site pc + table VA +
//!                             entry count, cited at byte level; entries
//!                             are read from ROM bytes, validated in-window,
//!                             and pinned as CFG successors — never roots,
//!                             never an exhaustiveness proof

use fn64_discover::banks::{self, materialize_rom_range, LoadImageTableInput, StaticRequestDmaInput};
use fn64_discover::grade_oot_functions::{grade_functions, AnswerFunction, JalTargets};
use fn64_discover::partition::{partition, same_bank_overlaps};
use fn64_discover::resolve::build_cfg_closed_with_facts_and_claims;
use std::collections::BTreeMap;
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

/// Cited jump-table claims (FN64_DISCOVER_JUMP_TABLES): an open `jr`
/// site whose switch table location and bound were adjudicated at byte
/// level (the claim file cites the construction instructions). Targets
/// are read from ROM bytes at the cited table and become CFG successors
/// of the site — case labels, not function roots.
#[derive(Deserialize)]
struct JumpTablesFile {
    jump_table_claims: Vec<JumpTableClaim>,
}

#[derive(Deserialize)]
struct JumpTableClaim {
    name: String,
    site_va: u32,
    table_va: u32,
    entry_count: u32,
}

/// Byte-adjudicated interior entries (FN64_DISCOVER_ADJUDICATED_ENTRIES):
/// real function starts a WIP answer key lumps into a neighboring symbol,
/// each adjudicated at byte level and cited in the claims file. Grading
/// excuses only — never discovery input.
#[derive(Deserialize)]
struct AdjudicatedEntriesFile {
    adjudicated_entries: Vec<AdjudicatedEntry>,
}

#[derive(Deserialize)]
struct AdjudicatedEntry {
    #[allow(dead_code)]
    name: String,
    va: u32,
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
    // Two evidentiary classes of root. `excused` roots are machine-checked
    // callable entries (the entrypoint, statically proven entry-argument
    // pointers, cross-bank jal targets) and join the interior-entry excuse
    // set. `roots` additionally carries unexcused SEEDS (script-embedded
    // pointers, donor-signature matches): nothing machine-checks those, so
    // one that splits a real answer function must surface as `wrong`, not
    // be excused as a legitimate interior entry.
    let mut roots = vec![entrypoint];
    let mut excused = vec![entrypoint];
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
                            excused.push(va);
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
                excused.push(target);
                cross_bank += 1;
            }
        }
    }
    if cross_bank != 0 || cross_bank_unreadable != 0 {
        println!(
            "cross-bank jal roots added={cross_bank} unreadable banks={cross_bank_unreadable}"
        );
    }

    // Stored code-pointer harvest: a `lui rt, HI` followed shortly by
    // `addiu/ori rd, rt, LO` constructing a 4-aligned address inside the
    // boot code window is, in compiled code, almost always a function
    // pointer being materialized (callback fields, handler registration —
    // switch labels come from tables, not HI/LO pairs). Scan the boot
    // window plus every materializable proven bank; accept only targets at
    // a plausible function boundary. Seeds are unexcused: a constant that
    // points mid-function shows up as `wrong`, never as an excuse.
    {
        let mut images: Vec<Vec<u32>> = Vec::new();
        images.push(
            bank_bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect(),
        );
        for fact in db.proven_rom_mappings() {
            let Fact::RomMapping { bank, rom_space, rom_start, rom_end, .. } = fact else {
                continue;
            };
            if bank == banks::BOOT_BANK {
                continue;
            }
            if let Ok(image) = materialize_rom_range(&rom, &db, *rom_space, *rom_start, *rom_end) {
                images.push(
                    image
                        .bytes
                        .chunks_exact(4)
                        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                        .collect(),
                );
            }
        }
        let boot_words = &images[0];
        let mut stored_ptr = 0usize;
        let mut candidates: Vec<u32> = Vec::new();
        for words in &images {
            for (index, word) in words.iter().enumerate() {
                if word >> 26 != 0x0f {
                    continue; // not lui
                }
                let rt = (word >> 16) & 0x1f;
                let hi = (word & 0xffff) << 16;
                // Pair with the nearest following addiu/ori on the same
                // register before that register is redefined.
                for follow in words.iter().skip(index + 1).take(8) {
                    let opcode = follow >> 26;
                    let rs = (follow >> 21) & 0x1f;
                    let rd = (follow >> 16) & 0x1f;
                    if (opcode == 0x09 || opcode == 0x0d) && rs == rt {
                        let low = follow & 0xffff;
                        let target = if opcode == 0x09 {
                            hi.wrapping_add((low as i16) as i32 as u32)
                        } else {
                            hi | low
                        };
                        if target % 4 == 0
                            && target > boot_va_start
                            && target < code_end
                        {
                            candidates.push(target);
                        }
                        break;
                    }
                    if rd == rt && opcode != 0x00 {
                        break; // rt redefined by another I-type
                    }
                }
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
        for target in candidates {
            let offset = ((target - boot_va_start) / 4) as usize;
            if offset >= boot_words.len() {
                continue;
            }
            if !fn64_discover::sig_scan::plausible_function_boundary(boot_words, offset) {
                continue;
            }
            if !roots.contains(&target) {
                roots.push(target);
                stored_ptr += 1;
            }
        }
        println!("stored code-pointer roots added={stored_ptr}");
    }

    // Donor-signature lane (FN64_DISCOVER_SIG_DONOR_ROM/_DUMP, ';'-lists
    // zipped pairwise): masked full-body signatures from other ROMs' answer
    // keys, matched into this ROM's boot window. Recovers statically-dead
    // SDK code no CFG descent (and no dynamic trace) can reach. Donor
    // bodies are read straight at each dump section's cited ROM offset —
    // no donor discovery run. A donor section stored compressed in its ROM
    // yields garbage words that match nothing (a full 8+ masked-word
    // accidental match against real code does not happen in practice, and
    // wrong==0 stands judge regardless). Seeds are unexcused.
    if let (Ok(donor_roms), Ok(donor_dumps)) = (
        std::env::var("FN64_DISCOVER_SIG_DONOR_ROM"),
        std::env::var("FN64_DISCOVER_SIG_DONOR_DUMP"),
    ) {
        // ';'-separated: real ROM filenames contain commas.
        let rom_paths: Vec<&str> = donor_roms.split(';').collect();
        let dump_paths: Vec<&str> = donor_dumps.split(';').collect();
        assert_eq!(
            rom_paths.len(),
            dump_paths.len(),
            "SIG_DONOR_ROM and SIG_DONOR_DUMP must list the same number of donors"
        );
        let target_words: Vec<u32> = bank_bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        // Optional per-donor geometry/claims (';'-lists parallel to the
        // donor lists; an empty segment means none): with them the donor's
        // proven banks are materialized through its own proof chain (Yaz0
        // included), so bodies in compressed segments — where OoT/MM keep
        // most of libultra/libc — become readable signatures too.
        let donor_tables_env =
            std::env::var("FN64_DISCOVER_SIG_DONOR_TABLES").unwrap_or_default();
        let donor_request_env =
            std::env::var("FN64_DISCOVER_SIG_DONOR_REQUEST_DMA").unwrap_or_default();
        let donor_tables_paths: Vec<&str> = donor_tables_env.split(';').collect();
        let donor_request_paths: Vec<&str> = donor_request_env.split(';').collect();
        for (donor_index, (donor_rom_path, donor_dump_path)) in
            rom_paths.iter().zip(&dump_paths).enumerate()
        {
            let donor_rom_bytes = std::fs::read(donor_rom_path)
                .unwrap_or_else(|error| panic!("reading {donor_rom_path}: {error}"));
            let donor_dump_text = std::fs::read_to_string(donor_dump_path)
                .unwrap_or_else(|error| panic!("reading {donor_dump_path}: {error}"));
            let donor_dump: Dump =
                toml::from_str(&donor_dump_text).expect("parsing donor answer-key dump");
            let load_donor_claims = |paths: &[&str]| -> Option<String> {
                paths
                    .get(donor_index)
                    .filter(|p| !p.is_empty())
                    .map(|p| p.to_string())
            };
            let donor_tables: Vec<LoadImageTableInput> = load_donor_claims(&donor_tables_paths)
                .map(|path| {
                    let text = std::fs::read_to_string(&path)
                        .unwrap_or_else(|error| panic!("reading {path}: {error}"));
                    toml::from_str::<TablesFile>(&text)
                        .unwrap_or_else(|error| panic!("parsing {path}: {error}"))
                        .load_image_tables
                })
                .unwrap_or_default();
            let donor_request: Vec<StaticRequestDmaInput> =
                load_donor_claims(&donor_request_paths)
                    .map(|path| {
                        let text = std::fs::read_to_string(&path)
                            .unwrap_or_else(|error| panic!("reading {path}: {error}"));
                        toml::from_str::<RequestDmaFile>(&text)
                            .unwrap_or_else(|error| panic!("parsing {path}: {error}"))
                            .request_dma
                    })
                    .unwrap_or_default();
            let (donor_rom, donor_db, _) = run_discovery_with_tables_and_request_dma(
                &donor_rom_bytes,
                None,
                &donor_tables,
                &donor_request,
            )
            .expect("donor discovery");
            // Materialize every proven donor bank once; bodies are then
            // looked up by VA containment.
            let mut donor_banks: Vec<(u32, Vec<u8>)> = Vec::new();
            for fact in donor_db.proven_rom_mappings() {
                let Fact::RomMapping { rom_space, rom_start, rom_end, va_start, .. } = fact
                else {
                    continue;
                };
                if let Ok(image) =
                    materialize_rom_range(&donor_rom, &donor_db, *rom_space, *rom_start, *rom_end)
                {
                    donor_banks.push((*va_start, image.bytes));
                }
            }
            let mut donor_functions: Vec<(String, u32, u32)> = Vec::new();
            let mut donor_words: Vec<u32> = Vec::new();
            let push_body = |name: &str, size: u32, bytes: &[u8], words: &mut Vec<u32>,
                                 functions: &mut Vec<(String, u32, u32)>| {
                // Re-home each body at a synthetic VA: its own offset in
                // the accumulated donor word buffer.
                let synthetic_va = (words.len() * 4) as u32;
                words.extend(
                    bytes
                        .chunks_exact(4)
                        .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap())),
                );
                functions.push((name.to_string(), synthetic_va, size));
            };
            for section in &donor_dump.sections {
                for function in &section.functions {
                    // Preferred: VA containment in a materialized proven
                    // bank (correct for compressed storage).
                    let from_bank = donor_banks.iter().find_map(|(va_start, bytes)| {
                        let offset = function.vram.checked_sub(*va_start)? as usize;
                        bytes.get(offset..offset + function.size as usize)
                    });
                    if let Some(bytes) = from_bank {
                        push_body(
                            &function.name,
                            function.size,
                            bytes,
                            &mut donor_words,
                            &mut donor_functions,
                        );
                        continue;
                    }
                    // Fallback: the section's cited ROM offset, valid for
                    // sections stored uncompressed outside proven banks.
                    let Some(delta) = function.vram.checked_sub(section.vram) else {
                        continue;
                    };
                    let start = (section.rom + delta) as usize;
                    if let Some(bytes) =
                        donor_rom.bytes.get(start..start + function.size as usize)
                    {
                        push_body(
                            &function.name,
                            function.size,
                            bytes,
                            &mut donor_words,
                            &mut donor_functions,
                        );
                    }
                }
            }
            let signatures = fn64_discover::sig_scan::donor_signatures(
                &donor_functions,
                &donor_words,
                0,
                fn64_discover::sig_scan::MIN_SIGNATURE_WORDS,
            );
            let matches = fn64_discover::sig_scan::scan_signatures(
                &signatures,
                &target_words,
                boot_va_start,
            );
            let mut seeded = 0usize;
            for m in &matches {
                if !roots.contains(&m.va) {
                    roots.push(m.va);
                    seeded += 1;
                }
            }
            println!(
                "signature lane {donor_rom_path}: donor functions={} signatures={} \
                 matches={} roots added={seeded}",
                donor_functions.len(),
                signatures.len(),
                matches.len()
            );
        }
    }

    // Cited jump-table claims: read each table's entries from the boot
    // image at its cited VA, validate hard (a malformed claim dies loudly,
    // never degrades), and pin the edges into the CFG fixed point.
    let mut claimed_edges: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if let Some(file) = load_toml_env::<JumpTablesFile>("FN64_DISCOVER_JUMP_TABLES") {
        for claim in &file.jump_table_claims {
            let table_offset = claim
                .table_va
                .checked_sub(boot_va_start)
                .map(|delta| (boot_rom_start + delta) as usize)
                .unwrap_or_else(|| panic!("jump-table {}: table below boot VA", claim.name));
            let mut targets = Vec::new();
            for index in 0..claim.entry_count {
                let at = table_offset + (index as usize) * 4;
                let word = rom
                    .bytes
                    .get(at..at + 4)
                    .map(|b| u32::from_be_bytes(b.try_into().unwrap()))
                    .unwrap_or_else(|| panic!("jump-table {}: entry {index} outside ROM", claim.name));
                assert!(
                    word % 4 == 0 && word >= boot_va_start && word < code_end,
                    "jump-table {}: entry {index} = {word:#x} outside the scanned code window",
                    claim.name
                );
                targets.push(word);
            }
            targets.sort_unstable();
            targets.dedup();
            println!(
                "jump-table claim {}: site 0x{:x}, {} unique target(s)",
                claim.name,
                claim.site_va,
                targets.len()
            );
            claimed_edges.insert(claim.site_va, targets);
        }
    }

    let (cfg, resolved) = build_cfg_closed_with_facts_and_claims(
        &db,
        banks::BOOT_BANK,
        bank_bytes,
        boot_va_start,
        &roots,
        &claimed_edges,
    );
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
    // entries ONLY: direct jal targets, the entrypoint, statically proven
    // entry-argument pointers, and cross-bank jal targets. Unexcused seeds
    // (script-embedded pointers, donor-signature matches) are deliberately
    // left out — a bogus seed splitting a real answer function must count
    // as `wrong`, which is the adversarial posture the seed lanes claim.
    // (Previously ALL roots were excused here, silently weakening that
    // claim for script-ptr seeds.)
    let mut jal_targets: JalTargets = cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    jal_targets.extend(excused.iter().copied());
    if let Some(file) =
        load_toml_env::<AdjudicatedEntriesFile>("FN64_DISCOVER_ADJUDICATED_ENTRIES")
    {
        println!(
            "adjudicated interior entries excused: {}",
            file.adjudicated_entries.len()
        );
        jal_targets.extend(file.adjudicated_entries.iter().map(|entry| entry.va));
    }
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
    // FN64_DISCOVER_PRINT_OPEN=1: list the still-open answer functions —
    // the measured frontier that decides where the next recall lane goes.
    if std::env::var("FN64_DISCOVER_PRINT_OPEN").is_ok() {
        let open: Vec<&str> = report
            .per_function
            .iter()
            .filter(|(_, grade)| {
                matches!(grade, fn64_discover::grade_oot_functions::FunctionGrade::Open)
            })
            .map(|(name, _)| name.as_str())
            .collect();
        println!("open functions ({}): {}", open.len(), open.join(", "));
    }
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
