use super::*;
use crate::overlay_regions::{CandidateRecord, DestinationFieldSemantics};

fn put(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// A table whose descriptor carries only `[rom_start, rom_end, dest]` -- the
/// shape that has no section extents to promote.
fn load_only_rom() -> (Vec<u8>, CandidateTable) {
    let mut rom = vec![0u8; 0x5000];
    // Distinct content per record. `index as u8` would NOT do: two 0x100-byte
    // ranges would each run 0x00..=0xff and digest identically, so the
    // per-record digest test would pass without proving anything.
    for (index, byte) in rom.iter_mut().enumerate().skip(0x2000).take(0x200) {
        *byte = (index as u8) ^ ((index >> 8) as u8);
    }
    let table = CandidateTable {
        table_rom_offset: 0x100,
        record_stride: 0x10,
        field_rom_start: 0x0,
        field_rom_end: 0x4,
        field_vram_dest: 0x8,
        destination_field: DestinationFieldSemantics::Start,
        records: vec![
            CandidateRecord {
                rom_start: 0x2000,
                rom_end: 0x2100,
                vram_dest: 0x8010_0000,
            },
            CandidateRecord {
                rom_start: 0x2100,
                rom_end: 0x2200,
                vram_dest: 0x8010_0100,
            },
        ],
    };
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base, record.rom_start);
        put(&mut rom, base + 4, record.rom_end);
        put(&mut rom, base + 8, record.vram_dest);
    }
    (rom, table)
}

#[test]
fn recovers_the_interval_and_destination_a_descriptor_actually_proves() {
    let (rom, table) = load_only_rom();

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert_eq!(mappings.len(), 2);
    assert_eq!(mappings[0].rom_start, 0x2000);
    assert_eq!(mappings[0].rom_end, 0x2100);
    assert_eq!(mappings[0].load_start, Some(0x8010_0000));
    assert_eq!(mappings[0].loaded_byte_len(), 0x100);
    assert_eq!(mappings[0].loaded_range(), Some(0x8010_0000..0x8010_0100));
    assert_eq!(mappings[0].schema, OVERLAY_LOAD_MAPPING_SCHEMA_V1);
}

/// The digest is over the loaded bytes, so two records covering different ROM
/// intervals cannot silently share provenance.
#[test]
fn each_mapping_digests_its_own_loaded_image() {
    let (rom, table) = load_only_rom();

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert_ne!(mappings[0].loaded_sha256, mappings[1].loaded_sha256);
    assert_eq!(mappings[0].loaded_sha256.len(), 64);
}

/// A weaker product must not be a less-checked one: if the ROM words no longer
/// say what the recovered record says, that is a failure, not a mapping.
#[test]
fn a_record_disagreeing_with_its_descriptor_words_is_refused() {
    let (mut rom, table) = load_only_rom();
    put(&mut rom, table.table_rom_offset as usize, 0xdead_0000);

    let error = parse_overlay_load_mappings_v1(&rom, &table).unwrap_err();

    assert_eq!(
        error,
        OverlayLoadMappingError::SourceFieldsChanged { record: 0 }
    );
}

#[test]
fn an_empty_or_reversed_rom_interval_is_refused() {
    let (mut rom, mut table) = load_only_rom();
    // Bottom of the 9th's last record has rom_start == rom_end.
    table.records[0].rom_end = table.records[0].rom_start;
    put(
        &mut rom,
        table.table_rom_offset as usize + 4,
        table.records[0].rom_end,
    );

    let error = parse_overlay_load_mappings_v1(&rom, &table).unwrap_err();

    assert_eq!(
        error,
        OverlayLoadMappingError::InvalidRomInterval { record: 0 }
    );
}

/// Competing admissions stay a failure here exactly as they do for recipes.
#[test]
fn competing_admitted_tables_are_still_refused() {
    let (rom, table) = load_only_rom();
    let admission = |table: CandidateTable| crate::overlay_regions::TableAdmission {
        region_deltas: vec![None; table.records.len()],
        table,
        mapped_regions: 0,
        admitted: true,
    };
    let recovery = crate::overlay_regions::OverlayRecovery {
        config: crate::overlay_regions::SearchConfig::aki_family(),
        delta_config: crate::delta_vote::DeltaVoteConfig::default(),
        min_mapped_regions: 1,
        candidate_tables: vec![table.clone()],
        admissions: vec![admission(table.clone()), admission(table)],
    };

    let error = admitted_overlay_load_mappings_v1(&rom, &recovery).unwrap_err();

    assert_eq!(
        error,
        OverlayLoadMappingError::NoUniqueAdmittedTable { admitted: 2 }
    );
}

/// Bottom of the 9th's measured shape: the destination field holds a pointer
/// to the overlay's NAME string ("bb2code", "tpitcode"), stepping a few bytes
/// per record while the images are kilobytes. Those pointers are plausible
/// RDRAM addresses, so every per-record check passes and the table is
/// admitted -- but reporting them as load addresses would be a confident
/// falsehood. The interval stays proven; the address must not be.
#[test]
fn a_name_pointer_field_yields_no_load_address() {
    let (mut rom, mut table) = load_only_rom();
    for (index, record) in table.records.iter_mut().enumerate() {
        record.vram_dest = 0x8005_9bc4 - (index as u32) * 12;
    }
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base + 8, record.vram_dest);
    }

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert_eq!(mappings.len(), 2, "the ROM intervals are still recovered");
    assert!(
        mappings.iter().all(|mapping| mapping.load_start.is_none()),
        "pointers that step by string lengths cannot be load destinations"
    );
    assert_eq!(mappings[0].rom_start, 0x2000);
    assert_eq!(mappings[0].loaded_range(), None);
}

/// Batman of the Future's measured shape: every record declares the SAME
/// destination -- one reused slot, the swapping-engine shape `overlay_regions`
/// already recognizes. Overlapping destinations are the norm here, not
/// evidence of a mis-read field, so the address must survive.
#[test]
fn a_reused_destination_slot_is_still_a_load_address() {
    let (mut rom, mut table) = load_only_rom();
    for record in table.records.iter_mut() {
        record.vram_dest = 0x8022_f2c0;
    }
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base + 8, record.vram_dest);
    }

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert!(
        mappings
            .iter()
            .all(|mapping| mapping.load_start == Some(0x8022_f2c0)),
        "a swapping engine reuses one slot; that is a real destination"
    );
}

/// The load-bearing property of this whole module. A load-only mapping proves
/// no section extents, so it must not be convertible into codegen input --
/// `DenseAotGenerationInput` requires all six and re-validates them, and
/// supplying invented values would mark guesses as proven.
///
/// This is a compile-time guarantee (no `From` impl exists), which no runtime
/// assertion can express; the test pins the invariant in prose so that adding
/// such a conversion has to consciously delete this.
#[test]
fn a_load_only_mapping_carries_no_section_extents() {
    let (rom, table) = load_only_rom();

    let mapping = &parse_overlay_load_mappings_v1(&rom, &table).unwrap()[0];
    let json = serde_json::to_value(mapping).unwrap();

    for absent in [
        "text_start",
        "text_end",
        "data_start",
        "data_end",
        "bss_start",
        "bss_end",
    ] {
        assert!(
            json.get(absent).is_none(),
            "{absent} must not appear: a load-only mapping does not prove it"
        );
    }
    assert_ne!(
        mapping.schema,
        crate::overlay_recipe::OVERLAY_RECIPE_SCHEMA_V1,
        "the schema string must distinguish this from a proven recipe"
    );
}
