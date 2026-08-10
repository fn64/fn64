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

/// A four-word record has no room for an allocation triple, and reading one
/// would consume the neighbouring record's fields.
#[test]
fn a_narrow_record_proves_no_allocation_extent() {
    let (rom, table) = load_only_rom();

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert!(mappings.iter().all(|m| m.allocation_end.is_none()));
    assert_eq!(
        mappings[0].proven_reach_end(),
        Some(0x8010_0100),
        "reach falls back to the loaded image"
    );
}

/// Batman's shape: a wide record whose first three words are
/// `[start, start, end]` above the loaded image. The repetition is the
/// evidence that this is an extent rather than two unrelated pointers.
#[test]
fn a_repeated_start_and_bound_prove_an_allocation_extent() {
    let mut rom = vec![0u8; 0x5000];
    let table = CandidateTable {
        table_rom_offset: 0x100,
        record_stride: 0x20,
        field_rom_start: 0x14,
        field_rom_end: 0x18,
        field_vram_dest: 0x1c,
        destination_field: DestinationFieldSemantics::Start,
        records: vec![CandidateRecord {
            rom_start: 0x2000,
            rom_end: 0x2100,
            vram_dest: 0x8010_0000,
        }],
    };
    let base = table.table_rom_offset as usize;
    // The allocation triple reaches past the image end (0x80100100).
    put(&mut rom, base, 0x8010_0400);
    put(&mut rom, base + 4, 0x8010_0400);
    put(&mut rom, base + 8, 0x8010_0900);
    put(&mut rom, base + 0x14, 0x2000);
    put(&mut rom, base + 0x18, 0x2100);
    put(&mut rom, base + 0x1c, 0x8010_0000);

    let mapping = &parse_overlay_load_mappings_v1(&rom, &table).unwrap()[0];

    assert_eq!(mapping.allocation_end, Some(0x8010_0900));
    assert_eq!(
        mapping.proven_reach_end(),
        Some(0x8010_0900),
        "reach must widen past the image, or a swap leaves stale bytes"
    );
}

/// Without the repetition, two words in ascending order are just two words.
#[test]
fn two_unrepeated_ascending_words_are_not_an_extent() {
    let mut rom = vec![0u8; 0x5000];
    let table = CandidateTable {
        table_rom_offset: 0x100,
        record_stride: 0x20,
        field_rom_start: 0x14,
        field_rom_end: 0x18,
        field_vram_dest: 0x1c,
        destination_field: DestinationFieldSemantics::Start,
        records: vec![CandidateRecord {
            rom_start: 0x2000,
            rom_end: 0x2100,
            vram_dest: 0x8010_0000,
        }],
    };
    let base = table.table_rom_offset as usize;
    put(&mut rom, base, 0x8010_0400);
    put(&mut rom, base + 4, 0x8010_0500); // differs: not a start/start/end
    put(&mut rom, base + 8, 0x8010_0900);
    put(&mut rom, base + 0x14, 0x2000);
    put(&mut rom, base + 0x18, 0x2100);
    put(&mut rom, base + 0x1c, 0x8010_0000);

    let mapping = &parse_overlay_load_mappings_v1(&rom, &table).unwrap()[0];

    assert_eq!(mapping.allocation_end, None);
}

/// The invalidation range the lane may act on. Per-overlay reach is a lower
/// bound, so the union over a shared slot is what makes invalidation sound:
/// swapping any one overlay clears everything any sibling could have written.
#[test]
fn a_shared_slot_unions_every_overlays_reach() {
    let (mut rom, mut table) = load_only_rom();
    for record in table.records.iter_mut() {
        record.vram_dest = 0x8022_f2c0;
    }
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base + 8, record.vram_dest);
    }

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();
    let range = shared_slot_invalidation_range(&mappings).unwrap();

    assert_eq!(range.start, 0x8022_f2c0);
    assert_eq!(
        range.end,
        0x8022_f2c0 + 0x100,
        "the union must cover the largest sibling, not just the first"
    );
}

/// Distinct destinations mean the overlays are not contending for one slot, so
/// a union over them would invalidate memory no single swap replaces.
#[test]
fn distinct_destinations_yield_no_shared_slot_range() {
    let (rom, table) = load_only_rom();

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert!(shared_slot_invalidation_range(&mappings).is_none());
}

/// A mapping with no proven destination cannot contribute to an invalidation
/// range at all -- Bottom of the 9th's name-pointer table must not produce one.
#[test]
fn mappings_without_a_destination_yield_no_range() {
    let (mut rom, mut table) = load_only_rom();
    for (index, record) in table.records.iter_mut().enumerate() {
        record.vram_dest = 0x8005_9bc4 - (index as u32) * 12;
    }
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base + 8, record.vram_dest);
    }

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert!(shared_slot_invalidation_range(&mappings).is_none());
}

/// Cruis'n USA's measured shape: two overlays share one slot while a third
/// sits alone elsewhere. That is resident overlays plus a swapping pair, not
/// one engine, and each slot deserves its own union -- a union ACROSS slots
/// would invalidate live memory belonging to an overlay never swapped.
#[test]
fn each_destination_slot_gets_its_own_invalidation_range() {
    let (mut rom, mut table) = load_only_rom();
    table.records.push(CandidateRecord {
        rom_start: 0x2200,
        rom_end: 0x2280,
        vram_dest: 0x8020_0000,
    });
    // Two records share 0x80100000; the third is far away.
    table.records[0].vram_dest = 0x8010_0000;
    table.records[1].vram_dest = 0x8010_0000;
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base, record.rom_start);
        put(&mut rom, base + 4, record.rom_end);
        put(&mut rom, base + 8, record.vram_dest);
    }

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();
    let ranges = per_slot_invalidation_ranges(&mappings).unwrap();

    assert_eq!(ranges.len(), 2, "one range per distinct slot");
    assert_eq!(
        ranges[0],
        0x8010_0000..0x8010_0100,
        "the shared slot unions its two members"
    );
    assert_eq!(ranges[1], 0x8020_0000..0x8020_0080);
    assert!(
        shared_slot_invalidation_range(&mappings).is_none(),
        "the single-slot helper still refuses a multi-slot table"
    );
}

/// If one slot's union reaches into another's, the groups are not independent
/// and invalidating either would clobber the other's live bytes.
#[test]
fn overlapping_slot_groups_are_refused() {
    let (mut rom, mut table) = load_only_rom();
    // Second slot starts inside the first overlay's own extent.
    table.records[0].vram_dest = 0x8010_0000;
    table.records[1].vram_dest = 0x8010_0080;
    for (index, record) in table.records.iter().enumerate() {
        let base = table.table_rom_offset as usize + index * table.record_stride as usize;
        put(&mut rom, base + 8, record.vram_dest);
    }

    let mappings = parse_overlay_load_mappings_v1(&rom, &table).unwrap();

    assert!(per_slot_invalidation_ranges(&mappings).is_none());
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
