//! Completeness regressions byte-cited to the public MIPS IV ISA Rev. 3.2
//! encoding tables A-39/B-25 and the NEC VR4300 User's Manual sections
//! 6.3.2 (FCR0/FCR31), 10.8 (LL/SC), and appendix D.2 (division by zero).

use fn64_cpu_runtime::{decode, Instruction, Rdram, RecompContext, RDRAM_VBASE};
use fn64_cpu_runtime_codegen::{emit_function, FuncInput};

#[test]
fn decode_missing_primary_slots() {
    assert_eq!(
        decode(0x9C88_1234),
        Instruction::Lwu {
            rt: 8,
            base: 4,
            off: 0x1234
        }
    );
    assert_eq!(
        decode(0xC088_0010),
        Instruction::Ll {
            rt: 8,
            base: 4,
            off: 0x10
        }
    );
    assert_eq!(
        decode(0xE088_0010),
        Instruction::Sc {
            rt: 8,
            base: 4,
            off: 0x10
        }
    );
    assert_eq!(
        decode(0xC888_0010),
        Instruction::Lwc2 {
            rt: 8,
            base: 4,
            off: 0x10
        }
    );
    assert_eq!(
        decode(0xD888_0010),
        Instruction::Ldc2 {
            rt: 8,
            base: 4,
            off: 0x10
        }
    );
    assert_eq!(
        decode(0xE888_0010),
        Instruction::Swc2 {
            rt: 8,
            base: 4,
            off: 0x10
        }
    );
    assert_eq!(
        decode(0xF888_0010),
        Instruction::Sdc2 {
            rt: 8,
            base: 4,
            off: 0x10
        }
    );
}

#[test]
fn decode_special_and_regimm_traps() {
    assert_eq!(
        decode(0x0085_0030),
        Instruction::Tge {
            rs: 4,
            rt: 5,
            code: 0
        }
    );
    assert_eq!(
        decode(0x0085_0031),
        Instruction::Tgeu {
            rs: 4,
            rt: 5,
            code: 0
        }
    );
    assert_eq!(
        decode(0x0085_0032),
        Instruction::Tlt {
            rs: 4,
            rt: 5,
            code: 0
        }
    );
    assert_eq!(
        decode(0x0085_0033),
        Instruction::Tltu {
            rs: 4,
            rt: 5,
            code: 0
        }
    );
    assert_eq!(
        decode(0x0085_0034),
        Instruction::Teq {
            rs: 4,
            rt: 5,
            code: 0
        }
    );
    assert_eq!(
        decode(0x0085_0036),
        Instruction::Tne {
            rs: 4,
            rt: 5,
            code: 0
        }
    );

    assert_eq!(decode(0x0488_0001), Instruction::Tgei { rs: 4, imm: 1 });
    assert_eq!(decode(0x0489_FFFF), Instruction::Tgeiu { rs: 4, imm: -1 });
    assert_eq!(
        decode(0x048A_8000),
        Instruction::Tlti {
            rs: 4,
            imm: i16::MIN
        }
    );
    assert_eq!(
        decode(0x048B_7FFF),
        Instruction::Tltiu {
            rs: 4,
            imm: i16::MAX
        }
    );
    assert_eq!(decode(0x048C_FFFE), Instruction::Teqi { rs: 4, imm: -2 });
    assert_eq!(decode(0x048E_0002), Instruction::Tnei { rs: 4, imm: 2 });
}

#[test]
fn decode_regimm_link_likely() {
    let bltzall = decode(0x0492_0002);
    let bgezall = decode(0x0493_0002);
    assert_eq!(bltzall, Instruction::Bltzall { rs: 4, off: 2 });
    assert_eq!(bgezall, Instruction::Bgezall { rs: 4, off: 2 });
    assert!(bltzall.has_delay_slot() && bltzall.is_branch_likely());
    assert!(bgezall.has_delay_slot() && bgezall.is_branch_likely());
}

#[test]
fn decode_cop0_condition_branches() {
    assert_eq!(decode(0x4100_0002), Instruction::Bc0f { off: 2 });
    assert_eq!(decode(0x4101_0002), Instruction::Bc0t { off: 2 });
    assert_eq!(decode(0x4102_0002), Instruction::Bc0fl { off: 2 });
    assert_eq!(decode(0x4103_0002), Instruction::Bc0tl { off: 2 });
    assert!(decode(0x4102_0002).is_branch_likely());
}

#[test]
fn decode_all_cop1_rounding_and_compare_functions() {
    use Instruction::*;
    let rounding = [
        (0x4600_1108, RoundLS { fd: 4, fs: 2 }),
        (0x4600_1109, TruncLS { fd: 4, fs: 2 }),
        (0x4600_110A, CeilLS { fd: 4, fs: 2 }),
        (0x4600_110B, FloorLS { fd: 4, fs: 2 }),
        (0x4600_110C, RoundWS { fd: 4, fs: 2 }),
        (0x4600_110D, TruncWS { fd: 4, fs: 2 }),
        (0x4600_110E, CeilWS { fd: 4, fs: 2 }),
        (0x4600_110F, FloorWS { fd: 4, fs: 2 }),
        (0x4620_1108, RoundLD { fd: 4, fs: 2 }),
        (0x4620_1109, TruncLD { fd: 4, fs: 2 }),
        (0x4620_110A, CeilLD { fd: 4, fs: 2 }),
        (0x4620_110B, FloorLD { fd: 4, fs: 2 }),
        (0x4620_110C, RoundWD { fd: 4, fs: 2 }),
        (0x4620_110D, TruncWD { fd: 4, fs: 2 }),
        (0x4620_110E, CeilWD { fd: 4, fs: 2 }),
        (0x4620_110F, FloorWD { fd: 4, fs: 2 }),
    ];
    for (word, expected) in rounding {
        assert_eq!(decode(word), expected, "word {word:#010X}");
    }

    for cond in 0u8..16 {
        let word = 0x4602_0030 | u32::from(cond);
        let expected = match cond {
            2 => CEqS { fs: 0, ft: 2 },
            12 => CLtS { fs: 0, ft: 2 },
            14 => CLeS { fs: 0, ft: 2 },
            _ => CCondS { fs: 0, ft: 2, cond },
        };
        assert_eq!(
            decode(word),
            expected,
            "C.cond.S funct {:#04X}",
            0x30 + cond
        );
    }
    for cond in 0u8..16 {
        let word = 0x4622_0030 | u32::from(cond);
        let expected = match cond {
            2 => CEqD { fs: 0, ft: 2 },
            12 => CLtD { fs: 0, ft: 2 },
            14 => CLeD { fs: 0, ft: 2 },
            _ => CCondD { fs: 0, ft: 2, cond },
        };
        assert_eq!(
            decode(word),
            expected,
            "C.cond.D funct {:#04X}",
            0x30 + cond
        );
    }
}

