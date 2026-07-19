//! OOT decompilation metadata adapter used only as a calibration corpus.
//!
//! The dump is external answer-key evidence. It is never consulted by the
//! normal ROM-only discovery path and never promotes a candidate to proof.

use crate::banks::{
    BankNamePattern, DestinationEnd, DestinationRangeFields, DestinationSpace, LoadImageTableInput,
    LoadImageTableShape, SourceRangeFields, TableLocation,
};
use crate::evidence::ExecutableRangeEvidence;
use crate::facts::RomAddressSpace;
use crate::facts::{Fact, FactDb};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OotDump {
    #[serde(rename = "section")]
    sections: Vec<OotSection>,
}

#[derive(Debug, Deserialize)]
struct OotSection {
    name: String,
    vram: u32,
    size: u32,
    #[serde(default)]
    functions: Vec<OotFunction>,
}

#[derive(Debug, Deserialize)]
struct OotFunction {
    vram: u32,
    size: u32,
}

/// Convert OOT's section/function dump into ROM-bound executable-range
/// candidates. Sections without functions are intentionally excluded because
/// they are assets or opaque data; callers still need to bind the returned
/// ranges to the normalized ROM digest before ingestion.
pub fn executable_ranges_from_oot_dump(
    dump: &str,
    source: &str,
) -> Result<Vec<ExecutableRangeEvidence>, toml::de::Error> {
    let parsed: OotDump = toml::from_str(dump)?;
    Ok(parsed
        .sections
        .into_iter()
        .filter(|section| !section.functions.is_empty())
        .map(|section| {
            let function_start = section
                .functions
                .iter()
                .map(|function| function.vram)
                .min()
                .unwrap_or(section.vram);
            let function_end = section
                .functions
                .iter()
                .map(|function| function.vram.saturating_add(function.size))
                .max()
                .unwrap_or(section.vram.saturating_add(section.size));
            ExecutableRangeEvidence {
                bank: section.name,
                va_start: function_start,
                va_end: function_end.max(function_start),
                source: source.to_owned(),
            }
        })
        .collect())
}

/// OOT NTSC 1.0 load-table geometry used to turn the reference dump's VA
/// ranges into native bank-qualified ranges. These are table shapes, not
/// function answers; the ROM contents still have to satisfy each table.
pub fn oot_load_image_tables() -> [LoadImageTableInput; 5] {
    [
        LoadImageTableInput {
            name: "dmadata".into(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: 0x7430,
                },
                record_count: 0x5f6,
                record_stride: 0x10,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::PhysicalRom,
                    field_start: 8,
                    end: DestinationEnd::FieldOrSourceLength(0x0c),
                },
            },
            bank_name: None,
        },
        LoadImageTableInput {
            name: "effect_overlays".into(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b5_dba0,
                },
                record_count: 0x25,
                record_stride: 0x1c,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 8,
                    end: DestinationEnd::Field(0x0c),
                },
            },
            bank_name: Some(BankNamePattern::new("effect_overlay_", 0, "")),
        },
        LoadImageTableInput {
            name: "actor_overlays".into(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b5_e490,
                },
                record_count: 0x1d7,
                record_stride: 0x20,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 8,
                    end: DestinationEnd::Field(0x0c),
                },
            },
            bank_name: Some(BankNamePattern::new("actor_overlay_", 0, "")),
        },
        LoadImageTableInput {
            name: "gamestate_overlays".into(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b6_72a0,
                },
                record_count: 6,
                record_stride: 0x30,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 4,
                    field_end: 8,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 0x0c,
                    end: DestinationEnd::Field(0x10),
                },
            },
            bank_name: Some(BankNamePattern::new("gamestate_overlay_", 0, "")),
        },
        LoadImageTableInput {
            name: "kaleido_overlays".into(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x00b7_43e0,
                },
                record_count: 2,
                record_stride: 0x1c,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 4,
                    field_end: 8,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 0x0c,
                    end: DestinationEnd::Field(0x10),
                },
            },
            bank_name: Some(BankNamePattern::new("kaleido_overlay_", 0, "")),
        },
    ]
}

