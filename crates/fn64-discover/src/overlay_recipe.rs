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
    /// SHA-256 of the TEXT extent only, i.e. `[text_start, text_end)`.
    ///
    /// `loaded_sha256` covers the whole loaded image, which includes the
    /// overlay's data section. A correct program writes its own data at
    /// runtime -- WM2000 stores four bytes at `0x80107efc`, inside overlay 0's
    /// `0x9eb0`-byte data span -- so a whole-image digest cannot survive
    /// execution. Only the text extent is immutable, so only it can identify a
    /// generation across time.
    pub text_sha256: String,
}

/// Shard granularity the dense-AOT pack tiles generations with.
pub const DENSE_SHARD_BYTES: u32 = 64 * 1024;

/// How many ROM bytes of an overlay a GENERATION covers.
///
/// A generation covers the overlay's TEXT, not its whole loaded image: the
/// data section is mutable, and a correct program writes it at runtime, so a
/// generation digested over it cannot survive execution. WM2000 stores four
/// bytes at VA `0x80107efc` inside overlay 0's data span, which invalidated
/// the whole generation and stopped the certified route.
///
/// This is EXACTLY the text length -- never rounded to a shard boundary.
///
/// Rounding up to whole shards was the earlier rule, on the belief that a
/// generation's image had to end on a shard boundary for the shard list to
/// tile. It does not. `PrecompiledGeneration::new`
/// (`fn64-cpu-runtime/src/generation/mod.rs:109-126`) requires only that shards
/// tile contiguously from `image_start` and COVER `image_end`; the final shard
/// may legitimately overhang, because "the digest covers
/// `[image_start, image_end)` only, which is precisely why a generation may end
/// mid-shard without weakening what it asserts."
///
/// Rounding up therefore bought nothing and cost correctness: it pulled the
/// overlay's own mutable DATA back inside the digested extent, which is the
/// exact failure this function exists to prevent. On WM2000 it put `0x2c80`
/// data bytes into overlay 0's digest and `0x5550` into overlay 3's. Once the
/// guest wrote its own data there, re-entering that overlay's TEXT could never
/// re-activate: the certified route died at `0x800E1FAC` -- an address inside
/// overlay 0's text -- because all three generations containing it digested
/// bytes the guest had legitimately written.
///
/// Every consumer derives from this one function so they cannot disagree --
/// the dense pack, the topology, the runtime catalog and the emitted pack all
/// fold shard extents into digests that must match.
pub fn generation_source_span(recipe: &OverlayLoadRecipeV1) -> u32 {
    recipe.text_end - recipe.load_start
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
            text_sha256: {
                // text_start/text_end are VIRTUAL; convert to the ROM window
                // the loaded slice already represents.
                // Must span exactly what a GENERATION covers, which is the
                // bare text extent. A generation folds this digest into its
                // identity, so any disagreement here would never match the
                // bytes the generation admits.
                //
                // Computed inline rather than via `generation_source_span`
                // because the recipe is still being constructed here. It must
                // stay identical to that function: the exact text length, with
                // no shard rounding, so the overlay's mutable data never lands
                // inside the digested extent.
                let span = text_end - load_start;
                let text_rom_start = rom_start;
                let text_rom_end = rom_start + span;
                let text = rom_bytes
                    .get(text_rom_start as usize..text_rom_end as usize)
                    .ok_or(OverlayRecipeError::DescriptorOutsideRom {
                        record: record_index,
                    })?;
                format!("{:x}", Sha256::digest(text))
            },
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