#[test]
fn unaligned_word_pairs_cover_every_byte_offset() {
    let mut raw = vec![0u8; 128];
    let base = RDRAM_VBASE.wrapping_add(16);
    let mut mem = Rdram::new(&mut raw);
    for i in 0..32u64 {
        mem.store_b(base.wrapping_add(i), (0x40 + i) as u8);
    }

    for start in 0..=12u64 {
        let addr = base.wrapping_add(start);
        let initial = 0xA5A5_5A5Au64;
        let left = mem.load_wl(initial, addr);
        let got = mem.load_wr(left as i64 as u64, addr.wrapping_add(3)) as u32;
        let expected = u32::from_be_bytes([
            (0x40 + start) as u8,
            (0x41 + start) as u8,
            (0x42 + start) as u8,
            (0x43 + start) as u8,
        ]);
        assert_eq!(got, expected, "LWL/LWR start offset {start}");
    }

    for start in 0..=12u64 {
        let addr = base.wrapping_add(start);
        let value = 0x1020_3040u32.wrapping_add(start as u32);
        mem.store_wl(addr, value);
        mem.store_wr(addr.wrapping_add(3), value);
        for (i, expected_byte) in value.to_be_bytes().into_iter().enumerate() {
            assert_eq!(
                mem.load_bu(addr.wrapping_add(i as u64)),
                expected_byte,
                "SWL/SWR start {start} byte {i}"
            );
        }
    }
}

#[test]
fn unaligned_doubleword_pairs_cover_every_byte_offset() {
    let mut raw = vec![0u8; 192];
    let base = RDRAM_VBASE.wrapping_add(32);
    let mut mem = Rdram::new(&mut raw);
    for i in 0..48u64 {
        mem.store_b(base.wrapping_add(i), (0x80 + i) as u8);
    }

    for start in 0..=16u64 {
        let addr = base.wrapping_add(start);
        let left = mem.load_dl(0xA5A5_5A5A_DEAD_BEEF, addr);
        let got = mem.load_dr(left, addr.wrapping_add(7));
        let expected = u64::from_be_bytes([
            (0x80 + start) as u8,
            (0x81 + start) as u8,
            (0x82 + start) as u8,
            (0x83 + start) as u8,
            (0x84 + start) as u8,
            (0x85 + start) as u8,
            (0x86 + start) as u8,
            (0x87 + start) as u8,
        ]);
        assert_eq!(got, expected, "LDL/LDR start offset {start}");
    }

    for start in 0..=16u64 {
        let addr = base.wrapping_add(start);
        let value = 0x1020_3040_5060_7080u64.wrapping_add(start);
        mem.store_dl(addr, value);
        mem.store_dr(addr.wrapping_add(7), value);
        for (i, expected_byte) in value.to_be_bytes().into_iter().enumerate() {
            assert_eq!(
                mem.load_bu(addr.wrapping_add(i as u64)),
                expected_byte,
                "SDL/SDR start {start} byte {i}"
            );
        }
    }
}

#[test]
fn unaligned_instruction_shapes_used_by_oot_oracle_decode() {
    // OoT's func_80B3C964 uses this LWL/LWR + SWL/SWR register/offset shape.
    // Construct the words from the public I-format fields so no game ROM bytes
    // or recompiled-game output is committed to this repository.
    let i = |op: u32, base: u32, rt: u32, imm: u16| {
        (op << 26) | (base << 21) | (rt << 16) | u32::from(imm)
    };
    assert_eq!(
        decode(i(0x22, 14, 24, 0)),
        Instruction::Lwl {
            rt: 24,
            base: 14,
            off: 0
        }
    );
    assert_eq!(
        decode(i(0x26, 14, 24, 3)),
        Instruction::Lwr {
            rt: 24,
            base: 14,
            off: 3
        }
    );
    assert_eq!(
        decode(i(0x2A, 5, 24, 0x17A)),
        Instruction::Swl {
            rt: 24,
            base: 5,
            off: 0x17A
        }
    );
    assert_eq!(
        decode(i(0x2E, 5, 24, 0x17D)),
        Instruction::Swr {
            rt: 24,
            base: 5,
            off: 0x17D
        }
    );
}

#[test]
fn ll_sc_reservation_succeeds_once_and_mismatch_fails() {
    let mut ctx = RecompContext::new();
    let a = RDRAM_VBASE.wrapping_add(0x100);
    ctx.set_ll_reservation(a, 4);
    assert!(ctx.take_ll_reservation(a, 4));
    assert!(!ctx.take_ll_reservation(a, 4), "SC must clear LLbit");
    ctx.set_ll_reservation(a, 8);
    assert!(!ctx.take_ll_reservation(a.wrapping_add(8), 8));
}

#[test]
fn vr4300_division_boundaries_and_zero_results() {
    let mut ctx = RecompContext::new();
    ctx.div_s32(7, 0);
    assert_eq!((ctx.lo, ctx.hi), (0x7FFF_FFFF, 7));
    ctx.div_s32(-7, 0);
    assert_eq!((ctx.lo, ctx.hi), (0xFFFF_FFFF_8000_0001, (-7i64) as u64));
    ctx.div_u32(0x8000_0001, 0);
    assert_eq!((ctx.lo, ctx.hi), (u64::MAX, 0xFFFF_FFFF_8000_0001));
    ctx.div_s32(i32::MIN, -1);
    assert_eq!((ctx.lo, ctx.hi), (0xFFFF_FFFF_8000_0000, 0));
    ctx.div_s64(i64::MIN, -1);
    assert_eq!((ctx.lo, ctx.hi), (0x8000_0000_0000_0000, 0));
}

#[test]
#[should_panic(expected = "DDIV by zero: result is not specified")]
fn ddiv_zero_is_a_loud_manual_uncertainty() {
    RecompContext::new().div_s64(1, 0);
}

