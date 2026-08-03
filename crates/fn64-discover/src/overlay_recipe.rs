//! Typed materialization recipes for the public libultra overlay sequence.
//!
//! A recovered three-field table proves only a ROM interval and load address.
//! AKI-family records carry the complete nine-word layout described by the
//! public overlay-loading sequence: ROM bounds, loaded image start, text/data
//! cache extents, and the zeroed BSS extent. This module promotes that wider
//! shape only when every word is present and all independent range equations
//! agree. A malformed or ambiguous table fails loudly; it never degrades to a
//! simple linear mapping.

use crate::overlay_regions::{CandidateTable, OverlayRecovery};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const OVERLAY_RECIPE_SCHEMA_V1: &str = "fn64.overlay-load-recipe.v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayLoadRecipeV1 {
    pub schema: String,
    pub descriptor_rom_offset: u32,
    pub rom_start: u32,
    pub rom_end: u32,
    pub load_start: u32,
    pub text_start: u32,
    pub text_end: u32,
    pub data_start: u32,
    pub data_end: u32,
    pub bss_start: u32,
    pub bss_end: u32,
    pub loaded_sha256: String,
}

impl OverlayLoadRecipeV1 {
    pub fn loaded_byte_len(&self) -> u32 {
        self.rom_end - self.rom_start
    }

    pub fn executable_byte_len(&self) -> u32 {
        self.text_end - self.text_start
    }

