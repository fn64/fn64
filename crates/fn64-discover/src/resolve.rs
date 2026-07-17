//! Phase 6 (docs/DISCOVER-DESIGN.md "resolve indirect transfers to a fixed
//! point"), the cheapest bounded case: mechanically resolve a `jr $rs` /
//! `jalr $rs` whose target register was constructed by a bounded HI/LO
//! (`lui` + `addiu`/`ori`) address-materialization sequence within the same
//! straight-line run reaching the transfer.
//!
//! # Why this one case first
//!
//! Each of the three independently graded N64 IPL3 boot stubs ends the same
//! way: it clears BSS, materializes
//! the resident C entrypoint's absolute address into a register with a
//! `lui`/`addiu` pair, and does `jr` to it. Concretely, in all three ROMs this
//! crate grades:
//!
//! ```text
//! OoT   0x80000420: lui   $t2, 0x8000     ; 3c0a8000
//!       0x80000428: addiu $t2, $t2, 0x0498 ; 254a0498  -> $t2 = 0x80000498
//!       0x8000042c: jr    $t2             ; 01400008  (bootproc)
//!
//! NW4E  0x80000428: lui   $t2, 0x8000     ; 3c0a8000
//!       0x8000042c: addiu $t2, $t2, 0x0460 ; 254a0460  -> $t2 = 0x80000460
//!       0x80000434: jr    $t2             ; 01400008  (main entry)
//!
//! NWXE  0x80000428: lui   $t2, 0x8000     ; 3c0a8000
//!       0x8000042c: addiu $t2, $t2, 0x0460 ; 254a0460  -> $t2 = 0x80000460
//!       0x80000434: jr    $t2             ; 01400008  (main entry)
//! ```
//!
//! Without resolving this one indirect edge, recursive-descent CFG discovery
//! dead-ends at the entrypoint stub (OoT: 1 owner / 0 direct calls). Feeding
//! the resolved target back as a root explodes the same bank into 46 owners
//! / 87 direct calls. It is the single highest-leverage indirect site in a
//! stripped N64 ROM, and it needs **no** game-specific input -- the resident
//! entry address that NW4E's `overlays.json` previously supplied by hand
//! (`main_entry_vram = 0x80000460`) falls straight out of the `lui`/`addiu`
//! pair.
//!
//! # Discipline
//!
//! This is a **bounded, exhaustive** resolution, not a heuristic guess: the
//! target is computed from a fully-known constant construction, and only
//! accepted when it lands inside a bank the caller vouches for
//! (`[va_start, va_end)`). A register whose value depends on a load, an
//! unknown input register, or any instruction this narrow tracker does not
//! model is left `Unknown`, and its `jr`/`jalr` stays an open indirect site
//! exactly as before -- never resolved to a fabricated address. The tracker
//! deliberately models only the handful of address-materialization opcodes
//! the bounded case needs; anything else clears the touched register rather
//! than pretend to know it.

use crate::cfg::{build_cfg, BlockTerminator, Cfg};
use std::collections::BTreeSet;

/// The subset of MIPS-III integer ops this bounded constant tracker models.
/// Every other opcode is treated as "clobbers its destination register with
/// an unknown value" (or, for stores/branches with no GPR destination, as a
/// no-op on the register file) -- the conservative choice that can never
/// invent a constant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConstOp {
    /// `lui $rt, imm16`: `$rt = imm16 << 16`.
    Lui { rt: u8, imm: u32 },
    /// `addiu $rt, $rs, imm16` / `addi`: `$rt = $rs + sign_extend(imm16)`.
    Addiu { rt: u8, rs: u8, imm: i32 },
    /// `ori $rt, $rs, imm16`: `$rt = $rs | imm16`.
    Ori { rt: u8, rs: u8, imm: u32 },
    /// `addu`/`or`/`daddu $rd, $rs, $zero` register move: `$rd = $rs`.
    /// Only the zero-second-operand move form is modeled (a genuine add of
    /// two live registers is not a single constant construction).
    Move { rd: u8, rs: u8 },
    /// An instruction that writes GPR `rd` with a value this tracker cannot
    /// derive -- the destination becomes `Unknown`.
    Clobber { rd: u8 },
    /// An instruction with no GPR destination (store, branch, nop, coprocessor
    /// op) -- leaves the register file unchanged.
    NoDest,
}