#[test]
fn fcsr_rounding_modes_flags_and_all_compare_predicates() {
    const CAUSE_V: u32 = 1 << 16;
    const FLAG_V: u32 = 1 << 6;

    let mut ctx = RecompContext::new();
    assert_eq!(ctx.read_fcr(0) >> 8 & 0xFF, 0x0B);
    ctx.write_fcr(31, 0x0180_007F);
    assert!(!ctx.fcsr_exception_pending());
    for exception in 0..5 {
        let cause = 1 << (12 + exception);
        let enable = 1 << (7 + exception);
        ctx.write_fcr(31, cause);
        assert!(!ctx.fcsr_exception_pending(), "Cause[{exception}] only");
        ctx.write_fcr(31, enable);
        assert!(!ctx.fcsr_exception_pending(), "Enable[{exception}] only");
        ctx.write_fcr(31, cause | enable);
        assert!(
            ctx.fcsr_exception_pending(),
            "Cause[{exception}] + Enable[{exception}]"
        );
    }
    ctx.write_fcr(31, 1 << 17);
    assert!(ctx.fcsr_exception_pending(), "Cause.E is always enabled");
    for (mode, expected) in [(0, 2), (1, 1), (2, 2), (3, 1)] {
        ctx.write_fcr(31, mode);
        ctx.set_f_s(0, 1.5);
        assert_eq!(
            ctx.try_fpu_to_i32_s(0, None).unwrap(),
            expected,
            "RM={mode}"
        );
        assert_ne!(ctx.read_fcr(31) & (1 << 2), 0, "inexact flag for RM={mode}");
    }

    ctx.set_f_s(0, 1.0);
    ctx.set_f_s(2, 2.0);
    for cond in 0u8..16 {
        ctx.try_fpu_compare_s(0, 2, cond).unwrap();
        assert_eq!(ctx.fpu_cond, cond & 4 != 0, "less predicate cond={cond:#x}");
    }
    ctx.set_f_s(0, 2.0);
    for cond in 0u8..16 {
        ctx.try_fpu_compare_s(0, 2, cond).unwrap();
        assert_eq!(
            ctx.fpu_cond,
            cond & 2 != 0,
            "equal predicate cond={cond:#x}"
        );
    }
    ctx.set_f_s(0, 3.0);
    for cond in 0u8..16 {
        ctx.try_fpu_compare_s(0, 2, cond).unwrap();
        assert!(!ctx.fpu_cond, "greater predicate cond={cond:#x}");
    }
    // VR4300 uses the legacy NaN convention: fraction MSB 0 is quiet and 1
    // is signaling (User's Manual, p.151).
    ctx.set_f_bits(0, 0x7F80_0001);
    for cond in 0u8..16 {
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_s(0, 2, cond).unwrap();
        assert_eq!(
            ctx.fpu_cond,
            cond & 1 != 0,
            "unordered predicate cond={cond:#x}"
        );
        assert_eq!(
            ctx.read_fcr(31) & (CAUSE_V | FLAG_V),
            if cond & 8 != 0 { CAUSE_V | FLAG_V } else { 0 },
            "single QNaN Invalid cond={cond:#x}"
        );
    }
    ctx.set_f_bits(0, 0x7FC0_0001);
    for cond in 0u8..16 {
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_s(0, 2, cond).unwrap();
        assert_eq!(ctx.fpu_cond, cond & 1 != 0, "single SNaN cond={cond:#x}");
        assert_eq!(
            ctx.read_fcr(31) & (CAUSE_V | FLAG_V),
            CAUSE_V | FLAG_V,
            "single SNaN Invalid cond={cond:#x}"
        );
    }
    ctx.set_f_s(0, 1.0);
    for (bits, snan) in [(0x7F80_0001, false), (0x7FC0_0001, true)] {
        ctx.set_f_bits(2, bits);
        for cond in 0u8..16 {
            ctx.write_fcr(31, 0);
            ctx.try_fpu_compare_s(0, 2, cond).unwrap();
            assert_eq!(
                ctx.fpu_cond,
                cond & 1 != 0,
                "single NaN in ft cond={cond:#x} snan={snan}"
            );
            assert_eq!(
                ctx.read_fcr(31) & (CAUSE_V | FLAG_V),
                if snan || cond & 8 != 0 {
                    CAUSE_V | FLAG_V
                } else {
                    0
                },
                "single NaN in ft Invalid cond={cond:#x} snan={snan}"
            );
        }
    }

    ctx.set_f_d(0, 1.0);
    ctx.set_f_d(2, 2.0);
    for cond in 0u8..16 {
        ctx.try_fpu_compare_d(0, 2, cond).unwrap();
        assert_eq!(ctx.fpu_cond, cond & 4 != 0, "double less cond={cond:#x}");
    }
    ctx.set_f_d(0, 2.0);
    for cond in 0u8..16 {
        ctx.try_fpu_compare_d(0, 2, cond).unwrap();
        assert_eq!(ctx.fpu_cond, cond & 2 != 0, "double equal cond={cond:#x}");
    }
    ctx.set_f_d(0, 3.0);
    for cond in 0u8..16 {
        ctx.try_fpu_compare_d(0, 2, cond).unwrap();
        assert!(!ctx.fpu_cond, "double greater cond={cond:#x}");
    }
    ctx.set_d_bits(0, 0x7FF0_0000_0000_0001);
    for cond in 0u8..16 {
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_d(0, 2, cond).unwrap();
        assert_eq!(
            ctx.fpu_cond,
            cond & 1 != 0,
            "double unordered cond={cond:#x}"
        );
        assert_eq!(
            ctx.read_fcr(31) & (CAUSE_V | FLAG_V),
            if cond & 8 != 0 { CAUSE_V | FLAG_V } else { 0 },
            "double QNaN Invalid cond={cond:#x}"
        );
    }
    ctx.set_d_bits(0, 0x7FF8_0000_0000_0001);
    for cond in 0u8..16 {
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_d(0, 2, cond).unwrap();
        assert_eq!(ctx.fpu_cond, cond & 1 != 0, "double SNaN cond={cond:#x}");
        assert_eq!(
            ctx.read_fcr(31) & (CAUSE_V | FLAG_V),
            CAUSE_V | FLAG_V,
            "double SNaN Invalid cond={cond:#x}"
        );
    }
    ctx.set_f_d(0, 1.0);
    for (bits, snan) in [
        (0x7FF0_0000_0000_0001, false),
        (0x7FF8_0000_0000_0001, true),
    ] {
        ctx.set_d_bits(2, bits);
        for cond in 0u8..16 {
            ctx.write_fcr(31, 0);
            ctx.try_fpu_compare_d(0, 2, cond).unwrap();
            assert_eq!(
                ctx.fpu_cond,
                cond & 1 != 0,
                "double NaN in ft cond={cond:#x} snan={snan}"
            );
            assert_eq!(
                ctx.read_fcr(31) & (CAUSE_V | FLAG_V),
                if snan || cond & 8 != 0 {
                    CAUSE_V | FLAG_V
                } else {
                    0
                },
                "double NaN in ft Invalid cond={cond:#x} snan={snan}"
            );
        }
    }
}

