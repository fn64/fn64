//! Mechanical recovery of a physical-ROM file table and VROM materialization.
//!
//! The recovered family is the public N64 DMA-file-table shape: aligned
//! `(vrom_start, vrom_end, rom_start, rom_end)` records stored in physical
//! cartridge ROM. Recovery enumerates record strides and field phases, then
//! validates a maximal run. VROM intervals must be strictly ordered and
//! contiguous modulo an enumerated alignment-padding boundary, every physical
//! backing must fit in the normalized ROM, and the run must begin with the
//! identity mapping (`vrom_start = rom_start = 0`).
//! Distinct phase aliases are canonicalized by their mappings. No table is
//! admitted unless exactly one distinct mapping sequence survives.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTableSearchConfig {
    /// Candidate record strides. Each usable stride is word aligned and has
    /// room for the four adjacent fields.
    pub strides: Vec<u32>,
    /// File starts may advance over alignment padding after the preceding
    /// logical VROM end. Each candidate alignment is enumerated, never
    /// inferred by score.
    pub vrom_alignments: Vec<u32>,
    /// A shorter run remains a candidate accident, not a file table.
    pub min_records: u32,
}

impl FileTableSearchConfig {
    pub fn n64_family() -> Self {
        Self {
            strides: vec![
                0x10, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x38, 0x40,
            ],
            vrom_alignments: vec![4, 0x10, 0x100, 0x1000],
            min_records: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileTableRecord {
    pub vrom_start: u32,
    pub vrom_end: u32,
    pub rom_start: u32,
    /// Zero is the file-table convention for an uncompressed equal-length
    /// physical backing.
    pub rom_end: u32,
}

impl FileTableRecord {
    pub fn vrom_len(self) -> u32 {
        self.vrom_end - self.vrom_start
    }

    pub fn physical_end(self) -> Option<u32> {
        if self.rom_end == 0 {
            self.rom_start.checked_add(self.vrom_len())
        } else {
            Some(self.rom_end)
        }
    }

    pub fn contains_vrom(self, start: u32, end: u32) -> bool {
        start >= self.vrom_start && end <= self.vrom_end && end > start
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateFileTable {
    pub table_rom_offset: u32,
    pub record_stride: u32,
    pub vrom_alignment: u32,
    pub field_vrom_start: u32,
    pub field_vrom_end: u32,
    pub field_rom_start: u32,
    pub field_rom_end: u32,
    pub records: Vec<FileTableRecord>,
}

impl CandidateFileTable {
    fn identity(&self) -> Vec<FileTableRecord> {
        self.records.clone()
    }

    pub fn max_vrom_end(&self) -> u32 {
        self.records.last().map_or(0, |record| record.vrom_end)
    }

    pub fn contains_vrom_range(&self, start: u32, end: u32) -> bool {
        self.records
            .iter()
            .copied()
            .any(|record| record.contains_vrom(start, end))
    }

    /// Resolve a VROM address to a physical byte offset when its containing
    /// file is stored uncompressed. Compressed files require materialization
    /// and deliberately return `None` here rather than pretending their byte
    /// positions are linearly related.
    pub fn translate_uncompressed(&self, vrom: u32) -> Option<u32> {
        let record = self
            .records
            .iter()
            .copied()
            .find(|record| (record.vrom_start..record.vrom_end).contains(&vrom))?;
        if record.rom_end != 0 {
            return None;
        }
        record
            .rom_start
            .checked_add(vrom.checked_sub(record.vrom_start)?)
    }

    /// Materialize one VROM range through exactly one recovered file record.
    /// Uncompressed files are sliced directly; Yaz0 is decoded with bounded
    /// input/output accesses and its declared size must equal the VROM file.
    pub fn materialize_vrom_range(
        &self,
        rom_bytes: &[u8],
        start: u32,
        end: u32,
    ) -> Result<Vec<u8>, String> {
        let matches: Vec<_> = self
            .records
            .iter()
            .copied()
            .filter(|record| record.contains_vrom(start, end))
            .collect();
        let [record] = matches.as_slice() else {
            return Err(format!(
                "VROM range [0x{start:x},0x{end:x}) has {} recovered file mappings; expected exactly one",
                matches.len()
            ));
        };
        let physical_end = record
            .physical_end()
            .ok_or_else(|| "file backing end overflowed u32".to_string())?;
        let physical = rom_bytes
            .get(record.rom_start as usize..physical_end as usize)
            .ok_or_else(|| "file backing exceeds normalized ROM".to_string())?;
        let expected_len = record.vrom_len() as usize;
        let file = if physical.starts_with(b"Yaz0") {
            decompress_yaz0(physical, expected_len)?
        } else {
            if physical.len() != expected_len {
                return Err(format!(
                    "non-Yaz0 backing length 0x{:x} differs from VROM length 0x{expected_len:x}",
                    physical.len()
                ));
            }
            physical.to_vec()
        };
        let relative_start = (start - record.vrom_start) as usize;
        let relative_end = (end - record.vrom_start) as usize;
        Ok(file[relative_start..relative_end].to_vec())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTableRecovery {
    pub config: FileTableSearchConfig,
    pub candidate_tables: Vec<CandidateFileTable>,
    /// Set only when one distinct mapping sequence survives enumeration.
    pub admitted_table: Option<CandidateFileTable>,
}

fn read_u32_be(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn record_at(bytes: &[u8], fields_start: u32) -> Option<FileTableRecord> {
    let start = fields_start as usize;
    Some(FileTableRecord {
        vrom_start: read_u32_be(bytes, start)?,
        vrom_end: read_u32_be(bytes, start + 4)?,
        rom_start: read_u32_be(bytes, start + 8)?,
        rom_end: read_u32_be(bytes, start + 12)?,
    })
}

/// The dmadata convention for a file present in the VROM address space but
/// with NO physical backing in this build: both physical fields are all-ones.
pub const NO_PHYSICAL_BACKING: u32 = 0xFFFF_FFFF;

/// Does this record declare a VROM extent with no bytes behind it?
pub fn has_no_physical_backing(record: FileTableRecord) -> bool {
    record.rom_start == NO_PHYSICAL_BACKING && record.rom_end == NO_PHYSICAL_BACKING
}

fn record_valid(
    record: FileTableRecord,
    previous_vrom_end: u32,
    vrom_alignment: u32,
    rom_len: u32,
) -> bool {
    let aligned_start = align_up(previous_vrom_end, vrom_alignment);
    let chains = record.vrom_start == previous_vrom_end || aligned_start == Some(record.vrom_start);
    let vrom_well_formed = chains
        && record.vrom_end > record.vrom_start
        && record.vrom_start.is_multiple_of(4)
        && record.vrom_end.is_multiple_of(4);

    // An unbacked file still OWNS its VROM interval: every later record chains
    // from its `vrom_end`. Rejecting it does not skip one entry, it truncates
    // the table there -- which is exactly what happened to Majora's Mask,
    // whose records 8 and 9 are unbacked. The table stopped at 8 records
    // instead of ~1500, so `code` was never reached and no overlay descriptor
    // table could be found in a ROM that plainly has them.
    //
    // Validate the VROM extent and accept; the record contributes no physical
    // mapping, and `materialize_vrom_range` still refuses to produce bytes for
    // it, so nothing downstream can mistake it for backed content.
    if has_no_physical_backing(record) {
        return vrom_well_formed;
    }

    if !vrom_well_formed
        || !record.rom_start.is_multiple_of(4)
        || record.rom_start >= rom_len
    {
        return false;
    }
    let Some(physical_end) = record.physical_end() else {
        return false;
    };
    physical_end > record.rom_start && physical_end <= rom_len && physical_end.is_multiple_of(4)
}

fn align_up(value: u32, alignment: u32) -> Option<u32> {
    if !alignment.is_power_of_two() || alignment < 4 {
        return None;
    }
    value
        .checked_add(alignment - 1)
        .map(|sum| sum & !(alignment - 1))
}

/// Enumerate physical file-table candidates, canonicalize phase aliases, and
/// admit only one distinct mapping sequence.
pub fn recover_file_table(rom_bytes: &[u8], config: &FileTableSearchConfig) -> FileTableRecovery {
    let rom_len = rom_bytes.len() as u32;
    let mut identity_field_positions = Vec::new();
    let mut fields_start = 0u32;
    while fields_start <= rom_len.saturating_sub(16) {
        if record_at(rom_bytes, fields_start).is_some_and(|record| {
            record.vrom_start == 0 && record.rom_start == 0 && record_valid(record, 0, 4, rom_len)
        }) {
            identity_field_positions.push(fields_start);
        }
        fields_start += 4;
    }

    let mut raw = Vec::new();
    for &identity_fields in &identity_field_positions {
        for &stride in &config.strides {
            if stride < 16 || !stride.is_multiple_of(4) {
                continue;
            }
            for &vrom_alignment in &config.vrom_alignments {
                if align_up(0, vrom_alignment).is_none() {
                    continue;
                }
                let mut field_vrom_start = 0u32;
                while field_vrom_start <= stride - 16 {
                    let Some(table_rom_offset) = identity_fields.checked_sub(field_vrom_start)
                    else {
                        field_vrom_start += 4;
                        continue;
                    };
                    let mut records = Vec::new();
                    let mut previous_vrom_end = 0u32;
                    let mut record_fields = identity_fields;
                    while let Some(record) = record_at(rom_bytes, record_fields) {
                        if !record_valid(record, previous_vrom_end, vrom_alignment, rom_len) {
                            break;
                        }
                        previous_vrom_end = record.vrom_end;
                        records.push(record);
                        let Some(next) = record_fields.checked_add(stride) else {
                            break;
                        };
                        record_fields = next;
                    }
                    if records.len() as u32 >= config.min_records {
                        raw.push(CandidateFileTable {
                            table_rom_offset,
                            record_stride: stride,
                            vrom_alignment,
                            field_vrom_start,
                            field_vrom_end: field_vrom_start + 4,
                            field_rom_start: field_vrom_start + 8,
                            field_rom_end: field_vrom_start + 12,
                            records,
                        });
                    }
                    field_vrom_start += 4;
                }
            }
        }
    }

    // Prefer the conventional phase-zero representative when several table
    // bases address the same four fields. Identity is the mapping sequence,
    // not an arbitrary pre-record phase.
    raw.sort_by(|a, b| {
        b.records.len().cmp(&a.records.len()).then(
            a.field_vrom_start
                .cmp(&b.field_vrom_start)
                .then(a.table_rom_offset.cmp(&b.table_rom_offset))
                .then(a.record_stride.cmp(&b.record_stride)),
        )
    });
    let mut seen = BTreeSet::new();
    let mut candidate_tables = Vec::new();
    for table in raw {
        if seen.insert(table.identity()) {
            candidate_tables.push(table);
        }
    }
    let identities: Vec<_> = candidate_tables
        .iter()
        .map(CandidateFileTable::identity)
        .collect();
    candidate_tables = candidate_tables
        .into_iter()
        .enumerate()
        .filter_map(|(index, table)| {
            let is_strict_prefix = identities.iter().enumerate().any(|(other_index, other)| {
                index != other_index
                    && other.len() > table.records.len()
                    && other.starts_with(&table.records)
            });
            (!is_strict_prefix).then_some(table)
        })
        .collect();
    candidate_tables.sort_by_key(|table| (table.table_rom_offset, table.record_stride));
    let admitted_table = match candidate_tables.as_slice() {
        [table] => Some(table.clone()),
        _ => None,
    };
    FileTableRecovery {
        config: config.clone(),
        candidate_tables,
        admitted_table,
    }
}

/// Bounded Yaz0 decoder matching the public stream format used by N64 load
/// images. Malformed streams stay unavailable rather than yielding partial
/// bytes.
fn decompress_yaz0(input: &[u8], expected_len: usize) -> Result<Vec<u8>, String> {
    if input.len() < 16 || &input[..4] != b"Yaz0" {
        return Err("missing or truncated Yaz0 header".into());
    }
    let declared = u32::from_be_bytes(input[4..8].try_into().unwrap()) as usize;
    if declared != expected_len {
        return Err(format!(
            "Yaz0 output length 0x{declared:x} differs from expected VROM length 0x{expected_len:x}"
        ));
    }
    let mut source = 16usize;
    let mut output = Vec::with_capacity(expected_len);
    let mut control = 0u8;
    let mut bits_left = 0u8;
    while output.len() < expected_len {
        if bits_left == 0 {
            control = *input
                .get(source)
                .ok_or_else(|| "Yaz0 stream ended before a control byte".to_string())?;
            source += 1;
            bits_left = 8;
        }
        if control & 0x80 != 0 {
            output.push(
                *input
                    .get(source)
                    .ok_or_else(|| "Yaz0 literal exceeds input".to_string())?,
            );
            source += 1;
        } else {
            let first = *input
                .get(source)
                .ok_or_else(|| "Yaz0 back-reference exceeds input".to_string())?;
            let second = *input
                .get(source + 1)
                .ok_or_else(|| "Yaz0 back-reference exceeds input".to_string())?;
            source += 2;
            let distance = ((((first as usize) & 0x0f) << 8) | second as usize) + 1;
            let mut length = (first as usize) >> 4;
            if length == 0 {
                length = (*input
                    .get(source)
                    .ok_or_else(|| "Yaz0 extended length exceeds input".to_string())?
                    as usize)
                    + 0x12;
                source += 1;
            } else {
                length += 2;
            }
            if distance > output.len() {
                return Err("Yaz0 back-reference precedes output".into());
            }
            for _ in 0..length {
                if output.len() == expected_len {
                    break;
                }
                let value = output[output.len() - distance];
                output.push(value);
            }
        }
        control <<= 1;
        bits_left -= 1;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn plant_record(bytes: &mut [u8], offset: usize, record: FileTableRecord) {
        put_u32(bytes, offset, record.vrom_start);
        put_u32(bytes, offset + 4, record.vrom_end);
        put_u32(bytes, offset + 8, record.rom_start);
        put_u32(bytes, offset + 12, record.rom_end);
    }

    #[test]
    fn synthetic_contiguous_file_table_is_uniquely_recovered() {
        let mut rom = vec![0xff; 0x20_000];
        let records = [
            FileTableRecord {
                vrom_start: 0,
                vrom_end: 0x900,
                rom_start: 0,
                rom_end: 0,
            },
            FileTableRecord {
                vrom_start: 0x1000,
                vrom_end: 0x2800,
                rom_start: 0x4000,
                rom_end: 0,
            },
            FileTableRecord {
                vrom_start: 0x3000,
                vrom_end: 0x5000,
                rom_start: 0x8000,
                rom_end: 0,
            },
        ];
        for (index, record) in records.into_iter().enumerate() {
            plant_record(&mut rom, 0x2000 + index * 0x10, record);
        }
        let recovery = recover_file_table(&rom, &FileTableSearchConfig::n64_family());
        let table = recovery.admitted_table.expect("one file table");
        assert_eq!(table.table_rom_offset, 0x2000);
        assert_eq!(table.record_stride, 0x10);
        assert_eq!(table.vrom_alignment, 0x1000);
        assert_eq!(table.field_vrom_start, 0);
        assert_eq!(table.records, records);
        assert_eq!(table.translate_uncompressed(0x3120), Some(0x8120));
    }

    #[test]
    fn an_unbacked_file_does_not_truncate_the_table() {
        // A file present in VROM with no physical backing writes all-ones into
        // both physical fields. It still OWNS its VROM interval, so every later
        // record chains from its end -- rejecting it truncates the table there
        // rather than skipping one entry.
        //
        // Measured consequence before this was handled: Majora's Mask records 8
        // and 9 are unbacked, so its table stopped at 8 records instead of
        // ~1500. `code` was never reached, no overlay descriptor table could be
        // found, and a ROM that plainly has them composed to the boot bank
        // alone.
        let mut rom = vec![0xff; 0x20_000];
        let records = [
            FileTableRecord {
                vrom_start: 0,
                vrom_end: 0x900,
                rom_start: 0,
                rom_end: 0,
            },
            FileTableRecord {
                vrom_start: 0x1000,
                vrom_end: 0x2800,
                rom_start: 0x4000,
                rom_end: 0,
            },
            FileTableRecord {
                vrom_start: 0x3000,
                vrom_end: 0x5000,
                rom_start: NO_PHYSICAL_BACKING,
                rom_end: NO_PHYSICAL_BACKING,
            },
            // The record that would be lost: it chains from the unbacked
            // file's vrom_end, so it is unreachable unless the run continues.
            FileTableRecord {
                vrom_start: 0x5000,
                vrom_end: 0x6000,
                rom_start: 0x8000,
                rom_end: 0,
            },
        ];
        for (index, record) in records.into_iter().enumerate() {
            plant_record(&mut rom, 0x2000 + index * 0x10, record);
        }
        let recovery = recover_file_table(&rom, &FileTableSearchConfig::n64_family());
        let table = recovery.admitted_table.expect("one file table");

        assert_eq!(table.records, records, "the run must span the unbacked file");
        assert!(has_no_physical_backing(table.records[2]));
        // The unbacked file yields no bytes, so nothing downstream can mistake
        // it for backed content.
        assert!(table
            .materialize_vrom_range(&rom, 0x3000, 0x5000)
            .is_err());
        // And the record after it is reachable, which is the whole point.
        assert_eq!(table.translate_uncompressed(0x5100), Some(0x8100));
    }

    #[test]
    fn non_contiguous_vrom_run_is_rejected() {
        let mut rom = vec![0xff; 0x20_000];
        let records = [
            FileTableRecord {
                vrom_start: 0,
                vrom_end: 0x1000,
                rom_start: 0,
                rom_end: 0,
            },
            FileTableRecord {
                vrom_start: 0x2000,
                vrom_end: 0x3000,
                rom_start: 0x4000,
                rom_end: 0,
            },
            FileTableRecord {
                vrom_start: 0x3000,
                vrom_end: 0x4000,
                rom_start: 0x5000,
                rom_end: 0,
            },
        ];
        for (index, record) in records.into_iter().enumerate() {
            plant_record(&mut rom, 0x2000 + index * 0x10, record);
        }
        let recovery = recover_file_table(&rom, &FileTableSearchConfig::n64_family());
        assert!(recovery.candidate_tables.is_empty());
        assert!(recovery.admitted_table.is_none());
    }
}
