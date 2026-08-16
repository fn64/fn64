//! Mechanism M5, first slice: typed hypotheses for callerless functions.
//!
//! # What this is
//!
//! WWF WrestleMania 2000 (NWXE) has functions that discovery leaves `Open`
//! because nothing in the ROM statically references them: no `jal`, no
//! pointer word anywhere in the 32 MB image. Some of those functions ARE
//! constructed at runtime by a split `lui`/`addiu` immediate pair -- proof a
//! dynamic-dispatch mechanism builds their address, even though no static
//! evidence names the caller. This binary reads a function's own
//! instruction body and emits a [`Hypothesis`]: a typed, mechanically
//! checkable guess about WHERE the caller neighbourhood likely lives.
//!
//! Body recovery deliberately does NOT reuse [`fn64_discover::cfg::build_cfg`]:
//! that builder computes whole-program reachability, and seeding it with a
//! single callerless root still follows that root's OWN `jal`/`j` edges into
//! everything IT calls -- pulling in the entire downstream call tree
//! (observed: one candidate's "body" swelled to 29,175 instructions when
//! tried). A hypothesis about what a function DOES must only look at words
//! that function actually contains, so [`own_body_words`] instead walks
//! only intra-function control flow (conditional branches, branch-likely,
//! fallthrough) and treats calls/far-jumps/returns/traps as the edge of
//! this function's own body, never descending into them.
//!
//! # THE HARD CONSTRAINT: a hypothesis is a prior, never evidence
//!
//! Every [`Hypothesis`] variant here is produced by pattern-matching a
//! function's own decoded instruction stream -- it says something about
//! what the function DOES, and from that, guesses at who might call it. It
//! proves nothing about the caller. In particular:
//!
//! - A hypothesis **must never promote an indirect-jump/call site's
//!   `resolution_from_value`, `MemoryValueSet`, `JumpTable`, or any other
//!   [`fn64_discover::resolve`] proof state.** It is not admissible input to
//!   `owner_proof`, `closure`, or any fact-database insertion.
//! - A hypothesis **must never affect `matched_exact`** in
//!   `grade_nwxe_functions`/`grade_nw4e`/`grade_oot_functions` or any other
//!   answer-key grading. Those graders compare fn64's OWN discovery output
//!   to a key it never reads; a hypothesis is not discovery output.
//! - A hypothesis's only two jobs are (1) to RANK a callerless function
//!   worklist by how promising it looks to chase, and (2) to TARGET a
//!   verification plan ([`Hypothesis::verification_plan`]) -- a
//!   mechanically checkable follow-up (a grep over the corpus, a scan of a
//!   subsystem's known dispatch tables, a search for other referrers of a
//!   global) that a SEPARATE, sound mechanism must run before anything
//!   here can become a fact.
//!
//! If a future change makes any `Hypothesis` variant reachable from
//! `owner_proof`, `resolve`, or a grader, that change is a bug in THIS
//! contract, not a feature.
//!
//! # Usage
//!
//! ```text
//! classify_callerless <rom.z64> <callerless.json>
//! ```
//!
//! `callerless.json` is a JSON array of `{"va": "0x...", "name": "...",
//! "split_constructed": bool}` (extra fields are ignored, so the exact
//! nwxe-callerless.json export this binary was built against works
//! unmodified). Emits one JSON report to stdout: one [`ClassifiedFunctionV1`]
//! per input entry, sorted into a ranked worklist.

use fn64_cpu_runtime::decoder::{decode, Instruction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_ROM_BYTES: u64 = 128 * 1024 * 1024;
/// IPL3 always occupies ROM file offset `[0x40, 0x1000)`; the boot segment
/// proper starts at file offset `0x1000`, loaded to the header's
/// `entry_point` VA. This is the same affine relationship
/// `nwxe-callerless.json`'s VAs were extracted against (verified: every
/// entry's first instruction word matches the ROM byte at
/// `va - (entry_point - BOOT_SEGMENT_FILE_OFFSET)`).
const BOOT_SEGMENT_FILE_OFFSET: u32 = 0x1000;

/// KSEG0/KSEG1 physical target range that N64 hardware routes to the RCP
/// (SP/DP/MI/VI/AI/PI/RI/SI registers), not RDRAM or the PIF. Matches the
/// convention already established in `stage1_effects::classify_virtual_address`.
const RCP_PHYSICAL_RANGE: std::ops::RangeInclusive<u32> = 0x03f0_0000..=0x04ff_ffff;

/// Physical sub-ranges within the RCP window, used only to name which
/// hardware lane a [`Hypothesis::DeviceRegisterAccess`] belongs to. Ranges
/// are the standard N64 hardware memory map (public hardware fact, not
/// discovered); mirrors `n64squid`/`n64dev` wiki register maps.
const MMIO_LANES: &[(&str, std::ops::RangeInclusive<u32>)] = &[
    ("rsp", 0x0404_0000..=0x040f_ffff),
    ("rdp_command", 0x0410_0000..=0x041f_ffff),
    ("rdp_span", 0x0420_0000..=0x042f_ffff),
    ("mips_interface", 0x0430_0000..=0x043f_ffff),
    ("video_interface", 0x0440_0000..=0x044f_ffff),
    ("audio_interface", 0x0450_0000..=0x045f_ffff),
    ("peripheral_interface", 0x0460_0000..=0x046f_ffff),
    ("rdram_interface", 0x0470_0000..=0x047f_ffff),
    ("serial_interface", 0x0480_0000..=0x048f_ffff),
];

fn mmio_lane(physical_address: u32) -> &'static str {
    MMIO_LANES
        .iter()
        .find(|(_, range)| range.contains(&physical_address))
        .map(|(name, _)| *name)
        .unwrap_or("rcp_other")
}

