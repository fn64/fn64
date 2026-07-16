//! `rsp_trace` — run a recompiled ucode through a reference interpreter that
//! mirrors the emitter's control flow, logging the pc trace so a runaway loop
//! or a wrong branch is diagnosable. Uses the SAME typed `RspMachine` runtime
//! the generated code targets, so a divergence here is a real semantic bug.

use std::collections::BTreeMap;

use fn64_audio::rsp::context::RspExitReason;
use fn64_audio::rsp::decode::{decode, BranchOp, BranchZOp, Instr};
use fn64_audio::rsp::ops::{dispatch, OpInvocation, OpStatus};
use fn64_audio::rsp::recomp::runtime::RspMachine;

fn read_words(path: &str) -> Vec<u32> {
    let bytes = std::fs::read(path).unwrap();
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let base: u32 = args
        .get(3)
        .map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
        .unwrap_or(0x1000);
    let cap: u64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(2000);
    let words = read_words(path);
    let n = words.len();

    let mut rdram = vec![0u8; 0x0080_0000];
    // Native-endian OSTask (fn64 rdram is native-endian word storage).
    let put_w = |rd: &mut [u8], off: usize, v: u32| {
        rd[off..off + 4].copy_from_slice(&v.to_ne_bytes());
    };
    let task = 0x2000usize;
    let ucode_data = 0x3000usize;
    let cmd_list = 0x5000usize;
    let src_pcm = 0x6000usize;

    // Seed the real DMEM data image (jump table) from ASPMAIN_DATA if provided.
    // The incbin is big-endian (ROM order); in the live game it reaches rdram
    // via the C runtime's `^3`-swizzled DMA (fn64 rdram is native-endian word
    // storage). Replicate that `^3` swizzle so the RSP's `^2`/`^3` DMEM
    // accessors read the jump table on the correct lanes.
    if let Some(dp) = std::env::var_os("ASPMAIN_DATA") {
        let d = std::fs::read(&dp).unwrap();
        for (k, &b) in d.iter().enumerate() {
            rdram[(ucode_data + k) ^ 3] = b;
        }
        put_w(&mut rdram, task + 0x1C, d.len() as u32);
    } else {
        put_w(&mut rdram, task + 0x1C, 0x0800);
    }
    // Nonzero source PCM ramp.
    for i in 0..0x100usize {
        rdram[src_pcm + i] = (i as u8).wrapping_add(1) | 0x40;
    }
    // A_LOADBUFF (20) then A_SAVEBUFF (21), 0x100 bytes through DMEM 0x0A0.
    let cmds: [u32; 4] = [
        (20u32 << 24) | ((0x100u32 >> 4) << 16) | 0x0A0,
        src_pcm as u32,
        (21u32 << 24) | ((0x100u32 >> 4) << 16) | 0x0A0,
        0x7000u32,
    ];
    for (i, &w) in cmds.iter().enumerate() {
        put_w(&mut rdram, cmd_list + i * 4, w);
    }
    put_w(&mut rdram, task + 0x18, ucode_data as u32); // ucode_data
    put_w(&mut rdram, task + 0x30, cmd_list as u32); // data_ptr
    put_w(&mut rdram, task + 0x34, (cmds.len() * 4) as u32); // data_size

    let mut m = RspMachine::new(&mut rdram);
    for i in 0..0x40usize {
        m.dmem.as_bytes_mut()[0xFC0 + i] = m.rdram[task + i];
    }
    // DMA the ucode_data image (0xF80 bytes) into DMEM 0x0000.
    for i in 0..0xF80usize {
        m.dmem.as_bytes_mut()[i] = m.rdram.get(ucode_data + i).copied().unwrap_or(0);
    }

    let mut pc = base;
    let mut steps = 0u64;
    let mut visit: BTreeMap<u32, u64> = BTreeMap::new();
    let trace_last = 60usize;
    let mut recent: Vec<(u32, String)> = Vec::new();

    let reason = loop {
        steps += 1;
        *visit.entry(pc).or_default() += 1;
        if steps > cap {
            break RspExitReason::UnhandledResumeTarget; // sentinel = "hit cap"
        }
        let idx = ((pc.wrapping_sub(base)) / 4) as usize;
        if idx >= n {
            println!("UNMAPPED jump target pc=0x{pc:04X} (idx {idx} >= n {n})");
            break RspExitReason::UnhandledJumpTarget;
        }
        let instr = decode(words[idx], pc);
        // Optional per-command-fetch / dispatch trace (RSP_TRACE_DISPATCH=1):
        // aspMain's command loop fetches at 0x1058 and dispatches via `jr r2`
        // at 0x1080; logging r2 there shows whether the jump-table lookup lands
        // on a valid handler (the DMEM-swizzle / base bugs both show up here).
        if std::env::var_os("RSP_TRACE_DISPATCH").is_some() {
            if pc == 0x1058 {
                println!(
                    "FETCH sp=0x{:03X} cmd_w0=0x{:08X} count(r30)={}",
                    m.reg(29),
                    m.load_w(m.reg(29)),
                    m.reg(30) as i32
                );
            }
            if pc == 0x1080 {
                println!("DISPATCH jr r2 -> 0x{:04X}", 0x1000 | (m.reg(2) & 0x0FFF));
            }
        }
        let delay = if idx + 1 < n {
            Some(decode(words[idx + 1], pc + 4))
        } else {
            None
        };
        recent.push((pc, format!("{instr:?}")));
        if recent.len() > trace_last {
            recent.remove(0);
        }
        match instr {
            Instr::Break => {
                m.break_rsp();
                break RspExitReason::Broke;
            }
            Instr::Nop => pc += 4,
            Instr::Lui { rt, imm } => {
                m.set_reg(rt, (imm as u32) << 16);
                pc += 4;
            }
            Instr::AluImm { op, rt, rs, imm } => {
                exec_alu_imm(&mut m, op, rt, rs, imm);
                pc += 4;
            }
            Instr::AluReg { op, rd, rs, rt } => {
                exec_alu_reg(&mut m, op, rd, rs, rt);
                pc += 4;
            }
            Instr::Shift { op, rd, rt, sa } => {
                exec_shift(&mut m, op, rd, rt, sa as u32, None);
                pc += 4;
            }
            Instr::ShiftVar { op, rd, rt, rs } => {
                exec_shift(&mut m, op, rd, rt, 0, Some(rs));
                pc += 4;
            }
            Instr::Load {
                op,
                rt,
                base: b,
                off,
            } => {
                exec_load(&mut m, op, rt, b, off);
                pc += 4;
            }
            Instr::Store {
                op,
                rt,
                base: b,
                off,
            } => {
                exec_store(&mut m, op, rt, b, off);
                pc += 4;
            }
            Instr::Mfc0 { rt, cop0 } => {
                let value = m.read_cp0(cop0);
                m.set_reg(rt, value);
                pc += 4;
            }
            Instr::Mtc0 { rt, cop0 } => {
                if let Some(reason) = exec_mtc0(&mut m, rt, cop0) {
                    m.ctx.resume_address = pc + 4;
                    break reason;
                }
                pc += 4;
            }
            Instr::Mfc2 { rt, vs, elem } => {
                let v = m.mfc2(vs, elem);
                m.set_reg(rt, v);
                pc += 4;
            }
            Instr::Mtc2 { rt, vs, elem } => {
                let v = m.reg(rt);
                m.mtc2(vs, elem, v);
                pc += 4;
            }
            Instr::Cfc2 { rt, cd } => {
                let v = m.cfc2(cd);
                m.set_reg(rt, v);
                pc += 4;
            }
            Instr::Ctc2 { rt, cd } => {
                let v = m.reg(rt);
                m.ctc2(cd, v);
                pc += 4;
            }
            Instr::VLoad {
                op,
                vt,
                elem,
                base: b,
                off,
            } => {
                let bv = m.reg(b);
                m.vload(op, vt, elem, bv, off);
                pc += 4;
            }
            Instr::VStore {
                op,
                vt,
                elem,
                base: b,
                off,
            } => {
                let bv = m.reg(b);
                m.vstore(op, vt, elem, bv, off);
                pc += 4;
            }
            Instr::Vu {
                op,
                vd,
                vs,
                vt,
                e,
                de,
            } => {
                let inv = OpInvocation {
                    vd: vd as usize,
                    vs: vs as usize,
                    vt: vt as usize,
                    e: e as usize,
                    de: de as usize,
                    vs_index: vs as usize,
                };
                match dispatch(m.vu(), op, inv) {
                    OpStatus::Executed => {}
                    OpStatus::Unimplemented(o) => {
                        println!("TRAP unimplemented VU {o:?} at 0x{pc:04X}");
                        break RspExitReason::Unsupported;
                    }
                }
                pc += 4;
            }
            Instr::Branch { op, rs, rt, target } => {
                let taken = match op {
                    BranchOp::Beq => m.reg(rs) == m.reg(rt),
                    BranchOp::Bne => m.reg(rs) != m.reg(rt),
                };
                let next_pc = if taken { target as u32 } else { pc + 8 };
                if let Some(reason) = run_delay(&mut m, delay, pc + 4, next_pc) {
                    break reason;
                }
                pc = next_pc;
            }
            Instr::BranchZ {
                op,
                rs,
                target,
                link,
            } => {
                let taken = match op {
                    BranchZOp::Blez => (m.reg(rs) as i32) <= 0,
                    BranchZOp::Bgtz => (m.reg(rs) as i32) > 0,
                    BranchZOp::Bltz => (m.reg(rs) as i32) < 0,
                    BranchZOp::Bgez => (m.reg(rs) as i32) >= 0,
                };
                if let Some(ret) = link {
                    m.set_reg(31, ret as u32);
                }
                let next_pc = if taken { target as u32 } else { pc + 8 };
                if let Some(reason) = run_delay(&mut m, delay, pc + 4, next_pc) {
                    break reason;
                }
                pc = next_pc;
            }
            Instr::Jump { target } => {
                let next_pc = target as u32;
                if let Some(reason) = run_delay(&mut m, delay, pc + 4, next_pc) {
                    break reason;
                }
                pc = next_pc;
            }
            Instr::Jal { target, ret } => {
                m.set_reg(31, ret as u32);
                let next_pc = target as u32;
                if let Some(reason) = run_delay(&mut m, delay, pc + 4, next_pc) {
                    break reason;
                }
                pc = next_pc;
            }
            Instr::Jr { rs } => {
                let jt = 0x1000 | (m.reg(rs) & 0x0FFF);
                if let Some(reason) = run_delay(&mut m, delay, pc + 4, jt) {
                    break reason;
                }
                pc = jt;
            }
            Instr::Jalr { rd, rs, ret } => {
                let jt = 0x1000 | (m.reg(rs) & 0x0FFF);
                m.set_reg(rd, ret as u32);
                if let Some(reason) = run_delay(&mut m, delay, pc + 4, jt) {
                    break reason;
                }
                pc = jt;
            }
            Instr::Unknown { word } => {
                println!("TRAP unknown 0x{word:08X} at 0x{pc:04X}");
                break RspExitReason::Unsupported;
            }
        }
    };

    println!("exit={reason:?} steps={steps}");
    println!("--- last {} instrs ---", recent.len());
    for (p, d) in &recent {
        println!("  0x{p:04X}: {d}");
    }
    // Hot pcs (loop body).
    let mut hot: Vec<_> = visit.iter().filter(|(_, c)| **c > 5).collect();
    hot.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    println!("--- hottest pcs ---");
    for (p, c) in hot.iter().take(20) {
        println!("  0x{p:04X}: {c} visits");
    }
}