    pub fn executable_range(&self) -> std::ops::Range<u32> {
        self.text_start..self.text_end
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayRecipeError {
    NoUniqueAdmittedTable { admitted: usize },
    DescriptorAddressOverflow { record: usize },
    DescriptorOutsideRom { record: usize },
    SourceFieldsChanged { record: usize },
    UnalignedField { record: usize, value: u32 },
    InvalidRangeRelations { record: usize },
    LoadedExtentMismatch { record: usize },
}

fn read_u32(bytes: &[u8], offset: u32) -> Option<u32> {
    bytes
        .get(offset as usize..offset as usize + 4)
        .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
}

/// Parse the complete nine-role records from one already-recovered table.
/// The table's phase-adjusted `field_rom_start` identifies the first semantic
/// word; consecutive semantic records remain exactly `record_stride` apart.
pub fn parse_overlay_load_recipes_v1(
    rom_bytes: &[u8],
    table: &CandidateTable,
) -> Result<Vec<OverlayLoadRecipeV1>, OverlayRecipeError> {
    let mut recipes = Vec::with_capacity(table.records.len());
    for (record_index, candidate) in table.records.iter().enumerate() {
        let record_delta = u32::try_from(record_index)
            .ok()
            .and_then(|index| index.checked_mul(table.record_stride))
            .ok_or(OverlayRecipeError::DescriptorAddressOverflow {
                record: record_index,
            })?;
        let descriptor_rom_offset = table
            .table_rom_offset
            .checked_add(record_delta)
            .and_then(|base| base.checked_add(table.field_rom_start))
            .ok_or(OverlayRecipeError::DescriptorAddressOverflow {
                record: record_index,
            })?;
        let mut fields = [0u32; 9];
        for (field_index, field) in fields.iter_mut().enumerate() {
            let offset = descriptor_rom_offset
                .checked_add(field_index as u32 * 4)
                .ok_or(OverlayRecipeError::DescriptorAddressOverflow {
                    record: record_index,
                })?;
            *field =
                read_u32(rom_bytes, offset).ok_or(OverlayRecipeError::DescriptorOutsideRom {
                    record: record_index,
                })?;
        }
        let [rom_start, rom_end, load_start, text_start, text_end, data_start, data_end, bss_start, bss_end] =
            fields;
        if (rom_start, rom_end, load_start)
            != (candidate.rom_start, candidate.rom_end, candidate.vram_dest)
        {
            return Err(OverlayRecipeError::SourceFieldsChanged {
                record: record_index,
            });
        }
        if let Some(value) = fields
            .iter()
            .copied()
            .find(|value| !value.is_multiple_of(4))
        {
            return Err(OverlayRecipeError::UnalignedField {
                record: record_index,
                value,
            });
        }
        if load_start != text_start
            || text_start >= text_end
            || text_end != data_start
            || data_start > data_end
            || data_end != bss_start
            || bss_start > bss_end
        {
            return Err(OverlayRecipeError::InvalidRangeRelations {
                record: record_index,
            });
        }
        if rom_end.checked_sub(rom_start) != data_end.checked_sub(load_start) {
            return Err(OverlayRecipeError::LoadedExtentMismatch {
                record: record_index,
            });
        }
        let loaded = rom_bytes.get(rom_start as usize..rom_end as usize).ok_or(
            OverlayRecipeError::DescriptorOutsideRom {
                record: record_index,
            },
        )?;
        recipes.push(OverlayLoadRecipeV1 {
            schema: OVERLAY_RECIPE_SCHEMA_V1.to_string(),
            descriptor_rom_offset,
            rom_start,
            rom_end,
            load_start,
            text_start,
            text_end,
            data_start,
            data_end,
            bss_start,
            bss_end,
            loaded_sha256: format!("{:x}", Sha256::digest(loaded)),
        });
    }
    Ok(recipes)
}

/// Recover recipes only from the single admitted physical table. Competing
/// admissions remain an explicit closure failure.
pub fn admitted_overlay_load_recipes_v1(
    rom_bytes: &[u8],
    recovery: &OverlayRecovery,
) -> Result<Vec<OverlayLoadRecipeV1>, OverlayRecipeError> {
    let admitted = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .collect::<Vec<_>>();
    let [admission] = admitted.as_slice() else {
        return Err(OverlayRecipeError::NoUniqueAdmittedTable {
            admitted: admitted.len(),
        });
    };
    parse_overlay_load_recipes_v1(rom_bytes, &admission.table)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_regions::CandidateRecord;

    fn put(words: &mut [u8], offset: usize, value: u32) {
        words[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn parses_phase_adjusted_complete_recipe() {
        let mut rom = vec![0u8; 0x5000];
        let table = CandidateTable {
            table_rom_offset: 0x100,
            record_stride: 0x24,
            field_rom_start: 0x18,
            field_rom_end: 0x1c,
            field_vram_dest: 0x20,
            destination_field: crate::overlay_regions::DestinationFieldSemantics::Start,
            records: vec![CandidateRecord {
                rom_start: 0x2000,
                rom_end: 0x2040,
                vram_dest: 0x8000_1000,
            }],
        };
        for (index, value) in [
            0x2000,
            0x2040,
            0x8000_1000,
            0x8000_1000,
            0x8000_1020,
            0x8000_1020,
            0x8000_1040,
            0x8000_1040,
            0x8000_1080,
        ]
        .into_iter()
        .enumerate()
        {
            put(&mut rom, 0x118 + index * 4, value);
        }
        let recipes = parse_overlay_load_recipes_v1(&rom, &table).unwrap();
        assert_eq!(recipes[0].descriptor_rom_offset, 0x118);
        assert_eq!(recipes[0].executable_range(), 0x8000_1000..0x8000_1020);
        assert_eq!(recipes[0].loaded_byte_len(), 0x40);
    }

    #[test]
    fn rejects_loaded_extent_that_disagrees_with_rom() {
        let mut rom = vec![0u8; 0x5000];
        let table = CandidateTable {
            table_rom_offset: 0x100,
            record_stride: 0x24,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
            destination_field: crate::overlay_regions::DestinationFieldSemantics::Start,
            records: vec![CandidateRecord {
                rom_start: 0x2000,
                rom_end: 0x2040,
                vram_dest: 0x8000_1000,
            }],
        };
        for (index, value) in [
            0x2000,
            0x2040,
            0x8000_1000,
            0x8000_1000,
            0x8000_1020,
            0x8000_1020,
            0x8000_1030,
            0x8000_1030,
            0x8000_1080,
        ]
        .into_iter()
        .enumerate()
        {
            put(&mut rom, 0x100 + index * 4, value);
        }
        assert_eq!(
            parse_overlay_load_recipes_v1(&rom, &table),
            Err(OverlayRecipeError::LoadedExtentMismatch { record: 0 })
        );
    }
}