/// Decode one big-endian MIPS word into the constant-tracking op it performs.
/// This is intentionally separate from `cfg::classify_control`: that module
/// answers "does control leave here", this one answers "what constant does
/// this write". Kept minimal on purpose (see the module doc's discipline
/// note).
fn classify_const(word: u32) -> ConstOp {
    let opcode = (word >> 26) & 0x3f;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    let imm16 = word & 0xffff;

    match opcode {
        0x0f => ConstOp::Lui {
            rt,
            imm: imm16 << 16,
        },
        // addi / addiu / daddi / daddiu all sign-extend the immediate; for a
        // pointer construction they are equivalent here.
        0x08 | 0x09 | 0x18 | 0x19 => ConstOp::Addiu {
            rt,
            rs,
            imm: (imm16 as i16) as i32,
        },
        0x0d => ConstOp::Ori { rt, rs, imm: imm16 },
        // These immediate forms write `rt`, but this bounded tracker does
        // not model their value. Leaving a previous constant live would
        // fabricate an indirect target after a real clobber.
        0x0a | 0x0b | 0x0c | 0x0e => ConstOp::Clobber { rd: rt },
        0x00 => {
            let funct = word & 0x3f;
            match funct {
                // addu/add/or/daddu/dadd with rt == $zero is a register move.
                0x20 | 0x21 | 0x25 | 0x2c | 0x2d if rt == 0 => ConstOp::Move { rd, rs },
                // Any other SPECIAL op with a real rd clobbers it; jr/jalr/
                // syscall/break and shifts-of-zero don't concern us, but a
                // plain "write rd from something we don't track" must clear.
                0x08 | 0x0c | 0x0d => ConstOp::NoDest, // jr/syscall/break: no GPR const dest
                0x09 if rd != 0 => ConstOp::Clobber { rd }, // jalr writes its link register
                0x09 => ConstOp::NoDest,
                _ if rd != 0 => ConstOp::Clobber { rd },
                _ => ConstOp::NoDest,
            }
        }
        // Loads (opcode 0x20..=0x27, 0x37, ...) write rt with a memory value
        // we cannot constant-fold in this bounded case: clobber rt.
        0x20..=0x27 | 0x30 | 0x34 | 0x37 | 0x1a | 0x1b => ConstOp::Clobber { rd: rt },
        // `jal` and branch-and-link write `$ra`. The exact link value is
        // PC-dependent and deliberately outside this narrow tracker.
        0x03 => ConstOp::Clobber { rd: 31 },
        0x01 if matches!(rt, 0x10..=0x13) => ConstOp::Clobber { rd: 31 },
        // Move/control-from-coprocessor forms write `rt`; move/control-to and
        // ordinary coprocessor operations do not write a GPR.
        0x10..=0x13 if matches!(rs, 0x00..=0x02) => ConstOp::Clobber { rd: rt },
        // Store-conditional writes success/failure back to `rt`.
        0x38 | 0x3c => ConstOp::Clobber { rd: rt },
        // Stores, ordinary branches, `j`, coprocessor: no GPR destination.
        _ => ConstOp::NoDest,
    }
}

/// One mechanically-resolved indirect edge: the `jr`/`jalr` site and the
/// bounded-exhaustive target its address construction proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedTarget {
    /// PC of the `jr`/`jalr` instruction.
    pub site_pc: u32,
    /// The resolved absolute target VA.
    pub target: u32,
    /// True when the site was `jalr` (a call: fallthrough returns), false for
    /// `jr` (a tail transfer / computed jump).
    pub via_call: bool,
    /// First instruction included in the bounded straight-line constant
    /// construction. This is provenance for Phase 3's resolved-jalr claim.
    pub construction_start: u32,
}

