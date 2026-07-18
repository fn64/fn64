//! Grading-only real-ROM gate for the NW4E selector-dispatcher evidence.
//!
//! Re-derives the recorded `NW4E_SELECTOR` facts mechanically from ROM bytes
//! and sweeps every canonical bank for references to the selector flag word
//! and mode byte. Expected values are grading data (like `gate_loaders`'
//! expected entry), never engine dispatch.
//!
//! What this gate proves and does not prove:
//!
//! - The dispatcher's flag reads, masks, branch senses, record-pointer
//!   materializations, and thread-entry registration are byte-derived here,
//!   at the corrected VA mapping (resident delta `0x7fff_f400`).
//! - Flag-store sites in overlay banks are **candidate** cross references
//!   from a linear scan of proven load-image bytes; executable permission
//!   and reachability of those sites are not claimed.
//! - Stored constants are linear fall-through values only; a join can
//!   supply other values (`gate` prints `Unresolved` in that case rather
//!   than guessing).

use fn64_discover::aki_reference::{NW4E_BANKS, NW4E_DESCRIPTOR_TABLE, NW4E_SELECTOR};
use fn64_discover::cfg::{classify_control, region_target, ControlOp};
use fn64_discover::loaders::{recognize_entry_stub_any, RecognizedEntryStub, VirtualAddress};
use fn64_discover::normalize;
use fn64_discover::xref::{scan_global_refs, GlobalRefSite, RefKind, StoredValue};

const RESIDENT_ROM_START: u32 = 0x1000;

struct Bank<'a> {
    name: &'static str,
    bytes: &'a [u8],
    va_start: u32,
}

fn main() {
    let Ok(path) = std::env::var("FN64_DISCOVER_NW4E_ROM") else {
        eprintln!("gate_selector: FN64_DISCOVER_NW4E_ROM is required");
        std::process::exit(1);
    };
    if let Err(error) = run(&path) {
        eprintln!("gate_selector: {error}");
        std::process::exit(1);
    }
}

fn run(path: &str) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("reading {path}: {error}"))?;
    let rom = normalize(&bytes).map_err(|error| error.to_string())?;
    let entry = rom.header.entry_point;
    let resident_delta = entry.wrapping_sub(RESIDENT_ROM_START);

    // 1. Generic boot facts: the entry stub gives the BSS clear range. The
    // flag and mode byte must fall inside it (zero-initialized state).
    let boot_words: Vec<u32> = rom.bytes[RESIDENT_ROM_START as usize..]
        .chunks_exact(4)
        .take(64)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect();
    let stub = recognize_entry_stub_any(&boot_words, VirtualAddress::new(entry))
        .map_err(|error| format!("entry stub not recognized: {error}"))?;
    let RecognizedEntryStub::Countdown(observation) = stub else {
        return Err("expected countdown entry stub form".to_string());
    };
    let bss_start = observation.zero_fill.start.get();
    let bss_end = observation.zero_fill.end_exclusive.get();
    for (label, va) in [
        ("flag", NW4E_SELECTOR.flag_va),
        ("mode byte", NW4E_SELECTOR.mode_byte_va),
    ] {
        if va < bss_start || va >= bss_end {
            return Err(format!(
                "{label} {va:#010x} is outside the derived BSS [{bss_start:#010x},{bss_end:#010x})"
            ));
        }
    }

    // 2. Canonical banks: the resident image up to the derived BSS start,
    // plus the five byte-verified overlay ranges.
    let resident_end_rom = bss_start.wrapping_sub(resident_delta);
    let resident = Bank {
        name: "resident",
        bytes: &rom.bytes[RESIDENT_ROM_START as usize..resident_end_rom as usize],
        va_start: entry,
    };
    let overlays: Vec<Bank> = NW4E_BANKS
        .iter()
        .map(|bank| Bank {
            name: bank.bank,
            bytes: &rom.bytes[bank.rom_start as usize..bank.rom_end as usize],
            va_start: bank.va_start,
        })
        .collect();
    let mut banks = vec![resident];
    banks.extend(overlays);

    // 3. Re-derive the dispatcher structure from resident bytes.
    verify_dispatcher(&banks[0], resident_delta)?;

    // 4. No direct transfer targets the dispatcher in any bank: its entry
    // is data-derived (thread creation), which step 3 verified.
    for bank in &banks {
        for (index, chunk) in bank.bytes.chunks_exact(4).enumerate() {
            let word = u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"));
            let pc = bank.va_start.wrapping_add(index as u32 * 4);
            let target = match classify_control(word) {
                ControlOp::J { target } | ControlOp::Jal { target } => {
                    Some(region_target(pc, target))
                }
                ControlOp::Branch { target, .. } | ControlOp::BranchLikely { target, .. } => Some(
                    pc.wrapping_add(4)
                        .wrapping_add(((target as i32) << 2) as u32),
                ),
                _ => None,
            };
            if target == Some(NW4E_SELECTOR.dispatcher_va) {
                return Err(format!(
                    "unexpected direct transfer to dispatcher from {}:{pc:#010x}",
                    bank.name
                ));
            }
        }
    }
    println!(
        "dispatcher: VA {:#010x} (ROM {:#x}), no direct j/jal/branch callers in any bank; \
         entry registered via thread-create wrapper {:#010x}",
        NW4E_SELECTOR.dispatcher_va,
        NW4E_SELECTOR.dispatcher_va.wrapping_sub(resident_delta),
        NW4E_SELECTOR.thread_create_wrapper_va,
    );

    // 5. Sweep every bank for flag and mode-byte references.
    let mut flag_stores: Vec<(String, u32)> = Vec::new();
    for bank in &banks {
        let flag_sites = scan_global_refs(bank.bytes, bank.va_start, NW4E_SELECTOR.flag_va, 4);
        let mode_sites = scan_global_refs(bank.bytes, bank.va_start, NW4E_SELECTOR.mode_byte_va, 1);
        println!(
            "{}: flag refs={} mode-byte refs={}",
            bank.name,
            flag_sites.len(),
            mode_sites.len()
        );
        for site in &flag_sites {
            println!("  flag {}", format_site(site));
            if matches!(site.kind, RefKind::Store { .. }) {
                flag_stores.push((bank.name.to_string(), site.pc));
            }
        }
        for site in &mode_sites {
            println!("  mode {}", format_site(site));
        }
    }

    // 6. Grade the store inventory against the byte-verified expectation.
    // These PCs are grading data derived by inspection of the same bytes;
    // the gate fails loudly if the mechanical sweep drifts from them.
    let expected_stores: Vec<(&str, u32)> = vec![
        ("resident", NW4E_SELECTOR.init_store_pc),
        ("R2", 0x80106940),
        ("R2", 0x80106dac),
        ("R2", 0x80106dec),
        ("R3", 0x80109124),
        ("R3", 0x80109140),
        ("R3", 0x80109178),
        ("R5", 0x80106824),
    ];
    let found: Vec<(&str, u32)> = flag_stores
        .iter()
        .map(|(bank, pc)| (bank.as_str(), *pc))
        .collect();
    if found != expected_stores {
        return Err(format!(
            "flag store inventory drifted: expected {expected_stores:x?}, found {found:x?}"
        ));
    }
    println!(
        "flag stores: {} sites match the graded inventory (1 resident init + 3 R2 + 3 R3 + 1 R5)",
        found.len()
    );
    println!("gate_selector PASSED");
    Ok(())
}