#[test]
fn compare_permits_signed_min_and_max_subnormals_in_either_operand() {
    const ALL_CAUSES_AND_FLAGS: u32 = (0x3F << 12) | (0x1F << 2);

    let mut ctx = RecompContext::new();
    for bits in [0x0000_0001, 0x007F_FFFF, 0x8000_0001, 0x807F_FFFF] {
        let value = f32::from_bits(bits);
        ctx.set_f_bits(0, bits);
        ctx.set_f_s(2, 0.0);
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_s(0, 2, 4).unwrap(); // c.olt.s
        assert_eq!(ctx.fpu_cond, value < 0.0, "single lhs bits={bits:#010x}");
        assert_eq!(ctx.read_fcr(31) & ALL_CAUSES_AND_FLAGS, 0);

        ctx.set_f_s(0, 0.0);
        ctx.set_f_bits(2, bits);
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_s(0, 2, 4).unwrap(); // c.olt.s
        assert_eq!(ctx.fpu_cond, 0.0 < value, "single rhs bits={bits:#010x}");
        assert_eq!(ctx.read_fcr(31) & ALL_CAUSES_AND_FLAGS, 0);
    }

    for bits in [
        0x0000_0000_0000_0001,
        0x000F_FFFF_FFFF_FFFF,
        0x8000_0000_0000_0001,
        0x800F_FFFF_FFFF_FFFF,
    ] {
        let value = f64::from_bits(bits);
        ctx.set_d_bits(0, bits);
        ctx.set_f_d(2, 0.0);
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_d(0, 2, 4).unwrap(); // c.olt.d
        assert_eq!(ctx.fpu_cond, value < 0.0, "double lhs bits={bits:#018x}");
        assert_eq!(ctx.read_fcr(31) & ALL_CAUSES_AND_FLAGS, 0);

        ctx.set_f_d(0, 0.0);
        ctx.set_d_bits(2, bits);
        ctx.write_fcr(31, 0);
        ctx.try_fpu_compare_d(0, 2, 4).unwrap(); // c.olt.d
        assert_eq!(ctx.fpu_cond, 0.0 < value, "double rhs bits={bits:#018x}");
        assert_eq!(ctx.read_fcr(31) & ALL_CAUSES_AND_FLAGS, 0);
    }
}

#[test]
fn precise_compare_exception_orders_cause_flag_enable_and_condition() {
    const FS: u32 = 1 << 24;
    const CAUSE_V: u32 = 1 << 16;
    const ENABLE_V: u32 = 1 << 11;
    const FLAG_V: u32 = 1 << 6;
    const FLAG_I: u32 = 1 << 2;

    let mut ctx = RecompContext::new();
    ctx.set_f_bits(0, 0x7F80_0001); // VR4300 legacy quiet NaN
    ctx.set_f_s(2, 1.0);
    ctx.write_fcr(31, FS | FLAG_I | 3);
    ctx.try_fpu_compare_s(0, 2, 8).unwrap(); // signaling false
    assert_eq!(ctx.read_fcr(31), FS | CAUSE_V | FLAG_V | FLAG_I | 3);
    assert!(!ctx.fpu_cond);

    // A quiet predicate over QNaN is unordered without Invalid.
    ctx.write_fcr(31, FS | FLAG_I | 3);
    ctx.try_fpu_compare_s(0, 2, 1).unwrap(); // unordered
    assert_eq!(ctx.read_fcr(31), FS | (1 << 23) | FLAG_I | 3);
    assert!(ctx.fpu_cond);

    // SNaN signals even for a quiet predicate. Enabled Invalid updates only
    // Cause and preserves both the prior Flag field and condition destination.
    ctx.set_f_bits(0, 0x7FC0_0001);
    ctx.write_fcr(31, FS | (1 << 23) | ENABLE_V | FLAG_I | 2);
    assert!(ctx.try_fpu_compare_s(0, 2, 2).is_err());
    assert_eq!(
        ctx.read_fcr(31),
        FS | (1 << 23) | CAUSE_V | ENABLE_V | FLAG_I | 2
    );
    assert_eq!(ctx.read_fcr(31) & FLAG_V, 0);
    assert!(ctx.fpu_cond);

    // Double SNaN follows the same precise path, while a following finite
    // compare rewrites Cause and leaves sticky Flags/controls untouched.
    ctx.set_d_bits(0, 0x7FF8_0000_0000_0001);
    ctx.set_f_d(2, 1.0);
    assert!(ctx.try_fpu_compare_d(0, 2, 1).is_err());
    ctx.set_f_d(0, 1.0);
    ctx.try_fpu_compare_d(0, 2, 2).unwrap();
    assert_eq!(ctx.read_fcr(31) & (0x3F << 12), 0);
    assert_eq!(
        ctx.read_fcr(31) & (FS | ENABLE_V | FLAG_I | 3),
        FS | ENABLE_V | FLAG_I | 2
    );
}

