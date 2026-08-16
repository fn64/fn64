//! AOT routing coverage for the public VR4300 S/D→W/L conversion matrix.
//!
//! Encoding fields follow the NEC VR4300 User's Manual COP1 instruction
//! format. This checks emitter routing only; raw exceptional behavior is
//! exercised through the typed runtime tests in `isa_completeness.rs`.

use fn64_cpu_runtime::BankId;
use fn64_recomp_rs_codegen::{emit_bank_runner, BankInput};

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    word: u32,
    helper: &'static str,
    destination: &'static str,
}

#[test]
fn aot_maps_all_twenty_float_to_fixed_encodings_by_source_and_destination_width() {
    const CASES: [Case; 20] = [
        Case {
            name: "ROUND.L.S",
            word: 0x4600_1108,
            helper: "ctx.try_fpu_to_i64_s(2, Some(0))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "TRUNC.L.S",
            word: 0x4600_1109,
            helper: "ctx.try_fpu_to_i64_s(2, Some(1))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "CEIL.L.S",
            word: 0x4600_110A,
            helper: "ctx.try_fpu_to_i64_s(2, Some(2))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "FLOOR.L.S",
            word: 0x4600_110B,
            helper: "ctx.try_fpu_to_i64_s(2, Some(3))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "ROUND.W.S",
            word: 0x4600_110C,
            helper: "ctx.try_fpu_to_i32_s(2, Some(0))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "TRUNC.W.S",
            word: 0x4600_110D,
            helper: "ctx.try_fpu_to_i32_s(2, Some(1))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "CEIL.W.S",
            word: 0x4600_110E,
            helper: "ctx.try_fpu_to_i32_s(2, Some(2))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "FLOOR.W.S",
            word: 0x4600_110F,
            helper: "ctx.try_fpu_to_i32_s(2, Some(3))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "CVT.W.S",
            word: 0x4600_1124,
            helper: "ctx.try_fpu_to_i32_s(2, None)",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "CVT.L.S",
            word: 0x4600_1125,
            helper: "ctx.try_fpu_to_i64_s(2, None)",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "ROUND.L.D",
            word: 0x4620_1108,
            helper: "ctx.try_fpu_to_i64_d(2, Some(0))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "TRUNC.L.D",
            word: 0x4620_1109,
            helper: "ctx.try_fpu_to_i64_d(2, Some(1))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "CEIL.L.D",
            word: 0x4620_110A,
            helper: "ctx.try_fpu_to_i64_d(2, Some(2))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "FLOOR.L.D",
            word: 0x4620_110B,
            helper: "ctx.try_fpu_to_i64_d(2, Some(3))",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
        Case {
            name: "ROUND.W.D",
            word: 0x4620_110C,
            helper: "ctx.try_fpu_to_i32_d(2, Some(0))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "TRUNC.W.D",
            word: 0x4620_110D,
            helper: "ctx.try_fpu_to_i32_d(2, Some(1))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "CEIL.W.D",
            word: 0x4620_110E,
            helper: "ctx.try_fpu_to_i32_d(2, Some(2))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "FLOOR.W.D",
            word: 0x4620_110F,
            helper: "ctx.try_fpu_to_i32_d(2, Some(3))",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "CVT.W.D",
            word: 0x4620_1124,
            helper: "ctx.try_fpu_to_i32_d(2, None)",
            destination: "ctx.set_f_bits(4, r as u32)",
        },
        Case {
            name: "CVT.L.D",
            word: 0x4620_1125,
            helper: "ctx.try_fpu_to_i64_d(2, None)",
            destination: "ctx.set_d_bits(4, r as u64)",
        },
    ];

    for case in CASES {
        let emitted = emit_bank_runner(&BankInput {
            name: "conversion_bank",
            bank: BankId::new(0xA07),
            vram: 0x8000_0000,
            words: &[case.word],
        });
        assert!(
            emitted.contains(case.helper),
            "{} did not use {}:\n{}",
            case.name,
            case.helper,
            emitted
        );
        assert!(
            emitted.contains(case.destination),
            "{} did not commit through {}:\n{}",
            case.name,
            case.destination,
            emitted
        );
    }
}
