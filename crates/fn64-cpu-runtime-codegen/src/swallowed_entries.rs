//! Cross-check a pre-baked external symbol dump against the ROM's own
//! `jal` evidence, and repair entries the dump silently swallowed.
//!
//! # The defect class
//!
//! `recompile_rom` loads an externally-generated symbol dump (spimdisasm /
//! splat output, harvested by a `glabel`-only scanner) and trusts its
//! function list completely. When the disassembler emits a real function
//! entry as an `alabel` (an *alternative* entry) instead of a `glabel`, the
//! `glabel`-only scanner drops it, and the PRECEDING function's declared
//! `size` silently extends over it. The entry then never reaches
//! `LOOKUP_TABLE`, and every `jal` to it becomes a runtime trap:
//!
//! ```text
//! lookup: no recompiled function or host shim at vram 0x80120854
//! ```
//!
//! This is a *build-time-detectable* condition: a plain static `jal`
//! immediate is independent, unambiguous evidence that its target is a real
//! callable root. `fn64-discover`'s CFG builder already encodes exactly this
//! rule (`crates/fn64-discover/src/cfg.rs`, the `ControlOp::Jal` arm promotes
//! every in-range target to a `proven_root`, while the `ControlOp::J` arm
//! deliberately does NOT, because ordinary intra-function jumps are what
//! spimdisasm legitimately represents as `alabel`s).
//!
//! `fn64-discover` depends on this crate, so this crate cannot depend back on
//! it. The single decode rule that matters here — "opcode 3 is `jal`; its
//! target is `((pc + 4) & 0xF000_0000) | (imm26 << 2)`" — is reproduced
//! directly against the ROM words, and [`region_target`] is verified against
//! `fn64-discover`'s own documented formula in this module's tests.
//!
//! # The repair, and why it is safe
//!
//! A wrong split is worse than no split: it would redirect a `jal` into the
//! middle of a live function body. So a candidate is only split when the
//! containing function has demonstrably RETURNED before the split point:
//!
//! * the word at `split - 8` is `jr $ra` (`0x03E0_0008`) and `split - 4` is
//!   its delay slot — i.e. the head's last real act is a return; or
//! * the same, with only `nop` (`0x0000_0000`) alignment padding between the
//!   delay slot and the split point.
//!
//! Anything else is REPORTED, never split.

/// The `jr $ra` encoding: `SPECIAL` / `rs = 31` / `funct = JR`.
const JR_RA: u32 = 0x03E0_0008;

/// MIPS `nop` (`sll $zero, $zero, 0`), the only padding permitted between a
/// head's return delay slot and a split point.
const NOP: u32 = 0x0000_0000;

/// `jal`'s primary opcode.
const OPCODE_JAL: u32 = 3;

/// `j`'s primary opcode.
const OPCODE_J: u32 = 2;

/// Resolve a `j`/`jal`'s 26-bit pseudo-region target: the high 4 bits of
/// `pc + 4` combined with `imm26 << 2`.
///
/// Mirrors `fn64_discover::cfg::region_target`; that crate depends on this
/// one, so the rule is restated here rather than imported.
pub fn region_target(pc: u32, target26: u32) -> u32 {
    ((pc.wrapping_add(4)) & 0xf000_0000) | (target26 << 2)
}

/// One function entry as the symbol dump declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpFunction {
    pub name: String,
    pub vram: u32,
    pub size: u32,
}

/// A linear code region to scan: its start vram and its big-endian words.
#[derive(Clone, Debug)]
pub struct CodeRegion<'a> {
    pub name: String,
    pub vram: u32,
    pub words: &'a [u32],
}

/// Why a proven root could not be safely split out of its containing
/// function. Each variant is a REPORTED outcome, never a silent skip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitRefusal {
    /// The head does not end in `jr $ra` + delay slot (allowing only `nop`
    /// padding). Splitting here would cut a live body in half.
    HeadDoesNotReturn,
    /// The split point is not word-aligned relative to the containing
    /// function, so no instruction boundary exists there.
    Misaligned,
    /// The containing function's declared range extends past the words this
    /// region actually holds, so the head cannot be inspected.
    OutOfRange,
}