#[test]
fn float_to_fixed_matrix_is_typed_before_destination_commit() {
    const CAUSE_I: u32 = 1 << 12;
    const CAUSE_V: u32 = 1 << 16;
    const CAUSE_E: u32 = 1 << 17;
    const ENABLE_I: u32 = 1 << 7;
    const ENABLE_V: u32 = 1 << 11;
    const FLAG_I: u32 = 1 << 2;
    const FLAG_V: u32 = 1 << 6;

    let mut ctx = RecompContext::new();
    ctx.set_f_s(0, 1.5);
    for (mode, expected) in [(0, 2), (1, 1), (2, 2), (3, 1)] {
        ctx.write_fcr(31, mode);
        assert_eq!(ctx.try_fpu_to_i32_s(0, None).unwrap(), expected);
        assert_eq!(ctx.read_fcr(31) & (CAUSE_I | FLAG_I), CAUSE_I | FLAG_I);

        ctx.write_fcr(31, 3);
        assert_eq!(ctx.try_fpu_to_i32_s(0, Some(mode as u8)).unwrap(), expected);
    }
    ctx.set_f_d(0, -1.5);
    for (mode, expected) in [(0, -2), (1, -1), (2, -1), (3, -2)] {
        ctx.write_fcr(31, mode);
        assert_eq!(ctx.try_fpu_to_i64_d(0, None).unwrap(), expected);
        ctx.write_fcr(31, 0);
        assert_eq!(ctx.try_fpu_to_i64_d(0, Some(mode as u8)).unwrap(), expected);
    }

    // Enabled Inexact returns before destination commit and adds no Flag.
    ctx.set_f_s(0, 1.5);
    ctx.write_fcr(31, ENABLE_I | FLAG_V);
    assert!(ctx.try_fpu_to_i32_s(0, Some(1)).is_err());
    assert_eq!(ctx.read_fcr(31), CAUSE_I | ENABLE_I | FLAG_V);

    // Legacy SNaN raises V. Disabled V returns the fixed-point QNaN default
    // for the destination width and accumulates Flag.V; enabled V returns
    // typed with no new Flag.
    ctx.set_f_bits(0, 0x7FC0_0001);
    ctx.write_fcr(31, FLAG_I);
    assert_eq!(ctx.try_fpu_to_i32_s(0, Some(1)).unwrap(), i32::MAX);
    assert_eq!(ctx.read_fcr(31), CAUSE_V | FLAG_V | FLAG_I);
    ctx.write_fcr(31, FLAG_I);
    assert_eq!(ctx.try_fpu_to_i64_s(0, Some(1)).unwrap(), i64::MAX);
    assert_eq!(ctx.read_fcr(31), CAUSE_V | FLAG_V | FLAG_I);
    ctx.write_fcr(31, ENABLE_V | FLAG_I);
    assert!(ctx.try_fpu_to_i64_s(0, Some(1)).is_err());
    assert_eq!(ctx.read_fcr(31), CAUSE_V | ENABLE_V | FLAG_I);
    ctx.set_d_bits(0, 0x7FF8_0000_0000_0001);
    ctx.write_fcr(31, FLAG_I);
    assert_eq!(ctx.try_fpu_to_i32_d(0, Some(1)).unwrap(), i32::MAX);
    assert_eq!(ctx.read_fcr(31), CAUSE_V | FLAG_V | FLAG_I);
    ctx.write_fcr(31, FLAG_I);
    assert_eq!(ctx.try_fpu_to_i64_d(0, Some(1)).unwrap(), i64::MAX);
    assert_eq!(ctx.read_fcr(31), CAUSE_V | FLAG_V | FLAG_I);
    ctx.write_fcr(31, ENABLE_V | FLAG_I);
    assert!(ctx.try_fpu_to_i64_d(0, Some(1)).is_err());
    assert_eq!(ctx.read_fcr(31), CAUSE_V | ENABLE_V | FLAG_I);

    // QNaN, denormal, infinity, and post-rounding out-of-range are E-only.
    // Cause.E has no Flag/Enable lane and always returns typed.
    for bits in [0x7F80_0001, 0x0000_0001, 0x7F80_0000, 0x5F00_0000] {
        ctx.set_f_bits(0, bits);
        ctx.write_fcr(31, FLAG_I | FLAG_V);
        assert!(
            ctx.try_fpu_to_i32_s(0, Some(1)).is_err(),
            "S/W bits={bits:#x}"
        );
        assert_eq!(ctx.read_fcr(31), CAUSE_E | FLAG_I | FLAG_V);
        ctx.write_fcr(31, FLAG_I | FLAG_V);
        assert!(
            ctx.try_fpu_to_i64_s(0, Some(1)).is_err(),
            "S/L bits={bits:#x}"
        );
        assert_eq!(ctx.read_fcr(31), CAUSE_E | FLAG_I | FLAG_V);
    }
    for bits in [
        0x7FF0_0000_0000_0001,
        0x0000_0000_0000_0001,
        0x7FF0_0000_0000_0000,
        0x43E0_0000_0000_0000,
    ] {
        ctx.set_d_bits(0, bits);
        ctx.write_fcr(31, FLAG_I | FLAG_V);
        assert!(
            ctx.try_fpu_to_i32_d(0, Some(1)).is_err(),
            "D/W bits={bits:#x}"
        );
        assert_eq!(ctx.read_fcr(31), CAUSE_E | FLAG_I | FLAG_V);
        ctx.write_fcr(31, FLAG_I | FLAG_V);
        assert!(
            ctx.try_fpu_to_i64_d(0, Some(1)).is_err(),
            "D/L bits={bits:#x}"
        );
        assert_eq!(ctx.read_fcr(31), CAUSE_E | FLAG_I | FLAG_V);
    }
}

