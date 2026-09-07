//! `osCreateThread`, `osStartThread`, and thread-priority accessor
//! recognizers.

use super::core::{
    imm, is_addiu, is_bne, is_jr_ra, is_lui, is_lw, is_lw_at, is_move_addu, is_sh, is_store_opcode,
    jal_field, op, rd, rs, rt, HostBindingDiscoveryError, HostBindingSymbol,
};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreateThreadValue {
    Unknown,
    Zero,
    NonZeroConstant,
    Thread,
    Id,
    Entry,
    Argument,
    StackArgument,
    Priority,
}

pub(super) fn is_create_thread(words: &[u32]) -> bool {
    const MIN_WORDS: usize = 42;
    if words.len() < MIN_WORDS || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let frame_size = i32::from(imm(words[0])).unsigned_abs();
    let (Ok(stack_arg_offset), Ok(priority_offset)) = (
        i16::try_from(frame_size + 16),
        i16::try_from(frame_size + 20),
    ) else {
        return false;
    };
    let mut registers = [CreateThreadValue::Unknown; 32];
    registers[0] = CreateThreadValue::Zero;
    registers[4] = CreateThreadValue::Thread;
    registers[5] = CreateThreadValue::Id;
    registers[6] = CreateThreadValue::Entry;
    registers[7] = CreateThreadValue::Argument;
    let mut stack = BTreeMap::new();
    let mut fields = BTreeMap::new();
    let mut saw_late_call = false;
    let mut saw_return = false;
    let mut saw_stack_argument_load = false;
    let mut clobber_after_instruction = false;

    for (index, &word) in words.iter().enumerate() {
        if is_jr_ra(word) {
            saw_return = true;
            break;
        }
        if jal_field(word).is_some() {
            if clobber_after_instruction {
                return false;
            }
            saw_late_call |= index >= 24;
            clobber_after_instruction = true;
            continue;
        }
        match op(word) {
            0x2b | 0x29 => {
                let source = registers[rt(word) as usize];
                if rs(word) == 29 {
                    if op(word) == 0x2b {
                        stack.insert(imm(word), source);
                    }
                } else if registers[rs(word) as usize] == CreateThreadValue::Thread {
                    fields.insert((op(word), imm(word)), source);
                }
            }
            0x23 => {
                let value = if rs(word) == 29 {
                    match imm(word) {
                        offset if offset == stack_arg_offset => {
                            saw_stack_argument_load = true;
                            CreateThreadValue::StackArgument
                        }
                        offset if offset == priority_offset => CreateThreadValue::Priority,
                        offset => stack
                            .get(&offset)
                            .copied()
                            .unwrap_or(CreateThreadValue::Unknown),
                    }
                } else {
                    CreateThreadValue::Unknown
                };
                if rt(word) != 0 {
                    registers[rt(word) as usize] = value;
                }
            }
            0 => {
                let destination = rd(word) as usize;
                if destination != 0 {
                    registers[destination] =
                        if matches!(word & 0x3f, 0x21 | 0x25) && (rs(word) == 0 || rt(word) == 0) {
                            let source = if rs(word) == 0 { rt(word) } else { rs(word) };
                            registers[source as usize]
                        } else {
                            CreateThreadValue::Unknown
                        };
                }
            }
            0x09 | 0x0d => {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = if rs(word) == 0 && imm(word) != 0 {
                        CreateThreadValue::NonZeroConstant
                    } else {
                        CreateThreadValue::Unknown
                    };
                }
            }
            0x0f => {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = CreateThreadValue::Unknown;
                }
            }
            opcode
                if matches!(
                    opcode,
                    0x08 | 0x0a..=0x0c | 0x0e | 0x20..=0x27 | 0x30..=0x37
                ) =>
            {
                if rt(word) != 0 {
                    registers[rt(word) as usize] = CreateThreadValue::Unknown;
                }
            }
            0x1c => {
                if rd(word) != 0 {
                    registers[rd(word) as usize] = CreateThreadValue::Unknown;
                }
            }
            _ => {}
        }
        if clobber_after_instruction {
            for register in 2..=15 {
                registers[register] = CreateThreadValue::Unknown;
            }
            registers[24] = CreateThreadValue::Unknown;
            registers[25] = CreateThreadValue::Unknown;
            registers[31] = CreateThreadValue::Unknown;
            clobber_after_instruction = false;
        }
    }

    let sw = |offset| fields.get(&(0x2b, offset)).copied();
    let sh = |offset| fields.get(&(0x29, offset)).copied();
    saw_return
        && saw_late_call
        && saw_stack_argument_load
        && sw(0) == Some(CreateThreadValue::Zero)
        && sw(4) == Some(CreateThreadValue::Priority)
        && sw(8) == Some(CreateThreadValue::Zero)
        && sh(0x10) == Some(CreateThreadValue::NonZeroConstant)
        && sh(0x12) == Some(CreateThreadValue::Zero)
        && sw(0x14) == Some(CreateThreadValue::Id)
        && sw(0x18) == Some(CreateThreadValue::Zero)
        && sw(0x38).is_some()
        && sw(0x3c) == Some(CreateThreadValue::Argument)
        && sw(0xf0).is_some()
        // The saved SP is the fifth argument minus the ABI call frame. The
        // exact subtraction schedule varies; requiring both context halves
        // plus the independently tagged fifth-argument load above avoids
        // baking one arithmetic sequence into this recognizer.
        && sw(0xf4).is_some()
        && sw(0x100).is_some()
        && sw(0x104).is_some()
        && sw(0x118).is_some()
        && sw(0x11c) == Some(CreateThreadValue::Entry)
        && sw(0x128).is_some()
        && sw(0x12c).is_some()
}

