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

/// Why a `jal`-proven root that sits in a GAP between declared functions
/// could not be adopted as a new entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GapRefusal {
    /// The gap's words do not begin a plausible function body: the entry
    /// itself must be a real instruction, and the gap must end in `jr $ra`
    /// plus its delay slot (allowing only `nop` tail padding).
    GapDoesNotReturn,
    /// The proven root is not word-aligned within the region.
    Misaligned,
    /// The gap extends past the words this region actually holds.
    OutOfRange,
    /// The root is not at the very start of the gap. Adopting it would leave
    /// unclaimed words before it, which is a different (unmapped-region)
    /// defect and is reported rather than guessed at.
    NotAtGapStart,
}

impl GapRefusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::GapDoesNotReturn => {
                "the uncovered range does not end in `jr $ra` + delay slot (nop tail padding only)"
            }
            Self::Misaligned => "proven root is not word-aligned in the region",
            Self::OutOfRange => "the uncovered range exceeds the region's words",
            Self::NotAtGapStart => {
                "proven root is not at the start of the uncovered range, so words before it \
                 would stay unclaimed"
            }
        }
    }
}

/// One function entry that `jal` evidence proves exists, and that the symbol
/// dump left entirely UNCOVERED — it falls in a gap between two declared
/// functions rather than inside one.
///
/// # Why this is a distinct case from [`SwallowedEntry`]
///
/// A swallowed entry is *hidden inside* a preceding function's declared
/// `size`; repairing it means SPLITTING that function, which is dangerous and
/// needs the head-returns precondition. An uncovered entry is claimed by
/// nobody: the dump simply omits the function. Adopting it takes bytes away
/// from no one, so it cannot corrupt a live body — the risk profile is
/// entirely different, and so is the repair.
///
/// This is exactly the shape of WM2000's `0x801226A0` and `0x80122F2C`,
/// which the `glabel`-only harvester dropped from `bank4_text` even though
/// the disassembly gives both a `glabel`, a stack prologue, and a `jr $ra`
/// epilogue. Because those vrams are ALSO interior addresses of other
/// overlay banks' functions, they were misfiled as bank-overlap cases; they
/// are not. Only one bank declares an entry there, so the ordinary flat
/// `LOOKUP_TABLE` carries them once the dump is repaired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UncoveredEntry {
    /// The region (config section) the evidence lives in.
    pub region: String,
    /// The proven entry point: a `jal` immediate target.
    pub vram: u32,
    /// The uncovered range this root starts: `[gap_start, gap_end)`.
    pub gap_start: u32,
    pub gap_end: u32,
    /// Every `jal` site in the region whose immediate targets `vram`, in
    /// ascending address order. Never empty — this IS the evidence.
    pub jal_sites: Vec<u32>,
    /// `None` when the entry can be safely adopted; `Some(reason)` otherwise.
    pub refusal: Option<GapRefusal>,
}

impl UncoveredEntry {
    pub fn is_repairable(&self) -> bool {
        self.refusal.is_none()
    }

    /// The size the adopted entry would declare: the whole uncovered range.
    pub fn adopted_size(&self) -> u32 {
        self.gap_end.wrapping_sub(self.gap_start)
    }
}

/// The whole cross-check outcome for one run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CrossCheck {
    /// Every swallowed entry found, in ascending `(region, vram)` order.
    pub swallowed: Vec<SwallowedEntry>,
    /// Every uncovered entry found, in ascending `(region, vram)` order.
    pub uncovered: Vec<UncoveredEntry>,
    /// Number of distinct `jal`-proven roots examined across all regions.
    pub proven_roots: usize,
}

impl CrossCheck {
    pub fn is_clean(&self) -> bool {
        self.swallowed.is_empty() && self.uncovered.is_empty()
    }

    pub fn uncovered_repairable(&self) -> impl Iterator<Item = &UncoveredEntry> {
        self.uncovered.iter().filter(|e| e.is_repairable())
    }

