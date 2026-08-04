    use super::*;

    fn fallback(source_start: u32, va_start: u32, len: u32) -> DescriptorFallback {
        DescriptorFallback {
            admission_index: 0,
            record_index: 0,
            mapping: MappingHypothesis {
                source_start,
                source_end: source_start + len,
                va_start,
                va_end: va_start + len,
            },
        }
    }

    #[test]
    fn descriptor_corroboration_is_not_gated_on_a_prior_delta_vote_admission() {
        // The deadlock this guards: `admitted` was computed from delta-vote
        // outcomes alone, and only admitted tables were offered descriptor
        // corroboration. A table whose regions all stayed open could never
        // reach the fallback that would resolve them, so it scored zero
        // forever. Here no region votes, yet each decodes as code at its
        // declared VA, so the table must still be admitted.
        let mut rom = vec![0u8; 0x4000];
        // Three disjoint regions of a trivial `jr $ra` + `nop` leaf function.
        for base in [0x1000usize, 0x2000, 0x3000] {
            rom[base..base + 4].copy_from_slice(&0x03e0_0008u32.to_be_bytes());
            rom[base + 4..base + 8].copy_from_slice(&0u32.to_be_bytes());
        }
        let records: Vec<_> = [0x1000u32, 0x2000, 0x3000]
            .iter()
            .enumerate()
            .map(|(index, start)| CandidateRecord {
                rom_start: *start,
                rom_end: start + 8,
                vram_dest: 0x8010_0000 + (index as u32) * 0x1000,
            })
            .collect();
        let table = CandidateTable {
            table_rom_offset: 0x100,
            record_stride: 0x0c,
            field_rom_start: 0,
            field_rom_end: 4,
            field_vram_dest: 8,
            destination_field: DestinationFieldSemantics::Start,
            records,
        };
        let recovery = admit_overlay_region_tables(
            &rom,
            &SearchConfig {
                min_region_len: 8,
                ..SearchConfig::aki_family()
            },
            &DeltaVoteConfig::default(),
            3,
            vec![table],
        );
        let admission = &recovery.admissions[0];
        assert_eq!(
            admission.mapped_regions, 3,
            "every corroborated region must count toward admission"
        );
        assert!(
            admission.admitted,
            "admission must be re-derived after descriptor fallbacks apply"
        );
    }

    #[test]
    fn two_regions_sharing_a_destination_stay_a_conflict() {
        // Ordinary ambiguity: VA uniqueness must remain the rule. Only heavy
        // concentration reads as a reused slot.
        let shared = vec![
            fallback(0x1000, 0x8020_0000, 0x100),
            fallback(0x2000, 0x8020_0000, 0x100),
        ];
        assert!(!overlapping_destination_is_engine_shape(&shared));
    }

    #[test]
    fn scattered_destinations_are_not_a_swapping_engine() {
        // One source per destination is the resident-overlay shape, whatever
        // the record count.
        let scattered: Vec<_> = (0..32)
            .map(|i| fallback(0x1000 + i * 0x100, 0x8020_0000 + i * 0x1000, 0x80))
            .collect();
        assert!(!overlapping_destination_is_engine_shape(&scattered));
    }

    #[test]
    fn two_abutting_sources_on_one_destination_are_a_swap_pair() {
        // WCW/nWo Revenge: ROM 0x3c770..0x834a0 and 0x834a0..0xdac50 both
        // declare 0x80090000. Two sources is an order of magnitude under the
        // concentration floor, so contiguity carries the evidence instead.
        // One sibling wins a raw delta vote while the other falls back, so
        // the shape is only visible across both evidence sets.
        let fallbacks = vec![fallback(0x834a0, 0x8009_0000, 0x577b0)];
        let mut raw = BTreeSet::new();
        raw.insert(MappingHypothesis {
            source_start: 0x3c770,
            source_end: 0x834a0,
            va_start: 0x8009_0000,
            va_end: 0x8009_0000 + 0x46d30,
        });
        assert!(overlapping_destination_is_engine_shape_across(
            &fallbacks, &raw
        ));
        // The strict (VROM) view sees only the fallback and must not.
        assert!(!overlapping_destination_is_engine_shape(&fallbacks));
    }

    #[test]
    fn non_abutting_sources_on_one_destination_stay_a_conflict() {
        // A gap between the sources means they do not tile a span, so the
        // shared destination is ordinary ambiguity, not a swap pair.
        let fallbacks = vec![fallback(0x90000, 0x8009_0000, 0x1000)];
        let mut raw = BTreeSet::new();
        raw.insert(MappingHypothesis {
            source_start: 0x3c770,
            source_end: 0x834a0,
            va_start: 0x8009_0000,
            va_end: 0x8009_0000 + 0x46d30,
        });
        assert!(!overlapping_destination_is_engine_shape_across(
            &fallbacks, &raw
        ));
    }

    #[test]
    fn many_sources_on_one_destination_read_as_a_reused_slot() {
        // Paper Mario (PAL) measured shape: 319 of 322 records in admitted
        // tables declare 0x80240000. Symmetric VA rejection discards them all,
        // so the swapping engine must be recognized instead.
        let swapped: Vec<_> = (0..24)
            .map(|i| fallback(0x1000 + i * 0x1000, 0x8024_0000, 0x400))
            .collect();
        assert!(overlapping_destination_is_engine_shape(&swapped));
    }

    #[test]
    fn a_swapping_engine_still_yields_to_an_independent_delta_vote() {
        // The safety property: a delta-vote mapping is independently derived,
        // so it outranks a merely declared destination even here.
        let swapped: Vec<_> = (0..24)
            .map(|i| fallback(0x1000 + i * 0x1000, 0x8024_0000, 0x400))
            .collect();
        let mut admissions = vec![(vec![None; 1], 0u32)];
        let mut raw = BTreeSet::new();
        raw.insert(MappingHypothesis {
            source_start: 0xdead_0000,
            source_end: 0xdead_0400,
            va_start: 0x8024_0000,
            va_end: 0x8024_0400,
        });
        let admitted = apply_unique_descriptor_fallbacks(
            &mut admissions,
            &raw,
            swapped,
            |entry| &mut entry.0,
            |entry| &mut entry.1,
        );
        assert!(
            admitted.is_empty(),
            "a conflicting raw delta-vote mapping must still reject declared destinations"
        );
    }

    #[test]
    fn oversized_yaz0_file_cannot_supply_vrom_descriptor_candidates() {
        const HUGE_OUTPUT: u32 = 0xffff_fff0;
        let mut rom = Vec::from(&b"Yaz0"[..]);
        rom.extend_from_slice(&HUGE_OUTPUT.to_be_bytes());
        rom.extend_from_slice(&[0; 8]);
        let file_table = CandidateFileTable {
            table_rom_offset: 0,
            record_stride: 0x10,
            vrom_alignment: 4,
            field_vrom_start: 0,
            field_vrom_end: 4,
            field_rom_start: 8,
            field_rom_end: 12,
            records: vec![crate::file_table::FileTableRecord {
                vrom_start: 0,
                vrom_end: HUGE_OUTPUT,
                rom_start: 0,
                rom_end: rom.len() as u32,
            }],
        };

        let mut limit_hits = BTreeSet::new();
        let candidates = enumerate_vrom_family_tables(
            &rom,
            &file_table,
            &SearchConfig::vrom_family(),
            2,
            VromMaterializationLimits {
                max_decoded_file_bytes: 1024,
            },
            &mut limit_hits,
        );
        assert!(candidates.is_empty());
        assert_eq!(
            limit_hits.into_iter().collect::<Vec<_>>(),
            vec![DecodedFileLimitHit {
                vrom_start: 0,
                vrom_end: HUGE_OUTPUT,
                decoded_file_bytes: HUGE_OUTPUT as usize,
                max_decoded_file_bytes: 1024,
            }]
        );
    }

    /// Straight exhaustive reference retained in tests so the sparse-index
    /// fast path is proved against the original stride/phase/offset walk.
    fn enumerate_family_tables_reference(
        rom_bytes: &[u8],
        config: &SearchConfig,
    ) -> Vec<CandidateTable> {
        let rom_len = rom_bytes.len() as u32;
        let mut raw = Vec::new();
        for &stride in &config.strides {
            if stride < 12 || !stride.is_multiple_of(4) {
                continue;
            }
            let max_field_start = stride - 12;
            let mut field_rom_start = 0u32;
            while field_rom_start <= max_field_start {
                let field_rom_end = field_rom_start + 4;
                let field_vram_dest = field_rom_start + 8;
                for destination_field in [
                    DestinationFieldSemantics::Start,
                    DestinationFieldSemantics::ExclusiveEnd,
                ] {
                    let mut offset = config.min_rom_offset;
                    let last_table_start =
                        rom_len.saturating_sub(stride.saturating_mul(config.min_records));
                    while offset <= last_table_start {
                        let read_rec = |base: u32| -> Option<CandidateRecord> {
                            let raw = CandidateRecord {
                                rom_start: read_u32_be(
                                    rom_bytes,
                                    (base + field_rom_start) as usize,
                                )?,
                                rom_end: read_u32_be(rom_bytes, (base + field_rom_end) as usize)?,
                                vram_dest: read_u32_be(
                                    rom_bytes,
                                    (base + field_vram_dest) as usize,
                                )?,
                            };
                            normalize_record_destination(raw, destination_field)
                        };
                        let first = read_rec(offset);
                        if !first.is_some_and(|record| record_valid(&record, rom_len, config, rom_bytes)) {
                            offset += 4;
                            continue;
                        }
                        let mut records = Vec::new();
                        let mut base = offset;
                        while base <= rom_len.saturating_sub(stride) {
                            match read_rec(base) {
                                Some(record) if record_valid(&record, rom_len, config, rom_bytes) => {
                                    records.push(record);
                                    base += stride;
                                }
                                _ => break,
                            }
                        }
                        if records.len() as u32 >= config.min_records
                            && intervals_non_overlapping(&records)
                        {
                            raw.push(CandidateTable {
                                table_rom_offset: offset,
                                record_stride: stride,
                                field_rom_start,
                                field_rom_end,
                                field_vram_dest,
                                destination_field,
                                records,
                            });
                            offset = base;
                        } else {
                            offset += 4;
                        }
                    }
                }
                field_rom_start += 4;
            }
        }
        canonicalize(raw)
    }

    /// Build a big-endian ROM with a planted descriptor table of a chosen
    /// shape. Region bodies are filled with a delta_vote-admissible code
    /// pattern (three distinct `jal`s onto prologues plus enough `lui`) so
    /// the uniqueness filter has something real to admit.
    struct RomBuilder {
        bytes: Vec<u8>,
    }

    impl RomBuilder {
        fn new(len: usize) -> Self {
            Self {
                bytes: vec![0u8; len],
            }
        }
        fn put_u32(&mut self, offset: u32, value: u32) {
            self.bytes[offset as usize..offset as usize + 4].copy_from_slice(&value.to_be_bytes());
        }
        /// Fill `[rom_start, rom_start+len)` with a region mapped to `va_start`
        /// that delta_vote admits: three distinct jals to prologues plus lui.
        fn plant_admissible_region(&mut self, rom_start: u32, len: u32, va_start: u32) {
            self.plant_admissible_bytes(rom_start, len, va_start);
        }
        fn plant_admissible_bytes(&mut self, physical_start: u32, len: u32, va_start: u32) {
            const PROLOGUE: u32 = 0x27bd_ffe0; // addiu $sp,$sp,-0x20
            let lui = 0x3c04_0000 | (va_start >> 16); // lui $a0, hi(va)
            let jal = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
            // Prologues at non-uniform offsets so no second delta aliases.
            let prologues = [0x40u32, 0x90, 0x100];
            self.put_u32(physical_start, jal(va_start + prologues[0]));
            self.put_u32(physical_start + 8, jal(va_start + prologues[1]));
            self.put_u32(physical_start + 16, jal(va_start + prologues[2]));
            for k in 0..4u32 {
                self.put_u32(physical_start + 24 + k * 4, lui);
            }
            for &p in &prologues {
                if p + 4 <= len {
                    self.put_u32(physical_start + p, PROLOGUE);
                }
            }
        }

        fn plant_admissible_and_corroborating_region(
            &mut self,
            physical_start: u32,
            va_start: u32,
        ) {
            let jal = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
            for (offset, word) in [
                (0x00, jal(va_start + 0x40)),
                (0x04, 0),
                (0x08, jal(va_start + 0x90)),
                (0x0c, 0),
                (0x10, jal(va_start + 0x100)),
                (0x14, 0),
                (0x18, 0x3c04_0000 | (va_start >> 16)),
                (0x1c, 0x3c05_0000 | (va_start >> 16)),
                (0x20, 0x3c06_0000 | (va_start >> 16)),
                (0x24, 0x3c07_0000 | (va_start >> 16)),
                (0x28, 0x03e0_0008),
                (0x2c, 0),
            ] {
                self.put_u32(physical_start + offset, word);
            }
            for offset in [0x40, 0x90, 0x100] {
                self.put_u32(physical_start + offset, 0x27bd_ffe0);
                self.put_u32(physical_start + offset + 4, 0x03e0_0008);
                self.put_u32(physical_start + offset + 8, 0);
            }
        }

        /// A small, complete leaf function with no absolute-address evidence:
        /// delta_vote stays open, while descriptor-rooted CFG validation has
        /// an independently decodable return path.
        fn plant_descriptor_corroborating_region(&mut self, physical_start: u32) {
            self.put_u32(physical_start, 0x27bd_ffe0); // addiu $sp,$sp,-0x20
            self.put_u32(physical_start + 4, 0x03e0_0008); // jr $ra
            self.put_u32(physical_start + 8, 0x0000_0000); // delay-slot nop
        }

        fn plant_file_record(
            &mut self,
            offset: u32,
            vrom_start: u32,
            vrom_end: u32,
            rom_start: u32,
        ) {
            self.put_u32(offset, vrom_start);
            self.put_u32(offset + 4, vrom_end);
            self.put_u32(offset + 8, rom_start);
            self.put_u32(offset + 12, 0);
        }
    }

    /// A planted table of a NON-NW4E shape (stride 0x10, fields at +0) is
    /// recovered, and its regions delta_vote-admit.
    #[test]
    fn planted_non_nw4e_table_is_recovered() {
        let mut rom = RomBuilder::new(0x40_000);
        let table_off = 0x2000u32;
        let stride = 0x10u32;
        // Three chained regions.
        let regions = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (i, &(rom_start, len, va)) in regions.iter().enumerate() {
            let base = table_off + i as u32 * stride;
            rom.put_u32(base, rom_start);
            rom.put_u32(base + 4, rom_start + len);
            rom.put_u32(base + 8, va);
            rom.plant_admissible_region(rom_start, len, va);
        }

        let config = SearchConfig::aki_family();
        assert_eq!(
            enumerate_family_tables(&rom.bytes, &config),
            enumerate_family_tables_reference(&rom.bytes, &config),
        );
        let recovery = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);

        // The planted table is present as a distinct candidate.
        let found = recovery
            .candidate_tables
            .iter()
            .find(|t| {
                t.interval_set() == vec![(0x8000, 0x10000), (0x10000, 0x16000), (0x16000, 0x1f000)]
            })
            .expect("planted table recovered");
        assert_eq!(found.records.len(), 3);
        assert_eq!(found.record_stride, 0x10);

        // Its regions delta_vote-admit and the table is admitted.
        let admission = recovery
            .admissions
            .iter()
            .find(|a| a.table.interval_set() == found.interval_set())
            .unwrap();
        assert_eq!(admission.mapped_regions, 3);
        assert!(admission.admitted);
        assert_eq!(
            admission.region_deltas[0],
            Some((0x8010_0000u32.wrapping_sub(0x8000), 0x8010_0000))
        );
    }

    #[test]
    fn exclusive_end_destination_fields_are_normalized_before_admission() {
        let mut rom = RomBuilder::new(0x40_000);
        let table_off = 0x2000u32;
        let stride = 0x10u32;
        let regions = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (index, &(rom_start, len, va_start)) in regions.iter().enumerate() {
            let base = table_off + index as u32 * stride;
            rom.put_u32(base, rom_start);
            rom.put_u32(base + 4, rom_start + len);
            rom.put_u32(base + 8, va_start + len);
            rom.plant_admissible_region(rom_start, len, va_start);
        }

        let config = SearchConfig::aki_family();
        let recovery = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);
        let intervals = vec![(0x8000, 0x10000), (0x10000, 0x16000), (0x16000, 0x1f000)];
        let end = recovery
            .admissions
            .iter()
            .find(|admission| {
                admission.table.interval_set() == intervals
                    && admission.table.destination_field == DestinationFieldSemantics::ExclusiveEnd
            })
            .expect("exclusive-end table variant recovered");

        assert!(end.admitted);
        assert_eq!(
            end.table
                .records
                .iter()
                .map(|record| record.vram_dest)
                .collect::<Vec<_>>(),
            vec![0x8010_0000, 0x8020_0000, 0x8030_0000]
        );
        let start = recovery
            .admissions
            .iter()
            .find(|admission| {
                admission.table.interval_set() == intervals
                    && admission.table.destination_field == DestinationFieldSemantics::Start
            })
            .expect("start-address interpretation remains an explicit candidate");
        assert!(!start.admitted, "delta disagreement cannot admit a layout");
    }

    #[test]
    fn equally_supported_destination_field_semantics_admit_neither() {
        let mut rom = RomBuilder::new(0x30_000);
        let records = [
            (DestinationFieldSemantics::Start, 0x8000, 0x8010_0000),
            (
                DestinationFieldSemantics::ExclusiveEnd,
                0x18000,
                0x8020_0000,
            ),
        ];
        let tables = records
            .into_iter()
            .enumerate()
            .map(|(index, (destination_field, rom_start, va_start))| {
                rom.plant_admissible_region(rom_start, 0x8000, va_start);
                CandidateTable {
                    table_rom_offset: 0x2000 + index as u32 * 0x10,
                    record_stride: 0x10,
                    field_rom_start: 0,
                    field_rom_end: 4,
                    field_vram_dest: 8,
                    destination_field,
                    records: vec![CandidateRecord {
                        rom_start,
                        rom_end: rom_start + 0x8000,
                        vram_dest: va_start,
                    }],
                }
            })
            .collect();

        let recovery = admit_overlay_region_tables(
            &rom.bytes,
            &SearchConfig::aki_family(),
            &DeltaVoteConfig::default(),
            1,
            tables,
        );

        assert!(recovery
            .admissions
            .iter()
            .all(|admission| !admission.admitted));
    }

    /// A table whose last record is out of bounds is not extended into it:
    /// the run stops at the last valid record, and if fewer than `min_records`
    /// remain the run is not a table at all.
    #[test]
    fn record_out_of_bounds_is_rejected() {
        let mut rom = RomBuilder::new(0x20_000);
        let table_off = 0x2000u32;
        // Two valid records then one whose rom_end exceeds the ROM.
        rom.put_u32(table_off, 0x4000);
        rom.put_u32(table_off + 4, 0x8000);
        rom.put_u32(table_off + 8, 0x8010_0000);
        rom.put_u32(table_off + 0x10, 0x8000);
        rom.put_u32(table_off + 0x14, 0xc000);
        rom.put_u32(table_off + 0x18, 0x8020_0000);
        // Out-of-bounds rom_end.
        rom.put_u32(table_off + 0x20, 0xc000);
        rom.put_u32(table_off + 0x24, 0x00ff_ffff); // > rom_len
        rom.put_u32(table_off + 0x28, 0x8030_0000);

        let config = SearchConfig::aki_family();
        let tables = enumerate_family_tables(&rom.bytes, &config);
        // The two-record prefix is below min_records=3, so no table with the
        // out-of-bounds interval is admitted anywhere.
        assert!(tables.iter().all(|t| t
            .records
            .iter()
            .all(|r| r.rom_end <= rom.bytes.len() as u32)));
        assert!(tables
            .iter()
            .all(|t| !t.interval_set().contains(&(0xc000, 0x00ff_ffff))));
    }

    /// Two distinct candidate tables both survive the raw family search;
    /// delta_vote admissibility keeps only the one whose regions are real
    /// code. The other (coincidental pointer pairs over zero-filled ROM)
    /// stays a candidate but is NOT admitted.
    #[test]
    fn delta_vote_disambiguates_two_candidate_tables() {
        let mut rom = RomBuilder::new(0x60_000);

        // Table A: real code regions -> delta_vote admits.
        let table_a = 0x2000u32;
        let regions_a = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (i, &(rs, len, va)) in regions_a.iter().enumerate() {
            let base = table_a + i as u32 * 0x10;
            rom.put_u32(base, rs);
            rom.put_u32(base + 4, rs + len);
            rom.put_u32(base + 8, va);
            rom.plant_admissible_region(rs, len, va);
        }

        // Table B: well-formed records pointing at zero-filled ROM regions
        // (no code) -> delta_vote finds no lui segment / no votes, stays open.
        let table_b = 0x3000u32;
        let regions_b = [
            (0x30000u32, 0x4000u32, 0x8040_0000u32),
            (0x34000, 0x4000, 0x8041_0000),
            (0x38000, 0x4000, 0x8042_0000),
        ];
        for (i, &(rs, len, va)) in regions_b.iter().enumerate() {
            let base = table_b + i as u32 * 0x10;
            rom.put_u32(base, rs);
            rom.put_u32(base + 4, rs + len);
            rom.put_u32(base + 8, va);
            // Deliberately leave the region zero-filled: not code.
        }

        let config = SearchConfig::aki_family();
        let recovery = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);

        let set_a = vec![(0x8000, 0x10000), (0x10000, 0x16000), (0x16000, 0x1f000)];
        let set_b = vec![(0x30000, 0x34000), (0x34000, 0x38000), (0x38000, 0x3c000)];

        // Both are candidates from the raw search.
        assert!(recovery
            .candidate_tables
            .iter()
            .any(|t| t.interval_set() == set_a));
        assert!(recovery
            .candidate_tables
            .iter()
            .any(|t| t.interval_set() == set_b));

        // delta_vote admits only A.
        let a = recovery
            .admissions
            .iter()
            .find(|x| x.table.interval_set() == set_a)
            .unwrap();
        let b = recovery
            .admissions
            .iter()
            .find(|x| x.table.interval_set() == set_b)
            .unwrap();
        assert!(a.admitted, "code-backed table must admit");
        assert!(!b.admitted, "zero-filled table must not admit");
        assert_eq!(b.mapped_regions, 0);

        // The admitted-intervals accessor returns exactly A's intervals.
        assert_eq!(recovery.admitted_intervals(), set_a);
    }

    #[test]
    fn recovery_is_byte_identical_across_runs() {
        let mut rom = RomBuilder::new(0x40_000);
        let table_off = 0x2000u32;
        let regions = [
            (0x8000u32, 0x8000u32, 0x8010_0000u32),
            (0x10000, 0x6000, 0x8020_0000),
            (0x16000, 0x9000, 0x8030_0000),
        ];
        for (i, &(rs, len, va)) in regions.iter().enumerate() {
            let base = table_off + i as u32 * 0x10;
            rom.put_u32(base, rs);
            rom.put_u32(base + 4, rs + len);
            rom.put_u32(base + 8, va);
            rom.plant_admissible_region(rs, len, va);
        }
        let config = SearchConfig::aki_family();
        let first = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);
        let second = recover_overlay_regions(&rom.bytes, &config, &DeltaVoteConfig::default(), 2);
        assert_eq!(
            serde_json::to_string(&first).unwrap(),
            serde_json::to_string(&second).unwrap()
        );
    }

    fn descriptor_fallback_fixture(third_va: u32, corroborating_code: bool) -> TableAdmission {
        let mut rom = RomBuilder::new(0x20_000);
        let records = [
            (0x8000u32, 0x400u32, 0x8010_0000u32),
            (0x9000, 0x400, 0x8020_0000),
            (0xa000, 0x400, third_va),
        ];
        for (index, &(rom_start, len, va_start)) in records.iter().enumerate() {
            let descriptor = 0x2000 + index as u32 * 0x10;
            rom.put_u32(descriptor, rom_start);
            rom.put_u32(descriptor + 4, rom_start + len);
            rom.put_u32(descriptor + 8, va_start);
            if index < 2 {
                rom.plant_admissible_region(rom_start, len, va_start);
            } else if corroborating_code {
                rom.plant_descriptor_corroborating_region(rom_start);
            }
        }

        let recovery = recover_overlay_regions(
            &rom.bytes,
            &SearchConfig::vrom_family(),
            &DeltaVoteConfig::default(),
            2,
        );
        recovery
            .admissions
            .into_iter()
            .find(|admission| {
                admission.table.interval_set()
                    == vec![(0x8000, 0x8400), (0x9000, 0x9400), (0xa000, 0xa400)]
            })
            .expect("planted descriptor table recovered")
    }

    #[test]
    fn descriptor_corroborates_delta_open_record() {
        let admission = descriptor_fallback_fixture(0x8030_0000, true);
        assert!(
            admission.admitted,
            "two delta-voted records admit the table"
        );
        assert_eq!(admission.mapped_regions, 3);
        assert_eq!(
            admission.region_deltas[2],
            Some((0x8030_0000u32.wrapping_sub(0xa000), 0x8030_0000))
        );
    }

    #[test]
    fn descriptor_without_region_corroboration_stays_open() {
        let admission = descriptor_fallback_fixture(0x8030_0000, false);
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 2);
        assert_eq!(admission.region_deltas[2], None);
    }

    #[test]
    fn descriptor_overlapping_delta_admitted_va_is_rejected_as_non_unique() {
        let admission = descriptor_fallback_fixture(0x8010_0000, true);
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 2);
        assert_eq!(admission.region_deltas[2], None);
    }

    fn one_delta_mapped_vrom_table(second_va: u32, corroborate_second: bool) -> VromTableAdmission {
        let mut rom = RomBuilder::new(0x20_000);
        for (index, &(vrom_start, vrom_end, physical_start)) in [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x2400, 0x8000),
            (0x2400, 0x2800, 0x8400),
        ]
        .iter()
        .enumerate()
        {
            rom.plant_file_record(
                0x2000 + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }

        let first_va = 0x8010_0000;
        for (index, &(start, end, va)) in [(0x2000, 0x2400, first_va), (0x2400, 0x2800, second_va)]
            .iter()
            .enumerate()
        {
            let descriptor = 0x4000 + index as u32 * 0x10;
            rom.put_u32(descriptor, start);
            rom.put_u32(descriptor + 4, end);
            rom.put_u32(descriptor + 8, va);
        }
        rom.plant_admissible_and_corroborating_region(0x8000, first_va);
        if corroborate_second {
            rom.plant_descriptor_corroborating_region(0x8400);
        }

        recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::vrom_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        )
        .admissions
        .into_iter()
        .find(|admission| {
            admission.table.interval_set() == vec![(0x2000, 0x2400), (0x2400, 0x2800)]
        })
        .expect("planted VROM descriptor table recovered")
    }

    #[test]
    fn one_delta_mapped_table_with_corroborated_unique_record_is_admitted() {
        let admission = one_delta_mapped_vrom_table(0x8020_0000, true);
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 2);
        assert_eq!(
            admission.region_diagnostics,
            [
                VromRecordMappingDiagnostic::DeltaVoteAndDescriptorCorroborated,
                VromRecordMappingDiagnostic::DescriptorCorroborated,
            ]
        );
    }

    #[test]
    fn one_delta_mapped_table_with_uncorroborated_record_stays_open() {
        let admission = one_delta_mapped_vrom_table(0x8020_0000, false);
        assert!(!admission.admitted);
        assert_eq!(admission.mapped_regions, 1);
        assert!(matches!(
            admission.region_diagnostics[1],
            VromRecordMappingDiagnostic::Open(DescriptorMappingFailure::Rule3Cfg(_))
        ));
    }

    #[test]
    fn one_delta_mapped_table_with_va_conflict_is_rejected() {
        let admission = one_delta_mapped_vrom_table(0x8010_0000, true);
        assert!(!admission.admitted);
        assert_eq!(admission.mapped_regions, 1);
        assert!(admission.region_diagnostics.iter().any(|diagnostic| {
            *diagnostic
                == VromRecordMappingDiagnostic::Open(DescriptorMappingFailure::Rule4VaConflict)
        }));
    }

    #[test]
    fn bounded_short_leaf_is_code_but_same_size_non_code_is_rejected() {
        let mapping = MappingHypothesis {
            source_start: 0x1000,
            source_end: 0x1080,
            va_start: 0x8080_0000,
            va_end: 0x8080_0080,
        };
        let mut leaf = vec![0u8; 0x80];
        leaf[..12].copy_from_slice(&[
            0x27, 0xbd, 0xff, 0xe0, 0x03, 0xe0, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
        ]);
        assert_eq!(descriptor_mapping_corroborated(&leaf, mapping), Ok(()));
        assert!(matches!(
            descriptor_mapping_corroborated(&[0u8; 0x80], mapping),
            Err(DescriptorCfgFailure::RanOffEnd { .. })
        ));
    }

    #[test]
    fn corroborated_one_record_gap_is_admitted_but_isolated_triple_is_not_enumerated() {
        let mut rom = RomBuilder::new(0x20_000);
        let files = [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x2100, 0x8000),
            (0x2100, 0x2200, 0x8100),
            (0x2200, 0x2300, 0x8200),
            (0x2300, 0x2400, 0x8300),
            (0x2400, 0x2500, 0x8400),
            (0x2500, 0x2600, 0x8500),
        ];
        for (index, &(vrom_start, vrom_end, physical_start)) in files.iter().enumerate() {
            rom.plant_file_record(
                0x2800 + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }

        let records = [
            (0x100, 0x2000, 0x2100, 0x8080_0000, 0x8000),
            (0x140, 0x2100, 0x2200, 0x8080_0100, 0x8100),
            (0x200, 0x2200, 0x2300, 0x8080_0200, 0x8200),
            (0x300, 0x2300, 0x2400, 0x8080_0300, 0x8300),
            (0x340, 0x2400, 0x2500, 0x8080_0400, 0x8400),
            // This also corroborates as code, but lies outside the envelope
            // formed by the two ordinary constant-stride table fragments.
            (0x500, 0x2500, 0x2600, 0x8080_0500, 0x8500),
        ];
        for &(table_offset, start, end, va, physical_start) in &records {
            rom.put_u32(0x4000 + table_offset, start);
            rom.put_u32(0x4000 + table_offset + 4, end);
            rom.put_u32(0x4000 + table_offset + 8, va);
            rom.plant_descriptor_corroborating_region(physical_start);
        }

        let recovery = recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::vrom_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        );
        let gap = recovery
            .admissions
            .iter()
            .find(|admission| admission.table.interval_set() == vec![(0x2200, 0x2300)])
            .expect("one-record gap inside the descriptor envelope");
        assert_eq!(gap.table.record_stride, 0);
        assert!(gap.admitted);
        assert_eq!(gap.mapped_regions, 1);
        assert_eq!(
            gap.region_diagnostics,
            [VromRecordMappingDiagnostic::DescriptorCorroborated]
        );
        assert!(recovery
            .candidate_tables
            .iter()
            .all(|table| table.interval_set() != vec![(0x2500, 0x2600)]));
    }

    #[test]
    fn virtual_table_is_resolved_through_recovered_file_table() {
        let mut rom = RomBuilder {
            bytes: vec![0xff; 0x20_000],
        };
        let file_table_offset = 0x2000u32;
        let files = [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x6000, 0x8000),
            (0x6000, 0xa000, 0xc000),
            (0xa000, 0xe000, 0x10000),
        ];
        for (index, &(vrom_start, vrom_end, physical_start)) in files.iter().enumerate() {
            rom.plant_file_record(
                file_table_offset + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }

        let overlays = [
            (0x2000, 0x6000, 0x8000, 0x8010_0000),
            (0x6000, 0xa000, 0xc000, 0x8020_0000),
            (0xa000, 0xe000, 0x10000, 0x8030_0000),
        ];
        for (index, &(vrom_start, vrom_end, physical_start, va_start)) in
            overlays.iter().enumerate()
        {
            let descriptor = 0x4000 + index as u32 * 0x10;
            rom.put_u32(descriptor, vrom_start);
            rom.put_u32(descriptor + 4, vrom_end);
            rom.put_u32(descriptor + 8, va_start);
            rom.plant_admissible_bytes(physical_start, vrom_end - vrom_start, va_start);
        }

        let recovery = recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::aki_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        );
        let file_table = recovery.file_table.admitted_table.as_ref().unwrap();
        assert_eq!(file_table.table_rom_offset, file_table_offset);
        assert_eq!(file_table.translate_uncompressed(0x1000), Some(0x4000));

        let expected = vec![(0x2000, 0x6000), (0x6000, 0xa000), (0xa000, 0xe000)];
        let admission = recovery
            .admissions
            .iter()
            .find(|admission| admission.table.interval_set() == expected)
            .expect("VROM-located descriptor table recovered");
        assert!(admission.admitted);
        assert_eq!(admission.mapped_regions, 3);
        assert_eq!(recovery.admitted_intervals(), expected);
    }

    #[test]
    fn single_image_file_table_does_not_manufacture_an_overlay_table() {
        let mut rom = RomBuilder {
            bytes: vec![0xff; 0x10_000],
        };
        for (index, &(vrom_start, vrom_end, physical_start)) in [
            (0x0000, 0x1000, 0x0000),
            (0x1000, 0x2000, 0x4000),
            (0x2000, 0x3000, 0x5000),
        ]
        .iter()
        .enumerate()
        {
            rom.plant_file_record(
                0x2000 + index as u32 * 0x10,
                vrom_start,
                vrom_end,
                physical_start,
            );
        }
        let recovery = recover_vrom_overlay_regions(
            &rom.bytes,
            &SearchConfig::aki_family(),
            &DeltaVoteConfig::default(),
            &FileTableSearchConfig::n64_family(),
            2,
            2,
        );
        assert!(recovery.file_table.admitted_table.is_some());
        assert!(recovery.candidate_tables.is_empty());
        assert!(recovery.admitted_intervals().is_empty());
    }

    /// One contiguous descriptor array is legible at every multiple of its
    /// true stride, and each alias proposes a strict subset of the dense
    /// table's records. Before this collapsed, Bottom of the 9th admitted the
    /// single array at 0x48038 four times -- at strides 0x10, 0x20, 0x30 and
    /// 0x40 -- and the recipe stage then refused the whole ROM for having no
    /// unique admitted table.
    #[test]
    fn stride_aliases_of_one_array_collapse_to_the_dense_reading() {
        let dense = CandidateTable {
            table_rom_offset: 0x1000,
            record_stride: 0x10,
            field_rom_start: 0x4,
            field_rom_end: 0x8,
            field_vram_dest: 0xc,
            destination_field: DestinationFieldSemantics::Start,
            records: (0..4)
                .map(|i| CandidateRecord {
                    rom_start: 0x2000 + i * 0x100,
                    rom_end: 0x2000 + (i + 1) * 0x100,
                    vram_dest: 0x8010_0000 + i * 0x100,
                })
                .collect(),
        };
        // Every second record of `dense`: a real under-sampling, not a rival.
        let alias = CandidateTable {
            table_rom_offset: 0x1000,
            record_stride: 0x20,
            records: dense.records.iter().step_by(2).cloned().collect(),
            ..dense.clone()
        };
        // Same subset relation, but its stride does not divide -- a distinct
        // array that happens to nest must survive.
        let nested = CandidateTable {
            record_stride: 0x18,
            ..alias.clone()
        };

        let kept = canonicalize(vec![dense.clone(), alias, nested.clone()]);

        assert!(
            kept.iter().any(|t| t.record_stride == dense.record_stride),
            "the dense reading is the real array and must survive"
        );
        assert!(
            !kept.iter().any(|t| t.record_stride == 0x20),
            "a stride multiple proposing a subset is one array under-sampled"
        );
        assert!(
            kept.iter().any(|t| t.record_stride == nested.record_stride),
            "subset alone must not drop a table whose stride is not a multiple"
        );
    }

    /// A record under `min_region_len` is normally dropped -- the floor is a
    /// heuristic about what is worth calling an overlay. But an image that
    /// declares its own length, and agrees with the descriptor about it, has
    /// proven it is a real overlay however small.
    ///
    /// Batman of the Future is why this matters: its rec1 spans 0xd60 bytes,
    /// under the 0x1000 floor, and the destination 0x80281b0c inside that
    /// record's declared allocation was the ROM's last unsupported
    /// destination.
    #[test]
    fn an_image_declaring_its_own_length_survives_the_size_floor() {
        let config = SearchConfig::aki_family();
        let short = config.min_region_len - 0x800;
        let rom_start = 0x4000u32;
        let mut rom = vec![0u8; 0x8000];
        // "MWo2", index, load address, declared length.
        rom[rom_start as usize..rom_start as usize + 4].copy_from_slice(&0x4d57_6f32u32.to_be_bytes());
        rom[rom_start as usize + 12..rom_start as usize + 16]
            .copy_from_slice(&short.to_be_bytes());
        let record = CandidateRecord {
            rom_start,
            rom_end: rom_start + short,
            vram_dest: 0x8010_0000,
        };

        assert!(
            record_valid(&record, rom.len() as u32, &config, &rom),
            "a header-declared length proves the record is real"
        );

        // Same record, no header: the floor applies as before.
        let bare = vec![0u8; 0x8000];
        assert!(
            !record_valid(&record, bare.len() as u32, &config, &bare),
            "without that proof the size floor must still reject it"
        );
    }

    /// The waiver is only ever a licence to admit, never to reject: a header
    /// that disagrees with the descriptor about the length proves nothing.
    #[test]
    fn a_header_length_disagreeing_with_the_record_waives_nothing() {
        let config = SearchConfig::aki_family();
        let short = config.min_region_len - 0x800;
        let rom_start = 0x4000u32;
        let mut rom = vec![0u8; 0x8000];
        rom[rom_start as usize..rom_start as usize + 4].copy_from_slice(&0x4d57_6f32u32.to_be_bytes());
        // Declares a different length than the record spans.
        rom[rom_start as usize + 12..rom_start as usize + 16]
            .copy_from_slice(&(short + 4).to_be_bytes());
        let record = CandidateRecord {
            rom_start,
            rom_end: rom_start + short,
            vram_dest: 0x8010_0000,
        };

        assert!(!record_valid(&record, rom.len() as u32, &config, &rom));
    }

    /// Starting a coarse walk one record early reads neighbouring words as
    /// fields, so a phase-shifted alias mixes genuine borrowed records with
    /// noise and is therefore NOT a subset. Bottom of the 9th's table at
    /// 0x48008 is this shape: two of its four records appear verbatim in the
    /// array at 0x48038, and one is a 4-byte "overlay" over ROM 0x0..0x4.
    #[test]
    fn phase_shifted_alias_mixing_borrowed_and_junk_records_collapses() {
        let dense = CandidateTable {
            table_rom_offset: 0x1000,
            record_stride: 0x10,
            field_rom_start: 0x4,
            field_rom_end: 0x8,
            field_vram_dest: 0xc,
            destination_field: DestinationFieldSemantics::Start,
            // Chained: each record's end opens the next record's start.
            records: (0..6)
                .map(|i| CandidateRecord {
                    rom_start: 0x2000 + i * 0x100,
                    rom_end: 0x2000 + (i + 1) * 0x100,
                    vram_dest: 0x8010_0000 + i * 0x100,
                })
                .collect(),
        };
        let alias = CandidateTable {
            record_stride: 0x40,
            records: vec![
                dense.records[1].clone(),
                dense.records[3].clone(),
                CandidateRecord {
                    rom_start: 0x9000,
                    rom_end: 0x9100,
                    vram_dest: 0x8090_0000,
                },
            ],
            ..dense.clone()
        };

        let kept = canonicalize(vec![dense.clone(), alias]);

        assert_eq!(
            kept.len(),
            1,
            "a majority-borrowed reading of a chained array is that array misread"
        );
        assert_eq!(kept[0].record_stride, dense.record_stride);
    }

    /// The gate on the majority-borrowing rule. Two independent arrays may
    /// share records without either being a misreading of the other, so a
    /// coarser table must not suppress a denser one that does not chain.
    #[test]
    fn majority_borrowing_from_an_unchained_table_is_not_an_alias() {
        let scattered = CandidateTable {
            table_rom_offset: 0x1000,
            record_stride: 0x10,
            field_rom_start: 0x4,
            field_rom_end: 0x8,
            field_vram_dest: 0xc,
            destination_field: DestinationFieldSemantics::Start,
            // Deliberate gaps: this proposes no contiguous span.
            records: (0..6)
                .map(|i| CandidateRecord {
                    rom_start: 0x2000 + i * 0x400,
                    rom_end: 0x2000 + i * 0x400 + 0x100,
                    vram_dest: 0x8010_0000 + i * 0x100,
                })
                .collect(),
        };
        let sharing = CandidateTable {
            record_stride: 0x20,
            records: vec![scattered.records[0].clone(), scattered.records[2].clone()],
            ..scattered.clone()
        };

        let kept = canonicalize(vec![scattered, sharing]);

        assert_eq!(
            kept.len(),
            2,
            "without the chained shape, shared records are not evidence of misreading"
        );
    }