/// Track register constants forward across a single straight-line block and,
/// if that block ends in a `jr`/`jalr` whose register holds a known constant,
/// return the resolved target. `None` when the terminating register's value
/// is not a proven constant.
///
/// `words` is the block's instruction words in order (including the delay
/// slot); `via_call` says whether the terminator was `jalr`. `jr_rs` is the
/// register the transfer reads. The delay-slot instruction is included in
/// `words` and its effect on the register file is applied, matching hardware
/// (the delay slot executes before the transfer takes effect).
fn resolve_block_target(
    words: &[(u32, u32)],
    site_pc: u32,
    jr_rs: u8,
    via_call: bool,
) -> Option<ResolvedTarget> {
    // reg[i] = Some(value) when GPR i holds a known constant, None otherwise.
    let mut reg: [Option<u32>; 32] = [None; 32];
    reg[0] = Some(0); // $zero is always 0.

    for &(_pc, word) in words {
        match classify_const(word) {
            ConstOp::Lui { rt, imm } => set(&mut reg, rt, Some(imm)),
            ConstOp::Addiu { rt, rs, imm } => {
                let v = reg[rs as usize].map(|b| (b as i64 + imm as i64) as u32);
                set(&mut reg, rt, v);
            }
            ConstOp::Ori { rt, rs, imm } => {
                let v = reg[rs as usize].map(|b| b | imm);
                set(&mut reg, rt, v);
            }
            ConstOp::Move { rd, rs } => {
                let v = reg[rs as usize];
                set(&mut reg, rd, v);
            }
            ConstOp::Clobber { rd } => set(&mut reg, rd, None),
            ConstOp::NoDest => {}
        }
    }

    let target = reg[jr_rs as usize]?;
    Some(ResolvedTarget {
        site_pc,
        target,
        via_call,
        construction_start: words.first().map_or(site_pc, |(pc, _)| *pc),
    })
}

/// True when `word` ends a straight-line constant-propagation region. The
/// instruction after its delay slot starts with an unknown register file;
/// carrying constants across either arm of a branch or across a call would
/// turn a bounded proof into a guess.
fn ends_linear_region(word: u32) -> bool {
    let opcode = (word >> 26) & 0x3f;
    match opcode {
        0x00 => matches!(word & 0x3f, 0x08 | 0x09 | 0x0c | 0x0d),
        0x01..=0x07 | 0x14..=0x17 => true,
        0x11 => ((word >> 21) & 0x1f) == 0x08,
        _ => false,
    }
}

/// Linearly scan a discovered load image for `jalr` sites whose source
/// register is resolved by the same bounded HI/LO tracker used by CFG
/// closure. Results are word-aligned; the Phase 3 bank model decides which
/// discovered load image, if any, owns each target. Unknown/clobbered
/// registers remain absent rather than producing a guess.
pub fn resolve_linear_jalr_sites(bank_bytes: &[u8], va_start: u32) -> Vec<ResolvedTarget> {
    let words: Vec<u32> = bank_bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap()))
        .collect();
    let mut region_start = 0usize;
    let mut next_region_start = None;
    let mut out = Vec::new();

    for (index, &word) in words.iter().enumerate() {
        if next_region_start == Some(index) {
            region_start = index;
            next_region_start = None;
        }

        if (word >> 26) == 0 && (word & 0x3f) == 0x09 {
            let rs = ((word >> 21) & 0x1f) as u8;
            let site_pc = va_start.wrapping_add((index * 4) as u32);
            let construction: Vec<(u32, u32)> = words[region_start..index]
                .iter()
                .enumerate()
                .map(|(relative, &instruction)| {
                    (
                        va_start.wrapping_add(((region_start + relative) * 4) as u32),
                        instruction,
                    )
                })
                .collect();
            if let Some(resolved) = resolve_block_target(&construction, site_pc, rs, true) {
                if resolved.target.is_multiple_of(4) {
                    out.push(resolved);
                }
            }
        }

        if ends_linear_region(word) {
            next_region_start = Some(index.saturating_add(2));
        }
    }

    out.sort_by_key(|target| target.site_pc);
    out.dedup();
    out
}

/// Set GPR `i`, keeping `$zero` pinned to 0 (a write to `$zero` is discarded
/// by hardware).
fn set(reg: &mut [Option<u32>; 32], i: u8, v: Option<u32>) {
    if i != 0 {
        reg[i as usize] = v;
    }
}

