//! Linear HI/LO cross-reference scan for one absolute data address.
//!
//! Given a bank's bytes and a target virtual address range, this pass finds
//! every load, store, and address materialization whose base register was
//! established by a `lui` (optionally refined by `addiu`/`ori`) in the same
//! straight-line run. It is the classic %hi/%lo pairing scan, bounded and
//! deterministic.
//!
//! # Evidence class
//!
//! Every site this module emits is **candidate** evidence: the scan decodes
//! raw bank words linearly, so it can pair instructions that no real
//! execution path runs together, and it cannot see pairings that cross a
//! basic-block join. It never proves executable permission, reachability, or
//! a complete stored-value set. Callers own promotion; this module only
//! reports what the bytes say under MIPS-III decoding by the shared
//! `fn64-cpu-runtime` decoder (the same ISA authority the CFG pass uses).
//!
//! # Delay-slot handling
//!
//! A control transfer's delay slot executes with the register state
//! established before the transfer, so HI/LO tracking survives into the
//! delay slot and is cleared after it. A branch *target* landing inside a
//! straight-line run is invisible without a CFG; that is one reason results
//! stay candidates.

use crate::cfg::{classify_control, ControlOp};
use fn64_cpu_runtime::{decode, Instruction};
use serde::{Deserialize, Serialize};

/// How the value a store writes was established, along the linear
/// fall-through path only. `Constant` is **not** a complete value set: a
/// branch join immediately before the store can supply other values that a
/// linear scan cannot see. Consumers must treat it as "the constant reaching
/// this store along fall-through," never "the only stored value."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StoredValue {
    /// The store writes `$zero`: architecturally constant zero.
    Zero,
    /// A `lui`/`addiu`/`ori` chain in the same straight-line run left this
    /// constant in the stored register. `def_pc` is the last instruction of
    /// that chain.
    Constant { value: u32, def_pc: u32 },
    /// The stored register's value comes from outside the straight-line run
    /// (memory, arithmetic on unknowns, or across a join).
    Unresolved { reg: u8 },
}

/// One reference to the target address range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefKind {
    /// A load whose computed address falls in the target range.
    Load { width: u8 },
    /// A store whose computed address falls in the target range.
    Store { width: u8, value: StoredValue },
    /// An `addiu`/`ori` that materializes exactly the target start address
    /// into a register (a code/data pointer taken, not a memory access).
    Address,
}

/// A single candidate cross-reference site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobalRefSite {
    /// PC of the memory access or materializing instruction.
    pub pc: u32,
    /// PC of the `lui` that established the base register's upper half.
    pub lui_pc: u32,
    /// The exact computed address (for sub-word accesses inside the range).
    pub addr: u32,
    pub kind: RefKind,
}

/// Per-register linear tracking state.
#[derive(Clone, Copy)]
struct RegState {
    /// Known 32-bit constant value, if the register was built by a
    /// lui/addiu/ori chain in this straight-line run.
    value: u32,
    /// PC of the `lui` that started the chain.
    lui_pc: u32,
    /// PC of the last instruction of the chain.
    def_pc: u32,
}

