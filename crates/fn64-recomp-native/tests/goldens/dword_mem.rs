// Recompiled from MIPS function `dword_mem` @ 0x80200000 (14 instructions).
// Emitted by fn64-recomp-native (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn dword_mem(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80200000;
    'run: loop { match pc {
        0x80200000 => {
            // 0x80200000: Ld { rt: 8, base: 4, off: 0 }
            ctx.set_r(8, mem.load_d(Rdram::eff_addr(ctx.r(4), 0)));
            // 0x80200004: Ld { rt: 9, base: 4, off: 8 }
            ctx.set_r(9, mem.load_d(Rdram::eff_addr(ctx.r(4), 8)));
            // 0x80200008: Daddu { rd: 10, rs: 8, rt: 9 }
            ctx.set_r(10, (ctx.r_u64(8)).wrapping_add(ctx.r_u64(9)));
            // 0x8020000C: Sd { rt: 10, base: 4, off: 16 }
            mem.store_d(Rdram::eff_addr(ctx.r(4), 16), ctx.r_u64(10));
            // 0x80200010: Ldl { rt: 11, base: 4, off: 3 }
            ctx.set_r(11, mem.load_dl(ctx.r(11), Rdram::eff_addr(ctx.r(4), 3)));
            // 0x80200014: Ldr { rt: 11, base: 4, off: 10 }
            ctx.set_r(11, mem.load_dr(ctx.r(11), Rdram::eff_addr(ctx.r(4), 10)));
            // 0x80200018: Sd { rt: 11, base: 4, off: 24 }
            mem.store_d(Rdram::eff_addr(ctx.r(4), 24), ctx.r_u64(11));
            // 0x8020001C: Sdl { rt: 8, base: 4, off: 32 }
            mem.store_dl(Rdram::eff_addr(ctx.r(4), 32), ctx.r_u64(8));
            // 0x80200020: Sdr { rt: 8, base: 4, off: 39 }
            mem.store_dr(Rdram::eff_addr(ctx.r(4), 39), ctx.r_u64(8));
            // 0x80200024: Lld { rt: 12, base: 4, off: 40 }
            ctx.set_r(12, mem.load_d(Rdram::eff_addr(ctx.r(4), 40)));
            // 0x80200028: Daddiu { rt: 12, rs: 12, imm: 1 }
            ctx.set_r(12, (ctx.r_u64(12)).wrapping_add(1i64 as u64));
            // 0x8020002C: Scd { rt: 12, base: 4, off: 40 }
            mem.store_d(Rdram::eff_addr(ctx.r(4), 40), ctx.r_u64(12));
            ctx.set_r(12, 1);
            // 0x80200030: Jr { rs: 31 }
            // delay: 0x80200034: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