#[test]
fn float_to_float_raw_vectors_cover_rounding_specials_and_fs() {
    const I_CAUSE: u32 = 1 << 12;
    const U_CAUSE: u32 = 1 << 13;
    const O_CAUSE: u32 = 1 << 14;
    const V_CAUSE: u32 = 1 << 16;
    const E_CAUSE: u32 = 1 << 17;
    const I_FLAG: u32 = 1 << 2;
    const U_FLAG: u32 = 1 << 3;
    const O_FLAG: u32 = 1 << 4;
    const V_FLAG: u32 = 1 << 6;
    const FS: u32 = 1 << 24;

    let mut ctx = RecompContext::new();
    for (single, double) in [
        (0x0000_0000, 0x0000_0000_0000_0000),
        (0x8000_0000, 0x8000_0000_0000_0000),
        (0x3FC0_0000, 0x3FF8_0000_0000_0000),
        (0x7F80_0000, 0x7FF0_0000_0000_0000),
    ] {
        ctx.set_f_bits(0, single);
        assert_eq!(ctx.try_cvt_d_s_bits(0), Ok(double));
        assert_eq!(ctx.read_fcr(31) & (0x3F << 12), 0);
    }

    ctx.set_f_bits(0, 1); // denormal operand
    assert!(ctx.try_cvt_d_s_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (0x3F << 12), E_CAUSE);
    ctx.set_f_bits(0, 0x7F80_0001); // legacy QNaN
    assert!(ctx.try_cvt_d_s_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (0x3F << 12), E_CAUSE);
    ctx.set_f_bits(0, 0x7FC0_0001); // legacy SNaN
    assert_eq!(ctx.try_cvt_d_s_bits(0), Ok(0x7FF7_FFFF_FFFF_FFFF));
    assert_eq!(ctx.read_fcr(31) & (V_CAUSE | V_FLAG), V_CAUSE | V_FLAG);
    ctx.write_fcr(31, 1 << 11); // Enable.V
    assert!(ctx.try_cvt_d_s_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (V_CAUSE | V_FLAG), V_CAUSE);

    ctx.write_fcr(31, 0);
    ctx.set_d_bits(0, 1); // denormal operand
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (0x3F << 12), E_CAUSE);
    ctx.set_d_bits(0, 0x7FF0_0000_0000_0001); // legacy QNaN
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (0x3F << 12), E_CAUSE);
    ctx.set_d_bits(0, 0x7FF8_0000_0000_0001); // legacy SNaN
    assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(0x7FBF_FFFF));
    assert_eq!(ctx.read_fcr(31) & (V_CAUSE | V_FLAG), V_CAUSE | V_FLAG);
    ctx.write_fcr(31, 1 << 11);
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (V_CAUSE | V_FLAG), V_CAUSE);

    for (double, single) in [
        (0x7FF0_0000_0000_0000, 0x7F80_0000),
        (0xFFF0_0000_0000_0000, 0xFF80_0000),
    ] {
        ctx.write_fcr(31, 0);
        ctx.set_d_bits(0, double);
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(single));
        assert_eq!(ctx.read_fcr(31) & (0x3F << 12), 0);
    }

    // Exactly halfway between 1.0f and its successor: nearest-even and both
    // non-increasing modes retain 1.0, RP selects the successor.
    let halfway = 0x3FF0_0000_1000_0000;
    for (mode, expected) in [
        (0, 0x3F80_0000),
        (1, 0x3F80_0000),
        (2, 0x3F80_0001),
        (3, 0x3F80_0000),
    ] {
        ctx.write_fcr(31, mode);
        ctx.set_d_bits(0, halfway);
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(expected));
        assert_eq!(ctx.read_fcr(31) & (I_CAUSE | I_FLAG), I_CAUSE | I_FLAG);
    }
    ctx.write_fcr(31, 1 << 7); // Enable.I
    ctx.set_d_bits(0, halfway);
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (I_CAUSE | I_FLAG), I_CAUSE);

    let negative_halfway = halfway | (1 << 63);
    for (mode, expected) in [
        (0, 0xBF80_0000),
        (1, 0xBF80_0000),
        (2, 0xBF80_0000),
        (3, 0xBF80_0001),
    ] {
        ctx.write_fcr(31, mode);
        ctx.set_d_bits(0, negative_halfway);
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(expected));
        assert_eq!(ctx.read_fcr(31) & (I_CAUSE | I_FLAG), I_CAUSE | I_FLAG);
    }

    // RN ties select the even retained significand. These two boundaries
    // exercise an odd retained LSB and carry out of the 24-bit significand.
    for (double, single) in [
        (0x3FF0_0000_3000_0000, 0x3F80_0002),
        (0x3FFF_FFFF_F000_0000, 0x4000_0000),
    ] {
        ctx.write_fcr(31, 0);
        ctx.set_d_bits(0, double);
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(single));
        assert_eq!(ctx.read_fcr(31) & (I_CAUSE | I_FLAG), I_CAUSE | I_FLAG);
    }

    // 2^128 overflows single. O+I is one atomic exception set, and a precise
    // trap updates neither sticky flag.
    for (mode, expected) in [
        (0, 0x7F80_0000),
        (1, 0x7F7F_FFFF),
        (2, 0x7F80_0000),
        (3, 0x7F7F_FFFF),
    ] {
        ctx.write_fcr(31, mode);
        ctx.set_d_bits(0, 0x47F0_0000_0000_0000);
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(expected));
        assert_eq!(
            ctx.read_fcr(31) & (O_CAUSE | I_CAUSE | O_FLAG | I_FLAG),
            O_CAUSE | I_CAUSE | O_FLAG | I_FLAG
        );
    }
    ctx.write_fcr(31, 1 << 9); // Enable.O
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(
        ctx.read_fcr(31) & (O_CAUSE | I_CAUSE | O_FLAG | I_FLAG),
        O_CAUSE | I_CAUSE
    );

    // The exact halfway boundary above maximum finite single exercises the
    // overflow decision after rounding, for both signs and every RM.
    let max_finite_boundary = 0x47EF_FFFF_F000_0000;
    for (negative, mode, expected, overflow) in [
        (false, 0, 0x7F80_0000, true),
        (false, 1, 0x7F7F_FFFF, false),
        (false, 2, 0x7F80_0000, true),
        (false, 3, 0x7F7F_FFFF, false),
        (true, 0, 0xFF80_0000, true),
        (true, 1, 0xFF7F_FFFF, false),
        (true, 2, 0xFF7F_FFFF, false),
        (true, 3, 0xFF80_0000, true),
    ] {
        ctx.write_fcr(31, mode);
        ctx.set_d_bits(0, max_finite_boundary | if negative { 1 << 63 } else { 0 });
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(expected));
        let expected_fcsr = I_CAUSE | I_FLAG | if overflow { O_CAUSE | O_FLAG } else { 0 };
        assert_eq!(
            ctx.read_fcr(31) & (O_CAUSE | I_CAUSE | O_FLAG | I_FLAG),
            expected_fcsr
        );
    }
    ctx.write_fcr(31, 1 << 7); // Enable.I on O+I
    ctx.set_d_bits(0, max_finite_boundary);
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(
        ctx.read_fcr(31) & (O_CAUSE | I_CAUSE | O_FLAG | I_FLAG),
        O_CAUSE | I_CAUSE
    );

    // Tininess is detected after rounding: this exact halfway value rounds to
    // minimum-normal and raises only I. 2^-127 remains tiny; FS=0 raises E,
    // while FS=1 flushes according to sign/RM and raises U+I.
    ctx.write_fcr(31, 0);
    ctx.set_d_bits(0, 0x380F_FFFF_E000_0000);
    assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(0x0080_0000));
    assert_eq!(ctx.read_fcr(31) & (U_CAUSE | I_CAUSE), I_CAUSE);
    ctx.set_d_bits(0, 0x3800_0000_0000_0000);
    assert!(ctx.try_cvt_s_d_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31) & (0x3F << 12), E_CAUSE);
    for (mode, negative, expected) in [
        (0, false, 0x0000_0000),
        (2, false, 0x0080_0000),
        (3, false, 0x0000_0000),
        (2, true, 0x8000_0000),
        (3, true, 0x8080_0000),
    ] {
        ctx.write_fcr(31, FS | mode);
        ctx.set_d_bits(
            0,
            0x3800_0000_0000_0000 | if negative { 1 << 63 } else { 0 },
        );
        assert_eq!(ctx.try_cvt_s_d_bits(0), Ok(expected));
        assert_eq!(
            ctx.read_fcr(31) & (U_CAUSE | I_CAUSE | U_FLAG | I_FLAG),
            U_CAUSE | I_CAUSE | U_FLAG | I_FLAG
        );
    }
    for enable in [1 << 8, 1 << 7] {
        ctx.write_fcr(31, FS | enable);
        ctx.set_d_bits(0, 0x3800_0000_0000_0000);
        assert!(ctx.try_cvt_s_d_bits(0).is_err());
        assert_eq!(ctx.read_fcr(31) & (0x3F << 12), E_CAUSE);
        assert_eq!(ctx.read_fcr(31) & (U_FLAG | I_FLAG), 0);
    }
}

#[test]
fn cvt_d_s_rebiases_low_normal_exponents_without_unsigned_underflow() {
    let mut ctx = RecompContext::new();
    for single in [
        0x0080_0000u32,
        0x0080_0001,
        0x3F00_0000,
        0x8080_0001,
        0x7F7F_FFFF,
    ] {
        ctx.set_f_bits(0, single);
        assert_eq!(
            ctx.try_cvt_d_s_bits(0),
            Ok((f32::from_bits(single) as f64).to_bits()),
            "single={single:#010x}"
        );
    }
}

#[test]
fn float_to_float_whole_function_uses_typed_golden() {
    let emitted = emit_function(&FuncInput {
        name: "float_to_float",
        vram: 0x8010_2000,
        words: &[
            0x4600_1121, // cvt.d.s $f4,$f2
            0x4620_21A0, // cvt.s.d $f6,$f4
            0x03E0_0008, // jr $ra
            0,
        ],
    });
    assert_eq!(
        emitted.trim_end(),
        include_str!("goldens/float_to_float.rs").trim_end()
    );
}

