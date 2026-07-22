// Recompiled from MIPS function `dynapoly_is_bg_id_bg_actor` @ 0x80031264 (8 instructions).
// Emitted by fn64-recomp-rs (typed Rust, no unsafe).
#[allow(unused_variables)]
pub fn dynapoly_is_bg_id_bg_actor(ctx: &mut RecompContext, mem: &mut Rdram) {
    fn64_recomp_rs::notify_function_entry(fn64_recomp_rs::TranslatedFunctionIdentity::new(0x80031264, "dynapoly_is_bg_id_bg_actor"));
    let mut pc: u32 = 0x80031264;
    'run: loop { match pc {
        0x80031264 => {
            // 0x80031264: Bltz { rs: 4, off: 3 }
            let _take = ctx.r_s64(4) < 0;
            // delay: 0x80031268: Slti { rt: 1, rs: 4, imm: 50 }
            ctx.set_r(1, if ctx.r_s64(4) < 50i64 { 1 } else { 0 });
            pc = if _take { 0x80031274 } else { 0x8003126C }; continue 'run;
        }
        0x8003126C => {
            // 0x8003126C: Bne { rs: 1, rt: 0, off: 3 }
            let _take = ctx.r(1) != 0i64 as u64;
            // delay: 0x80031270: Addiu { rt: 2, rs: 0, imm: 1 }
            ctx.set_r32(2, (0i32).wrapping_add(1));
            pc = if _take { 0x8003127C } else { 0x80031274 }; continue 'run;
        }
        0x80031274 => {
            // 0x80031274: Jr { rs: 31 }
            // delay: 0x80031278: Or { rd: 2, rs: 0, rt: 0 }
            ctx.set_r(2, 0i64 as u64 | 0i64 as u64);
            return;
        }
        0x8003127C => {
            // 0x8003127C: Jr { rs: 31 }
            // delay: 0x80031280: Nop
            // nop
            return;
        }
        _ => unreachable!("jumped to unmapped vram {:#X}", pc),
    } }
}
