// Recompiled from MIPS function `bg_breakwall_lava_cover_move` @ 0x80901694 (21 instructions).
// Emitted by fn64-recomp-rs (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn bg_breakwall_lava_cover_move(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(0x80901694, "bg_breakwall_lava_cover_move"));
    let mut pc: u32 = 0x80901694;
    'run: loop { match pc {
        0x80901694 => {
            // 0x80901694: Addiu { rt: 29, rs: 29, imm: -24 }
            ctx.set_r32(29, (ctx.r_s32(29)).wrapping_add(-24));
            // 0x80901698: Sw { rt: 31, base: 29, off: 20 }
            mem.store_w(Rdram::eff_addr(ctx.r(29), 20), ctx.r_u32(31));
            // 0x8090169C: Sw { rt: 5, base: 29, off: 28 }
            mem.store_w(Rdram::eff_addr(ctx.r(29), 28), ctx.r_u32(5));
            // 0x809016A0: Or { rd: 7, rs: 4, rt: 0 }
            ctx.set_r(7, ctx.r(4) | 0i64 as u64);
            // 0x809016A4: Lui { rt: 14, imm: 32786 }
            ctx.set_r32(14, 0x80120000u32 as i32);
            // 0x809016A8: Lw { rt: 14, base: 14, off: -17920 }
            ctx.set_r32(14, mem.load_w(Rdram::eff_addr(ctx.r(14), -17920)));
            // 0x809016AC: Lwc1 { ft: 8, base: 7, off: 12 }
            ctx.set_f_bits(8, mem.load_w(Rdram::eff_addr(ctx.r(7), 12)) as u32);
            // 0x809016B0: Addiu { rt: 4, rs: 7, imm: 40 }
            ctx.set_r32(4, (ctx.r_s32(7)).wrapping_add(40));
            // 0x809016B4: Lh { rt: 15, base: 14, off: 2676 }
            ctx.set_r32(15, mem.load_h(Rdram::eff_addr(ctx.r(14), 2676)) as i32);
            // 0x809016B8: Lui { rt: 6, imm: 16256 }
            ctx.set_r32(6, 0x3F800000u32 as i32);
            // 0x809016BC: Mtc1 { rt: 15, fs: 4 }
            ctx.set_f_bits(4, ctx.r_u32(15));
            // 0x809016C0: Nop
            // nop
            // 0x809016C4: CvtSW { fd: 6, fs: 4 }
            ctx.set_f_s(6, (ctx.f_bits(4) as i32) as f32);
            // 0x809016C8: AddS { fd: 10, fs: 6, ft: 8 }
            ctx.fpu_add_s(10, 6, 8);
            // 0x809016CC: Mfc1 { rt: 5, fs: 10 }
            ctx.set_r32(5, ctx.f_bits(10) as i32);
            // 0x809016D0: Jal { target: 101911 }
            ctx.set_r32(31, 0x809016D8u32 as i32);
            // delay: 0x809016D4: Nop
            // nop
            call_host_or_recompiled(0x8006385C, Math_StepToF, ctx, mem);
            pc = 0x809016D8; continue 'run;
        }
        0x809016D8 => {
            // 0x809016D8: Lw { rt: 31, base: 29, off: 20 }
            ctx.set_r32(31, mem.load_w(Rdram::eff_addr(ctx.r(29), 20)));
            // 0x809016DC: Addiu { rt: 29, rs: 29, imm: 24 }
            ctx.set_r32(29, (ctx.r_s32(29)).wrapping_add(24));
            // 0x809016E0: Jr { rs: 31 }
            // delay: 0x809016E4: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
