//! COP0 system-control / COP2-stub / trap family tests for
//! `fn64-recomp-rs`.
//!
//! # The oracle
//!
//! The reference behaviour is a **real OoT libultra function** that uses an op
//! from this family: `osGetCount` @ vram `0x80004D50` (4 words, extracted from
//! `oot-ntsc-1.0.z64` ROM offset `0x5950`). Its body is the canonical
//! Count-register read:
//!
//! ```asm
//! osGetCount:
//!   0x80004D50  40024800  mfc0  $v0, $9   ; $v0 = COP0 Count
//!   0x80004D54  03E00008  jr    $ra
//!   0x80004D58  00000000  nop             ; (delay slot)
//!   0x80004D5C  00000000  nop             ; (alignment padding)
//! ```
//!
//! `mfc0 $v0, $9` reads COP0 register 9 (`Count`) — the free-running cycle
//! counter — into `$v0` and returns it. The decomp/N64Recomp semantics are
//! simply `return c0_count`. [`os_get_count_oracle`] is that behaviour,
//! hand-transcribed independently of the emitter. The differential test
//! recompiles the SAME ROM bytes with OUR emitter, executes the emitted Rust,
//! and asserts `$v0` equals the oracle across a sweep of `Count` values
//! spanning zero, small, sign-bit-set, and max. This is the strong check, not
//! a bbox/fuzzy one.
//!
//! The arbitrary-PC lane models Status/Cause/EPC moves, ERET, typed
//! synchronous exceptions, and the public indexed write/read/probe TLB
//! management operations. TLBWR, address translation, and unavailable
//! coprocessor state remain loud.
//! Decoder and structural emitter tests keep that boundary explicit.

use fn64_recomp_rs::{decode, emit_function, FuncInput, Instruction, Rdram, RecompContext};

/// Real ROM bytes of `osGetCount` (big-endian words).
const OSGETCOUNT_WORDS: [u32; 4] = [
    0x40024800, // mfc0 $v0, $9   (Count)
    0x03E00008, // jr   $ra
    0x00000000, // nop            (delay slot)
    0x00000000, // nop            (alignment padding)
];
const OSGETCOUNT_VRAM: u32 = 0x80004D50;

// --- The oracle: hand-transcribed from the ISA/decomp, NOT the emitter. ---
//
// `mfc0 rt, $9` sign-extends the 32-bit Count register into the 64-bit GPR
// (MFC0 is a 32-bit move, so bit 31 fills bits 63..32). Returns the value left
// in $v0 (ctx.r(2)).
fn os_get_count_oracle(count: u32) -> u64 {
    // 32-bit read, sign-extended into the 64-bit register.
    count as i32 as i64 as u64
}

// --- The emitter's output, pasted VERBATIM (kept honest by the golden). ---
//
// `unused_labels` is allowed because this particular function returns without
// ever using `continue 'run` (osGetCount has no in-function branch); the
// emitter still emits the `'run:` label uniformly, and the golden compares the
// emitter's text, not this fn's attributes.
#[allow(unused_variables, unused_labels, clippy::all)]
pub fn os_get_count(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(
        0x80004D50,
        "os_get_count",
    ));
    let mut pc: u32 = 0x80004D50;
    'run: loop {
        match pc {
            0x80004D50 => {
                // 0x80004D50: Mfc0 { rt: 2, cop0d: 9 }
                ctx.set_r32(2, ctx.cop0_count as i32);
                // 0x80004D54: Jr { rs: 31 }
                // delay: 0x80004D58: Nop
                // nop
                return;
            }
            0x80004D5C => {
                // 0x80004D5C: Nop
                // nop
                pc = 0x80004D60;
            }
            _ => unreachable!("jumped to unmapped vram {:#X}", pc),
        }
    }
}

/// The pasted-verbatim body above must be byte-identical to what the live
/// emitter produces (ignoring the emitted fn NAME, which the test fixes here by
/// naming both `os_get_count`). If the emitter changes shape, this fails loudly
/// and the paste + golden must be refreshed — keeping the executed code honest.
#[test]
fn emitter_output_matches_pasted_os_get_count() {
    let input = FuncInput {
        name: "os_get_count",
        vram: OSGETCOUNT_VRAM,
        words: &OSGETCOUNT_WORDS,
    };
    let emitted = emit_function(&input);
    let pasted = include_str!("goldens/os_get_count.rs");
    let norm = |s: &str| s.trim_end().replace("\r\n", "\n");
    assert_eq!(
        norm(&emitted),
        norm(pasted),
        "emitter output drifted from tests/goldens/os_get_count.rs; refresh the golden \
         and the pasted fn if this change is intended"
    );
}

