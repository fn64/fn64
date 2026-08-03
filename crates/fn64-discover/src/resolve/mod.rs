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

use crate::cfg::{build_cfg_fenced, BasicBlock, BlockTerminator, Cfg, WordClass};
use fn64_recomp_rs::decoder::{decode, Instruction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_VALUE_SET: usize = 256;
const COP0_STATUS_BEV: u32 = 1 << 22;
// A loop may otherwise grow a finite set one element per trip. Widening every
// non-zero register and tracked store to Unknown after this bound can only
// turn a site into `open`; it cannot fabricate an exhaustive target.
const MAX_BLOCK_REVISITS: usize = 8;

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

/// Phase 6's discrete exhaustiveness result for one computed transfer.
/// Only `Exhaustive` records are allowed to feed CFG closure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndirectProofState {
    Exhaustive,
    Bounded,
    Open,
}

/// The machine-checkable construction that closed an indirect target set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndirectResolutionKind {
    Constant,
    MemoryValueSet,
    JumpTable,
}

/// One indirect site's finite target-set result. Jump-table targets are CFG
/// successors, never callable entries; `via_call` is the sole authority for
/// promoting exhaustive targets to function roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectResolution {
    pub site_pc: u32,
    pub via_call: bool,
    pub state: IndirectProofState,
    pub kind: Option<IndirectResolutionKind>,
    pub targets: Vec<u32>,
    /// Concrete load addresses whose values formed the target set. Empty for
    /// register-only constant constructions.
    pub memory_sources: Vec<u32>,
}

/// A Phase 4-6 fixed point: CFG reachability plus every indirect site's
/// explicit proof state, including sites that remain open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClosureResult {
    pub cfg: Cfg,
    pub indirect: Vec<IndirectResolution>,
}

/// The statically established callee set at one reachable call boundary.
///
/// Direct calls always carry one target. Resolved indirect calls retain their
/// complete finite target set instead of selecting one target by registration
/// or address order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallBoundaryCalleeV1 {
    Direct { target: u32 },
    ResolvedIndirect { targets: Vec<u32> },
}

/// Public projection of one GPR's bounded abstract value at a call boundary.
///
/// Stack locations are symbolic: `root` identifies the analysis root/frame,
/// not a runtime stack address. Consumers may compare two locations for exact
/// identity but must not reinterpret either field as an RDRAM pointer.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallBoundaryValueV1 {
    Concrete { values: Vec<u32> },
    StackLocations { root: u32, offsets: Vec<i32> },
    Open,
}

/// Reasons a diagnostic value projection is not an exact call operand proof.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallBoundaryValueBlockerV1 {
    NoReachableObservation,
    ValueOpen,
    PathDisagreement,
    RevisitWidened,
    /// A load-image word is only an initial value. It may have changed before
    /// the call, so even a singleton diagnostic value is not exact authority.
    MutableStaticMemorySource {
        addresses: Vec<u32>,
    },
}

/// One requested register projected without exposing the resolver's mutable
/// internal state. Empty `blockers` is the only exact proof state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallBoundaryRegisterProofV1 {
    pub register: u8,
    pub value: CallBoundaryValueV1,
    pub memory_sources: Vec<u32>,
    pub through_memory: bool,
    pub blockers: Vec<CallBoundaryValueBlockerV1>,
}

impl CallBoundaryRegisterProofV1 {
    pub fn exact_concrete_values(&self) -> Option<&[u32]> {
        if !self.blockers.is_empty() {
            return None;
        }
        match &self.value {
            CallBoundaryValueV1::Concrete { values } => Some(values),
            CallBoundaryValueV1::StackLocations { .. } | CallBoundaryValueV1::Open => None,
        }
    }
}

/// Exact bounded register state after a call's delay slot and before applying
/// the unknown callee's effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallBoundaryProofV1 {
    pub site_pc: u32,
    pub callee: CallBoundaryCalleeV1,
    pub registers: Vec<CallBoundaryRegisterProofV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallBoundaryAnalysisV1 {
    pub requested_registers: Vec<u8>,
    pub calls: Vec<CallBoundaryProofV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallBoundaryAnalysisErrorV1 {
    InvalidRegister { register: u8 },
}

/// One load-image word whose initial value has been admitted by the caller.
///
/// Admission binds the expected bytes; it does **not** prove that the runtime
/// word remains unchanged until a recovered load executes.  Consequently
/// [`ConditionalFixedWordStore`] states that source-stability condition
/// explicitly instead of promoting an initial load-image value into an
/// unconditional runtime claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AdmittedWordSource {
    pub address: u32,
    pub value: u32,
}

