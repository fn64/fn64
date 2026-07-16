// Recompiled from MIPS function `truncf_recomp` @ 0x800CD930 (3 instructions).
// Emitted by fn64-recomp-rs (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn truncf_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
    let mut pc: u32 = 0x800CD930;
    'run: loop { match pc {
        0x800CD930 => {
            // 0x800CD930: TruncWS { fd: 12, fs: 12 }
            { let v = ctx.f_s(12) as f64; let r = ctx.fpu_to_i32(v, Some(1)); ctx.set_f_bits(12, r as u32); }
            // 0x800CD934: Jr { rs: 31 }
            // delay: 0x800CD938: CvtSW { fd: 0, fs: 12 }
            ctx.set_f_s(0, (ctx.f_bits(12) as i32) as f32);
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