#[test]
fn float_to_fixed_signed_boundaries_and_half_units_cover_every_rounding_mode() {
    let mut ctx = RecompContext::new();
    let half_units = [
        (-1.5, [-2, -1, -1, -2]),
        (-0.5, [0, 0, 0, -1]),
        (0.5, [0, 0, 1, 0]),
        (1.5, [2, 1, 2, 1]),
    ];

    for (value, expected) in half_units {
        ctx.set_f_s(0, value as f32);
        for (mode, expected) in expected.into_iter().enumerate() {
            ctx.write_fcr(31, 0);
            assert_eq!(
                ctx.try_fpu_to_i32_s(0, Some(mode as u8)).unwrap(),
                expected,
                "S/W value={value} mode={mode}"
            );
            ctx.write_fcr(31, 0);
            assert_eq!(
                ctx.try_fpu_to_i64_s(0, Some(mode as u8)).unwrap(),
                i64::from(expected),
                "S/L value={value} mode={mode}"
            );
        }
    }

    let w_boundaries: &[(f64, [Option<i32>; 4])] = &[
        (
            -2_147_483_648.5,
            [Some(i32::MIN), Some(i32::MIN), Some(i32::MIN), None],
        ),
        (-2_147_483_648.0, [Some(i32::MIN); 4]),
        (
            -2_147_483_647.5,
            [
                Some(i32::MIN),
                Some(i32::MIN + 1),
                Some(i32::MIN + 1),
                Some(i32::MIN),
            ],
        ),
        (
            2_147_483_646.5,
            [
                Some(i32::MAX - 1),
                Some(i32::MAX - 1),
                Some(i32::MAX),
                Some(i32::MAX - 1),
            ],
        ),
        (2_147_483_647.0, [Some(i32::MAX); 4]),
        (
            2_147_483_647.5,
            [None, Some(i32::MAX), None, Some(i32::MAX)],
        ),
        (2_147_483_648.0, [None; 4]),
    ];
    for &(value, expected) in w_boundaries {
        ctx.set_f_d(0, value);
        for (mode, expected) in expected.into_iter().enumerate() {
            ctx.write_fcr(31, 0);
            let actual = ctx.try_fpu_to_i32_d(0, Some(mode as u8));
            match expected {
                Some(expected) => {
                    assert_eq!(actual.unwrap(), expected, "D/W value={value} mode={mode}")
                }
                None => assert!(actual.is_err(), "D/W value={value} mode={mode}"),
            }
        }
    }

    let below_positive_l_limit = f64::from_bits((2f64.powi(63)).to_bits() - 1);
    let above_negative_l_limit = f64::from_bits((-2f64.powi(63)).to_bits() - 1);
    let below_negative_l_limit = f64::from_bits((-2f64.powi(63)).to_bits() + 1);
    assert_eq!(below_positive_l_limit, 2f64.powi(63) - 1024.0);
    assert_eq!(above_negative_l_limit, -2f64.powi(63) + 1024.0);
    assert_eq!(below_negative_l_limit, -2f64.powi(63) - 2048.0);

    for mode in 0..4 {
        for (value, expected) in [
            (-2f64.powi(63), i64::MIN),
            (above_negative_l_limit, i64::MIN + 1024),
            (below_positive_l_limit, i64::MAX - 1023),
        ] {
            ctx.set_f_d(0, value);
            ctx.write_fcr(31, 0);
            assert_eq!(
                ctx.try_fpu_to_i64_d(0, Some(mode)).unwrap(),
                expected,
                "D/L value={value} mode={mode}"
            );
        }
        for value in [below_negative_l_limit, 2f64.powi(63)] {
            ctx.set_f_d(0, value);
            ctx.write_fcr(31, 0);
            assert!(
                ctx.try_fpu_to_i64_d(0, Some(mode)).is_err(),
                "D/L value={value} mode={mode}"
            );
        }
    }
}

#[test]
fn fixed_to_float_boundaries_cover_rounding_exceptions_and_signed_56() {
    const CAUSE_I: u32 = 1 << 12;
    const CAUSE_E: u32 = 1 << 17;
    const ENABLE_I: u32 = 1 << 7;
    const FLAG_I: u32 = 1 << 2;
    const FLAG_V: u32 = 1 << 6;

    let mut ctx = RecompContext::new();
    let single_cases = [
        (
            (1i64 << 24) + 1,
            [0x4B80_0000, 0x4B80_0000, 0x4B80_0001, 0x4B80_0000],
        ),
        (
            -((1i64 << 24) + 1),
            [0xCB80_0000, 0xCB80_0000, 0xCB80_0000, 0xCB80_0001],
        ),
        (
            (1i64 << 24) + 3,
            [0x4B80_0002, 0x4B80_0001, 0x4B80_0002, 0x4B80_0001],
        ),
        (
            -((1i64 << 24) + 3),
            [0xCB80_0002, 0xCB80_0001, 0xCB80_0001, 0xCB80_0002],
        ),
    ];
    for (source, expected) in single_cases {
        ctx.set_d_bits(0, source as u64);
        for (mode, expected) in expected.into_iter().enumerate() {
            ctx.write_fcr(31, mode as u32 | FLAG_V);
            assert_eq!(
                ctx.try_cvt_s_l_bits(0).unwrap(),
                expected,
                "CVT.S.L source={source} RM={mode}"
            );
            assert_eq!(ctx.read_fcr(31), mode as u32 | CAUSE_I | FLAG_I | FLAG_V);
        }
    }

    let double_cases = [
        (
            (1i64 << 53) + 1,
            [
                0x4340_0000_0000_0000,
                0x4340_0000_0000_0000,
                0x4340_0000_0000_0001,
                0x4340_0000_0000_0000,
            ],
        ),
        (
            -((1i64 << 53) + 1),
            [
                0xC340_0000_0000_0000,
                0xC340_0000_0000_0000,
                0xC340_0000_0000_0000,
                0xC340_0000_0000_0001,
            ],
        ),
    ];
    for (source, expected) in double_cases {
        ctx.set_d_bits(0, source as u64);
        for (mode, expected) in expected.into_iter().enumerate() {
            ctx.write_fcr(31, mode as u32);
            assert_eq!(
                ctx.try_cvt_d_l_bits(0).unwrap(),
                expected,
                "CVT.D.L source={source} RM={mode}"
            );
            assert_eq!(ctx.read_fcr(31), mode as u32 | CAUSE_I | FLAG_I);
        }
    }

    // Every W source is admitted. W->D is exact at both signed endpoints;
    // W->S rounds the positive maximum at its adjacent representable values.
    for (source, expected_s, expected_d, inexact_s) in [
        (i32::MIN, [0xCF00_0000; 4], 0xC1E0_0000_0000_0000, false),
        (
            i32::MAX,
            [0x4F00_0000, 0x4EFF_FFFF, 0x4F00_0000, 0x4EFF_FFFF],
            0x41DF_FFFF_FFC0_0000,
            true,
        ),
    ] {
        ctx.set_f_bits(0, source as u32);
        for (mode, expected_s) in expected_s.into_iter().enumerate() {
            let mode = mode as u32;
            ctx.write_fcr(31, mode | CAUSE_E | FLAG_V);
            assert_eq!(
                ctx.try_cvt_s_w_bits(0).unwrap(),
                expected_s,
                "CVT.S.W source={source} RM={mode}"
            );
            assert_eq!(
                ctx.read_fcr(31),
                mode | FLAG_V | if inexact_s { CAUSE_I | FLAG_I } else { 0 }
            );

            ctx.write_fcr(31, mode | CAUSE_E | FLAG_V);
            assert_eq!(
                ctx.try_cvt_d_w_bits(0).unwrap(),
                expected_d,
                "CVT.D.W source={source} RM={mode}"
            );
            assert_eq!(ctx.read_fcr(31), mode | FLAG_V);
        }
    }

    // Disabled Inexact commits a result and accumulates Flag.I. Enabled
    // Inexact returns before the caller can replace its destination and adds
    // no new Flag.I.
    ctx.set_d_bits(0, ((1i64 << 53) + 1) as u64);
    ctx.write_fcr(31, ENABLE_I | FLAG_V);
    assert!(ctx.try_cvt_d_l_bits(0).is_err());
    assert_eq!(ctx.read_fcr(31), CAUSE_I | ENABLE_I | FLAG_V);

    // L-format accepts exactly signed 56 bits. E is always enabled, has no
    // Flag lane, and is independent of RM and destination precision.
    let signed_56_min = -(1i64 << 55);
    let signed_56_max = (1i64 << 55) - 1;
    for (source, expected_s, expected_d, inexact) in [
        (
            signed_56_min,
            [0xDB00_0000; 4],
            [0xC360_0000_0000_0000; 4],
            false,
        ),
        (
            signed_56_max,
            [0x5B00_0000, 0x5AFF_FFFF, 0x5B00_0000, 0x5AFF_FFFF],
            [
                0x4360_0000_0000_0000,
                0x435F_FFFF_FFFF_FFFF,
                0x4360_0000_0000_0000,
                0x435F_FFFF_FFFF_FFFF,
            ],
            true,
        ),
    ] {
        ctx.set_d_bits(0, source as u64);
        for mode in 0..4usize {
            let rm = mode as u32;
            let expected_fcsr = rm | FLAG_V | if inexact { CAUSE_I | FLAG_I } else { 0 };
            ctx.write_fcr(31, rm | FLAG_V);
            assert_eq!(
                ctx.try_cvt_s_l_bits(0).unwrap(),
                expected_s[mode],
                "CVT.S.L signed-56 endpoint={source} RM={mode}"
            );
            assert_eq!(ctx.read_fcr(31), expected_fcsr);
            ctx.write_fcr(31, rm | FLAG_V);
            assert_eq!(
                ctx.try_cvt_d_l_bits(0).unwrap(),
                expected_d[mode],
                "CVT.D.L signed-56 endpoint={source} RM={mode}"
            );
            assert_eq!(ctx.read_fcr(31), expected_fcsr);
        }
    }
    // The two immediately adjacent L values are both outside signed 56 bits:
    // +2^55 and -2^55-1.
    for source in [signed_56_min - 1, signed_56_max + 1] {
        ctx.set_d_bits(0, source as u64);
        for mode in 0..4 {
            for single in [true, false] {
                ctx.write_fcr(31, mode | FLAG_I | FLAG_V);
                let result = if single {
                    ctx.try_cvt_s_l_bits(0).map(u64::from)
                } else {
                    ctx.try_cvt_d_l_bits(0)
                };
                assert!(result.is_err(), "source={source} RM={mode} S={single}");
                assert_eq!(ctx.read_fcr(31), mode | CAUSE_E | FLAG_I | FLAG_V);
            }
        }
    }
}

