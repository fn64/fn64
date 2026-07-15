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
    let base: u32 = 0x1080;
    let cap: u64 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(2000);
    let words = read_words(path);
    let n = words.len();

    let mut rdram = vec![0u8; 0x0080_0000];
    // Seed a minimal but well-formed-ish task at 0x1000.
    let task = 0x1000usize;
    rdram[task + 0x18..task + 0x1C].copy_from_slice(&0x2000u32.to_be_bytes()); // ucode_data
    rdram[task + 0x1C..task + 0x20].copy_from_slice(&0x0800u32.to_be_bytes());
    rdram[task + 0x30..task + 0x34].copy_from_slice(&0x4000u32.to_be_bytes()); // data_ptr
    rdram[task + 0x34..task + 0x38].copy_from_slice(&0x0000u32.to_be_bytes()); // data_size=0

    // ucode_data image lives at rdram 0x2000 in this synthetic setup.
    let mut m = RspMachine::new(&mut rdram);
    for i in 0..0x40usize {
        m.dmem.as_bytes_mut()[0xFC0 + i] = m.rdram[task + i];
    }
    // DMA the ucode_data image (0xF80 bytes) into DMEM 0x0000.
    let ucode_data = 0x2000usize;
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
            break RspExitReason::UnhandledJumpTarget;
        }
        let instr = decode(words[idx], pc);
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
            Instr::Break => break RspExitReason::Broke,
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
            Instr::CondMove {
                on_zero,
                rd,
                rs,
                rt,
            } => {
                let c = if on_zero {
                    m.reg(rt) == 0
                } else {
                    m.reg(rt) != 0
                };
                if c {
                    let v = m.reg(rs);
                    m.set_reg(rd, v);
                }
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
            Instr::Mfc0 { rt, .. } => {
                m.set_reg(rt, 0);
                pc += 4;
            }
            Instr::Mtc0 { rt, cop0 } => {
                exec_mtc0(&mut m, rt, cop0);
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
                run_delay(&mut m, delay);
                pc = if taken { target as u32 } else { pc + 8 };
            }
            Instr::BranchZ { op, rs, target } => {
                let taken = match op {
                    BranchZOp::Blez => (m.reg(rs) as i32) <= 0,
                    BranchZOp::Bgtz => (m.reg(rs) as i32) > 0,
                    BranchZOp::Bltz => (m.reg(rs) as i32) < 0,
                    BranchZOp::Bgez => (m.reg(rs) as i32) >= 0,
                };
                run_delay(&mut m, delay);
                pc = if taken { target as u32 } else { pc + 8 };
            }
            Instr::Jump { target } => {
                run_delay(&mut m, delay);
                pc = target as u32;
            }
            Instr::Jal { target, ret } => {
                m.set_reg(31, ret as u32);
                run_delay(&mut m, delay);
                pc = target as u32;
            }
            Instr::Jr { rs } => {
                let jt = m.reg(rs) & 0x1FFF;
                run_delay(&mut m, delay);
                pc = jt;
            }
            Instr::Jalr { rd, rs, ret } => {
                let jt = m.reg(rs) & 0x1FFF;
                m.set_reg(rd, ret as u32);
                run_delay(&mut m, delay);
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

fn run_delay(m: &mut RspMachine, delay: Option<Instr>) {
    match delay {
        Some(Instr::AluImm { op, rt, rs, imm }) => exec_alu_imm(m, op, rt, rs, imm),
        Some(Instr::AluReg { op, rd, rs, rt }) => exec_alu_reg(m, op, rd, rs, rt),
        Some(Instr::Shift { op, rd, rt, sa }) => exec_shift(m, op, rd, rt, sa as u32, None),
        Some(Instr::ShiftVar { op, rd, rt, rs }) => exec_shift(m, op, rd, rt, 0, Some(rs)),
        Some(Instr::Lui { rt, imm }) => m.set_reg(rt, (imm as u32) << 16),
        Some(Instr::Load { op, rt, base, off }) => exec_load(m, op, rt, base, off),
        Some(Instr::Store { op, rt, base, off }) => exec_store(m, op, rt, base, off),
        Some(Instr::Mtc0 { rt, cop0 }) => exec_mtc0(m, rt, cop0),
        Some(Instr::Mtc2 { rt, vs, elem }) => {
            let v = m.reg(rt);
            m.mtc2(vs, elem, v);
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
        }
        _ => {}
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

fn exec_mtc0(m: &mut RspMachine, rt: u8, cop0: u8) {
    let v = m.reg(rt);
    match cop0 {
        0 => m.set_dma_mem(v),
        1 => m.set_dma_dram(v),
        2 => {
            let _ = m.dma_read(v);
        }
        3 => m.dma_write(v),
        _ => {}
    }
}
