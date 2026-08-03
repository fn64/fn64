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
//!                             TOML for callbacks not derived automatically;
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
//!                             ';'-lists zipped pairwise for multiple donors.
//!                             SAME-ENGINE donors are the strongest form:
//!                             the AKI titles donate to each other (NWXE
//!                             graded with NW4E as donor recovers +85, NW4E
//!                             with NWXE +72, wrong=0 both ways) because the
//!                             shared engine's functions match reference-
//!                             free — exactly the struct-callback residual
//!                             no static reachability evidence can touch.
//!                             The canonical per-game invocations live in
//!                             reference/corpus-invocations.md
//!   FN64_DISCOVER_SIG_DONOR_TABLES / FN64_DISCOVER_SIG_DONOR_REQUEST_DMA
//!                             optional ';'-lists parallel to the donor
//!                             lists (empty segment = none): the donor's own
//!                             geometry/claims, so its compressed banks
//!                             materialize and contribute signatures
//!   FN64_DISCOVER_ADJUDICATED_ENTRIES
//!                             optional cited byte-adjudicated interior
//!                             entries (WIP answer keys); grading excuses
//!                             only, never discovery input
//!   FN64_DISCOVER_OVL_RELOCS  set to 1 for Zelda-format overlay reloc
//!                             harvesting: every materializable proven bank
//!                             that parses as an overlay contributes its
//!                             typed R_MIPS_26 jal targets (machine-checked
//!                             callable entries, excused) and R_MIPS_32
//!                             data pointers (unexcused seeds) into the
//!                             boot window
//!   FN64_DISCOVER_GRADE_OVERLAYS
//!                             set to 1 to grade function boundaries in
//!                             every proven bank that parses as a Zelda
//!                             overlay, not just the boot bank: the reloc
//!                             section supplies typed in-bank jal targets
//!                             (excused roots) and typed data pointers
//!                             (unexcused seeds, e.g. actor init/update/
//!                             draw callbacks), the footer's text_size is
//!                             the mechanical code window, and the same
//!                             affine answer-selection rule picks each
//!                             bank's answer functions. wrong==0 across
//!                             ALL graded banks is required
//!   FN64_DISCOVER_TRACE       optional dynamic-trace JSONL (schema-bound
//!                             to this ROM's normalized sha256). Observed
//!                             indirect-transfer targets fold into the
//!                             fact database and, in the code-segment
//!                             grade, become excused owner roots — the
//!                             dynamic evidence for handler dispatch the
//!                             static lanes leave open
//!   FN64_DISCOVER_JUMP_TABLES optional cited jump-table claims for open
//!                             `jr` sites the bounded resolver cannot prove
//!                             (load-derived index): site pc + table VA +
//!                             entry count, cited at byte level; entries
//!                             are read from ROM bytes, validated in-window,
//!                             and pinned as CFG successors — never roots,
//!                             never an exhaustiveness proof
//!   FN64_DISCOVER_PRINT_ROOTS set to print every callable root added by
//!                             composed snapshot authority; the default
//!                             reports only the bank/root totals
//!   FN64_DISCOVER_PRINT_GRADES
//!                             emit one tab-separated answer-key row with its
//!                             exact grade for corpus-only adjacency studies;
//!                             never consumed by discovery

use fn64_discover::banks::{
    self, materialize_rom_range, LoadImageTableInput, StaticRequestDmaInput,
};
use fn64_discover::grade_oot_functions::{grade_functions, AnswerFunction, JalTargets};
use fn64_discover::partition::{
    partition, partition_with_authoritative_entries, same_bank_overlaps,
};
use fn64_discover::resolve::{build_cfg_closed_with_facts_and_claims, build_cfg_value_set_closed};
use fn64_discover::snapshot::{compose_materialized_banks_v1, MaterializedBankInput};
use std::collections::{BTreeMap, BTreeSet};

/// Shortest run of consecutive boundary-plausible in-window code pointers
/// that counts as a handler-dispatch table (see
/// `sig_scan::detect_handler_tables`). Short runs collide with incidental
/// pointer pairs in data; a real action/camera/cutscene table is longer.
const HANDLER_TABLE_MIN_RUN: usize = 4;

/// Distinct site addresses that must name a boot-window target before the
/// cross-bank jal lane converts it into an excused callable root.
///
/// One, deliberately, and the corroboration ladder was measured rather than
/// assumed. Requiring 2/3/4 sites costs recall monotonically and buys no
/// soundness: WM2000 786 -> 766 -> 737 -> 731 and No Mercy 931 -> 911 -> 884
/// -> 870 (donor configurations), with `wrong=0` at every rung including this
/// one. Whatever the sole-sited targets are, none of them splits a real answer
/// function, so charging 20-50 correct boundaries to exclude them is a bad
/// trade. `FN64_DISCOVER_CROSS_BANK_MIN_SITES` re-runs that ladder without a
/// rebuild; it exists to keep the measurement reproducible, not as a knob to
/// tune per game.
const CROSS_BANK_MIN_SITES: usize = 1;
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