#[test]
fn oot_os_get_fpc_csr_shape_reads_real_fcr31_state() {
    // __osGetFpcCsr in the allowed MIT OoT output is CFC1 v0,FCR31; JR ra;
    // NOP. The CFC1 word is the public COP1 move encoding, not game content.
    let words = [0x4442_F800, 0x03E0_0008, 0];
    assert_eq!(decode(words[0]), Instruction::Cfc1 { rt: 2, fs: 31 });
    let emitted = emit_function(&FuncInput {
        name: "__osGetFpcCsr",
        vram: 0x8000_0000,
        words: &words,
    });
    assert!(emitted.contains("let v = ctx.read_fcr(31); ctx.set_r32(2, v as i32);"));

    let mut ctx = RecompContext::new();
    ctx.write_fcr(31, (1 << 24) | (1 << 23) | 3);
    assert_eq!(ctx.read_fcr(31), (1 << 24) | (1 << 23) | 3);
}

#[test]
#[should_panic(expected = "FR=0 doubleword read from odd FPR f3")]
fn fr0_odd_double_is_loudly_invalid() {
    let ctx = RecompContext::new();
    let _ = ctx.d_bits(3);
}

#[test]
fn all_integer_branch_likely_delay_slots_are_inside_taken_arm() {
    for word in [0x5085_0002, 0x5485_0002, 0x5880_0002, 0x5C80_0002] {
        let words = [word, 0x2463_0001, 0x2402_0002, 0x03E0_0008, 0];
        let emitted = emit_function(&FuncInput {
            name: "likely",
            vram: 0x8010_0000,
            words: &words,
        });
        let branch_if = emitted.find("            if ").expect("likely branch if");
        let delay = emitted.find("// delay:").expect("delay slot");
        let else_arm = emitted[branch_if..]
            .find("            } else {")
            .expect("else arm")
            + branch_if;
        assert!(
            branch_if < delay && delay < else_arm,
            "delay was not nullified for {word:#010X}\n{emitted}"
        );
    }
}

#[test]
fn jalr_snapshots_target_and_honors_encoded_rd_zero() {
    let emitted = emit_function(&FuncInput {
        name: "jalr_rd0",
        vram: 0x8010_0000,
        words: &[0x0080_0009, 0], // JALR $zero,$a0; NOP
    });
    assert!(emitted.contains("let _target = ctx.r_u32(4);"));
    assert!(emitted.contains("ctx.set_r32(0, 0x80100008u32 as i32);"));
    assert!(emitted.contains("lookup(_target)(ctx, mem);"));
    assert!(!emitted.contains("ctx.set_r32(31,"));
}

#[test]
fn local_jr_jump_table_can_enter_a_straight_line_instruction() {
    // Before the `jr $t9` fix, 0x80100004 was folded into the entry arm because
    // no static branch named it. The emitted local-target range check accepted
    // that address and then fell into the dispatcher's unmapped-vram trap.
    let emitted = emit_function(&FuncInput {
        name: "local_jump_table",
        vram: 0x8010_0000,
        words: &[
            0x2402_0001, // addiu $v0, $zero, 1
            0x2442_0001, // addiu $v0, $v0, 1 -- computed target
            0x0320_0008, // jr $t9
            0x0000_0000, // nop
        ],
    });

    assert!(
        emitted.contains("        0x80100004 => {"),
        "computed local target lacks a dispatcher arm:\n{emitted}"
    );
}
