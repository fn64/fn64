// Recompiled from MIPS function `ctc1_enabled_recomp` @ 0x80110000 (3 instructions).
// Emitted by fn64-recomp-rs (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn ctc1_enabled_recomp(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(0x80110000, "ctc1_enabled_recomp"));
    let mut pc: u32 = 0x80110000;
    'run: loop { match pc {
        0x80110000 => {
            // 0x80110000: Ctc1 { rt: 2, fs: 31 }
            ctx.write_fcr(31, ctx.r_u32(2));
            if ctx.fcsr_exception_pending() { fn64_recomp_rs::trap_unsupported("enabled FCSR cause written by CTC1 in whole-function lane"); }
            // 0x80110004: Jr { rs: 31 }
            // delay: 0x80110008: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