impl SplitRefusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::HeadDoesNotReturn => {
                "containing function does not return (jr $ra + delay slot, nop padding only) \
                 immediately before the proven root"
            }
            Self::Misaligned => "proven root is not word-aligned inside the containing function",
            Self::OutOfRange => "containing function's declared range exceeds the region's words",
        }
    }
}

/// One function entry that `jal` evidence proves exists, but that the symbol
/// dump swallowed inside a preceding function's declared size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwallowedEntry {
    /// The region (config section) the evidence and the containing function
    /// both live in.
    pub region: String,
    /// The proven entry point: a `jal` immediate target.
    pub vram: u32,
    /// The dump function whose declared range swallowed it.
    pub containing_name: String,
    pub containing_vram: u32,
    pub containing_size: u32,
    /// Every `jal` site in the region whose immediate targets `vram`, in
    /// ascending address order. Never empty — this IS the evidence.
    pub jal_sites: Vec<u32>,
    /// `None` when the entry can be safely split out; `Some(reason)` when the
    /// head fails the return precondition and the entry is reported only.
    pub refusal: Option<SplitRefusal>,
}

impl SwallowedEntry {
    /// Whether the containing function can be split at this entry.
    pub fn is_repairable(&self) -> bool {
        self.refusal.is_none()
    }

    /// Byte offset of the proven root inside its containing function.
    pub fn offset_in_containing(&self) -> u32 {
        self.vram.wrapping_sub(self.containing_vram)
    }
}

/// The whole cross-check outcome for one run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrossCheck {
    /// Every swallowed entry found, in ascending `(region, vram)` order.
    pub swallowed: Vec<SwallowedEntry>,
    /// Number of distinct `jal`-proven roots examined across all regions.
    pub proven_roots: usize,
}

impl CrossCheck {
    pub fn is_clean(&self) -> bool {
        self.swallowed.is_empty()
    }

    pub fn repairable(&self) -> impl Iterator<Item = &SwallowedEntry> {
        self.swallowed.iter().filter(|e| e.is_repairable())
    }

    pub fn refused(&self) -> impl Iterator<Item = &SwallowedEntry> {
        self.swallowed.iter().filter(|e| !e.is_repairable())
    }

    /// A named, human-readable build-time diagnostic. Empty when clean, so a
    /// caller can print it unconditionally.
    ///
    /// This is the whole point of the check: it converts a mysterious runtime
    /// `lookup:` trap into a build-time message that names the exact address,
    /// the function that swallowed it, and the call sites that prove it.
    pub fn render_diagnostic(&self) -> String {
        if self.is_clean() {
            return String::new();
        }
        let mut s = String::new();
        s.push_str("SWALLOWED-FUNCTION-ENTRY: the symbol dump is missing function entries that\n");
        s.push_str("static `jal` immediates prove exist. Each was absorbed by the preceding\n");
        s.push_str("function's declared size, so it never reaches LOOKUP_TABLE and every call\n");
        s.push_str("to it traps at runtime with `lookup: no recompiled function ... at vram`.\n\n");
        for entry in &self.swallowed {
            s.push_str(&format!(
                "  {:#010X} in section {} swallowed by {} ({:#010X}, size {:#X}, at offset {:#X})\n",
                entry.vram,
                entry.region,
                entry.containing_name,
                entry.containing_vram,
                entry.containing_size,
                entry.offset_in_containing(),
            ));
            let sites: Vec<String> = entry
                .jal_sites
                .iter()
                .map(|pc| format!("{pc:#010X}"))
                .collect();
            s.push_str(&format!(
                "    proven by {} jal site(s): {}\n",
                entry.jal_sites.len(),
                sites.join(", ")
            ));
            match entry.refusal {
                None => s.push_str("    repair: SPLIT (head returns before this point)\n"),
                Some(reason) => {
                    s.push_str(&format!("    repair: REFUSED -- {}\n", reason.reason()));
                }
            }
        }
        s
    }
}