fn verify_dispatcher(resident: &Bank, resident_delta: u32) -> Result<(), String> {
    let sel = NW4E_SELECTOR;
    let window_start = (sel.dispatcher_va.wrapping_sub(resident.va_start)) as usize;
    let window_end = window_start + 0x400;
    let window = resident
        .bytes
        .get(window_start..window_end)
        .ok_or("dispatcher window out of resident bounds")?;
    let word_at = |pc: u32| -> Result<u32, String> {
        let offset =
            pc.checked_sub(sel.dispatcher_va)
                .ok_or_else(|| format!("pc {pc:#010x} precedes dispatcher"))? as usize;
        let chunk = window
            .get(offset..offset + 4)
            .ok_or_else(|| format!("pc {pc:#010x} outside dispatcher window"))?;
        Ok(u32::from_be_bytes(chunk.try_into().expect("four bytes")))
    };

    // Flag references inside the window: exactly the init store plus the
    // four test loads, at the recorded PCs.
    let sites = scan_global_refs(window, sel.dispatcher_va, sel.flag_va, 4);
    let mut loads = Vec::new();
    let mut stores = Vec::new();
    for site in &sites {
        match site.kind {
            RefKind::Load { width: 4 } => loads.push(site.pc),
            RefKind::Store { width: 4, value } => stores.push((site.pc, value)),
            other => return Err(format!("unexpected dispatcher flag ref kind {other:?}")),
        }
    }
    if stores != vec![(sel.init_store_pc, StoredValue::Zero)] {
        return Err(format!(
            "dispatcher flag stores drifted from init evidence: {stores:x?}"
        ));
    }
    let expected_loads: Vec<u32> = [
        sel.r2_test_pc,
        sel.r3_test_pc,
        sel.r5_test_pc,
        sel.loop_test_pc,
    ]
    .iter()
    .map(|lui_pc| lui_pc + 4)
    .collect();
    if loads != expected_loads {
        return Err(format!(
            "dispatcher flag loads drifted: expected {expected_loads:x?}, found {loads:x?}"
        ));
    }

    // Each test: lui / lw / andi mask / branch, with the recorded sense.
    // andi rt,rs,imm encoding: 0x30000000 | rs<<21 | rt<<16 | imm.
    for (label, test_pc, mask, taken_skips) in [
        ("r2", sel.r2_test_pc, sel.r2_skip_mask, true),
        ("r3", sel.r3_test_pc, sel.r3_skip_mask, true),
        ("r5", sel.r5_test_pc, sel.r5_take_mask, false),
        ("loop", sel.loop_test_pc, sel.loop_mask, true),
    ] {
        let andi = word_at(test_pc + 8)?;
        if andi >> 26 != 0x0C || andi & 0xFFFF != mask {
            return Err(format!(
                "{label} test at {test_pc:#010x}: expected andi mask {mask:#x}, word {andi:#010x}"
            ));
        }
        let branch = word_at(test_pc + 12)?;
        let opcode = branch >> 26;
        // bnez = bne rs,$zero (0x05); beqz = beq rs,$zero (0x04).
        let expected_opcode = if taken_skips { 0x05 } else { 0x04 };
        if opcode != expected_opcode {
            return Err(format!(
                "{label} test branch at {test_pc:#010x}: opcode {opcode:#x}, expected {expected_opcode:#x}"
            ));
        }
    }
    // The loop-back branch target is the loop head.
    let loop_branch = word_at(sel.loop_test_pc + 12)?;
    let offset = (loop_branch & 0xFFFF) as i16 as i32;
    let loop_target = (sel.loop_test_pc + 12)
        .wrapping_add(4)
        .wrapping_add((offset * 4) as u32);
    if loop_target != sel.loop_head_pc {
        return Err(format!(
            "loop branch targets {loop_target:#010x}, expected head {:#010x}",
            sel.loop_head_pc
        ));
    }

    // Record-pointer materializations: each descriptor record's biased
    // pointer (record base VA + 0x10) is materialized once in the window.
    let table_va = NW4E_DESCRIPTOR_TABLE
        .table_rom_offset
        .wrapping_add(resident_delta);
    for record in 0..NW4E_DESCRIPTOR_TABLE.record_count {
        let biased = table_va + record * NW4E_DESCRIPTOR_TABLE.record_stride + 0x10;
        let sites = scan_global_refs(window, sel.dispatcher_va, biased, 4);
        let count = sites
            .iter()
            .filter(|site| site.kind == RefKind::Address)
            .count();
        if count != 1 {
            return Err(format!(
                "record {record} biased pointer {biased:#010x}: {count} materializations, expected 1"
            ));
        }
    }

    // Thread registration: the wrapper materializes the dispatcher address
    // and calls the create/start pair.
    let wrapper_start = (sel.thread_create_wrapper_va - resident.va_start) as usize;
    let wrapper = resident
        .bytes
        .get(wrapper_start..wrapper_start + 0x60)
        .ok_or("thread wrapper window out of resident bounds")?;
    let entry_sites = scan_global_refs(wrapper, sel.thread_create_wrapper_va, sel.dispatcher_va, 4);
    let materializations: Vec<u32> = entry_sites
        .iter()
        .filter(|site| site.kind == RefKind::Address)
        .map(|site| site.pc)
        .collect();
    if materializations != vec![sel.entry_materialize_pc] {
        return Err(format!(
            "dispatcher entry materialization drifted: {materializations:x?}"
        ));
    }
    let mut wrapper_calls = Vec::new();
    for (index, chunk) in wrapper.chunks_exact(4).enumerate() {
        let word = u32::from_be_bytes(chunk.try_into().expect("four bytes"));
        let pc = sel.thread_create_wrapper_va + index as u32 * 4;
        if let ControlOp::Jal { target } = classify_control(word) {
            wrapper_calls.push(region_target(pc, target));
        }
    }
    if wrapper_calls != vec![sel.thread_create_call_va, sel.thread_start_call_va] {
        return Err(format!(
            "thread wrapper call sequence drifted: {wrapper_calls:x?}"
        ));
    }

    println!(
        "dispatcher rederivation: init store {:#010x} (sw $zero), tests r2&{:#x} r3&{:#x} r5&{:#x} loop&{:#x} \
         with recorded senses, loop head {:#010x}, 5 record pointers, thread registration verified",
        sel.init_store_pc,
        sel.r2_skip_mask,
        sel.r3_skip_mask,
        sel.r5_take_mask,
        sel.loop_mask,
        sel.loop_head_pc,
    );
    Ok(())
}

fn format_site(site: &GlobalRefSite) -> String {
    let kind = match site.kind {
        RefKind::Load { width } => format!("load{width}"),
        RefKind::Store { width, value } => match value {
            StoredValue::Zero => format!("store{width} value=0 (zero reg)"),
            StoredValue::Constant { value, def_pc } => {
                format!("store{width} linear-value={value:#x} def={def_pc:#010x}")
            }
            StoredValue::Unresolved { reg } => format!("store{width} value=unresolved(r{reg})"),
        },
        RefKind::Address => "address-materialization".to_string(),
    };
    format!(
        "{kind} at {:#010x} (lui {:#010x}, addr {:#010x})",
        site.pc, site.lui_pc, site.addr
    )
}
