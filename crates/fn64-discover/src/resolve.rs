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

use crate::cfg::{build_cfg_with_indirect, BasicBlock, BlockTerminator, Cfg};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

const MAX_VALUE_SET: usize = 256;
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum AbstractValue {
    Unknown,
    Concrete(BTreeSet<u32>),
    Stack { root: u32, offsets: BTreeSet<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrackedValue {
    value: AbstractValue,
    memory_sources: BTreeSet<u32>,
    bounded_index: bool,
    through_memory: bool,
    from_static_memory: bool,
}

impl TrackedValue {
    fn unknown() -> Self {
        Self {
            value: AbstractValue::Unknown,
            memory_sources: BTreeSet::new(),
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
        }
    }

    fn constant(value: u32) -> Self {
        Self::concrete([value])
    }

    fn concrete(values: impl IntoIterator<Item = u32>) -> Self {
        let values: BTreeSet<u32> = values.into_iter().collect();
        if values.is_empty() || values.len() > MAX_VALUE_SET {
            return Self::unknown();
        }
        Self {
            value: AbstractValue::Concrete(values),
            memory_sources: BTreeSet::new(),
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
        }
    }

    fn stack(root: u32, offset: i32) -> Self {
        Self {
            value: AbstractValue::Stack {
                root,
                offsets: BTreeSet::from([offset]),
            },
            memory_sources: BTreeSet::new(),
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
        }
    }

    fn join(&self, other: &Self) -> Self {
        let value = match (&self.value, &other.value) {
            (AbstractValue::Concrete(left), AbstractValue::Concrete(right)) => {
                let union: BTreeSet<u32> = left.union(right).copied().collect();
                if union.len() <= MAX_VALUE_SET {
                    AbstractValue::Concrete(union)
                } else {
                    AbstractValue::Unknown
                }
            }
            (
                AbstractValue::Stack {
                    root: left_root,
                    offsets: left,
                },
                AbstractValue::Stack {
                    root: right_root,
                    offsets: right,
                },
            ) if left_root == right_root => {
                let union: BTreeSet<i32> = left.union(right).copied().collect();
                if union.len() <= MAX_VALUE_SET {
                    AbstractValue::Stack {
                        root: *left_root,
                        offsets: union,
                    }
                } else {
                    AbstractValue::Unknown
                }
            }
            _ => AbstractValue::Unknown,
        };
        if matches!(value, AbstractValue::Unknown) {
            return Self::unknown();
        }
        Self {
            value,
            memory_sources: self
                .memory_sources
                .union(&other.memory_sources)
                .copied()
                .collect(),
            bounded_index: self.bounded_index && other.bounded_index,
            through_memory: self.through_memory || other.through_memory,
            from_static_memory: self.from_static_memory || other.from_static_memory,
        }
    }

    fn concrete_values(&self) -> Option<&BTreeSet<u32>> {
        match &self.value {
            AbstractValue::Concrete(values) => Some(values),
            _ => None,
        }
    }

    fn map_concrete(&self, op: impl Fn(u32) -> u32) -> Self {
        let Some(values) = self.concrete_values() else {
            return Self::unknown();
        };
        let mut result = Self::concrete(values.iter().copied().map(op));
        result.memory_sources = self.memory_sources.clone();
        result.bounded_index = self.bounded_index;
        result.through_memory = self.through_memory;
        result.from_static_memory = self.from_static_memory;
        result
    }

    fn add_immediate(&self, immediate: i32) -> Self {
        match &self.value {
            AbstractValue::Concrete(values) => {
                let mut result = Self::concrete(
                    values
                        .iter()
                        .map(|value| value.wrapping_add(immediate as u32)),
                );
                result.memory_sources = self.memory_sources.clone();
                result.bounded_index = self.bounded_index;
                result.through_memory = self.through_memory;
                result.from_static_memory = self.from_static_memory;
                result
            }
            AbstractValue::Stack { root, offsets } => {
                let offsets: BTreeSet<i32> = offsets
                    .iter()
                    .map(|offset| offset.wrapping_add(immediate))
                    .collect();
                if offsets.len() > MAX_VALUE_SET {
                    Self::unknown()
                } else {
                    Self {
                        value: AbstractValue::Stack {
                            root: *root,
                            offsets,
                        },
                        memory_sources: self.memory_sources.clone(),
                        bounded_index: self.bounded_index,
                        through_memory: self.through_memory,
                        from_static_memory: self.from_static_memory,
                    }
                }
            }
            AbstractValue::Unknown => Self::unknown(),
        }
    }

    fn binary(&self, other: &Self, op: impl Fn(u32, u32) -> u32) -> Self {
        let (Some(left), Some(right)) = (self.concrete_values(), other.concrete_values()) else {
            return Self::unknown();
        };
        if left.len().saturating_mul(right.len()) > MAX_VALUE_SET {
            return Self::unknown();
        }
        let values = left
            .iter()
            .flat_map(|left| right.iter().map(|right| op(*left, *right)));
        let mut result = Self::concrete(values);
        result.memory_sources = self
            .memory_sources
            .union(&other.memory_sources)
            .copied()
            .collect();
        result.bounded_index = self.bounded_index || other.bounded_index;
        result.through_memory = self.through_memory || other.through_memory;
        result.from_static_memory = self.from_static_memory || other.from_static_memory;
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum MemoryLocation {
    Concrete(u32),
    Stack { root: u32, offset: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnalysisState {
    registers: [TrackedValue; 32],
    memory: BTreeMap<MemoryLocation, TrackedValue>,
}

impl AnalysisState {
    fn at_root(root: u32) -> Self {
        let mut registers = std::array::from_fn(|_| TrackedValue::unknown());
        registers[0] = TrackedValue::constant(0);
        registers[29] = TrackedValue::stack(root, 0);
        Self {
            registers,
            memory: BTreeMap::new(),
        }
    }

    fn set_register(&mut self, register: u8, value: TrackedValue) {
        if register != 0 {
            self.registers[register as usize] = value;
        }
        self.registers[0] = TrackedValue::constant(0);
    }

    fn widened() -> Self {
        let mut registers = std::array::from_fn(|_| TrackedValue::unknown());
        registers[0] = TrackedValue::constant(0);
        Self {
            registers,
            memory: BTreeMap::new(),
        }
    }

    fn join(&self, other: &Self) -> Self {
        let registers =
            std::array::from_fn(|index| self.registers[index].join(&other.registers[index]));
        let memory = self
            .memory
            .iter()
            .filter_map(|(location, left)| {
                other
                    .memory
                    .get(location)
                    .map(|right| (location.clone(), left.join(right)))
            })
            .collect();
        Self { registers, memory }
    }

    fn refine_unsigned_upper_bound(&mut self, register: u8, upper: u32) {
        if upper == 0 || upper as usize > MAX_VALUE_SET {
            return;
        }
        let range: BTreeSet<u32> = (0..upper).collect();
        let values = match self.registers[register as usize].concrete_values() {
            Some(existing) => existing.intersection(&range).copied().collect(),
            None => range,
        };
        let mut refined = TrackedValue::concrete(values);
        refined.bounded_index = true;
        self.set_register(register, refined);
    }

    fn clobber_callers(&mut self) {
        for register in [
            1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 24, 25, 31,
        ] {
            self.set_register(register, TrackedValue::unknown());
        }
        // An unknown callee may mutate any memory reachable through globals
        // or pointer arguments. Keeping pre-call stores would turn a stale
        // value into a fabricated exhaustive target set.
        self.memory.clear();
    }
}

fn value_locations(value: &TrackedValue) -> Option<Vec<MemoryLocation>> {
    match &value.value {
        AbstractValue::Concrete(values) => Some(
            values
                .iter()
                .copied()
                .map(MemoryLocation::Concrete)
                .collect(),
        ),
        AbstractValue::Stack { root, offsets } => Some(
            offsets
                .iter()
                .copied()
                .map(|offset| MemoryLocation::Stack {
                    root: *root,
                    offset,
                })
                .collect(),
        ),
        AbstractValue::Unknown => None,
    }
}

fn read_static_word(bank_bytes: &[u8], va_start: u32, address: u32) -> Option<u32> {
    let offset = address.checked_sub(va_start)? as usize;
    let bytes = bank_bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

fn load_word(
    state: &AnalysisState,
    address: &TrackedValue,
    bank_bytes: &[u8],
    va_start: u32,
) -> TrackedValue {
    let Some(locations) = value_locations(address) else {
        return TrackedValue::unknown();
    };
    if locations.is_empty() || locations.len() > MAX_VALUE_SET {
        return TrackedValue::unknown();
    }

    let mut loaded: Option<TrackedValue> = None;
    let mut sources = BTreeSet::new();
    let mut read_static = false;
    for location in locations {
        let value = match &location {
            MemoryLocation::Concrete(address) => {
                sources.insert(*address);
                if let Some(value) = state.memory.get(&location).cloned() {
                    Some(value)
                } else {
                    read_static = true;
                    read_static_word(bank_bytes, va_start, *address).map(TrackedValue::constant)
                }
            }
            MemoryLocation::Stack { .. } => state.memory.get(&location).cloned(),
        };
        let Some(value) = value else {
            return TrackedValue::unknown();
        };
        loaded = Some(match loaded {
            Some(previous) => previous.join(&value),
            None => value,
        });
    }

    let mut loaded = loaded.unwrap_or_else(TrackedValue::unknown);
    if matches!(loaded.value, AbstractValue::Unknown) {
        return loaded;
    }
    loaded.memory_sources.extend(sources);
    loaded.bounded_index |= address.bounded_index;
    loaded.through_memory = true;
    loaded.from_static_memory |= read_static;
    loaded
}

fn store_word(state: &mut AnalysisState, address: &TrackedValue, value: TrackedValue) {
    let Some(locations) = value_locations(address) else {
        // The store may alias any exact stack/global value we retained.
        // Forgetting those values is the only sound bounded response.
        state.memory.clear();
        return;
    };
    if locations.len() != 1 {
        state.memory.clear();
        return;
    }
    state.memory.insert(locations[0].clone(), value);
}

fn execute_instruction(
    state: &mut AnalysisState,
    pc: u32,
    word: u32,
    bank_bytes: &[u8],
    va_start: u32,
) {
    let opcode = (word >> 26) & 0x3f;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    let shift = (word >> 6) & 0x1f;
    let immediate = (word & 0xffff) as i16 as i32;

    match opcode {
        0x00 => match word & 0x3f {
            0x00 => {
                let value = state.registers[rt as usize].map_concrete(|value| value << shift);
                state.set_register(rd, value);
            }
            0x02 => {
                let value = state.registers[rt as usize].map_concrete(|value| value >> shift);
                state.set_register(rd, value);
            }
            0x20 | 0x21 | 0x2c | 0x2d => {
                let value = state.registers[rs as usize]
                    .binary(&state.registers[rt as usize], u32::wrapping_add);
                state.set_register(rd, value);
            }
            0x22 | 0x23 | 0x2e | 0x2f => {
                let value = state.registers[rs as usize]
                    .binary(&state.registers[rt as usize], u32::wrapping_sub);
                state.set_register(rd, value);
            }
            0x25 => {
                let value = state.registers[rs as usize]
                    .binary(&state.registers[rt as usize], |left, right| left | right);
                state.set_register(rd, value);
            }
            0x09 => state.set_register(rd, TrackedValue::constant(pc.wrapping_add(8))),
            0x08 | 0x0c | 0x0d => {}
            _ if rd != 0 => state.set_register(rd, TrackedValue::unknown()),
            _ => {}
        },
        0x0f => state.set_register(rt, TrackedValue::constant((word & 0xffff) << 16)),
        0x08 | 0x09 | 0x18 | 0x19 => {
            let value = state.registers[rs as usize].add_immediate(immediate);
            state.set_register(rt, value);
        }
        0x0c => {
            let mask = word & 0xffff;
            let value = state.registers[rs as usize].map_concrete(|value| value & mask);
            state.set_register(rt, value);
        }
        0x0d => {
            let immediate = word & 0xffff;
            let value = state.registers[rs as usize].map_concrete(|value| value | immediate);
            state.set_register(rt, value);
        }
        0x0e => {
            let immediate = word & 0xffff;
            let value = state.registers[rs as usize].map_concrete(|value| value ^ immediate);
            state.set_register(rt, value);
        }
        0x23 | 0x27 => {
            let address = state.registers[rs as usize].add_immediate(immediate);
            let value = load_word(state, &address, bank_bytes, va_start);
            state.set_register(rt, value);
        }
        0x2b => {
            let address = state.registers[rs as usize].add_immediate(immediate);
            let value = state.registers[rt as usize].clone();
            store_word(state, &address, value);
        }
        0x03 => state.set_register(31, TrackedValue::constant(pc.wrapping_add(8))),
        0x01 if matches!(rt, 0x10..=0x13) => {
            state.set_register(31, TrackedValue::constant(pc.wrapping_add(8)));
        }
        0x0a | 0x0b | 0x20..=0x22 | 0x24..=0x26 | 0x30 | 0x34 | 0x37 | 0x38 | 0x3c => {
            state.set_register(rt, TrackedValue::unknown());
        }
        0x10..=0x13 if matches!(rs, 0x00..=0x02) => {
            state.set_register(rt, TrackedValue::unknown());
        }
        _ => {}
    }
}

fn read_block_words(block: &BasicBlock, bank_bytes: &[u8], va_start: u32) -> Vec<(u32, u32)> {
    let mut words = Vec::new();
    let mut pc = block.start_va;
    while pc < block.end_va {
        let Some(offset) = pc.checked_sub(va_start).map(|offset| offset as usize) else {
            break;
        };
        let Some(bytes) = bank_bytes.get(offset..offset.saturating_add(4)) else {
            break;
        };
        words.push((pc, u32::from_be_bytes(bytes.try_into().unwrap())));
        pc = pc.wrapping_add(4);
    }
    words
}

fn written_gpr(word: u32) -> Option<u8> {
    let opcode = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    match opcode {
        0x00 => (rd != 0).then_some(rd),
        0x01 if matches!(rt, 0x10..=0x13) => Some(31),
        0x03 => Some(31),
        0x08..=0x0f | 0x18..=0x1b | 0x20..=0x27 | 0x30..=0x37 => (rt != 0).then_some(rt),
        0x10..=0x13 if matches!(rs, 0x00..=0x02) => (rt != 0).then_some(rt),
        _ => None,
    }
}

fn branch_bound(words: &[(u32, u32)], terminator: &BlockTerminator) -> Option<(u32, u8, u32)> {
    let (target, fallthrough) = match terminator {
        BlockTerminator::Branch {
            target,
            fallthrough,
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
        } => (*target, *fallthrough),
        _ => return None,
    };
    let &(_, branch) = words.get(words.len().checked_sub(2)?)?;
    let opcode = branch >> 26;
    if !matches!(opcode, 0x04 | 0x05) {
        return None;
    }
    let branch_rs = ((branch >> 21) & 0x1f) as u8;
    let branch_rt = ((branch >> 16) & 0x1f) as u8;
    let predicate = if branch_rs == 0 {
        branch_rt
    } else if branch_rt == 0 {
        branch_rs
    } else {
        return None;
    };
    let before_branch = &words[..words.len().saturating_sub(2)];
    let sltiu_index = before_branch
        .iter()
        .rposition(|(_, word)| written_gpr(*word) == Some(predicate))?;
    let (_, sltiu) = before_branch[sltiu_index];
    if sltiu >> 26 != 0x0b {
        return None;
    }
    let index = ((sltiu >> 21) & 0x1f) as u8;
    if index == predicate
        || before_branch[sltiu_index + 1..]
            .iter()
            .any(|(_, word)| written_gpr(*word) == Some(index))
    {
        return None;
    }
    let upper = sltiu & 0xffff;
    let valid_successor = if opcode == 0x04 { fallthrough } else { target };
    Some((valid_successor, index, upper))
}

fn block_successors(block: &BasicBlock) -> Vec<u32> {
    match &block.terminator {
        BlockTerminator::Fallthrough { next } => vec![*next],
        BlockTerminator::Tail { target } => vec![*target],
        BlockTerminator::Call { next, .. } => vec![*next],
        BlockTerminator::Branch {
            target,
            fallthrough,
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
        } => vec![*target, *fallthrough],
        BlockTerminator::ResolvedIndirect {
            targets,
            via_call: false,
        } => targets.clone(),
        BlockTerminator::ResolvedIndirect { via_call: true, .. } => vec![block.end_va],
        BlockTerminator::Indirect { via_call: true } => vec![block.end_va],
        BlockTerminator::Return
        | BlockTerminator::Indirect { via_call: false }
        | BlockTerminator::Trap
        | BlockTerminator::InvalidInstruction { .. }
        | BlockTerminator::MissingDelaySlot { .. }
        | BlockTerminator::RanOffEnd => Vec::new(),
    }
}

fn resolution_from_value(site_pc: u32, via_call: bool, value: &TrackedValue) -> IndirectResolution {
    let memory_sources: Vec<u32> = value.memory_sources.iter().copied().collect();
    let Some(targets) = value.concrete_values() else {
        return IndirectResolution {
            site_pc,
            via_call,
            state: IndirectProofState::Open,
            kind: None,
            targets: Vec::new(),
            memory_sources,
        };
    };
    let is_jump_table = memory_sources.len() > 1 && value.bounded_index;
    if value.from_static_memory && !is_jump_table {
        // Load-image bytes prove only an initial value for arbitrary mutable
        // memory. Without a dominating table bound or an exact tracked store,
        // the runtime target universe is not closed.
        return IndirectResolution {
            site_pc,
            via_call,
            state: IndirectProofState::Open,
            kind: None,
            targets: Vec::new(),
            memory_sources,
        };
    }
    let kind = if is_jump_table {
        IndirectResolutionKind::JumpTable
    } else if value.through_memory {
        IndirectResolutionKind::MemoryValueSet
    } else {
        IndirectResolutionKind::Constant
    };
    IndirectResolution {
        site_pc,
        via_call,
        state: IndirectProofState::Exhaustive,
        kind: Some(kind),
        targets: targets.iter().copied().collect(),
        memory_sources,
    }
}

/// Run bounded forward value-set analysis over the currently reachable CFG.
/// Joins that exceed [`MAX_VALUE_SET`] become `open`; no widening guesses a
/// target. Bounds from `sltiu` + dominating `beq`/`bne` edges refine only the
/// guarded successor, so a path that bypasses the check joins back to `open`.
pub fn resolve_value_sets(cfg: &Cfg, bank_bytes: &[u8], va_start: u32) -> Vec<IndirectResolution> {
    resolve_value_sets_from_roots(cfg, bank_bytes, va_start, &cfg.proven_roots)
}

fn resolve_value_sets_from_roots(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    analysis_roots: &[u32],
) -> Vec<IndirectResolution> {
    let blocks: BTreeMap<u32, &BasicBlock> = cfg
        .blocks
        .iter()
        .map(|block| (block.start_va, block))
        .collect();
    let mut incoming: BTreeMap<u32, AnalysisState> = BTreeMap::new();
    let mut worklist = VecDeque::new();
    for &root in analysis_roots {
        if !blocks.contains_key(&root) {
            continue;
        }
        let root_state = AnalysisState::at_root(root);
        let next = incoming
            .get(&root)
            .map_or_else(|| root_state.clone(), |state| state.join(&root_state));
        if incoming.get(&root) != Some(&next) {
            incoming.insert(root, next);
            worklist.push_back(root);
        }
    }

    let mut resolutions: BTreeMap<u32, IndirectResolution> = BTreeMap::new();
    let mut visits: BTreeMap<u32, usize> = BTreeMap::new();
    while let Some(start) = worklist.pop_front() {
        let Some(block) = blocks.get(&start) else {
            continue;
        };
        let Some(mut state) = incoming.get(&start).cloned() else {
            continue;
        };
        let visit = visits.entry(start).or_default();
        *visit += 1;
        if *visit > MAX_BLOCK_REVISITS {
            let widened = AnalysisState::widened();
            if state == widened {
                continue;
            }
            incoming.insert(start, widened.clone());
            state = widened;
        }
        let words = read_block_words(block, bank_bytes, va_start);
        let bound = branch_bound(&words, &block.terminator);
        let transfer_pc = block.end_va.checked_sub(8);
        let delay_pc = block.end_va.checked_sub(4);
        let mut before_delay = None;
        for &(pc, word) in &words {
            if Some(pc) == delay_pc {
                before_delay = Some(state.clone());
            }
            if Some(pc) == transfer_pc {
                if let BlockTerminator::Indirect { via_call }
                | BlockTerminator::ResolvedIndirect { via_call, .. } = &block.terminator
                {
                    let register = ((word >> 21) & 0x1f) as usize;
                    let candidate =
                        resolution_from_value(pc, *via_call, &state.registers[register]);
                    resolutions
                        .entry(pc)
                        .and_modify(|existing| {
                            if *existing != candidate {
                                existing.state = IndirectProofState::Open;
                                existing.kind = None;
                                existing.targets.clear();
                            }
                        })
                        .or_insert(candidate);
                }
            }
            execute_instruction(&mut state, pc, word, bank_bytes, va_start);
        }

        let is_call = matches!(
            block.terminator,
            BlockTerminator::Call { .. }
                | BlockTerminator::Indirect { via_call: true }
                | BlockTerminator::ResolvedIndirect { via_call: true, .. }
        );
        if is_call {
            state.clobber_callers();
        }
        for successor in block_successors(block) {
            if !blocks.contains_key(&successor) {
                continue;
            }
            let mut outgoing = state.clone();
            if let Some((valid_successor, register, upper)) = bound {
                if successor == valid_successor {
                    // The branch condition is decided before its delay slot.
                    // Refine on the selected edge first, then execute the slot
                    // so a compiler-scheduled `sll index,2` inherits the
                    // proven finite set instead of remaining open.
                    if let (Some(mut refined), Some(&(delay_pc, delay_word))) =
                        (before_delay.clone(), words.last())
                    {
                        refined.refine_unsigned_upper_bound(register, upper);
                        execute_instruction(
                            &mut refined,
                            delay_pc,
                            delay_word,
                            bank_bytes,
                            va_start,
                        );
                        outgoing = refined;
                    } else {
                        outgoing.refine_unsigned_upper_bound(register, upper);
                    }
                }
            }
            if matches!(
                block.terminator,
                BlockTerminator::BranchLikely { fallthrough, .. } if successor == fallthrough
            ) {
                // A not-taken likely branch annuls its delay slot.
                if let Some(pre_delay) = &before_delay {
                    outgoing = pre_delay.clone();
                }
            }
            let next = incoming
                .get(&successor)
                .map_or_else(|| outgoing.clone(), |current| current.join(&outgoing));
            if incoming.get(&successor) != Some(&next) {
                incoming.insert(successor, next);
                worklist.push_back(successor);
            }
        }
    }

    for site in &cfg.indirect_sites {
        resolutions.entry(site.pc).or_insert(IndirectResolution {
            site_pc: site.pc,
            via_call: site.via_call,
            state: IndirectProofState::Open,
            kind: None,
            targets: Vec::new(),
            memory_sources: Vec::new(),
        });
    }
    resolutions.into_values().collect()
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
        let targets_are_usable = !resolution.targets.is_empty()
            && resolution.targets.iter().all(|target| {
                target.is_multiple_of(4) && (resolution.via_call || in_bank(*target))
            });
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
    let roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    let va_end = va_start.wrapping_add(bank_bytes.len() as u32);
    let mut exhaustive: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut history = vec![exhaustive.clone()];

    loop {
        let root_vec: Vec<u32> = roots.iter().copied().collect();
        let cfg = build_cfg_with_indirect(bank, bank_bytes, va_start, &root_vec, &exhaustive);
        let mut resolutions = resolve_value_sets_from_roots(&cfg, bank_bytes, va_start, &root_vec);
        reject_unusable_targets(&mut resolutions, va_start, va_end);

        let next: BTreeMap<u32, Vec<u32>> = resolutions
            .iter()
            .filter(|resolution| resolution.state == IndirectProofState::Exhaustive)
            .map(|resolution| (resolution.site_pc, resolution.targets.clone()))
            .collect();
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
                let cfg =
                    build_cfg_with_indirect(bank, bank_bytes, va_start, &root_vec, &conservative);
                let mut resolutions =
                    resolve_value_sets_from_roots(&cfg, bank_bytes, va_start, &root_vec);
                reject_unusable_targets(&mut resolutions, va_start, va_end);
                let resolved: BTreeMap<u32, Vec<u32>> = resolutions
                    .iter()
                    .filter(|resolution| resolution.state == IndirectProofState::Exhaustive)
                    .map(|resolution| (resolution.site_pc, resolution.targets.clone()))
                    .collect();
                let confirmed: BTreeMap<u32, Vec<u32>> = conservative
                    .iter()
                    .filter(|(site, targets)| resolved.get(site) == Some(*targets))
                    .map(|(site, targets)| (*site, targets.clone()))
                    .collect();
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
    let mut roots: BTreeSet<u32> = seed_roots.iter().copied().collect();
    roots.extend(db.proven_function_entries(bank));
    build_cfg_closed(
        bank,
        bank_bytes,
        va_start,
        &roots.into_iter().collect::<Vec<_>>(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::build_cfg;

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
    fn fixed_point_traverses_resolved_jump_without_inventing_callable_root() {
        // Boot stub jumps to 0x80000100, which itself just returns. The
        // exhaustive successor must be traversed, but a link-free `jr` does
        // not prove that target is a callable function entry.
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
        assert!(!cfg.proven_roots.contains(&0x8000_0100));
        assert_eq!(
            cfg.word_class.get(&0x8000_0100),
            Some(&crate::cfg::WordClass::ProvenCode)
        );
        assert!(cfg.blocks.iter().any(|block| block.start_va == 0x8000_0100));
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

    #[test]
    fn bounded_jump_table_targets_are_reachable_but_not_function_roots() {
        // sltiu $at,$a0,3 ; beq $at,$zero,default ; nop
        // sll $t0,$a0,2 ; lui $t1,0x8000 ; addu $t1,$t1,$t0
        // lw $t9,0x40($t1) ; jr $t9 ; nop
        let sltiu = (0x0bu32 << 26) | (4 << 21) | (1 << 16) | 3;
        let beq_default = (0x04u32 << 26) | (1 << 21) | 7;
        let sll = (4u32 << 16) | (8 << 11) | (2 << 6);
        let lui_t1 = 0x3c09_8000;
        let addu = (9u32 << 21) | (8 << 16) | (9 << 11) | 0x21;
        let lw_t9 = (0x23u32 << 26) | (9 << 21) | (25 << 16) | 0x40;
        let jr_t9 = (25u32 << 21) | 0x08;
        let jr_ra = 0x03e0_0008;
        let mut bytes = asm(&[
            sltiu,
            beq_default,
            NOP,
            sll,
            lui_t1,
            addu,
            lw_t9,
            jr_t9,
            NOP,
            jr_ra,
            NOP,
        ]);
        bytes.resize(0xc0, 0);
        for (offset, target) in [
            (0x40, 0x8000_0080u32),
            (0x44, 0x8000_0090),
            (0x48, 0x8000_00a0),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&target.to_be_bytes());
            let target_offset = (target - 0x8000_0000) as usize;
            bytes[target_offset..target_offset + 4].copy_from_slice(&jr_ra.to_be_bytes());
            bytes[target_offset + 4..target_offset + 8].copy_from_slice(&NOP.to_be_bytes());
        }

        let closure = build_cfg_value_set_closed("switch", &bytes, 0x8000_0000, &[0x8000_0000]);
        let table = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_001c)
            .unwrap();
        assert_eq!(table.state, IndirectProofState::Exhaustive);
        assert_eq!(table.kind, Some(IndirectResolutionKind::JumpTable));
        assert_eq!(table.targets, vec![0x8000_0080, 0x8000_0090, 0x8000_00a0]);
        assert_eq!(
            table.memory_sources,
            vec![0x8000_0040, 0x8000_0044, 0x8000_0048]
        );
        for target in &table.targets {
            assert!(closure
                .cfg
                .blocks
                .iter()
                .any(|block| block.start_va == *target));
            assert!(!closure.cfg.proven_roots.contains(target));
        }
        let partition = crate::partition::partition(&closure.cfg);
        assert_eq!(partition.owners.len(), 1);
    }

    #[test]
    fn gp_relative_shifted_jump_table_closes_from_a_dominating_bound() {
        let sltiu = (0x0bu32 << 26) | (2 << 21) | (1 << 16) | 2;
        let beq_default = (0x04u32 << 26) | (1 << 21) | 7;
        let sll = (2u32 << 16) | (8 << 11) | (2 << 6);
        let addu = (28u32 << 21) | (8 << 16) | (9 << 11) | 0x21;
        let lw_t9 = (0x23u32 << 26) | (9 << 21) | (25 << 16) | 0x40;
        let jr_t9 = (25u32 << 21) | 0x08;
        let jr_ra = 0x03e0_0008;
        let mut bytes = asm(&[
            0x3c1c_8000, // lui $gp,0x8000
            sltiu,
            beq_default,
            NOP,
            sll,
            addu,
            lw_t9,
            jr_t9,
            NOP,
            jr_ra,
            NOP,
        ]);
        bytes.resize(0xa0, 0);
        for (offset, target) in [(0x40, 0x8000_0080u32), (0x44, 0x8000_0090)] {
            bytes[offset..offset + 4].copy_from_slice(&target.to_be_bytes());
            let target_offset = (target - 0x8000_0000) as usize;
            bytes[target_offset..target_offset + 4].copy_from_slice(&jr_ra.to_be_bytes());
            bytes[target_offset + 4..target_offset + 8].copy_from_slice(&NOP.to_be_bytes());
        }

        let closure = build_cfg_value_set_closed("switch", &bytes, 0x8000_0000, &[0x8000_0000]);
        let table = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_001c)
            .unwrap();
        assert_eq!(table.state, IndirectProofState::Exhaustive);
        assert_eq!(table.kind, Some(IndirectResolutionKind::JumpTable));
        assert_eq!(table.targets, vec![0x8000_0080, 0x8000_0090]);
    }

    #[test]
    fn bound_reaches_a_shift_scheduled_in_the_branch_delay_slot() {
        let sltiu = (0x0bu32 << 26) | (4 << 21) | (1 << 16) | 2;
        let beq_default = (0x04u32 << 26) | (1 << 21) | 7;
        let sll_delay = (4u32 << 16) | (8 << 11) | (2 << 6);
        let addu = (9u32 << 21) | (8 << 16) | (9 << 11) | 0x21;
        let lw_t9 = (0x23u32 << 26) | (9 << 21) | (25 << 16) | 0x40;
        let jr_t9 = (25u32 << 21) | 0x08;
        let jr_ra = 0x03e0_0008;
        let mut bytes = asm(&[
            sltiu,
            beq_default,
            sll_delay,
            0x3c09_8000,
            addu,
            lw_t9,
            jr_t9,
            NOP,
            NOP,
            jr_ra,
            NOP,
        ]);
        bytes.resize(0xa0, 0);
        for (offset, target) in [(0x40, 0x8000_0080u32), (0x44, 0x8000_0090)] {
            bytes[offset..offset + 4].copy_from_slice(&target.to_be_bytes());
            let target_offset = (target - 0x8000_0000) as usize;
            bytes[target_offset..target_offset + 4].copy_from_slice(&jr_ra.to_be_bytes());
            bytes[target_offset + 4..target_offset + 8].copy_from_slice(&NOP.to_be_bytes());
        }

        let closure = build_cfg_value_set_closed("switch", &bytes, 0x8000_0000, &[0x8000_0000]);
        let table = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_0018)
            .unwrap();
        assert_eq!(table.state, IndirectProofState::Exhaustive);
        assert_eq!(table.kind, Some(IndirectResolutionKind::JumpTable));
        assert_eq!(table.targets, vec![0x8000_0080, 0x8000_0090]);
    }

    #[test]
    fn indirect_call_pointer_survives_a_stack_store_and_reload() {
        let addiu_sp = 0x27bd_fff0;
        let lui_t0 = 0x3c08_8000;
        let addiu_t0 = 0x2508_0080;
        let sw_t0 = 0xafa8_0000;
        let lw_t9 = 0x8fb9_0000;
        let jalr_t9 = (25u32 << 21) | (31 << 11) | 0x09;
        let jr_ra = 0x03e0_0008;
        let mut bytes = asm(&[
            addiu_sp, lui_t0, addiu_t0, sw_t0, lw_t9, jalr_t9, NOP, jr_ra, NOP,
        ]);
        bytes.resize(0xa0, 0);
        bytes[0x80..0x84].copy_from_slice(&jr_ra.to_be_bytes());
        bytes[0x84..0x88].copy_from_slice(&NOP.to_be_bytes());

        let closure = build_cfg_value_set_closed("calls", &bytes, 0x8000_0000, &[0x8000_0000]);
        let call = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_0014)
            .unwrap();
        assert_eq!(call.state, IndirectProofState::Exhaustive);
        assert_eq!(call.kind, Some(IndirectResolutionKind::MemoryValueSet));
        assert_eq!(call.targets, vec![0x8000_0080]);
        assert!(closure.cfg.proven_roots.contains(&0x8000_0080));
    }

    #[test]
    fn bounded_code_pointer_array_proves_each_call_root() {
        let sltiu = (0x0bu32 << 26) | (4 << 21) | (1 << 16) | 2;
        let beq_default = (0x04u32 << 26) | (1 << 21) | 7;
        let sll_delay = (4u32 << 16) | (8 << 11) | (2 << 6);
        let addu = (9u32 << 21) | (8 << 16) | (9 << 11) | 0x21;
        let lw_t9 = (0x23u32 << 26) | (9 << 21) | (25 << 16) | 0x40;
        let jalr_t9 = (25u32 << 21) | (31 << 11) | 0x09;
        let jr_ra = 0x03e0_0008;
        let mut bytes = asm(&[
            sltiu,
            beq_default,
            sll_delay,
            0x3c09_8000,
            addu,
            lw_t9,
            jalr_t9,
            NOP,
            jr_ra,
            jr_ra,
            NOP,
        ]);
        bytes.resize(0xa0, 0);
        for (offset, target) in [(0x40, 0x8000_0080u32), (0x44, 0x8000_0090)] {
            bytes[offset..offset + 4].copy_from_slice(&target.to_be_bytes());
            let target_offset = (target - 0x8000_0000) as usize;
            bytes[target_offset..target_offset + 4].copy_from_slice(&jr_ra.to_be_bytes());
            bytes[target_offset + 4..target_offset + 8].copy_from_slice(&NOP.to_be_bytes());
        }

        let closure = build_cfg_value_set_closed("callbacks", &bytes, 0x8000_0000, &[0x8000_0000]);
        let call = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_0018)
            .unwrap();
        assert!(call.via_call);
        assert_eq!(call.state, IndirectProofState::Exhaustive);
        assert_eq!(call.kind, Some(IndirectResolutionKind::JumpTable));
        assert_eq!(call.targets, vec![0x8000_0080, 0x8000_0090]);
        for target in &call.targets {
            assert!(closure.cfg.proven_roots.contains(target));
        }
    }

    #[test]
    fn singleton_load_image_pointer_stays_open() {
        let jalr_t9 = (25u32 << 21) | (31 << 11) | 0x09;
        let mut bytes = asm(&[
            0x3c08_8000, // lui $t0,0x8000
            0x8d19_0020, // lw $t9,0x20($t0)
            jalr_t9,
            NOP,
        ]);
        bytes.resize(0x40, 0);
        bytes[0x20..0x24].copy_from_slice(&0x8000_0030u32.to_be_bytes());
        bytes[0x30..0x34].copy_from_slice(&0x03e0_0008u32.to_be_bytes());

        let closure = build_cfg_value_set_closed("pointers", &bytes, 0x8000_0000, &[0x8000_0000]);
        let call = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_0008)
            .unwrap();
        assert_eq!(call.state, IndirectProofState::Open);
        assert_eq!(call.kind, None);
        assert!(call.targets.is_empty());
        assert!(!closure.cfg.proven_roots.contains(&0x8000_0030));
    }

    #[test]
    fn overwritten_index_invalidates_a_prior_switch_bound() {
        let sltiu = (0x0bu32 << 26) | (4 << 21) | (1 << 16) | 2;
        let overwrite_a0 = (5u32 << 21) | (4 << 11) | 0x21; // addu $a0,$a1,$zero
        let beq_default = (0x04u32 << 26) | (1 << 21) | 7;
        let sll = (4u32 << 16) | (8 << 11) | (2 << 6);
        let addu = (9u32 << 21) | (8 << 16) | (9 << 11) | 0x21;
        let lw_t9 = (0x23u32 << 26) | (9 << 21) | (25 << 16) | 0x40;
        let jr_t9 = (25u32 << 21) | 0x08;
        let bytes = asm(&[
            sltiu,
            overwrite_a0,
            beq_default,
            NOP,
            sll,
            0x3c09_8000,
            addu,
            lw_t9,
            jr_t9,
            NOP,
            0x03e0_0008,
            NOP,
        ]);

        let closure = build_cfg_value_set_closed("switch", &bytes, 0x8000_0000, &[0x8000_0000]);
        let table = closure
            .indirect
            .iter()
            .find(|resolution| resolution.site_pc == 0x8000_0020)
            .unwrap();
        assert_eq!(table.state, IndirectProofState::Open);
        assert_eq!(table.kind, None);
    }

    #[test]
    fn unbounded_table_index_keeps_the_indirect_site_open() {
        let sll = (4u32 << 16) | (8 << 11) | (2 << 6);
        let lui_t1 = 0x3c09_8000;
        let addu = (9u32 << 21) | (8 << 16) | (9 << 11) | 0x21;
        let lw_t9 = (0x23u32 << 26) | (9 << 21) | (25 << 16) | 0x40;
        let jr_t9 = (25u32 << 21) | 0x08;
        let bytes = asm(&[sll, lui_t1, addu, lw_t9, jr_t9, NOP]);
        let closure = build_cfg_value_set_closed("open", &bytes, 0x8000_0000, &[0x8000_0000]);
        assert_eq!(closure.indirect.len(), 1);
        assert_eq!(closure.indirect[0].state, IndirectProofState::Open);
        assert!(closure.indirect[0].targets.is_empty());
    }

    #[test]
    fn closure_cycle_keeps_only_entries_identical_in_every_state() {
        let state_a = BTreeMap::from([
            (0x8000_0010, vec![0x8000_0100]),
            (0x8000_0020, vec![0x8000_0200]),
        ]);
        let state_b = BTreeMap::from([
            (0x8000_0010, vec![0x8000_0100]),
            (0x8000_0020, vec![0x8000_0300]),
            (0x8000_0030, vec![0x8000_0400]),
        ]);

        assert_eq!(
            retain_cycle_stable_entries(&[state_a, state_b]),
            BTreeMap::from([(0x8000_0010, vec![0x8000_0100])])
        );
    }
}