fn run_delay(
    m: &mut RspMachine,
    delay: Option<Instr>,
    delay_pc: u32,
    resume_pc: u32,
) -> Option<RspExitReason> {
    match delay {
        None | Some(Instr::Nop) => None,
        Some(Instr::AluImm { op, rt, rs, imm }) => {
            exec_alu_imm(m, op, rt, rs, imm);
            None
        }
        Some(Instr::AluReg { op, rd, rs, rt }) => {
            exec_alu_reg(m, op, rd, rs, rt);
            None
        }
        Some(Instr::Shift { op, rd, rt, sa }) => {
            exec_shift(m, op, rd, rt, sa as u32, None);
            None
        }
        Some(Instr::ShiftVar { op, rd, rt, rs }) => {
            exec_shift(m, op, rd, rt, 0, Some(rs));
            None
        }
        Some(Instr::Lui { rt, imm }) => {
            m.set_reg(rt, (imm as u32) << 16);
            None
        }
        // A COP0 status read in a delay slot (the DMA-busy wait loops do exactly
        // this: `bnez a0,.. ; mfc0 a0,SP_DMA_BUSY`) — model as 0 like the emitter.
        Some(Instr::Mfc0 { rt, cop0 }) => {
            let value = m.read_cp0(cop0);
            m.set_reg(rt, value);
            None
        }
        Some(Instr::Load { op, rt, base, off }) => {
            exec_load(m, op, rt, base, off);
            None
        }
        Some(Instr::Store { op, rt, base, off }) => {
            exec_store(m, op, rt, base, off);
            None
        }
        Some(Instr::Mtc0 { rt, cop0 }) => {
            let reason = exec_mtc0(m, rt, cop0);
            if reason.is_some() {
                m.ctx.resume_address = resume_pc;
            }
            reason
        }
        Some(Instr::Mfc2 { rt, vs, elem }) => {
            let value = m.mfc2(vs, elem);
            m.set_reg(rt, value);
            None
        }
        Some(Instr::Mtc2 { rt, vs, elem }) => {
            let v = m.reg(rt);
            m.mtc2(vs, elem, v);
            None
        }
        Some(Instr::Cfc2 { rt, cd }) => {
            let value = m.cfc2(cd);
            m.set_reg(rt, value);
            None
        }
        Some(Instr::Ctc2 { rt, cd }) => {
            let value = m.reg(rt);
            m.ctc2(cd, value);
            None
        }
        Some(Instr::VLoad {
            op,
            vt,
            elem,
            base,
            off,
        }) => {
            let bv = m.reg(base);
            m.vload(op, vt, elem, bv, off);
            None
        }
        Some(Instr::VStore {
            op,
            vt,
            elem,
            base,
            off,
        }) => {
            let bv = m.reg(base);
            m.vstore(op, vt, elem, bv, off);
            None
        }
        Some(Instr::Vu {
            op,
            vd,
            vs,
            vt,
            e,
            de,
        }) => {
            let invocation = OpInvocation {
                vd: vd as usize,
                vs: vs as usize,
                vt: vt as usize,
                e: e as usize,
                de: de as usize,
                vs_index: vs as usize,
            };
            match dispatch(m.vu(), op, invocation) {
                OpStatus::Executed => None,
                OpStatus::Unimplemented(op) => {
                    println!("TRAP unimplemented VU {op:?} at 0x{delay_pc:04X}");
                    Some(RspExitReason::Unsupported)
                }
            }
        }
        Some(Instr::Break) => {
            m.break_rsp();
            Some(RspExitReason::Broke)
        }
        Some(Instr::Unknown { word }) => {
            println!("TRAP unknown 0x{word:08X} in delay slot at 0x{delay_pc:04X}");
            Some(RspExitReason::Unsupported)
        }
        Some(
            illegal @ (Instr::Branch { .. }
            | Instr::BranchZ { .. }
            | Instr::Jump { .. }
            | Instr::Jal { .. }
            | Instr::Jr { .. }
            | Instr::Jalr { .. }),
        ) => panic!(
            "unsupported RSP control transfer {illegal:?} in delay slot at IMEM 0x{delay_pc:04X}"
        ),
    }
}