/// A word store whose address and unchanged loaded value agree on every
/// bounded abstract visit to the store site.
///
/// The conclusion is conditional: if `source` still contains `value` when the
/// cited load executes and control reaches `site_pc`, that MIPS instruction
/// stores the exact word at `destination`. A separate writer-closure receipt
/// must discharge source stability, and separate control-flow evidence must
/// establish that the store runs, before a consumer calls this an
/// unconditional image proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ConditionalFixedWordStore {
    pub site_pc: u32,
    pub destination: u32,
    pub value: u32,
    pub source: AdmittedWordSource,
}

/// Why a reachable word store that may touch a watched destination could not
/// be reduced to one conditional exact-copy result.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum FixedWordStoreBlocker {
    AddressOpen,
    AddressSetAmbiguous {
        addresses: Vec<u32>,
    },
    ValueOpen,
    ValueSetAmbiguous {
        values: Vec<u32>,
    },
    ValueNotUnchangedStaticLoad,
    SourceNotAdmitted {
        address: u32,
    },
    SourceValueMismatch {
        address: u32,
        admitted: u32,
        recovered: u32,
    },
    PathDisagreement,
    RevisitWidened,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenFixedWordStore {
    pub site_pc: u32,
    pub blockers: Vec<FixedWordStoreBlocker>,
}

/// Deterministic bounded result for stores that may touch the watched word
/// addresses.  `conditional` and `open` are each ordered by ascending site PC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixedWordStoreReport {
    pub conditional: Vec<ConditionalFixedWordStore>,
    pub open: Vec<OpenFixedWordStore>,
}

/// Encoding class of one aligned word that writes COP0 Status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Cop0StatusWriteKind {
    Mtc0,
    Dmtc0,
}

/// One aligned word whose typed decode writes COP0 register 12 (Status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cop0StatusWriteSite {
    pub site_pc: u32,
    pub instruction_word: u32,
    pub source_register: u8,
    pub kind: Cop0StatusWriteKind,
    pub word_class: Option<WordClass>,
}

/// Exhaustive aligned-word inventory of direct guest writes to COP0 Status in
/// one supplied image, plus the bounded CFG frontier that qualifies which of
/// those words are proven executable.
///
/// This is deliberately not a BEV invariant proof. `unclassified_writes` and
/// `open_indirect_sites` prevent a consumer from mistaking a closed reachable
/// subset for whole-image execution closure. `proven_data_words` remain in the
/// receipt so the raw decode denominator is independently auditable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cop0StatusWriteInventory {
    pub proven_code_writes: Vec<Cop0StatusWriteSite>,
    pub proven_data_words: Vec<Cop0StatusWriteSite>,
    pub unclassified_writes: Vec<Cop0StatusWriteSite>,
    pub open_indirect_sites: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Cop0StatusValueBlocker {
    NoReachableObservation,
    ValueOpen,
    RevisitWidened,
    ValueSetOverflow { observed: u32 },
    MutableStaticMemorySource { addresses: Vec<u32> },
    Dmtc0Unsupported,
}

/// Bounded source-register proof at one proven-code Status write. `values`
/// retains a small exact set when available; `known_zero`/`known_one` retain
/// path-invariant bits when the full value remains open. Consumers apply their
/// own bit invariant and must retain the typed blockers.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cop0StatusValueProof {
    pub site_pc: u32,
    pub values: Vec<u32>,
    pub known_zero: u32,
    pub known_one: u32,
    pub blockers: Vec<Cop0StatusValueBlocker>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cop0StatusWriteAnalysis {
    pub inventory: Cop0StatusWriteInventory,
    pub proven_code_value_proofs: Vec<Cop0StatusValueProof>,
}

