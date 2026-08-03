use super::*;

/// Set GPR `i`, keeping `$zero` pinned to 0 (a write to `$zero` is discarded
/// by hardware).
pub(super) fn set(reg: &mut [Option<u32>; 32], i: u8, v: Option<u32>) {
    if i != 0 {
        reg[i as usize] = v;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum AbstractValue {
    Unknown,
    Concrete(BTreeSet<u32>),
    Stack { root: u32, offsets: BTreeSet<i32> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TrackedValue {
    value: AbstractValue,
    known_zero: u32,
    known_one: u32,
    memory_sources: BTreeSet<u32>,
    /// A singleton load-image word copied without arithmetic transformation.
    /// This is deliberately distinct from `memory_sources`: the latter is
    /// data-flow provenance and survives arithmetic for indirect-resolution
    /// diagnostics, while this identity proof must be cleared by arithmetic.
    unchanged_static_word_source: Option<u32>,
    bounded_index: bool,
    through_memory: bool,
    from_static_memory: bool,
    /// When this value is the 0/1 result of a `sltiu index_reg, upper`, records
    /// `(index_reg, upper)`. A dominating `beq`/`bne $this,$zero` then proves
    /// `index_reg < upper` on its guarded edge, even when the `sltiu` and the
    /// branch land in different basic blocks (a common compiler schedule).
    /// Cleared the instant `index_reg` is rewritten (see `set_register`), so the
    /// tag can never refer to a stale index.
    sltiu_bound: Option<(u8, u32)>,
}

impl TrackedValue {
    pub(super) fn unknown() -> Self {
        Self {
            value: AbstractValue::Unknown,
            known_zero: 0,
            known_one: 0,
            memory_sources: BTreeSet::new(),
            unchanged_static_word_source: None,
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
            sltiu_bound: None,
        }
    }

    /// The 0/1 abstract result of `sltiu index_reg, upper`. Numerically
    /// unknown -- only the recorded `(index_reg, upper)` bound matters, and it
    /// is consumed by a dominating `beq`/`bne $pred,$zero`.
    fn sltiu_flag(index_reg: u8, upper: u32) -> Self {
        Self {
            value: AbstractValue::Unknown,
            known_zero: 0xffff_fffc,
            known_one: 0,
            memory_sources: BTreeSet::new(),
            unchanged_static_word_source: None,
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
            sltiu_bound: Some((index_reg, upper)),
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
        let known_one = values
            .iter()
            .copied()
            .reduce(|left, right| left & right)
            .unwrap();
        let known_zero = values
            .iter()
            .copied()
            .map(|value| !value)
            .reduce(|left, right| left & right)
            .unwrap();
        Self {
            value: AbstractValue::Concrete(values),
            known_zero,
            known_one,
            memory_sources: BTreeSet::new(),
            unchanged_static_word_source: None,
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
            sltiu_bound: None,
        }
    }

    fn stack(root: u32, offset: i32) -> Self {
        Self {
            value: AbstractValue::Stack {
                root,
                offsets: BTreeSet::from([offset]),
            },
            known_zero: 0,
            known_one: 0,
            memory_sources: BTreeSet::new(),
            unchanged_static_word_source: None,
            bounded_index: false,
            through_memory: false,
            from_static_memory: false,
            sltiu_bound: None,
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
        let mut joined = Self {
            value,
            known_zero: self.known_zero & other.known_zero,
            known_one: self.known_one & other.known_one,
            memory_sources: self
                .memory_sources
                .union(&other.memory_sources)
                .copied()
                .collect(),
            unchanged_static_word_source: if self.unchanged_static_word_source
                == other.unchanged_static_word_source
            {
                self.unchanged_static_word_source
            } else {
                None
            },
            bounded_index: self.bounded_index && other.bounded_index,
            through_memory: self.through_memory || other.through_memory,
            from_static_memory: self.from_static_memory || other.from_static_memory,
            // A joined value is no longer a single sltiu flag; only a value that
            // both sides agree is the same sltiu result keeps the bound.
            sltiu_bound: if self.sltiu_bound == other.sltiu_bound {
                self.sltiu_bound
            } else {
                None
            },
        };
        if matches!(joined.value, AbstractValue::Unknown) {
            joined.memory_sources.clear();
            joined.unchanged_static_word_source = None;
            joined.bounded_index = false;
            joined.through_memory = false;
            joined.from_static_memory = false;
            joined.sltiu_bound = None;
        }
        joined
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
        result.unchanged_static_word_source = None;
        result.bounded_index = self.bounded_index;
        result.through_memory = self.through_memory;
        result.from_static_memory = self.from_static_memory;
        result
    }

    fn known_zero(mask: u32) -> Self {
        let mut value = Self::unknown();
        value.known_zero = mask;
        value
    }

    fn bitwise(
        &self,
        other: &Self,
        op: impl Fn(u32, u32) -> u32,
        known_zero: u32,
        known_one: u32,
    ) -> Self {
        let mut result = self.binary(other, op);
        result.known_zero = known_zero;
        result.known_one = known_one;
        result.memory_sources = self
            .memory_sources
            .union(&other.memory_sources)
            .copied()
            .collect();
        result.unchanged_static_word_source = None;
        result.through_memory = self.through_memory || other.through_memory;
        result.from_static_memory = self.from_static_memory || other.from_static_memory;
        result
    }

    fn bitand(&self, other: &Self) -> Self {
        self.bitwise(
            other,
            |left, right| left & right,
            self.known_zero | other.known_zero,
            self.known_one & other.known_one,
        )
    }

    fn bitor(&self, other: &Self) -> Self {
        self.bitwise(
            other,
            |left, right| left | right,
            self.known_zero & other.known_zero,
            self.known_one | other.known_one,
        )
    }

    fn bitxor(&self, other: &Self) -> Self {
        self.bitwise(
            other,
            |left, right| left ^ right,
            (self.known_zero & other.known_zero) | (self.known_one & other.known_one),
            (self.known_zero & other.known_one) | (self.known_one & other.known_zero),
        )
    }

    fn add_immediate(&self, immediate: i32) -> Self {
        if immediate == 0 {
            return self.clone();
        }
        match &self.value {
            AbstractValue::Concrete(values) => {
                let mut result = Self::concrete(
                    values
                        .iter()
                        .map(|value| value.wrapping_add(immediate as u32)),
                );
                result.memory_sources = self.memory_sources.clone();
                result.unchanged_static_word_source = None;
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
                        known_zero: 0,
                        known_one: 0,
                        memory_sources: self.memory_sources.clone(),
                        unchanged_static_word_source: None,
                        bounded_index: self.bounded_index,
                        through_memory: self.through_memory,
                        from_static_memory: self.from_static_memory,
                        sltiu_bound: None,
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
        result.unchanged_static_word_source = None;
        result.bounded_index = self.bounded_index || other.bounded_index;
        result.through_memory = self.through_memory || other.through_memory;
        result.from_static_memory = self.from_static_memory || other.from_static_memory;
        result
    }

    fn shift_left(&self, shift: u32) -> Self {
        let mut result = self.map_concrete(|value| value << shift);
        result.known_zero = (self.known_zero << shift) | ((1u32 << shift) - 1);
        result.known_one = self.known_one << shift;
        result.memory_sources = self.memory_sources.clone();
        result.through_memory = self.through_memory;
        result.from_static_memory = self.from_static_memory;
        result
    }

    fn shift_right(&self, shift: u32) -> Self {
        let mut result = self.map_concrete(|value| value >> shift);
        result.known_zero = (self.known_zero >> shift) | (!0u32 << (32 - shift));
        result.known_one = self.known_one >> shift;
        result.memory_sources = self.memory_sources.clone();
        result.through_memory = self.through_memory;
        result.from_static_memory = self.from_static_memory;
        result
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum MemoryLocation {
    Concrete(u32),
    Stack { root: u32, offset: i32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnalysisState {
    pub(super) registers: [TrackedValue; 32],
    memory: BTreeMap<MemoryLocation, TrackedValue>,
    staged_tlb: [TrackedValue; 5],
    proven_tlb_entries: BTreeMap<u32, TlbWriteProofV1>,
    tlb_blockers: BTreeSet<TlbTransferBlockerV1>,
}

impl AnalysisState {
    fn at_root(root: u32) -> Self {
        let mut registers = std::array::from_fn(|_| TrackedValue::unknown());
        registers[0] = TrackedValue::constant(0);
        registers[29] = TrackedValue::stack(root, 0);
        Self {
            registers,
            memory: BTreeMap::new(),
            staged_tlb: std::array::from_fn(|_| TrackedValue::unknown()),
            proven_tlb_entries: BTreeMap::new(),
            tlb_blockers: BTreeSet::new(),
        }
    }

    fn set_register(&mut self, register: u8, value: TrackedValue) {
        if register != 0 {
            self.registers[register as usize] = value;
            // Any pending `sltiu` flag whose index register is the one we just
            // overwrote no longer proves a bound on the new value. Invalidate
            // it so a dominating branch can never refine a stale index.
            for slot in &mut self.registers {
                if let Some((index_reg, _)) = slot.sltiu_bound {
                    if index_reg == register {
                        slot.sltiu_bound = None;
                    }
                }
            }
        }
        self.registers[0] = TrackedValue::constant(0);
    }

    pub(super) fn widened() -> Self {
        let mut registers = std::array::from_fn(|_| TrackedValue::unknown());
        registers[0] = TrackedValue::constant(0);
        Self {
            registers,
            memory: BTreeMap::new(),
            staged_tlb: std::array::from_fn(|_| TrackedValue::unknown()),
            proven_tlb_entries: BTreeMap::new(),
            tlb_blockers: BTreeSet::from([TlbTransferBlockerV1::RevisitWidened]),
        }
    }

    fn widened_preserving_tlb(&self) -> Self {
        let mut registers = std::array::from_fn(|_| TrackedValue::unknown());
        registers[0] = TrackedValue::constant(0);
        Self {
            registers,
            memory: BTreeMap::new(),
            staged_tlb: self.staged_tlb.clone(),
            proven_tlb_entries: self.proven_tlb_entries.clone(),
            tlb_blockers: self.tlb_blockers.clone(),
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
        let staged_tlb =
            std::array::from_fn(|index| self.staged_tlb[index].join(&other.staged_tlb[index]));
        let proven_tlb_entries = self
            .proven_tlb_entries
            .iter()
            .filter_map(|(index, left)| {
                (other.proven_tlb_entries.get(index) == Some(left))
                    .then_some((*index, left.clone()))
            })
            .collect();
        let mut tlb_blockers = self
            .tlb_blockers
            .union(&other.tlb_blockers)
            .cloned()
            .collect::<BTreeSet<_>>();
        if self.proven_tlb_entries != other.proven_tlb_entries {
            tlb_blockers.insert(TlbTransferBlockerV1::TlbPathDisagreement);
        }
        Self {
            registers,
            memory,
            staged_tlb,
            proven_tlb_entries,
            tlb_blockers,
        }
    }

    fn refine_unsigned_upper_bound(&mut self, register: u8, upper: u32) {
        if upper == 0 || upper as usize > MAX_VALUE_SET {
            return;
        }
        let range: BTreeSet<u32> = (0..upper).collect();
        let current = &self.registers[register as usize];
        // A `sltiu index,upper` proves only `index < upper` at runtime. When the
        // index's abstract value is a genuine compile-time register constant we
        // may intersect it with `[0,upper)` and select the exact case. But when
        // the index reached us *through a load* -- most commonly a mutable
        // global whose load-image byte was folded to a singleton by
        // `read_static_word` -- that singleton is only the initial value, not a
        // proven runtime value. The proven runtime universe is the whole
        // `[0,upper)` the compiler's own bound check guarantees. Trusting the
        // static singleton would both under-recover the table (one case instead
        // of the full switch) and, worse, could fabricate a wrong exhaustive
        // target if the initial byte selected a different case than runtime.
        // Widening a memory-derived index to the guaranteed range is therefore
        // strictly more sound and recovers the full jump table.
        let values: BTreeSet<u32> = match current.concrete_values() {
            Some(existing) if !current.through_memory => {
                existing.intersection(&range).copied().collect()
            }
            _ => range,
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
        self.staged_tlb = std::array::from_fn(|_| TrackedValue::unknown());
        self.proven_tlb_entries.clear();
        self.tlb_blockers
            .insert(TlbTransferBlockerV1::UnknownCallEffects);
    }

    fn staged_tlb_slot(cop0d: u8) -> Option<usize> {
        match cop0d {
            0 => Some(0),
            2 => Some(1),
            3 => Some(2),
            5 => Some(3),
            10 => Some(4),
            _ => None,
        }
    }

    fn stage_tlb_register(&mut self, cop0d: u8, value: TrackedValue) {
        if let Some(slot) = Self::staged_tlb_slot(cop0d) {
            self.staged_tlb[slot] = value;
        }
    }

    fn open_staged_tlb_register(&mut self, cop0d: u8) {
        if let Some(slot) = Self::staged_tlb_slot(cop0d) {
            self.staged_tlb[slot] = TrackedValue::unknown();
        }
    }

    fn record_tlbwi(&mut self, pc: u32) {
        let exact = self
            .staged_tlb
            .iter()
            .map(|value| {
                let values = value.concrete_values()?;
                (values.len() == 1 && !value.from_static_memory)
                    .then_some(*values.iter().next().unwrap())
            })
            .collect::<Option<Vec<_>>>();
        let Some(values) = exact else {
            for (slot, value) in self.staged_tlb.iter().enumerate() {
                let cop0d = [0, 2, 3, 5, 10][slot];
                if value.from_static_memory {
                    self.tlb_blockers
                        .insert(TlbTransferBlockerV1::MutableStaticMemorySource {
                            cop0d,
                            addresses: value.memory_sources.iter().copied().collect(),
                        });
                } else if value.concrete_values().is_none_or(|set| set.len() != 1) {
                    self.tlb_blockers
                        .insert(TlbTransferBlockerV1::TlbSetupOpen { cop0d });
                }
            }
            if let Some(index_values) = self.staged_tlb[0].concrete_values() {
                for index in index_values {
                    self.proven_tlb_entries.remove(&(index & 31));
                }
            } else {
                self.proven_tlb_entries.clear();
            }
            return;
        };
        let proof = TlbWriteProofV1 {
            tlbwi_pc: pc,
            index_raw: values[0],
            entry_lo0_raw: values[1],
            entry_lo1_raw: values[2],
            page_mask_raw: values[3],
            entry_hi_raw: values[4],
        };
        self.proven_tlb_entries.insert(values[0] & 31, proof);
    }
}

pub(super) fn value_locations(value: &TrackedValue) -> Option<Vec<MemoryLocation>> {
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

pub(super) fn read_static_word(bank_bytes: &[u8], va_start: u32, address: u32) -> Option<u32> {
    let offset = address.checked_sub(va_start)? as usize;
    let bytes = bank_bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes(bytes.try_into().ok()?))
}

pub(super) fn load_word(
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
                    read_static_word(bank_bytes, va_start, *address).map(|word| {
                        let mut value = TrackedValue::constant(word);
                        // Load-image bytes are candidate initial values, not
                        // immutable runtime bits. Keep the concrete value for
                        // diagnostics but require subsequent operations to
                        // establish any known-zero/known-one invariant.
                        value.known_zero = 0;
                        value.known_one = 0;
                        value.unchanged_static_word_source = Some(*address);
                        value
                    })
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

pub(super) fn store_word(state: &mut AnalysisState, address: &TrackedValue, value: TrackedValue) {
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

pub(super) fn execute_instruction(
    state: &mut AnalysisState,
    pc: u32,
    word: u32,
    bank_bytes: &[u8],
    va_start: u32,
) {
    match decode(word) {
        Instruction::Mtc0 { rt, cop0d } => {
            state.stage_tlb_register(cop0d, state.registers[rt as usize].clone());
        }
        Instruction::Dmtc0 { cop0d, .. } => {
            if AnalysisState::staged_tlb_slot(cop0d).is_some() {
                state.open_staged_tlb_register(cop0d);
                state
                    .tlb_blockers
                    .insert(TlbTransferBlockerV1::Dmtc0Unsupported { cop0d });
            }
        }
        Instruction::Tlbwi => state.record_tlbwi(pc),
        Instruction::Tlbwr => {
            state.proven_tlb_entries.clear();
            state
                .tlb_blockers
                .insert(TlbTransferBlockerV1::RandomIndexedWrite);
        }
        Instruction::Tlbp => state.open_staged_tlb_register(0),
        Instruction::Tlbr => {
            for cop0d in [2, 3, 5, 10] {
                state.open_staged_tlb_register(cop0d);
            }
        }
        _ => {}
    }
    let opcode = (word >> 26) & 0x3f;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    let shift = (word >> 6) & 0x1f;
    let immediate = (word & 0xffff) as i16 as i32;

    match opcode {
        0x00 => match word & 0x3f {
            0x00 if shift == 0 => {
                state.set_register(rd, state.registers[rt as usize].clone());
            }
            0x00 => {
                let value = state.registers[rt as usize].shift_left(shift);
                state.set_register(rd, value);
            }
            0x02 if shift == 0 => {
                state.set_register(rd, state.registers[rt as usize].clone());
            }
            0x02 => {
                let value = state.registers[rt as usize].shift_right(shift);
                state.set_register(rd, value);
            }
            0x20 | 0x21 | 0x2c | 0x2d if rt == 0 => {
                state.set_register(rd, state.registers[rs as usize].clone());
            }
            0x20 | 0x21 | 0x2c | 0x2d if rs == 0 => {
                state.set_register(rd, state.registers[rt as usize].clone());
            }
            0x20 | 0x21 | 0x2c | 0x2d => {
                let value = state.registers[rs as usize]
                    .binary(&state.registers[rt as usize], u32::wrapping_add);
                state.set_register(rd, value);
            }
            0x22 | 0x23 | 0x2e | 0x2f if rt == 0 => {
                state.set_register(rd, state.registers[rs as usize].clone());
            }
            0x22 | 0x23 | 0x2e | 0x2f => {
                let value = state.registers[rs as usize]
                    .binary(&state.registers[rt as usize], u32::wrapping_sub);
                state.set_register(rd, value);
            }
            0x24 if rt == 0 => {
                state.set_register(rd, TrackedValue::constant(0));
            }
            0x24 if rs == 0 => {
                state.set_register(rd, TrackedValue::constant(0));
            }
            0x24 => {
                let value = state.registers[rs as usize].bitand(&state.registers[rt as usize]);
                state.set_register(rd, value);
            }
            0x25 if rt == 0 => {
                state.set_register(rd, state.registers[rs as usize].clone());
            }
            0x25 if rs == 0 => {
                state.set_register(rd, state.registers[rt as usize].clone());
            }
            0x25 => {
                let value = state.registers[rs as usize].bitor(&state.registers[rt as usize]);
                state.set_register(rd, value);
            }
            0x26 => {
                let value = state.registers[rs as usize].bitxor(&state.registers[rt as usize]);
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
            let value = state.registers[rs as usize].bitand(&TrackedValue::constant(mask));
            state.set_register(rt, value);
        }
        0x0d => {
            let immediate = word & 0xffff;
            let value = if immediate == 0 {
                state.registers[rs as usize].clone()
            } else {
                state.registers[rs as usize].bitor(&TrackedValue::constant(immediate))
            };
            state.set_register(rt, value);
        }
        0x0e => {
            let immediate = word & 0xffff;
            let value = state.registers[rs as usize].bitxor(&TrackedValue::constant(immediate));
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
        0x0b => {
            // `sltiu $rt, $rs, imm16` -- the standard bounded-switch guard. Its
            // 0/1 result carries an `$rs < imm16` bound (imm16 zero-extended)
            // that a dominating `beq`/`bne $rt,$zero` consumes to refine `$rs`,
            // even across a block boundary. Numerically the flag is unknown.
            state.set_register(rt, TrackedValue::sltiu_flag(rs, word & 0xffff));
        }
        0x0a | 0x20..=0x22 | 0x24..=0x26 | 0x30 | 0x34 | 0x37 | 0x38 | 0x3c => {
            state.set_register(rt, TrackedValue::unknown());
        }
        0x10 if rs == 0 && rd == 12 => {
            state.set_register(rt, TrackedValue::known_zero(COP0_STATUS_BEV));
        }
        0x10..=0x13 if matches!(rs, 0x00..=0x02) => {
            state.set_register(rt, TrackedValue::unknown());
        }
        _ => {}
    }
}

pub(super) fn read_block_words(block: &BasicBlock, bank_bytes: &[u8], va_start: u32) -> Vec<(u32, u32)> {
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

pub(crate) fn written_gpr(word: u32) -> Option<u8> {
    let opcode = word >> 26;
    let rs = ((word >> 21) & 0x1f) as u8;
    let rt = ((word >> 16) & 0x1f) as u8;
    let rd = ((word >> 11) & 0x1f) as u8;
    match opcode {
        0x00 => (rd != 0).then_some(rd),
        0x01 if matches!(rt, 0x10..=0x13) => Some(31),
        0x03 => Some(31),
        0x08..=0x0f | 0x18..=0x1b | 0x20..=0x27 | 0x30..=0x38 | 0x3c => (rt != 0).then_some(rt),
        0x10..=0x13 if matches!(rs, 0x00..=0x02) => (rt != 0).then_some(rt),
        _ => None,
    }
}

/// The guarded successor and `(index_register, upper)` bound proved by a block
/// terminating in `beq`/`bne $pred,$zero` whose `$pred` is the abstract result
/// of a `sltiu index,upper`. `state` is the register file immediately before
/// the branch's delay slot -- the point at which `$pred` still holds the value
/// the branch tested. Reading the bound from `$pred`'s tracked provenance,
/// rather than by scanning this block's own words, lets the `sltiu` sit in a
/// dominating predecessor block (a compiler schedule the word-local scan
/// misses) without ever losing soundness: the tag is cleared the instant its
/// index register is rewritten, and a join of disagreeing tags drops it.
pub(super) fn threaded_branch_bound(
    state: &AnalysisState,
    words: &[(u32, u32)],
    terminator: &BlockTerminator,
) -> Option<(u32, u8, u32)> {
    let (target, fallthrough) = match terminator {
        BlockTerminator::Branch {
            target,
            fallthrough,
            ..
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
            ..
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
    let (index, upper) = state.registers[predicate as usize].sltiu_bound?;
    // beq $pred,$zero takes the branch when the sltiu returned 0 (index >=
    // upper): the in-bounds edge is the fallthrough. bne is the mirror image.
    let valid_successor = if opcode == 0x04 { fallthrough } else { target };
    Some((valid_successor, index, upper))
}

pub(super) fn branch_bound(words: &[(u32, u32)], terminator: &BlockTerminator) -> Option<(u32, u8, u32)> {
    let (target, fallthrough) = match terminator {
        BlockTerminator::Branch {
            target,
            fallthrough,
            ..
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
            ..
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

pub(super) fn block_successors(block: &BasicBlock) -> Vec<u32> {
    match &block.terminator {
        BlockTerminator::Fallthrough { next } => vec![*next],
        BlockTerminator::Tail { target } => vec![*target],
        BlockTerminator::Call { next, .. } => vec![*next],
        BlockTerminator::Branch {
            target,
            fallthrough,
            ..
        }
        | BlockTerminator::BranchLikely {
            target,
            fallthrough,
            ..
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
        | BlockTerminator::RanOffEnd
        | BlockTerminator::DataFence { .. }
        | BlockTerminator::SelfReferentialBranch { .. } => Vec::new(),
    }
}

pub(super) fn resolution_from_value(site_pc: u32, via_call: bool, value: &TrackedValue) -> IndirectResolution {
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

/// Interprocedural `$a0..$a3` seeds, keyed by function-entry VA.
///
/// A missing entry or a `None` slot means the register starts Unknown, which
/// is byte-identical to the unseeded analysis for that entry.
pub(super) type EntryArgumentSeeds = BTreeMap<u32, [Option<TrackedValue>; 4]>;

/// Derive sound `$a0..$a3` entry seeds from certified call-boundary proofs.
///
/// A seed asserts "every runtime activation of this entry receives exactly
/// these argument values", so admission must rule out callers the analysis
/// cannot see. Three conditions, all fail-closed:
///
/// 1. The entry has at least one certified caller: a call-boundary proof
///    whose callee is `Direct` to it or whose `ResolvedIndirect` target set
///    contains it. Resolved-indirect callers are exhaustive by construction,
///    so they are known callers, not a hole.
/// 2. The entry's address is never materialized as a value: its VA does not
///    occur as an aligned word anywhere in the bank image (pointer tables),
///    and no address-ordered `lui`+`addiu`/`ori` pair in the image constructs
///    it. Without a materialized address, a still-open indirect site cannot
///    reach this entry, so the certified callers are all the callers.
///    The scan is deliberately over-broad -- a code word that merely encodes
///    the same bits also rejects -- because over-rejection only loses a seed.
///    Residual risk: a construction scheduled across a backward branch feeds
///    a still-open site; the six-game wrong==0 grading is the measured
///    firewall for that gap.
/// 3. Per register, every certified caller proves the same `Concrete` value
///    set with no blockers. Disagreement or opacity degrades that register
///    to Unknown rather than rejecting the whole entry.
pub(super) fn compute_entry_argument_seeds(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    analysis_roots: &[u32],
) -> EntryArgumentSeeds {
    let boundaries =
        super::analyses::analyze_call_boundaries_from_roots(cfg, bank_bytes, va_start, analysis_roots);

    let mut callers: BTreeMap<u32, Vec<&CallBoundaryProofV1>> = BTreeMap::new();
    for call in &boundaries.calls {
        match &call.callee {
            CallBoundaryCalleeV1::Direct { target } => {
                callers.entry(*target).or_default().push(call);
            }
            CallBoundaryCalleeV1::ResolvedIndirect { targets } => {
                for target in targets {
                    callers.entry(*target).or_default().push(call);
                }
            }
        }
    }

    // Condition 2: collect every VA that is materialized in the image, either
    // as an aligned data word or by an address-ordered lui/addiu|ori pair.
    let mut materialized: BTreeSet<u32> = BTreeSet::new();
    let mut lui_imm: [Option<u16>; 32] = [None; 32];
    for chunk_start in (0..bank_bytes.len().saturating_sub(3)).step_by(4) {
        let word = u32::from_be_bytes([
            bank_bytes[chunk_start],
            bank_bytes[chunk_start + 1],
            bank_bytes[chunk_start + 2],
            bank_bytes[chunk_start + 3],
        ]);
        materialized.insert(word);
        let op = word >> 26;
        let rs = ((word >> 21) & 0x1f) as usize;
        let rt = ((word >> 16) & 0x1f) as usize;
        let imm = (word & 0xffff) as u16;
        match op {
            0x0f => lui_imm[rt] = Some(imm), // lui
            0x09 => {
                // addiu: sign-extended low half
                if let Some(hi) = lui_imm[rs] {
                    materialized
                        .insert(((hi as u32) << 16).wrapping_add(imm as i16 as i32 as u32));
                }
                if rt != rs {
                    lui_imm[rt] = None;
                }
            }
            0x0d => {
                // ori: zero-extended low half
                if let Some(hi) = lui_imm[rs] {
                    materialized.insert(((hi as u32) << 16) | imm as u32);
                }
                if rt != rs {
                    lui_imm[rt] = None;
                }
            }
            _ => {
                // Any other write to rt invalidates a pending lui in it. The
                // written-register decoder covers the forms that matter here.
                if let Some(written) = written_gpr(word) {
                    lui_imm[written as usize] = None;
                }
            }
        }
    }

    let root_set: BTreeSet<u32> = analysis_roots.iter().copied().collect();
    let mut seeds = EntryArgumentSeeds::new();
    for (entry, proofs) in callers {
        if !root_set.contains(&entry) || materialized.contains(&entry) {
            continue;
        }
        let mut slots: [Option<TrackedValue>; 4] = [None, None, None, None];
        for slot in 0..4u8 {
            let register = 4 + slot;
            let mut merged: Option<BTreeSet<u32>> = None;
            let mut sound = true;
            for proof in &proofs {
                let Some(reg_proof) = proof.registers.iter().find(|p| p.register == register)
                else {
                    sound = false;
                    break;
                };
                if !reg_proof.blockers.is_empty() {
                    sound = false;
                    break;
                }
                let CallBoundaryValueV1::Concrete { values } = &reg_proof.value else {
                    sound = false;
                    break;
                };
                let values: BTreeSet<u32> = values.iter().copied().collect();
                match &merged {
                    None => merged = Some(values),
                    Some(existing) if *existing == values => {}
                    Some(_) => {
                        // Condition 3: cross-caller disagreement degrades the
                        // register, never joins the sets -- a join would claim
                        // a per-activation value no single caller proves.
                        sound = false;
                        break;
                    }
                }
            }
            if sound {
                if let Some(values) = merged {
                    slots[slot as usize] = Some(TrackedValue::concrete(values));
                }
            }
        }
        if slots.iter().any(Option::is_some) {
            seeds.insert(entry, slots);
        }
    }
    seeds
}

/// Run bounded forward value-set analysis over the currently reachable CFG.
/// Joins that exceed [`MAX_VALUE_SET`] become `open`; no widening guesses a
/// target. Bounds from `sltiu` + dominating `beq`/`bne` edges refine only the
/// guarded successor, so a path that bypasses the check joins back to `open`.
pub fn resolve_value_sets(cfg: &Cfg, bank_bytes: &[u8], va_start: u32) -> Vec<IndirectResolution> {
    resolve_value_sets_from_roots(cfg, bank_bytes, va_start, &cfg.proven_roots)
}

pub(super) fn resolve_value_sets_from_roots(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    analysis_roots: &[u32],
) -> Vec<IndirectResolution> {
    // Interprocedural argument seeds are derived here, once, and then applied
    // to the same forward pass every other consumer already runs. Deriving
    // them inside this entry point means every caller of the resolver benefits
    // without threading a new argument through its own signature; an entry
    // that fails admission simply gets no seed, so the pass is identical to
    // the unseeded one for it.
    let seeds = compute_entry_argument_seeds(cfg, bank_bytes, va_start, analysis_roots);
    resolve_value_sets_from_roots_observing(
        cfg,
        bank_bytes,
        va_start,
        analysis_roots,
        None,
        None,
        None,
        None,
        &seeds,
    )
}

pub(super) fn resolve_value_sets_from_roots_observing(
    cfg: &Cfg,
    bank_bytes: &[u8],
    va_start: u32,
    analysis_roots: &[u32],
    mut stores: Option<&mut BTreeMap<u32, BTreeSet<RawWordStoreObservation>>>,
    mut status_writes: Option<&mut BTreeMap<u32, BTreeSet<RawCop0StatusObservation>>>,
    mut tlb_transfers: Option<&mut BTreeMap<u32, Vec<RawTlbTransferObservation>>>,
    mut call_boundaries: Option<(
        &[u8],
        &mut BTreeMap<u32, BTreeSet<RawCallBoundaryObservation>>,
    )>,
    entry_seeds: &EntryArgumentSeeds,
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
        let mut root_state = AnalysisState::at_root(root);
        // Interprocedural admission happened in compute_entry_argument_seeds;
        // an absent entry leaves every argument register Unknown, identical
        // to the historical unseeded analysis.
        if let Some(slots) = entry_seeds.get(&root) {
            for (index, slot) in slots.iter().enumerate() {
                if let Some(value) = slot {
                    root_state.set_register(4 + index as u8, value.clone());
                }
            }
        }
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
        let words = read_block_words(block, bank_bytes, va_start);
        let visit = visits.entry(start).or_default();
        *visit += 1;
        let mut widened_visit = false;
        if *visit > MAX_BLOCK_REVISITS {
            let mut widened = state.widened_preserving_tlb();
            if words.iter().any(|(_, word)| {
                matches!(
                    decode(*word),
                    Instruction::Mtc0 {
                        cop0d: 0 | 2 | 3 | 5 | 10,
                        ..
                    } | Instruction::Dmtc0 {
                        cop0d: 0 | 2 | 3 | 5 | 10,
                        ..
                    } | Instruction::Tlbwi
                        | Instruction::Tlbwr
                        | Instruction::Tlbp
                        | Instruction::Tlbr
                )
            }) {
                widened.staged_tlb = std::array::from_fn(|_| TrackedValue::unknown());
                widened.proven_tlb_entries.clear();
                widened
                    .tlb_blockers
                    .insert(TlbTransferBlockerV1::RevisitWidened);
            }
            if state == widened {
                continue;
            }
            incoming.insert(start, widened.clone());
            state = widened;
            widened_visit = true;
        }
        let transfer_pc = block.end_va.checked_sub(8);
        let delay_pc = block.end_va.checked_sub(4);
        let mut before_delay = None;
        let mut pending_tlb_transfer = None;
        for &(pc, word) in &words {
            if Some(pc) == delay_pc {
                before_delay = Some(state.clone());
            }
            if Some(pc) == transfer_pc {
                // The ordinary target verdict meets only real (non-widened)
                // visits; widening can only lose target precision. The TLB
                // observer below retains widened visits as explicit blockers,
                // because an active mapping must agree after every delay slot.
                if let BlockTerminator::Indirect { via_call }
                | BlockTerminator::ResolvedIndirect { via_call, .. } = &block.terminator
                {
                    let register = ((word >> 21) & 0x1f) as usize;
                    let candidate =
                        resolution_from_value(pc, *via_call, &state.registers[register]);
                    pending_tlb_transfer = Some(candidate.clone());
                    if !widened_visit {
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
            }
            if word >> 26 == 0x2b {
                if let Some(stores) = stores.as_deref_mut() {
                    let base = ((word >> 21) & 0x1f) as usize;
                    let value_register = ((word >> 16) & 0x1f) as usize;
                    let immediate = (word & 0xffff) as i16 as i32;
                    let address = state.registers[base].add_immediate(immediate);
                    stores
                        .entry(pc)
                        .or_default()
                        .insert(RawWordStoreObservation {
                            addresses: concrete_values(&address),
                            values: concrete_values(&state.registers[value_register]),
                            unchanged_static_word_source: state.registers[value_register]
                                .unchanged_static_word_source,
                            widened: widened_visit,
                        });
                }
            }
            if matches!(
                decode(word),
                Instruction::Mtc0 { cop0d: 12, .. } | Instruction::Dmtc0 { cop0d: 12, .. }
            ) {
                if let Some(status_writes) = status_writes.as_deref_mut() {
                    let source_register = ((word >> 16) & 0x1f) as usize;
                    let value = &state.registers[source_register];
                    status_writes
                        .entry(pc)
                        .or_default()
                        .insert(RawCop0StatusObservation {
                            values: concrete_values(value),
                            known_zero: value.known_zero,
                            known_one: value.known_one,
                            memory_sources: value.memory_sources.iter().copied().collect(),
                            from_static_memory: value.from_static_memory,
                            widened: widened_visit,
                        });
                }
            }
            execute_instruction(&mut state, pc, word, bank_bytes, va_start);
        }

        if let (Some(candidate), Some(tlb_transfers)) =
            (pending_tlb_transfer, tlb_transfers.as_deref_mut())
        {
            let entry_hi = &state.staged_tlb[4];
            let mut blockers = state.tlb_blockers.clone();
            let entry_hi_at_transfer = entry_hi.concrete_values().and_then(|values| {
                (values.len() == 1 && !entry_hi.from_static_memory)
                    .then(|| u64::from(*values.iter().next().unwrap()))
            });
            if entry_hi_at_transfer.is_none() {
                if entry_hi.from_static_memory {
                    blockers.insert(TlbTransferBlockerV1::MutableStaticMemorySource {
                        cop0d: 10,
                        addresses: entry_hi.memory_sources.iter().copied().collect(),
                    });
                } else {
                    blockers.insert(TlbTransferBlockerV1::TlbSetupOpen { cop0d: 10 });
                }
            }
            tlb_transfers
                .entry(candidate.site_pc)
                .or_default()
                .push(RawTlbTransferObservation {
                    target: candidate,
                    entry_hi_at_transfer,
                    active_writes: state.proven_tlb_entries.values().cloned().collect(),
                    blockers: blockers.into_iter().collect(),
                    widened: widened_visit,
                });
        }

        // The branch tests `$pred` before its delay slot runs, so the bound is
        // read from the pre-delay register file. Falling back to the word-local
        // scan keeps behavior for any branch shape the threaded reader declines.
        let bound = before_delay
            .as_ref()
            .and_then(|pre| threaded_branch_bound(pre, &words, &block.terminator))
            .or_else(|| branch_bound(&words, &block.terminator));

        let is_call = matches!(
            block.terminator,
            BlockTerminator::Call { .. }
                | BlockTerminator::Indirect { via_call: true }
                | BlockTerminator::ResolvedIndirect { via_call: true, .. }
        );
        if is_call {
            if let Some((requested_registers, observations)) = call_boundaries.as_mut() {
                let registers = requested_registers
                    .iter()
                    .copied()
                    .map(|register| {
                        let value = &state.registers[register as usize];
                        RawCallRegisterObservation {
                            register,
                            value: value.value.clone(),
                            memory_sources: value.memory_sources.iter().copied().collect(),
                            through_memory: value.through_memory,
                            from_static_memory: value.from_static_memory,
                        }
                    })
                    .collect();
                observations
                    .entry(transfer_pc.expect("call block includes control and delay words"))
                    .or_default()
                    .insert(RawCallBoundaryObservation {
                        registers,
                        widened: widened_visit,
                    });
            }
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

pub(super) fn concrete_values(value: &TrackedValue) -> Option<Vec<u32>> {
    value
        .concrete_values()
        .map(|values| values.iter().copied().collect())
}