fn exec_alu_imm(
    m: &mut RspMachine,
    op: fn64_audio::rsp::decode::AluImmOp,
    rt: u8,
    rs: u8,
    imm: u16,
) {
    use fn64_audio::rsp::decode::AluImmOp::*;
    let simm = imm as i16 as i32 as u32;
    let v = match op {
        Addi | Addiu => m.reg(rs).wrapping_add(simm),
        Andi => m.reg(rs) & imm as u32,
        Ori => m.reg(rs) | imm as u32,
        Xori => m.reg(rs) ^ imm as u32,
        Slti => ((m.reg(rs) as i32) < (imm as i16 as i32)) as u32,
        Sltiu => (m.reg(rs) < simm) as u32,
    };
    m.set_reg(rt, v);
}

fn exec_alu_reg(m: &mut RspMachine, op: fn64_audio::rsp::decode::AluRegOp, rd: u8, rs: u8, rt: u8) {
    use fn64_audio::rsp::decode::AluRegOp::*;
    let v = match op {
        Add | Addu => m.reg(rs).wrapping_add(m.reg(rt)),
        Sub | Subu => m.reg(rs).wrapping_sub(m.reg(rt)),
        And => m.reg(rs) & m.reg(rt),
        Or => m.reg(rs) | m.reg(rt),
        Xor => m.reg(rs) ^ m.reg(rt),
        Nor => !(m.reg(rs) | m.reg(rt)),
        Slt => ((m.reg(rs) as i32) < (m.reg(rt) as i32)) as u32,
        Sltu => (m.reg(rs) < m.reg(rt)) as u32,
    };
    m.set_reg(rd, v);
}