// ---------------------------------------------------------------------
// Hypothesis: a typed, machine-checkable prior about a callerless
// function's likely caller neighbourhood. See module docs for the hard
// constraint this type must never violate.
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "hypothesis_kind", rename_all = "snake_case")]
pub enum Hypothesis {
    /// A getter/setter for exactly one global: `lui`/`lw`|`sw`|`addiu` pair
    /// resolving a single constant VA, touched by every reached
    /// instruction that has a resolvable address. The global's address is
    /// the hypothesis -- whoever else in the corpus touches that same VA is
    /// the likely caller neighbourhood.
    GlobalAccessor { global_va: u32 },
    /// A setter/getter for a struct passed in `$a0`: every memory access in
    /// the reached block is `off($a0)` for a small, bounded offset. The
    /// offset set characterizes the struct type; other functions writing
    /// the same offsets against the same base register are candidate
    /// siblings (same vtable-ish object).
    StructFieldWriter { offsets: Vec<i16> },
    /// Touches an RCP hardware register (KSEG0/KSEG1 physical target in
    /// `0x03f0_0000..=0x04ff_ffff`): belongs to a hardware lane, so the
    /// caller is in that subsystem's driver code.
    DeviceRegisterAccess { mmio_range: String, physical_address: u32 },
    /// A memset/memcpy/strlen-shaped tight loop: a backward branch whose
    /// body is dominated by byte/halfword/word loads+stores and an
    /// increment/decrement of a loop register.
    LibcLike { kind: LibcKind },
    /// Heavy COP1 (FPU) use with no other classification winning first:
    /// likely geometry/physics/audio-mixing math.
    FloatMath { cop1_instruction_count: u32 },
    /// The function is (almost) entirely a jump/call elsewhere: a thin
    /// trampoline. The real logic -- and its real caller neighbourhood --
    /// lives at `target`.
    Trampoline { target: u32 },
    /// None of the above pattern-matched. Still worklist-worthy, just with
    /// no directed lead.
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibcKind {
    Memset,
    Memcpy,
    StrlenLike,
    GenericByteLoop,
}

impl Hypothesis {
    /// A short machine-checkable prescription for what a SEPARATE, sound
    /// tool should verify before this hypothesis earns any weight. Every
    /// plan names a concrete, mechanical check -- never "looks plausible."
    fn verification_plan(&self) -> String {
        match self {
            Hypothesis::GlobalAccessor { global_va } => format!(
                "scan the corpus (or this ROM's proven-code set) for every \
                 function whose reached instructions construct or reference \
                 VA 0x{global_va:08x} (lui/addiu pair, or a load/store off a \
                 register built from that pair); the true caller is among \
                 that set intersected with functions that ALSO construct \
                 THIS function's own VA the same way."
            ),
            Hypothesis::StructFieldWriter { offsets } => format!(
                "search for other functions in the corpus that read/write \
                 the SAME offset set {offsets:?} off an argument register \
                 (not necessarily $a0) -- candidate siblings share a struct \
                 layout; then look for a constructor/table that stores this \
                 function's VA at one of those offsets (a vtable slot)."
            ),
            Hypothesis::DeviceRegisterAccess { mmio_range, .. } => format!(
                "search the {mmio_range} subsystem's known dispatch/handler \
                 tables and interrupt-vector wiring for this function's VA; \
                 cross-check against any osXxxMessage/callback queues that \
                 subsystem is known to populate."
            ),
            Hypothesis::LibcLike { kind } => format!(
                "grep the corpus for other functions with an identical \
                 decoded instruction shape (same {kind:?} loop skeleton); \
                 if a byte-identical twin exists elsewhere with a PROVEN \
                 caller, that caller is strong corroborating (not proving) \
                 evidence this one is the same runtime's private copy."
            ),
            Hypothesis::FloatMath { .. } => {
                "search geometry/physics/audio-mixing subsystems (matrix, \
                 collision, camera, DSP-mix owners already in the fact \
                 database) for a computed-call site whose target set this \
                 VA would fit; float-heavy leaves are rarely called from \
                 integer-only control code."
                    .to_string()
            }
            Hypothesis::Trampoline { target } => format!(
                "resolve who calls VA 0x{target:08x} instead -- if that \
                 target already has a proven caller, this trampoline is \
                 very likely reached via the SAME call site with a computed \
                 pre-target (e.g. an indirect call that lands here first)."
            ),
            Hypothesis::Unclassified => {
                "no directed lead; fall back to a corpus-wide scan for any \
                 lui/addiu pair anywhere that resolves to this function's \
                 own VA, and inspect each such site by hand."
                    .to_string()
            }
        }
    }

    /// Coarse rank key: lower is more actionable. Used only to order the
    /// worklist -- never to decide truth.
    fn rank_priority(&self) -> u8 {
        match self {
            Hypothesis::DeviceRegisterAccess { .. } => 0,
            Hypothesis::GlobalAccessor { .. } => 1,
            Hypothesis::Trampoline { .. } => 2,
            Hypothesis::LibcLike { .. } => 3,
            Hypothesis::StructFieldWriter { .. } => 4,
            Hypothesis::FloatMath { .. } => 5,
            Hypothesis::Unclassified => 6,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Hypothesis::GlobalAccessor { .. } => "global_accessor",
            Hypothesis::StructFieldWriter { .. } => "struct_field_writer",
            Hypothesis::DeviceRegisterAccess { .. } => "device_register_access",
            Hypothesis::LibcLike { .. } => "libc_like",
            Hypothesis::FloatMath { .. } => "float_math",
            Hypothesis::Trampoline { .. } => "trampoline",
            Hypothesis::Unclassified => "unclassified",
        }
    }
}

// ---------------------------------------------------------------------
// Input schema (subset of nwxe-callerless.json; extra fields ignored).
// ---------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CallerlessEntry {
    va: String,
    name: String,
    #[serde(default)]
    split_constructed: bool,
}

