//! gp-base gate runner: recovers the IDO small-data `$gp` base for the NW4E
//! and NWXE resident boot banks by constrained voting ([`gp_base::analyze`]),
//! then reports the admitted base (or Open), how many gp-relative accesses it
//! explains vs. the total seen, and how many resolved data addresses land
//! in-range vs. out-of-range under the admitted base.
//!
//! Discovery inputs are ROM bytes only. The mapped data range each base is
//! voted against is derived from proven, hardware-fixed facts: the resident
//! boot mapping's VA base (header entry point), and the entry-stub zero-fill
//! loop's BSS end (`loaders::recognize_entry_stub_any`).
//!
//! No answer key or symbol table enters the vote. If a decomp dump
//! (`FN64_DISCOVER_{NW4E,NWXE}_DUMP`) names a `_gp`/`gp` symbol, it is used
//! ONLY as a post-hoc grading assertion against the admitted base -- never as
//! an inference input.
//!
//! Same posture as the other gate binaries: this is the auditable "did the
//! mechanism run on real bytes and report real, non-fabricated counts"
//! artifact. The correctness proof for the mechanism is `gp_base.rs`'s unit
//! tests. Ten byte-identical re-runs are required before the analysis is
//! called determined.

use fn64_discover::banks;
use fn64_discover::gp_base::{analyze, DataRange, GpBaseAnalysis, GpBaseOutcome, GpBaseSource};
use fn64_discover::loaders::{recognize_entry_stub_any, RecognizedEntryStub, VirtualAddress};
use fn64_discover::{run_discovery, Fact};

fn required(variable: &str, what: &str) -> Result<String, String> {
    fn64_discover::required_env_path(variable, what)
}

fn main() {
    let mut exit_code = 0;
    println!("=== fn64-discover gp-base gate ===\n");

    for (label, rom_var, dump_var) in [
        ("NW4E", "FN64_DISCOVER_NW4E_ROM", "FN64_DISCOVER_NW4E_DUMP"),
        ("NWXE", "FN64_DISCOVER_NWXE_ROM", "FN64_DISCOVER_NWXE_DUMP"),
    ] {
        match run_one(label, rom_var, dump_var) {
            Ok(()) => println!(),
            Err(error) => {
                eprintln!("{label} gp-base gate FAILED: {error}\n");
                exit_code = 1;
            }
        }
    }

    std::process::exit(exit_code);
}

/// Resolve the resident boot bank geometry for one ROM, from proven facts.
///
/// The hardware boot copy DMAs a fixed 1 MiB from ROM into RAM, but only the
/// low part of that window is resident code+data; the tail is unrelated ROM
/// content (overlay/asset bytes) that must NOT be decoded as code. The
/// entry-stub zero-fill loop bounds the real image: its `start` is the top of
/// initialized data (`.bss` begins there), its `end` is the top of `.bss`.
///
/// - `code_bytes` is `[va_start, bss_start)`: the ROM-backed code+data region
///   the access/construction scan runs over. Decoding stops at `bss_start` so
///   the scan never mistakes post-image ROM content for gp accesses.
/// - `data` is `[va_start, bss_end)`: the whole resident image including
///   zeroed `.bss`, the window a gp-relative access may legitimately target.
struct ResidentGeometry {
    code_bytes: Vec<u8>,
    va_start: u32,
    bss_start: u32,
    bss_end: u32,
    data: DataRange,
}