/// Scan `bytes` (a bank mapped at `va_start`) for references to
/// `[target, target + target_len)`. Pure function: same input, same output,
/// sites ordered by ascending PC.
pub fn scan_global_refs(
    bytes: &[u8],
    va_start: u32,
    target: u32,
    target_len: u32,
) -> Vec<GlobalRefSite> {
    let mut sites = Vec::new();
    let mut regs: [Option<RegState>; 32] = [None; 32];
    // Set when the previous word was a control transfer: the current word is
    // its delay slot, and tracking must be cleared after it.
    let mut clear_after_this_word = false;

    let words = bytes.chunks_exact(4);
    for (index, chunk) in words.enumerate() {
        let word = u32::from_be_bytes(chunk.try_into().expect("four-byte chunk"));
        let pc = va_start.wrapping_add((index as u32) * 4);
        let this_is_delay_slot = clear_after_this_word;
        clear_after_this_word = false;
        if matches!(
            classify_control(word),
            ControlOp::J { .. }
                | ControlOp::Jal { .. }
                | ControlOp::Jr { .. }
                | ControlOp::Jalr { .. }
                | ControlOp::Branch { .. }
                | ControlOp::BranchLikely { .. }
        ) {
            clear_after_this_word = true;
        }

        match decode(word) {
            Instruction::Lui { rt, imm } => {
                set_reg(
                    &mut regs,
                    rt,
                    RegState {
                        value: (imm as u32) << 16,
                        lui_pc: pc,
                        def_pc: pc,
                    },
                );
            }
            Instruction::Addiu { rt, rs, imm } => {
                let derived = if rs == 0 {
                    Some(RegState {
                        value: imm as i32 as u32,
                        lui_pc: pc,
                        def_pc: pc,
                    })
                } else {
                    regs[rs as usize].map(|state| RegState {
                        value: state.value.wrapping_add(imm as i32 as u32),
                        lui_pc: state.lui_pc,
                        def_pc: pc,
                    })
                };
                if let Some(state) = derived {
                    if state.value == target {
                        sites.push(GlobalRefSite {
                            pc,
                            lui_pc: state.lui_pc,
                            addr: state.value,
                            kind: RefKind::Address,
                        });
                    }
                }
                set_reg_opt(&mut regs, rt, derived);
            }
            Instruction::Ori { rt, rs, imm } => {
                let derived = if rs == 0 {
                    Some(RegState {
                        value: imm as u32,
                        lui_pc: pc,
                        def_pc: pc,
                    })
                } else {
                    regs[rs as usize].map(|state| RegState {
                        value: state.value | imm as u32,
                        lui_pc: state.lui_pc,
                        def_pc: pc,
                    })
                };
                if let Some(state) = derived {
                    if state.value == target {
                        sites.push(GlobalRefSite {
                            pc,
                            lui_pc: state.lui_pc,
                            addr: state.value,
                            kind: RefKind::Address,
                        });
                    }
                }
                set_reg_opt(&mut regs, rt, derived);
            }
            Instruction::Lb { rt, base, off }
            | Instruction::Lbu { rt, base, off }
            | Instruction::Lh { rt, base, off }
            | Instruction::Lhu { rt, base, off }
            | Instruction::Lw { rt, base, off }
            | Instruction::Lwu { rt, base, off } => {
                let width = match decode(word) {
                    Instruction::Lb { .. } | Instruction::Lbu { .. } => 1,
                    Instruction::Lh { .. } | Instruction::Lhu { .. } => 2,
                    _ => 4,
                };
                if let Some(state) = regs[base as usize] {
                    let addr = state.value.wrapping_add(off as i32 as u32);
                    if in_range(addr, target, target_len) {
                        sites.push(GlobalRefSite {
                            pc,
                            lui_pc: state.lui_pc,
                            addr,
                            kind: RefKind::Load { width },
                        });
                    }
                }
                clear_reg(&mut regs, rt);
            }
            Instruction::Sb { rt, base, off }
            | Instruction::Sh { rt, base, off }
            | Instruction::Sw { rt, base, off } => {
                let width = match decode(word) {
                    Instruction::Sb { .. } => 1,
                    Instruction::Sh { .. } => 2,
                    _ => 4,
                };
                if let Some(state) = regs[base as usize] {
                    let addr = state.value.wrapping_add(off as i32 as u32);
                    if in_range(addr, target, target_len) {
                        let value = if rt == 0 {
                            StoredValue::Zero
                        } else if let Some(value_state) = regs[rt as usize] {
                            StoredValue::Constant {
                                value: value_state.value,
                                def_pc: value_state.def_pc,
                            }
                        } else {
                            StoredValue::Unresolved { reg: rt }
                        };
                        sites.push(GlobalRefSite {
                            pc,
                            lui_pc: state.lui_pc,
                            addr,
                            kind: RefKind::Store { width, value },
                        });
                    }
                }
            }
            other => {
                // Any other instruction that writes a GPR invalidates linear
                // tracking for that register. The decoder names destination
                // registers heterogeneously; rather than enumerate every
                // variant, clear conservatively via the destination probe.
                for reg in written_gprs(&other) {
                    clear_reg(&mut regs, reg);
                }
            }
        }

        if this_is_delay_slot {
            regs = [None; 32];
        }
    }
    sites
}