/// Read the register a `jr`/`jalr` terminator transfers through, directly
/// from the terminator instruction word at `end_va - 8` (the transfer word;
/// its delay slot is at `end_va - 4`). Returns `(rs, via_call)`.
fn terminator_register(bank_bytes: &[u8], va_start: u32, end_va: u32) -> Option<(u8, bool)> {
    // The Indirect terminator's transfer instruction sits two words before the
    // block's exclusive end (transfer word + its delay slot).
    let transfer_va = end_va.checked_sub(8)?;
    let off = transfer_va.checked_sub(va_start)? as usize;
    let word = u32::from_be_bytes(bank_bytes.get(off..off + 4)?.try_into().ok()?);
    let opcode = (word >> 26) & 0x3f;
    if opcode != 0 {
        return None;
    }
    let funct = word & 0x3f;
    let rs = ((word >> 21) & 0x1f) as u8;
    match funct {
        0x08 => Some((rs, false)), // jr
        0x09 => Some((rs, true)),  // jalr
        _ => None,
    }
}

/// Resolve every open indirect site in `cfg` that terminates a block whose
/// register construction is a bounded-exhaustive constant, keeping only
/// targets that land inside `[va_start, va_start + bank_bytes.len())` (a
/// resolved-but-out-of-bank target is a cross-bank tail transfer this bank's
/// CFG cannot own -- reported by returning it so the caller can decide, but
/// it will simply not seed a new in-bank root).
///
/// Returns the resolved targets in ascending `site_pc` order (deterministic).
pub fn resolve_indirect_sites(cfg: &Cfg, bank_bytes: &[u8], va_start: u32) -> Vec<ResolvedTarget> {
    let va_end = va_start.wrapping_add(bank_bytes.len() as u32);
    let mut out = Vec::new();

    for block in &cfg.blocks {
        let via_call = match block.terminator {
            BlockTerminator::Indirect { via_call } => via_call,
            _ => continue,
        };
        let Some((jr_rs, term_via_call)) = terminator_register(bank_bytes, va_start, block.end_va)
        else {
            continue;
        };
        debug_assert_eq!(term_via_call, via_call);

        // Gather this block's instruction words in order, up to but NOT
        // including the `jr`/`jalr` transfer word itself (at end_va - 8): on
        // MIPS the transfer register is read when the jump issues, so the
        // delay slot (end_va - 4) cannot change the target, and the transfer
        // word has no GPR-const effect. Tracking `[start, site_pc)` is both
        // correct and avoids letting a delay-slot write to the target
        // register spuriously alter it.
        let site_pc = block.end_va.wrapping_sub(8);
        let mut words = Vec::new();
        let mut pc = block.start_va;
        while pc < site_pc {
            let Some(off) = pc.checked_sub(va_start) else {
                break;
            };
            let off = off as usize;
            let Some(bytes) = bank_bytes.get(off..off + 4) else {
                break;
            };
            words.push((pc, u32::from_be_bytes(bytes.try_into().unwrap())));
            pc = pc.wrapping_add(4);
        }

        if let Some(resolved) = resolve_block_target(&words, site_pc, jr_rs, via_call) {
            // Only keep in-bank targets aligned to a word boundary; anything
            // else is either a cross-bank tail call or a malformed construction
            // and is not seeded as a root here.
            if resolved.target >= va_start
                && resolved.target < va_end
                && (resolved.target - va_start).is_multiple_of(4)
            {
                out.push(resolved);
            }
        }
    }

    out.sort_by_key(|r| r.site_pc);
    out.dedup();
    out
}

/// Build a bank's CFG and iterate Phase 6 resolution to a fixed point:
/// resolve bounded indirect targets, add them as roots, rebuild the CFG, and
/// repeat until no new in-bank target appears. Returns the closed CFG plus the
/// full set of resolved targets discovered along the way.
///
/// This is the mechanical replacement for hand-seeding a resident entry
/// address: the caller supplies only the header-derived roots it already
/// proves (e.g. the ROM entrypoint), and the fixed point discovers the rest
/// through bounded constant resolution alone.
pub fn build_cfg_closed(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
) -> (Cfg, Vec<ResolvedTarget>) {
    let mut roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    let mut resolved_all: Vec<ResolvedTarget> = Vec::new();
    let mut resolved_seen: BTreeSet<u32> = BTreeSet::new();

    // A finite fixed point: each iteration can only ever add roots (targets
    // are bounded by bank size), and roots are a monotonically growing set, so
    // this terminates in at most (bank_words) iterations -- in practice one or
    // two, since one boot stub resolves one entry that then reaches everything
    // by direct calls.
    loop {
        let root_vec: Vec<u32> = roots.iter().copied().collect();
        let cfg = build_cfg(bank, bank_bytes, va_start, &root_vec);
        let resolved = resolve_indirect_sites(&cfg, bank_bytes, va_start);

        let mut added_new = false;
        for r in &resolved {
            if resolved_seen.insert(r.site_pc) {
                resolved_all.push(*r);
            }
            if roots.insert(r.target) {
                added_new = true;
            }
        }

        if !added_new {
            resolved_all.sort_by_key(|r| r.site_pc);
            return (cfg, resolved_all);
        }
    }
}