fn resident_geometry(
    rom: &fn64_discover::NormalizedRom,
    db: &fn64_discover::FactDb,
) -> Result<ResidentGeometry, String> {
    let boot = db
        .proven_rom_mappings()
        .into_iter()
        .find(|fact| matches!(fact, Fact::RomMapping { bank, .. } if bank == banks::BOOT_BANK))
        .ok_or("boot bank not proven")?;
    let (rom_start, rom_end, va_start) = match boot {
        Fact::RomMapping {
            rom_start,
            rom_end,
            va_start,
            ..
        } => (*rom_start, *rom_end, *va_start),
        _ => unreachable!(),
    };

    // Recover the BSS start and end from the entry-stub zero-fill loop -- both
    // are proven, hardware-rooted facts about the resident image's extent.
    let words: Vec<u32> = rom
        .bytes
        .get(0x1000..)
        .ok_or("ROM has no hardware boot-copy source")?
        .chunks_exact(4)
        .take(1024)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect();
    let mut bss = None;
    for word_count in [16usize, 32, 64, 128, 256, 512, 1024] {
        match recognize_entry_stub_any(
            &words[..word_count.min(words.len())],
            VirtualAddress::new(rom.header.entry_point),
        ) {
            Ok(RecognizedEntryStub::Countdown(observation)) => {
                bss = Some((
                    observation.zero_fill.start.get(),
                    observation.zero_fill.end_exclusive.get(),
                ));
                break;
            }
            Ok(RecognizedEntryStub::EndPointer(observation)) => {
                bss = Some((
                    observation.zero_fill.start.get(),
                    observation.zero_fill.end_exclusive.get(),
                ));
                break;
            }
            Err(_) => {}
        }
    }
    let (bss_start, bss_end) =
        bss.ok_or("no accepted entry stub in 1024-word budget; cannot bound the resident image")?;
    if bss_start <= va_start || bss_end <= bss_start {
        return Err(format!(
            "recovered BSS [{bss_start:#010x},{bss_end:#010x}) is not a proper interval above \
             the resident VA base {va_start:#010x}"
        ));
    }

    // Slice code+data as [va_start, bss_start). Clamp to the ROM-backed boot
    // copy: initialized data cannot exceed what the DMA loaded.
    let code_va_len = bss_start - va_start;
    let rom_backed = rom_end - rom_start;
    let code_len = code_va_len.min(rom_backed) as usize;
    let code_bytes = rom
        .bytes
        .get(rom_start as usize..rom_start as usize + code_len)
        .ok_or("code+data interval falls outside normalized ROM")?
        .to_vec();

    Ok(ResidentGeometry {
        code_bytes,
        va_start,
        bss_start,
        bss_end,
        data: DataRange {
            start: va_start,
            end: bss_end,
        },
    })
}

fn run_one(label: &str, rom_var: &str, dump_var: &str) -> Result<(), String> {
    let rom_path = required(rom_var, &format!("the {label} .z64"))?;
    if !std::path::Path::new(&rom_path).exists() {
        return Err(format!("{rom_path} not found"));
    }
    let rom_bytes = std::fs::read(&rom_path).map_err(|e| format!("reading {rom_path}: {e}"))?;
    let (rom, db) =
        run_discovery(&rom_bytes, None).map_err(|e| format!("normalizing {label} ROM: {e}"))?;
    println!("{label} ROM: {} bytes, sha256={}", rom.len(), rom.sha256);

    let geom = resident_geometry(&rom, &db)?;
    println!(
        "  resident image: code+data=[{:#010x},{:#010x}) ({} bytes scanned); bss=[{:#010x},{:#010x}); vote range=[{:#010x},{:#010x})",
        geom.va_start,
        geom.bss_start,
        geom.code_bytes.len(),
        geom.bss_start,
        geom.bss_end,
        geom.data.start,
        geom.data.end,
    );

    let analysis = analyze(&geom.code_bytes, geom.va_start, geom.data);
    report_analysis(label, &analysis)?;

    // Grading assertion only: cross-check the admitted base against a `_gp`
    // symbol if the dump names one. The dump is NOT fed into inference.
    if let Ok(dump_path) = std::env::var(dump_var) {
        cross_check_gp_symbol(label, &dump_path, &analysis)?;
    } else {
        println!("  (no {dump_var} set; skipping _gp cross-check)");
    }

    // Determinism: 10 consecutive byte-identical analyses over the same bank.
    let baseline = serde_json::to_string(&analysis).map_err(|e| e.to_string())?;
    for run in 1..10 {
        let again = analyze(&geom.code_bytes, geom.va_start, geom.data);
        let again_json = serde_json::to_string(&again).map_err(|e| e.to_string())?;
        if again_json != baseline {
            return Err(format!("analysis run {run} diverged from run 0"));
        }
    }
    println!("  determinism: 10/10 byte-identical analyses");

    Ok(())
}

