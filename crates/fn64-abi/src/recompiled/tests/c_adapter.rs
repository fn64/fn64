use super::*;

    #[test]
    fn c_adapter_round_trips_all_gprs_and_forces_zero() {
        let mut recompiled = RsContext::new();
        for i in 1..32 {
            recompiled.set_r(i, 0xA000_0000_0000_0000 | i as u64);
        }
        let mut c = c_from_recompiled(&recompiled);
        c.r0 = u64::MAX;
        c.r2 = 0x1234;
        copy_c_back(&c, &mut recompiled);
        assert_eq!(recompiled.r(0), 0);
        assert_eq!(recompiled.r(2), 0x1234);
        assert_eq!(recompiled.r(31), 0xA000_0000_0000_001F);
    }

    pub(super) unsafe extern "C" fn no_op_fpr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.assert_float_mode_matches_status();
        let expected = if ctx.mips3_float_mode == 0 {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f0.u32_halves.1 as *mut u32 }
        } else {
            // Safety: taking a union field address does not read that field.
            unsafe { &mut ctx.f1.u32_halves.0 as *mut u32 }
        };
        assert_eq!(ctx.f_odd, expected);
    }

    pub(super) unsafe extern "C" fn write_f5_word_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` arms `f_odd` for this live context. N64Recomp's
        // generated odd-register expression for f5 is `(5 - 1) * 2`.
        unsafe { *(*ctx).f_odd.add(8) = 0xDEAD_BEEF };
    }

    pub(super) unsafe extern "C" fn change_fr_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        let ctx = unsafe { &mut *ctx };
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
    }

    pub(super) unsafe extern "C" fn change_bev_shim(_rdram: *mut u8, ctx: *mut CContext) {
        // Safety: `call_c` supplies its live stack-local C context.
        unsafe { &mut *ctx }.status_reg ^= STATUS_BEV;
    }

    unsafe extern "C" fn transient_fr_write_shim(_rdram: *mut u8, ctx: *mut CContext) {
        TRANSIENT_FR_SHIM_ENTERED.store(true, Ordering::SeqCst);
        // Safety: the regression deliberately models a raw ABI shim which
        // changes to the other FPR view, accesses it, then restores the entry
        // mode before returning.
        let ctx = unsafe { &mut *ctx };
        let entry_status = ctx.status_reg;
        let entry_mode = ctx.mips3_float_mode;
        ctx.status_reg ^= STATUS_FR;
        ctx.mips3_float_mode ^= 1;
        ctx.arm_fpr_alias();
        // Safety: `arm_fpr_alias` made this pointer live for the transient
        // view. The generated odd-register expression for f5 is `(5-1)*2`.
        unsafe { *ctx.f_odd.add(8) = 0xA11C_E55E };
        ctx.status_reg = entry_status;
        ctx.mips3_float_mode = entry_mode;
        ctx.arm_fpr_alias();
    }


    #[test]
    fn c_adapter_layout_is_reversible_and_mode_exact() {
        let physical = patterned_fgr_state(0xA5A5_5A5A_DEAD_BEEF);
        let words = physical.into_words();
        for fr in [false, true] {
            let mut source = RsContext::new();
            source.cop0_status = if fr { STATUS_FR } else { 0 };
            source.replace_physical_fgr_state(physical);
            let c = c_from_recompiled(&source);
            c.assert_float_mode_matches_status();
            let image = c.fpr_u64_bits();
            if fr {
                assert_eq!(image, words);
            } else {
                for pair in 0..16 {
                    let even = pair * 2;
                    let odd = even + 1;
                    assert_eq!(
                        image[even],
                        u64::from(words[even] as u32) | (u64::from(words[odd] as u32) << 32)
                    );
                    assert_eq!(
                        image[odd],
                        (words[even] >> 32) | (words[odd] & 0xFFFF_FFFF_0000_0000)
                    );
                }
            }

            let mut restored = RsContext::new();
            copy_c_back(&c, &mut restored);
            assert_eq!(restored.physical_fgr_state(), physical);
            assert_eq!(restored.cop0_status & STATUS_FR != 0, fr);
        }
    }


    #[test]
    fn c_adapter_noop_preserves_every_physical_fgr_in_both_fr_modes() {
        for (fr, bev) in [(false, false), (false, true), (true, false), (true, true)] {
            let expected = patterned_fgr_state(if fr {
                0xA5A5_5A5A_DEAD_BEEF
            } else {
                0x1122_3344_5566_7788
            });
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 } | if bev { STATUS_BEV } else { 0 };
            ctx.replace_physical_fgr_state(expected);
            let mut bytes = [];
            let mut mem = Rdram::new(&mut bytes);

            call_c(&mut ctx, &mut mem, "no_op_fpr_shim", no_op_fpr_shim);

            assert_eq!(ctx.physical_fgr_state(), expected, "FR={fr}");
            assert_eq!(ctx.cop0_status & STATUS_FR != 0, fr);
            assert_eq!(ctx.cop0_status & STATUS_BEV != 0, bev);
        }
    }


    #[test]
    fn c_adapter_rejects_bev_changes_before_status_copyback() {
        for entry_bev in [false, true] {
            let mut ctx = RsContext::new();
            ctx.cop0_status = if entry_bev { STATUS_BEV } else { 0 };
            let mut bytes = [];
            let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                call_c(
                    &mut ctx,
                    &mut Rdram::new(&mut bytes),
                    "change_bev_shim",
                    change_bev_shim,
                );
            }));
            assert!(rejected.is_err());
            assert_eq!(ctx.cop0_status & STATUS_BEV != 0, entry_bev);
        }
    }


    #[test]
    fn c_adapter_f_odd_write_targets_physical_fgr5_in_both_modes() {
        for fr in [false, true] {
            let initial = patterned_fgr_state(0x1234_5678_9ABC_DEF0).into_words();
            let mut ctx = RsContext::new();
            ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            let mut bytes = [];
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "write_f5_word_shim",
                write_f5_word_shim,
            );
            let mut expected = initial;
            expected[5] = (expected[5] & 0xFFFF_FFFF_0000_0000) | 0xDEAD_BEEF;
            assert_eq!(ctx.physical_fgr_state().into_words(), expected, "FR={fr}");
        }
    }


    #[test]
    fn c_adapter_rejects_an_fr_transition_before_decoding_entry_view_bytes() {
        let expected = patterned_fgr_state(0x0BAD_F00D_CAFE_BABE);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "change_fr_shim",
                change_fr_shim,
            );
        }));
        assert!(rejected.is_err());
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }


    #[test]
    fn c_adapter_rejects_a_transient_fr_transition_before_the_shim_runs() {
        TRANSIENT_FR_SHIM_ENTERED.store(false, Ordering::SeqCst);
        let expected = patterned_fgr_state(0x1357_9BDF_2468_ACE0);
        let mut ctx = RsContext::new();
        ctx.replace_physical_fgr_state(expected);
        let mut bytes = [];
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            call_c(
                &mut ctx,
                &mut Rdram::new(&mut bytes),
                "transient_fr_write_shim",
                transient_fr_write_shim,
            );
        }));
        let panic = rejected.expect_err("unadmitted transient-FR shim must be rejected");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .expect("registry rejection must use a string panic payload");
        assert!(
            message.contains("is not in the FR-stable adapter registry"),
            "unexpected rejection: {message}"
        );
        assert!(!TRANSIENT_FR_SHIM_ENTERED.load(Ordering::SeqCst));
        assert_eq!(ctx.cop0_status & STATUS_FR, 0);
        assert_eq!(ctx.physical_fgr_state(), expected);
    }


    #[test]
    fn c_adapter_float_helpers_return_through_f0_in_both_fr_modes() {
        let value = 0xFEDC_BA98_7654_3210u64;
        for fr in [false, true] {
            let initial = patterned_fgr_state(0xC001_D00D_A55A_5AA5).into_words();

            let mut float_ctx = RsContext::new();
            float_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            float_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            float_ctx.set_r(4, value >> 32);
            float_ctx.set_r(5, value as u32 as u64);
            let mut float_bytes = [];
            ull_to_f(&mut float_ctx, &mut Rdram::new(&mut float_bytes));
            assert_eq!(float_ctx.f_bits(0), (value as f32).to_bits(), "FR={fr}");
            let mut expected_float = initial;
            expected_float[0] =
                (expected_float[0] & 0xFFFF_FFFF_0000_0000) | u64::from((value as f32).to_bits());
            assert_eq!(
                float_ctx.physical_fgr_state().into_words(),
                expected_float,
                "FR={fr} float result changed non-result state"
            );

            let mut double_ctx = RsContext::new();
            double_ctx.cop0_status = if fr { STATUS_FR } else { 0 };
            double_ctx.replace_physical_fgr_state(PhysicalFgrState::from_words(initial));
            double_ctx.set_r(4, value >> 32);
            double_ctx.set_r(5, value as u32 as u64);
            let mut double_bytes = [];
            ull_to_d(&mut double_ctx, &mut Rdram::new(&mut double_bytes));
            let result = (value as f64).to_bits();
            assert_eq!(double_ctx.d_bits(0), result, "FR={fr}");
            let mut expected_double = initial;
            if fr {
                expected_double[0] = result;
            } else {
                expected_double[0] =
                    (expected_double[0] & 0xFFFF_FFFF_0000_0000) | u64::from(result as u32);
                expected_double[1] = (expected_double[1] & 0xFFFF_FFFF_0000_0000) | (result >> 32);
            }
            assert_eq!(
                double_ctx.physical_fgr_state().into_words(),
                expected_double,
                "FR={fr} double result changed non-result state"
            );
        }
    }
