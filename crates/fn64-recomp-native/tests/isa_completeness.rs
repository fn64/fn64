//! Completeness regressions byte-cited to the public MIPS IV ISA Rev. 3.2
//! encoding tables A-39/B-25 and the NEC VR4300 User's Manual sections
//! 6.3.2 (FCR0/FCR31), 10.8 (LL/SC), and appendix D.2 (division by zero).

use fn64_recomp_native::{
    decode, emit_function, FuncInput, Instruction, Rdram, RecompContext, RDRAM_VBASE,
};

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
    let mut ctx = RecompContext::new();
    assert_eq!(ctx.read_fcr(0) >> 8 & 0xFF, 0x0B);
    for (mode, expected) in [(0, 2), (1, 1), (2, 2), (3, 1)] {
        ctx.write_fcr(31, mode);
        assert_eq!(ctx.fpu_to_i32(1.5, None), expected, "RM={mode}");
        assert_ne!(ctx.read_fcr(31) & (1 << 2), 0, "inexact flag for RM={mode}");
    }

    ctx.set_f_s(0, 1.0);
    ctx.set_f_s(2, 2.0);
    for cond in 0u8..16 {
        ctx.fpu_compare_s(0, 2, cond);
        assert_eq!(ctx.fpu_cond, cond & 4 != 0, "less predicate cond={cond:#x}");
    }
    ctx.set_f_s(0, 2.0);
    for cond in 0u8..16 {
        ctx.fpu_compare_s(0, 2, cond);
        assert_eq!(
            ctx.fpu_cond,
            cond & 2 != 0,
            "equal predicate cond={cond:#x}"
        );
    }
    ctx.set_f_s(0, f32::NAN);
    for cond in 0u8..16 {
        ctx.fpu_compare_s(0, 2, cond);
        assert_eq!(
            ctx.fpu_cond,
            cond & 1 != 0,
            "unordered predicate cond={cond:#x}"
        );
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
#[should_panic(expected = "FR=0 doubleword read from odd FPR")]
fn fr0_rejects_odd_double_register() {
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
