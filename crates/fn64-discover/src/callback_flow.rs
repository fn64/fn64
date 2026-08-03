//! Mechanical discovery of callback-argument contracts.
//!
//! A callable owner is a callback consumer when an o32 argument reaches the
//! target register of a reachable `jalr`. This is a data-flow statement, not
//! a name/signature guess: registers, saved-register moves, and stack
//! spill/reload pairs are tracked over the owner's CFG. Merged disagreement,
//! arithmetic, loads from non-stack memory, and call-clobbered values become
//! unknown and confer no authority.

use crate::cfg::{BasicBlock, BlockTerminator, Cfg};
use crate::partition::partition;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallbackArgumentContract {
    pub callee: u32,
    pub pointer_arg_register: u8,
    pub jalr_sites: Vec<u32>,
}

/// A list head reached as `*(pointer_word_address) + field_offset`.
/// Keeping both steps explicit distinguishes MM's context-pointer field from
/// an absolute global and lets independent registrar/dispatcher owners agree
/// without knowing either function's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IndirectListHead {
    pub pointer_word_address: u32,
    pub field_offset: i32,
}

/// Cross-function proof that one o32 argument is stored as a callback in an
/// intrusive-list node and a reachable dispatcher invokes that exact field.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CallbackRegistryContract {
    pub registrar: u32,
    pub dispatcher: u32,
    pub object_arg_register: u8,
    pub pointer_arg_register: u8,
    pub callback_offset: i32,
    pub link_offset: i32,
    pub list_head: IndirectListHead,
    pub callback_store_site: u32,
    pub list_insert_site: u32,
    pub jalr_site: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackFlowError {
    UnalignedImageStart { va_start: u32 },
    ImageAddressOverflow,
    BlockOutsideImage { start: u32, end: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FlowValue {
    Unknown,
    Argument(u8),
    Stack(i32),
}

impl FlowValue {
    fn join(self, other: Self) -> Self {
        if self == other {
            self
        } else {
            Self::Unknown
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FlowState {
    registers: [FlowValue; 32],
    stack_words: BTreeMap<i32, FlowValue>,
}

impl FlowState {
    fn at_entry() -> Self {
        let mut registers = [FlowValue::Unknown; 32];
        for register in 4..=7 {
            registers[register] = FlowValue::Argument(register as u8);
        }
        registers[29] = FlowValue::Stack(0);
        Self {
            registers,
            stack_words: BTreeMap::new(),
        }
    }

    fn set_register(&mut self, register: usize, value: FlowValue) {
        if register != 0 {
            self.registers[register] = value;
        }
    }

    fn join(&self, other: &Self) -> Self {
        let registers =
            std::array::from_fn(|index| self.registers[index].join(other.registers[index]));
        let stack_words = self
            .stack_words
            .iter()
            .filter_map(|(offset, left)| {
                other.stack_words.get(offset).and_then(|right| {
                    let joined = left.join(*right);
                    (joined != FlowValue::Unknown).then_some((*offset, joined))
                })
            })
            .collect();
        Self {
            registers,
            stack_words,
        }
    }

    fn clobber_caller_saved(&mut self) {
        for register in 2..=15 {
            self.registers[register] = FlowValue::Unknown;
        }
        self.registers[24] = FlowValue::Unknown;
        self.registers[25] = FlowValue::Unknown;
        self.registers[31] = FlowValue::Unknown;
    }
}

fn op(word: u32) -> u32 {
    word >> 26
}

fn rs(word: u32) -> usize {
    ((word >> 21) & 31) as usize
}

fn rt(word: u32) -> usize {
    ((word >> 16) & 31) as usize
}

fn rd(word: u32) -> usize {
    ((word >> 11) & 31) as usize
}

fn immediate(word: u32) -> i32 {
    i32::from(word as i16)
}

fn move_source(word: u32) -> Option<usize> {
    if op(word) != 0 || !matches!(word & 0x3f, 0x20 | 0x21 | 0x25 | 0x2c | 0x2d) {
        return None;
    }
    match (rs(word), rt(word)) {
        (0, source) | (source, 0) => Some(source),
        _ => None,
    }
}

fn execute_word(state: &mut FlowState, word: u32) {
    match op(word) {
        0 => {
            let destination = rd(word);
            if let Some(source) = move_source(word) {
                state.set_register(destination, state.registers[source]);
            } else if destination != 0 && !matches!(word & 0x3f, 0x08 | 0x0c | 0x0d) {
                state.set_register(destination, FlowValue::Unknown);
            } else if word & 0x3f == 0x09 {
                state.set_register(destination, FlowValue::Unknown);
            }
        }
        // addi/addiu/daddi/daddiu preserve only exact moves and the stack root.
        0x08 | 0x09 | 0x18 | 0x19 => {
            let value = match state.registers[rs(word)] {
                FlowValue::Stack(offset) => FlowValue::Stack(offset.wrapping_add(immediate(word))),
                value if immediate(word) == 0 => value,
                _ => FlowValue::Unknown,
            };
            state.set_register(rt(word), value);
        }
        // OR-immediate zero is another compiler spelling of a move.
        0x0d if word as u16 == 0 => {
            state.set_register(rt(word), state.registers[rs(word)]);
        }
        0x0f | 0x0a..=0x0c | 0x0e => state.set_register(rt(word), FlowValue::Unknown),
        // Direct calls and branch-and-link overwrite the link register before
        // their delay slot executes. Leaving a prior argument tag in `$ra`
        // could otherwise fabricate a later callback contract.
        0x03 => state.set_register(31, FlowValue::Unknown),
        0x01 if matches!(rt(word), 0x10..=0x13) => {
            state.set_register(31, FlowValue::Unknown);
        }
        // Word load/store are sufficient for o32 argument spills. Other loads
        // conservatively clobber their destination below.
        0x23 => {
            let value = match state.registers[rs(word)] {
                FlowValue::Stack(base) => state
                    .stack_words
                    .get(&base.wrapping_add(immediate(word)))
                    .copied()
                    .unwrap_or(FlowValue::Unknown),
                _ => FlowValue::Unknown,
            };
            state.set_register(rt(word), value);
        }
        0x2b => {
            if let FlowValue::Stack(base) = state.registers[rs(word)] {
                state.stack_words.insert(
                    base.wrapping_add(immediate(word)),
                    state.registers[rt(word)],
                );
            }
        }
        0x20..=0x27 | 0x30 | 0x34 | 0x37 | 0x1a | 0x1b => {
            state.set_register(rt(word), FlowValue::Unknown);
        }
        // COP move-from forms and store-conditional write a GPR.
        0x10..=0x13 if matches!(rs(word), 0x00..=0x02) => {
            state.set_register(rt(word), FlowValue::Unknown);
        }
        0x38 | 0x3c => state.set_register(rt(word), FlowValue::Unknown),
        _ => {}
    }
}

fn read_words<'a>(
    block: &BasicBlock,
    bytes: &'a [u8],
    va_start: u32,
) -> Result<impl Iterator<Item = (u32, u32)> + 'a, CallbackFlowError> {
    let start =
        block
            .start_va
            .checked_sub(va_start)
            .ok_or(CallbackFlowError::BlockOutsideImage {
                start: block.start_va,
                end: block.end_va,
            })? as usize;
    let end = block
        .end_va
        .checked_sub(va_start)
        .ok_or(CallbackFlowError::BlockOutsideImage {
            start: block.start_va,
            end: block.end_va,
        })? as usize;
    let slice = bytes
        .get(start..end)
        .filter(|slice| slice.len().is_multiple_of(4))
        .ok_or(CallbackFlowError::BlockOutsideImage {
            start: block.start_va,
            end: block.end_va,
        })?;
    let block_start = block.start_va;
    Ok(slice.chunks_exact(4).enumerate().map(move |(index, word)| {
        (
            block_start + index as u32 * 4,
            u32::from_be_bytes(word.try_into().expect("four-byte instruction")),
        )
    }))
}

fn transfer_block(
    block: &BasicBlock,
    bytes: &[u8],
    va_start: u32,
    input: &FlowState,
    annul_final_word: bool,
    mut callback: impl FnMut(u32, u8),
) -> Result<FlowState, CallbackFlowError> {
    let mut state = input.clone();
    for (pc, word) in read_words(block, bytes, va_start)? {
        if annul_final_word && pc.checked_add(4) == Some(block.end_va) {
            break;
        }
        if op(word) == 0 && word & 0x3f == 0x09 {
            if let FlowValue::Argument(register) = state.registers[rs(word)] {
                callback(pc, register);
            }
        }
        execute_word(&mut state, word);
    }
    if matches!(
        block.terminator,
        BlockTerminator::Call { .. }
            | BlockTerminator::Indirect { via_call: true }
            | BlockTerminator::ResolvedIndirect { via_call: true, .. }
    ) {
        state.clobber_caller_saved();
    }
    Ok(state)
}

fn successors(block: &BasicBlock) -> Vec<(u32, bool)> {
    match &block.terminator {
        BlockTerminator::Fallthrough { next } => vec![(*next, false)],
        BlockTerminator::Tail { target } => vec![(*target, false)],
        BlockTerminator::Call { next, .. } => vec![(*next, false)],
        BlockTerminator::Branch {
            target,
            fallthrough,
            ..
        } => vec![(*target, false), (*fallthrough, false)],
        BlockTerminator::BranchLikely {
            target,
            fallthrough,
            ..
        } => vec![(*target, false), (*fallthrough, true)],
        BlockTerminator::ResolvedIndirect {
            targets,
            via_call: false,
        } => targets
            .iter()
            .copied()
            .map(|target| (target, false))
            .collect(),
        BlockTerminator::ResolvedIndirect { via_call: true, .. }
        | BlockTerminator::Indirect { via_call: true } => vec![(block.end_va, false)],
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

/// Discover every exact o32 argument that reaches a reachable `jalr` target
/// within an uncontested owner of `cfg`.
pub fn discover_callback_argument_contracts(
    cfg: &Cfg,
    bytes: &[u8],
    va_start: u32,
) -> Result<Vec<CallbackArgumentContract>, CallbackFlowError> {
    if !va_start.is_multiple_of(4) {
        return Err(CallbackFlowError::UnalignedImageStart { va_start });
    }
    va_start
        .checked_add(
            u32::try_from(bytes.len()).map_err(|_| CallbackFlowError::ImageAddressOverflow)?,
        )
        .ok_or(CallbackFlowError::ImageAddressOverflow)?;
    let blocks: BTreeMap<u32, &BasicBlock> = cfg
        .blocks
        .iter()
        .map(|block| (block.start_va, block))
        .collect();
    let mut discovered: BTreeMap<(u32, u8), BTreeSet<u32>> = BTreeMap::new();

    for owner in partition(cfg).owners {
        let owned: BTreeSet<u32> = owner.block_starts.iter().copied().collect();
        if !owned.contains(&owner.root_va) {
            continue;
        }
        let mut inputs = BTreeMap::from([(owner.root_va, FlowState::at_entry())]);
        let mut work = VecDeque::from([owner.root_va]);
        while let Some(start) = work.pop_front() {
            let Some(block) = blocks.get(&start).copied() else {
                continue;
            };
            let output = transfer_block(block, bytes, va_start, &inputs[&start], false, |_, _| {})?;
            for (successor, annulled_delay) in successors(block) {
                if !owned.contains(&successor) {
                    continue;
                }
                let candidate = if annulled_delay {
                    transfer_block(block, bytes, va_start, &inputs[&start], true, |_, _| {})?
                } else {
                    output.clone()
                };
                let joined = inputs
                    .get(&successor)
                    .map(|existing| existing.join(&candidate))
                    .unwrap_or(candidate);
                if inputs.get(&successor) != Some(&joined) {
                    inputs.insert(successor, joined);
                    work.push_back(successor);
                }
            }
        }

        for (&start, input) in &inputs {
            let Some(block) = blocks.get(&start).copied() else {
                continue;
            };
            transfer_block(block, bytes, va_start, input, false, |site, register| {
                discovered
                    .entry((owner.root_va, register))
                    .or_default()
                    .insert(site);
            })?;
        }
    }

    Ok(discovered
        .into_iter()
        .map(
            |((callee, pointer_arg_register), sites)| CallbackArgumentContract {
                callee,
                pointer_arg_register,
                jalr_sites: sites.into_iter().collect(),
            },
        )
        .collect())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldValue {
    Unknown,
    Argument(u8),
    Stack(i32),
    Constant(u32),
    LoadedAbsolute(u32),
    Cursor(IndirectListHead),
    CursorField { head: IndirectListHead, offset: i32 },
}

impl FieldValue {
    fn join(self, other: Self) -> Self {
        if self == other {
            self
        } else {
            Self::Unknown
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FieldState {
    registers: [FieldValue; 32],
    stack_words: BTreeMap<i32, FieldValue>,
}

impl FieldState {
    fn at_entry() -> Self {
        let mut registers = [FieldValue::Unknown; 32];
        registers[0] = FieldValue::Constant(0);
        for register in 4..=7 {
            registers[register] = FieldValue::Argument(register as u8);
        }
        registers[29] = FieldValue::Stack(0);
        Self {
            registers,
            stack_words: BTreeMap::new(),
        }
    }

    fn set_register(&mut self, register: usize, value: FieldValue) {
        if register != 0 {
            self.registers[register] = value;
        }
    }

    fn join(&self, other: &Self) -> Self {
        let registers =
            std::array::from_fn(|index| self.registers[index].join(other.registers[index]));
        let stack_words = self
            .stack_words
            .iter()
            .filter_map(|(offset, left)| {
                other.stack_words.get(offset).and_then(|right| {
                    let joined = left.join(*right);
                    (joined != FieldValue::Unknown).then_some((*offset, joined))
                })
            })
            .collect();
        Self {
            registers,
            stack_words,
        }
    }

    fn clobber_caller_saved(&mut self) {
        for register in 2..=15 {
            self.registers[register] = FieldValue::Unknown;
        }
        self.registers[24] = FieldValue::Unknown;
        self.registers[25] = FieldValue::Unknown;
        self.registers[31] = FieldValue::Unknown;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum FieldEvent {
    HeadLoad {
        site: u32,
        head: IndirectListHead,
    },
    CursorFieldLoad {
        site: u32,
        head: IndirectListHead,
        offset: i32,
    },
    CallbackStore {
        site: u32,
        object_arg: u8,
        callback_arg: u8,
        offset: i32,
    },
    LinkStore {
        site: u32,
        object_arg: u8,
        head: IndirectListHead,
        offset: i32,
    },
    HeadStore {
        site: u32,
        head: IndirectListHead,
        object_arg: u8,
    },
    JalrField {
        site: u32,
        head: IndirectListHead,
        offset: i32,
    },
}

fn effective(base: u32, offset: i32) -> u32 {
    base.wrapping_add(offset as u32)
}

fn execute_field_word(
    state: &mut FieldState,
    pc: u32,
    word: u32,
    image_start: u32,
    image_end: u32,
    cursor_link: Option<(IndirectListHead, i32)>,
    events: &mut BTreeSet<FieldEvent>,
) {
    if op(word) == 0 && word & 0x3f == 0x09 {
        if let FieldValue::CursorField { head, offset } = state.registers[rs(word)] {
            events.insert(FieldEvent::JalrField {
                site: pc,
                head,
                offset,
            });
        }
    }

    match op(word) {
        0 => {
            let destination = rd(word);
            if let Some(source) = move_source(word) {
                state.set_register(destination, state.registers[source]);
            } else if destination != 0 && !matches!(word & 0x3f, 0x08 | 0x0c | 0x0d) {
                state.set_register(destination, FieldValue::Unknown);
            } else if word & 0x3f == 0x09 {
                state.set_register(destination, FieldValue::Unknown);
            }
        }
        0x08 | 0x09 | 0x18 | 0x19 => {
            let offset = immediate(word);
            let value = match state.registers[rs(word)] {
                FieldValue::Stack(base) => FieldValue::Stack(base.wrapping_add(offset)),
                FieldValue::Constant(base) => FieldValue::Constant(effective(base, offset)),
                value if offset == 0 => value,
                _ => FieldValue::Unknown,
            };
            state.set_register(rt(word), value);
        }
        0x0d => {
            let value = match state.registers[rs(word)] {
                FieldValue::Constant(base) => FieldValue::Constant(base | u32::from(word as u16)),
                value if word as u16 == 0 => value,
                _ => FieldValue::Unknown,
            };
            state.set_register(rt(word), value);
        }
        0x0f => state.set_register(rt(word), FieldValue::Constant(u32::from(word as u16) << 16)),
        0x0a..=0x0c | 0x0e => state.set_register(rt(word), FieldValue::Unknown),
        0x03 => state.set_register(31, FieldValue::Unknown),
        0x01 if matches!(rt(word), 0x10..=0x13) => {
            state.set_register(31, FieldValue::Unknown);
        }
        0x23 => {
            let offset = immediate(word);
            let value = match state.registers[rs(word)] {
                FieldValue::Stack(base) => state
                    .stack_words
                    .get(&base.wrapping_add(offset))
                    .copied()
                    .unwrap_or(FieldValue::Unknown),
                FieldValue::Constant(base) => {
                    let address = effective(base, offset);
                    if address.is_multiple_of(4) && address >= image_start && address < image_end {
                        FieldValue::LoadedAbsolute(address)
                    } else {
                        FieldValue::Unknown
                    }
                }
                FieldValue::LoadedAbsolute(pointer_word_address) => {
                    let head = IndirectListHead {
                        pointer_word_address,
                        field_offset: offset,
                    };
                    events.insert(FieldEvent::HeadLoad { site: pc, head });
                    FieldValue::Cursor(head)
                }
                FieldValue::Cursor(head) => {
                    events.insert(FieldEvent::CursorFieldLoad {
                        site: pc,
                        head,
                        offset,
                    });
                    if cursor_link == Some((head, offset)) {
                        FieldValue::Cursor(head)
                    } else {
                        FieldValue::CursorField { head, offset }
                    }
                }
                _ => FieldValue::Unknown,
            };
            state.set_register(rt(word), value);
        }
        0x2b => {
            let offset = immediate(word);
            let base = state.registers[rs(word)];
            let value = state.registers[rt(word)];
            match (base, value) {
                (FieldValue::Stack(stack), value) => {
                    state.stack_words.insert(stack.wrapping_add(offset), value);
                }
                (FieldValue::Argument(object_arg), FieldValue::Argument(callback_arg))
                    if object_arg != callback_arg =>
                {
                    events.insert(FieldEvent::CallbackStore {
                        site: pc,
                        object_arg,
                        callback_arg,
                        offset,
                    });
                }
                (FieldValue::Argument(object_arg), FieldValue::Cursor(head)) => {
                    events.insert(FieldEvent::LinkStore {
                        site: pc,
                        object_arg,
                        head,
                        offset,
                    });
                }
                (
                    FieldValue::LoadedAbsolute(pointer_word_address),
                    FieldValue::Argument(object_arg),
                ) => {
                    events.insert(FieldEvent::HeadStore {
                        site: pc,
                        head: IndirectListHead {
                            pointer_word_address,
                            field_offset: offset,
                        },
                        object_arg,
                    });
                }
                _ => {}
            }
        }
        0x20..=0x27 | 0x30 | 0x34 | 0x37 | 0x1a | 0x1b => {
            state.set_register(rt(word), FieldValue::Unknown);
        }
        0x10..=0x13 if matches!(rs(word), 0x00..=0x02) => {
            state.set_register(rt(word), FieldValue::Unknown);
        }
        0x38 | 0x3c => state.set_register(rt(word), FieldValue::Unknown),
        _ => {}
    }
}

fn transfer_field_block(
    block: &BasicBlock,
    bytes: &[u8],
    va_start: u32,
    image_end: u32,
    input: &FieldState,
    annul_final_word: bool,
    cursor_link: Option<(IndirectListHead, i32)>,
    events: &mut BTreeSet<FieldEvent>,
) -> Result<FieldState, CallbackFlowError> {
    let mut state = input.clone();
    for (pc, word) in read_words(block, bytes, va_start)? {
        if annul_final_word && pc.checked_add(4) == Some(block.end_va) {
            break;
        }
        execute_field_word(
            &mut state,
            pc,
            word,
            va_start,
            image_end,
            cursor_link,
            events,
        );
    }
    if matches!(
        block.terminator,
        BlockTerminator::Call { .. }
            | BlockTerminator::Indirect { via_call: true }
            | BlockTerminator::ResolvedIndirect { via_call: true, .. }
    ) {
        state.clobber_caller_saved();
    }
    Ok(state)
}

fn analyze_field_owner(
    owner_root: u32,
    owned: &BTreeSet<u32>,
    blocks: &BTreeMap<u32, &BasicBlock>,
    bytes: &[u8],
    va_start: u32,
    image_end: u32,
    cursor_link: Option<(IndirectListHead, i32)>,
) -> Result<BTreeSet<FieldEvent>, CallbackFlowError> {
    let mut inputs = BTreeMap::from([(owner_root, FieldState::at_entry())]);
    let mut work = VecDeque::from([owner_root]);
    let mut discarded_events = BTreeSet::new();
    while let Some(start) = work.pop_front() {
        let Some(block) = blocks.get(&start).copied() else {
            continue;
        };
        let output = transfer_field_block(
            block,
            bytes,
            va_start,
            image_end,
            &inputs[&start],
            false,
            cursor_link,
            &mut discarded_events,
        )?;
        for (successor, annulled_delay) in successors(block) {
            if !owned.contains(&successor) {
                continue;
            }
            let candidate = if annulled_delay {
                transfer_field_block(
                    block,
                    bytes,
                    va_start,
                    image_end,
                    &inputs[&start],
                    true,
                    cursor_link,
                    &mut discarded_events,
                )?
            } else {
                output.clone()
            };
            let joined = inputs
                .get(&successor)
                .map(|existing| existing.join(&candidate))
                .unwrap_or(candidate);
            if inputs.get(&successor) != Some(&joined) {
                inputs.insert(successor, joined);
                work.push_back(successor);
            }
        }
    }

    let mut events = BTreeSet::new();
    for (&start, input) in &inputs {
        let Some(block) = blocks.get(&start).copied() else {
            continue;
        };
        transfer_field_block(
            block,
            bytes,
            va_start,
            image_end,
            input,
            false,
            cursor_link,
            &mut events,
        )?;
    }
    Ok(events)
}

fn block_containing<'a>(
    blocks: &'a BTreeMap<u32, &BasicBlock>,
    owned: &BTreeSet<u32>,
    site: u32,
) -> Option<&'a BasicBlock> {
    blocks
        .range(..=site)
        .next_back()
        .map(|(_, block)| *block)
        .filter(|block| owned.contains(&block.start_va) && site < block.end_va)
}

fn site_reaches(
    blocks: &BTreeMap<u32, &BasicBlock>,
    owned: &BTreeSet<u32>,
    from: u32,
    to: u32,
) -> bool {
    let Some(from_block) = block_containing(blocks, owned, from) else {
        return false;
    };
    let Some(to_block) = block_containing(blocks, owned, to) else {
        return false;
    };
    if from_block.start_va == to_block.start_va && from <= to {
        return true;
    }
    let mut seen = BTreeSet::new();
    let mut work: VecDeque<u32> = successors(from_block)
        .into_iter()
        .map(|(successor, _)| successor)
        .filter(|successor| owned.contains(successor))
        .collect();
    while let Some(start) = work.pop_front() {
        if start == to_block.start_va {
            return true;
        }
        if !seen.insert(start) {
            continue;
        }
        let Some(block) = blocks.get(&start).copied() else {
            continue;
        };
        work.extend(
            successors(block)
                .into_iter()
                .map(|(successor, _)| successor)
                .filter(|successor| owned.contains(successor)),
        );
    }
    false
}

/// Discover callback registration/dispatch pairs without API names. A result
/// requires one path through the registrar that stores the callback argument,
/// links the object to the old head, and publishes it, plus one path through a
/// reachable dispatcher that loads the same head, invokes the same object
/// field, and loads the registrar's exact link field after the call.
pub fn discover_callback_registry_contracts(
    cfg: &Cfg,
    bytes: &[u8],
    va_start: u32,
) -> Result<Vec<CallbackRegistryContract>, CallbackFlowError> {
    if !va_start.is_multiple_of(4) {
        return Err(CallbackFlowError::UnalignedImageStart { va_start });
    }
    let image_end = va_start
        .checked_add(
            u32::try_from(bytes.len()).map_err(|_| CallbackFlowError::ImageAddressOverflow)?,
        )
        .ok_or(CallbackFlowError::ImageAddressOverflow)?;
    let blocks: BTreeMap<u32, &BasicBlock> = cfg
        .blocks
        .iter()
        .map(|block| (block.start_va, block))
        .collect();
    let analyses: Vec<_> = partition(cfg)
        .owners
        .into_iter()
        .filter_map(|owner| {
            let owned: BTreeSet<u32> = owner.block_starts.iter().copied().collect();
            owned
                .contains(&owner.root_va)
                .then_some((owner.root_va, owned))
        })
        .map(|(root, owned)| {
            analyze_field_owner(root, &owned, &blocks, bytes, va_start, image_end, None)
                .map(|events| (root, owned, events))
        })
        .collect::<Result<_, _>>()?;

    let mut contracts = BTreeSet::new();
    let mut dispatcher_cache = BTreeMap::new();
    for (registrar, registrar_blocks, registrar_events) in &analyses {
        for callback in registrar_events {
            let FieldEvent::CallbackStore {
                site: callback_store_site,
                object_arg,
                callback_arg,
                offset: callback_offset,
            } = *callback
            else {
                continue;
            };
            for link in registrar_events {
                let FieldEvent::LinkStore {
                    site: link_store_site,
                    object_arg: link_object_arg,
                    head,
                    offset: link_offset,
                } = *link
                else {
                    continue;
                };
                if link_object_arg != object_arg
                    || link_offset == callback_offset
                    || !site_reaches(
                        &blocks,
                        registrar_blocks,
                        callback_store_site,
                        link_store_site,
                    )
                {
                    continue;
                }
                for insert in registrar_events {
                    let FieldEvent::HeadStore {
                        site: list_insert_site,
                        head: insert_head,
                        object_arg: insert_object_arg,
                    } = *insert
                    else {
                        continue;
                    };
                    if insert_head != head
                        || insert_object_arg != object_arg
                        || !site_reaches(
                            &blocks,
                            registrar_blocks,
                            link_store_site,
                            list_insert_site,
                        )
                    {
                        continue;
                    }
                    for (dispatcher, dispatcher_blocks, _) in &analyses {
                        let cache_key = (*dispatcher, head, link_offset);
                        if !dispatcher_cache.contains_key(&cache_key) {
                            let events = analyze_field_owner(
                                *dispatcher,
                                dispatcher_blocks,
                                &blocks,
                                bytes,
                                va_start,
                                image_end,
                                Some((head, link_offset)),
                            )?;
                            dispatcher_cache.insert(cache_key, events);
                        }
                        let dispatcher_events = &dispatcher_cache[&cache_key];
                        for jalr in dispatcher_events {
                            let FieldEvent::JalrField {
                                site: jalr_site,
                                head: jalr_head,
                                offset: jalr_offset,
                            } = *jalr
                            else {
                                continue;
                            };
                            if jalr_head != head || jalr_offset != callback_offset {
                                continue;
                            }
                            let head_reaches_jalr = dispatcher_events.iter().any(|event| {
                                matches!(
                                    event,
                                    FieldEvent::HeadLoad { site, head: loaded_head }
                                        if *loaded_head == head
                                            && site_reaches(
                                                &blocks,
                                                dispatcher_blocks,
                                                *site,
                                                jalr_site,
                                            )
                                )
                            });
                            let jalr_reaches_link = dispatcher_events.iter().any(|event| {
                                matches!(
                                    event,
                                    FieldEvent::CursorFieldLoad {
                                        site,
                                        head: loaded_head,
                                        offset,
                                    } if *loaded_head == head
                                        && *offset == link_offset
                                        && site_reaches(
                                            &blocks,
                                            dispatcher_blocks,
                                            jalr_site,
                                            *site,
                                        )
                                )
                            });
                            if head_reaches_jalr && jalr_reaches_link {
                                contracts.insert(CallbackRegistryContract {
                                    registrar: *registrar,
                                    dispatcher: *dispatcher,
                                    object_arg_register: object_arg,
                                    pointer_arg_register: callback_arg,
                                    callback_offset,
                                    link_offset,
                                    list_head: head,
                                    callback_store_site,
                                    list_insert_site,
                                    jalr_site,
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(contracts.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::build_cfg_value_set_closed;

    const BASE: u32 = 0x8000_0000;
    const NOP: u32 = 0;
    const JR_RA: u32 = 0x03e0_0008;

    fn asm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|word| word.to_be_bytes()).collect()
    }

    fn contracts(words: &[u32]) -> Vec<CallbackArgumentContract> {
        let bytes = asm(words);
        let closure = build_cfg_value_set_closed("bank", &bytes, BASE, &[BASE]);
        discover_callback_argument_contracts(&closure.cfg, &bytes, BASE).unwrap()
    }

    fn jal(target: u32) -> u32 {
        0x0c00_0000 | ((target >> 2) & 0x03ff_ffff)
    }

    fn registry_fixture(dispatch_link_offset: i16, call_dispatcher: bool) -> Vec<u32> {
        const REGISTRAR_INDEX: usize = 32;
        const DISPATCHER_INDEX: usize = 64;
        const POINTER_WORD_OFFSET: i16 = 0x180;
        let registrar = BASE + REGISTRAR_INDEX as u32 * 4;
        let dispatcher = BASE + DISPATCHER_INDEX as u32 * 4;
        let mut words = vec![NOP; 112];
        words[0] = jal(registrar);
        words[1] = NOP;
        words[2] = if call_dispatcher {
            jal(dispatcher)
        } else {
            NOP
        };
        words[3] = NOP;
        words[4] = JR_RA;
        words[5] = NOP;

        words[REGISTRAR_INDEX..REGISTRAR_INDEX + 11].copy_from_slice(&[
            0x0080_8025, // move s0, a0
            0xae05_0004, // sw   a1, 4(s0)
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0180, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d2a_0020, // lw   t2, 0x20(t1): old head
            0xae0a_0000, // sw   t2, 0(s0): link
            0x8d0b_0000, // lw   t3, 0(t0)
            0xad70_0020, // sw   s0, 0x20(t3): publish
            JR_RA,
            NOP,
        ]);

        words[DISPATCHER_INDEX..DISPATCHER_INDEX + 10].copy_from_slice(&[
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0180, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d30_0020, // lw   s0, 0x20(t1): head
            0x8e19_0004, // lw   t9, 4(s0): callback
            0x0320_f809, // jalr t9
            NOP,
            0x8e10_0000 | u32::from(dispatch_link_offset as u16), // lw s0, link(s0)
            JR_RA,
            NOP,
        ]);
        assert_eq!(POINTER_WORD_OFFSET, 0x180);
        words
    }

    fn registry_contracts(words: &[u32]) -> Vec<CallbackRegistryContract> {
        let bytes = asm(words);
        let closure = build_cfg_value_set_closed("bank", &bytes, BASE, &[BASE]);
        discover_callback_registry_contracts(&closure.cfg, &bytes, BASE).unwrap()
    }

    #[test]
    fn saved_argument_reaching_jalr_is_a_contract() {
        let move_s4_a0 = (4 << 21) | (20 << 11) | 0x21;
        let jalr_s4 = (20 << 21) | (31 << 11) | 0x09;
        let words = [move_s4_a0, jalr_s4, NOP, JR_RA, NOP];
        assert_eq!(
            contracts(&words),
            vec![CallbackArgumentContract {
                callee: BASE,
                pointer_arg_register: 4,
                jalr_sites: vec![BASE + 4],
            }]
        );
    }

    #[test]
    fn stack_spill_and_reload_preserve_argument_identity() {
        let addiu_sp = 0x27bd_ffe8;
        let sw_a1 = 0xafa5_0010;
        let lw_t9 = 0x8fb9_0010;
        let jalr_t9 = 0x0320_f809;
        assert_eq!(
            contracts(&[addiu_sp, sw_a1, lw_t9, jalr_t9, NOP, JR_RA, NOP]),
            vec![CallbackArgumentContract {
                callee: BASE,
                pointer_arg_register: 5,
                jalr_sites: vec![BASE + 12],
            }]
        );
    }

    #[test]
    fn clobber_and_direct_call_do_not_retain_stale_arguments() {
        let move_s4_a0 = (4 << 21) | (20 << 11) | 0x21;
        let addiu_s4 = 0x2694_0004;
        let jalr_s4 = (20 << 21) | (31 << 11) | 0x09;
        assert!(contracts(&[move_s4_a0, addiu_s4, jalr_s4, NOP, JR_RA, NOP]).is_empty());

        let jal = 0x0c00_0004;
        let jalr_a0 = (4 << 21) | (31 << 11) | 0x09;
        assert!(contracts(&[jal, NOP, jalr_a0, NOP, JR_RA, NOP]).is_empty());
    }

    #[test]
    fn printf_style_loop_preserves_saved_callback_argument() {
        let words = [
            0x27bd_ff28,
            0xafb7_0034,
            0xafb6_0030,
            0xafb5_002c,
            0xafbe_0038,
            0xafb4_0028,
            0xafb3_0024,
            0xafa7_00e4,
            0x3c15_8009,
            0x3c16_800a,
            0x3c17_8009,
            0x00c0_3825,
            0x00a0_9825,
            0x0080_a025,
            0xafbf_003c,
            0xafb2_0020,
            0xafb1_001c,
            0xafb0_0018,
            0xafa6_00e0,
            0xafa0_00cc,
            0x26f7_7f20,
            0x26d6_9304,
            0x26b5_7f44,
            0x241e_000a,
            0x90e2_0000,
            0x00e0_9025,
            0x2403_0025,
            0x1040_0009,
            0x0040_8025,
            0x5062_0008,
            0x0247_1023,
            0x9242_0001,
            0x2652_0001,
            0x1040_0003,
            0x0040_8025,
            0x5462_fffc,
            0x9242_0001,
            0x0247_1023,
            0x1840_000d,
            0x0260_2025,
            0x00e0_2825,
            0x0040_8825,
            0x0280_f809,
            0x0040_3025,
            JR_RA,
            NOP,
        ];
        let mut state = FlowState::at_entry();
        for word in &words[..42] {
            execute_word(&mut state, *word);
        }
        assert_eq!(state.registers[20], FlowValue::Argument(4));
        let bytes = asm(&words);
        let closure = build_cfg_value_set_closed("bank", &bytes, BASE, &[BASE]);
        let part = partition(&closure.cfg);
        let derived = discover_callback_argument_contracts(&closure.cfg, &bytes, BASE).unwrap();
        assert!(
            derived.iter().any(|contract| {
                contract.pointer_arg_register == 4 && contract.jalr_sites == vec![BASE + 0xa8]
            }),
            "{derived:#x?}; blocks={:#x?}; partition={part:#x?}",
            closure.cfg.blocks
        );
    }

    #[test]
    fn registry_contract_recognizes_intrusive_list_registration_and_dispatch() {
        assert_eq!(
            registry_contracts(&registry_fixture(0, true)),
            vec![CallbackRegistryContract {
                registrar: BASE + 0x80,
                dispatcher: BASE + 0x100,
                object_arg_register: 4,
                pointer_arg_register: 5,
                callback_offset: 4,
                link_offset: 0,
                list_head: IndirectListHead {
                    pointer_word_address: BASE + 0x180,
                    field_offset: 0x20,
                },
                callback_store_site: BASE + 0x84,
                list_insert_site: BASE + 0xa0,
                jalr_site: BASE + 0x114,
            }]
        );
    }

    #[test]
    fn registry_contract_requires_reachable_dispatch_and_matching_link_field() {
        assert!(registry_contracts(&registry_fixture(8, true)).is_empty());
        assert!(registry_contracts(&registry_fixture(0, false)).is_empty());
    }

    #[test]
    fn registry_contract_rejects_branch_dependent_callback_value_at_join() {
        const DISPATCHER_INDEX: usize = 64;
        let mut words = registry_fixture(0, true);
        words[DISPATCHER_INDEX..DISPATCHER_INDEX + 16].copy_from_slice(&[
            0x3c08_8000, // lui  t0, 0x8000
            0x2508_0180, // addiu t0, t0, pointer word
            0x8d09_0000, // lw   t1, 0(t0)
            0x8d30_0020, // lw   s0, 0x20(t1): head
            0x1080_0004, // beq  a0, zero, alternate
            NOP,
            0x8e19_0004, // path A: lw t9, 4(s0)
            0x1000_0003, // b merge
            NOP,
            0x0000_c825, // alternate: move t9, zero
            NOP,
            0x0320_f809, // merge: jalr t9 is not uniformly a callback
            NOP,
            0x8e10_0000, // lw s0, 0(s0): link
            JR_RA,
            NOP,
        ]);
        assert!(registry_contracts(&words).is_empty());
    }
}