/// The core oracle validation: our emitted+executed Rust must agree with the
/// hand-transcribed oracle on the return value ($v0 = ctx.r(2)) for every
/// sampled Count value, including sign-bit-set (which the 32-bit `MFC0` must
/// sign-extend into the 64-bit GPR).
#[test]
fn os_get_count_matches_oracle() {
    let counts: [u32; 8] = [
        0,
        1,
        0x0000_1234,
        0x7FFF_FFFF, // largest positive 32-bit
        0x8000_0000, // sign bit set -> must sign-extend to 0xFFFF_FFFF_8000_0000
        0xFFFF_FFFF, // -1 -> 0xFFFF_FFFF_FFFF_FFFF
        0xDEAD_BEEF,
        0x1357_9BDF,
    ];
    for &count in &counts {
        let mut mem_buf = vec![0u8; 64];
        let mut mem = Rdram::new(&mut mem_buf);
        let mut ctx = RecompContext::new();
        ctx.cop0_count = count;

        os_get_count(&mut ctx, &mut mem);

        let got = ctx.r(2); // $v0
        let expected = os_get_count_oracle(count);
        assert_eq!(
            got, expected,
            "divergence from oracle for Count = {count:#010X}: emitter got {got:#018X}, \
             oracle {expected:#018X}"
        );
    }
}

/// MTC0 to Compare (reg 11) round-trips through the typed context — the
/// `osSetTimer` write path. Exercises the write half of the modeled COP0 state
/// with a hand-built `mtc0 $a0, $11; jr $ra; nop` function.
#[test]
fn mtc0_compare_round_trips() {
    // mtc0 $a0, $11 = opcode 0x10, rs=0x04 (MTC0), rt=4 ($a0), rd=11 (Compare).
    // 0x40 | (0x04<<21) | (4<<16) | (11<<11) = 0x40845800.
    let words: [u32; 3] = [0x40845800, 0x03E00008, 0x00000000];
    let input = FuncInput {
        name: "set_compare",
        vram: 0x80001000,
        words: &words,
    };
    let emitted = emit_function(&input);
    assert!(
        emitted.contains("ctx.write_cop0(11, ctx.r_u32(4));"),
        "mtc0 $a0,$11 should write cop0_compare from $a0; emitted:\n{emitted}"
    );

    // And executing it (via a local paste of the emitted shape) must land the
    // value. We assert the decode + emit intent here; execution parity is
    // covered by the osGetCount differential above (same set_r32/context path).
    assert_eq!(decode(0x40845800), Instruction::Mtc0 { rt: 4, cop0d: 11 });
}

#[test]
fn mtc0_status_and_cause_use_the_typed_interrupt_state() {
    let status = emit_function(&FuncInput {
        name: "set_status",
        vram: 0x8000_1000,
        words: &[0x4084_6000, 0x03E0_0008, 0],
    });
    assert!(status.contains("ctx.write_cop0(12, ctx.r_u32(4));"));

    let cause = emit_function(&FuncInput {
        name: "set_cause",
        vram: 0x8000_1000,
        words: &[0x4084_6800, 0x03E0_0008, 0],
    });
    assert!(cause.contains("ctx.write_cop0(13, ctx.r_u32(4));"));
}

// --- Decoder unit tests (known word -> right op), fail-against-bug. ---

#[test]
fn decode_cop0_moves() {
    // mfc0 $v0, $9  (Count)    = 0x40024800
    assert_eq!(decode(0x40024800), Instruction::Mfc0 { rt: 2, cop0d: 9 });
    // mtc0 $a0, $11 (Compare)  = 0x40845800
    assert_eq!(decode(0x40845800), Instruction::Mtc0 { rt: 4, cop0d: 11 });
    // mfc0 $t0, $12 (Status)   = 0x40086000  (rt=8, rd=12)
    assert_eq!(decode(0x40086000), Instruction::Mfc0 { rt: 8, cop0d: 12 });
    // mtc0 $t0, $12 (Status)   = 0x40886000
    assert_eq!(decode(0x40886000), Instruction::Mtc0 { rt: 8, cop0d: 12 });
    // dmfc0 $v0, $9            = rs=0x01 -> 0x40224800
    assert_eq!(decode(0x40224800), Instruction::Dmfc0 { rt: 2, cop0d: 9 });
    // dmtc0 $v0, $9            = rs=0x05 -> 0x40A24800
    assert_eq!(decode(0x40A24800), Instruction::Dmtc0 { rt: 2, cop0d: 9 });
}

