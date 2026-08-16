    use super::*;
    use crate::cfg::BlockTerminator;
    use crate::facts::{
        evaluated_image_receipt_sha256_v1, EvaluatedImageReceiptV1, Fact,
        MaterializationEvaluatorV1, MaterializedImageSourceV1, ProofState, RomAddressSpace,
    };
    use crate::materialized_image::evaluate_materialized_image_v1;
    use flate2::{write::DeflateEncoder, Compression};
    use fn64_cpu_runtime::{BankId, CpuFaultKind, ExecutionKey, GuestPc, InstructionBudget};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    const BANK_A: u64 = 0x11;
    const BANK_B: u64 = 0x22;
    const ENTRY_PC: u32 = 0x8000_1000;
    const HOLE_PC: u32 = 0x8000_1010;
    const UNIQUE_B_PC: u32 = 0x8000_2000;
    const ROM_BASE: u32 = 0x1000;
    const JR_RA: u32 = 0x03e0_0008;
    const NOP: u32 = 0x0000_0000;

    fn rom_with(words: &[u32]) -> NormalizedRom {
        let bank = words
            .iter()
            .flat_map(|word| word.to_be_bytes())
            .collect::<Vec<_>>();
        let mut bytes = vec![0u8; ROM_BASE as usize + bank.len()];
        bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
        bytes[ROM_BASE as usize..].copy_from_slice(&bank);
        crate::normalize(&bytes).unwrap()
    }

    fn raw_deflate_stream(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut stream = Vec::with_capacity(6 + compressed.len());
        stream.extend_from_slice(&[0x11, 0x72]);
        stream.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        stream.extend_from_slice(&compressed);
        stream
    }

    fn materialized_fixture() -> (NormalizedRom, FactDb, EvaluatedImageReceiptV1, Vec<u8>) {
        let output = [NOP, JR_RA, NOP]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let encoded = raw_deflate_stream(&output);
        let mut rom_bytes = vec![0; (ROM_BASE as usize + encoded.len() + 3) & !3];
        rom_bytes[0..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
        rom_bytes[ROM_BASE as usize..ROM_BASE as usize + encoded.len()].copy_from_slice(&encoded);
        let rom = crate::normalize(&rom_bytes).unwrap();
        let source = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: ROM_BASE,
            rom_end: ROM_BASE + encoded.len() as u32,
            cursor: 0,
        };
        let mut facts = FactDb::new();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &facts,
            &source,
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let receipt = evaluation.receipt().clone();
        let image = facts.insert(Fact::EvaluatedImage {
            bank: "boot".into(),
            va_start: ENTRY_PC,
            va_end: ENTRY_PC + output.len() as u32,
            receipt: receipt.clone(),
        });
        facts
            .conclude(
                "bank:boot",
                ProofState::Proven,
                vec![image],
                "test evaluated image",
            )
            .unwrap();
        (rom, facts, receipt, output)
    }

    fn reachable_block(
        start_va: u32,
        end_va: u32,
        terminator: BlockTerminator,
    ) -> ReachableCodeBlock {
        ReachableCodeBlock {
            bank: "boot".into(),
            start_va,
            end_va,
            authoritative_roots: crate::block_proof::AuthoritativeReachabilityRoots::new([
                ENTRY_PC,
            ])
            .unwrap(),
            backing: BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_BASE + (start_va - ENTRY_PC),
                rom_end: ROM_BASE + (end_va - ENTRY_PC),
            },
            terminator,
        }
    }

    #[test]
    fn observed_execution_augments_exact_words_and_required_delay_slot() {
        let jal = 0x0c00_0800;
        let rom = rom_with(&[NOP, jal, NOP, NOP]);
        let digest = crate::trace::NormalizedRomDigest::try_from(rom.sha256.clone()).unwrap();
        let report = crate::trace::IngestReport {
            header: crate::trace::TraceHeader {
                schema_version: crate::trace::TRACE_SCHEMA_VERSION,
                normalized_rom_sha256: digest,
                trace_id: "observed-aot-test".into(),
                producer: "synthetic-test".into(),
            },
            completion: crate::trace::TraceCompletion::Completed,
            final_sequence: 2,
            counts: crate::trace::TraceEventCounts {
                executed_pc: 1,
                ..Default::default()
            },
            observations_with_unknown_bank: 0,
            facts: vec![crate::trace::ObservedTraceFact::ExecutedPc {
                sequence: 1,
                pc: crate::trace::ObservedAddress {
                    address: ENTRY_PC + 4,
                    bank: crate::trace::BankContext::Known {
                        bank: "boot".into(),
                        activation: 0,
                    },
                },
            }],
            exhaustiveness: Vec::new(),
        };
        let mut bank = MaterializedPackedBank {
            bank: "boot".into(),
            bank_id: BANK_A,
            blocks: vec![MaterializedPackedBlock {
                start_va: ENTRY_PC,
                words: vec![NOP],
            }],
        };

        let augmented = augment_with_observed_execution(
            &mut bank, &report, &rom, "boot", 0, ROM_BASE, ENTRY_PC, 16,
        )
        .unwrap();
        assert_eq!(
            augmented,
            ObservedExecutionAugmentReport {
                observed_words: 1,
                required_delay_slot_words: 1,
                newly_admitted_words: 2,
            }
        );
        assert_eq!(bank.blocks.len(), 1);
        assert_eq!(bank.blocks[0].start_va, ENTRY_PC);
        assert_eq!(bank.blocks[0].words, vec![NOP, jal, NOP]);
    }

    #[test]
    fn severed_proven_delay_slot_is_reattached_to_its_control_block() {
        let rom = rom_with(&[NOP, JR_RA, NOP]);
        let control = reachable_block(
            ENTRY_PC,
            ENTRY_PC + 8,
            BlockTerminator::Fallthrough { next: ENTRY_PC + 8 },
        );
        let word_class = BTreeMap::from([
            (ENTRY_PC, WordClass::ProvenCode),
            (ENTRY_PC + 4, WordClass::ProvenCode),
            (ENTRY_PC + 8, WordClass::ProvenCode),
        ]);
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&control)],
            &word_class,
            &rom,
            &crate::facts::FactDb::new(),
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 0x0c);
        assert_eq!(
            geometry[0].backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_BASE,
                rom_end: ROM_BASE + 0x0c,
            }
        );
    }

    #[test]
    fn admitted_delay_slot_is_not_duplicated() {
        let rom = rom_with(&[NOP, JR_RA, NOP, JR_RA, NOP]);
        let control = reachable_block(
            ENTRY_PC,
            ENTRY_PC + 8,
            BlockTerminator::Fallthrough { next: ENTRY_PC + 8 },
        );
        let next = reachable_block(ENTRY_PC + 8, ENTRY_PC + 0x14, BlockTerminator::Return);
        let word_class = (0..5)
            .map(|index| (ENTRY_PC + index * 4, WordClass::ProvenCode))
            .collect();
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&control), PackBlockView::from(&next)],
            &word_class,
            &rom,
            &crate::facts::FactDb::new(),
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 8);
        assert_eq!(geometry[1].start_va, ENTRY_PC + 8);
    }

    #[test]
    fn control_shaped_word_with_unproven_successor_is_not_extended() {
        let rom = rom_with(&[NOP, JR_RA, JR_RA]);
        let control = reachable_block(ENTRY_PC, ENTRY_PC + 0x0c, BlockTerminator::Return);
        let word_class = BTreeMap::from([
            (ENTRY_PC, WordClass::ProvenCode),
            (ENTRY_PC + 4, WordClass::ProvenCode),
            (ENTRY_PC + 8, WordClass::ProvenCode),
        ]);
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&control)],
            &word_class,
            &rom,
            &crate::facts::FactDb::new(),
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 0x0c);
    }

    #[test]
    fn materialized_pack_rederives_receipt_and_extends_tagged_delay_slot() {
        let (rom, facts, receipt, output) = materialized_fixture();
        let receipt_sha256 = evaluated_image_receipt_sha256_v1(&receipt);
        let block = ReachableCodeBlock {
            bank: "boot".into(),
            start_va: ENTRY_PC,
            end_va: ENTRY_PC + 8,
            authoritative_roots: crate::block_proof::AuthoritativeReachabilityRoots::new([
                ENTRY_PC,
            ])
            .unwrap(),
            backing: BankBackingSpanV1::Materialized {
                receipt_sha256: receipt_sha256.clone(),
                output_start: 0,
                output_end: 8,
            },
            terminator: BlockTerminator::Fallthrough { next: ENTRY_PC + 8 },
        };
        let word_class = BTreeMap::from([
            (ENTRY_PC, WordClass::ProvenCode),
            (ENTRY_PC + 4, WordClass::ProvenCode),
            (ENTRY_PC + 8, WordClass::ProvenCode),
        ]);
        let geometry = complete_severed_delay_slots(
            &[PackBlockView::from(&block)],
            &word_class,
            &rom,
            &facts,
            MaterializedImageLimitsV1::default(),
            &mut MaterializedBackingSpanCacheV1::default(),
        )
        .unwrap();
        assert_eq!(geometry[0].end_va, ENTRY_PC + 12);
        assert_eq!(
            geometry[0].backing,
            BankBackingSpanV1::Materialized {
                receipt_sha256: receipt_sha256.clone(),
                output_start: 0,
                output_end: 12,
            }
        );

        let pack = BlockPackV1 {
            schema_version: BLOCK_PACK_SCHEMA_V3,
            normalized_rom_sha256: rom.sha256.clone(),
            banks: vec![PackedBankV1 {
                bank: "boot".into(),
                bank_id: BANK_A,
                blocks: vec![PackedBlockV1 {
                    start_va: ENTRY_PC,
                    end_va: ENTRY_PC + 12,
                    backing: geometry[0].backing.clone(),
                    bytes_sha256: sha256_hex(&output),
                    terminator: block.terminator.clone(),
                }],
            }],
        };
        let materialized = materialize_block_pack_with_facts(&pack, &rom, Some(&facts)).unwrap();
        assert_eq!(materialized[0].blocks[0].words, vec![NOP, JR_RA, NOP]);
        let wire = serde_json::to_string(&pack).unwrap();
        assert!(wire.contains("\"kind\":\"materialized\""));
        assert!(!wire.contains("\"words\""));

        let mut tampered_receipt = receipt;
        tampered_receipt.streams[0].output_sha256 = "00".repeat(32);
        let tampered_digest = evaluated_image_receipt_sha256_v1(&tampered_receipt);
        let mut tampered_facts = FactDb::new();
        let tampered_image = tampered_facts.insert(Fact::EvaluatedImage {
            bank: "boot".into(),
            va_start: ENTRY_PC,
            va_end: ENTRY_PC + 12,
            receipt: tampered_receipt,
        });
        tampered_facts
            .conclude(
                "bank:boot",
                ProofState::Proven,
                vec![tampered_image],
                "test tampered evaluated image",
            )
            .unwrap();
        let mut tampered_pack = pack;
        let BankBackingSpanV1::Materialized { receipt_sha256, .. } =
            &mut tampered_pack.banks[0].blocks[0].backing
        else {
            panic!("test pack is materialized")
        };
        *receipt_sha256 = tampered_digest;
        assert!(matches!(
            materialize_block_pack_with_facts(&tampered_pack, &rom, Some(&tampered_facts),),
            Err(BlockPackError::EvaluatedImageRederivation {
                error: MaterializedImageErrorV1::ReceiptMismatch { .. },
                ..
            })
        ));
    }

    #[test]
    fn legacy_v1_v2_block_wires_deserialize_to_affine_backing() {
        let terminator = serde_json::to_value(BlockTerminator::Return).unwrap();
        let v1 = serde_json::json!({
            "start_va": ENTRY_PC,
            "end_va": ENTRY_PC + 4,
            "rom_start": ROM_BASE,
            "rom_end": ROM_BASE + 4,
            "bytes_sha256": "11".repeat(32),
            "terminator": terminator,
        });
        let v1: PackedBlockV1 = serde_json::from_value(v1).unwrap();
        assert_eq!(
            v1.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Physical,
                rom_start: ROM_BASE,
                rom_end: ROM_BASE + 4,
            }
        );
        validate_schema_backing(BLOCK_PACK_SCHEMA_V1, "boot", &v1).unwrap();
        validate_packed_geometry("boot", std::slice::from_ref(&v1)).unwrap();

        let v2 = serde_json::json!({
            "start_va": ENTRY_PC,
            "end_va": ENTRY_PC + 4,
            "rom_space": "Virtual",
            "rom_start": 0x2000,
            "rom_end": 0x2004,
            "bytes_sha256": "22".repeat(32),
            "terminator": BlockTerminator::Return,
        });
        let v2: PackedBlockV1 = serde_json::from_value(v2).unwrap();
        assert_eq!(
            v2.backing,
            BankBackingSpanV1::RomAffine {
                rom_space: RomAddressSpace::Virtual,
                rom_start: 0x2000,
                rom_end: 0x2004,
            }
        );
        validate_schema_backing(BLOCK_PACK_SCHEMA_V2, "boot", &v2).unwrap();
        validate_packed_geometry("boot", std::slice::from_ref(&v2)).unwrap();
    }

    fn synthetic_pack() -> (BlockPackV1, NormalizedRom) {
        let words = [0x2402_0007u32, 0, 0x2402_0009, 0x2403_0005];
        let mut bytes = vec![0u8; ROM_BASE as usize + words.len() * 4];
        bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        bytes[8..12].copy_from_slice(&ENTRY_PC.to_be_bytes());
        for (index, word) in words.into_iter().enumerate() {
            let start = ROM_BASE as usize + index * 4;
            bytes[start..start + 4].copy_from_slice(&word.to_be_bytes());
        }
        let rom = crate::normalize(&bytes).unwrap();
        let block = |start_va: u32, word_index: u32| {
            let rom_start = ROM_BASE + word_index * 4;
            PackedBlockV1 {
                start_va,
                end_va: start_va + 4,
                backing: BankBackingSpanV1::RomAffine {
                    rom_space: crate::facts::RomAddressSpace::Physical,
                    rom_start,
                    rom_end: rom_start + 4,
                },
                bytes_sha256: sha256_hex(&rom.bytes[rom_start as usize..rom_start as usize + 4]),
                terminator: crate::cfg::BlockTerminator::Fallthrough { next: start_va + 4 },
            }
        };
        (
            BlockPackV1 {
                schema_version: BLOCK_PACK_SCHEMA_V1,
                normalized_rom_sha256: rom.sha256.clone(),
                banks: vec![
                    PackedBankV1 {
                        bank: "resident".into(),
                        bank_id: BANK_A,
                        blocks: vec![block(ENTRY_PC, 0), block(0x8000_1020, 1)],
                    },
                    PackedBankV1 {
                        bank: "overlay".into(),
                        bank_id: BANK_B,
                        blocks: vec![block(ENTRY_PC, 2), block(UNIQUE_B_PC, 3)],
                    },
                ],
            },
            rom,
        )
    }

    fn source_config() -> BlockProgramSourceConfig {
        BlockProgramSourceConfig {
            entry: ExecutionKey::new(BankId::new(BANK_A), GuestPc::new(ENTRY_PC)),
            instruction_budget: InstructionBudget::new(2).unwrap(),
        }
    }

    #[test]
    fn block_program_source_is_deterministic_sparse_and_identity_bound() {
        let (pack, rom) = synthetic_pack();
        let source = emit_block_program_source(&pack, &rom, source_config()).unwrap();
        let mut reordered = pack.clone();
        reordered.banks.reverse();
        for bank in &mut reordered.banks {
            bank.blocks.reverse();
        }
        assert_eq!(
            source,
            emit_block_program_source(&reordered, &rom, source_config()).unwrap()
        );
        assert_eq!(
            source
                .matches("GeneratedBankRunner::new_with_artifact_identity")
                .count(),
            2
        );
        assert!(!source.contains("GeneratedBankRunner::new("));
        assert!(source.contains("0x80001000..=0x80001003 | 0x80001020..=0x80001023"));
        assert!(!source.contains("0x80001004..=0x8000101F"));
        assert!(source.contains("pub fn entry_lookup(target_pc: GuestPc)"));
        assert!(source.contains("CpuFaultKind::AmbiguousPc"));
    }

    #[test]
    fn block_program_source_rejects_unadmitted_entry_and_malformed_pack() {
        let (pack, rom) = synthetic_pack();
        let config = BlockProgramSourceConfig {
            entry: ExecutionKey::new(BankId::new(BANK_A), GuestPc::new(HOLE_PC)),
            instruction_budget: InstructionBudget::new(2).unwrap(),
        };
        assert!(matches!(
            emit_block_program_source(&pack, &rom, config),
            Err(BlockProgramSourceError::EntryFault(fn64_cpu_runtime::CpuFault {
                at,
                kind: CpuFaultKind::UnmappedPc {
                    bank_start: ENTRY_PC,
                    bank_end: 0x8000_1024,
                },
            })) if at == config.entry
        ));

        let mut wrong_schema = pack.clone();
        // A version newer than any this build supports, so the check stays
        // meaningful as supported versions are added.
        wrong_schema.schema_version = BLOCK_PACK_SCHEMA_V3 + 1;
        assert!(matches!(
            materialize_block_pack(&wrong_schema, &rom),
            Err(BlockPackError::UnsupportedSchema { .. })
        ));

        let mut false_legacy_virtual = pack.clone();
        let BankBackingSpanV1::RomAffine { rom_space, .. } =
            &mut false_legacy_virtual.banks[0].blocks[0].backing
        else {
            panic!("synthetic V1 block is affine")
        };
        *rom_space = RomAddressSpace::Virtual;
        assert!(matches!(
            materialize_block_pack_with_facts(
                &false_legacy_virtual,
                &rom,
                Some(&crate::facts::FactDb::new()),
            ),
            Err(BlockPackError::LegacySchemaVirtualBacking {
                bank,
                start_va: ENTRY_PC,
            }) if bank == "resident"
        ));

        let mut malformed = pack;
        malformed.banks[0].blocks[0].end_va += 4;
        assert!(matches!(
            materialize_block_pack(&malformed, &rom),
            Err(BlockPackError::InvalidGeometry { .. })
        ));

        let (mut trailing_bytes, rom) = synthetic_pack();
        let BankBackingSpanV1::RomAffine { rom_end, .. } =
            &mut trailing_bytes.banks[0].blocks[0].backing
        else {
            panic!("synthetic V1 block is affine")
        };
        *rom_end += 1;
        assert!(matches!(
            materialize_block_pack(&trailing_bytes, &rom),
            Err(BlockPackError::InvalidGeometry { .. })
        ));
    }

    #[test]
    fn generated_block_program_compiles_executes_and_rejects_ambiguity() {
        let (pack, rom) = synthetic_pack();
        let source = emit_block_program_source(&pack, &rom, source_config()).unwrap();
        let wrapper = format!(
            r#"{source}

fn main() {{
    let artifact = ProgramArtifactIdentity::new([0xA5; 32]);
    let program = build_block_program(artifact).unwrap();
    assert_eq!(entry(), ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({ENTRY_PC})));
    assert_eq!(instruction_budget().get(), 2);

    let evidence = program.evidence_snapshot();
    assert_eq!(evidence.banks.len(), 2);
    assert!(evidence.banks.iter().all(|bank| bank.runner_artifact_identity == artifact));
    assert_eq!(evidence.banks[0].spans.len(), 2);
    assert_eq!(evidence.banks[0].spans[0].vram_start, GuestPc::new({ENTRY_PC}));
    assert_eq!(evidence.banks[0].spans[1].vram_start, GuestPc::new(0x8000_1020));

    assert!(matches!(
        entry_lookup(GuestPc::new({ENTRY_PC})),
        Err(CpuFault {{
            kind: CpuFaultKind::AmbiguousPc {{
                first_candidate,
                second_candidate,
                candidate_count: 2,
            }},
            ..
        }}) if first_candidate == BankId::new({BANK_A}) && second_candidate == BankId::new({BANK_B})
    ));
    assert_eq!(
        transfer_lookup(BankId::new({BANK_A}), GuestPc::new({ENTRY_PC})).unwrap(),
        ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({ENTRY_PC}))
    );
    assert_eq!(
        transfer_lookup(BankId::new({BANK_B}), GuestPc::new({ENTRY_PC})).unwrap(),
        ExecutionKey::new(BankId::new({BANK_B}), GuestPc::new({ENTRY_PC}))
    );
    assert_eq!(
        transfer_lookup(BankId::new({BANK_A}), GuestPc::new({UNIQUE_B_PC})).unwrap(),
        ExecutionKey::new(BankId::new({BANK_B}), GuestPc::new({UNIQUE_B_PC}))
    );
    assert!(matches!(
        entry_lookup(GuestPc::new({HOLE_PC})),
        Err(CpuFault {{
            at,
            kind: CpuFaultKind::UnmappedPc {{
                bank_start: {ENTRY_PC},
                bank_end: 0x8000_1024,
            }},
        }}) if at == ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({HOLE_PC}))
    ));
    assert!(matches!(
        transfer_lookup(BankId::new({BANK_A}), GuestPc::new({HOLE_PC})),
        Err(CpuFault {{
            at,
            kind: CpuFaultKind::UnmappedPc {{
                bank_start: {ENTRY_PC},
                bank_end: 0x8000_1024,
            }},
        }}) if at == ExecutionKey::new(BankId::new({BANK_A}), GuestPc::new({HOLE_PC}))
    ));

    let mut backing = vec![0u8; fn64_cpu_runtime::RDRAM_LEN];
    let mut mem = Rdram::new(&mut backing);
    let mut ctx = RecompContext::default();
    let run = program.run(entry(), instruction_budget(), &mut ctx, &mut mem);
    assert_eq!(ctx.r_u32(2), 7);
    assert_eq!(run.instructions, 1);
    assert_eq!(
        run.exit,
        BlockExit::ResolveTransfer {{
            source_bank: BankId::new({BANK_A}),
            target_pc: GuestPc::new(0x8000_1004),
        }}
    );
}}
"#
        );
        compile_and_run(&wrapper);
    }

    fn compile_and_run(source: &str) {
        let deps = current_dependency_dir();
        let rlib = current_recomp_rlib(&deps);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let temp = std::env::temp_dir().join(format!(
            "fn64-block-program-source-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&temp).unwrap();
        let source_path = temp.join("main.rs");
        let binary_path = temp.join("generated-block-program");
        std::fs::write(&source_path, source).unwrap();
        let compile = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
            .arg("--edition=2021")
            .arg(&source_path)
            .arg("--extern")
            .arg(format!("fn64_cpu_runtime={}", rlib.display()))
            .arg("-L")
            .arg(format!("dependency={}", deps.display()))
            .arg("-o")
            .arg(&binary_path)
            .output()
            .unwrap();
        assert!(
            compile.status.success(),
            "generated source failed to compile:\nstdout:\n{}\nstderr:\n{}\nsource:\n{}",
            String::from_utf8_lossy(&compile.stdout),
            String::from_utf8_lossy(&compile.stderr),
            source
        );
        let run = Command::new(&binary_path).output().unwrap();
        assert!(
            run.status.success(),
            "generated source failed to execute:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    fn current_dependency_dir() -> PathBuf {
        let executable = std::env::current_exe().unwrap();
        executable
            .parent()
            .expect("fn64-discover test executable has a dependency directory")
            .to_owned()
    }

    fn current_recomp_rlib(deps: &Path) -> PathBuf {
        std::fs::read_dir(deps)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("libfn64_cpu_runtime-") && name.ends_with(".rlib")
                    })
            })
            .max_by_key(|path| {
                path.metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
            })
            .expect("fn64-cpu-runtime rlib is beside fn64-discover test executable")
    }
