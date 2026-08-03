//! Shared `dump.toml` loading for corpus-scale ROM assembly.
//!
//! Extracted from `bin/gate_corpus_homology.rs` so a second consumer
//! ([`bin/corpus_index.rs`](../../src/bin/corpus_index.rs)) assembles
//! [`CorpusRom`](crate::corpus_homology::CorpusRom)s through the SAME code
//! path rather than a second parallel implementation. Behavior is
//! byte-for-byte unchanged from the gate: only resident-image functions
//! (section ROM range inside the physical ROM) are used, and a `func_ADDR`
//! autoname carries no real-name signal.

use crate::callgraph_match::FunctionBody;
use crate::rom::NormalizedRom;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub struct SymbolsDoc {
    #[serde(default)]
    pub section: Vec<SectionDoc>,
}

#[derive(Debug, Deserialize)]
pub struct SectionDoc {
    pub name: String,
    pub rom: u32,
    pub vram: u32,
    pub size: u32,
    #[serde(default)]
    pub functions: Vec<FunctionDoc>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionDoc {
    pub name: String,
    pub vram: u32,
    pub size: u32,
}

/// One corpus ROM as loaded from a `dump.toml`: the matcher input plus the
/// held-out oracle (real symbol name per entry VA).
pub struct LoadedRom {
    pub label: String,
    pub functions: Vec<FunctionBody>,
    /// entry VA -> real (non-`func_ADDR`) symbol name.
    pub real_name_by_va: BTreeMap<u32, String>,
}

/// Load one ROM + its `dump.toml` boundary/name source. A malformed dump is a
/// loud error, never a silent skip.
pub fn load_rom(label: &str, rom_path: &str, dump_path: &str) -> Result<LoadedRom, String> {
    if !std::path::Path::new(rom_path).exists() {
        return Err(format!("{label} ROM {rom_path} does not exist"));
    }
    if !std::path::Path::new(dump_path).exists() {
        return Err(format!("{label} dump {dump_path} does not exist"));
    }
    let bytes = std::fs::read(rom_path).map_err(|error| format!("reading {rom_path}: {error}"))?;
    let rom = crate::rom::normalize(&bytes).map_err(|error| format!("{label} ROM: {error}"))?;
    let doc = parse_dump(dump_path)?;
    let (functions, real_name_by_va) = functions_from_dump(label, &rom, &doc)?;
    Ok(LoadedRom {
        label: label.to_string(),
        functions,
        real_name_by_va,
    })
}

pub fn parse_dump(path: &str) -> Result<SymbolsDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("reading {path}: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("parsing {path}: {error}"))
}

/// Turn one dump into function bodies plus the real-name oracle. Only
/// resident-image functions (section ROM range inside the physical ROM) are
/// used, matching gate_callgraph_match: the resident image is the shared
/// engine surface. A malformed dump is a loud error, not a silent skip.
pub fn functions_from_dump(
    label: &str,
    rom: &NormalizedRom,
    doc: &SymbolsDoc,
) -> Result<(Vec<FunctionBody>, BTreeMap<u32, String>), String> {
    let mut functions = Vec::new();
    let mut real_name_by_va = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for section in &doc.section {
        if !section.size.is_multiple_of(4) || !section.rom.is_multiple_of(4) {
            return Err(format!(
                "{label} section {:?} is not word-aligned",
                section.name
            ));
        }
        let section_end = section
            .rom
            .checked_add(section.size)
            .ok_or_else(|| format!("{label} section {:?} extent overflows", section.name))?;
        // Overlay sections whose ROM range is outside the physical image are
        // resident-relative VROM; skip them rather than read garbage.
        if section_end as usize > rom.len() {
            continue;
        }
        for function in &section.functions {
            if !function.size.is_multiple_of(4) {
                return Err(format!(
                    "{label} function {:?} has non-word size {:#x}",
                    function.name, function.size
                ));
            }
            if function.size == 0 {
                continue;
            }
            let section_offset = function.vram.checked_sub(section.vram).ok_or_else(|| {
                format!(
                    "{label} function {:?} precedes section {:?}",
                    function.name, section.name
                )
            })?;
            let function_end = section_offset
                .checked_add(function.size)
                .ok_or_else(|| format!("{label} function {:?} extent overflows", function.name))?;
            if function_end > section.size {
                return Err(format!(
                    "{label} function {:?} exceeds section {:?}",
                    function.name, section.name
                ));
            }
            let rom_start = section.rom.checked_add(section_offset).ok_or_else(|| {
                format!("{label} function {:?} ROM offset overflows", function.name)
            })?;
            let bytes = rom
                .bytes
                .get(rom_start as usize..(rom_start + function.size) as usize)
                .ok_or_else(|| format!("{label} function {:?} exceeds ROM", function.name))?;
            // A dump can list the same vram twice across aliased sections; the
            // call graph requires unique entries, so keep the first.
            if !seen.insert(function.vram) {
                continue;
            }
            if let Some(name) = real_name(&function.name) {
                real_name_by_va.insert(function.vram, name);
            }
            functions.push(FunctionBody {
                identity: format!("{}:{}", section.name, function.name),
                va_start: function.vram,
                words: words_from_be(bytes),
            });
        }
    }
    Ok((functions, real_name_by_va))
}

/// A `func_ADDR` autoname is address-derived and differs between games even
/// for the same function, so it carries no correspondence signal. Only a
/// hand-named symbol (e.g. `osSetIntMask`) is a held-out oracle.
pub fn real_name(name: &str) -> Option<String> {
    if name.starts_with("func_") {
        None
    } else {
        Some(name.to_string())
    }
}

pub fn words_from_be(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks_exact(4)
        .map(|word| u32::from_be_bytes(word.try_into().expect("four-byte chunk")))
        .collect()
}
