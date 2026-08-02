    use super::*;
    use crate::facts::{MaterializationEvaluatorV1, MaterializedImageSourceV1, RomAddressSpace};
    use crate::materialized_image::evaluate_materialized_image_v1;
    use crate::normalize;

    const CODE_PA: u32 = 0x1000;
    const CODE_VA: u32 = 0x8000_1000;
    const SOURCE_PA: u32 = 0x2000;
    const SOURCE_VA: u32 = 0x8000_2000;
    const OUTPUT_PA: u32 = 0x3000;
    const OUTPUT_VA: u32 = 0x8000_3000;
    const RETURN_PC: u32 = 0xffff_fffc;

    fn i(op: u32, rs: u8, rt: u8, imm: i16) -> u32 {
        (op << 26) | (u32::from(rs) << 21) | (u32::from(rt) << 16) | u32::from(imm as u16)
    }

    fn r(rs: u8, rt: u8, rd: u8, funct: u32) -> u32 {
        (u32::from(rs) << 21) | (u32::from(rt) << 16) | (u32::from(rd) << 11) | funct
    }

    fn copy_wrapper(payload_len: usize) -> Vec<u8> {
        [
            i(0x09, 0, 9, payload_len as i16), // addiu t1,zero,len
            i(0x24, 4, 8, 0),                  // lbu t0,0(a0)
            i(0x28, 5, 8, 0),                  // sb t0,0(a1)
            i(0x09, 4, 4, 1),                  // addiu a0,a0,1
            i(0x09, 5, 5, 1),                  // addiu a1,a1,1
            i(0x09, 9, 9, -1),                 // addiu t1,t1,-1
            i(0x05, 9, 0, -6),                 // bne t1,zero,loop
            0,                                 // nop
            r(31, 0, 0, 0x08),                 // jr ra
            0,                                 // nop
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect()
    }

    fn pointer_cell_copy_wrapper() -> Vec<u8> {
        [
            i(0x23, 4, 8, 0),  // lw t0,0(a0): encoded-source cursor
            i(0x23, 5, 9, 0),  // lw t1,0(a1): output cursor
            r(8, 7, 8, 0x21),  // addu t0,t0,a3: skip stream header
            i(0x24, 8, 10, 0), // lbu t2,0(t0)
            i(0x28, 9, 10, 0), // sb t2,0(t1)
            i(0x09, 8, 8, 1),  // addiu t0,t0,1
            i(0x09, 9, 9, 1),  // addiu t1,t1,1
            i(0x09, 6, 6, -1), // addiu a2,a2,-1
            i(0x05, 6, 0, -6), // bne a2,zero,copy
            0,                 // nop
            i(0x2b, 4, 8, 0),  // sw t0,0(a0)
            i(0x2b, 5, 9, 0),  // sw t1,0(a1)
            r(31, 0, 0, 0x08), // jr ra
            0,                 // nop
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect()
    }

    fn prefixed_copy_wrapper(prefix: &[u32], payload_len: usize) -> Vec<u8> {
        prefix
            .iter()
            .copied()
            .flat_map(u32::to_be_bytes)
            .chain(copy_wrapper(payload_len))
            .collect()
    }

    fn stored_stream(payload: &[u8]) -> Vec<u8> {
        let len = payload.len() as u16;
        let mut bytes = vec![0x11, 0x72];
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.push(0x01); // final stored raw-DEFLATE block
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&(!len).to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn fixture() -> (NormalizedRom, EvaluatedImageReceiptV1, Vec<u8>, Vec<u8>) {
        let payload = b"exact transformed payload".to_vec();
        let source = stored_stream(&payload);
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let source_spec = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: rom_offset as u32,
            rom_end: rom_offset as u32 + source.len() as u32,
            cursor: 0,
        };
        let evaluator =
            MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 1 };
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &source_spec,
            &evaluator,
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        (rom, evaluation.receipt().clone(), source, payload)
    }

    fn request<'a>(code: &'a [u8], _payload: &'a [u8]) -> TransformInvocationRequestV1<'a> {
        TransformInvocationRequestV1 {
            entry_pc: CODE_VA,
            return_pc: RETURN_PC,
            a0: SOURCE_VA + 11,
            a1: OUTPUT_VA,
            additional_gpr_seeds: &[],
            expected_output: ExpectedEvaluatedOutputV1::Aggregate,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            additional_allowed_writes: &[],
        }
    }

    #[test]
    fn exact_stored_deflate_payload_copy_binds_receipt_and_transcript() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();

        assert_eq!(result.output(), payload);
        assert_eq!(
            result.certificate().evaluated_image_receipt_sha256,
            evaluated_image_receipt_sha256_v1(&receipt)
        );
        assert!(result
            .certificate()
            .memory_events
            .iter()
            .any(|event| matches!(event, TransformMemoryEventV1::Read { .. })));
        assert!(result
            .certificate()
            .memory_events
            .iter()
            .any(|event| matches!(event, TransformMemoryEventV1::Write { .. })));
        assert_eq!(
            transform_invocation_certificate_sha256_v1(result.certificate()),
            "826e55db18f6a3f8f9f6bdaee8aa4b275efea75429e58009521c379485847cad"
        );
        let repeated = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();
        assert_eq!(repeated.certificate(), result.certificate());
    }

    #[test]
    fn selected_second_stream_certifies_one_invocation_against_aggregate_receipt() {
        let first_payload = b"first stream".to_vec();
        let second_payload = b"second stream selected".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let source_spec = MaterializedImageSourceV1 {
            rom_space: RomAddressSpace::Physical,
            rom_start: rom_offset as u32,
            rom_end: rom_offset as u32 + source.len() as u32,
            cursor: 0,
        };
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &source_spec,
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let code = copy_wrapper(second_payload.len());
        let zeros = vec![0; second_payload.len()];
        let mut request = request(&code, &zeros);
        request.a0 = SOURCE_VA + first_source.len() as u32 + 11;
        request.expected_output = ExpectedEvaluatedOutputV1::Stream { ordinal: 1 };

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();

        assert_eq!(result.output(), second_payload);
        assert_eq!(
            result.certificate().expected_output,
            ExpectedEvaluatedOutputV1::Stream { ordinal: 1 }
        );
        assert_eq!(
            (
                result.certificate().expected_output_start,
                result.certificate().expected_output_end,
            ),
            (
                first_payload.len() as u32,
                (first_payload.len() + second_payload.len()) as u32,
            )
        );
    }

    #[test]
    fn ordered_sequence_binds_shared_pointer_evolution_and_aggregate_output() {
        const SOURCE_CELL_PA: u32 = 0x4000;
        const OUTPUT_CELL_PA: u32 = 0x4004;
        const SOURCE_CELL_VA: u32 = 0x8000_4000;
        const OUTPUT_CELL_VA: u32 = 0x8000_4004;

        let first_payload = b"first ordered stream".to_vec();
        let second_payload = b"second ordered stream with a different length".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let expected_output = [first_payload.as_slice(), second_payload.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: rom_offset as u32,
                rom_end: rom_offset as u32 + source.len() as u32,
                cursor: 0,
            },
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let code = pointer_cell_copy_wrapper();
        let first_seeds = [
            GprSeedV1 {
                register: 6,
                value: first_payload.len() as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let second_seeds = [
            GprSeedV1 {
                register: 6,
                value: second_payload.len() as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let first_source_after = (SOURCE_VA + first_source.len() as u32).to_be_bytes();
        let first_output_after = (OUTPUT_VA + first_payload.len() as u32).to_be_bytes();
        let final_source_after = (SOURCE_VA + source.len() as u32).to_be_bytes();
        let final_output_after = (OUTPUT_VA + expected_output.len() as u32).to_be_bytes();
        let first_expected_mutable = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                bytes: &first_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                bytes: &first_output_after,
            },
        ];
        let second_expected_mutable = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                bytes: &final_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                bytes: &final_output_after,
            },
        ];
        let steps = [
            TransformInvocationStepRequestV1 {
                entry_pc: CODE_VA,
                return_pc: RETURN_PC,
                a0: SOURCE_CELL_VA,
                a1: OUTPUT_CELL_VA,
                additional_gpr_seeds: &first_seeds,
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
                expected_mutable_memory_after: &first_expected_mutable,
            },
            TransformInvocationStepRequestV1 {
                entry_pc: CODE_VA,
                return_pc: RETURN_PC,
                a0: SOURCE_CELL_VA,
                a1: OUTPUT_CELL_VA,
                additional_gpr_seeds: &second_seeds,
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                expected_mutable_memory_after: &second_expected_mutable,
            },
        ];
        let source_cell_initial = SOURCE_VA.to_be_bytes();
        let output_cell_initial = OUTPUT_VA.to_be_bytes();
        let mutable = [
            SharedMutableMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                initial_bytes: &source_cell_initial,
            },
            SharedMutableMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                initial_bytes: &output_cell_initial,
            },
        ];
        let request = TransformInvocationSequenceRequestV1 {
            steps: &steps,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: &code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            shared_mutable_memory: &mutable,
            additional_allowed_writes: &[],
        };

        let result = certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();

        assert_eq!(result.output(), expected_output);
        assert_eq!(result.certificate().steps.len(), 2);
        assert_eq!(
            result.certificate().executed_units,
            result
                .certificate()
                .steps
                .iter()
                .map(|step| step.units.len() as u32)
                .sum::<u32>()
        );
        assert_eq!(
            result.certificate().retired_instructions,
            result
                .certificate()
                .steps
                .iter()
                .map(|step| step.retired_instructions)
                .sum::<u32>()
        );
        let first_source_end = SOURCE_VA + first_source.len() as u32;
        let final_source_end = SOURCE_VA + source.len() as u32;
        let first_output_end = OUTPUT_VA + first_payload.len() as u32;
        let final_output_end = OUTPUT_VA + expected_output.len() as u32;
        let source_commitment = |value: u32| sha256(&value.to_be_bytes());
        let commitment_for = |commitments: &[ContentCommitmentV1], role: &str| {
            commitments
                .iter()
                .find(|commitment| commitment.role == role)
                .unwrap()
                .sha256
                .clone()
        };
        let first = &result.certificate().steps[0];
        let second = &result.certificate().steps[1];
        assert_eq!(
            commitment_for(&first.mutable_memory_before, "source_cursor"),
            source_commitment(SOURCE_VA)
        );
        assert_eq!(
            commitment_for(&first.mutable_memory_after, "source_cursor"),
            source_commitment(first_source_end)
        );
        assert_eq!(
            first.mutable_memory_after, second.mutable_memory_before,
            "the second call must consume the first call's exact mutable state"
        );
        assert_eq!(
            commitment_for(&second.mutable_memory_after, "source_cursor"),
            source_commitment(final_source_end)
        );
        assert_eq!(
            commitment_for(&first.mutable_memory_after, "output_cursor"),
            source_commitment(first_output_end)
        );
        assert_eq!(
            commitment_for(&second.mutable_memory_after, "output_cursor"),
            source_commitment(final_output_end)
        );
        assert_eq!(
            transform_invocation_sequence_certificate_sha256_v1(result.certificate()),
            "3171969d740169ce789ec740b75d6dddb60eb47ace6fc6a1fbfc711f5e6679a1"
        );
        let repeated = certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .unwrap();
        assert_eq!(repeated.certificate(), result.certificate());

        let mut exact_unit_bound = TransformInvocationLimitsV1::default();
        exact_unit_bound.max_units = result.certificate().executed_units;
        assert!(certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &request,
            exact_unit_bound,
        )
        .is_ok());
        exact_unit_bound.max_units -= 1;
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &request,
                exact_unit_bound,
            ),
            Err(TransformInvocationErrorV1::UnitLimitExceeded)
        );

        let mut bounded = TransformInvocationLimitsV1::default();
        bounded.max_instructions = result.certificate().retired_instructions - 1;
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &request,
                bounded,
            ),
            Err(TransformInvocationErrorV1::InstructionLimitExceeded)
        );

        let wrong_final_output_after = (final_output_end + 1).to_be_bytes();
        let wrong_second_expected = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: SOURCE_CELL_PA,
                bytes: &final_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: OUTPUT_CELL_PA,
                bytes: &wrong_final_output_after,
            },
        ];
        let wrong_steps = [
            steps[0].clone(),
            TransformInvocationStepRequestV1 {
                expected_mutable_memory_after: &wrong_second_expected,
                ..steps[1].clone()
            },
        ];
        let wrong_mutable = TransformInvocationSequenceRequestV1 {
            steps: &wrong_steps,
            ..request.clone()
        };
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &wrong_mutable,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::MutableMemoryMismatch {
                ordinal: 1,
                role: "output_cursor".to_owned(),
            })
        );
    }

    #[test]
    fn sequence_rejects_missing_reordered_and_cross_stream_writes() {
        let first_payload = b"first stream".to_vec();
        let second_payload = b"second stream".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: rom_offset as u32,
                rom_end: rom_offset as u32 + source.len() as u32,
                cursor: 0,
            },
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();
        let code = pointer_cell_copy_wrapper();
        let seeds = [
            GprSeedV1 {
                register: 6,
                value: first_payload.len() as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let first_source_after = (SOURCE_VA + first_source.len() as u32).to_be_bytes();
        let first_output_after = (OUTPUT_VA + first_payload.len() as u32).to_be_bytes();
        let first_expected_mutable = [
            CommittedMemoryRangeV1 {
                role: "source_cursor",
                physical_start: 0x4000,
                bytes: &first_source_after,
            },
            CommittedMemoryRangeV1 {
                role: "output_cursor",
                physical_start: 0x4004,
                bytes: &first_output_after,
            },
        ];
        let first = TransformInvocationStepRequestV1 {
            entry_pc: CODE_VA,
            return_pc: RETURN_PC,
            a0: 0x8000_4000,
            a1: 0x8000_4004,
            additional_gpr_seeds: &seeds,
            expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
            expected_mutable_memory_after: &first_expected_mutable,
        };
        let source_cell = SOURCE_VA.to_be_bytes();
        let output_cell = OUTPUT_VA.to_be_bytes();
        let mutable = [
            SharedMutableMemoryRangeV1 {
                role: "source_cursor",
                physical_start: 0x4000,
                initial_bytes: &source_cell,
            },
            SharedMutableMemoryRangeV1 {
                role: "output_cursor",
                physical_start: 0x4004,
                initial_bytes: &output_cell,
            },
        ];
        let one_step = [first.clone()];
        let missing = TransformInvocationSequenceRequestV1 {
            steps: &one_step,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: &code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            shared_mutable_memory: &mutable,
            additional_allowed_writes: &[],
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &missing,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::InvalidInput(_))
        ));

        let valid_ordinals = [
            first.clone(),
            TransformInvocationStepRequestV1 {
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                ..first.clone()
            },
        ];
        let output_alias = [PhysicalRangeV1 {
            start: OUTPUT_PA,
            len: (first_payload.len() + second_payload.len()) as u32,
        }];
        let aliased_output = TransformInvocationSequenceRequestV1 {
            steps: &valid_ordinals,
            additional_allowed_writes: &output_alias,
            ..missing.clone()
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &aliased_output,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::InvalidInput(_))
        ));

        let reordered_steps = [
            TransformInvocationStepRequestV1 {
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                ..first.clone()
            },
            TransformInvocationStepRequestV1 {
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
                ..first.clone()
            },
        ];
        let reordered = TransformInvocationSequenceRequestV1 {
            steps: &reordered_steps,
            ..missing.clone()
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &reordered,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::InvalidInput(_))
        ));

        let oversized_seeds = [
            GprSeedV1 {
                register: 6,
                value: (first_payload.len() + 1) as u64,
            },
            GprSeedV1 {
                register: 7,
                value: 11,
            },
        ];
        let crossing_steps = [
            TransformInvocationStepRequestV1 {
                additional_gpr_seeds: &oversized_seeds,
                ..first.clone()
            },
            TransformInvocationStepRequestV1 {
                additional_gpr_seeds: &seeds,
                expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                ..first
            },
        ];
        let crossing = TransformInvocationSequenceRequestV1 {
            steps: &crossing_steps,
            ..missing
        };
        assert!(matches!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &crossing,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::WriteOutsideAllowed { .. })
        ));
    }

    #[test]
    fn sequence_scratch_must_be_rewritten_before_a_later_step_reads_it() {
        const SCRATCH_PA: u32 = 0x5000;
        const SCRATCH_VA: u64 = 0x8000_5000;

        let first_payload = b"first scratch stream".to_vec();
        let second_payload = b"second scratch stream".to_vec();
        let first_source = stored_stream(&first_payload);
        let second_source = stored_stream(&second_payload);
        let source = [first_source.as_slice(), second_source.as_slice()].concat();
        let rom_offset = 0x80usize;
        let mut rom_bytes = vec![0; (rom_offset + source.len() + 3) & !3];
        rom_bytes[..4].copy_from_slice(&0x8037_1240u32.to_be_bytes());
        rom_bytes[8..12].copy_from_slice(&0x8000_0400u32.to_be_bytes());
        rom_bytes[rom_offset..rom_offset + source.len()].copy_from_slice(&source);
        let rom = normalize(&rom_bytes).unwrap();
        let evaluation = evaluate_materialized_image_v1(
            &rom,
            &FactDb::new(),
            &MaterializedImageSourceV1 {
                rom_space: RomAddressSpace::Physical,
                rom_start: rom_offset as u32,
                rom_end: rom_offset as u32 + source.len() as u32,
                cursor: 0,
            },
            &MaterializationEvaluatorV1::HeaderedRawDeflateSequenceV1 { stream_count: 2 },
            MaterializedImageLimitsV1::default(),
        )
        .unwrap();

        let first_code = prefixed_copy_wrapper(
            &[i(0x28, 12, 11, 0)], // sb t3,0(t4)
            first_payload.len(),
        );
        let second_read_code = prefixed_copy_wrapper(
            &[i(0x24, 12, 11, 0)], // lbu t3,0(t4)
            second_payload.len(),
        );
        let second_rewrite_code = prefixed_copy_wrapper(
            &[
                i(0x28, 12, 11, 0), // sb t3,0(t4)
                i(0x24, 12, 11, 0), // lbu t3,0(t4)
            ],
            second_payload.len(),
        );
        let second_read_entry = CODE_VA + first_code.len() as u32;
        let second_rewrite_entry = second_read_entry + second_read_code.len() as u32;
        let code = [
            first_code.as_slice(),
            second_read_code.as_slice(),
            second_rewrite_code.as_slice(),
        ]
        .concat();
        let scratch_seeds = [
            GprSeedV1 {
                register: 11,
                value: 0xa5,
            },
            GprSeedV1 {
                register: 12,
                value: SCRATCH_VA,
            },
        ];
        let steps_for = |second_entry| {
            [
                TransformInvocationStepRequestV1 {
                    entry_pc: CODE_VA,
                    return_pc: RETURN_PC,
                    a0: SOURCE_VA + 11,
                    a1: OUTPUT_VA,
                    additional_gpr_seeds: &scratch_seeds,
                    expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 0 },
                    expected_mutable_memory_after: &[],
                },
                TransformInvocationStepRequestV1 {
                    entry_pc: second_entry,
                    return_pc: RETURN_PC,
                    a0: SOURCE_VA + first_source.len() as u32 + 11,
                    a1: OUTPUT_VA + first_payload.len() as u32,
                    additional_gpr_seeds: &scratch_seeds,
                    expected_output: ExpectedEvaluatedOutputV1::Stream { ordinal: 1 },
                    expected_mutable_memory_after: &[],
                },
            ]
        };
        let scratch = [PhysicalRangeV1 {
            start: SCRATCH_PA,
            len: 1,
        }];
        let make_request = |steps| TransformInvocationSequenceRequestV1 {
            steps,
            code: KnownTransformCodeImageV1 {
                virtual_start: CODE_VA,
                physical_start: CODE_PA,
                bytes: &code,
            },
            source_physical_start: SOURCE_PA,
            output_physical_start: OUTPUT_PA,
            committed_memory: &[],
            shared_mutable_memory: &[],
            additional_allowed_writes: &scratch,
        };

        let read_before_rewrite = steps_for(second_read_entry);
        assert_eq!(
            certify_transform_invocation_sequence_v1(
                &rom,
                &FactDb::new(),
                evaluation.receipt(),
                &make_request(&read_before_rewrite),
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::ReadOutsideCommitted {
                physical_offset: SCRATCH_PA,
                len: 1,
            })
        );

        let rewrite_before_read = steps_for(second_rewrite_entry);
        assert!(certify_transform_invocation_sequence_v1(
            &rom,
            &FactDb::new(),
            evaluation.receipt(),
            &make_request(&rewrite_before_read),
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn unseeded_register_dependency_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[..4].copy_from_slice(&i(0x09, 10, 9, payload.len() as i16).to_be_bytes());
        let zeros = vec![0; payload.len()];
        let unseeded_request = request(&code, &zeros);

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &unseeded_request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::UnseededRegisterRead { register: 10, .. })
        ));

        let seeds = [GprSeedV1 {
            register: 10,
            value: 0,
        }];
        let mut seeded_request = request(&code, &zeros);
        seeded_request.additional_gpr_seeds = &seeds;
        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &seeded_request,
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn read_outside_committed_source_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.a0 = 0x8000_7000;

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::ReadOutsideCommitted { .. })
        ));
    }

    #[test]
    fn adjacent_commitments_cover_one_boundary_spanning_read() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = i(0x23, 10, 8, 0).to_be_bytes().to_vec(); // lw t0,0(t2)
        code.extend(copy_wrapper(payload.len()));
        let left = [1u8, 2];
        let right = [3u8, 4];
        let committed = [
            CommittedMemoryRangeV1 {
                role: "left_half",
                physical_start: 0x7000,
                bytes: &left,
            },
            CommittedMemoryRangeV1 {
                role: "right_half",
                physical_start: 0x7002,
                bytes: &right,
            },
        ];
        let seeds = [GprSeedV1 {
            register: 10,
            value: 0x8000_7000,
        }];
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.committed_memory = &committed;
        request.additional_gpr_seeds = &seeds;

        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn output_mismatch_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[4..8].copy_from_slice(&i(0x09, 0, 8, 0).to_be_bytes());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);

        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::OutputMismatch)
        );
    }

    #[test]
    fn matching_initial_output_without_stores_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let code = [r(31, 0, 0, 0x08), 0]
            .into_iter()
            .flat_map(u32::to_be_bytes)
            .collect::<Vec<_>>();
        let request = request(&code, &payload);

        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::OutputNotFullyWritten {
                first_unwritten_physical_offset: OUTPUT_PA,
            })
        );
    }

    #[test]
    fn write_outside_allowed_ranges_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.a1 = 0x8000_7000;

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::WriteOutsideAllowed { .. })
        ));
    }

    #[test]
    fn code_write_is_rejected_before_allowed_range_classification() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut request = request(&code, &zeros);
        request.a1 = CODE_VA;

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::CodeWrite { .. })
        ));
    }

    #[test]
    fn partial_unaligned_store_family_is_rejected() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[8..12].copy_from_slice(&i(0x2a, 5, 8, 0).to_be_bytes()); // swl t0,0(a1)
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);

        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::UnsupportedInstruction { .. })
        ));
    }

    #[test]
    fn branch_likely_register_dependencies_are_audited() {
        let (rom, receipt, _source, payload) = fixture();
        let mut code = copy_wrapper(payload.len());
        code[6 * 4..7 * 4].copy_from_slice(&i(0x15, 9, 0, -6).to_be_bytes()); // bnel
        let zeros = vec![0; payload.len()];
        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request(&code, &zeros),
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());

        code[6 * 4..7 * 4].copy_from_slice(&i(0x15, 10, 0, -6).to_be_bytes());
        assert!(matches!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &request(&code, &zeros),
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::UnseededRegisterRead { register: 10, .. })
        ));

        let mut annulled = [
            i(0x15, 0, 0, 1),  // bnel zero,zero,+1: not taken
            i(0x23, 10, 8, 0), // lw t0,0(t2): annulled, t2 is unseeded
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
        annulled.extend(copy_wrapper(payload.len()));
        assert!(certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request(&annulled, &zeros),
            TransformInvocationLimitsV1::default(),
        )
        .is_ok());
    }

    #[test]
    fn code_escape_and_instruction_saturation_are_typed() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let mut escaped = request(&code, &zeros);
        escaped.entry_pc = CODE_VA + code.len() as u32;
        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &escaped,
                TransformInvocationLimitsV1::default(),
            ),
            Err(TransformInvocationErrorV1::CodeEscape {
                pc: escaped.entry_pc
            })
        );

        let bounded = request(&code, &zeros);
        let mut limits = TransformInvocationLimitsV1::default();
        limits.max_instructions = 1;
        assert_eq!(
            certify_transform_wrapper_invocation_v1(
                &rom,
                &FactDb::new(),
                &receipt,
                &bounded,
                limits,
            ),
            Err(TransformInvocationErrorV1::InstructionLimitExceeded)
        );
    }

    fn ambient_read(_: GuestReadEvent) {}

    thread_local! {
        static AMBIENT_WRITES: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    fn ambient_write(_: GuestWriteEvent) {
        AMBIENT_WRITES.with(|count| count.set(count.get() + 1));
    }

    #[test]
    fn caller_read_observer_is_isolated_and_restored() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        let previous = set_read_observer(Some(ambient_read));

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        );
        let restored = set_read_observer(previous);

        assert!(result.is_ok());
        assert!(restored.is_some());
    }

    #[test]
    fn caller_write_observer_is_isolated_from_setup_and_execution() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        AMBIENT_WRITES.with(|count| count.set(0));
        let previous = set_write_observer(Some(ambient_write));

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        );
        let restored = set_write_observer(previous);

        assert!(result.is_ok());
        assert_eq!(AMBIENT_WRITES.with(std::cell::Cell::get), 0);
        assert!(restored.is_some());
    }

    fn executable_changed(_: GuestWriteEvent) -> fn64_recomp_rs::GuestWriteBoundary {
        fn64_recomp_rs::GuestWriteBoundary::ExecutableChanged
    }

    #[test]
    fn isolated_run_preserves_caller_writer_session_and_pending_request() {
        let (rom, receipt, _source, payload) = fixture();
        let code = copy_wrapper(payload.len());
        let zeros = vec![0; payload.len()];
        let request = request(&code, &zeros);
        assert!(
            fn64_recomp_rs::set_guest_write_boundary_observer(Some(executable_changed)).is_none()
        );
        fn64_recomp_rs::notify_cpu_instruction_store(0x4000, 4);
        let token_before = fn64_recomp_rs::guest_write_token(0x4000, 4);

        let result = certify_transform_wrapper_invocation_v1(
            &rom,
            &FactDb::new(),
            &receipt,
            &request,
            TransformInvocationLimitsV1::default(),
        );

        assert!(result.is_ok());
        assert_eq!(fn64_recomp_rs::guest_write_token(0x4000, 4), token_before);
        assert!(fn64_recomp_rs::take_executable_write_boundary());
        assert!(fn64_recomp_rs::set_guest_write_boundary_observer(None).is_some());
    }