struct OwnedMaterializedBank {
    bank: String,
    va_start: u32,
    bytes: Vec<u8>,
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

/// Back-scan the canonical IDO switch construction from an open `jr`
/// site: `lui $b, HI; ...; lw $t, LO($b); jr $t`. Returns the table VA
/// on the dominant pattern; `None` (site stays open) otherwise.
fn switch_table_base(text_words: &[u32], va_start: u32, site_pc: u32) -> Option<u32> {
    let site = ((site_pc.checked_sub(va_start)?) / 4) as usize;
    let jr = *text_words.get(site)?;
    if jr & 0xfc1f_ffff != 0x0000_0008 {
        return None; // not jr
    }
    let jr_reg = (jr >> 21) & 0x1f;
    let scan_from = site.saturating_sub(8);
    let (lw_index, lo, base_reg) = (scan_from..site).rev().find_map(|index| {
        let word = text_words[index];
        (word >> 26 == 0x23 && (word >> 16) & 0x1f == jr_reg).then_some((
            index,
            (word & 0xffff) as i16 as i32,
            (word >> 21) & 0x1f,
        ))
    })?;
    let hi = (scan_from..lw_index).rev().find_map(|index| {
        let word = text_words[index];
        (word >> 26 == 0x0f && (word >> 16) & 0x1f == base_reg).then_some(word & 0xffff)
    })?;
    Some(((hi << 16) as i64 + lo as i64) as u32)
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
    let (rom, mut db, request_report) =
        run_discovery_with_tables_and_request_dma(&rom_bytes, None, &tables, &request_dma)
            .expect("baseline discovery");
    if !request_dma.is_empty() {
        println!(
            "request-dma banks proven={} open sites={}",
            request_report.proven_banks.len(),
            request_report.open.len()
        );
    }

    // Key-free overlay recovery (unconditional; no per-game switch). The
    // descriptor-family search reads only the normalized ROM's own bytes and
    // proves an overlay mapping when delta_vote derives one destination for
    // the whole admitted table — the identical machinery gate_d1_overlays
    // grades held-out. A ROM with no recoverable table (measured: WCW/nWo
    // Revenge, zero candidate tables of any family) simply gains no mappings
    // and every lane below sees exactly the banks it saw before, so this is
    // safe to run everywhere rather than gated on a game the caller names.
    //
    // Its purpose here is the cross-bank jal lane, which iterates
    // `proven_rom_mappings` and was a no-op on every AKI title because the
    // gate composed the boot bank alone.
    let overlay_search = fn64_discover::overlay_regions::SearchConfig::aki_family();
    let overlay_recovery = fn64_discover::add_recovered_overlay_regions(
        &rom,
        &mut db,
        &fn64_discover::RecoveredOverlayInput {
            min_mapped_regions: overlay_search.min_records,
            search: overlay_search,
            delta_vote: fn64_discover::delta_vote::DeltaVoteConfig::default(),
            table_name: "recovered_overlay_descriptors".to_string(),
            bank_name: banks::BankNamePattern::new("recovered_overlay_", 0, ""),
        },
    );
    println!(
        "recovered overlays: candidate tables={} admitted={}",
        overlay_recovery.candidate_tables.len(),
        overlay_recovery
            .admissions
            .iter()
            .filter(|admission| admission.admitted)
            .count(),
    );

    // Optional dynamic trace (FN64_DISCOVER_TRACE): fold observed
    // indirect transfers into the fact database as ObservedIndirectTarget
    // existence evidence. An observed jr/jalr target is a machine-checked
    // callable entry — the target demonstrably executed from that site —
    // so the code-segment grade later excuses these targets as owner
    // roots (same evidentiary class as a jal target). This is the dynamic
    // evidence for load-derived/input-dependent dispatch (the MM
    // audio/camera handler tables) the static lanes leave open.
    if let Ok(trace_path) = std::env::var("FN64_DISCOVER_TRACE") {
        let file = std::fs::File::open(&trace_path)
            .unwrap_or_else(|error| panic!("opening trace {trace_path}: {error}"));
        let expected = fn64_discover::trace::NormalizedRomDigest::try_from(rom.sha256.clone())
            .expect("normalized ROM sha256 is a valid digest");
        let ingest = fn64_discover::trace::ingest_jsonl(std::io::BufReader::new(file), &expected)
            .unwrap_or_else(|error| panic!("ingesting trace {trace_path}: {error}"));
        let report = fn64_discover::trace::fold_indirect_targets_into_fact_db(
            &mut db,
            &ingest.header.trace_id,
            &ingest.facts,
            |_bank: &str, _va: u32| None,
        );
        println!(
            "trace {:?}: {} observed indirect edges folded ({} new, {} unknown-bank skipped)",
            ingest.header.trace_id,
            report.facts_added,
            report.new_edges.len(),
            report.unknown_bank_skipped,
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
    let bank_bytes = &rom.bytes[boot_rom_start as usize..boot_rom_start as usize + code_len];
    // The proven bank base IS the effective entry: IPL3 jumps to where it
    // copied the image (header entry adjusted by the identified variant's
    // relocation delta) — for 6102/6105-class IPL3s the two coincide.
    let entrypoint = boot_va_start;
    println!(
        "boot bank: rom=0x{boot_rom_start:x}..0x{boot_rom_end:x} va_start=0x{boot_va_start:x} \
         answer_code_end=0x{code_end:x} entrypoint=0x{entrypoint:x} answer_functions={}",
        answer.len()
    );

    // Snapshot composition first derives callable entries for mechanisms it
    // can identify semantically, currently resident osCreateThread calls.
    // Optional cited entry-argument claims cover callbacks not yet derived
    // automatically. Constant operands recovered from the scanned image become
    // additional CFG roots; open operands and out-of-window pointers are
    // reported, never guessed.
    // Two evidentiary classes of root. `excused` roots are machine-checked
    // callable entries (the entrypoint, statically proven entry-argument
    // pointers, cross-bank jal targets) and join the interior-entry excuse
    // set. `roots` additionally carries unexcused SEEDS (script-embedded
    // pointers, donor-signature matches): nothing machine-checks those, so
    // one that splits a real answer function must surface as `wrong`, not
    // be excused as a legitimate interior entry.
    let mut roots = vec![entrypoint];
    let mut excused = vec![entrypoint];
    let full_boot_bytes = &rom.bytes[boot_rom_start as usize..boot_rom_end as usize];
    let mut baseline_seed_roots = db.proven_function_entries(banks::BOOT_BANK);
    baseline_seed_roots.push(entrypoint);
    baseline_seed_roots.sort_unstable();
    baseline_seed_roots.dedup();
    let baseline_authority = build_cfg_value_set_closed(
        banks::BOOT_BANK,
        full_boot_bytes,
        boot_va_start,
        &baseline_seed_roots,
    );
    // Compose the resident bank together with only the load images proven by
    // the cited request-DMA scan. This is the smallest catalog that can carry
    // a semantic call chain out of the resident bank and back again; unrelated
    // table-derived overlays do not become authority merely because their
    // bytes are materializable.
    let mut composition_storage = vec![OwnedMaterializedBank {
        bank: banks::BOOT_BANK.to_owned(),
        va_start: boot_va_start,
        bytes: full_boot_bytes.to_vec(),
    }];
    for bank_name in &request_report.proven_banks {
        let mappings: Vec<_> = db
            .proven_rom_mappings()
            .into_iter()
            .filter(|fact| matches!(fact, Fact::RomMapping { bank, .. } if bank == bank_name))
            .collect();
        let [Fact::RomMapping {
            rom_space,
            rom_start,
            rom_end,
            va_start,
            ..
        }] = mappings.as_slice()
        else {
            panic!(
                "request-DMA bank {bank_name} must have exactly one proven ROM mapping, got {}",
                mappings.len()
            );
        };
        let image = materialize_rom_range(&rom, &db, *rom_space, *rom_start, *rom_end)
            .unwrap_or_else(|error| panic!("materializing request-DMA bank {bank_name}: {error}"));
        composition_storage.push(OwnedMaterializedBank {
            bank: bank_name.clone(),
            va_start: *va_start,
            bytes: image.bytes,
        });
    }
    let composition_inputs: Vec<_> = composition_storage
        .iter()
        .map(|bank| MaterializedBankInput {
            bank: &bank.bank,
            va_start: bank.va_start,
            bytes: &bank.bytes,
            seed_roots: &[],
        })
        .collect();
    let composed_authority = compose_materialized_banks_v1(&rom, &db, &composition_inputs)
        .expect("byte-verified multi-bank authority composition");
    let composed_boot = composed_authority
        .iter()
        .find(|snapshot| snapshot.banks[0].input.bank == banks::BOOT_BANK)
        .expect("composed snapshots retain the boot input");
    let baseline_roots: BTreeSet<u32> = baseline_authority.cfg.proven_roots.into_iter().collect();
    let mut authority_delta: Vec<u32> = composed_boot.banks[0]
        .closure
        .cfg
        .proven_roots
        .iter()
        .copied()
        .filter(|root| !baseline_roots.contains(root) && *root < code_end)
        .collect();
    authority_delta.sort_unstable();
    authority_delta.dedup();
    for root in &authority_delta {
        if std::env::var("FN64_DISCOVER_PRINT_ROOTS").is_ok() {
            println!("inductive callable root: 0x{root:x}");
        }
        roots.push(*root);
        excused.push(*root);
    }
    println!(
        "multi-bank inductive callable seeding: banks={} roots added={}",
        composition_inputs.len(),
        authority_delta.len(),
    );
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
                    Some(pointer) if pointer.get() >= boot_va_start && pointer.get() < code_end => {
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

    // In-bank flat jal sweep: the boot bank is a library whose engine
    // functions are frequently entered only through indirect dispatch
    // (GObj process/handler tables, callback fields). The recursive CFG
    // walk from the entrypoint never enters those callers, so a plain
    // `jal <leaf>` sitting in an unreached caller block is never followed
    // and the leaf stays open. Scanning the boot bank's own text flatly
    // for direct jals landing in the code window recovers exactly those
    // machine-checked callable entries — the same evidentiary class as a
    // cross-bank jal target. A bogus target that splits a real answer
    // function surfaces as `wrong`, which the wrong==0 posture judges.
    let mut in_bank_jal = 0usize;
    for (index, chunk) in bank_bytes.chunks_exact(4).enumerate() {
        let word = u32::from_be_bytes(chunk.try_into().unwrap());
        if word >> 26 != 0x03 {
            continue;
        }
        let pc = boot_va_start.wrapping_add((index as u32) * 4);
        let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
        if target >= boot_va_start && target < code_end && !roots.contains(&target) {
            roots.push(target);
            excused.push(target);
            in_bank_jal += 1;
        }
    }
    if in_bank_jal != 0 {
        println!("in-bank flat jal roots added={in_bank_jal}");
    }

    // Multi-bank recall: the boot bank is a library, so many of its
    // functions are entered only from other segments. Every additional
    // proven bank's image (materialized through its own proof chain —
    // physical bytes directly, VROM through exactly one proven file
    // record, Yaz0 included) is scanned for direct jals landing in the
    // boot window; each such target is a machine-checked callable entry,
    // the same evidentiary class as an in-bank jal target.
    //
    // Every recovered overlay image is materialized once here and kept, both
    // to sweep for jals and to answer the slot question below.
    let mut cross_bank = 0usize;
    let mut cross_bank_unreadable = 0usize;
    let mut slot_images: Vec<fn64_discover::overlay_slots::SlotImage> = Vec::new();
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
        slot_images.push(fn64_discover::overlay_slots::SlotImage {
            bank: bank.clone(),
            va_start: *va_start,
            bytes: image.bytes,
        });
    }
    let slot_catalog = fn64_discover::overlay_slots::SlotCatalog::new(slot_images);
    if !slot_catalog.is_empty() {
        println!(
            "recovered overlay images={} VA slots={} aliased slots={}",
            slot_catalog.images().len(),
            slot_catalog.slots().len(),
            slot_catalog.aliased_slot_count(),
        );
    }
    // A cross-bank jal edge is an ordered pair: the word that encodes the
    // call, and the boot-window address it names.
    //
    // The TARGET half carries no slot-aliasing hazard here and the catalog
    // says so: every target is required to lie in the boot code window, and
    // measured on all three AKI titles no recovered overlay slot overlaps it
    // (WM2000 boot code ends 0x8004_8FC0, slots at 0x800E_1B90/0x8011_C900;
    // No Mercy 0x8005_1708, slots at 0x800D_9960/0x8010_6760). The boot bank
    // is a single image, so a boot-window VA names one function. The
    // `Uncovered` check below is what enforces that rather than assuming it:
    // a target that any recovered overlay DOES cover is a VA two images
    // disagree about, and is left unconverted.
    //
    // The SITE half is where the measured hazard bites. A recovered image is
    // `.text` followed by `.data`/`.rodata`, and a data word aliases as `jal`
    // roughly one time in 64. The site's own bytes are never ambiguous — they
    // come from the materialized image, identified by bank and offset, not by
    // a VA — but whether they are an INSTRUCTION is exactly the open
    // question, and this gate has no key-free text extent for an AKI overlay
    // (unlike a Zelda overlay, whose footer states `text_size`).
    //
    // The discriminator that needs neither an answer key nor a text-size
    // guess is CORROBORATION: require the same target to be named from at
    // least `CROSS_BANK_MIN_SITES` distinct site addresses. A real engine
    // entry point is called from many places; a data word that happens to
    // encode `jal X` names an X nothing else names. Measured on No Mercy, the
    // 12-strong cluster of bogus "targets" 4 bytes apart inside one data
    // array is single-sited to a word, while genuine boot API entries carry
    // many callers. Targets short of the bar stay unconverted — the honest
    // outcome, and M2's problem rather than M1's.
    let mut target_sites: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut aliased_targets = 0usize;
    for image in slot_catalog.images() {
        for (index, chunk) in image.bytes.chunks_exact(4).enumerate() {
            let word = u32::from_be_bytes(chunk.try_into().unwrap());
            if word >> 26 != 0x03 {
                continue;
            }
            let pc = image.va_start.wrapping_add((index as u32) * 4);
            let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
            if target < boot_va_start || target >= code_end {
                continue;
            }
            // A boot-window target that a recovered overlay also covers is a
            // VA whose meaning depends on what is resident. Convert it only
            // when the aliases agree byte-identically there.
            let resolution = slot_catalog.resolve(target);
            if !matches!(
                resolution,
                fn64_discover::overlay_slots::SlotResolution::Uncovered
            ) && !resolution.admissible()
            {
                aliased_targets += 1;
                continue;
            }
            target_sites.entry(target).or_default().insert(pc);
        }
    }
    let min_sites = std::env::var("FN64_DISCOVER_CROSS_BANK_MIN_SITES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(CROSS_BANK_MIN_SITES);
    let mut uncorroborated = 0usize;
    for (target, sites) in &target_sites {
        if roots.contains(target) {
            continue;
        }
        if sites.len() < min_sites {
            uncorroborated += 1;
            continue;
        }
        roots.push(*target);
        excused.push(*target);
        cross_bank += 1;
    }
    if cross_bank != 0 || cross_bank_unreadable != 0 || !target_sites.is_empty() {
        println!(
            "cross-bank jal roots added={cross_bank} left ambiguous={uncorroborated} \
             (distinct targets seen={}, targets rejected by slot aliasing={aliased_targets}, \
             min sites={min_sites}) unreadable banks={cross_bank_unreadable}",
            target_sites.len(),
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
            let Fact::RomMapping {
                bank,
                rom_space,
                rom_start,
                rom_end,
                ..
            } = fact
            else {
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
        // Zelda overlay reloc harvest (FN64_DISCOVER_OVL_RELOCS=1): the
        // engine's own relocation tables type every reference. Typed jal
        // targets are the same evidentiary class as cross-bank jals
        // (excused); typed data pointers are unexcused seeds.
        if std::env::var("FN64_DISCOVER_OVL_RELOCS").is_ok() {
            let mut ovl_files = 0usize;
            let mut ovl_jal = 0usize;
            let mut ovl_ptr_candidates: Vec<u32> = Vec::new();
            for words in images.iter().skip(1) {
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_be_bytes()).collect();
                let Some(refs) = fn64_discover::overlay_reloc::parse_zelda_overlay(&bytes) else {
                    continue;
                };
                ovl_files += 1;
                for target in refs.jal_targets {
                    if target >= boot_va_start && target < code_end && !roots.contains(&target) {
                        roots.push(target);
                        excused.push(target);
                        ovl_jal += 1;
                    }
                }
                ovl_ptr_candidates.extend(refs.data_pointers);
            }
            let boot_words_ref = &images[0];
            let mut ovl_ptr = 0usize;
            ovl_ptr_candidates.sort_unstable();
            ovl_ptr_candidates.dedup();
            for target in ovl_ptr_candidates {
                if target % 4 != 0 || target <= boot_va_start || target >= code_end {
                    continue;
                }
                let offset = ((target - boot_va_start) / 4) as usize;
                if offset >= boot_words_ref.len()
                    || !fn64_discover::sig_scan::plausible_function_boundary(boot_words_ref, offset)
                {
                    continue;
                }
                if !roots.contains(&target) {
                    roots.push(target);
                    ovl_ptr += 1;
                }
            }
            println!(
                "overlay relocs: files parsed={ovl_files} jal roots added={ovl_jal} \
                 data-pointer roots added={ovl_ptr}"
            );
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
                        if target % 4 == 0 && target > boot_va_start && target < code_end {
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
            let Some(&first) = boot_words.get(offset) else {
                continue;
            };
            if !fn64_discover::sig_scan::plausible_function_boundary(boot_words, offset) {
                continue;
            }
            // A `lui/addiu`-constructed in-window address is a FUNCTION
            // pointer only if it points at a real function opener. The AKI
            // engines construct DATA-array bases the same way (`lui;addiu`
            // then `subu`/`sw`/`addu` index arithmetic), and those land in
            // an embedded read-only region — a hex-digit ASCII table
            // ("0123456789ABCDEF"), a zero-filled scratch run, then a small
            // pointer table. Every word in that region aliases as a
            // boundary-plausible address (a zero run trips the double-nop
            // padding rule), so per-opcode guards just chase the split root
            // a few words forward. Reject on the data tells a real IDO/KMC
            // prologue never has: first word is `nop`/zero (no function
            // opens with a bare nop — this subsumes the whole zero run),
            // all-printable-ASCII (string/number table), a coprocessor op
            // (0x10-0x13, e.g. mfc0), `addi` (0x08, trapping; compilers
            // emit `addiu`), or a reserved encoding the decoder rejects.
            // wrong==0 judges.
            let opcode = first >> 26;
            let is_ascii = first
                .to_be_bytes()
                .iter()
                .all(|&b| (0x20..=0x7e).contains(&b));
            if first == 0
                || is_ascii
                || (0x10..=0x13).contains(&opcode)
                || opcode == 0x08
                || matches!(
                    fn64_recomp_rs::decode(first),
                    fn64_recomp_rs::Instruction::Unknown { .. }
                )
            {
                continue;
            }
            if !roots.contains(&target) {
                roots.push(target);
                stored_ptr += 1;
            }
        }
        println!("stored code-pointer roots added={stored_ptr}");
    }

    // Handler-table lane: object-action / camera-mode / cutscene-shot
    // dispatch tables (`const Func table[]` arrays) stored in the bank's
    // own data. A dense run of consecutive in-window code pointers whose
    // every entry lands on a plausible function boundary IS such a table;
    // its entries are callable dispatch handlers the CFG never reaches
    // statically (the interpreter indexes the array at runtime). This is
    // the dominant SM64 open cluster (behavior/camera/cutscene handlers).
    // Entries are excused: a proven dense table pointing at real function
    // starts is machine-checked callable-entry evidence, the same class as
    // a jal target. The boundary-plausibility of *every* run entry is the
    // guard against float/fixed-point alias runs.
    {
        // The handler TABLES live in the bank's .data/.rodata, which sits
        // PAST the answer key's code_end — so scan the FULL bank image
        // [rom_start, rom_end), not just the [va_start, code_end) code
        // window (`bank_bytes`). Entries are still validated against the
        // code window via `text_words`.
        let full_bank = &rom.bytes[boot_rom_start as usize..boot_rom_end as usize];
        let bank_words: Vec<u32> = full_bank
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        let text_words: Vec<u32> = bank_bytes
            .chunks_exact(4)
            .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
            .collect();
        let handlers = fn64_discover::sig_scan::detect_handler_tables(
            &bank_words,
            &text_words,
            boot_va_start,
            code_end,
            HANDLER_TABLE_MIN_RUN,
        );
        let mut added = 0usize;
        for entry in handlers {
            if !roots.contains(&entry) {
                roots.push(entry);
                excused.push(entry);
                added += 1;
            }
        }
        println!("handler-table roots added={added}");
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
        let donor_tables_env = std::env::var("FN64_DISCOVER_SIG_DONOR_TABLES").unwrap_or_default();
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
            let donor_request: Vec<StaticRequestDmaInput> = load_donor_claims(&donor_request_paths)
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
                let Fact::RomMapping {
                    rom_space,
                    rom_start,
                    rom_end,
                    va_start,
                    ..
                } = fact
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
            let push_body = |name: &str,
                             size: u32,
                             bytes: &[u8],
                             words: &mut Vec<u32>,
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
                    if let Some(bytes) = donor_rom.bytes.get(start..start + function.size as usize)
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
            let matches =
                fn64_discover::sig_scan::scan_signatures(&signatures, &target_words, boot_va_start);
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
                    .unwrap_or_else(|| {
                        panic!("jump-table {}: entry {index} outside ROM", claim.name)
                    });
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
    // Machine-checked callable entries (direct jal targets + the excused set:
    // entrypoint, cross-bank jals, proven handler-table entries) are HARD
    // function boundaries — a proven-callable VA cannot be the interior of
    // another function. Passing them as authoritative entries fences each
    // enclosing closure at the boundary, so an adjacent proven leaf whose
    // block the previous function's fallthrough also reaches becomes its own
    // owner instead of a 2-claimant `ambiguous` block that grades `open`.
    // Only these excused entries re-carve geometry; unexcused seeds
    // (script/donor matches) stay soft and still surface as `wrong` if they
    // split a real function — the wrong==0 firewall is unchanged.
    let authoritative_entries: BTreeSet<u32> =
        cfg.direct_calls.iter().map(|(_, target)| *target).collect();
    let part = partition_with_authoritative_entries(&cfg, &authoritative_entries);
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
    if let Some(file) = load_toml_env::<AdjudicatedEntriesFile>("FN64_DISCOVER_ADJUDICATED_ENTRIES")
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
                matches!(
                    grade,
                    fn64_discover::grade_oot_functions::FunctionGrade::Open
                )
            })
            .map(|(name, _)| name.as_str())
            .collect();
        println!("open functions ({}): {}", open.len(), open.join(", "));
    }
    if std::env::var("FN64_DISCOVER_PRINT_GRADES").is_ok() {
        for (answer_function, (graded_name, grade)) in answer.iter().zip(&report.per_function) {
            assert_eq!(answer_function.name, *graded_name);
            println!(
                "adjacency-grade\t0x{:08x}\t{}\t{grade:?}",
                answer_function.va_start, graded_name
            );
        }
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

    // Recovered-overlay grading: the boot rule, applied per recovered bank.
    //
    // Until now an overlay function could not be graded at all — the gate
    // composed the boot bank alone, so every one of them was invisible rather
    // than open. Grading them is what makes the roadmap's remaining
    // mechanisms measurable.
    //
    // Each bank is its OWN universe, and that is what defuses slot aliasing
    // rather than working around it. A bank's answer functions are selected by
    // its ROM range plus affine agreement — the same two-part rule the boot
    // grade uses, and the same reason it needs both halves: ROM containment
    // alone would sweep in a neighbouring image's sections, VA agreement alone
    // would sweep in every image that shares the slot. Because selection is
    // keyed on ROM, two images at one VA never see each other's answers and no
    // function is graded twice. Roots come from the bank's own text (an
    // in-bank flat jal sweep, whose targets are same-image by construction);
    // cross-bank authority into an aliased slot is M2's problem and is
    // deliberately absent here.
    if !slot_catalog.is_empty() {
        // M2: inductive entry authority across composed banks.
        //
        // The in-bank sweep below roots each image from its own text, so an
        // overlay function called only from a SIBLING image stays unrooted --
        // overlay->overlay authority is circular with nothing to break in.
        // The boot bank breaks it: every boot-window root this gate already
        // proved is swept for jals into overlay windows, and those entries
        // seed a fixpoint that carries authority image to image.
        //
        // Bank-keyed throughout (see `inductive_bank_roots`), so two images
        // sharing a VA slot never merge root sets, and a target whose aliases
        // disagree stays unconverted rather than guessing which is resident.
        let mut boot_seeded: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        let mut boot_to_overlay = 0usize;
        for (index, chunk) in full_boot_bytes.chunks_exact(4).enumerate() {
            let word = u32::from_be_bytes(chunk.try_into().unwrap());
            if word >> 26 != 0x03 {
                continue;
            }
            let pc = boot_va_start.wrapping_add((index as u32) * 4);
            let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
            if target >= boot_va_start && target < code_end {
                continue;
            }
            match slot_catalog.resolve(target) {
                fn64_discover::overlay_slots::SlotResolution::Unique { bank } => {
                    if boot_seeded.entry(bank).or_default().insert(target) {
                        boot_to_overlay += 1;
                    }
                }
                fn64_discover::overlay_slots::SlotResolution::AgreedAcrossAliases { banks } => {
                    let mut any = false;
                    for bank in banks {
                        if boot_seeded.entry(bank).or_default().insert(target) {
                            any = true;
                        }
                    }
                    if any {
                        boot_to_overlay += 1;
                    }
                }
                _ => {}
            }
        }
        // A budget the AKI corpus clears in a handful of rounds; exhausting it
        // means the induction did not settle, and the gate keeps M1's
        // in-bank-only roots rather than shipping a half-converged set.
        const INDUCTIVE_MAX_ROUNDS: usize = 16;
        let inductive = slot_catalog.inductive_bank_roots(&boot_seeded, INDUCTIVE_MAX_ROUNDS);
        match &inductive {
            Some(fixpoint) => println!(
                "inductive overlay authority: boot->overlay seeds={boot_to_overlay} \
                 rounds={} targets rejected by slot aliasing={}",
                fixpoint.rounds, fixpoint.rejected_ambiguous,
            ),
            None => println!(
                "inductive overlay authority: DID NOT CONVERGE in \
                 {INDUCTIVE_MAX_ROUNDS} rounds -- falling back to in-bank roots"
            ),
        }
        let mut graded = 0usize;
        let mut skipped_no_answer = 0usize;
        let mut totals = (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut overlay_wrong: Vec<String> = Vec::new();
        for image in slot_catalog.images() {
            let Some((bank_rom_start, bank_rom_end)) = db
                .proven_rom_mappings()
                .into_iter()
                .find_map(|fact| match fact {
                    Fact::RomMapping {
                        bank,
                        rom_start,
                        rom_end,
                        ..
                    } if *bank == image.bank => Some((*rom_start, *rom_end)),
                    _ => None,
                })
            else {
                continue;
            };
            let bank_va = image.va_start;
            let mut bank_answer: Vec<AnswerFunction> = Vec::new();
            let mut bank_code_end = bank_va;
            for section in &dump.sections {
                if section.rom < bank_rom_start || section.rom >= bank_rom_end {
                    continue;
                }
                if section.vram != bank_va + (section.rom - bank_rom_start) {
                    continue;
                }
                for function in &section.functions {
                    bank_code_end = bank_code_end.max(function.vram.saturating_add(function.size));
                    bank_answer.push(AnswerFunction {
                        name: function.name.clone(),
                        va_start: function.vram,
                    });
                }
            }
            if bank_answer.is_empty() {
                skipped_no_answer += 1;
                continue;
            }
            bank_answer.sort_by_key(|function| function.va_start);
            bank_code_end = bank_code_end.min(bank_va + image.bytes.len() as u32);
            let text_len = (bank_code_end - bank_va) as usize;
            let text_bytes = &image.bytes[..text_len.min(image.bytes.len())];

            // In-bank flat jal sweep over this image's own text. Same
            // evidentiary class as the boot bank's: a direct call encoded in
            // proven bytes, landing in this image's own window.
            let mut bank_roots: Vec<u32> = Vec::new();
            let mut bank_excused: Vec<u32> = Vec::new();
            for (index, chunk) in text_bytes.chunks_exact(4).enumerate() {
                let word = u32::from_be_bytes(chunk.try_into().unwrap());
                if word >> 26 != 0x03 {
                    continue;
                }
                let pc = bank_va.wrapping_add((index as u32) * 4);
                let target = (pc & 0xf000_0000) | ((word & 0x03ff_ffff) << 2);
                if target >= bank_va && target < bank_code_end && !bank_roots.contains(&target) {
                    bank_roots.push(target);
                    bank_excused.push(target);
                }
            }
            // M2 roots for THIS bank: entries the induction proved callable
            // from the boot bank or a sibling image. Same evidentiary class as
            // the in-bank sweep above -- a direct call encoded in proven bytes,
            // resolved to this image by the slot catalog -- so they join the
            // excuse set alongside it.
            if let Some(fixpoint) = &inductive {
                if let Some(proven) = fixpoint.roots.get(&image.bank) {
                    for target in proven {
                        if *target >= bank_va
                            && *target < bank_code_end
                            && !bank_roots.contains(target)
                        {
                            bank_roots.push(*target);
                            bank_excused.push(*target);
                        }
                    }
                }
            }
            if bank_roots.is_empty() {
                skipped_no_answer += 1;
                continue;
            }
            let (bank_cfg, _) = build_cfg_closed_with_facts_and_claims(
                &db,
                &image.bank,
                text_bytes,
                bank_va,
                &bank_roots,
                &BTreeMap::new(),
            );
            let authoritative: BTreeSet<u32> = bank_cfg
                .direct_calls
                .iter()
                .map(|(_, target)| *target)
                .collect();
            let bank_part = partition_with_authoritative_entries(&bank_cfg, &authoritative);
            let bank_overlaps = same_bank_overlaps(&bank_part, &bank_cfg);
            assert!(
                bank_overlaps.is_empty(),
                "bank {}: same-bank owner overlaps must be zero, got {bank_overlaps:?}",
                image.bank
            );
            let mut bank_jal: JalTargets = authoritative.iter().copied().collect();
            bank_jal.extend(bank_excused.iter().copied());
            let bank_report =
                grade_functions(&bank_part.owners, &bank_answer, bank_code_end, &bank_jal);
            graded += 1;
            totals.0 += bank_report.total;
            totals.1 += bank_report.matched_exact;
            totals.2 += bank_report.matched_coarse + bank_report.interior_entries;
            totals.3 += bank_report.open;
            totals.4 += bank_report.wrong;
            println!(
                "  recovered bank {} @0x{bank_va:x} (rom 0x{bank_rom_start:x}..0x{bank_rom_end:x}): \
                 total={} matched_exact={} coarse+interior={} open={} wrong={}",
                image.bank,
                bank_report.total,
                bank_report.matched_exact,
                bank_report.matched_coarse + bank_report.interior_entries,
                bank_report.open,
                bank_report.wrong,
            );
            for (function, grade) in &bank_report.per_function {
                if let fn64_discover::grade_oot_functions::FunctionGrade::WrongSplit {
                    owner_root,
                } = grade
                {
                    overlay_wrong.push(format!(
                        "bank {}: {function} split by 0x{owner_root:x}",
                        image.bank
                    ));
                }
            }
        }
        println!(
            "recovered-overlay grade: banks={graded} (skipped without answers={skipped_no_answer}) \
             total={} matched_exact={} coarse+interior={} open={} wrong={}",
            totals.0, totals.1, totals.2, totals.3, totals.4
        );
        if totals.4 != 0 {
            for example in overlay_wrong.iter().take(10) {
                println!("wrong: {example}");
            }
            eprintln!(
                "gate_decomp_functions FAILED: recovered-overlay wrong={}",
                totals.4
            );
            std::process::exit(1);
        }
        if graded != 0 {
            println!("recovered-overlay function grade PASSED (wrong=0)");
        }
    }

    // Overlay grading (FN64_DISCOVER_GRADE_OVERLAYS=1): the boot rule,
    // generalized per proven bank. Only banks that parse as Zelda
    // overlays are graded — their reloc section supplies typed roots and
    // the mechanical text window; banks the answer key has no affine
    // functions for are skipped (nothing to grade against).
    if std::env::var("FN64_DISCOVER_GRADE_OVERLAYS").is_ok() {
        let mut graded_banks = 0usize;
        let mut skipped_unparsed = 0usize;
        let mut overlay_tables_paired = 0usize;
        // Cross-bank evidence accumulated during the overlay pass, and
        // the non-overlay banks (the statically-linked code segment) to
        // grade afterwards from that evidence.
        let mut typed_jal_all: Vec<u32> = Vec::new();
        let mut typed_ptr_all: Vec<u32> = Vec::new();
        let mut all_images: Vec<Vec<u8>> = vec![bank_bytes.to_vec()];
        let mut non_overlay_banks: Vec<(String, u32, Vec<u8>)> = Vec::new();
        let mut totals = (0usize, 0usize, 0usize, 0usize, 0usize); // total, exact, coarse+interior, open, wrong
        let mut wrong_examples: Vec<String> = Vec::new();
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
            let Ok(image) = materialize_rom_range(&rom, &db, *rom_space, *rom_start, *rom_end)
            else {
                continue;
            };
            let Some(refs) = fn64_discover::overlay_reloc::parse_zelda_overlay(&image.bytes) else {
                skipped_unparsed += 1;
                non_overlay_banks.push((bank.clone(), *va_start, image.bytes.clone()));
                all_images.push(image.bytes);
                continue;
            };
            typed_jal_all.extend(&refs.jal_targets);
            typed_ptr_all.extend(refs.data_pointers.iter().chain(&refs.hi_lo_pointers));
            all_images.push(image.bytes.clone());
            // Mechanical text window from the overlay's own footer.
            let text_size = {
                let len = image.bytes.len();
                let section_size =
                    u32::from_be_bytes(image.bytes[len - 4..].try_into().unwrap()) as usize;
                u32::from_be_bytes(
                    image.bytes[len - section_size..len - section_size + 4]
                        .try_into()
                        .unwrap(),
                ) as usize
            };
            let bank_va_start = *va_start;
            let text_end = bank_va_start + text_size as u32;

            // Affine answer selection, per bank.
            let mut bank_answer: Vec<AnswerFunction> = Vec::new();
            for section in &dump.sections {
                if section.rom < *rom_start || section.rom >= *rom_end {
                    continue;
                }
                if section.vram != bank_va_start + (section.rom - rom_start) {
                    continue;
                }
                for function in &section.functions {
                    if function.vram < text_end {
                        bank_answer.push(AnswerFunction {
                            name: function.name.clone(),
                            va_start: function.vram,
                        });
                    }
                }
            }
            if bank_answer.is_empty() {
                continue;
            }
            bank_answer.sort_by_key(|function| function.va_start);

            let text_bytes = &image.bytes[..text_size.min(image.bytes.len())];
            let text_words: Vec<u32> = text_bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect();
            let mut bank_roots: Vec<u32> = Vec::new();
            let mut bank_excused: Vec<u32> = Vec::new();
            for &target in &refs.jal_targets {
                if target >= bank_va_start && target < text_end && !bank_roots.contains(&target) {
                    bank_roots.push(target);
                    bank_excused.push(target);
                }
            }
            for &pointer in refs.data_pointers.iter().chain(&refs.hi_lo_pointers) {
                if pointer % 4 != 0 || pointer < bank_va_start || pointer >= text_end {
                    continue;
                }
                let offset = ((pointer - bank_va_start) / 4) as usize;
                // A nop word is never a function start (same rule as
                // donor signatures): constructed constants can point at
                // trailing padding — measured as a split root in
                // BgHidanHamstep_Draw's final padding on OoT.
                if offset < text_words.len()
                    && text_words[offset] != 0
                    && fn64_discover::sig_scan::plausible_function_boundary(&text_words, offset)
                    && !bank_roots.contains(&pointer)
                {
                    bank_roots.push(pointer);
                }
            }
            if bank_roots.is_empty() {
                continue;
            }
            let (first_pass, _) = build_cfg_closed_with_facts_and_claims(
                &db,
                bank,
                text_bytes,
                bank_va_start,
                &bank_roots,
                &BTreeMap::new(),
            );
            // Pair each open `jr` site with the typed rodata run at its
            // back-scanned table base. The reloc typing bounds the table
            // where value-set analysis could not: the run of consecutive
            // typed words at the base IS the entry list, and it ends
            // where the typing ends. Every entry must be a 4-aligned
            // text VA or the site stays open.
            let mut bank_claims: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
            // Sorted first-pass roots bound each site's own function: a
            // switch's case labels are LOCAL (between the site's owning
            // root and the next discovered root). A typed rodata run
            // whose entries land outside that window is a function-
            // POINTER array (jr tail dispatch), not a jump table —
            // injecting one as intra-owner successors swallowed
            // neighboring owners and tripped the overlap assert on MM's
            // actor_overlay_164.
            let mut sorted_roots: Vec<u32> = first_pass.proven_roots.clone();
            sorted_roots.extend(bank_roots.iter().copied());
            sorted_roots.sort_unstable();
            sorted_roots.dedup();
            for site in &first_pass.indirect_sites {
                if site.via_call {
                    continue;
                }
                let owner_root = match sorted_roots.partition_point(|root| *root <= site.pc) {
                    0 => continue,
                    index => sorted_roots[index - 1],
                };
                let next_root = sorted_roots
                    .get(sorted_roots.partition_point(|root| *root <= site.pc))
                    .copied()
                    .unwrap_or(text_end);
                let Some(base) = switch_table_base(&text_words, bank_va_start, site.pc) else {
                    continue;
                };
                let Some(base_offset) = base.checked_sub(bank_va_start) else {
                    continue;
                };
                let start = refs
                    .rodata_words
                    .partition_point(|(offset, _)| *offset < base_offset);
                if refs.rodata_words.get(start).map(|(offset, _)| *offset) != Some(base_offset) {
                    continue;
                }
                let mut targets: Vec<u32> = Vec::new();
                let mut expected = base_offset;
                for (offset, value) in &refs.rodata_words[start..] {
                    if *offset != expected
                        || *value % 4 != 0
                        || *value < bank_va_start
                        || *value >= text_end
                    {
                        break;
                    }
                    targets.push(*value);
                    expected += 4;
                }
                if targets.len() < 2 {
                    continue;
                }
                if targets
                    .iter()
                    .any(|target| *target < owner_root || *target >= next_root)
                {
                    continue; // function-pointer array, not case labels
                }
                targets.sort_unstable();
                targets.dedup();
                bank_claims.insert(site.pc, targets);
            }
            let bank_cfg = if bank_claims.is_empty() {
                first_pass
            } else {
                overlay_tables_paired += bank_claims.len();
                build_cfg_closed_with_facts_and_claims(
                    &db,
                    bank,
                    text_bytes,
                    bank_va_start,
                    &bank_roots,
                    &bank_claims,
                )
                .0
            };
            let bank_part = partition(&bank_cfg);
            let bank_overlaps = same_bank_overlaps(&bank_part, &bank_cfg);
            assert!(
                bank_overlaps.is_empty(),
                "bank {bank}: same-bank owner overlaps must be zero"
            );
            let mut bank_jal: JalTargets = bank_cfg
                .direct_calls
                .iter()
                .map(|(_, target)| *target)
                .collect();
            bank_jal.extend(bank_excused.iter().copied());
            let bank_report = grade_functions(&bank_part.owners, &bank_answer, text_end, &bank_jal);
            graded_banks += 1;
            totals.0 += bank_report.total;
            totals.1 += bank_report.matched_exact;
            totals.2 += bank_report.matched_coarse + bank_report.interior_entries;
            totals.3 += bank_report.open;
            totals.4 += bank_report.wrong;
            if bank_report.wrong != 0 {
                for (function, grade) in &bank_report.per_function {
                    if let fn64_discover::grade_oot_functions::FunctionGrade::WrongSplit {
                        owner_root,
                    } = grade
                    {
                        wrong_examples
                            .push(format!("bank {bank}: {function} split by 0x{owner_root:x}"));
                    }
                }
            }
        }
        println!(
            "overlay grade: banks={graded_banks} (skipped non-overlay={skipped_unparsed}) \
             jump-tables paired={overlay_tables_paired} \
             total={} matched_exact={} coarse+interior={} open={} wrong={}",
            totals.0, totals.1, totals.2, totals.3, totals.4
        );
        if totals.0 != 0 {
            println!(
                "  matched%={:.1} open%={:.1}",
                (totals.1 as f64 / totals.0 as f64) * 100.0,
                (totals.3 as f64 / totals.0 as f64) * 100.0
            );
        }
        if totals.4 != 0 {
            for example in wrong_examples.iter().take(10) {
                println!("wrong: {example}");
            }
            eprintln!("gate_decomp_functions FAILED: overlay wrong={}", totals.4);
            std::process::exit(1);
        }
        println!("overlay function grade PASSED (wrong=0)");

        // Non-overlay bank grading (the statically-linked code segment).
        // No reloc section here, so the roots come from elsewhere: typed
        // overlay references INTO the bank (the overlays' reloc tables
        // name the code-segment API they call), a blind jal sweep across
        // every materialized bank, a pointer sweep of the bank's own data
        // region (actor tables, scene-command handlers, gamestate tables),
        // and — when a trace was folded — observed indirect-transfer
        // targets (the handler-dispatch entries no static signal reaches).
        typed_jal_all.sort_unstable();
        typed_jal_all.dedup();
        typed_ptr_all.sort_unstable();
        typed_ptr_all.dedup();
        // Observed indirect targets from any folded trace: each is a
        // machine-checked callable entry (it ran from an observed site),
        // so it joins the excused root set — same evidentiary class as a
        // jal target, never a guess.
        let observed_targets: Vec<fn64_discover::facts::BankAddr> = db
            .facts()
            .iter()
            .filter_map(|fact| match fact {
                Fact::ObservedIndirectTarget { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect();
        let mut code_totals = (0usize, 0usize, 0usize, 0usize, 0usize);
        let mut code_wrong: Vec<String> = Vec::new();
        for (bank, bank_va, image) in &non_overlay_banks {
            let bank_va = *bank_va;
            // Affine answer selection against this bank's image extent.
            let (bank_rom_start, bank_rom_end) = match db
                .proven_rom_mappings()
                .into_iter()
                .find_map(|fact| match fact {
                    Fact::RomMapping {
                        bank: b,
                        rom_start,
                        rom_end,
                        ..
                    } if b == bank => Some((*rom_start, *rom_end)),
                    _ => None,
                }) {
                Some(range) => range,
                None => continue,
            };
            let mut bank_answer: Vec<AnswerFunction> = Vec::new();
            let mut bank_code_end = bank_va;
            for section in &dump.sections {
                if section.rom < bank_rom_start || section.rom >= bank_rom_end {
                    continue;
                }
                if section.vram != bank_va + (section.rom - bank_rom_start) {
                    continue;
                }
                for function in &section.functions {
                    bank_code_end = bank_code_end.max(function.vram.saturating_add(function.size));
                    bank_answer.push(AnswerFunction {
                        name: function.name.clone(),
                        va_start: function.vram,
                    });
                }
            }
            let bank_va_end = bank_va + image.len() as u32;
            bank_code_end = bank_code_end.min(bank_va_end);
            if bank_answer.is_empty() {
                continue;
            }
            bank_answer.sort_by_key(|function| function.va_start);
            let window = |target: u32| target >= bank_va && target < bank_code_end;

            let text_len = (bank_code_end - bank_va) as usize;
            let text_bytes = &image[..text_len.min(image.len())];
            let text_words: Vec<u32> = text_bytes
                .chunks_exact(4)
                .map(|chunk| u32::from_be_bytes(chunk.try_into().unwrap()))
                .collect();
            let mut bank_roots: Vec<u32> = Vec::new();
            let mut bank_excused: Vec<u32> = Vec::new();
            // Blind jal sweep across every materialized bank (machine-
            // checked callable entries — same class as cross-bank jals).
            for scan in &all_images {
                for chunk in scan.chunks_exact(4) {
                    let word = u32::from_be_bytes(chunk.try_into().unwrap());
                    if word >> 26 != 0x03 {
                        continue;
                    }
                    let target = 0x8000_0000 | ((word & 0x03ff_ffff) << 2);
                    if window(target) && !bank_roots.contains(&target) {
                        bank_roots.push(target);
                        bank_excused.push(target);
                    }
                }
            }
            // Observed indirect-transfer targets into this bank: a jr/jalr
            // demonstrably reached them at runtime, so they are excused
            // callable entries — this is the trace lane recovering the
            // handler-dispatch functions no static signal can reach.
            for observed in &observed_targets {
                if observed.bank == *bank
                    && window(observed.pc)
                    && !bank_roots.contains(&observed.pc)
                {
                    bank_roots.push(observed.pc);
                    bank_excused.push(observed.pc);
                }
            }
            // Typed overlay pointers into this bank, plus this bank's own
            // data region swept for absolute in-window pointers. Both
            // unexcused, boundary-guarded, never at a nop.
            // Typed overlay pointers keep the boundary guard alone; the
            // BLIND own-data sweep additionally requires the target to
            // open with a stack prologue (`addiu $sp,$sp,-N`) — raw data
            // words (floats, fixed-point) alias as plausible 0x80xxxxxx
            // addresses, and boundary shape alone let 4 such aliases
            // split real MM code functions. Leaf functions this misses
            // are jal-reachable through the blind jal sweep instead.
            let mut pointer_seeds: Vec<(u32, bool)> =
                typed_ptr_all.iter().map(|p| (*p, false)).collect();
            for chunk in image[text_len.min(image.len())..].chunks_exact(4) {
                pointer_seeds.push((u32::from_be_bytes(chunk.try_into().unwrap()), true));
            }
            pointer_seeds.sort_unstable();
            pointer_seeds.dedup_by_key(|(pointer, _)| *pointer);
            for (pointer, blind) in pointer_seeds {
                if pointer % 4 != 0 || !window(pointer) || pointer == bank_va {
                    continue;
                }
                let offset = ((pointer - bank_va) / 4) as usize;
                let Some(&first_word) = text_words.get(offset) else {
                    continue;
                };
                if blind && (first_word >> 16 != 0x27bd || first_word & 0x8000 == 0) {
                    continue;
                }
                if first_word != 0
                    && fn64_discover::sig_scan::plausible_function_boundary(&text_words, offset)
                    && !bank_roots.contains(&pointer)
                {
                    bank_roots.push(pointer);
                }
            }
            if bank_roots.is_empty() {
                continue;
            }
            let (bank_cfg, _) = build_cfg_closed_with_facts_and_claims(
                &db,
                bank,
                text_bytes,
                bank_va,
                &bank_roots,
                &BTreeMap::new(),
            );
            let bank_part = partition(&bank_cfg);
            let bank_overlaps = same_bank_overlaps(&bank_part, &bank_cfg);
            assert!(
                bank_overlaps.is_empty(),
                "bank {bank}: same-bank owner overlaps must be zero"
            );
            let mut bank_jal: JalTargets = bank_cfg
                .direct_calls
                .iter()
                .map(|(_, target)| *target)
                .collect();
            bank_jal.extend(bank_excused.iter().copied());
            let bank_report =
                grade_functions(&bank_part.owners, &bank_answer, bank_code_end, &bank_jal);
            code_totals.0 += bank_report.total;
            code_totals.1 += bank_report.matched_exact;
            code_totals.2 += bank_report.matched_coarse + bank_report.interior_entries;
            code_totals.3 += bank_report.open;
            code_totals.4 += bank_report.wrong;
            // FN64_DISCOVER_PRINT_OPEN=1: list open code-segment functions
            // with their VAs — the exact targets a dynamic trace would need
            // to observe to recover them (see FN64_DISCOVER_TRACE).
            if std::env::var("FN64_DISCOVER_PRINT_OPEN").is_ok() {
                let open: Vec<String> = bank_report
                    .per_function
                    .iter()
                    .filter(|(_, grade)| {
                        matches!(
                            grade,
                            fn64_discover::grade_oot_functions::FunctionGrade::Open
                        )
                    })
                    .map(|(name, _)| {
                        let va = bank_answer
                            .iter()
                            .find(|answer| answer.name == *name)
                            .map(|answer| answer.va_start)
                            .unwrap_or(0);
                        format!("{name}@0x{va:08x}")
                    })
                    .collect();
                println!(
                    "code-seg open [{bank}] ({}): {}",
                    open.len(),
                    open.join(", ")
                );
            }
            if bank_report.wrong != 0 {
                for (function, grade) in &bank_report.per_function {
                    if let fn64_discover::grade_oot_functions::FunctionGrade::WrongSplit {
                        owner_root,
                    } = grade
                    {
                        code_wrong
                            .push(format!("bank {bank}: {function} split by 0x{owner_root:x}"));
                    }
                }
            }
        }
        if code_totals.0 != 0 {
            println!(
                "code-segment grade: total={} matched_exact={} coarse+interior={} open={} wrong={}",
                code_totals.0, code_totals.1, code_totals.2, code_totals.3, code_totals.4
            );
            println!(
                "  matched%={:.1} open%={:.1}",
                (code_totals.1 as f64 / code_totals.0 as f64) * 100.0,
                (code_totals.3 as f64 / code_totals.0 as f64) * 100.0
            );
            if code_totals.4 != 0 {
                for example in code_wrong.iter().take(10) {
                    println!("wrong: {example}");
                }
                eprintln!(
                    "gate_decomp_functions FAILED: code-segment wrong={}",
                    code_totals.4
                );
                std::process::exit(1);
            }
            println!("code-segment function grade PASSED (wrong=0)");
        }
    }
}