/// Bind OOT section/function ranges to the native mappings already recovered
/// from the ROM. A section is usable only when one and only one proven bank
/// contains its complete VA extent; this prevents answer-key ranges from
/// bypassing bank identity or overlay ambiguity.
pub fn bind_ranges_to_fact_db(
    dump: &str,
    source: &str,
    db: &FactDb,
) -> Result<Vec<ExecutableRangeEvidence>, String> {
    let (bound, unresolved) = bind_ranges_to_fact_db_partial(dump, source, db)?;
    if let Some(first) = unresolved.first() {
        return Err(first.clone());
    }
    Ok(bound)
}

/// Best-effort binding for calibration gates. Exact native owners are retained
/// while unresolved ranges remain visible diagnostics.
pub fn bind_ranges_to_fact_db_partial(
    dump: &str,
    source: &str,
    db: &FactDb,
) -> Result<(Vec<ExecutableRangeEvidence>, Vec<String>), String> {
    let ranges = executable_ranges_from_oot_dump(dump, source)
        .map_err(|error| format!("parsing OOT dump: {error}"))?;
    let mappings = db.proven_rom_mappings();
    let mut bound = Vec::with_capacity(ranges.len());
    let mut unresolved = Vec::new();
    for range in ranges {
        let matches = mappings
            .iter()
            .filter_map(|fact| match fact {
                Fact::RomMapping {
                    bank,
                    va_start,
                    va_end,
                    ..
                } if range.va_start >= *va_start && range.va_end <= *va_end => Some(bank.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [bank] => bound.push(ExecutableRangeEvidence {
                bank: bank.clone(),
                ..range
            }),
            [] => {
                unresolved.push(format!(
                    "dump section {} range {}..{} has no proven mapping",
                    range.bank, range.va_start, range.va_end
                ));
            }
            many => {
                unresolved.push(format!(
                    "dump section {} range {}..{} has {} proven mappings",
                    range.bank,
                    range.va_start,
                    range.va_end,
                    many.len()
                ));
            }
        }
    }
    Ok((bound, unresolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_function_bearing_sections_become_ranges() {
        let dump = r#"
            [[section]]
            name = "text"
            rom = 0x1000
            vram = 0x80000400
            size = 0x100
            functions = [
                { name = "a", vram = 0x80000420, size = 0x10 },
                { name = "b", vram = 0x80000430, size = 0x20 },
            ]

            [[section]]
            name = "assets"
            rom = 0x2000
            vram = 0x80001000
            size = 0x100
        "#;
        let ranges = executable_ranges_from_oot_dump(dump, "oot-test").unwrap();
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].bank, "text");
        assert_eq!(ranges[0].va_start, 0x8000_0420);
        assert_eq!(ranges[0].va_end, 0x8000_0450);
        assert_eq!(ranges[0].source, "oot-test");
    }

    #[test]
    fn binding_requires_one_complete_mapping() {
        let dump = r#"
            [[section]]
            name = "text"
            rom = 0x1000
            vram = 0x80000400
            size = 0x40
            functions = [{ name = "a", vram = 0x80000400, size = 0x20 }]
        "#;
        let mut db = FactDb::new();
        let id = db.insert(Fact::RomMapping {
            bank: "boot".to_string(),
            rom_space: crate::facts::RomAddressSpace::Physical,
            rom_start: 0x1000,
            rom_end: 0x2000,
            va_start: 0x80000400,
            va_end: 0x80001400,
        });
        db.conclude(
            "bank:boot",
            crate::facts::ProofState::Proven,
            vec![id],
            "test",
        )
        .unwrap();
        let ranges = bind_ranges_to_fact_db(dump, "oot-test", &db).unwrap();
        assert_eq!(ranges[0].bank, "boot");
    }
}
