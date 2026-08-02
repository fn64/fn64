    use super::*;

    const START: u32 = 0x8000_1000;
    const CALLEE: u32 = 0x8000_4000;

    fn i(op: u32, rs: u8, rt: u8, immediate: i16) -> u32 {
        (op << 26) | ((rs as u32) << 21) | ((rt as u32) << 16) | immediate as u16 as u32
    }

    fn jal(target: u32) -> u32 {
        (0x03 << 26) | ((target >> 2) & 0x03ff_ffff)
    }

    fn sw(base: u8, value: u8, offset: i16) -> u32 {
        i(0x2b, base, value, offset)
    }

    fn canonical_call(device: u32, dram: u32, size: u32) -> Vec<u32> {
        vec![
            i(0x09, 29, 29, -0x30),
            i(0x09, 0, 6, 0),
            i(0x0f, 0, 7, (device >> 16) as i16),
            i(0x0d, 7, 7, device as i16),
            i(0x0f, 0, 8, (dram >> 16) as i16),
            i(0x0d, 8, 8, dram as i16),
            sw(29, 8, 0x10),
            i(0x0f, 0, 9, (size >> 16) as i16),
            i(0x0d, 9, 9, size as i16),
            sw(29, 9, 0x14),
            jal(CALLEE),
            0,
        ]
    }

    fn canonical_epi_call(device: u32, dram: u32, size: u32, global: bool) -> Vec<u32> {
        let message_setup = if global {
            vec![i(0x0f, 0, 5, 0x8001u16 as i16), i(0x0d, 5, 5, 0x2000)]
        } else {
            vec![i(0x09, 29, 5, 0x18)]
        };
        let mut words = vec![
            i(0x09, 29, 29, -0x40),
            i(0x0f, 0, 4, 0x8000u16 as i16),
            i(0x0d, 4, 4, 0x3000),
        ];
        words.extend(message_setup);
        words.extend([
            i(0x09, 0, 6, 0),
            i(0x0f, 0, 8, (dram >> 16) as i16),
            i(0x0d, 8, 8, dram as i16),
            sw(5, 8, 0x08),
            i(0x0f, 0, 9, (device >> 16) as i16),
            i(0x0d, 9, 9, device as i16),
            sw(5, 9, 0x0c),
            i(0x0f, 0, 10, (size >> 16) as i16),
            i(0x0d, 10, 10, size as i16),
            sw(5, 10, 0x10),
            jal(CALLEE),
            0,
        ]);
        words
    }

    #[test]
    fn recovers_exact_read_geometry_but_keeps_candidate_type() {
        let words = canonical_call(0x0012_3400, 0x8030_0000, 0x2400);
        let slices = slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap();
        assert_eq!(slices.len(), 1);
        assert_eq!(slices[0].direction.proven(), Some(&PiDmaDirection::ToRdram));
        assert_eq!(
            slices[0].device_address.proven(),
            Some(&PiDeviceAddress::new(0x0012_3400))
        );
        assert_eq!(
            slices[0].rdram_address.proven(),
            Some(&RdramAddress::new(0x0030_0000))
        );
        assert_eq!(slices[0].byte_count.proven().map(|v| v.get()), Some(0x2400));
        assert_eq!(
            slices[0].candidate(),
            Some(StaticPiDmaCandidate {
                call_pc: VirtualAddress::new(START + 40),
                direction: PiDmaDirection::ToRdram,
                device_address: PiDeviceAddress::new(0x0012_3400),
                rdram_address: RdramAddress::new(0x0030_0000),
                byte_count: NonZeroU32::new(0x2400).unwrap(),
            })
        );
    }

    #[test]
    fn delay_slot_write_is_visible_to_callee() {
        let mut words = canonical_call(0x1000, 0x8000_2000, 0x80);
        words[9] = 0;
        words[11] = sw(29, 9, 0x14);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert_eq!(slice.byte_count.proven().map(|v| v.get()), Some(0x80));
    }

    #[test]
    fn recovers_epi_message_geometry_from_stack_local_object() {
        let words = canonical_epi_call(0x0012_3400, 0x8030_0000, 0x2400, false);
        let slices = slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap();
        assert_eq!(slices.len(), 1);
        let slice = &slices[0];
        assert_eq!(slice.direction.proven(), Some(&PiDmaDirection::ToRdram));
        assert_eq!(
            slice.device_address.proven(),
            Some(&PiDeviceAddress::new(0x0012_3400))
        );
        assert_eq!(
            slice.rdram_address.proven(),
            Some(&RdramAddress::new(0x0030_0000))
        );
        assert_eq!(
            slice.byte_count.proven().map(|size| size.get()),
            Some(0x2400)
        );
        assert!(matches!(
            slice.message_pointer,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::StackPointerUnresolved]
        ));
        assert!(slice.handle_pointer.proven().is_some());
        assert!(slice.candidate().is_some());
    }

    #[test]
    fn recovers_epi_message_geometry_from_global_object() {
        let words = canonical_epi_call(0x2000, 0xa000_4000, 0x80, true);
        let slice = &slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert_eq!(
            slice.message_pointer.proven(),
            Some(&VirtualAddress::new(0x8001_2000))
        );
        assert_eq!(
            slice.rdram_address.proven(),
            Some(&RdramAddress::new(0x4000))
        );
    }

    #[test]
    fn epi_delay_slot_message_store_is_visible() {
        let mut words = canonical_epi_call(0x2000, 0x8000_4000, 0x80, false);
        let call = words.len() - 2;
        words[call - 1] = 0;
        words[call + 1] = sw(5, 10, 0x10);
        let slice = &slice_os_epi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert_eq!(slice.byte_count.proven().map(|size| size.get()), Some(0x80));
    }

    #[test]
    fn missing_epi_message_field_and_ambiguous_store_alias_stay_open() {
        let mut missing = canonical_epi_call(0x2000, 0x8000_4000, 0x80, false);
        let size_store = missing.len() - 3;
        missing[size_store] = 0;
        let slice = &slice_os_epi_start_dma_calls(
            &missing,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::MessageFieldNotWritten { offset: 0x10 }]
        ));

        let mut aliased = canonical_epi_call(0x2000, 0x8000_4000, 0x80, false);
        let call = aliased.len() - 2;
        aliased.insert(call, sw(11, 10, 0));
        let slice = &slice_os_epi_start_dma_calls(
            &aliased,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if matches!(blockers.as_slice(), [SliceBlocker::PotentialStackAlias { .. }])
        ));
    }

    #[test]
    fn unresolved_load_and_missing_stack_write_stay_open() {
        let mut words = canonical_call(0x1000, 0x8000_2000, 0x80);
        words[8] = i(0x23, 4, 9, 0);
        words[3] = i(0x23, 4, 7, 0);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.device_address,
            StaticOperand::Open { ref blockers }
                if matches!(blockers.as_slice(), [SliceBlocker::LoadedFromMemory { register: 7, .. }])
        ));
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if matches!(blockers.as_slice(), [SliceBlocker::LoadedFromMemory { register: 9, .. }])
        ));
        assert_eq!(slice.candidate(), None);
    }

    #[test]
    fn never_propagates_values_across_an_earlier_control_transfer() {
        let mut words = canonical_call(0x1000, 0x8000_2000, 0x80);
        words.insert(4, jal(0x8000_3000));
        words.insert(5, 0);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(slice.device_address.proven().is_none());
        assert!(slice.rdram_address.proven().is_some());
    }

    #[test]
    fn rejects_non_kseg_and_out_of_bounds_dram_ranges() {
        let non_kseg = canonical_call(0x1000, 0x1000, 0x80);
        let slice = &slice_os_pi_start_dma_calls(
            &non_kseg,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.rdram_address,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::DramPointerOutsideKseg { raw: 0x1000 }]
        ));

        let too_long = canonical_call(0x1000, 0x807f_fff0, 0x20);
        let slice = &slice_os_pi_start_dma_calls(
            &too_long,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.rdram_address,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::RdramRangeOutOfBounds {
                    end_exclusive: 0x80_0010,
                    rdram_len: 0x80_0000,
                }]
        ));
    }

    #[test]
    fn rejects_zero_size_invalid_direction_and_device_overflow() {
        let mut words = canonical_call(0xffff_fff0, 0x8000_2000, 0);
        words[1] = i(0x09, 0, 6, 3);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.direction,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::InvalidDirection { raw: 3 }]
        ));
        assert!(matches!(
            slice.byte_count,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::ZeroByteCount]
        ));

        let words = canonical_call(0xffff_fff0, 0x8000_2000, 0x20);
        let slice = &slice_os_pi_start_dma_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x80_0000,
        )
        .unwrap()[0];
        assert!(matches!(
            slice.device_address,
            StaticOperand::Open { ref blockers }
                if blockers == &vec![SliceBlocker::DeviceRangeOverflow]
        ));
    }

    #[test]
    fn rejects_unaligned_overflowing_and_impossible_inputs() {
        assert_eq!(
            slice_os_pi_start_dma_calls(
                &[0],
                VirtualAddress::new(START + 2),
                VirtualAddress::new(CALLEE),
                0x80_0000,
            ),
            Err(PiDmaSliceError::ImageAddressUnaligned {
                start: VirtualAddress::new(START + 2),
            })
        );
        assert_eq!(
            slice_os_pi_start_dma_calls(
                &[0, 0],
                VirtualAddress::new(0xffff_fffc),
                VirtualAddress::new(CALLEE),
                0x80_0000,
            ),
            Err(PiDmaSliceError::ImageAddressOverflow)
        );
        assert!(matches!(
            slice_os_pi_start_dma_calls(
                &[0],
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                KSEG_SIZE + 1,
            ),
            Err(PiDmaSliceError::RdramLengthOutsidePhysicalDomain { .. })
        ));
    }

    #[test]
    fn caller_frame_slots_and_saved_registers_cross_direct_calls_only() {
        const START: u32 = 0x8000_0000;
        const CALLEE: u32 = 0x8000_2000;
        let jal_word = |target: u32| 0x0c00_0000 | ((target >> 2) & 0x03ff_ffff);
        // sw of a constant arg to the frame, an intervening helper call,
        // then the sliced call reloading the spill in its delay slot —
        // the SM64 dma_read shape.
        let body = |middle: u32, escape: Option<u32>| {
            let mut words = vec![
                0x3c08_00aa, // lui   r8, 0x00aa      (spilled value)
                0xafa8_0024, // sw    r8, 0x24(sp)
            ];
            if let Some(word) = escape {
                words.push(word);
            }
            words.extend([
                middle,      // intervening transfer
                0x0000_0000, // its delay slot
                0x3c06_0123, // lui   r6, 0x0123      (device register a2)
                jal_word(CALLEE),
                0x8fa6_0024, // delay: lw r6, 0x24(sp) — reload the spill
                0x0000_0000,
            ]);
            words
        };
        let device = |words: &[u32]| {
            slice_pointer_arg_calls(
                words,
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                0x80_0000,
                6,
            )
            .unwrap()[0]
                .pointer
                .proven()
                .copied()
        };

        // Direct helper call, no escape: the spill survives and the reload
        // yields the stored constant.
        let direct = body(jal_word(0x8000_3000), None);
        assert_eq!(device(&direct).map(|va| va.get()), Some(0x00aa_0000));

        // Same shape with a materialized frame address: persistence off.
        let escaped = body(jal_word(0x8000_3000), Some(0x27a4_0024)); // addiu a0, sp, 0x24
        assert_eq!(device(&escaped), None);

        // A branch instead of a call: full barrier, as ever.
        let branch = body(0x1000_0004, None); // beq r0, r0, +4
        assert_eq!(device(&branch), None);
    }

    #[test]
    fn batched_pointer_contracts_equal_independent_slices_and_deduplicate() {
        const OTHER_CALLEE: u32 = 0x8000_5000;
        let words = vec![
            i(0x0f, 0, 4, 0x8001u16 as i16),
            i(0x0d, 4, 4, 0x1110),
            i(0x0f, 0, 5, 0x8002u16 as i16),
            i(0x0d, 5, 5, 0x2220),
            jal(CALLEE),
            0,
            i(0x0f, 0, 4, 0x8003u16 as i16),
            i(0x0d, 4, 4, 0x3330),
            jal(OTHER_CALLEE),
            0,
        ];
        let start = VirtualAddress::new(START);
        let contracts = [
            (VirtualAddress::new(OTHER_CALLEE), 4),
            (VirtualAddress::new(CALLEE), 5),
            (VirtualAddress::new(CALLEE), 4),
            (VirtualAddress::new(CALLEE), 4),
        ];
        let batch =
            slice_pointer_arg_call_contracts(&words, start, 0x0080_0000, &contracts).unwrap();

        let mut independent = Vec::new();
        for &(callee, register) in &contracts[..3] {
            independent.extend(
                slice_pointer_arg_calls(&words, start, callee, 0x0080_0000, register).unwrap(),
            );
        }
        independent.sort_by_key(|slice| (slice.call_pc, slice.pointer_register));

        assert_eq!(batch, independent);
        assert_eq!(batch.len(), 3);
        assert_eq!(
            batch
                .iter()
                .map(|slice| (
                    slice.call_pc.get(),
                    slice.pointer_register,
                    slice.pointer.proven().map(|pointer| pointer.get()),
                ))
                .collect::<Vec<_>>(),
            vec![
                (START + 0x10, 4, Some(0x8001_1110)),
                (START + 0x10, 5, Some(0x8002_2220)),
                (START + 0x20, 4, Some(0x8003_3330)),
            ]
        );
    }

    #[test]
    fn pointer_contract_register_domain_is_checked_before_slicing() {
        assert_eq!(
            slice_pointer_arg_call_contracts(&[0], VirtualAddress::new(START), 0x0080_0000, &[],),
            Ok(Vec::new())
        );
        let error = slice_pointer_arg_call_contracts(
            &[0],
            VirtualAddress::new(START),
            0x0080_0000,
            &[(VirtualAddress::new(CALLEE), 32)],
        );
        assert_eq!(
            error,
            Err(PiDmaSliceError::InvalidPointerRegister { register: 32 })
        );
        assert_eq!(
            slice_pointer_arg_calls(
                &[0],
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                0x0080_0000,
                32,
            ),
            error
        );
    }

    #[test]
    fn load_request_slice_folds_subu_of_link_constants() {
        // IDO computes a request's byte count as `subu end, start` of two
        // link-time constants (MM boot's Main_Init code load). The slicer
        // must fold it; a size taken from memory must stay open.
        const START: u32 = 0x8008_0000;
        const CALLEE: u32 = 0x8008_0c04;
        let jal = 0x0c00_0000 | ((CALLEE >> 2) & 0x03ff_ffff);
        let words = vec![
            0x3c03_00b4, // lui   r3, 0x00b4
            0x3c0f_00c8, // lui   r15, 0x00c8
            0x2466_c000, // addiu r6, r3, -0x4000   -> device 0x00b3c000
            0x25ef_a4e0, // addiu r15, r15, -0x5b20 -> 0x00c7a4e0
            0x3c05_800a, // lui   r5, 0x800a
            0x24a5_5ac0, // addiu r5, r5, 0x5ac0    -> dram 0x800a5ac0
            0x01e6_3823, // subu  r7, r15, r6       -> size 0x0013e4e0
            jal,         // jal   callee
            0x0000_0000, // nop
            0x0000_0000,
        ];
        let slices = slice_load_request_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x0080_0000,
            5,
            6,
            7,
        )
        .unwrap();
        assert_eq!(slices.len(), 1);
        let candidate = slices[0].candidate().expect("all operands constant");
        assert_eq!(candidate.device_address.get(), 0x00b3_c000);
        // MM's real map places code at exactly this size (0x13E4E0): the
        // addiu immediate 0xA4E0 sign-extends negative, which is the very
        // subtlety a naive unsigned reading of the fixture gets wrong.
        assert_eq!(candidate.byte_count.get(), 0x0013_e4e0);
        assert_eq!(slices[0].dram_pointer.proven().unwrap().get(), 0x800a_5ac0);
        assert_eq!(candidate.direction, PiDmaDirection::ToRdram);

        // Same call, size loaded from memory: candidate must not form.
        let mut open_words = words.clone();
        open_words[6] = 0x8ce7_0000; // lw r7, 0x0(r7)
        let slices = slice_load_request_calls(
            &open_words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x0080_0000,
            5,
            6,
            7,
        )
        .unwrap();
        assert!(slices[0].candidate().is_none());
    }

    fn branch_guarded_load_request(branch_op: u32, branch_target: usize) -> Vec<u32> {
        let branch_index = 2usize;
        let displacement = i16::try_from(branch_target - (branch_index + 1)).unwrap();
        vec![
            0x3c05_00b9,                      // lui   a1, 0x00b9
            0x24a5_ad30,                      // addiu a1, a1, -0x52d0
            i(branch_op, 8, 0, displacement), // branch to a point after/at call
            0x3c04_801c,                      // delay: lui a0, 0x801c
            0x3c0f_00ba,                      // lui   t7, 0x00ba
            0x25ef_da40,                      // addiu t7, t7, -0x25c0
            0x01e5_3023,                      // subu  a2, t7, a1
            jal(CALLEE),                      // call_index
            0x2484_6e80,                      // delay: addiu a0, a0, 0x6e80
            0,
            0,
            0,
        ]
    }

    #[test]
    fn load_request_slice_crosses_ordinary_branch_that_skips_the_call() {
        let words = branch_guarded_load_request(0x05, 10);
        let slices = slice_load_request_calls(
            &words,
            VirtualAddress::new(START),
            VirtualAddress::new(CALLEE),
            0x0080_0000,
            4,
            5,
            6,
        )
        .unwrap();
        let candidate = slices[0]
            .candidate()
            .expect("not-taken call path has exact operands");
        assert_eq!(slices[0].dram_pointer.proven().unwrap().get(), 0x801c_6e80);
        assert_eq!(candidate.device_address.get(), 0x00b8_ad30);
        assert_eq!(candidate.byte_count.get(), 0x0001_2d10);
    }

    #[test]
    fn load_request_slice_keeps_annulled_and_merging_branches_hard() {
        let mut always_taken = branch_guarded_load_request(0x04, 10);
        always_taken[2] = i(0x04, 8, 8, 7); // beq t0, t0, after_call
        for words in [
            branch_guarded_load_request(0x15, 10),
            branch_guarded_load_request(0x05, 7),
            branch_guarded_load_request(0x05, 8),
            always_taken,
        ] {
            let slices = slice_load_request_calls(
                &words,
                VirtualAddress::new(START),
                VirtualAddress::new(CALLEE),
                0x0080_0000,
                4,
                5,
                6,
            )
            .unwrap();
            assert!(slices[0].candidate().is_none());
        }
    }