    pub fn uncovered_refused(&self) -> impl Iterator<Item = &UncoveredEntry> {
        self.uncovered.iter().filter(|e| !e.is_repairable())
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
        if !self.uncovered.is_empty() {
            s.push_str(
                "\nUNCOVERED-FUNCTION-ENTRY: the symbol dump omits function entries that static\n\
                 `jal` immediates prove exist. Each falls in a GAP between two declared\n\
                 functions -- claimed by nobody -- so it never reaches LOOKUP_TABLE and every\n\
                 call to it traps at runtime. Adopting one takes bytes from no declared\n\
                 function, so it cannot corrupt a live body.\n\n",
            );
            for entry in &self.uncovered {
                s.push_str(&format!(
                    "  {:#010X} in section {} uncovered, gap {:#010X}..{:#010X} (size {:#X})\n",
                    entry.vram,
                    entry.region,
                    entry.gap_start,
                    entry.gap_end,
                    entry.adopted_size(),
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
                    None => s.push_str("    repair: ADOPT (gap is a self-contained body)\n"),
                    Some(reason) => {
                        s.push_str(&format!("    repair: REFUSED -- {}\n", reason.reason()));
                    }
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

/// Decide whether a `jal`-proven root starting an uncovered range may be
/// adopted as a new declared entry covering `[gap_start, gap_end)`.
///
/// # The precondition, and why it is the right one
///
/// Adopting removes bytes from no declared function, so the split rule's
/// "the head must have returned" question does not arise. What must instead
/// be true is that the uncovered range is a SELF-CONTAINED BODY:
///
/// * the root sits exactly at `gap_start` — otherwise words before it would
///   remain unclaimed, which is a different defect (an unmapped region), not
///   this one; and
/// * the range ENDS in `jr $ra` plus its delay slot, with only `nop` tail
///   padding after it — i.e. control leaves the range by returning, so the
///   adopted entry does not run off its own end into the next function.
///
/// A range that fails either test is REPORTED, never adopted.
pub fn classify_gap_adoption(
    region: &CodeRegion<'_>,
    gap_start: u32,
    gap_end: u32,
    root: u32,
) -> Option<GapRefusal> {
    let word_at = |vram: u32| -> Option<u32> {
        let delta = vram.checked_sub(region.vram)?;
        if delta % 4 != 0 {
            return None;
        }
        region.words.get(delta as usize / 4).copied()
    };
    if root.checked_sub(region.vram).is_none_or(|d| d % 4 != 0) {
        return Some(GapRefusal::Misaligned);
    }
    if root != gap_start {
        return Some(GapRefusal::NotAtGapStart);
    }
    if gap_end <= gap_start {
        return Some(GapRefusal::OutOfRange);
    }
    // The whole range must be inspectable.
    if word_at(gap_start).is_none() || word_at(gap_end.wrapping_sub(4)).is_none() {
        return Some(GapRefusal::OutOfRange);
    }
    // The range must END by returning. Scan back from the last word for a
    // `jr $ra`, requiring everything after its delay slot to be `nop`
    // alignment padding.
    //
    // The delay slot is very often itself a `nop`, so a naive "walk back over
    // nops first" would consume it and then look one word too early. Instead
    // try each candidate `jr $ra` position from the end backwards, and
    // require every word after its delay slot to be `nop` — the same shape
    // `classify_split` uses for the head side.
    let mut ret = gap_end.wrapping_sub(8);
    while ret >= gap_start {
        if word_at(ret) == Some(JR_RA) {
            let padding_is_clean = (ret.wrapping_add(8)..gap_end)
                .step_by(4)
                .all(|vram| word_at(vram) == Some(NOP));
            return if padding_is_clean {
                None
            } else {
                // A `jr $ra` followed by real instructions before the end
                // means control resumed inside the range and then ran off it.
                Some(GapRefusal::GapDoesNotReturn)
            };
        }
        // NOTE: no second "only nop may precede the end" guard here, unlike
        // `classify_split`. It would be redundant: the padding check above
        // already refuses every range this guard could, verified by
        // exhaustively comparing both forms over all small gaps built from
        // {jr $ra, nop, live}. Keeping it would be an equivalent-mutant trap
        // — untestable code that looks load-bearing.
        match ret.checked_sub(4) {
            Some(previous) => ret = previous,
            None => break,
        }
    }
    Some(GapRefusal::GapDoesNotReturn)
}

/// Cross-check one region's dump functions against its `jal` evidence.
///
/// A proven root is reported when it is NOT itself a declared function entry
/// but DOES fall strictly inside some declared function's range.
pub fn cross_check_region(region: &CodeRegion<'_>, functions: &[DumpFunction]) -> CrossCheck {
    let declared: std::collections::BTreeSet<u32> = functions.iter().map(|f| f.vram).collect();
    let mut sorted: Vec<&DumpFunction> = functions.iter().collect();
    sorted.sort_by_key(|f| f.vram);

    let region_end = region
        .vram
        .wrapping_add((region.words.len() as u32).wrapping_mul(4));
    let roots = jal_proven_roots(region);
    let mut swallowed = Vec::new();
    let mut uncovered = Vec::new();
    for (target, jal_sites) in &roots {
        if declared.contains(target) {
            continue;
        }
        let Some(containing) = sorted
            .iter()
            .find(|f| f.vram < *target && *target < f.vram.wrapping_add(f.size))
        else {
            // A `jal` target claimed by NO declared function: the dump omits
            // the function entirely. Bound the uncovered range by the nearest
            // declared neighbours (falling back to the region's own bounds)
            // and decide whether it is a self-contained body worth adopting.
            let gap_start = sorted
                .iter()
                .map(|f| f.vram.wrapping_add(f.size))
                .filter(|end| *end <= *target)
                .max()
                .unwrap_or(region.vram);
            let gap_end = sorted
                .iter()
                .map(|f| f.vram)
                .filter(|start| *start > *target)
                .min()
                .unwrap_or(region_end);
            let refusal = classify_gap_adoption(region, gap_start, gap_end, *target);
            uncovered.push(UncoveredEntry {
                region: region.name.clone(),
                vram: *target,
                gap_start,
                gap_end,
                jal_sites: jal_sites.clone(),
                refusal,
            });
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
    uncovered.sort_by_key(|e| e.vram);
    CrossCheck {
        swallowed,
        uncovered,
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

/// Insert every adoptable uncovered entry from `check` into `functions`.
///
/// Unlike [`apply_repairs`], this changes no existing entry's `vram` or
/// `size`: it only claims a range nothing declared. Returns the number
/// adopted.
///
/// The generated name follows the dump's own `func_<VRAM>` convention so the
/// emitted body, the `LOOKUP_TABLE` row, and any report line all agree.
pub fn apply_gap_adoptions(functions: &mut Vec<DumpFunction>, check: &CrossCheck) -> usize {
    let mut adopted = 0usize;
    for entry in check.uncovered_repairable() {
        // Never adopt an address some function already declares, and never
        // adopt into a range that overlaps a declared function: both would
        // mean the list changed under us since the check ran.
        let overlaps = functions
            .iter()
            .any(|f| f.vram < entry.gap_end && entry.gap_start < f.vram.wrapping_add(f.size));
        if overlaps {
            continue;
        }
        functions.push(DumpFunction {
            name: format!("func_{:08X}_uncovered", entry.vram),
            vram: entry.vram,
            size: entry.adopted_size(),
        });
        adopted += 1;
    }
    functions.sort_by_key(|f| f.vram);
    adopted
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

    /// `jr $ra` in the delay-slot-bearing shape used by the gap fixtures.
    ///
    /// Layout shared by the uncovered-entry tests below, derived by hand from
    /// WM2000's `bank4_text` around `0x801226A0`:
    ///
    /// ```text
    ///   0x80000000  jal 0x80000010     <- the caller, INSIDE this region
    ///   0x80000004  nop                   (delay slot)
    ///   0x80000008  jr $ra             <- `head` ends here
    ///   0x8000000C  nop                   (delay slot)
    ///   ---- head's declared range ends at 0x80000010 ----
    ///   0x80000010  addiu $sp, $sp, -0x18   <- UNCOVERED entry, prologue
    ///   0x80000014  jr $ra
    ///   0x80000018  nop                     (delay slot)
    ///   ---- gap ends at 0x8000001C ----
    ///   0x8000001C  nop                <- `tail`, a declared function
    /// ```
    fn uncovered_fixture() -> ([u32; 8], Vec<DumpFunction>) {
        let words = [
            jal(0x8000_0010),
            NOP,
            JR_RA,
            NOP,
            0x27BD_FFE8, // addiu $sp, $sp, -0x18
            JR_RA,
            NOP,
            NOP,
        ];
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_001C,
                size: 0x4,
            },
        ];
        (words, functions)
    }

    /// FAILS BEFORE this change (`cross_check_region` skipped every `jal`
    /// target claimed by no declared function, so `uncovered` was empty and
    /// the address stayed undispatchable); PASSES AFTER.
    ///
    /// This is the reduced form of WM2000's `0x801226A0`: a real function
    /// entry sitting in a GAP between two declared entries, not swallowed
    /// inside either of them.
    #[test]
    fn a_jal_proven_root_in_a_gap_is_reported_as_uncovered_and_adopted() {
        let (words, functions) = uncovered_fixture();
        let region = CodeRegion {
            name: "bank4_text".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        // It is NOT a swallowed entry: no declared function contains it.
        assert!(check.swallowed.is_empty(), "{:?}", check.swallowed);
        assert_eq!(check.uncovered.len(), 1, "{:?}", check.uncovered);
        let entry = &check.uncovered[0];
        assert_eq!(entry.vram, 0x8000_0010);
        // Gap bounds derived by hand: head ends at 0x10, tail starts at 0x1C.
        assert_eq!(entry.gap_start, 0x8000_0010);
        assert_eq!(entry.gap_end, 0x8000_001C);
        assert_eq!(entry.adopted_size(), 0xC);
        assert_eq!(entry.jal_sites, vec![0x8000_0000]);
        assert_eq!(entry.refusal, None);

        // And the repair actually lands a dispatchable entry.
        let mut repaired = functions.clone();
        assert_eq!(apply_gap_adoptions(&mut repaired, &check), 1);
        let adopted = repaired
            .iter()
            .find(|f| f.vram == 0x8000_0010)
            .expect("the adopted entry must be in the function list");
        assert_eq!(adopted.size, 0xC);
        assert_eq!(adopted.name, "func_80000010_uncovered");
        // Neither neighbour may be disturbed: adoption claims only the gap.
        assert_eq!(repaired.iter().find(|f| f.name == "head").unwrap().size, 0x10);
        assert_eq!(repaired.iter().find(|f| f.name == "tail").unwrap().size, 0x4);
    }

    /// MUTATION GUARD for the `jr $ra` terminator precondition. Identical to
    /// the adoptable fixture except the gap never returns, so control would
    /// run off the adopted entry's end into the next function. Must be
    /// REPORTED and REFUSED. Deleting the terminator check fails this.
    #[test]
    fn an_uncovered_gap_that_never_returns_is_refused() {
        let words = [
            jal(0x8000_0010),
            NOP,
            JR_RA,
            NOP,
            0x27BD_FFE8, // addiu $sp, $sp, -0x18
            0x27BD_FFE8, // ... and no return anywhere in the gap
            0x27BD_FFE8,
            NOP,
        ];
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_001C,
                size: 0x4,
            },
        ];
        let region = CodeRegion {
            name: "bank4_text".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1, "still reported");
        assert_eq!(
            check.uncovered[0].refusal,
            Some(GapRefusal::GapDoesNotReturn)
        );
        let mut repaired = functions.clone();
        assert_eq!(apply_gap_adoptions(&mut repaired, &check), 0);
        assert_eq!(repaired, functions);
    }

    /// MUTATION GUARD for the "root must be at the gap start" precondition.
    /// This is the shape of WM2000's REAL refusal, `0x800400CC`: a word in a
    /// large uncovered span that merely decodes as a `jal` target. Adopting
    /// it would leave every word before it unclaimed. Dropping the
    /// `NotAtGapStart` arm fails this.
    #[test]
    fn an_uncovered_root_that_is_not_at_the_gap_start_is_refused() {
        // The proven root is 0x80000014, but the gap starts at 0x80000010.
        let words = [
            jal(0x8000_0014),
            NOP,
            JR_RA,
            NOP,
            NOP, // 0x80000010: unclaimed, and BEFORE the root
            0x27BD_FFE8,
            JR_RA,
            NOP,
        ];
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_0020,
                size: 0x4,
            },
        ];
        let region = CodeRegion {
            name: "bank4_text".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1);
        assert_eq!(check.uncovered[0].vram, 0x8000_0014);
        assert_eq!(check.uncovered[0].gap_start, 0x8000_0010);
        assert_eq!(
            check.uncovered[0].refusal,
            Some(GapRefusal::NotAtGapStart)
        );
        let mut repaired = functions.clone();
        assert_eq!(apply_gap_adoptions(&mut repaired, &check), 0);
        assert_eq!(repaired, functions);
    }

    /// MUTATION GUARD separating the two repair classes. A root INSIDE a
    /// declared function must stay a `SwallowedEntry` and must NOT be
    /// adopted as uncovered; if the gap branch ever swallowed this case it
    /// would split nothing and silently claim overlapping bytes.
    #[test]
    fn a_root_inside_a_declared_function_is_swallowed_not_uncovered() {
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
        assert!(
            check.uncovered.is_empty(),
            "a swallowed entry must not also be reported as uncovered: {:?}",
            check.uncovered
        );
        // And the adoption pass must not touch it.
        let mut repaired = functions.clone();
        assert_eq!(apply_gap_adoptions(&mut repaired, &check), 0);
    }

    /// MUTATION GUARD: `nop` tail padding between the gap's return and the
    /// next declared function is permitted (the real WM2000 gaps are
    /// 4-to-12-byte aligned), but the return must still be there.
    #[test]
    fn nop_tail_padding_after_the_gaps_return_is_allowed() {
        let words = [
            jal(0x8000_0010),
            NOP,
            JR_RA,
            NOP,
            0x27BD_FFE8,
            JR_RA,
            NOP, // delay slot
            NOP, // alignment padding
            NOP, // alignment padding
            NOP, // tail function
        ];
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_0024,
                size: 0x4,
            },
        ];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1);
        assert_eq!(check.uncovered[0].refusal, None, "nop tail padding is fine");
        assert_eq!(check.uncovered[0].adopted_size(), 0x14);
    }

    /// MUTATION GUARD, added after a surviving mutant, then RE-derived after
    /// the first attempt still failed to kill it.
    ///
    /// Replacing the tail-padding check with `true` survived every earlier
    /// fixture, because in each of them an *earlier* guard already refused
    /// the range, so the mutated expression was never evaluated. This is the
    /// trap the brief warns about: a fixture that samples a point where the
    /// correct and incorrect answers coincide.
    ///
    /// The shape that actually reaches it, derived by exhaustively searching
    /// small gaps for a divergence between the real and mutated classifier:
    /// the return sits at `gap_end - 12`, so the backward scan's first step
    /// lands on the delay slot, whose own `+8` probe is at `gap_end` and is
    /// therefore skipped — leaving the padding check as the only thing that
    /// can refuse.
    ///
    /// ```text
    ///   0x80000010  jr $ra                  <- proven root / gap start
    ///   0x80000014  nop                        (delay slot)
    ///   0x80000018  addiu $sp, $sp, -0x18   <- LIVE word, not padding
    ///   ---- gap ends at 0x8000001C ----
    /// ```
    ///
    /// Control returns at `0x80000010` and then `0x80000018` still executes
    /// and runs off the adopted entry's end, so the range is not a
    /// self-contained body and must be REFUSED.
    #[test]
    fn a_gap_whose_trailing_words_are_live_instructions_is_refused() {
        let words = [
            jal(0x8000_0010),
            NOP,
            JR_RA,
            NOP,
            JR_RA,       // 0x80000010: gap start, and the return
            NOP,         // 0x80000014: its delay slot
            0x27BD_FFE8, // 0x80000018: a live word, NOT nop padding
            NOP,         // 0x8000001C: the tail function
        ];
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_001C,
                size: 0x4,
            },
        ];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1, "still reported");
        assert_eq!(
            check.uncovered[0].refusal,
            Some(GapRefusal::GapDoesNotReturn),
            "trailing live instructions are not nop padding"
        );
        let mut repaired = functions.clone();
        assert_eq!(apply_gap_adoptions(&mut repaired, &check), 0);
        assert_eq!(repaired, functions);
    }

    /// MUTATION GUARD: `gap_end` must be the NEAREST declared function above
    /// the root (`min`), not the farthest (`max`). With two functions above
    /// the gap, `max` would claim bytes that the intervening function
    /// already declares. Earlier fixtures had exactly one function above the
    /// gap, so `min` and `max` coincided and the mutant survived.
    ///
    /// ```text
    ///   0x80000010  addiu $sp, $sp, -0x18   <- root / gap start
    ///   0x80000014  jr $ra
    ///   0x80000018  nop                        (delay slot)
    ///   0x8000001C  `mid`   (declared, size 4) <- the NEAREST neighbour
    ///   0x80000020  `far`   (declared, size 4)
    /// ```
    #[test]
    fn the_gap_ends_at_the_nearest_declared_function_not_the_farthest() {
        let words = [
            jal(0x8000_0010),
            NOP,
            JR_RA,
            NOP,
            0x27BD_FFE8,
            JR_RA,
            NOP,
            NOP,
            NOP,
        ];
        let functions = vec![
            DumpFunction {
                name: "head".into(),
                vram: 0x8000_0000,
                size: 0x10,
            },
            DumpFunction {
                name: "mid".into(),
                vram: 0x8000_001C,
                size: 0x4,
            },
            DumpFunction {
                name: "far".into(),
                vram: 0x8000_0020,
                size: 0x4,
            },
        ];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1);
        assert_eq!(
            check.uncovered[0].gap_end, 0x8000_001C,
            "the gap must stop at `mid`, never swallow it to reach `far`"
        );
        assert_eq!(check.uncovered[0].adopted_size(), 0xC);
    }

    /// MUTATION GUARD, the `gap_start` mirror of the `gap_end` test above:
    /// the gap must begin at the END of the NEAREST declared function below
    /// the root (`max` of the ends), not the farthest. Earlier fixtures had
    /// exactly one function below the gap, so both coincided.
    ///
    /// ```text
    ///   0x80000000  `first`  (declared, size 0x8)
    ///   0x80000008  `second` (declared, size 0x8)  <- the NEAREST below
    ///   0x80000010  addiu $sp, $sp, -0x18          <- root / true gap start
    ///   0x80000014  jr $ra
    ///   0x80000018  nop                               (delay slot)
    ///   0x8000001C  `tail`   (declared, size 4)
    /// ```
    #[test]
    fn the_gap_starts_at_the_nearest_declared_function_below_the_root() {
        let words = [
            jal(0x8000_0010),
            NOP,
            JR_RA,
            NOP,
            0x27BD_FFE8,
            JR_RA,
            NOP,
            NOP,
        ];
        let functions = vec![
            DumpFunction {
                name: "first".into(),
                vram: 0x8000_0000,
                size: 0x8,
            },
            DumpFunction {
                name: "second".into(),
                vram: 0x8000_0008,
                size: 0x8,
            },
            DumpFunction {
                name: "tail".into(),
                vram: 0x8000_001C,
                size: 0x4,
            },
        ];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1);
        assert_eq!(
            check.uncovered[0].gap_start, 0x8000_0010,
            "the gap must start after `second`, not back at `first`"
        );
        // And with the correct start the root IS at the start, so it adopts.
        assert_eq!(check.uncovered[0].refusal, None);
        assert_eq!(check.uncovered[0].adopted_size(), 0xC);
    }

    /// MUTATION GUARD for `apply_gap_adoptions`' overlap check. If the
    /// function list already claims the range (for instance because another
    /// repair ran first, or the caller mutated it after the check), adoption
    /// must decline rather than push a second, overlapping entry.
    #[test]
    fn adoption_declines_when_the_range_is_already_claimed() {
        let (words, functions) = uncovered_fixture();
        let region = CodeRegion {
            name: "bank4_text".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        assert_eq!(check.uncovered.len(), 1);
        assert_eq!(check.uncovered[0].refusal, None, "adoptable in isolation");

        // Now simulate the range having been claimed since the check ran.
        let mut claimed = functions.clone();
        claimed.push(DumpFunction {
            name: "already_there".into(),
            vram: 0x8000_0010,
            size: 0xC,
        });
        claimed.sort_by_key(|f| f.vram);
        let before = claimed.clone();
        assert_eq!(
            apply_gap_adoptions(&mut claimed, &check),
            0,
            "must not double-claim an occupied range"
        );
        assert_eq!(claimed, before);
    }

    /// The diagnostic must NAME an uncovered entry and its evidence, so the
    /// build reports what the runtime would otherwise only discover as a
    /// trap 2,483 VI swaps into a run.
    #[test]
    fn the_diagnostic_names_uncovered_entries_and_their_evidence() {
        let (words, functions) = uncovered_fixture();
        let region = CodeRegion {
            name: "bank4_text".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let check = cross_check_region(&region, &functions);
        let text = check.render_diagnostic();
        assert!(text.contains("UNCOVERED-FUNCTION-ENTRY"), "{text}");
        assert!(text.contains("0x80000010"), "{text}");
        assert!(text.contains("bank4_text"), "{text}");
        assert!(text.contains("0x80000000"), "the jal site:\n{text}");
        assert!(text.contains("repair: ADOPT"), "{text}");
        assert!(!check.is_clean(), "an uncovered entry is not a clean check");
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

    /// MUTATION GUARD for the "already declared" filter specifically.
    ///
    /// The fixture above cannot distinguish the filter from the containment
    /// test, because its declared entry sits exactly at a function boundary
    /// and so falls strictly inside nobody. Here the proven root is BOTH
    /// declared as its own entry AND strictly inside an overlapping
    /// declaration -- the shape a partially-corrected dump has. Only the
    /// "already declared" filter can keep this clean; dropping it reports a
    /// function that is already perfectly dispatchable.
    #[test]
    fn a_declared_entry_inside_an_overlapping_declaration_is_still_clean() {
        let words = [jal(0x8000_0010), NOP, JR_RA, NOP, NOP, JR_RA, NOP];
        let region = CodeRegion {
            name: "sec".into(),
            vram: 0x8000_0000,
            words: &words,
        };
        let functions = vec![
            // Declared size still spans the whole region...
            DumpFunction {
                name: "outer".into(),
                vram: 0x8000_0000,
                size: 0x1C,
            },
            // ...but 0x80000010 is ALSO declared in its own right, so it
            // already reaches LOOKUP_TABLE and nothing is swallowed.
            DumpFunction {
                name: "inner".into(),
                vram: 0x8000_0010,
                size: 0xC,
            },
        ];
        let check = cross_check_region(&region, &functions);
        assert!(check.is_clean(), "{:?}", check.swallowed);
        assert_eq!(check.proven_roots, 1);
    }

    /// SUPERSEDED ASSERTION, kept as a guard. This test used to assert that
    /// a `jal` target claimed by no declared function was "a different
    /// (coverage) problem, deliberately not reported here" and left the
    /// check CLEAN. That assumption is exactly what let WM2000's
    /// `0x801226A0` reach a live run as a `lookup:` trap, so it no longer
    /// holds: such a target is now always REPORTED as uncovered.
    ///
    /// What is still true, and is what this test now pins, is that it is not
    /// a SWALLOWED entry and — for this fixture, whose gap never returns —
    /// not adoptable either.
    #[test]
    fn a_jal_target_inside_no_declared_function_is_reported_but_not_swallowed() {
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
        assert!(
            check.swallowed.is_empty(),
            "not the swallowed class: {:?}",
            check.swallowed
        );
        assert_eq!(check.uncovered.len(), 1, "but it MUST be reported");
        // The gap is `[nop, nop]` with no `jr $ra`, so it is not a
        // self-contained body and must not be adopted.
        assert_eq!(
            check.uncovered[0].refusal,
            Some(GapRefusal::GapDoesNotReturn)
        );
        assert!(!check.is_clean(), "silence is what shipped the trap");
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
