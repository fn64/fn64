// Recompiled from MIPS function `dword_alu2` @ 0x80300000 (25 instructions).
// Emitted by fn64-recomp-native (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn dword_alu2(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80300000;
    'run: loop { match pc {
        0x80300000 => {
            // 0x80300000: Dmultu { rs: 4, rt: 5 }
            { let p = (ctx.r_u64(4) as u128) * (ctx.r_u64(5) as u128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }
            // 0x80300004: Mflo { rd: 8 }
            ctx.set_r(8, ctx.lo);
            // 0x80300008: Dmultu { rs: 4, rt: 5 }
            { let p = (ctx.r_u64(4) as u128) * (ctx.r_u64(5) as u128); ctx.lo = p as u64; ctx.hi = (p >> 64) as u64; }
            // 0x8030000C: Mfhi { rd: 9 }
            ctx.set_r(9, ctx.hi);
            // 0x80300010: Ddivu { rs: 4, rt: 5 }
            ctx.div_u64(ctx.r_u64(4), ctx.r_u64(5));
            // 0x80300014: Mflo { rd: 10 }
            ctx.set_r(10, ctx.lo);
            // 0x80300018: Ddivu { rs: 4, rt: 5 }
            ctx.div_u64(ctx.r_u64(4), ctx.r_u64(5));
            // 0x8030001C: Mfhi { rd: 11 }
            ctx.set_r(11, ctx.hi);
            // 0x80300020: Dsrl { rd: 12, rt: 4, sa: 5 }
            ctx.set_r(12, (ctx.r_u64(4)) >> 5);
            // 0x80300024: Dsll32 { rd: 13, rt: 5, sa: 7 }
            ctx.set_r(13, (ctx.r_u64(5)) << 39);
            // 0x80300028: Dsra32 { rd: 14, rt: 4, sa: 9 }
            ctx.set_r(14, ((ctx.r_s64(4)) >> 41) as u64);
            // 0x8030002C: Dsrlv { rd: 15, rt: 5, rs: 4 }
            ctx.set_r(15, (ctx.r_u64(5)) >> (ctx.r_u64(4) & 63));
            // 0x80300030: Dsrav { rd: 24, rt: 4, rs: 5 }
            ctx.set_r(24, ((ctx.r_s64(4)) >> (ctx.r_u64(5) & 63)) as u64);
            // 0x80300034: Daddiu { rt: 25, rs: 4, imm: 127 }
            ctx.set_r(25, (ctx.r_u64(4)).wrapping_add(127i64 as u64));
            // 0x80300038: Daddu { rd: 2, rs: 8, rt: 9 }
            ctx.set_r(2, (ctx.r_u64(8)).wrapping_add(ctx.r_u64(9)));
            // 0x8030003C: Daddu { rd: 2, rs: 2, rt: 10 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(10)));
            // 0x80300040: Daddu { rd: 2, rs: 2, rt: 11 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(11)));
            // 0x80300044: Daddu { rd: 2, rs: 2, rt: 12 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(12)));
            // 0x80300048: Daddu { rd: 2, rs: 2, rt: 13 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(13)));
            // 0x8030004C: Daddu { rd: 2, rs: 2, rt: 14 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(14)));
            // 0x80300050: Daddu { rd: 2, rs: 2, rt: 15 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(15)));
            // 0x80300054: Daddu { rd: 2, rs: 2, rt: 24 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(24)));
            // 0x80300058: Daddu { rd: 2, rs: 2, rt: 25 }
            ctx.set_r(2, (ctx.r_u64(2)).wrapping_add(ctx.r_u64(25)));
            // 0x8030005C: Jr { rs: 31 }
            // delay: 0x80300060: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
