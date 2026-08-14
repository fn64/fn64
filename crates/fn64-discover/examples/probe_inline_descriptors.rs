//! Overlay descriptors that were never DATA: recovering the triple from
//! `lui`/`addiu` immediates at the call site.
//!
//! The AKI-style descriptor search enumerates ZERO candidates on 22 corpus
//! ROMs that demonstrably have multi-span code. Those games never store a
//! (rom_start, rom_end, vram_dest) record in a table -- the compiler
//! materialized each constant as an instruction pair immediately before the
//! DMA call, so there is nothing in the data segment to find.
//!
//! Ground truth, International Superstar Soccer '98 (USA) at ROM 0x571bc:
//!
//! ```text
//!   3c04002d  lui   a0,0x002d
//!   24845d30  addiu a0,a0,0x5d30    -> a0 = 0x002d5d30   rom_start
//!   3c05803d  lui   a1,0x803d
//!   24a5f000  addiu a1,a1,0xf000    -> a1 = 0x803cf000   vram_dest (KSEG0)
//!   3c06002e  lui   a2,0x002e
//!   24c64380  addiu a2,a2,0x4380    -> a2 = 0x002e4380   rom_end
//!   0c00081a  jal   0x2068
//!   00c43023  subu  a2,a2,a0        -> length = end - start (delay slot)
//! ```
//!
//! This probe reconstructs every 32-bit constant in the image from
//! `lui`+`addiu`/`ori` pairs, then admits a triple inside a sliding window
//! when the constants have overlay shape AND the implied ROM span is actually
//! dense with code. The density test is what separates a real descriptor from
//! a coincidental cluster of large integers in a counter array.
//!
//! Usage:
//!   probe_inline_descriptors <rom.z64> [--verbose]
//!   probe_inline_descriptors --corpus <dir>

use std::collections::BTreeMap;

/// `jr $ra`, the same structural code proxy `code_span_locality` uses.
const JR_RA: [u8; 4] = [0x03, 0xe0, 0x00, 0x08];

/// Instruction bytes a triple must fit inside. Three `lui`/`addiu` pairs plus
/// a `jal` is 32 bytes; 0x40 leaves room for interleaved setup without
/// admitting constants from an unrelated basic block.
const WINDOW_BYTES: usize = 0x40;

/// Below this offset is the header and boot stub, never an overlay payload.
const MIN_ROM_OFFSET: u32 = 0x1000;

/// KSEG0 window an overlay is loaded into. 8 MiB covers every N64 RDRAM
/// configuration including the Expansion Pak.
const KSEG0_LO: u32 = 0x8000_0000;
const KSEG0_HI: u32 = 0x8080_0000;

/// An overlay smaller than this is a stub, larger than this is the whole ROM.
const MIN_SPAN_LEN: u32 = 0x200;
const MAX_SPAN_LEN: u32 = 0x40_0000;

/// `jr $ra` per KiB the implied span must reach to count as real code.
/// Measured reference: the ISS'98 span [0x2d5d30,0x2e4380) holds 127 sites
/// over 58 KiB = 2.21/KiB.
const MIN_JR_RA_PER_KIB: f64 = 0.5;

#[derive(Clone, Copy)]
struct Constant {
    offset: usize,
    reg: u8,
    value: u32,
}

/// Reconstruct 32-bit constants from `lui`+`addiu`/`ori` pairs on the same
/// register. `addiu` sign-extends its immediate; `ori` does not.
fn recover_constants(rom: &[u8]) -> Vec<Constant> {
    let mut out = Vec::new();
    // Last `lui` seen per register, and where it was.
    let mut pending: [Option<(usize, u32)>; 32] = [None; 32];
    let mut offset = 0usize;
    while offset + 4 <= rom.len() {
        let word = u32::from_be_bytes(rom[offset..offset + 4].try_into().unwrap());
        let op = word >> 26;
        let rt = ((word >> 16) & 0x1f) as u8;
        let rs = ((word >> 21) & 0x1f) as u8;
        let imm = (word & 0xffff) as u16;
        match op {
            // lui rt, imm
            0x0f => pending[rt as usize] = Some((offset, (imm as u32) << 16)),
            // addiu rt, rs, imm  (op 0x09) / ori rt, rs, imm (op 0x0d)
            0x09 | 0x0d => {
                if let Some((lui_offset, hi)) = pending[rs as usize] {
                    // Only a same-register (or forwarded) pair is a constant.
                    let value = if op == 0x09 {
                        hi.wrapping_add(imm as i16 as i32 as u32)
                    } else {
                        hi | imm as u32
                    };
                    out.push(Constant {
                        offset: lui_offset,
                        reg: rt,
                        value,
                    });
                    if rt == rs {
                        pending[rs as usize] = None;
                    }
                }
            }
            _ => {}
        }
        offset += 4;
    }
    out
}