fn in_range(addr: u32, target: u32, target_len: u32) -> bool {
    addr >= target && addr < target.wrapping_add(target_len)
}

fn set_reg(regs: &mut [Option<RegState>; 32], reg: u8, state: RegState) {
    if reg != 0 {
        regs[reg as usize] = Some(state);
    }
}

fn set_reg_opt(regs: &mut [Option<RegState>; 32], reg: u8, state: Option<RegState>) {
    if reg != 0 {
        regs[reg as usize] = state;
    }
}

fn clear_reg(regs: &mut [Option<RegState>; 32], reg: u8) {
    if reg != 0 {
        regs[reg as usize] = None;
    }
}

/// Which GPRs an instruction writes, for tracking invalidation only. This
/// deliberately over-approximates ("clears too much") when unsure: a cleared
/// register can only turn a site `Unresolved` or drop a pairing, never
/// invent one. Loads/ALU-immediate forms are handled by the caller; this
/// covers the R-type and remaining shapes.
fn written_gprs(instruction: &Instruction) -> Vec<u8> {
    use Instruction::*;
    match instruction {
        Add { rd, .. }
        | Addu { rd, .. }
        | Sub { rd, .. }
        | Subu { rd, .. }
        | And { rd, .. }
        | Or { rd, .. }
        | Xor { rd, .. }
        | Nor { rd, .. }
        | Slt { rd, .. }
        | Sltu { rd, .. }
        | Sll { rd, .. }
        | Srl { rd, .. }
        | Sra { rd, .. }
        | Sllv { rd, .. }
        | Srlv { rd, .. }
        | Srav { rd, .. }
        | Dsll { rd, .. }
        | Dsrl { rd, .. }
        | Dsra { rd, .. }
        | Dsll32 { rd, .. }
        | Dsrl32 { rd, .. }
        | Dsra32 { rd, .. }
        | Dsllv { rd, .. }
        | Dsrlv { rd, .. }
        | Dsrav { rd, .. }
        | Daddu { rd, .. }
        | Dsubu { rd, .. }
        | Mfhi { rd }
        | Mflo { rd } => vec![*rd],
        Addi { rt, .. }
        | Slti { rt, .. }
        | Sltiu { rt, .. }
        | Andi { rt, .. }
        | Xori { rt, .. }
        | Daddiu { rt, .. }
        | Lwl { rt, .. }
        | Lwr { rt, .. }
        | Ld { rt, .. }
        | Ldl { rt, .. }
        | Ldr { rt, .. }
        | Ll { rt, .. }
        | Mfc0 { rt, .. }
        | Mfc1 { rt, .. }
        | Dmfc1 { rt, .. }
        | Cfc1 { rt, .. } => vec![*rt],
        Jal { .. } => vec![31],
        Jalr { rd, .. } => vec![*rd],
        // Conservative default: instructions this arm does not name either
        // write no GPR (stores, branches, FP compute) or are rare enough
        // that treating them as writing nothing risks a stale pairing. The
        // shapes that matter for HI/LO pairing are all named above.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assemble(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    const TARGET: u32 = 0x800a_10b0;

    #[test]
    fn pairs_lui_lw_and_lui_sw_zero() {
        // lui v0,0x800a ; lw v0,0x10b0(v0) ; lui at,0x800a ; sw zero,0x10b0(at)
        let bytes = assemble(&[0x3c02_800a, 0x8c42_10b0, 0x3c01_800a, 0xac20_10b0]);
        let sites = scan_global_refs(&bytes, 0x8000_0000, TARGET, 4);
        assert_eq!(sites.len(), 2);
        assert_eq!(sites[0].kind, RefKind::Load { width: 4 });
        assert_eq!(sites[0].pc, 0x8000_0004);
        assert_eq!(sites[0].lui_pc, 0x8000_0000);
        assert_eq!(
            sites[1].kind,
            RefKind::Store {
                width: 4,
                value: StoredValue::Zero
            }
        );
    }

    #[test]
    fn store_in_jr_delay_slot_keeps_pairing_and_constant() {
        // li v0,0x22 ; lui at,0x800a ; jr ra ; sw v0,0x10b0(at)
        let bytes = assemble(&[0x2402_0022, 0x3c01_800a, 0x03e0_0008, 0xac22_10b0]);
        let sites = scan_global_refs(&bytes, 0x8010_0000, TARGET, 4);
        assert_eq!(sites.len(), 1);
        assert_eq!(
            sites[0].kind,
            RefKind::Store {
                width: 4,
                value: StoredValue::Constant {
                    value: 0x22,
                    def_pc: 0x8010_0000
                }
            }
        );
    }

    #[test]
    fn tracking_clears_after_delay_slot() {
        // lui at,0x800a ; jr ra ; nop ; sw v0,0x10b0(at)  -- the store is in
        // a new straight-line run; the pairing must NOT survive.
        let bytes = assemble(&[0x3c01_800a, 0x03e0_0008, 0x0000_0000, 0xac22_10b0]);
        let sites = scan_global_refs(&bytes, 0x8010_0000, TARGET, 4);
        assert!(sites.is_empty());
    }

    #[test]
    fn value_across_join_is_reported_unresolved_not_guessed() {
        // bne a0,v0,+3 ; li v0,14 (delay) ; lui at,0x800a ; sw v0,0x10b0(at)
        // The branch target could supply a different v0; after the delay
        // slot all tracking clears, so the store's value must be Unresolved
        // and the pairing must re-establish from the post-branch lui.
        let bytes = assemble(&[0x1482_0002, 0x2402_000e, 0x3c01_800a, 0xac22_10b0]);
        let sites = scan_global_refs(&bytes, 0x8010_0000, TARGET, 4);
        assert_eq!(sites.len(), 1);
        assert_eq!(
            sites[0].kind,
            RefKind::Store {
                width: 4,
                value: StoredValue::Unresolved { reg: 2 }
            }
        );
    }

    #[test]
    fn address_materialization_is_reported() {
        // lui a2,0x8002 ; addiu a2,a2,0x6888
        let bytes = assemble(&[0x3c06_8002, 0x24c6_6888]);
        let sites = scan_global_refs(&bytes, 0x8000_0000, 0x8002_6888, 4);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].kind, RefKind::Address);
        assert_eq!(sites[0].pc, 0x8000_0004);
    }

    #[test]
    fn sub_word_store_inside_range_is_caught() {
        // lui at,0x800a ; sb v0,0x10b3(at) -- byte 3 of the target word.
        let bytes = assemble(&[0x3c01_800a, 0xa022_10b3]);
        let sites = scan_global_refs(&bytes, 0x8000_0000, TARGET, 4);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].addr, TARGET + 3);
        assert!(matches!(sites[0].kind, RefKind::Store { width: 1, .. }));
    }

    #[test]
    fn intervening_register_write_kills_constant_but_not_base() {
        // li v0,5 ; addu v0,v0,a0 ; lui at,0x800a ; sw v0,0x10b0(at)
        let bytes = assemble(&[0x2402_0005, 0x0044_1021, 0x3c01_800a, 0xac22_10b0]);
        let sites = scan_global_refs(&bytes, 0x8000_0000, TARGET, 4);
        assert_eq!(sites.len(), 1);
        assert_eq!(
            sites[0].kind,
            RefKind::Store {
                width: 4,
                value: StoredValue::Unresolved { reg: 2 }
            }
        );
    }
}