/// Build fixed-point CFG closure with authoritative Phase 3 entries from the
/// shared fact database. Candidate, supported, rejected, conflict, and open
/// conclusions are intentionally not promoted to roots.
pub fn build_cfg_closed_with_facts(
    db: &crate::facts::FactDb,
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
) -> (Cfg, Vec<ResolvedTarget>) {
    let mut roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    roots.extend(db.proven_function_entries(bank));
    build_cfg_closed(
        bank,
        bank_bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    const NOP: u32 = 0x0000_0000;

    #[test]
    fn resolves_lui_addiu_jr_boot_stub_target() {
        // Exactly the OoT boot stub shape:
        //   lui   $t2, 0x8000
        //   addiu $t2, $t2, 0x0100   -> $t2 = 0x80000100
        //   jr    $t2
        //   nop  (delay slot)
        let lui = 0x3c0a_8000u32; // lui $t2 (reg 10), 0x8000
        let addiu = 0x254a_0100u32; // addiu $t2, $t2, 0x0100
        let jr_t2 = (10u32 << 21) | 0x08; // jr $t2
        let mut bytes = asm(&[lui, addiu, jr_t2, NOP]);
        bytes.resize(0x200, 0);
        // Put something at the target so it is a valid in-bank address.
        bytes[0x100..0x104].copy_from_slice(&NOP.to_be_bytes());

        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let resolved = resolve_indirect_sites(&cfg, &bytes, 0x8000_0000);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].target, 0x8000_0100);
        assert!(!resolved[0].via_call);
        assert_eq!(resolved[0].site_pc, 0x8000_0008);
    }

    #[test]
    fn fixed_point_seeds_resolved_target_as_a_real_root() {
        // Boot stub jumps to 0x80000100, which itself just returns. After the
        // fixed point, the target must be a proven root (i.e. the CFG built
        // with it seeded reaches it).
        let lui = 0x3c0a_8000u32;
        let addiu = 0x254a_0100u32;
        let jr_t2 = (10u32 << 21) | 0x08;
        let jr_ra = 0x03e0_0008u32;
        let mut bytes = asm(&[lui, addiu, jr_t2, NOP]);
        bytes.resize(0x200, 0);
        bytes[0x100..0x104].copy_from_slice(&jr_ra.to_be_bytes());
        bytes[0x104..0x108].copy_from_slice(&NOP.to_be_bytes());

        let (cfg, resolved) = build_cfg_closed("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(resolved.len(), 1);
        assert!(
            cfg.proven_roots.contains(&0x8000_0100),
            "resolved target must be seeded as a root: {:?}",
            cfg.proven_roots
        );
    }

    #[test]
    fn unresolvable_register_stays_open_never_fabricated() {
        // jr $t2 where $t2 was loaded from memory (lw) -- not a constant.
        let lw_t2 = 0x8d4a_0000u32; // lw $t2, 0($t2)
        let jr_t2 = (10u32 << 21) | 0x08;
        let bytes = asm(&[lw_t2, jr_t2, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let resolved = resolve_indirect_sites(&cfg, &bytes, 0x8000_0000);
        assert!(
            resolved.is_empty(),
            "a load-derived register must not resolve to a fabricated target"
        );
    }

    #[test]
    fn out_of_bank_resolved_target_is_not_seeded_as_root() {
        // Resolves to 0x8fff0000, far outside this small bank -- must be
        // dropped (a cross-bank tail transfer, not an in-bank root).
        let lui = 0x3c0a_8fffu32; // lui $t2, 0x8fff
        let jr_t2 = (10u32 << 21) | 0x08;
        let bytes = asm(&[lui, jr_t2, NOP]);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let resolved = resolve_indirect_sites(&cfg, &bytes, 0x8000_0000);
        assert!(resolved.is_empty());
    }

    #[test]
    fn ori_low_half_is_tracked() {
        // lui $t2, 0x8000 ; ori $t2, $t2, 0x0100 ; jr $t2
        let lui = 0x3c0a_8000u32;
        let ori = 0x354a_0100u32; // ori $t2, $t2, 0x0100
        let jr_t2 = (10u32 << 21) | 0x08;
        let mut bytes = asm(&[lui, ori, jr_t2, NOP]);
        bytes.resize(0x200, 0);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let resolved = resolve_indirect_sites(&cfg, &bytes, 0x8000_0000);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].target, 0x8000_0100);
    }

    #[test]
    fn register_move_propagates_constant() {
        // lui $t0,0x8000 ; addiu $t0,$t0,0x0100 ; move $t2,$t0 (or $t2,$t0,$zero) ; jr $t2
        let lui = 0x3c08_8000u32; // lui $t0
        let addiu = 0x2508_0100u32; // addiu $t0,$t0,0x100
                                    // or $t2, $t0, $zero  -> rd=10, rs=8, rt=0 (the shifted-0 field is
                                    // elided to satisfy clippy::identity_op), funct=0x25
        let mov = (8u32 << 21) | (10u32 << 11) | 0x25;
        let jr_t2 = (10u32 << 21) | 0x08;
        let mut bytes = asm(&[lui, addiu, mov, jr_t2, NOP]);
        bytes.resize(0x200, 0);
        let cfg = build_cfg("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let resolved = resolve_indirect_sites(&cfg, &bytes, 0x8000_0000);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].target, 0x8000_0100);
    }

    #[test]
    fn linear_jalr_scan_resolves_bounded_target_and_rejects_a_clobber() {
        let lui = 0x3c19_8000u32; // lui $t9, 0x8000
        let addiu = 0x2739_0100u32; // addiu $t9, $t9, 0x100
        let jalr = (25u32 << 21) | (31u32 << 11) | 0x09;
        let mut resolvable = asm(&[lui, addiu, jalr, NOP]);
        resolvable.resize(0x200, 0);
        let resolved = resolve_linear_jalr_sites(&resolvable, 0x8000_0000);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].target, 0x8000_0100);
        assert_eq!(resolved[0].construction_start, 0x8000_0000);

        let andi_t9 = 0x3339_ffffu32;
        let mut clobbered = asm(&[lui, addiu, andi_t9, jalr, NOP]);
        clobbered.resize(0x200, 0);
        assert!(resolve_linear_jalr_sites(&clobbered, 0x8000_0000).is_empty());
    }

    #[test]
    fn fact_integrated_closure_seeds_only_proven_entries() {
        use crate::facts::{
            function_entry_subject, BankAddr, CandidateDetector, Fact, FactDb,
            FunctionEntryEvidence, ProofState,
        };

        let target = 0x8000_0100;
        let jr_ra = 0x03e0_0008u32;
        let mut bytes = asm(&[jr_ra, NOP]);
        bytes.resize(0x200, 0);
        bytes[0x100..0x104].copy_from_slice(&jr_ra.to_be_bytes());

        let mut db = FactDb::new();
        for (pc, state) in [
            (target, ProofState::Proven),
            (0x8000_0180, ProofState::Candidate),
        ] {
            let address = BankAddr::new("boot", pc);
            let fact = db.insert(Fact::FunctionEntryClaim {
                target: address.clone(),
                detector: CandidateDetector::TableDerived,
                evidence: FunctionEntryEvidence::TableEntry {
                    table: BankAddr::new("boot", 0x8000_0080),
                    index: 0,
                },
                proposed_state: state,
            });
            db.conclude(function_entry_subject(&address), state, vec![fact], "test")
                .unwrap();
        }

        let (cfg, _) =
            build_cfg_closed_with_facts(&db, "boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert!(cfg.proven_roots.contains(&target));
        assert!(!cfg.proven_roots.contains(&0x8000_0180));
    }

    #[test]
    fn deterministic_across_repeated_runs() {
        let lui = 0x3c0a_8000u32;
        let addiu = 0x254a_0100u32;
        let jr_t2 = (10u32 << 21) | 0x08;
        let mut bytes = asm(&[lui, addiu, jr_t2, NOP]);
        bytes.resize(0x200, 0);
        let (_c1, r1) = build_cfg_closed("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        let (_c2, r2) = build_cfg_closed("boot", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(r1, r2);
    }
}
