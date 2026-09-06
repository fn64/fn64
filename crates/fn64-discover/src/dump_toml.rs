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

/// Errors loading a corpus ROM plus its `dump.toml` boundary/name source.
#[derive(Debug, thiserror::Error)]
pub enum DumpTomlError {
    #[error("{label} ROM {rom_path} does not exist")]
    RomMissing { label: String, rom_path: String },
    #[error("{label} dump {dump_path} does not exist")]
    DumpMissing { label: String, dump_path: String },
    #[error("reading {path}: {error}")]
    Read { path: String, error: std::io::Error },
    #[error("{label} ROM: {error}")]
    NormalizeRom {
        label: String,
        error: crate::rom::RomRejectReason,
    },
    #[error("parsing {path}: {error}")]
    Parse { path: String, error: toml::de::Error },
    #[error("{label} section {section:?} is not word-aligned")]
    SectionNotWordAligned { label: String, section: String },
    #[error("{label} section {section:?} extent overflows")]
    SectionExtentOverflows { label: String, section: String },
    #[error("{label} function {function:?} has non-word size {size:#x}")]
    FunctionNonWordSize {
        label: String,
        function: String,
        size: u32,
    },
    #[error("{label} function {function:?} precedes section {section:?}")]
    FunctionPrecedesSection {
        label: String,
        function: String,
        section: String,
    },
    #[error("{label} function {function:?} extent overflows")]
    FunctionExtentOverflows { label: String, function: String },
    #[error("{label} function {function:?} exceeds section {section:?}")]
    FunctionExceedsSection {
        label: String,
        function: String,
        section: String,
    },
    #[error("{label} function {function:?} ROM offset overflows")]
    FunctionRomOffsetOverflows { label: String, function: String },
    #[error("{label} function {function:?} exceeds ROM")]
    FunctionExceedsRom { label: String, function: String },
}

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
pub fn load_rom(label: &str, rom_path: &str, dump_path: &str) -> Result<LoadedRom, DumpTomlError> {
    if !std::path::Path::new(rom_path).exists() {
        return Err(DumpTomlError::RomMissing {
            label: label.to_string(),
            rom_path: rom_path.to_string(),
        });
    }
    if !std::path::Path::new(dump_path).exists() {
        return Err(DumpTomlError::DumpMissing {
            label: label.to_string(),
            dump_path: dump_path.to_string(),
        });
    }
    let bytes = std::fs::read(rom_path).map_err(|error| DumpTomlError::Read {
        path: rom_path.to_string(),
        error,
    })?;
    let rom = crate::rom::normalize(&bytes).map_err(|error| DumpTomlError::NormalizeRom {
        label: label.to_string(),
        error,
    })?;
    let doc = parse_dump(dump_path)?;
    let (functions, real_name_by_va) = functions_from_dump(label, &rom, &doc)?;
    Ok(LoadedRom {
        label: label.to_string(),
        functions,
        real_name_by_va,
    })
}

pub fn parse_dump(path: &str) -> Result<SymbolsDoc, DumpTomlError> {
    let text = std::fs::read_to_string(path).map_err(|error| DumpTomlError::Read {
        path: path.to_string(),
        error,
    })?;
    toml::from_str(&text).map_err(|error| DumpTomlError::Parse {
        path: path.to_string(),
        error,
    })
}

/// Turn one dump into function bodies plus the real-name oracle. Only
/// resident-image functions (section ROM range inside the physical ROM) are
/// used, matching gate_callgraph_match: the resident image is the shared
/// engine surface. A malformed dump is a loud error, not a silent skip.
pub fn functions_from_dump(
    label: &str,
    rom: &NormalizedRom,
    doc: &SymbolsDoc,
) -> Result<(Vec<FunctionBody>, BTreeMap<u32, String>), DumpTomlError> {
    let mut functions = Vec::new();
    let mut real_name_by_va = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for section in &doc.section {
        if !section.size.is_multiple_of(4) || !section.rom.is_multiple_of(4) {
            return Err(DumpTomlError::SectionNotWordAligned {
                label: label.to_string(),
                section: section.name.clone(),
            });
        }
        let section_end = section.rom.checked_add(section.size).ok_or_else(|| {
            DumpTomlError::SectionExtentOverflows {
                label: label.to_string(),
                section: section.name.clone(),
            }
        })?;
        // Overlay sections whose ROM range is outside the physical image are
        // resident-relative VROM; skip them rather than read garbage.
        if section_end as usize > rom.len() {
            continue;
        }
        for function in &section.functions {
            if !function.size.is_multiple_of(4) {
                return Err(DumpTomlError::FunctionNonWordSize {
                    label: label.to_string(),
                    function: function.name.clone(),
                    size: function.size,
                });
            }
            if function.size == 0 {
                continue;
            }
            let section_offset =
                function
                    .vram
                    .checked_sub(section.vram)
                    .ok_or_else(|| DumpTomlError::FunctionPrecedesSection {
                        label: label.to_string(),
                        function: function.name.clone(),
                        section: section.name.clone(),
                    })?;
            let function_end = section_offset.checked_add(function.size).ok_or_else(|| {
                DumpTomlError::FunctionExtentOverflows {
                    label: label.to_string(),
                    function: function.name.clone(),
                }
            })?;
            if function_end > section.size {
                return Err(DumpTomlError::FunctionExceedsSection {
                    label: label.to_string(),
                    function: function.name.clone(),
                    section: section.name.clone(),
                });
            }
            let rom_start = section.rom.checked_add(section_offset).ok_or_else(|| {
                DumpTomlError::FunctionRomOffsetOverflows {
                    label: label.to_string(),
                    function: function.name.clone(),
                }
            })?;
            let bytes = rom
                .bytes
                .get(rom_start as usize..(rom_start + function.size) as usize)
                .ok_or_else(|| DumpTomlError::FunctionExceedsRom {
                    label: label.to_string(),
                    function: function.name.clone(),
                })?;
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