#[test]
fn decode_cop0_privileged() {
    // These are the "CO=1" (rs bit 25 set) funct-encoded ops. Encoding =
    // opcode 0x10 | (0x10<<21) [CO bit] | funct.  0x42000000 | funct.
    // tlbr  funct 0x01 = 0x42000001
    assert_eq!(decode(0x42000001), Instruction::Tlbr);
    // tlbwi funct 0x02 = 0x42000002
    assert_eq!(decode(0x42000002), Instruction::Tlbwi);
    // tlbwr funct 0x06 = 0x42000006
    assert_eq!(decode(0x42000006), Instruction::Tlbwr);
    // tlbp  funct 0x08 = 0x42000008
    assert_eq!(decode(0x42000008), Instruction::Tlbp);
    // eret  funct 0x18 = 0x42000018
    assert_eq!(decode(0x42000018), Instruction::Eret);
}

#[test]
fn decode_cache_and_sync() {
    // cache 0x00, 0(base=$a0) : opcode 0x2F, base(rs)=4, op(rt)=0, off=0
    //   0xBC800000 | (4<<21) = 0xBC800000; rs=4 -> 0xBC80_0000 with rs bits
    //   (0x2F<<26)=0xBC000000, (4<<21)=0x00800000 -> 0xBC800000.
    assert_eq!(
        decode(0xBC800000),
        Instruction::Cache {
            op: 0,
            base: 4,
            off: 0
        }
    );
    // cache 0x14, 0x10($t0): op field = 0x14 (rt), base=8, off=0x10
    //   0xBC000000 | (8<<21) | (0x14<<16) | 0x10 = 0xBD140010
    assert_eq!(
        decode(0xBD140010),
        Instruction::Cache {
            op: 0x14,
            base: 8,
            off: 0x10
        }
    );
    // sync  = SPECIAL funct 0x0F = 0x0000000F
    assert_eq!(decode(0x0000000F), Instruction::Sync);
}

#[test]
fn decode_cop2_stubs() {
    // mfc2 $v0, $3 : opcode 0x12, rs=0x00, rt=2, rd=3 -> 0x48021800
    assert_eq!(decode(0x48021800), Instruction::Mfc2 { rt: 2, rd: 3 });
    // cfc2 $v0, $3 : rs=0x02 -> 0x48421800
    assert_eq!(decode(0x48421800), Instruction::Cfc2 { rt: 2, rd: 3 });
    // mtc2 $v0, $3 : rs=0x04 -> 0x48821800
    assert_eq!(decode(0x48821800), Instruction::Mtc2 { rt: 2, rd: 3 });
    // ctc2 $v0, $3 : rs=0x06 -> 0x48C21800
    assert_eq!(decode(0x48C21800), Instruction::Ctc2 { rt: 2, rd: 3 });
}

#[test]
fn decode_traps() {
    // syscall (code 0)  = 0x0000000C
    assert_eq!(decode(0x0000000C), Instruction::Syscall { code: 0 });
    // break   (code 0)  = 0x0000000D
    assert_eq!(decode(0x0000000D), Instruction::Break { code: 0 });
    // syscall with code 0x2A in the code field (bits 25..6):
    //   0x2A << 6 = 0xA80, | funct 0x0C = 0xA8C
    assert_eq!(decode(0x00000A8C), Instruction::Syscall { code: 0x2A });
    // break with code 0x7: (0x7<<6)|0x0D = 0x1CD
    assert_eq!(decode(0x000001CD), Instruction::Break { code: 0x7 });
}

/// The whole family is straight-line (no delay slot) — none is a branch.
#[test]
fn cop0_family_has_no_delay_slot() {
    assert!(!decode(0x40024800).has_delay_slot()); // mfc0
    assert!(!decode(0x42000018).has_delay_slot()); // eret
    assert!(!decode(0x0000000F).has_delay_slot()); // sync
    assert!(!decode(0x0000000C).has_delay_slot()); // syscall
    assert!(!decode(0xBC800000).has_delay_slot()); // cache
    assert!(!decode(0x48021800).has_delay_slot()); // mfc2
}

// --- Structural emit tests: privileged ops trap, cache/sync are no-ops. ---

