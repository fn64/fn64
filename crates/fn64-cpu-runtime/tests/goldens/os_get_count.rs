// Recompiled from MIPS function `os_get_count` @ 0x80004D50 (4 instructions).
// Emitted by fn64-cpu-runtime (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn os_get_count(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_cpu_runtime::notify_function_entry(fn64_cpu_runtime::TranslatedFunctionIdentity::new(0x80004D50, "os_get_count"));
    let mut pc: u32 = 0x80004D50;
    'run: loop { match pc {
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
    } }
}