fn report_analysis(label: &str, analysis: &GpBaseAnalysis) -> Result<(), String> {
    match &analysis.outcome {
        GpBaseOutcome::Admitted {
            base,
            source,
            explained,
            total,
            out_of_range,
        } => {
            let source_str = match source {
                GpBaseSource::BootConstruction { def_pc } => {
                    format!("boot construction (def_pc={def_pc:#010x})")
                }
                GpBaseSource::OffsetHistogram => "offset histogram (fallback)".to_string(),
            };
            println!("  {label} admitted $gp base = {base:#010x} via {source_str}");
            println!(
                "    explained {explained}/{total} gp-relative accesses; sites emitted={}",
                analysis.sites.len()
            );
            println!("    resolved in-range={explained}  out-of-range={out_of_range}");
            if *out_of_range > *explained {
                return Err(format!(
                    "red flag: more gp accesses resolve OUT of the data range ({out_of_range}) \
                     than in ({explained}) under the admitted base -- the base is suspect"
                ));
            }
            if *out_of_range > 0 {
                println!(
                    "    note: {out_of_range} access(es) resolve out of range (small-data \
                     accesses to non-resident addresses, or a decode artifact)"
                );
            }
        }
        GpBaseOutcome::Open { contenders } => {
            println!(
                "  {label} $gp base: OPEN ({} contender(s), no unique dominating winner)",
                contenders.len()
            );
            for vote in contenders {
                println!(
                    "    candidate {:#010x} in_range={} out_of_range={} ({:?})",
                    vote.candidate.base, vote.in_range, vote.out_of_range, vote.candidate.source
                );
            }
            println!(
                "    total gp-relative accesses seen={}; leaving base Open (honest, not a failure)",
                analysis.total_accesses
            );
        }
        GpBaseOutcome::NoGpAccesses => {
            println!(
                "  {label} $gp base: no gp-relative accesses in the resident bank (nothing to base)"
            );
        }
    }
    Ok(())
}

/// If the dump names a `_gp` (or `gp`) symbol, assert the admitted base equals
/// its vram. A missing symbol is reported, not an error: the dumps here carry
/// no data/gp symbols by construction.
fn cross_check_gp_symbol(
    label: &str,
    dump_path: &str,
    analysis: &GpBaseAnalysis,
) -> Result<(), String> {
    let text =
        std::fs::read_to_string(dump_path).map_err(|e| format!("reading {dump_path}: {e}"))?;
    let Some(sym_vram) = find_gp_symbol(&text) else {
        println!("  (dump names no _gp/gp symbol; nothing to cross-check)");
        return Ok(());
    };
    match &analysis.outcome {
        GpBaseOutcome::Admitted { base, .. } => {
            if *base == sym_vram {
                println!(
                    "  {label} _gp cross-check PASS: admitted base {base:#010x} == dump _gp {sym_vram:#010x}"
                );
                Ok(())
            } else {
                Err(format!(
                    "_gp cross-check FAIL: admitted base {base:#010x} != dump _gp {sym_vram:#010x}"
                ))
            }
        }
        _ => {
            println!(
                "  {label} _gp cross-check: dump _gp={sym_vram:#010x} but base is Open/absent (no assertion)"
            );
            Ok(())
        }
    }
}

/// Find a `_gp`/`gp` symbol's vram in a dump.toml. Recognizes the same
/// `{ name = "...", vram = 0x... }` shape the answer keys use. Returns the
/// first match. Deliberately narrow: only an exact `_gp` or `gp` name counts.
fn find_gp_symbol(toml_text: &str) -> Option<u32> {
    for line in toml_text.lines() {
        let line = line.trim();
        if !line.contains("name") || !line.contains("vram") {
            continue;
        }
        let name = extract_field(line, "name")?;
        if name != "_gp" && name != "gp" {
            continue;
        }
        let vram = extract_hex_field(line, "vram")?;
        return Some(vram);
    }
    None
}

fn extract_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(key)?;
    let rest = &line[start + key.len()..];
    let quote = rest.find('"')?;
    let after = &rest[quote + 1..];
    let end = after.find('"')?;
    Some(&after[..end])
}

fn extract_hex_field(line: &str, key: &str) -> Option<u32> {
    let start = line.find(key)?;
    let rest = &line[start + key.len()..];
    let hex = rest.find("0x")?;
    let after = &rest[hex + 2..];
    let end = after
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(after.len());
    u32::from_str_radix(&after[..end], 16).ok()
}