/// Emit each privileged/unused op inside a tiny function and assert the
/// generated Rust is a loud trap — NEVER a silent nop. Unsupported runtime
/// gaps use the host-observed trap endpoint; architectural guest traps retain
/// their direct panic.
#[test]
fn privileged_ops_emit_loud_traps() {
    struct Case {
        word: u32,
        needle: &'static str,
    }
    let cases = [
        Case {
            word: 0x40224800,
            needle: "unsupported dmfc0",
        }, // dmfc0
        Case {
            word: 0x40A24800,
            needle: "unsupported dmtc0",
        }, // dmtc0
        Case {
            word: 0x42000018,
            needle: "eret executed in recompiled code",
        }, // eret
        // tlbwi
        Case {
            word: 0x42000006,
            needle: "tlbwr",
        }, // tlbwr
        Case {
            word: 0x48021800,
            needle: "COP2 access in recompiled code",
        }, // mfc2
        Case {
            word: 0x48821800,
            needle: "COP2 access in recompiled code",
        }, // mtc2
        Case {
            word: 0x48421800,
            needle: "COP2 access in recompiled code",
        }, // cfc2
        Case {
            word: 0x48C21800,
            needle: "COP2 access in recompiled code",
        }, // ctc2
        Case {
            word: 0x0000000C,
            needle: "syscall (code 0x0) executed",
        }, // syscall
        Case {
            word: 0x0000000D,
            needle: "break (code 0x0) executed",
        }, // break
    ];
    for c in &cases {
        // op ; jr $ra ; nop
        let words = [c.word, 0x03E00008, 0x00000000];
        let input = FuncInput {
            name: "t",
            vram: 0x80001000,
            words: &words,
        };
        let emitted = emit_function(&input);
        assert!(
            emitted.contains("panic!") || emitted.contains("trap_unsupported"),
            "op {:#010X} must emit a loud trap, got:\n{emitted}",
            c.word
        );
        assert!(
            emitted.contains(c.needle),
            "op {:#010X} trap message missing {:?}; emitted:\n{emitted}",
            c.word,
            c.needle
        );
    }
}

/// `cache` and `sync` must emit as *no-ops with a comment* — the correct
/// behaviour on a coherent host rdram — and must NOT emit a panic (they are
/// legitimate, frequent instructions, not privileged traps).
#[test]
fn cache_and_sync_emit_noops() {
    // cache 0x14, 0x10($t0)
    let words = [0xBD140010u32, 0x03E00008, 0x00000000];
    let input = FuncInput {
        name: "t",
        vram: 0x80001000,
        words: &words,
    };
    let emitted = emit_function(&input);
    assert!(
        emitted.contains("// cache op 0x14: no-op"),
        "cache should no-op:\n{emitted}"
    );
    assert!(
        !emitted.contains("panic!"),
        "cache must not trap:\n{emitted}"
    );

    // sync
    let words = [0x0000000Fu32, 0x03E00008, 0x00000000];
    let input = FuncInput {
        name: "t2",
        vram: 0x80001000,
        words: &words,
    };
    let emitted = emit_function(&input);
    assert!(
        emitted.contains("// sync: no-op"),
        "sync should no-op:\n{emitted}"
    );
    assert!(
        !emitted.contains("panic!"),
        "sync must not trap:\n{emitted}"
    );
}

/// A `mfc0` from an unsupported register decodes fine but
/// its emitted body traps at runtime. Prove the emitted panic message names the
/// register, so a game hitting it fails audibly with a diagnosable cause.
#[test]
fn unsupported_cop0_read_names_the_register() {
    // mfc0 $t0, $7 : rt=8, rd=7 -> 0x40083800
    assert_eq!(decode(0x40083800), Instruction::Mfc0 { rt: 8, cop0d: 7 });
    let words = [0x40083800u32, 0x03E00008, 0x00000000];
    let input = FuncInput {
        name: "t",
        vram: 0x80001000,
        words: &words,
    };
    let emitted = emit_function(&input);
    assert!(
        emitted.contains("unsupported mfc0 from COP0 register 7"),
        "unsupported read should name register 7:\n{emitted}"
    );
}

