    use super::*;
    use crate::rom::normalize;

    fn make_test_rom(entry: u32, extra_len: usize) -> NormalizedRom {
        let mut buf = vec![0u8; 0x1000 + extra_len];
        buf[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        buf[8..12].copy_from_slice(&entry.to_be_bytes());
        buf[0x20..0x24].copy_from_slice(b"TEST");
        buf[0x3b..0x3f].copy_from_slice(b"CTSE");
        normalize(&buf).expect("valid synthetic z64")
    }

    #[test]
    fn recognized_entry_loading_ipl3_publishes_header_mapping() {
        let rom = make_test_rom(0x8000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );
        assert!(matches!(
            outcome,
            BootBankDiscovery::Proven { load_delta: 0, .. }
        ));

        let concl = db.conclusion("bank:boot").expect("boot bank concluded");
        assert_eq!(concl.state, ProofState::Proven);

        let mapping = db
            .facts()
            .iter()
            .find(|f| matches!(f, Fact::RomMapping { bank, .. } if bank == BOOT_BANK))
            .expect("rom mapping fact present");
        match mapping {
            Fact::RomMapping {
                rom_start,
                va_start,
                ..
            } => {
                assert_eq!(*rom_start, BOOT_COPY_ROM_START);
                assert_eq!(*va_start, 0x8000_0400);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn recognized_ipl3_with_truncated_boot_copy_stays_open() {
        let rom = make_test_rom(0x8000_0400, 0x2000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6105Or7105,
            IPL3_SHA256_CIC_6105_7105.to_string(),
        );
        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::TruncatedBootCopy { .. }
            }
        ));
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_rom_mappings().is_empty());
    }

    fn write_record(buf: &mut [u8], base: usize, rom_start: u32, rom_end: u32, vram: u32) {
        buf[base..base + 4].copy_from_slice(&rom_start.to_be_bytes());
        buf[base + 4..base + 8].copy_from_slice(&rom_end.to_be_bytes());
        buf[base + 8..base + 12].copy_from_slice(&vram.to_be_bytes());
    }

    #[test]
    fn descriptor_table_accepts_well_formed_records_and_rejects_malformed() {
        let mut rom_bytes = vec![0u8; 0x1000 + 0x10000];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");

        let table_off = 0x2000usize;
        // record 0: well-formed
        write_record(&mut rom_bytes, table_off, 0x3000, 0x4000, 0x8010_0000);
        // record 1: inverted interval (rom_end < rom_start) -- malformed
        write_record(
            &mut rom_bytes,
            table_off + 0x10,
            0x5000,
            0x4500,
            0x8020_0000,
        );
        // record 2: zero vram_dest -- malformed
        write_record(&mut rom_bytes, table_off + 0x20, 0x6000, 0x7000, 0);

        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        let shape = DescriptorTableShape {
            table_rom_offset: table_off as u32,
            record_count: 3,
            record_stride: 0x10,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
        };
        let accepted = scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db);

        assert_eq!(accepted, vec!["overlay_0".to_string()]);
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Proven
        );
        assert_eq!(
            db.conclusion("bank:overlay_1").unwrap().state,
            ProofState::Rejected
        );
        assert_eq!(
            db.conclusion("bank:overlay_2").unwrap().state,
            ProofState::Rejected
        );
    }

    #[test]
    fn descriptor_table_out_of_bounds_record_is_open_not_dropped() {
        let mut rom_bytes = vec![0u8; 0x1000 + 0x100];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        let rom = normalize(&rom_bytes).unwrap();

        let mut db = FactDb::new();
        let shape = DescriptorTableShape {
            table_rom_offset: 0x2000, // beyond this tiny ROM
            record_count: 1,
            record_stride: 0x10,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
        };
        scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db);
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Open
        );
    }

    #[test]
    fn descriptor_table_scan_is_byte_identical_across_runs() {
        let mut rom_bytes = vec![0u8; 0x1000 + 0x10000];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        write_record(&mut rom_bytes, 0x2000, 0x3000, 0x4000, 0x8010_0000);
        let rom = normalize(&rom_bytes).unwrap();
        let shape = DescriptorTableShape {
            table_rom_offset: 0x2000,
            record_count: 1,
            record_stride: 0x10,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
        };

        let mut db_a = FactDb::new();
        scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db_a);
        let mut db_b = FactDb::new();
        scan_descriptor_table(&rom, shape, |i| format!("overlay_{i}"), &mut db_b);

        let json_a = serde_json::to_string(&db_a).unwrap();
        let json_b = serde_json::to_string(&db_b).unwrap();
        assert_eq!(json_a, json_b, "repeated generation must be byte-identical");
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }

    fn literal_yaz0(bytes: &[u8]) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(16 + bytes.len() + bytes.len().div_ceil(8));
        encoded.extend_from_slice(b"Yaz0");
        encoded.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&[0; 8]);
        for chunk in bytes.chunks(8) {
            encoded.push(0xff);
            encoded.extend_from_slice(chunk);
        }
        encoded
    }

    fn file_table_input(count: u32) -> LoadImageTableInput {
        LoadImageTableInput {
            name: "files".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: 0x2000,
                },
                record_count: count,
                record_stride: 0x10,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::PhysicalRom,
                    field_start: 8,
                    end: DestinationEnd::FieldOrSourceLength(0xc),
                },
            },
            bank_name: None,
        }
    }

    #[test]
    fn generalized_tables_resolve_vrom_table_and_yaz0_load_image() {
        let mut rom_bytes = vec![0u8; 0x6000];
        write_u32(&mut rom_bytes, 0, 0x8037_1240);
        write_u32(&mut rom_bytes, 8, 0x8000_0400);
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");

        let mut overlay_table = vec![0u8; 0x20];
        write_u32(&mut overlay_table, 0, 0x0002_0000);
        write_u32(&mut overlay_table, 4, 0x0002_0010);
        write_u32(&mut overlay_table, 8, 0x8080_0000);
        write_u32(&mut overlay_table, 0xc, 0x8080_0020);
        let compressed_table = literal_yaz0(&overlay_table);
        rom_bytes[0x3000..0x3000 + compressed_table.len()].copy_from_slice(&compressed_table);

        let overlay_bytes: Vec<u8> = (0..0x10).map(|value| value as u8).collect();
        rom_bytes[0x4000..0x4010].copy_from_slice(&overlay_bytes);

        write_u32(&mut rom_bytes, 0x2000, 0x0001_0000);
        write_u32(&mut rom_bytes, 0x2004, 0x0001_0020);
        write_u32(&mut rom_bytes, 0x2008, 0x3000);
        write_u32(
            &mut rom_bytes,
            0x200c,
            0x3000 + compressed_table.len() as u32,
        );
        write_u32(&mut rom_bytes, 0x2010, 0x0002_0000);
        write_u32(&mut rom_bytes, 0x2014, 0x0002_0010);
        write_u32(&mut rom_bytes, 0x2018, 0x4000);
        write_u32(&mut rom_bytes, 0x201c, 0);

        let overlay = LoadImageTableInput {
            name: "effects".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Virtual,
                    offset: 0x0001_0000,
                },
                record_count: 1,
                record_stride: 0x20,
                source: SourceRangeFields {
                    space: RomAddressSpace::Virtual,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 8,
                    end: DestinationEnd::Field(0xc),
                },
            },
            bank_name: Some(BankNamePattern::new("effect_", 0, "")),
        };
        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        let accepted =
            scan_load_image_tables(&rom, &[overlay.clone(), file_table_input(2)], &mut db);

        assert_eq!(accepted, ["effect_0"]);
        assert_eq!(
            db.conclusion("bank:effect_0").unwrap().state,
            ProofState::Proven
        );
        let record = db
            .facts()
            .iter()
            .find(|fact| {
                matches!(
                    fact,
                    Fact::LoadImageTableRecord { table, index: 0, .. }
                        if table == "effects"
                )
            })
            .expect("typed table/record evidence");
        assert!(matches!(
            record,
            Fact::LoadImageTableRecord {
                source_start: 0x0002_0000,
                destination_start: 0x8080_0000,
                ..
            }
        ));
        let materialized = materialize_rom_range(
            &rom,
            &db,
            RomAddressSpace::Virtual,
            0x0002_0000,
            0x0002_0010,
        )
        .unwrap();
        assert_eq!(materialized.bytes, overlay_bytes);

        let mut repeated = FactDb::new();
        scan_load_image_tables(&rom, &[overlay, file_table_input(2)], &mut repeated);
        assert_eq!(
            serde_json::to_string(&db).unwrap(),
            serde_json::to_string(&repeated).unwrap(),
            "generalized table discovery must be byte-identical"
        );
    }

    #[test]
    fn overlapping_source_and_vram_images_surface_as_conflict() {
        let mut rom_bytes = vec![0u8; 0x6000];
        write_u32(&mut rom_bytes, 0, 0x8037_1240);
        write_u32(&mut rom_bytes, 8, 0x8000_0400);
        rom_bytes[0x20..0x24].copy_from_slice(b"TEST");
        rom_bytes[0x3b..0x3f].copy_from_slice(b"CTSE");
        for (base, source_start, source_end, vram_start, vram_end) in [
            (0x2000, 0x4000, 0x4020, 0x8010_0000, 0x8010_0020),
            (0x2010, 0x4010, 0x4030, 0x8010_0010, 0x8010_0030),
        ] {
            write_u32(&mut rom_bytes, base, source_start);
            write_u32(&mut rom_bytes, base + 4, source_end);
            write_u32(&mut rom_bytes, base + 8, vram_start);
            write_u32(&mut rom_bytes, base + 0xc, vram_end);
        }
        let input = LoadImageTableInput {
            name: "overlays".to_string(),
            shape: LoadImageTableShape {
                location: TableLocation {
                    space: RomAddressSpace::Physical,
                    offset: 0x2000,
                },
                record_count: 2,
                record_stride: 0x10,
                source: SourceRangeFields {
                    space: RomAddressSpace::Physical,
                    field_start: 0,
                    field_end: 4,
                },
                destination: DestinationRangeFields {
                    space: DestinationSpace::Vram,
                    field_start: 8,
                    end: DestinationEnd::Field(0xc),
                },
            },
            bank_name: Some(BankNamePattern::new("overlay_", 0, "")),
        };
        let rom = normalize(&rom_bytes).unwrap();
        let mut db = FactDb::new();
        scan_load_image_tables(&rom, &[input], &mut db);

        for index in 0..2 {
            assert_eq!(
                db.conclusion(&format!("bank:overlay_{index}"))
                    .unwrap()
                    .state,
                ProofState::Conflict
            );
            assert_eq!(
                db.conclusion(&load_image_table_record_subject("overlays", index))
                    .unwrap()
                    .state,
                ProofState::Conflict
            );
        }
        assert!(db.proven_rom_mappings().is_empty());
    }

    fn recovered_table(
        table_rom_offset: u32,
        rom_start: u32,
        vram_dest: u32,
        inferred_va: u32,
    ) -> crate::overlay_regions::TableAdmission {
        let table = crate::overlay_regions::CandidateTable {
            table_rom_offset,
            record_stride: 0x24,
            field_rom_start: 0x18,
            field_rom_end: 0x1c,
            field_vram_dest: 0x20,
            destination_field: crate::overlay_regions::DestinationFieldSemantics::Start,
            records: vec![crate::overlay_regions::CandidateRecord {
                rom_start,
                rom_end: rom_start + 0x1000,
                vram_dest,
            }],
        };
        crate::overlay_regions::TableAdmission {
            table,
            region_deltas: vec![Some((inferred_va.wrapping_sub(rom_start), inferred_va))],
            mapped_regions: 1,
            admitted: true,
        }
    }

    fn recovery_with(
        admissions: Vec<crate::overlay_regions::TableAdmission>,
    ) -> crate::overlay_regions::OverlayRecovery {
        crate::overlay_regions::OverlayRecovery {
            config: crate::overlay_regions::SearchConfig::aki_family(),
            delta_config: crate::delta_vote::DeltaVoteConfig::default(),
            min_mapped_regions: 1,
            candidate_tables: admissions
                .iter()
                .map(|admission| admission.table.clone())
                .collect(),
            admissions,
        }
    }

    #[test]
    fn unique_recovered_table_with_matching_delta_proves_load_image() {
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![recovered_table(
            0x1800,
            0x2000,
            0x8010_0000,
            0x8010_0000,
        )]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert_eq!(banks, ["overlay_0"]);
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Proven
        );
        assert_eq!(
            db.conclusion(&load_image_table_record_subject("recovered_overlays", 0))
                .unwrap()
                .state,
            ProofState::Proven
        );
        assert!(db.facts().iter().any(|fact| matches!(
            fact,
            Fact::RomMapping {
                bank,
                rom_start: 0x2000,
                rom_end: 0x3000,
                va_start: 0x8010_0000,
                va_end: 0x8010_1000,
                ..
            } if bank == "overlay_0"
        )));
    }

    #[test]
    fn recovered_delta_disagreeing_with_descriptor_stays_conflict() {
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![recovered_table(
            0x1800,
            0x2000,
            0x8010_0000,
            0x8010_1000,
        )]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert!(banks.is_empty());
        assert_eq!(
            db.conclusion("bank:overlay_0").unwrap().state,
            ProofState::Conflict
        );
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn multiple_admissions_over_disjoint_sources_merge_into_one_geometry() {
        // Several admitted tables are fragments or stride aliases of one
        // descriptor array unless they actually disagree. These two claim
        // disjoint ROM sources at disjoint destinations, so both map.
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![
            recovered_table(0x1800, 0x2000, 0x8010_0000, 0x8010_0000),
            recovered_table(0x1900, 0x3000, 0x8020_0000, 0x8020_0000),
        ]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert_eq!(banks.len(), 2, "both non-contradicting records must map");
        assert_eq!(db.proven_rom_mappings().len(), 2);
    }

    #[test]
    fn admissions_disagreeing_on_a_destination_still_map_nothing() {
        // The contradiction that matters: one source interval declared at two
        // different VAs. Nothing may be admitted from either table.
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![
            recovered_table(0x1800, 0x2000, 0x8010_0000, 0x8010_0000),
            recovered_table(0x1900, 0x2000, 0x8020_0000, 0x8020_0000),
        ]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert!(banks.is_empty());
        assert_eq!(
            db.conclusion("load-image-table:recovered_overlays")
                .unwrap()
                .state,
            ProofState::Conflict
        );
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn partially_overlapping_sources_are_a_conflict_not_a_merge() {
        // One ROM byte cannot belong to two differently-based images, and no
        // stride alias produces a partial overlap -- aliases repeat whole
        // records.
        let rom = make_test_rom(0x8000_0400, 0x5000);
        let recovery = recovery_with(vec![
            recovered_table(0x1800, 0x2000, 0x8010_0000, 0x8010_0000),
            recovered_table(0x1900, 0x2800, 0x8020_0000, 0x8020_0000),
        ]);
        let mut db = FactDb::new();
        let banks = scan_recovered_overlay_regions(
            &rom,
            &recovery,
            "recovered_overlays",
            &BankNamePattern::new("overlay_", 0, ""),
            &mut db,
        );

        assert!(banks.is_empty());
        assert_eq!(
            db.conclusion("load-image-table:recovered_overlays")
                .unwrap()
                .state,
            ProofState::Conflict
        );
    }

    #[test]
    fn only_exact_standard_ipl3_hashes_have_relocation_behavior() {
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6102_7101),
            Some(RecognizedIpl3::Cic6102Or7101)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6103_7103),
            Some(RecognizedIpl3::Cic6103Or7103)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6105_7105),
            Some(RecognizedIpl3::Cic6105Or7105)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_6106_7106),
            Some(RecognizedIpl3::Cic6106Or7106)
        );
        assert_eq!(
            classify_ipl3_sha256(IPL3_SHA256_CIC_7102),
            Some(RecognizedIpl3::Cic7102)
        );
        assert_eq!(RecognizedIpl3::Cic6102Or7101.load_delta(), 0);
        assert_eq!(RecognizedIpl3::Cic6103Or7103.load_delta(), 0x10_0000);
        assert_eq!(RecognizedIpl3::Cic6105Or7105.load_delta(), 0);
        assert_eq!(RecognizedIpl3::Cic6106Or7106.load_delta(), 0x20_0000);
        assert_eq!(RecognizedIpl3::Cic7102.load_delta(), 0);
        assert_eq!(classify_ipl3_sha256(&"00".repeat(32)), None);
    }

    #[test]
    fn unknown_complete_ipl3_records_open_without_mapping_or_entry() {
        let rom = make_test_rom(0x8000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = discover_boot_bank(&rom, &mut db);

        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::UnrecognizedIpl3 { .. }
            }
        ));
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_rom_mappings().is_empty());
        assert!(db.proven_function_entries(BOOT_BANK).is_empty());
    }

    #[test]
    fn truncated_ipl3_records_typed_open_frontier() {
        let mut bytes = vec![0u8; 0x100];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        let rom = normalize(&bytes).expect("header-sized synthetic z64");
        let mut db = FactDb::new();
        let outcome = discover_boot_bank(&rom, &mut db);

        assert_eq!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::TruncatedIpl3 {
                    available_bytes: 0xc0,
                    required_bytes: 0xfc0,
                }
            }
        );
        assert_eq!(db.conclusion("bank:boot").unwrap().state, ProofState::Open);
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn relocating_ipl3_rejects_entrypoint_subtraction_underflow() {
        let rom = make_test_rom(0x0000_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6103Or7103,
            IPL3_SHA256_CIC_6103_7103.to_string(),
        );

        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::InvalidEntrypoint {
                    entry_point: 0x0000_0400,
                    load_delta: 0x10_0000,
                    ..
                }
            }
        ));
        assert!(db.proven_rom_mappings().is_empty());
    }

    #[test]
    fn entry_loading_ipl3_rejects_address_range_overflow() {
        let rom = make_test_rom(0xfff0_0400, BOOT_COPY_SIZE as usize + 0x1000);
        let mut db = FactDb::new();
        let outcome = publish_boot_bank(
            &rom,
            &mut db,
            RecognizedIpl3::Cic6102Or7101,
            IPL3_SHA256_CIC_6102_7101.to_string(),
        );

        assert!(matches!(
            outcome,
            BootBankDiscovery::Open {
                reason: BootBankOpenReason::InvalidLoadRange {
                    va_start: 0xfff0_0400,
                    byte_length: BOOT_COPY_SIZE,
                    ..
                }
            }
        ));
        assert!(db.proven_rom_mappings().is_empty());
    }