/// Collect every distinct in-region `jal` immediate target in `region`,
/// paired with the ascending list of call sites that prove it.
///
/// Only `jal` counts. A `j` immediate is deliberately excluded: it is an
/// ordinary intra-function jump, which is precisely what a disassembler
/// correctly represents as an `alabel`, and promoting those would over-split
/// real functions. This mirrors `fn64-discover`'s documented split between
/// its `ControlOp::J` and `ControlOp::Jal` arms.
pub fn jal_proven_roots(region: &CodeRegion<'_>) -> Vec<(u32, Vec<u32>)> {
    let mut by_target: std::collections::BTreeMap<u32, Vec<u32>> =
        std::collections::BTreeMap::new();
    let end = region
        .vram
        .wrapping_add((region.words.len() as u32).wrapping_mul(4));
    for (index, &word) in region.words.iter().enumerate() {
        if word >> 26 != OPCODE_JAL {
            continue;
        }
        let pc = region.vram.wrapping_add((index as u32).wrapping_mul(4));
        let target = region_target(pc, word & 0x03FF_FFFF);
        if target >= region.vram && target < end {
            by_target.entry(target).or_default().push(pc);
        }
    }
    by_target.into_iter().collect()
}

/// Whether `region` contains at least one `j` or `jal` immediate whose
/// resolved target is exactly `vram`.
///
/// A second, independent corroboration required before any split: the
/// address must be a real transfer destination in the section's own bytes.
pub fn has_immediate_transfer_to(region: &CodeRegion<'_>, vram: u32) -> bool {
    region.words.iter().enumerate().any(|(index, &word)| {
        let opcode = word >> 26;
        if opcode != OPCODE_J && opcode != OPCODE_JAL {
            return false;
        }
        let pc = region.vram.wrapping_add((index as u32).wrapping_mul(4));
        region_target(pc, word & 0x03FF_FFFF) == vram
    })
}

/// Decide whether `containing` may be split at `split_vram`.
///
/// Requires the head to have RETURNED: the last non-`nop` word before
/// `split_vram` must be a `jr $ra` delay slot, with `jr $ra` itself
/// immediately before it. Only `nop` may sit between that delay slot and the
/// split point (alignment padding).
pub fn classify_split(
    region: &CodeRegion<'_>,
    containing: &DumpFunction,
    split_vram: u32,
) -> Option<SplitRefusal> {
    let offset = match split_vram.checked_sub(containing.vram) {
        Some(offset) if offset % 4 == 0 => offset,
        _ => return Some(SplitRefusal::Misaligned),
    };
    let word_at = |vram: u32| -> Option<u32> {
        let delta = vram.checked_sub(region.vram)?;
        if delta % 4 != 0 {
            return None;
        }
        region.words.get(delta as usize / 4).copied()
    };
    // The containing function's whole declared range must be inspectable.
    let containing_end = containing.vram.wrapping_add(containing.size);
    if word_at(containing_end.wrapping_sub(4)).is_none() || word_at(containing.vram).is_none() {
        return Some(SplitRefusal::OutOfRange);
    }
    let _ = offset;
    // The head must end in `jr $ra` + its delay slot, with only `nop`
    // alignment padding between that delay slot and the split point.
    //
    // The delay slot is very often itself a `nop`, so a naive "walk back over
    // nops" would consume it and then look for `jr $ra` one word too early.
    // Instead: try each candidate `jr $ra` position, from the one closest to
    // the split point backwards, and require every word after its delay slot
    // to be `nop`.
    let mut ret = split_vram.wrapping_sub(8);
    while ret >= containing.vram {
        if word_at(ret) == Some(JR_RA) {
            // Everything in `ret + 8 .. split_vram` must be `nop` padding.
            let padding_is_clean = (ret.wrapping_add(8)..split_vram)
                .step_by(4)
                .all(|vram| word_at(vram) == Some(NOP));
            if padding_is_clean {
                return None;
            }
            // A `jr $ra` followed by real instructions before the split means
            // the head resumed executing: keep looking further back only if
            // the intervening words could still all be padding, which they
            // cannot. Stop here.
            return Some(SplitRefusal::HeadDoesNotReturn);
        }
        // Only `nop` may separate the return's delay slot from the split; any
        // other word between here and the split ends the search.
        if word_at(ret.wrapping_add(8)).is_some_and(|w| w != NOP)
            && ret.wrapping_add(8) < split_vram
        {
            return Some(SplitRefusal::HeadDoesNotReturn);
        }
        match ret.checked_sub(4) {
            Some(previous) => ret = previous,
            None => break,
        }
    }
    Some(SplitRefusal::HeadDoesNotReturn)
}