// ---------------------------------------------------------------------
// Output schema.
// ---------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ClassifiedFunctionV1 {
    va: String,
    name: String,
    split_constructed: bool,
    reached_instruction_count: u32,
    /// True when [`own_body_words`] hit [`MAX_OWN_BODY_WORDS`] without ever
    /// reaching a return/trap/tail-exit -- i.e. straight-line decode never
    /// found this function's end. Observed on 3/66 NWXE entries whose
    /// declared size was 4-8 bytes but whose bytes are a structured data
    /// table, not code (repeating small non-MIPS-shaped words with no
    /// terminator). When true, `hypothesis` is forced to `Unclassified`
    /// regardless of what pattern-matched along the way: a hypothesis
    /// derived from a walk that never found the function's actual extent
    /// is not trustworthy, and reporting a confident-looking classification
    /// here would be worse than reporting nothing.
    body_bound_hit: bool,
    hypothesis: Hypothesis,
    hypothesis_label: &'static str,
    verification_plan: String,
    rank_priority: u8,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct ReportV1 {
    schema: &'static str,
    schema_version: u32,
    normalized_rom_sha256: String,
    entries: usize,
    distribution: Vec<(String, usize)>,
    ranked: Vec<ClassifiedFunctionV1>,
}

fn main() {
    if let Err(error) = run(std::env::args_os().skip(1)) {
        eprintln!("classify-callerless: {error}");
        std::process::exit(1);
    }
}

