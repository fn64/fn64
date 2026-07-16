// Recompiled from MIPS function `dword_alu` @ 0x80100000 (21 instructions).
// Emitted by fn64-recomp-native (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn dword_alu(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80100000;
    'run: loop { match pc {
        0x80100000 => {
            // 0x80100000: Daddu { rd: 8, rs: 4, rt: 5 }
            ctx.set_r(8, (ctx.r_u64(4)).wrapping_add(ctx.r_u64(5)));
            // 0x80100004: Dsubu { rd: 9, rs: 4, rt: 5 }
            ctx.set_r(9, (ctx.r_u64(4)).wrapping_sub(ctx.r_u64(5)));
            // 0x80100008: Dsll { rd: 10, rt: 4, sa: 3 }
            ctx.set_r(10, (ctx.r_u64(4)) << 3);
            // 0x8010000C: Dsra { rd: 11, rt: 5, sa: 2 }
            ctx.set_r(11, ((ctx.r_s64(5)) >> 2) as u64);
            // 0x80100010: Dsrl32 { rd: 12, rt: 4, sa: 4 }
            ctx.set_r(12, (ctx.r_u64(4)) >> 36);
            // 0x80100014: Dsllv { rd: 13, rt: 5, rs: 4 }
            ctx.set_r(13, (ctx.r_u64(5)) << (ctx.r_u64(4) & 63));
            // 0x80100018: Daddiu { rt: 14, rs: 4, imm: 256 }
            ctx.set_r(14, (ctx.r_u64(4)).wrapping_add(256i64 as u64));
            // 0x8010001C: Dmult { rs: 4, rt: 5 }
            { let p = (ctx.r_s64(4) as i128) * (ctx.r_s64(5) as i128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }
            // 0x80100020: Mflo { rd: 15 }
            ctx.set_r(15, ctx.lo);
            // 0x80100024: Ddiv { rs: 4, rt: 5 }
            { let a = ctx.r_s64(4); let b = ctx.r_s64(5); if b != 0 { if a == i64::MIN && b == -1 { ctx.lo = a as u64; ctx.hi = 0; } else { ctx.lo = a.wrapping_div(b) as u64; ctx.hi = a.wrapping_rem(b) as u64; } } }
            // 0x80100028: Mflo { rd: 24 }
            ctx.set_r(24, ctx.lo);
            // 0x8010002C: Daddu { rd: 2, rs: 8, rt: 9 }
            ctx.set_r(2, (ctx.r_u64(8)).wrapping_add(ctx.r_u64(9)));
            // 0x80100030: Daddu { rd: 2, rs: 2, rt: 10 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(10)));
            // 0x80100034: Daddu { rd: 2, rs: 2, rt: 11 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(11)));
            // 0x80100038: Daddu { rd: 2, rs: 2, rt: 12 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(12)));
            // 0x8010003C: Daddu { rd: 2, rs: 2, rt: 13 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(13)));
            // 0x80100040: Daddu { rd: 2, rs: 2, rt: 14 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(14)));
            // 0x80100044: Daddu { rd: 2, rs: 2, rt: 15 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(15)));
            // 0x80100048: Daddu { rd: 2, rs: 2, rt: 24 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(24)));
            // 0x8010004C: Jr { rs: 31 }
            // delay: 0x80100050: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