pub(super) fn is_start_thread(words: &[u32]) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V {
        Unknown,
        Thread,
        State,
        One,
        Two,
        Eight,
        Priority,
        SavedMask,
        Static,
    }

    if words.len() < 15 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let mut regs = [V::Unknown; 32];
    regs[4] = V::Thread;
    let mut spill = BTreeMap::new();
    let mut calls = 0usize;
    let mut saw_state_read = false;
    let mut compared_one = false;
    let mut compared_eight = false;
    let mut wrote_two = false;
    let mut inserted_thread = false;
    let mut priority_reads = 0usize;
    let mut priority_compare = false;
    let mut restored_interrupts = false;
    let mut returned = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            returned = true;
            break;
        }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                match op(slot) {
                    0 => {
                        if matches!(slot & 0x3f, 0x21 | 0x25) {
                            let source = if rt(slot) == 0 {
                                rs(slot)
                            } else if rs(slot) == 0 {
                                rt(slot)
                            } else {
                                0
                            };
                            if rd(slot) != 0 {
                                regs[rd(slot) as usize] = regs[source as usize];
                            }
                        }
                    }
                    0x23 => {
                        if rt(slot) != 0 {
                            regs[rt(slot) as usize] = if rs(slot) == 29 {
                                spill.get(&imm(slot)).copied().unwrap_or(V::Unknown)
                            } else if regs[rs(slot) as usize] == V::Thread && imm(slot) == 8 {
                                V::Static
                            } else {
                                V::Unknown
                            };
                        }
                    }
                    0x09 => {
                        if rt(slot) != 0 && regs[rs(slot) as usize] == V::Static {
                            regs[rt(slot) as usize] = V::Static;
                        }
                    }
                    _ => {}
                }
            }
            inserted_thread |= regs[5] == V::Thread;
            restored_interrupts |= calls > 0 && regs[4] == V::SavedMask;
            calls += 1;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                regs[caller_saved] = V::Unknown;
            }
            regs[2] = if calls == 1 { V::SavedMask } else { V::Unknown };
            index += 2;
            continue;
        }
        match op(word) {
            0x2b => {
                if rs(word) == 29 {
                    spill.insert(imm(word), regs[rt(word) as usize]);
                }
            }
            0x29 => {
                if regs[rs(word) as usize] == V::Thread
                    && imm(word) == 0x10
                    && regs[rt(word) as usize] == V::Two
                {
                    wrote_two = true;
                }
            }
            0x23 => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = if rs(word) == 29 {
                        spill.get(&imm(word)).copied().unwrap_or(V::Unknown)
                    } else if imm(word) == 4 {
                        priority_reads += 1;
                        V::Priority
                    } else {
                        V::Unknown
                    };
                }
            }
            0x25 => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] =
                        if regs[rs(word) as usize] == V::Thread && imm(word) == 0x10 {
                            saw_state_read = true;
                            V::State
                        } else {
                            V::Unknown
                        };
                }
            }
            0x09 | 0x0d => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = if rs(word) == 0 {
                        match imm(word) {
                            1 => V::One,
                            2 => V::Two,
                            8 => V::Eight,
                            _ => V::Unknown,
                        }
                    } else if regs[rs(word) as usize] == V::Static {
                        V::Static
                    } else {
                        V::Unknown
                    };
                }
            }
            0x0f => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = V::Static;
                }
            }
            0x04 | 0x05 => {
                let pair = (regs[rs(word) as usize], regs[rt(word) as usize]);
                compared_one |= matches!(pair, (V::State, V::One) | (V::One, V::State));
                compared_eight |= matches!(pair, (V::State, V::Eight) | (V::Eight, V::State));
            }
            0 => match word & 0x3f {
                0x20 | 0x21 | 0x25 => {
                    let value = if rt(word) == 0 {
                        regs[rs(word) as usize]
                    } else if rs(word) == 0 {
                        regs[rt(word) as usize]
                    } else {
                        V::Unknown
                    };
                    if rd(word) != 0 {
                        regs[rd(word) as usize] = value;
                    }
                }
                0x2a | 0x2b => {
                    priority_compare |= regs[rs(word) as usize] == V::Priority
                        && regs[rt(word) as usize] == V::Priority;
                    if rd(word) != 0 {
                        regs[rd(word) as usize] = V::Unknown;
                    }
                }
                0x08 | 0x09 => {}
                _ => {
                    if rd(word) != 0 {
                        regs[rd(word) as usize] = V::Unknown;
                    }
                }
            },
            0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
            _ => {
                if rt(word) != 0 && !is_store_opcode(op(word)) {
                    regs[rt(word) as usize] = V::Unknown;
                }
            }
        }
        index += 1;
    }
    (returned
        && calls >= 4
        && saw_state_read
        && compared_one
        && compared_eight
        && wrote_two
        && inserted_thread
        && priority_reads >= 2
        && priority_compare
        && restored_interrupts)
        || is_start_thread_register_resident(words)
}