/// Cross-check one region's dump functions against its `jal` evidence.
///
/// A proven root is reported when it is NOT itself a declared function entry
/// but DOES fall strictly inside some declared function's range.
pub fn cross_check_region(region: &CodeRegion<'_>, functions: &[DumpFunction]) -> CrossCheck {
    let declared: std::collections::BTreeSet<u32> = functions.iter().map(|f| f.vram).collect();
    let mut sorted: Vec<&DumpFunction> = functions.iter().collect();
    sorted.sort_by_key(|f| f.vram);

    let roots = jal_proven_roots(region);
    let mut swallowed = Vec::new();
    for (target, jal_sites) in &roots {
        if declared.contains(target) {
            continue;
        }
        let Some(containing) = sorted
            .iter()
            .find(|f| f.vram < *target && *target < f.vram.wrapping_add(f.size))
        else {
            // A `jal` target outside every declared function is a different
            // defect (an entirely unmapped region), not this one. Left to the
            // existing coverage reporting.
            continue;
        };
        let refusal = classify_split(region, containing, *target).or_else(|| {
            // Corroboration: the address must be a real immediate transfer
            // destination in this section's bytes. `jal_proven_roots` already
            // guarantees this, so this can only ever confirm; it is kept as an
            // explicit precondition so a future caller supplying roots from
            // another source still gets the check.
            (!has_immediate_transfer_to(region, *target)).then_some(SplitRefusal::HeadDoesNotReturn)
        });
        swallowed.push(SwallowedEntry {
            region: region.name.clone(),
            vram: *target,
            containing_name: containing.name.clone(),
            containing_vram: containing.vram,
            containing_size: containing.size,
            jal_sites: jal_sites.clone(),
            refusal,
        });
    }
    swallowed.sort_by_key(|e| e.vram);
    CrossCheck {
        swallowed,
        proven_roots: roots.len(),
    }
}

