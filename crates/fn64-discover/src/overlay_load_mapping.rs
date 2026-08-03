//! Load-only overlay mappings: what a descriptor proves when it carries no
//! section extents.
//!
//! [`crate::overlay_recipe::OverlayLoadRecipeV1`] promotes a table only when
//! every record supplies the full nine-role layout and all range equations
//! agree. Several ROM families never supply that layout. Batman of the Future
//! records a constant destination slot plus an allocation triple unrelated to
//! the loaded image; Bottom of the 9th records a name pointer where a
//! destination would sit, and resolves the address by name at run time. In
//! both, the text/data/bss extents are *absent from the ROM*, not encoded
//! differently.
//!
//! Refusing those tables outright loses a real fact: the ROM interval and, when
//! the descriptor carries one, the load address. This module recovers exactly
//! that fact and nothing more.
//!
//! What it deliberately does NOT do is convert into
//! [`crate::dense_aot_pack::DenseAotGenerationInput`]. That type requires all
//! six section fields and re-validates them, so feeding it a load-only mapping
//! would mean inventing extents and marking them proven -- the degradation
//! `overlay_recipe` exists to refuse. A load-only mapping is evidence for
//! bank composition and reporting; it is not a recompilation input, and the
//! absence of a `From` impl is the mechanism that keeps it out.

use crate::overlay_regions::{CandidateTable, OverlayRecovery};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const OVERLAY_LOAD_MAPPING_SCHEMA_V1: &str = "fn64.overlay-load-mapping.v1";

/// One overlay's proven ROM interval, with its destination when the descriptor
/// supplies one.
///
/// `load_start` is `None` when the table's destination field cannot be a load
/// address -- Bottom of the 9th points it at the overlay's name string, and
/// those pointers pass every per-record plausibility check. A `None` here is a
/// measurement, not a missing value: the ROM interval is still proven, and no
/// consumer may default the address to zero, to the ROM offset, or to anything
/// else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlayLoadMappingV1 {
    pub schema: String,
    pub descriptor_rom_offset: u32,
    pub rom_start: u32,
    pub rom_end: u32,
    pub load_start: Option<u32>,
    pub loaded_sha256: String,
}

impl OverlayLoadMappingV1 {
    pub fn loaded_byte_len(&self) -> u32 {
        self.rom_end - self.rom_start
    }

