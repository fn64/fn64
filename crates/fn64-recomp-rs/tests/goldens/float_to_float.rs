// Recompiled from MIPS function `float_to_float` @ 0x80102000 (4 instructions).
// Emitted by fn64-recomp-rs (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn float_to_float(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(0x80102000, "float_to_float"));
    let mut pc: u32 = 0x80102000;
    'run: loop { match pc {
        0x80102000 => {
            // 0x80102000: CvtDS { fd: 4, fs: 2 }
            { let r = ctx.cvt_d_s_bits(2); ctx.set_d_bits(4, r); }
            // 0x80102004: CvtSD { fd: 6, fs: 4 }
            { let r = ctx.cvt_s_d_bits(4); ctx.set_f_bits(6, r); }
            // 0x80102008: Jr { rs: 31 }
            // delay: 0x8010200C: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