fn is_start_thread_register_resident(words: &[u32]) -> bool {
    words.len() >= 15
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && jal_field(words[5]).is_some()
        && op(words[7]) == 0x25
        && rt(words[7]) == 3
        && rs(words[7]) == 16
        && imm(words[7]) == 0x10
        && is_addiu(words[9], 2, 0, 1)
        && op(words[10]) == 4
        && rs(words[10]) == 3
        && rt(words[10]) == 2
        && is_addiu(words[11], 2, 0, 8)
        && is_bne(words[12], 3, 2)
        && is_addiu(words[13], 2, 0, 2)
        && is_sh(words[14], 2, 16, 0x10)
}

pub(super) fn is_get_thread_pri(words: &[u32]) -> bool {
    words.len() >= 6
        && op(words[0]) == 5
        && rs(words[0]) == 4
        && rt(words[0]) == 0
        && words[1] == 0
        && is_lui(words[2], 4)
        && is_lw(words[3], 4, 4)
        && is_jr_ra(words[4])
        && is_lw_at(words[5], 2, 4, 4)
}

pub(super) fn is_set_thread_pri(words: &[u32]) -> bool {
    // `osSetThreadPri(OSThread* t = a0, OSPri pri = a1)` writes `pri` into a
    // thread's priority field (`+4`), short-circuits when it is already at that
    // priority, and reschedules by walking `__osRunQueue`. Two
    // register-allocation layouts occur: the 1998 register-resident build keeps
    // `pri` in callee-saved registers (the old positional match pinned
    // words[0..19] and regs 16/17/18/2); World Tour's 1997 build spills every
    // argument to the stack (`sw a1,44(sp)`) and reloads per use, so the
    // priority write lands outside that fixed window. Match the order-free ABI
    // facts, taint-tracking `a1` through moves and stack spill/reload.
    if words.len() < 20 {
        return false;
    }
    // Entry anchor: the routine establishes its own frame at word[0]; requiring
    // the stack adjust at the head rejects windows that begin mid-body.
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return false;
    }

    // `true` in slot `r` marks that register currently holds `pri` (a1), traced
    // through `move`/`addu $x,$zero,a1` and stack spill/reload.
    let mut pri_tag = [false; 32];
    pri_tag[5] = true; // a1 = pri on entry
    let mut pri_spill: BTreeMap<i16, bool> = BTreeMap::new();

    let mut wrote_priority_field = false; // `sw <pri>, +4(base)`
    let mut read_priority_field = false; // `lw <x>, +4(base)`
    let mut priority_field_reg = [false; 32]; // regs holding a `+4` field load
    let mut compared_priority = false; // `beq/bne` of pri-tagged vs a +4 read
    let mut loaded_run_queue = false; // `lui hi; lw x, lo(x)` global (__osRunQueue)
    let mut prev_lui_reg: Option<u32> = None;

    for &word in words {
        let opcode = op(word);
        match opcode {
            0x00 => {
                let funct = word & 0x3f;
                if funct == 0x21 || funct == 0x20 {
                    let (d, s, t) = (rd(word), rs(word), rt(word));
                    let src = if t == 0 {
                        Some(s)
                    } else if s == 0 {
                        Some(t)
                    } else {
                        None
                    };
                    match src {
                        Some(src) => {
                            pri_tag[d as usize] = pri_tag[src as usize];
                            priority_field_reg[d as usize] = false;
                        }
                        None => {
                            pri_tag[d as usize] = false;
                            priority_field_reg[d as usize] = false;
                        }
                    }
                }
                prev_lui_reg = None;
            }
            0x0f => {
                prev_lui_reg = Some(rt(word));
            }
            0x23 => {
                let (base, dst, off) = (rs(word), rt(word), imm(word));
                if base == 29 {
                    pri_tag[dst as usize] = *pri_spill.get(&off).unwrap_or(&false);
                    priority_field_reg[dst as usize] = false;
                } else if off == 4 {
                    read_priority_field = true;
                    priority_field_reg[dst as usize] = true;
                    pri_tag[dst as usize] = false;
                } else {
                    if prev_lui_reg == Some(base) {
                        loaded_run_queue = true;
                    }
                    priority_field_reg[dst as usize] = false;
                    pri_tag[dst as usize] = false;
                }
                if prev_lui_reg != Some(base) {
                    prev_lui_reg = None;
                }
            }
            0x2b => {
                let (base, src, off) = (rs(word), rt(word), imm(word));
                if base == 29 {
                    pri_spill.insert(off, pri_tag[src as usize]);
                } else if off == 4 && pri_tag[src as usize] {
                    wrote_priority_field = true;
                }
                prev_lui_reg = None;
            }
            0x04 | 0x05 => {
                let (a, b) = (rs(word), rt(word));
                let pri_vs_field = (pri_tag[a as usize] && priority_field_reg[b as usize])
                    || (pri_tag[b as usize] && priority_field_reg[a as usize]);
                if pri_vs_field {
                    compared_priority = true;
                }
                prev_lui_reg = None;
            }
            _ => {
                prev_lui_reg = None;
            }
        }
    }

    wrote_priority_field && read_priority_field && compared_priority && loaded_run_queue
}

pub(super) fn unique_create_thread_match(
    words: &[u32],
    va_start: u32,
) -> Result<u32, HostBindingDiscoveryError> {
    const MIN_WORDS: usize = 42;
    const MAX_WORDS: usize = 96;
    let mut candidates = (0..=words.len().saturating_sub(MIN_WORDS))
        .filter_map(|index| {
            let end = words.len().min(index + MAX_WORDS);
            is_create_thread(&words[index..end])
                .then(|| va_start.checked_add(u32::try_from(index).ok()?.checked_mul(4)?))?
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.as_slice() {
        [address] => Ok(*address),
        _ => Err(HostBindingDiscoveryError::NonUniqueSemanticMatch {
            symbol: HostBindingSymbol::OsCreateThread,
            candidates,
        }),
    }
}