/// Prefix count of `jr $ra` sites, so span density is O(1) per query.
fn jr_ra_prefix(rom: &[u8]) -> Vec<u32> {
    let words = rom.len() / 4;
    let mut prefix = vec![0u32; words + 1];
    for index in 0..words {
        let at = index * 4;
        let hit = u32::from(rom[at..at + 4] == JR_RA);
        prefix[index + 1] = prefix[index] + hit;
    }
    prefix
}

fn sites_in(prefix: &[u32], start: u32, end: u32) -> u32 {
    let lo = (start as usize / 4).min(prefix.len() - 1);
    let hi = (end as usize / 4).min(prefix.len() - 1);
    prefix[hi].saturating_sub(prefix[lo])
}

#[derive(Clone, Copy, Debug)]
struct Triple {
    site: usize,
    rom_start: u32,
    rom_end: u32,
    vram: u32,
    jr_ra: u32,
}

fn plausible_rom_offset(value: u32, rom_len: usize) -> bool {
    value >= MIN_ROM_OFFSET && (value as usize) < rom_len && value % 4 == 0
}

fn plausible_kseg0(value: u32) -> bool {
    (KSEG0_LO..KSEG0_HI).contains(&value) && value % 8 == 0
}

/// Is there a `jal` / `jalr` inside this window? The descriptor exists only to
/// feed a DMA call, so a triple with no call in reach is not a descriptor.
/// This is the constraint that separates a real call site from three large
/// integers that happen to share a basic block.
fn window_has_call(rom: &[u8], start: usize, end: usize) -> bool {
    let end = end.min(rom.len().saturating_sub(4));
    let mut at = start;
    while at + 4 <= end + 4 && at + 4 <= rom.len() {
        let word = u32::from_be_bytes(rom[at..at + 4].try_into().unwrap());
        let op = word >> 26;
        // jal (0x03), or SPECIAL jalr (op 0, funct 0x09).
        if op == 0x03 || (op == 0 && (word & 0x3f) == 0x09) {
            return true;
        }
        at += 4;
    }
    false
}

fn admit_triples(rom: &[u8], constants: &[Constant], prefix: &[u32]) -> Vec<Triple> {
    let strict = std::env::var("FN64_INLINE_STRICT").is_ok();
    let rom_len = rom.len();
    let mut admitted: Vec<Triple> = Vec::new();
    for anchor in 0..constants.len() {
        let window_end = constants[anchor].offset + WINDOW_BYTES;
        let mut roms: Vec<Constant> = Vec::new();
        let mut vrams: Vec<Constant> = Vec::new();
        for candidate in &constants[anchor..] {
            if candidate.offset >= window_end {
                break;
            }
            if plausible_rom_offset(candidate.value, rom_len) {
                roms.push(*candidate);
            }
            if plausible_kseg0(candidate.value) {
                vrams.push(*candidate);
            }
        }
        if roms.len() < 2 || vrams.is_empty() {
            continue;
        }
        // A descriptor is built to be passed to a DMA routine. No call in the
        // window means these constants were never arguments to anything.
        if strict && !window_has_call(rom, constants[anchor].offset, window_end) {
            continue;
        }
        // Pick the widest admissible ROM pair in the window, then the first
        // KSEG0 constant loaded into a different register.
        let mut best: Option<Triple> = None;
        for (index, start) in roms.iter().enumerate() {
            for end in &roms[index + 1..] {
                let (lo, hi) = if start.value <= end.value {
                    (start.value, end.value)
                } else {
                    (end.value, start.value)
                };
                let len = hi - lo;
                if len <= MIN_SPAN_LEN || len >= MAX_SPAN_LEN {
                    continue;
                }
                let sites = sites_in(prefix, lo, hi);
                let density = f64::from(sites) / (f64::from(len) / 1024.0);
                if density < MIN_JR_RA_PER_KIB {
                    continue;
                }
                // Strict: the three constants must land in three DIFFERENT
                // registers, as real arguments do. The permissive fallback of
                // reusing a register is what let counter arrays through.
                let vram = vrams.iter().find(|v| v.reg != start.reg && v.reg != end.reg);
                let vram = if strict {
                    match vram {
                        Some(vram) if start.reg != end.reg => vram,
                        _ => continue,
                    }
                } else {
                    match vram.or_else(|| vrams.first()) {
                        Some(vram) => vram,
                        None => continue,
                    }
                };
                let triple = Triple {
                    site: constants[anchor].offset,
                    rom_start: lo,
                    rom_end: hi,
                    vram: vram.value,
                    jr_ra: sites,
                };
                if best.is_none_or(|b| sites > b.jr_ra) {
                    best = Some(triple);
                }
            }
        }
        if let Some(triple) = best {
            admitted.push(triple);
        }
    }
    // Collapse duplicates: many overlapping windows recover the same span.
    admitted.sort_by_key(|t| (t.rom_start, t.rom_end, t.site));
    admitted.dedup_by_key(|t| (t.rom_start, t.rom_end));
    admitted
}