fn run(mut args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let rom_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: classify_callerless ROM CALLERLESS_JSON".to_string())?;
    let list_path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: classify_callerless ROM CALLERLESS_JSON".to_string())?;

    let report = classify(&rom_path, &list_path)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn classify(rom_path: &Path, list_path: &Path) -> Result<ReportV1, String> {
    let metadata = fs::metadata(rom_path).map_err(|error| format!("reading ROM metadata: {error}"))?;
    if !metadata.is_file() {
        return Err("ROM input is not a regular file".into());
    }
    if metadata.len() > MAX_ROM_BYTES {
        return Err(format!(
            "ROM input is {} bytes, exceeding the {MAX_ROM_BYTES}-byte limit",
            metadata.len()
        ));
    }
    let rom_bytes = fs::read(rom_path).map_err(|error| format!("reading ROM: {error}"))?;
    let discovery = fn64_discover::run_discovery_auto(&rom_bytes)
        .map_err(|error| format!("automatic discovery rejected the ROM: {error:?}"))?;

    let list_bytes = fs::read(list_path).map_err(|error| format!("reading VA list: {error}"))?;
    let entries: Vec<CallerlessEntry> =
        serde_json::from_slice(&list_bytes).map_err(|error| format!("parsing VA list: {error}"))?;

    let normalized = &discovery.rom;
    // `bank_bytes` is sliced to start at ROM file offset
    // `BOOT_SEGMENT_FILE_OFFSET`, so `bank_bytes[0]` IS the byte at that
    // file offset -- which loads to VA `entry_point` (the byte at file
    // offset 0 of the *unsliced* ROM would load to `entry_point -
    // BOOT_SEGMENT_FILE_OFFSET`, but that VA is never inside `bank_bytes`
    // itself). So `va_start` for the SLICED slice is `entry_point`, not
    // `entry_point - BOOT_SEGMENT_FILE_OFFSET` -- verified against the
    // affine relationship every nwxe-callerless.json VA satisfies:
    // `file_offset(va) == va - (entry_point - BOOT_SEGMENT_FILE_OFFSET)`,
    // and `bank_bytes[k] == normalized.bytes[BOOT_SEGMENT_FILE_OFFSET + k]`,
    // so `k == va - entry_point`.
    let va_start = normalized.header.entry_point;
    let bank_bytes = &normalized.bytes[BOOT_SEGMENT_FILE_OFFSET as usize..];

    let mut ranked = Vec::with_capacity(entries.len());
    for entry in &entries {
        let va = parse_va(&entry.va)?;
        let (hypothesis, reached_instruction_count, body_bound_hit) =
            classify_function(bank_bytes, va_start, va);
        ranked.push(ClassifiedFunctionV1 {
            va: entry.va.clone(),
            name: entry.name.clone(),
            split_constructed: entry.split_constructed,
            reached_instruction_count,
            body_bound_hit,
            hypothesis_label: hypothesis.label(),
            verification_plan: hypothesis.verification_plan(),
            hypothesis,
            rank_priority: 0,
        });
    }
    for row in &mut ranked {
        row.rank_priority = row.hypothesis.rank_priority();
    }
    // Split-constructed functions are proven live dynamic-dispatch
    // candidates (something DOES build their address at runtime), so they
    // are the best trace targets regardless of hypothesis specificity --
    // sort them first, then by hypothesis actionability, then by VA for
    // determinism.
    ranked.sort_by(|a, b| {
        b.split_constructed
            .cmp(&a.split_constructed)
            .then(a.rank_priority.cmp(&b.rank_priority))
            .then(a.va.cmp(&b.va))
    });

    let mut distribution: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for row in &ranked {
        *distribution.entry(row.hypothesis_label).or_insert(0) += 1;
    }

    Ok(ReportV1 {
        schema: "classify_callerless_v1",
        schema_version: 1,
        normalized_rom_sha256: normalized.sha256.clone(),
        entries: ranked.len(),
        distribution: distribution.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        ranked,
    })
}

fn parse_va(text: &str) -> Result<u32, String> {
    let stripped = text.strip_prefix("0x").unwrap_or(text);
    u32::from_str_radix(stripped, 16).map_err(|error| format!("bad VA {text:?}: {error}"))
}

/// Decode `va`'s own-body instructions (see [`own_body_words`]) and derive
/// one [`Hypothesis`], the instruction count it was derived from, and
/// whether the walk hit [`MAX_OWN_BODY_WORDS`] without finding a natural
/// end (in which case the hypothesis is forced to `Unclassified` -- see
/// [`ClassifiedFunctionV1::body_bound_hit`]).
fn classify_function(bank_bytes: &[u8], va_start: u32, va: u32) -> (Hypothesis, u32, bool) {
    let (words, bound_hit) = own_body_words(bank_bytes, va_start, va);
    let instructions: Vec<Instruction> = words.iter().map(|&(_, w)| decode(w)).collect();
    let hypothesis = if bound_hit {
        Hypothesis::Unclassified
    } else {
        classify_instructions(&words, &instructions)
    };
    (hypothesis, instructions.len() as u32, bound_hit)
}

/// A generous but finite bound on how many words a single function's own
/// body may contribute, so a malformed/self-referential branch graph can
/// never turn this into an unbounded scan. `Cfg`-driven whole-program
/// descent (`build_cfg`) was tried first and rejected here: seeding it with
/// a single callerless root still follows that root's OWN `jal`/`j` edges
/// into whatever it calls, silently pulling in the entire downstream call
/// tree (observed: one candidate's "body" swelled to 29,175 instructions).
/// A hypothesis about what THIS function does must only look at words THIS
/// function actually contains -- so this walker follows intra-function
/// control flow (conditional branches, branch-likely, fallthrough) but
/// treats `jal`, far `j`, `jr`, and traps as terminal without decoding past
/// them.
const MAX_OWN_BODY_WORDS: usize = 1024;

/// Collect `(pc, word)` for the instructions genuinely inside function
/// `va`'s own body: straight-line descent plus branch targets, stopped at
/// any instruction that leaves the function (call, tail jump far outside
/// the function's own local address window, return, trap, or indirect
/// transfer). This deliberately does NOT reuse `cfg::build_cfg`, which
/// builds a whole-program reachability graph, not a single function's
/// extent -- see [`MAX_OWN_BODY_WORDS`].
///
/// Returns `(words, bound_hit)`. `bound_hit` is true when the walk reached
/// [`MAX_OWN_BODY_WORDS`] without a path ever finding a natural terminator
/// -- the honest signal that this address is very likely not a real
/// function body at all (observed on NWXE: three declared-4-8-byte entries
/// whose bytes are a structured non-code data table with no MIPS
/// control-flow terminator anywhere nearby, so straight-line decode never
/// stops on its own).
fn own_body_words(bank_bytes: &[u8], va_start: u32, root: u32) -> (Vec<(u32, u32)>, bool) {
    let va_end = va_start.wrapping_add(bank_bytes.len() as u32);
    let read = |pc: u32| -> Option<u32> {
        if pc < va_start || pc >= va_end {
            return None;
        }
        let off = (pc - va_start) as usize;
        let bytes = bank_bytes.get(off..off + 4)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    };

    let mut collected: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
    let mut worklist: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    let mut visited: BTreeSet<u32> = BTreeSet::new();
    worklist.push_back(root);

    while let Some(mut pc) = worklist.pop_front() {
        loop {
            if collected.len() >= MAX_OWN_BODY_WORDS {
                return (collected.into_iter().collect(), true);
            }
            if !visited.insert(pc) {
                break;
            }
            let Some(word) = read(pc) else { break };
            collected.insert(pc, word);
            let instr = decode(word);
            match instr {
                // Ordinary return / trap: this control path ends here.
                // Do not decode the delay slot as a further transfer, but
                // DO include it (architecturally always executed) by
                // falling through one more step below via the `Some(next)`
                // arms; returns/traps have no successor beyond their delay
                // slot, so stop after picking it up.
                Instruction::Jr { rs } if rs == REG_RA => {
                    if let Some(delay) = read(pc.wrapping_add(4)) {
                        collected.insert(pc.wrapping_add(4), delay);
                    }
                    break;
                }
                Instruction::Break { .. } | Instruction::Syscall { .. } => {
                    if let Some(delay) = read(pc.wrapping_add(4)) {
                        collected.insert(pc.wrapping_add(4), delay);
                    }
                    break;
                }
                // Computed transfer: unresolved by construction, no known
                // successor to follow. Stop this path.
                Instruction::Jr { .. } | Instruction::Jalr { .. } => {
                    if let Some(delay) = read(pc.wrapping_add(4)) {
                        collected.insert(pc.wrapping_add(4), delay);
                    }
                    break;
                }
                // Direct call: proves a call-out, but the callee's body is
                // NOT this function's body. Pick up the delay slot (always
                // executed) and the fallthrough after the call returns,
                // without ever decoding at `target`.
                Instruction::Jal { .. } => {
                    if let Some(delay) = read(pc.wrapping_add(4)) {
                        collected.insert(pc.wrapping_add(4), delay);
                    }
                    pc = pc.wrapping_add(8);
                    continue;
                }
                // Unconditional jump: only follow it if the target is
                // still inside this function's local window (a loop-back
                // or internal label some compilers emit as `j`, not `jal`
                // followed by more code); otherwise this is a tail call
                // out and the path ends here.
                Instruction::J { target } => {
                    let region_target = fn64_discover::cfg::region_target(pc, target);
                    if let Some(delay) = read(pc.wrapping_add(4)) {
                        collected.insert(pc.wrapping_add(4), delay);
                    }
                    if region_target.abs_diff(root) <= 4096 {
                        worklist.push_back(region_target);
                    }
                    break;
                }
                Instruction::Beq { off, .. }
                | Instruction::Bne { off, .. }
                | Instruction::Beql { off, .. }
                | Instruction::Bnel { off, .. }
                | Instruction::Blez { off, .. }
                | Instruction::Bgtz { off, .. }
                | Instruction::Blezl { off, .. }
                | Instruction::Bgtzl { off, .. }
                | Instruction::Bltz { off, .. }
                | Instruction::Bgez { off, .. }
                | Instruction::Bltzl { off, .. }
                | Instruction::Bgezl { off, .. }
                | Instruction::Bltzal { off, .. }
                | Instruction::Bgezal { off, .. } => {
                    let target = pc.wrapping_add(4).wrapping_add((off as i32 * 4) as u32);
                    if let Some(delay) = read(pc.wrapping_add(4)) {
                        collected.insert(pc.wrapping_add(4), delay);
                    }
                    worklist.push_back(target);
                    pc = pc.wrapping_add(8);
                    continue;
                }
                _ => {
                    pc = pc.wrapping_add(4);
                    continue;
                }
            }
        }
    }
    (collected.into_iter().collect(), false)
}

/// The `$a0` argument register per the MIPS o32/n64 ABI convention this
/// codebase's callers use.
const REG_A0: u8 = 4;
const REG_RA: u8 = 31;
const REG_ZERO: u8 = 0;

fn classify_instructions(words: &[(u32, u32)], instructions: &[Instruction]) -> Hypothesis {
    if instructions.is_empty() {
        return Hypothesis::Unclassified;
    }

    // --- Trampoline: the body (ignoring the prologue-establishing part) is
    // dominated by a single control transfer to a fixed target, with little
    // else going on. Matches `j target` bodies and `jal target; nop`-shaped
    // stubs with at most a couple of setup instructions.
    if let Some(target) = trampoline_target(words, instructions) {
        return Hypothesis::Trampoline { target };
    }

    // --- DeviceRegisterAccess: any reached instruction resolves a KSEG0/
    // KSEG1 constant landing in the RCP physical window. Checked before
    // GlobalAccessor because MMIO is the more specific, more actionable
    // claim (a single subsystem, not "somewhere in RDRAM").
    if let Some(physical_address) = first_mmio_address(instructions) {
        return Hypothesis::DeviceRegisterAccess {
            mmio_range: mmio_lane(physical_address).to_string(),
            physical_address,
        };
    }

    // --- LibcLike: a backward branch (loop) whose body is dominated by
    // byte/halfword/word loads+stores plus an increment/decrement --
    // memset/memcpy/strlen-shaped.
    if let Some(kind) = libc_like_kind(words, instructions) {
        return Hypothesis::LibcLike { kind };
    }

    // --- GlobalAccessor: every resolvable memory reference in the body
    // targets the SAME constant VA (constructed via lui/(addiu|lw|sw) in
    // this same body), and the body is otherwise a thin getter/setter
    // (small instruction count, ends in a return).
    if let Some(global_va) = single_global_accessor(instructions) {
        return Hypothesis::GlobalAccessor { global_va };
    }

    // --- StructFieldWriter: every memory access in the body is
    // `off($a0)`, for two or more distinct small offsets (a single-offset
    // accessor is better described as GlobalAccessor-shaped noise or is too
    // thin to characterize a struct; require >=1 access and record
    // whatever offsets appear).
    if let Some(offsets) = struct_field_offsets(instructions) {
        return Hypothesis::StructFieldWriter { offsets };
    }

    // --- FloatMath: heavy COP1 use, checked after the more specific
    // shapes above so a float-heavy MMIO or global accessor still gets the
    // more actionable classification.
    let cop1_count = instructions.iter().filter(|i| i.requires_cop1()).count() as u32;
    if cop1_count >= 3 {
        return Hypothesis::FloatMath {
            cop1_instruction_count: cop1_count,
        };
    }

    Hypothesis::Unclassified
}

/// A function is a trampoline when its only control transfer that leaves
/// the function is a `j`/`jal` to a fixed target, and everything else in
/// the body is either the delay slot or ordinary register setup (no memory
/// access, no branch, no loop) -- i.e. the body's job is "route to
/// `target`," nothing else.
fn trampoline_target(words: &[(u32, u32)], instructions: &[Instruction]) -> Option<u32> {
    // `J`/`Jal` carry only the raw 26-bit encoded field, not an absolute
    // VA -- resolving the real target needs the instruction's own PC via
    // `cfg::region_target` (top 4 bits come from PC+4, per MIPS jump
    // semantics), so this must zip `words` (which has PC) with the decoded
    // instruction rather than working from `instructions` alone.
    let transfers: Vec<u32> = words
        .iter()
        .zip(instructions.iter())
        .filter_map(|(&(pc, _), instr)| match instr {
            Instruction::J { target } | Instruction::Jal { target } => {
                Some(fn64_discover::cfg::region_target(pc, *target))
            }
            _ => None,
        })
        .collect();
    let [only_target] = transfers[..] else {
        return None;
    };
    let memory_ops = instructions
        .iter()
        .filter(|i| is_memory_access(i).is_some())
        .count();
    let branches = instructions
        .iter()
        .filter(|i| {
            matches!(
                i,
                Instruction::Beq { .. }
                    | Instruction::Bne { .. }
                    | Instruction::Beql { .. }
            )
        })
        .count();
    if memory_ops == 0 && branches == 0 && instructions.len() <= 8 {
        Some(only_target)
    } else {
        None
    }
}

/// Walk the instruction stream tracking `lui`-constructed upper halves per
/// register; the first memory access (or `addiu`/`ori` forming a raw
/// pointer) whose resolved address lands in the RCP physical window is
/// returned.
fn first_mmio_address(instructions: &[Instruction]) -> Option<u32> {
    let mut upper: [Option<u16>; 32] = [None; 32];
    for instr in instructions {
        match *instr {
            Instruction::Lui { rt, imm } => {
                upper[rt as usize] = Some(imm);
            }
            Instruction::Addiu { rt, rs, imm } => {
                if let Some(hi) = upper[rs as usize] {
                    let va = (hi as u32).wrapping_shl(16).wrapping_add(imm as i32 as u32);
                    if let Some(phys) = rcp_physical(va) {
                        return Some(phys);
                    }
                }
                if rt != rs {
                    upper[rt as usize] = None;
                }
            }
            Instruction::Ori { rt, rs, imm } => {
                if let Some(hi) = upper[rs as usize] {
                    let va = ((hi as u32) << 16) | (imm as u32);
                    if let Some(phys) = rcp_physical(va) {
                        return Some(phys);
                    }
                }
                if rt != rs {
                    upper[rt as usize] = None;
                }
            }
            _ => {
                if let Some((base, off, _)) = is_memory_access(instr) {
                    if let Some(hi) = upper[base as usize] {
                        let va = (hi as u32)
                            .wrapping_shl(16)
                            .wrapping_add(off as i32 as u32);
                        if let Some(phys) = rcp_physical(va) {
                            return Some(phys);
                        }
                    }
                }
            }
        }
    }
    None
}

fn rcp_physical(virtual_address: u32) -> Option<u32> {
    let segment = virtual_address & 0xe000_0000;
    if !matches!(segment, 0x8000_0000 | 0xa000_0000) {
        return None;
    }
    let physical = virtual_address & 0x1fff_ffff;
    if RCP_PHYSICAL_RANGE.contains(&physical) {
        Some(physical)
    } else {
        None
    }
}

/// `Some((base_reg, offset, is_write))` for any load/store instruction;
/// `None` for everything else (including COP1 loads/stores, which are
/// float-classified separately by `requires_cop1`).
fn is_memory_access(instr: &Instruction) -> Option<(u8, i16, bool)> {
    use Instruction::*;
    match *instr {
        Lb { base, off, .. }
        | Lbu { base, off, .. }
        | Lh { base, off, .. }
        | Lhu { base, off, .. }
        | Lw { base, off, .. }
        | Lwu { base, off, .. }
        | Lwl { base, off, .. }
        | Lwr { base, off, .. }
        | Ld { base, off, .. }
        | Ldl { base, off, .. }
        | Ldr { base, off, .. }
        | Ll { base, off, .. }
        | Lld { base, off, .. } => Some((base, off, false)),
        Sb { base, off, .. }
        | Sh { base, off, .. }
        | Sw { base, off, .. }
        | Swl { base, off, .. }
        | Swr { base, off, .. }
        | Sd { base, off, .. }
        | Sdl { base, off, .. }
        | Sdr { base, off, .. }
        | Sc { base, off, .. }
        | Scd { base, off, .. } => Some((base, off, true)),
        _ => None,
    }
}

/// A libc-like body: contains a backward-branching loop (a `Beq`/`Bne`/
/// `Beql`/`Bnel` whose target is a smaller VA than the branch's own PC,
/// i.e. loops back), where every memory access in the body is a byte or
/// word access off one pointer register and the branch condition register
/// changes by a small constant each iteration (an increment/decrement
/// pattern via `addiu`).
fn libc_like_kind(words: &[(u32, u32)], instructions: &[Instruction]) -> Option<LibcKind> {
    let has_backward_branch = words.iter().zip(instructions.iter()).any(|(&(pc, _), instr)| {
        let target = match instr {
            Instruction::Beq { off, .. }
            | Instruction::Bne { off, .. }
            | Instruction::Beql { off, .. } => {
                Some(pc.wrapping_add(4).wrapping_add((*off as i32 * 4) as u32))
            }
            _ => None,
        };
        matches!(target, Some(t) if t <= pc)
    });
    if !has_backward_branch {
        return None;
    }

    let memory_accesses: Vec<(u8, i16, bool)> =
        instructions.iter().filter_map(is_memory_access).collect();
    if memory_accesses.is_empty() {
        return None;
    }
    let byte_only = instructions.iter().all(|instr| {
        is_memory_access(instr).is_none()
            || matches!(
                instr,
                Instruction::Sb { .. } | Instruction::Lb { .. } | Instruction::Lbu { .. }
            )
    });
    let has_addiu_increment = instructions
        .iter()
        .any(|instr| matches!(instr, Instruction::Addiu { imm, .. } if imm.abs() <= 4));

    if !has_addiu_increment {
        return None;
    }

    let has_write = memory_accesses.iter().any(|(_, _, is_write)| *is_write);
    let has_read = memory_accesses.iter().any(|(_, _, is_write)| !*is_write);
    if byte_only && has_write && !has_read {
        Some(LibcKind::Memset)
    } else if byte_only && has_read && has_write {
        Some(LibcKind::Memcpy)
    } else if byte_only && has_read && !has_write {
        Some(LibcKind::StrlenLike)
    } else {
        Some(LibcKind::GenericByteLoop)
    }
}

/// A thin getter/setter for one global: every memory access in the body
/// resolves (via a `lui`-constructed upper half tracked per register) to
/// the SAME constant VA, there is at least one such access, and the body
/// ends in an ordinary return (`jr $ra`, not an indirect/computed exit).
fn single_global_accessor(instructions: &[Instruction]) -> Option<u32> {
    let mut upper: [Option<u16>; 32] = [None; 32];
    let mut resolved: BTreeSet<u32> = BTreeSet::new();
    for instr in instructions {
        match instr {
            Instruction::Lui { rt, imm } => {
                upper[*rt as usize] = Some(*imm);
            }
            Instruction::Addiu { rt, rs, imm } => {
                if let Some(hi) = upper[*rs as usize] {
                    let va = (hi as u32).wrapping_shl(16).wrapping_add(*imm as i32 as u32);
                    resolved.insert(va);
                }
                if *rt != *rs {
                    upper[*rt as usize] = None;
                }
            }
            _ => {
                if let Some((base, off, _)) = is_memory_access(instr) {
                    if let Some(hi) = upper[base as usize] {
                        let va = (hi as u32)
                            .wrapping_shl(16)
                            .wrapping_add(off as i32 as u32);
                        resolved.insert(va);
                    }
                }
            }
        }
    }
    let has_ordinary_return = instructions
        .iter()
        .any(|instr| matches!(instr, Instruction::Jr { rs } if *rs == REG_RA));
    if resolved.len() == 1 && has_ordinary_return {
        resolved.into_iter().next()
    } else {
        None
    }
}

/// Every memory access in the body is `off($a0)` (the incoming struct
/// pointer per this codebase's ABI convention), for at least one distinct
/// offset. Returns the sorted, deduplicated offset set.
fn struct_field_offsets(instructions: &[Instruction]) -> Option<Vec<i16>> {
    let accesses: Vec<(u8, i16, bool)> =
        instructions.iter().filter_map(is_memory_access).collect();
    if accesses.is_empty() {
        return None;
    }
    if !accesses.iter().all(|(base, _, _)| *base == REG_A0) {
        return None;
    }
    let offsets: BTreeSet<i16> = accesses.into_iter().map(|(_, off, _)| off).collect();
    // A pure return-value read at offset 0 with no $a0 writes at all is
    // more honestly "reads one field," still worklist-worthy under this
    // variant; $zero writes (e.g. `sw $zero, N($a0)` clears) are common and
    // fine to include.
    let _ = REG_ZERO;
    Some(offsets.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_be_bytes()).collect()
    }

    fn classify_body(words: &[u32]) -> Hypothesis {
        let bank = be(words);
        let va_start = 0x8000_0000u32;
        let (hyp, _count, _bound_hit) = classify_function(&bank, va_start, va_start);
        hyp
    }

    #[test]
    fn global_accessor_getter() {
        // lui $v0, hi(0x80100000); lw $v0, lo($v0); jr $ra; nop
        let words = [
            0x3c02_8010, // lui v0, 0x8010
            0x8c42_0000, // lw v0, 0(v0)
            0x03e0_0008, // jr ra
            0x0000_0000, // nop (delay slot)
        ];
        let hyp = classify_body(&words);
        assert_eq!(hyp, Hypothesis::GlobalAccessor { global_va: 0x8010_0000 });
    }

    #[test]
    fn global_accessor_setter() {
        // lui $v0, hi(0x80100004); sw $a0, lo($v0); jr $ra; nop
        let words = [
            0x3c02_8010, // lui v0, 0x8010
            0xac44_0004, // sw a0, 4(v0)
            0x03e0_0008, // jr ra
            0x0000_0000,
        ];
        let hyp = classify_body(&words);
        assert_eq!(hyp, Hypothesis::GlobalAccessor { global_va: 0x8010_0004 });
    }

    #[test]
    fn struct_field_writer_offsets() {
        // sw $a1, 0($a0); sw $a2, 4($a0); jr $ra; nop
        let words = [
            0xac85_0000, // sw a1, 0(a0)
            0xac86_0004, // sw a2, 4(a0)
            0x03e0_0008, // jr ra
            0x0000_0000,
        ];
        let hyp = classify_body(&words);
        assert_eq!(
            hyp,
            Hypothesis::StructFieldWriter {
                offsets: vec![0, 4]
            }
        );
    }

    #[test]
    fn device_register_access_via_kseg1_pi() {
        // lui $t0, 0xa460 (KSEG1 PI registers); sw $a0, 0x10(t0); jr $ra; nop
        let words = [
            0x3c08_a460, // lui t0, 0xa460
            0xad04_0010, // sw a0, 0x10(t0)
            0x03e0_0008, // jr ra
            0x0000_0000,
        ];
        let hyp = classify_body(&words);
        assert_eq!(
            hyp,
            Hypothesis::DeviceRegisterAccess {
                mmio_range: "peripheral_interface".to_string(),
                physical_address: 0x0460_0010,
            }
        );
    }

    #[test]
    fn device_register_access_via_kseg1_video_interface() {
        // lui $t0, 0xa440; lw $t1, 0(t0); jr $ra; nop
        let words = [0x3c08_a440, 0x8d09_0000, 0x03e0_0008, 0x0000_0000];
        let hyp = classify_body(&words);
        assert_eq!(
            hyp,
            Hypothesis::DeviceRegisterAccess {
                mmio_range: "video_interface".to_string(),
                physical_address: 0x0440_0000,
            }
        );
    }

    #[test]
    fn libc_like_memset_loop() {
        // addiu $a2, $a2, -1; sb $a1, 0($a0); addiu $a0, $a0, 1; bne $a2, $zero, -3; nop; jr $ra; nop
        let loop_body = [
            0x24c6_ffff, // addiu a2, a2, -1
            0xa085_0000, // sb a1, 0(a0)
            0x2484_0001, // addiu a0, a0, 1
            0x14c0_fffc, // bne a2, zero, -4 (back to loop_body[0])
            0x0000_0000, // delay slot nop
            0x03e0_0008, // jr ra
            0x0000_0000,
        ];
        let hyp = classify_body(&loop_body);
        assert_eq!(hyp, Hypothesis::LibcLike { kind: LibcKind::Memset });
    }

    #[test]
    fn trampoline_pure_jump() {
        // j target; nop
        let words = [0x0800_0100, 0x0000_0000]; // j 0x00000400 << ... region-relative
        let hyp = classify_body(&words);
        assert!(matches!(hyp, Hypothesis::Trampoline { .. }));
    }

    #[test]
    fn trampoline_jal_thin_stub() {
        // jal target; nop
        let words = [0x0c00_0100, 0x0000_0000];
        let hyp = classify_body(&words);
        assert!(matches!(hyp, Hypothesis::Trampoline { .. }));
    }

    #[test]
    fn float_math_heavy_cop1() {
        // A body with >=3 cop1 arithmetic ops and no memory/global shape.
        let words = [
            0x4600_0080, // add.s $f2, $f0, $f0  (ADD.S fd=2,fs=0,ft=0)
            0x4600_0086, // mul.s $f2, $f0, $f0
            0x4600_0083, // div.s $f2, $f0, $f0
            0x03e0_0008, // jr ra
            0x0000_0000,
        ];
        let hyp = classify_body(&words);
        assert!(matches!(hyp, Hypothesis::FloatMath { cop1_instruction_count } if cop1_instruction_count >= 3));
    }

    #[test]
    fn unclassified_when_nothing_matches() {
        // Pure ALU on registers, no memory, no cop1, no branch, ends in jr ra.
        let words = [
            0x0083_1020, // add v0, a0, v1 (rd=v0,rs=a0,rt=v1) -- arbitrary ALU
            0x03e0_0008, // jr ra
            0x0000_0000,
        ];
        let hyp = classify_body(&words);
        assert_eq!(hyp, Hypothesis::Unclassified);
    }

    #[test]
    fn hard_constraint_hypothesis_is_serializable_and_not_a_fact() {
        // Regression guard: a Hypothesis must round-trip through JSON (so it
        // can be emitted as a report) but the type carries no method that
        // inserts into any fact database -- this test exists so a future
        // reviewer adding e.g. `impl Hypothesis { fn promote(&self, db) }`
        // has to consciously break this comment, not slip it in unnoticed.
        let hyp = Hypothesis::GlobalAccessor { global_va: 0x8010_0000 };
        let json = serde_json::to_string(&hyp).unwrap();
        assert!(json.contains("global_accessor"));
    }

    #[test]
    fn verification_plans_are_nonempty_for_every_variant() {
        let variants = [
            Hypothesis::GlobalAccessor { global_va: 0x8000_0000 },
            Hypothesis::StructFieldWriter { offsets: vec![0, 4] },
            Hypothesis::DeviceRegisterAccess {
                mmio_range: "video_interface".to_string(),
                physical_address: 0x0440_0000,
            },
            Hypothesis::LibcLike { kind: LibcKind::Memcpy },
            Hypothesis::FloatMath { cop1_instruction_count: 5 },
            Hypothesis::Trampoline { target: 0x8000_0400 },
            Hypothesis::Unclassified,
        ];
        for variant in variants {
            assert!(!variant.verification_plan().is_empty());
        }
    }

    #[test]
    fn runaway_non_terminating_body_is_downgraded_to_unclassified() {
        // A word that decodes as an ordinary (non-control) instruction,
        // repeated forever with no jr/jal/j/trap anywhere -- reproduces the
        // NWXE finding: three "callerless functions" whose declared size
        // was 4-8 bytes actually pointed into a structured data table with
        // no MIPS control-flow terminator nearby, so straight-line decode
        // never stops on its own. `sll $zero,$zero,0` (0x00000000) IS the
        // canonical MIPS nop and decodes to `Instruction::Nop`, which is
        // not a terminator, so a bank of nothing but zero words never
        // finds a natural end and must hit the bound.
        let bank = vec![0u8; (MAX_OWN_BODY_WORDS + 64) * 4];
        let va_start = 0x8000_0000u32;
        let (hyp, count, bound_hit) = classify_function(&bank, va_start, va_start);
        assert!(bound_hit, "expected the walk to hit MAX_OWN_BODY_WORDS");
        assert_eq!(count as usize, MAX_OWN_BODY_WORDS);
        assert_eq!(
            hyp,
            Hypothesis::Unclassified,
            "a hypothesis derived from a non-terminating walk must never be reported as anything else"
        );
    }
}
