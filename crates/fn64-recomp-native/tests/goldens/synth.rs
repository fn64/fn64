// Recompiled from MIPS function `synth_recomp` @ 0x80100000 (12 instructions).
// Emitted by fn64-recomp-native (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn synth_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x80100000;
    'run: loop { match pc {
        0x80100000 => {
            // 0x80100000: Mtc1 { rt: 4, fs: 4 }
            ctx.set_f_bits(4, ctx.r_u32(4));
            // 0x80100004: CvtSW { fd: 4, fs: 4 }
            ctx.set_f_s(4, (ctx.f_bits(4) as i32) as f32);
            // 0x80100008: Lwc1 { ft: 6, base: 5, off: 0 }
            ctx.set_f_bits(6, mem.load_w(Rdram::eff_addr(ctx.r(5), 0)) as u32);
            // 0x8010000C: MulS { fd: 8, fs: 4, ft: 6 }
            ctx.set_f_s(8, ctx.f_s(4) * ctx.f_s(6));
            // 0x80100010: AddS { fd: 0, fs: 8, ft: 4 }
            ctx.set_f_s(0, ctx.f_s(8) + ctx.f_s(4));
            // 0x80100014: CLtS { fs: 0, ft: 6 }
            ctx.fpu_cond = ctx.f_s(0) < ctx.f_s(6);
            // 0x80100018: Bc1t { off: 2 }
            let _take = ctx.fpu_cond;
            // delay: 0x8010001C: Addiu { rt: 2, rs: 0, imm: 1 }
            ctx.set_r32(2, (0i32).wrapping_add(1));
            pc = if _take { 0x80100024 } else { 0x80100020 }; continue 'run;
        }
        0x80100020 => {
            // 0x80100020: Addiu { rt: 2, rs: 0, imm: 7 }
            ctx.set_r32(2, (0i32).wrapping_add(7));
            pc = 0x80100024;
        }
        0x80100024 => {
            // 0x80100024: Swc1 { ft: 0, base: 6, off: 0 }
            mem.store_w(Rdram::eff_addr(ctx.r(6), 0), ctx.f_bits(0));
            // 0x80100028: Jr { rs: 31 }
            // delay: 0x8010002C: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