struct RomReport {
    label: String,
    class: String,
    triples: usize,
    covered_bytes: u64,
    non_boot_sites: u32,
    non_boot_covered: u32,
}

fn analyze(path: &std::path::Path) -> Option<(RomReport, Vec<Triple>)> {
    let bytes = std::fs::read(path).ok()?;
    let rom = fn64_discover::rom::normalize(&bytes).ok()?;
    let label = path.file_stem()?.to_string_lossy().into_owned();
    let locality = fn64_discover::code_span_locality::measure_code_span_locality(&rom.bytes);
    let class = match locality.class {
        fn64_discover::code_span_locality::CodeSpanClass::NoCodeFound => "NO_CODE_FOUND",
        fn64_discover::code_span_locality::CodeSpanClass::SingleBank => "SINGLE_BANK",
        fn64_discover::code_span_locality::CodeSpanClass::MostlySingleBank => "MOSTLY_SINGLE_BANK",
        fn64_discover::code_span_locality::CodeSpanClass::MultiSpan => "MULTI_SPAN",
    };

    let prefix = jr_ra_prefix(&rom.bytes);
    let constants = recover_constants(&rom.bytes);
    let triples = admit_triples(&rom.bytes, &constants, &prefix);

    // "Non-boot" = every jr-ra site outside the largest (resident) span. That
    // is exactly the code a BootBankOnly verdict fails to account for.
    let (boot_lo, boot_hi) = locality.largest_span.unwrap_or((0, 0));
    let mut non_boot_sites = 0u32;
    let mut non_boot_covered = 0u32;
    let words = rom.bytes.len() / 4;
    for index in 0..words {
        let at = index * 4;
        if rom.bytes[at..at + 4] != JR_RA {
            continue;
        }
        if at >= boot_lo && at <= boot_hi {
            continue;
        }
        non_boot_sites += 1;
        let at = at as u32;
        if triples
            .iter()
            .any(|t| at >= t.rom_start && at < t.rom_end)
        {
            non_boot_covered += 1;
        }
    }

    // Coverage in bytes, counting overlapping spans once.
    let mut merged: Vec<(u32, u32)> = triples.iter().map(|t| (t.rom_start, t.rom_end)).collect();
    merged.sort_unstable();
    let mut covered_bytes = 0u64;
    let mut cursor = 0u32;
    for (lo, hi) in merged {
        let lo = lo.max(cursor);
        if hi > lo {
            covered_bytes += u64::from(hi - lo);
            cursor = hi;
        }
    }

    Some((
        RomReport {
            label,
            class: class.to_string(),
            triples: triples.len(),
            covered_bytes,
            non_boot_sites,
            non_boot_covered,
        },
        triples,
    ))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--corpus") {
        let dir = args.get(1).cloned().unwrap_or_else(|| {
            std::env::var("FN64_ROM_CORPUS_DIR").expect("FN64_ROM_CORPUS_DIR or --corpus <dir>")
        });
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .expect("read corpus dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                matches!(
                    path.extension().and_then(|e| e.to_str()),
                    Some("z64" | "n64" | "v64")
                )
            })
            .collect();
        paths.sort();
        let mut by_class: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
        println!("class\trom\ttriples\tcovered_bytes\tnon_boot_jr_ra\tcovered_jr_ra\tfraction");
        for path in &paths {
            let Some((report, _)) = analyze(path) else {
                continue;
            };
            let fraction = if report.non_boot_sites == 0 {
                0.0
            } else {
                f64::from(report.non_boot_covered) / f64::from(report.non_boot_sites)
            };
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{:.3}",
                report.class,
                report.label,
                report.triples,
                report.covered_bytes,
                report.non_boot_sites,
                report.non_boot_covered,
                fraction
            );
            let slot = by_class.entry(report.class.clone()).or_insert((0, 0, 0));
            slot.0 += 1;
            slot.1 += report.triples;
            if report.triples > 0 {
                slot.2 += 1;
            }
        }
        println!("#");
        println!("# class\troms\ttotal_triples\troms_with_any_triple");
        for (class, (roms, triples, firing)) in by_class {
            println!("# {class}\t{roms}\t{triples}\t{firing}");
        }
        return;
    }

    let path = std::path::PathBuf::from(args.first().expect("usage: <rom.z64> | --corpus <dir>"));
    let verbose = args.iter().any(|a| a == "--verbose");
    let (report, triples) = analyze(&path).expect("analyze rom");
    println!(
        "{}\t{}\ttriples={} covered={} non_boot_jr_ra={} covered={}",
        report.class,
        report.label,
        report.triples,
        report.covered_bytes,
        report.non_boot_sites,
        report.non_boot_covered
    );
    if verbose {
        for triple in &triples {
            println!(
                "  site={:#x} rom=[{:#x},{:#x}) len={:#x} vram={:#010x} jr_ra={}",
                triple.site,
                triple.rom_start,
                triple.rom_end,
                triple.rom_end - triple.rom_start,
                triple.vram,
                triple.jr_ra
            );
        }
    }
}