/// One exact indexed TLB write retained by the whole-CFG abstract state.
///
/// These are raw COP0 values. Address translation remains the execution
/// runtime's responsibility so discovery cannot grow a second TLB model.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TlbWriteProofV1 {
    pub tlbwi_pc: u32,
    pub index_raw: u32,
    pub page_mask_raw: u32,
    pub entry_hi_raw: u32,
    pub entry_lo0_raw: u32,
    pub entry_lo1_raw: u32,
}

/// Why a reachable computed transfer lacks one path-invariant TLB setup.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TlbTransferBlockerV1 {
    NoReachableObservation,
    ViaCall,
    TargetOpen,
    TargetPathDisagreement,
    NoProvenTlbWrite,
    TlbPathDisagreement,
    EntryHiPathDisagreement,
    TlbSetupOpen { cop0d: u8 },
    MutableStaticMemorySource { cop0d: u8, addresses: Vec<u32> },
    Dmtc0Unsupported { cop0d: u8 },
    RandomIndexedWrite,
    UnknownCallEffects,
    RevisitWidened,
}

/// A computed transfer paired with the exact indexed writes active after its
/// delay slot. Empty `blockers` is the only admissible proof state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlbTransferProofV1 {
    pub transfer_pc: u32,
    pub target: Option<u32>,
    /// Exact COP0 EntryHi value active after the transfer's delay slot. This
    /// supplies the ASID/Region used by the runtime translator independently
    /// of the indexed entries' stored EntryHi values.
    pub entry_hi_at_transfer: Option<u64>,
    pub active_writes: Vec<TlbWriteProofV1>,
    pub blockers: Vec<TlbTransferBlockerV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstantTlbTransferAnalysisV1 {
    pub transfers: Vec<TlbTransferProofV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cop0StatusWriteInventoryError {
    UnalignedImage,
    AddressOverflow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixedWordStoreInputError {
    UnalignedWatchedDestination {
        address: u32,
    },
    UnalignedSource {
        address: u32,
    },
    ConflictingSourceValues {
        address: u32,
        first: u32,
        second: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RawWordStoreObservation {
    addresses: Option<Vec<u32>>,
    values: Option<Vec<u32>>,
    unchanged_static_word_source: Option<u32>,
    widened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RawCop0StatusObservation {
    values: Option<Vec<u32>>,
    known_zero: u32,
    known_one: u32,
    memory_sources: Vec<u32>,
    from_static_memory: bool,
    widened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawTlbTransferObservation {
    target: IndirectResolution,
    entry_hi_at_transfer: Option<u64>,
    active_writes: Vec<TlbWriteProofV1>,
    blockers: Vec<TlbTransferBlockerV1>,
    widened: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RawCallRegisterObservation {
    register: u8,
    value: AbstractValue,
    memory_sources: Vec<u32>,
    through_memory: bool,
    from_static_memory: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RawCallBoundaryObservation {
    registers: Vec<RawCallRegisterObservation>,
    widened: bool,
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
    let closure = build_cfg_value_set_closed(bank, bank_bytes, va_start, seed_roots);
    let block_starts: BTreeMap<u32, u32> = closure
        .cfg
        .blocks
        .iter()
        .flat_map(|block| {
            closure
                .indirect
                .iter()
                .filter(move |resolution| {
                    resolution.site_pc >= block.start_va && resolution.site_pc < block.end_va
                })
                .map(move |resolution| (resolution.site_pc, block.start_va))
        })
        .collect();
    let mut legacy = Vec::new();
    for resolution in &closure.indirect {
        if resolution.state != IndirectProofState::Exhaustive {
            continue;
        }
        for &target in &resolution.targets {
            legacy.push(ResolvedTarget {
                site_pc: resolution.site_pc,
                target,
                via_call: resolution.via_call,
                construction_start: block_starts
                    .get(&resolution.site_pc)
                    .copied()
                    .unwrap_or(resolution.site_pc),
            });
        }
    }
    legacy.sort_by_key(|resolution| (resolution.site_pc, resolution.target));
    (closure.cfg, legacy)
}

/// Build CFG + bounded value-set closure to a fixed point. Exhaustive
/// computed jumps add ordinary intra-owner successors; exhaustive computed
/// calls add callable roots. Bounded/open sites remain explicit records and
/// never enter the successor map.
fn retain_cycle_stable_entries(states: &[BTreeMap<u32, Vec<u32>>]) -> BTreeMap<u32, Vec<u32>> {
    let Some(first) = states.first() else {
        return BTreeMap::new();
    };
    first
        .iter()
        .filter(|(site, targets)| {
            states
                .iter()
                .skip(1)
                .all(|state| state.get(site) == Some(*targets))
        })
        .map(|(site, targets)| (*site, targets.clone()))
        .collect()
}

fn reject_unusable_targets(resolutions: &mut [IndirectResolution], va_start: u32, va_end: u32) {
    let in_bank = |target: u32| {
        target >= va_start && target < va_end && (target - va_start).is_multiple_of(4)
    };
    for resolution in resolutions {
        if resolution.state != IndirectProofState::Exhaustive {
            continue;
        }
        // ponytail: a jump table is a contiguous in-bank array of code
        // pointers; every real slot lands in this bank. The `via_call`
        // exemption below exists for a genuine cross-bank library CALL, whose
        // target is a *single* address materialized by `lui`/`addiu`
        // (`Constant`) into a mapped sibling bank -- NOT a table read. When a
        // `sltiu`-bounded switch's bound is wider than the table's populated
        // extent, the read walks PAST the real table into zeroed/garbage pages;
        // those stray words decode as word-aligned "targets" that are outside
        // every mapping (WM2000/NWXE: 20 such destinations, all
        // `OutsideAllMappings`). The resolver only knows *this* bank's range, so
        // it cannot tell "another mapped bank" from "unmapped over-walk garbage"
        // -- but a JumpTable slot is never a legitimate cross-bank call, so the
        // exemption must not apply to it. Require every JumpTable slot in-bank
        // regardless of `via_call`. Dropping the resolution to `Bounded` is
        // sound (the interpreter covers the site via dynamic_mips); admitting an
        // unprovable target is not.
        let allow_cross_bank =
            resolution.via_call && resolution.kind != Some(IndirectResolutionKind::JumpTable);
        let targets_are_usable = !resolution.targets.is_empty()
            && resolution
                .targets
                .iter()
                .all(|target| target.is_multiple_of(4) && (allow_cross_bank || in_bank(*target)));
        if !targets_are_usable {
            resolution.state = IndirectProofState::Bounded;
        }
    }
}

pub fn build_cfg_value_set_closed(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
) -> ClosureResult {
    build_cfg_value_set_closed_with_claims(bank, bank_bytes, va_start, seed_roots, &BTreeMap::new())
}

/// [`build_cfg_value_set_closed`] with cited indirect-edge claims pinned
/// into the fixed point: `claimed` maps a `jr` site pc to its
/// externally-cited jump-table targets (read from ROM bytes by the
/// caller). Claims are unioned into every iteration's exhaustive map —
/// they are citations, not derivations, so the resolver can neither
/// confirm nor demote them; the wrong==0 grade judges them instead. The
/// claimed sites' own resolution records stay exactly what the resolver
/// can prove (usually `Open`) — injecting an edge never fabricates an
/// exhaustiveness proof.
pub fn build_cfg_value_set_closed_with_claims(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
    claimed: &BTreeMap<u32, Vec<u32>>,
) -> ClosureResult {
    build_cfg_value_set_closed_with_claims_fenced(
        bank,
        bank_bytes,
        va_start,
        seed_roots,
        claimed,
        &BTreeSet::new(),
    )
}

/// [`build_cfg_value_set_closed_with_claims`] plus caller-supplied data
/// fences (see [`crate::cfg::build_cfg_fenced`]). Fenced VAs stop
/// straight-line descent so embedded read-only data is never decoded as
/// instructions; the fence is machine-checked evidence, not a guess.
pub fn build_cfg_value_set_closed_with_claims_fenced(
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
    claimed: &BTreeMap<u32, Vec<u32>>,
    data_fence: &BTreeSet<u32>,
) -> ClosureResult {
    let roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    let va_end = va_start.wrapping_add(bank_bytes.len() as u32);
    let mut exhaustive: BTreeMap<u32, Vec<u32>> = claimed.clone();
    let mut history = vec![exhaustive.clone()];

    loop {
        let root_vec: Vec<u32> = roots.iter().copied().collect();
        let cfg = build_cfg_fenced(
            bank,
            bank_bytes,
            va_start,
            &root_vec,
            &exhaustive,
            data_fence,
        );
        let mut resolutions = resolve_value_sets_from_roots(&cfg, bank_bytes, va_start, &root_vec);
        backslice_open_sites(&cfg, bank_bytes, va_start, &mut resolutions);
        reject_unusable_targets(&mut resolutions, va_start, va_end);

        let mut next: BTreeMap<u32, Vec<u32>> = resolutions
            .iter()
            .filter(|resolution| resolution.state == IndirectProofState::Exhaustive)
            .map(|resolution| (resolution.site_pc, resolution.targets.clone()))
            .collect();
        for (site, targets) in claimed {
            next.insert(*site, targets.clone());
        }
        if next == exhaustive {
            return ClosureResult {
                cfg,
                indirect: resolutions,
            };
        }
        if let Some(cycle_start) = history.iter().position(|state| state == &next) {
            // Adding one exhaustive indirect edge can expose a path that
            // invalidates that same edge on the next analysis pass. NWXE's
            // fourth recovered overlay produced the concrete two-cycle
            // 96 -> 97 -> 96 forever. Choosing either side would fabricate
            // exhaustiveness. Keep only entries identical throughout the
            // cycle, then monotonically remove any that the reduced graph no
            // longer confirms; every oscillating/new site stays explicitly
            // Open in the returned evidence.
            let mut conservative = retain_cycle_stable_entries(&history[cycle_start..]);
            loop {
                let cfg = build_cfg_fenced(
                    bank,
                    bank_bytes,
                    va_start,
                    &root_vec,
                    &conservative,
                    data_fence,
                );
                let mut resolutions =
                    resolve_value_sets_from_roots(&cfg, bank_bytes, va_start, &root_vec);
                backslice_open_sites(&cfg, bank_bytes, va_start, &mut resolutions);
                reject_unusable_targets(&mut resolutions, va_start, va_end);
                let resolved: BTreeMap<u32, Vec<u32>> = resolutions
                    .iter()
                    .filter(|resolution| resolution.state == IndirectProofState::Exhaustive)
                    .map(|resolution| (resolution.site_pc, resolution.targets.clone()))
                    .collect();
                let mut confirmed: BTreeMap<u32, Vec<u32>> = conservative
                    .iter()
                    .filter(|(site, targets)| resolved.get(site) == Some(*targets))
                    .map(|(site, targets)| (*site, targets.clone()))
                    .collect();
                for (site, targets) in claimed {
                    confirmed.insert(*site, targets.clone());
                }
                if confirmed != conservative {
                    conservative = confirmed;
                    continue;
                }
                for resolution in &mut resolutions {
                    if resolution.state == IndirectProofState::Exhaustive
                        && conservative.get(&resolution.site_pc) != Some(&resolution.targets)
                    {
                        resolution.state = IndirectProofState::Open;
                        resolution.kind = None;
                        resolution.targets.clear();
                    }
                }
                return ClosureResult {
                    cfg,
                    indirect: resolutions,
                };
            }
        }
        history.push(next.clone());
        exhaustive = next;
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
    let closure = build_cfg_value_set_closed_with_facts(db, bank, bank_bytes, va_start, seed_roots);
    closure_into_legacy(closure)
}

/// Build the same fact-seeded fixed point as [`build_cfg_closed_with_facts`]
/// while retaining every indirect site's exhaustive/bounded/open state.
/// Consumers building closure evidence must use this form instead of
/// reconstructing an indirect inventory from the legacy resolved-target list.
pub fn build_cfg_value_set_closed_with_facts(
    db: &crate::facts::FactDb,
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
) -> ClosureResult {
    let mut roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    roots.extend(db.proven_function_entries(bank));
    build_cfg_value_set_closed(
        bank,
        bank_bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    )
}

fn closure_into_legacy(closure: ClosureResult) -> (Cfg, Vec<ResolvedTarget>) {
    let block_starts: BTreeMap<u32, u32> = closure
        .cfg
        .blocks
        .iter()
        .flat_map(|block| {
            closure
                .indirect
                .iter()
                .filter(move |resolution| {
                    resolution.site_pc >= block.start_va && resolution.site_pc < block.end_va
                })
                .map(move |resolution| (resolution.site_pc, block.start_va))
        })
        .collect();
    let mut legacy = Vec::new();
    for resolution in &closure.indirect {
        if resolution.state != IndirectProofState::Exhaustive {
            continue;
        }
        for &target in &resolution.targets {
            legacy.push(ResolvedTarget {
                site_pc: resolution.site_pc,
                target,
                via_call: resolution.via_call,
                construction_start: block_starts
                    .get(&resolution.site_pc)
                    .copied()
                    .unwrap_or(resolution.site_pc),
            });
        }
    }
    legacy.sort_by_key(|resolution| (resolution.site_pc, resolution.target));
    (closure.cfg, legacy)
}

/// [`build_cfg_closed_with_facts`] plus cited jump-table claims (see
/// [`build_cfg_value_set_closed_with_claims`]).
pub fn build_cfg_closed_with_facts_and_claims(
    db: &crate::facts::FactDb,
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
    claimed: &BTreeMap<u32, Vec<u32>>,
) -> (Cfg, Vec<ResolvedTarget>) {
    build_cfg_closed_with_facts_claims_fenced(
        db,
        bank,
        bank_bytes,
        va_start,
        seed_roots,
        claimed,
        &BTreeSet::new(),
    )
}

/// [`build_cfg_closed_with_facts_and_claims`] plus caller-supplied data
/// fences (see [`crate::cfg::build_cfg_fenced`]).
pub fn build_cfg_closed_with_facts_claims_fenced(
    db: &crate::facts::FactDb,
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
    claimed: &BTreeMap<u32, Vec<u32>>,
    data_fence: &BTreeSet<u32>,
) -> (Cfg, Vec<ResolvedTarget>) {
    let mut roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    roots.extend(db.proven_function_entries(bank));
    let closure = build_cfg_value_set_closed_with_claims_fenced(
        bank,
        bank_bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
        claimed,
        data_fence,
    );
    let block_starts: BTreeMap<u32, u32> = closure
        .cfg
        .blocks
        .iter()
        .flat_map(|block| {
            closure
                .indirect
                .iter()
                .filter(move |resolution| {
                    resolution.site_pc >= block.start_va && resolution.site_pc < block.end_va
                })
                .map(move |resolution| (resolution.site_pc, block.start_va))
        })
        .collect();
    let mut legacy = Vec::new();
    for resolution in &closure.indirect {
        if resolution.state != IndirectProofState::Exhaustive {
            continue;
        }
        for &target in &resolution.targets {
            legacy.push(ResolvedTarget {
                site_pc: resolution.site_pc,
                target,
                via_call: resolution.via_call,
                construction_start: block_starts
                    .get(&resolution.site_pc)
                    .copied()
                    .unwrap_or(resolution.site_pc),
            });
        }
    }
    legacy.sort_by_key(|resolution| (resolution.site_pc, resolution.target));
    (closure.cfg, legacy)
}

/// Exploratory counterpart to [`build_cfg_closed_with_facts`]. It seeds the
/// fixed-point walk with candidate/supported entries so coverage can be
/// measured, but its output must remain candidate evidence and cannot feed
/// exact-owner admission.
pub fn build_cfg_exploratory_with_candidates(
    db: &crate::facts::FactDb,
    bank: &str,
    bank_bytes: &[u8],
    va_start: u32,
    seed_roots: &[u32],
) -> (Cfg, Vec<ResolvedTarget>) {
    let mut roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    roots.extend(db.candidate_function_entries(bank));
    build_cfg_closed(
        bank,
        bank_bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    )
}

mod analyses;
mod value_track;

pub use analyses::*;
pub use value_track::*;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cop0_status_write_inventory_tests;

#[cfg(test)]
mod tlb_transfer_tests;

#[cfg(test)]
mod fixed_word_store_tests;

#[cfg(test)]
mod probe_tests;
