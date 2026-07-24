// Recompiled from MIPS function `truncf_recomp` @ 0x800CD930 (3 instructions).
// Emitted by fn64-recomp-rs (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn truncf_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(0x800CD930, "truncf_recomp"));
    let mut pc: u32 = 0x800CD930;
    'run: loop { match pc {
        0x800CD930 => {
            // 0x800CD930: TruncWS { fd: 12, fs: 12 }
            { let r = ctx.fpu_to_i32_s(12, Some(1)); ctx.set_f_bits(12, r as u32); }
            // 0x800CD934: Jr { rs: 31 }
            // delay: 0x800CD938: CvtSW { fd: 0, fs: 12 }
            { let r = ctx.cvt_s_w_bits(12); ctx.set_f_bits(0, r); }
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