    /// The destination interval, when the descriptor proved a load address.
    pub fn loaded_range(&self) -> Option<std::ops::Range<u32>> {
        let start = self.load_start?;
        Some(start..start.checked_add(self.loaded_byte_len())?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverlayLoadMappingError {
    NoUniqueAdmittedTable {
        admitted: usize,
    },
    DescriptorAddressOverflow {
        record: usize,
    },
    DescriptorOutsideRom {
        record: usize,
    },
    /// The recovered record disagrees with the descriptor words it names --
    /// the same guard `overlay_recipe` applies, kept here so a weaker product
    /// is not also a less-checked one.
    SourceFieldsChanged {
        record: usize,
    },
    InvalidRomInterval {
        record: usize,
    },
}

/// Whether the table's destination field can be a load address at all.
///
/// Admission proves each destination is a plausible RDRAM address; it does not
/// prove the field means "destination". Bottom of the 9th points that field at
/// the overlay's name string, and those pointers pass every per-record check.
///
/// Two overlays resident at once cannot occupy the same bytes, so real
/// destinations either stay disjoint or repeat one address exactly (a reused
/// slot -- the swapping-engine shape `overlay_regions` already recognizes). A
/// name table satisfies neither: its pointers step by a few bytes per entry
/// while the images are kilobytes, so consecutive images would overlap almost
/// entirely at distinct addresses.
///
/// Returns false only for that partial-overlap shape, so the check costs
/// nothing for tables that are genuinely load descriptors.
fn destinations_are_loadable(table: &CandidateTable) -> bool {
    let mut images: Vec<(u32, u32)> = table
        .records
        .iter()
        .filter_map(|record| {
            let len = record.rom_end.checked_sub(record.rom_start)?;
            Some((record.vram_dest, record.vram_dest.checked_add(len)?))
        })
        .collect();
    images.sort_unstable();
    images.dedup_by_key(|(start, _)| *start);
    // After collapsing reused slots to one entry per distinct start, anything
    // genuinely loadable is disjoint. A swapping engine leaves a single entry
    // and passes trivially; a name table leaves one entry per record, still
    // overlapping, and fails.
    images.windows(2).all(|pair| pair[1].0 >= pair[0].1)
}

/// Recover load-only mappings from one already-recovered table.
///
/// Every record is re-read from the ROM at its descriptor offset and checked
/// against the recovered candidate, so a mapping never reports geometry the
/// descriptor words do not still say.
pub fn parse_overlay_load_mappings_v1(
    rom_bytes: &[u8],
    table: &CandidateTable,
) -> Result<Vec<OverlayLoadMappingV1>, OverlayLoadMappingError> {
    let loadable = destinations_are_loadable(table);
    let mut mappings = Vec::with_capacity(table.records.len());
    for (record_index, candidate) in table.records.iter().enumerate() {
        let record_delta = u32::try_from(record_index)
            .ok()
            .and_then(|index| index.checked_mul(table.record_stride))
            .ok_or(OverlayLoadMappingError::DescriptorAddressOverflow {
                record: record_index,
            })?;
        let descriptor_rom_offset = table.table_rom_offset.checked_add(record_delta).ok_or(
            OverlayLoadMappingError::DescriptorAddressOverflow {
                record: record_index,
            },
        )?;
        let read = |field_offset: u32| -> Result<u32, OverlayLoadMappingError> {
            let offset = descriptor_rom_offset.checked_add(field_offset).ok_or(
                OverlayLoadMappingError::DescriptorAddressOverflow {
                    record: record_index,
                },
            )?;
            rom_bytes
                .get(offset as usize..offset as usize + 4)
                .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
                .ok_or(OverlayLoadMappingError::DescriptorOutsideRom {
                    record: record_index,
                })
        };

        let rom_start = read(table.field_rom_start)?;
        let rom_end = read(table.field_rom_end)?;
        if (rom_start, rom_end) != (candidate.rom_start, candidate.rom_end) {
            return Err(OverlayLoadMappingError::SourceFieldsChanged {
                record: record_index,
            });
        }
        if rom_end <= rom_start || rom_end as usize > rom_bytes.len() {
            return Err(OverlayLoadMappingError::InvalidRomInterval {
                record: record_index,
            });
        }

        // `overlay_regions` has already normalized the destination to a start
        // address under the table's semantics, so the normalized value -- not a
        // re-read of the raw field -- is what a mapping would report.
        //
        // But admission does not prove that field IS a destination. Bottom of
        // the 9th stores a pointer to the overlay's NAME there ("bb2code",
        // "tpitcode"): plausible RDRAM addresses that pass every per-record
        // check, so the table is admitted and `vram_dest` is a string pointer.
        // Reporting it as a load address would be a confident falsehood, which
        // is worse than the refusal this product replaces.
        //
        // Destinations of a real load do not overlap: two overlays resident at
        // once cannot occupy the same bytes, and a reused slot repeats one
        // address exactly rather than sliding by a few bytes. A name table
        // fails both ways at once -- its pointers step by string lengths, so
        // consecutive images at those addresses would overlap almost entirely.
        // `destinations_are_loadable` below tests exactly that, table-wide.
        let load_start = loadable.then_some(candidate.vram_dest);

        let loaded = rom_bytes.get(rom_start as usize..rom_end as usize).ok_or(
            OverlayLoadMappingError::DescriptorOutsideRom {
                record: record_index,
            },
        )?;
        mappings.push(OverlayLoadMappingV1 {
            schema: OVERLAY_LOAD_MAPPING_SCHEMA_V1.to_string(),
            descriptor_rom_offset,
            rom_start,
            rom_end,
            load_start,
            loaded_sha256: format!("{:x}", Sha256::digest(loaded)),
        });
    }
    Ok(mappings)
}

/// Recover load-only mappings from the single admitted physical table.
///
/// Competing admissions stay an explicit failure here exactly as they are for
/// recipes: a weaker product must not also be a laxer one about which table it
/// describes.
pub fn admitted_overlay_load_mappings_v1(
    rom_bytes: &[u8],
    recovery: &OverlayRecovery,
) -> Result<Vec<OverlayLoadMappingV1>, OverlayLoadMappingError> {
    let admitted = recovery
        .admissions
        .iter()
        .filter(|admission| admission.admitted)
        .collect::<Vec<_>>();
    let [admission] = admitted.as_slice() else {
        return Err(OverlayLoadMappingError::NoUniqueAdmittedTable {
            admitted: admitted.len(),
        });
    };
    parse_overlay_load_mappings_v1(rom_bytes, &admission.table)
}

#[cfg(test)]
mod tests;