/// Apply every repairable split in `check` to `functions`, in place.
///
/// The head's `size` shrinks to end exactly at the proven root, and a new
/// entry covering the remainder is inserted. Refused entries are left alone.
/// Returns the number of splits applied.
///
/// The generated name follows the dump's own `func_<VRAM>` convention so the
/// emitted body, the `LOOKUP_TABLE` row, and any report line all agree.
pub fn apply_repairs(functions: &mut Vec<DumpFunction>, check: &CrossCheck) -> usize {
    // Apply highest-vram-first so that splitting one function twice (two
    // swallowed entries in one head) keeps each earlier split's arithmetic
    // valid against the not-yet-shrunk head.
    let mut repairs: Vec<&SwallowedEntry> = check.repairable().collect();
    repairs.sort_by_key(|e| std::cmp::Reverse(e.vram));
    let mut applied = 0usize;
    for entry in repairs {
        let Some(index) = functions
            .iter()
            .position(|f| f.vram == entry.containing_vram && f.name == entry.containing_name)
        else {
            continue;
        };
        let head = &functions[index];
        let head_end = head.vram.wrapping_add(head.size);
        if !(head.vram < entry.vram && entry.vram < head_end) {
            // Already split by an earlier repair, or the caller mutated the
            // list; never guess.
            continue;
        }
        let tail = DumpFunction {
            name: format!("func_{:08X}_split", entry.vram),
            vram: entry.vram,
            size: head_end.wrapping_sub(entry.vram),
        };
        functions[index].size = entry.vram.wrapping_sub(head.vram);
        functions.insert(index + 1, tail);
        applied += 1;
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Assemble `jal <target>` at `pc`, by hand from the MIPS-III encoding:
    /// opcode 3 in bits 31..26, `target >> 2` in bits 25..0.
    fn jal(target: u32) -> u32 {
        (OPCODE_JAL << 26) | ((target >> 2) & 0x03FF_FFFF)
    }

    fn j(target: u32) -> u32 {
        (OPCODE_J << 26) | ((target >> 2) & 0x03FF_FFFF)
    }

    #[test]
    fn region_target_matches_the_documented_pseudo_region_rule() {
        // Derived by hand from the WM2000 wire bytes: at pc 0x801207C4 the
        // word 0x0C048215 must resolve to 0x80120854.
        //   imm26 = 0x0C048215 & 0x03FFFFFF = 0x0048215
        //   imm26 << 2                      = 0x0120854
        //   (pc + 4) & 0xF0000000           = 0x80000000
        //   => 0x80120854
        assert_eq!(
            region_target(0x8012_07C4, 0x0C04_8215 & 0x03FF_FFFF),
            0x8012_0854
        );
        // Region wrap: a `jal` in the 0x9... region stays in 0x9....
        assert_eq!(region_target(0x9000_0000, 0x0000_0004), 0x9000_0010);
    }

    #[test]
    fn jal_immediates_are_proven_roots_and_j_immediates_are_not() {
        // Layout, by hand:
        //   0x80000000 jal 0x80000010   <- proves 0x80000010
        //   0x80000004 nop  (delay)
        //   0x80000008 j   0x80000014   <- must NOT prove 0x80000014
        //   0x8000000C nop  (delay)
        //   0x80000010 nop
        //   0x80000014 nop
        let words = [jal(0x8000_0010), NOP, j(0x8000_0014), NOP, NOP, NOP];
        let region = CodeRegion {
            name: "t".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let roots = jal_proven_roots(&region);
        assert_eq!(roots, vec![(0x8000_0010, vec![0x8000_0000])]);
    }

    #[test]
    fn out_of_region_jal_targets_are_not_proven_roots() {
        // A `jal` to an address past the region's own words proves nothing
        // about THIS region's function list.
        let words = [jal(0x8000_1000), NOP];
        let region = CodeRegion {
            name: "t".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        assert!(jal_proven_roots(&region).is_empty());
    }

    #[test]
    fn every_jal_site_to_one_target_is_recorded_in_ascending_order() {
        let words = [jal(0x8000_0014), NOP, NOP, NOP, jal(0x8000_0014), NOP];
        let region = CodeRegion {
            name: "t".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let roots = jal_proven_roots(&region);
        assert_eq!(roots, vec![(0x8000_0014, vec![0x8000_0000, 0x8000_0010])]);
    }

    /// A head that returns (`jr $ra` + delay slot) immediately before the
    /// split point: repairable.
    #[test]
    fn swallowed_entry_after_a_returning_head_is_repairable() {
        // 0x80000000 jal 0x80000010     (site)
        // 0x80000004 nop                (delay)
        // 0x80000008 jr $ra             (head returns)
        // 0x8000000C nop                (delay slot)
        // 0x80000010 nop                <- swallowed entry
        // 0x80000014 jr $ra
        // 0x80000018 nop
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x1C,
        }];
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.swallowed.len(), 1);
        let entry = &check.swallowed[0];
        assert_eq!(entry.vram, 0x8000_0010);
        assert_eq!(entry.containing_name, "head");
        assert_eq!(entry.jal_sites, vec![0x8000_0000]);
        assert!(entry.is_repairable(), "{:?}", entry.refusal);
    }

    /// MUTATION GUARD: the same shape, but the head does NOT return before
    /// the split point. Must be reported and REFUSED, never split. If the
    /// `jr $ra` precondition were dropped, this test fails.
    #[test]
    fn swallowed_entry_after_a_non_returning_head_is_refused() {
        // Identical to the repairable case except 0x80000008 is an ordinary
        // instruction (`addiu $sp, $sp, -0x20`) rather than `jr $ra`, so the
        // head is still live at the split point.
        let words = [jal(0x8000_0010), NOP, 0x27BD_FFE0, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x1C,
        }];
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.swallowed.len(), 1, "still reported");
        assert_eq!(
            check.swallowed[0].refusal,
            Some(SplitRefusal::HeadDoesNotReturn)
        );
        // And a repair pass must leave the function list untouched.
        let mut repaired = functions.clone();
        assert_eq!(apply_repairs(&mut repaired, &check), 0);
        assert_eq!(repaired, functions);
    }

    /// MUTATION GUARD: `nop` padding between the return delay slot and the
    /// split point is permitted, but ONLY `nop`.
    #[test]
    fn nop_padding_between_the_return_and_the_split_is_allowed() {
        // 0x80000000 jal 0x80000014
        // 0x80000004 nop   (delay)
        // 0x80000008 jr $ra
        // 0x8000000C nop   (delay slot)
        // 0x80000010 nop   (alignment padding)
        // 0x80000014 nop   <- swallowed entry
        let words = [jal(0x8000_0014), NOP, JR_RA, NOP, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x20,
        }];
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.swallowed.len(), 1);
        assert!(check.swallowed[0].is_repairable());
    }

    /// MUTATION GUARD: non-`nop` padding is NOT skipped. Here the word just
    /// before the split is a live instruction, so walking back over it would
    /// be wrong.
    #[test]
    fn non_nop_filler_before_the_split_is_not_skipped() {
        // 0x80000010 is `addiu $sp,$sp,0x20` -- a real instruction, not
        // padding -- so the head has NOT returned at 0x80000014.
        let words = [
            jal(0x8000_0014),
            NOP,
            JR_RA,
            NOP,
            0x27BD_0020,
            NOP,
            JR_RA,
            NOP,
        ];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x20,
        }];
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.swallowed.len(), 1);
        assert_eq!(
            check.swallowed[0].refusal,
            Some(SplitRefusal::HeadDoesNotReturn)
        );
    }

    #[test]
    fn a_declared_function_entry_is_never_reported_as_swallowed() {
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_0010,
                size: 0xC,
            },
        ];
        let check = cross_check_region(&region, &functions);
        assert!(check.is_clean(), "{:?}", check.swallowed);
        assert_eq!(check.proven_roots, 1);
    }

    #[test]
    fn a_jal_target_inside_no_declared_function_is_not_this_defect() {
        // The target falls in a gap between declared functions: a different
        // (coverage) problem, deliberately not reported here.
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x10,
        }];
        let check = cross_check_region(&region, &functions);
        assert!(check.is_clean(), "{:?}", check.swallowed);
    }

    #[test]
    fn repair_splits_the_head_and_inserts_the_proven_entry() {
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let mut functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x1C,
        }];
        let check = cross_check_region(&region, &functions);
        assert_eq!(apply_repairs(&mut functions, &check), 1);
        // Derived by hand: head 0x80000000..0x80000010 (0x10 bytes), tail
        // 0x80000010..0x8000001C (0xC bytes). Sizes must still tile exactly.
        assert_eq!(functions.len(), 2);
        assert_eq!(functions[0].vram, 0x8000_0000);
        assert_eq!(functions[0].size, 0x10);
        assert_eq!(functions[1].vram, 0x8000_0010);
        assert_eq!(functions[1].size, 0xC);
        assert_eq!(functions[1].name, "func_80000010_split");
        // Re-running the check on the repaired list must now be clean: the
        // entry is declared, so nothing is swallowed.
        assert!(cross_check_region(&region, &functions).is_clean());
    }

    /// Two swallowed entries in ONE head. Highest-first application must
    /// produce three exactly-tiling functions.
    #[test]
    fn two_swallowed_entries_in_one_head_both_split_correctly() {
        // 0x80000000 jal 0x80000010
        // 0x80000004 nop
        // 0x80000008 jal 0x80000018
        // 0x8000000C nop
        // 0x80000010 jr $ra            (head returns before BOTH splits)
        // -- wait: 0x80000010 IS the first split; the head must return before
        //    it, so lay the return at 0x80000008/0x8000000C instead.
        //
        // Final hand-derived layout:
        //   0x80000000 jal 0x80000014
        //   0x80000004 jal 0x8000001C   (in the delay slot position, still a
        //                                 real jal word for evidence purposes)
        //   0x80000008 jr $ra
        //   0x8000000C nop              (delay slot)
        //   0x80000010 nop              (padding)
        //   0x80000014 nop              <- entry A
        //   0x80000018 jr $ra
        //   0x8000001C nop              (delay slot) -- but entry B is here,
        //   so instead put the return at 0x80000014/0x80000018 and B at 0x1C.
        let words = [
            jal(0x8000_0014), // 0x00
            jal(0x8000_0020), // 0x04
            JR_RA,            // 0x08
            NOP,              // 0x0C delay slot
            NOP,              // 0x10 padding
            NOP,              // 0x14 <- entry A
            JR_RA,            // 0x18
            NOP,              // 0x1C delay slot
            NOP,              // 0x20 <- entry B
            JR_RA,            // 0x24
            NOP,              // 0x28
        ];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let mut functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x2C,
        }];
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.swallowed.len(), 2);
        assert!(check.swallowed.iter().all(|e| e.is_repairable()));
        assert_eq!(apply_repairs(&mut functions, &check), 2);
        // By hand: 0x00..0x14 (0x14), 0x14..0x20 (0xC), 0x20..0x2C (0xC).
        let shape: Vec<(u32, u32)> = functions.iter().map(|f| (f.vram, f.size)).collect();
        assert_eq!(
            shape,
            vec![
                (0x8000_0000, 0x14),
                (0x8000_0014, 0x0C),
                (0x8000_0020, 0x0C),
            ]
        );
        assert!(cross_check_region(&region, &functions).is_clean());
    }

    #[test]
    fn a_clean_check_renders_an_empty_diagnostic() {
        assert_eq!(CrossCheck::default().render_diagnostic(), "");
    }

    #[test]
    fn the_diagnostic_names_the_address_the_owner_and_every_site() {
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "bank3_text".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "func_80000000".into(),
            vram: 0x8000_0000,
            size: 0x1C,
        }];
        let text = cross_check_region(&region, &functions).render_diagnostic();
        assert!(text.contains("SWALLOWED-FUNCTION-ENTRY"), "{text}");
        assert!(text.contains("0x80000010"), "{text}");
        assert!(text.contains("func_80000000"), "{text}");
        assert!(text.contains("bank3_text"), "{text}");
        assert!(text.contains("0x80000000"), "{text}");
        assert!(text.contains("SPLIT"), "{text}");
    }

    #[test]
    fn a_refused_entry_renders_its_reason_not_a_split() {
        let words = [jal(0x8000_0010), NOP, 0x27BD_FFE0, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x1C,
        }];
        let text = cross_check_region(&region, &functions).render_diagnostic();
        assert!(text.contains("REFUSED"), "{text}");
        assert!(text.contains("does not return"), "{text}");
        assert!(!text.contains("SPLIT"), "{text}");
    }

    #[test]
    fn has_immediate_transfer_to_accepts_j_and_jal_and_rejects_others() {
        let words = [j(0x8000_000C), NOP, jal(0x8000_0010), NOP, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        assert!(has_immediate_transfer_to(&region, 0x8000_000C));
        assert!(has_immediate_transfer_to(&region, 0x8000_0010));
        assert!(!has_immediate_transfer_to(&region, 0x8000_0004));
    }

    /// MUTATION GUARD: a split point that is not word-aligned inside the
    /// containing function is refused as `Misaligned`, never split.
    #[test]
    fn a_misaligned_split_point_is_refused() {
        let words = [JR_RA, NOP, NOP, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let containing = DumpFunction {
            name: "head".into(),
            vram: 0x8000_0001,
            size: 0xC,
        };
        assert_eq!(
            classify_split(&region, &containing, 0x8000_0008),
            Some(SplitRefusal::Misaligned)
        );
    }

    /// MUTATION GUARD: a containing function declaring more bytes than the
    /// region holds is refused as `OutOfRange`, never split.
    #[test]
    fn a_containing_function_past_the_region_end_is_refused() {
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let containing = DumpFunction {
            name: "head".into(),
            vram: 0x8000_0000,
            size: 0x100,
        };
        assert_eq!(
            classify_split(&region, &containing, 0x8000_0010),
            Some(SplitRefusal::OutOfRange)
        );
    }
}