#[test]
fn modeled_exception_register_reads_emit_typed_accesses() {
    for (word, reg) in [
        (0x4008_4000, 8u8),
        (0x4008_6000, 12),
        (0x4008_6800, 13),
        (0x4008_7000, 14),
        (0x4008_F000, 30),
    ] {
        let emitted = emit_function(&FuncInput {
            name: "t",
            vram: 0x8000_1000,
            words: &[word, 0x03E0_0008, 0],
        });
        assert!(
            emitted.contains(&format!("ctx.read_cop0({reg})")),
            "COP0 register {reg} should use typed state:\n{emitted}"
        );
        assert!(!emitted.contains("panic!"));
    }
}

#[test]
fn tlbwi_records_the_staged_entry_instead_of_trapping() {
    // tlbwi ; jr $ra ; nop -- boot-time TLB setup must run, not trap;
    // translation through recorded entries stays unmodeled (use faults).
    let words = [0x42000002, 0x03E00008, 0x00000000];
    let input = FuncInput {
        name: "t",
        vram: 0x80001000,
        words: &words,
    };
    let emitted = emit_function(&input);
    assert!(
        emitted.contains("ctx.tlbwi_record();"),
        "tlbwi must record the staged entry; emitted:\n{emitted}"
    );
}

#[test]
fn tlbr_and_tlbp_emit_typed_management_operations() {
    for (word, needle) in [
        (0x4200_0001, "ctx.tlbr_read();"),
        (0x4200_0008, "ctx.tlbp_probe();"),
    ] {
        let emitted = emit_function(&FuncInput {
            name: "t",
            vram: 0x8000_1000,
            words: &[word, 0x03E0_0008, 0],
        });
        assert!(
            emitted.contains(needle),
            "TLB management op {word:#010x} must use typed state:\n{emitted}"
        );
        assert!(!emitted.contains("trap_unsupported"));
    }
}

#[test]
fn tlb_read_and_probe_follow_vpn_mask_asid_and_global_rules() {
    let mut ctx = RecompContext::new();
    ctx.cop0_index = 7;
    ctx.cop0_page_mask = 0x0000_6000;
    ctx.cop0_entry_hi = 0x1234_400a;
    ctx.cop0_entry_lo0 = 0x0000_0046;
    ctx.cop0_entry_lo1 = 0x0000_0086;
    ctx.tlbwi_record();

    ctx.cop0_page_mask = 0;
    ctx.cop0_entry_hi = 0;
    ctx.cop0_entry_lo0 = 0;
    ctx.cop0_entry_lo1 = 0;
    ctx.tlbr_read();
    assert_eq!(ctx.cop0_page_mask, 0x0000_6000);
    assert_eq!(ctx.cop0_entry_hi, 0x1234_400a);
    assert_eq!(ctx.cop0_entry_lo0, 0x0000_0046);
    assert_eq!(ctx.cop0_entry_lo1, 0x0000_0086);

    // The entry's 16 KiB pair masks VPN2 bits 14:13; same ASID matches.
    ctx.cop0_entry_hi = 0x1234_200a;
    ctx.tlbp_probe();
    assert_eq!(ctx.cop0_index, 7);

    // A non-global entry requires ASID equality.
    ctx.cop0_entry_hi = 0x1234_200b;
    ctx.tlbp_probe();
    assert_eq!(ctx.cop0_index, 0x8000_0000);

    // Both EntryLo G bits make the mapping global; Valid is irrelevant to the
    // probe comparison.
    ctx.tlb_entries[7].entry_lo0 = 1;
    ctx.tlb_entries[7].entry_lo1 = 1;
    ctx.tlbp_probe();
    assert_eq!(ctx.cop0_index, 7);

    ctx.cop0_entry_hi = 0x2234_200b;
    ctx.tlbp_probe();
    assert_eq!(ctx.cop0_index, 0x8000_0000);
}

#[test]
fn tlbp_rejects_the_architecturally_undefined_multiple_match_case() {
    let mut ctx = RecompContext::new();
    ctx.cop0_entry_hi = 0x3456_700c;
    for index in [2u32, 19] {
        ctx.cop0_index = index;
        ctx.cop0_page_mask = 0;
        ctx.cop0_entry_hi = 0x3456_600c;
        ctx.cop0_entry_lo0 = 0;
        ctx.cop0_entry_lo1 = 0;
        ctx.tlbwi_record();
    }
    ctx.cop0_entry_hi = 0x3456_700c;
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.tlbp_probe()))
        .expect_err("multiple matching TLB entries must remain loud");
    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .expect("TLBP undefined-case panic must be text");
    assert_eq!(
        message,
        "TLBP found multiple matching entries; VR4300 behavior is undefined"
    );
}