fn exec_shift(
    m: &mut RspMachine,
    op: fn64_audio::rsp::decode::ShiftOp,
    rd: u8,
    rt: u8,
    sa: u32,
    rs: Option<u8>,
) {
    use fn64_audio::rsp::decode::ShiftOp::*;
    let amt = match rs {
        Some(r) => m.reg(r) & 31,
        None => sa,
    };
    let v = match op {
        Sll => m.reg(rt) << amt,
        Srl => m.reg(rt) >> amt,
        Sra => ((m.reg(rt) as i32) >> amt) as u32,
    };
    m.set_reg(rd, v);
}

fn exec_load(m: &mut RspMachine, op: fn64_audio::rsp::decode::LoadOp, rt: u8, base: u8, off: i16) {
    use fn64_audio::rsp::decode::LoadOp::*;
    let a = m.reg(base).wrapping_add(off as i32 as u32);
    let v = match op {
        Lb => m.load_b(a),
        Lbu => m.load_bu(a),
        Lh => m.load_h(a),
        Lhu => m.load_hu(a),
        Lw => m.load_w(a),
    };
    m.set_reg(rt, v);
}

fn exec_store(
    m: &mut RspMachine,
    op: fn64_audio::rsp::decode::StoreOp,
    rt: u8,
    base: u8,
    off: i16,
) {
    use fn64_audio::rsp::decode::StoreOp::*;
    let a = m.reg(base).wrapping_add(off as i32 as u32);
    let v = m.reg(rt);
    match op {
        Sb => m.store_b(a, v),
        Sh => m.store_h(a, v),
        Sw => m.store_w(a, v),
    }
}

fn exec_mtc0(m: &mut RspMachine, rt: u8, cop0: u8) -> Option<RspExitReason> {
    let v = m.reg(rt);
    m.write_cp0(cop0, v)
}
