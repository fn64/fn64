//! RSP task-load, start-go, yield, and yielded-query recognizers.

use super::core::{
    imm, is_addiu, is_andi, is_beq, is_jr_ra, is_lui, is_lw_at, is_move_addu, is_store_opcode,
    is_sw, jal_field, op, rd, rs, rt,
};
use std::collections::BTreeMap;

pub(super) fn is_sp_task_load(words: &[u32]) -> bool {
    sp_task_load_helpers(words).is_some() || is_sp_task_load_register_resident(words)
}

fn is_sp_task_load_register_resident(words: &[u32]) -> bool {
    words.len() >= 131
        && is_addiu(words[0], 29, 29, imm(words[0]))
        && imm(words[0]) < 0
        && is_move_addu(words[2], 16, 4)
        && is_move_addu(words[6], 5, 17)
        && jal_field(words[8]).is_some()
        && is_addiu(words[9], 6, 0, 0x40)
        && is_andi(words[68], 2, 2, 1)
        && is_beq(words[69], 2, 0)
        && is_lw_at(words[79], 2, 16, 4)
        && is_addiu(words[80], 3, 0, -2)
        && op(words[81]) == 0
        && words[81] & 0x3f == 0x24
        && rd(words[81]) == 2
        && rs(words[81]) == 2
        && rt(words[81]) == 3
        && is_sw(words[82], 2, 16, 4)
        && is_andi(words[85], 2, 2, 4)
        && is_beq(words[86], 2, 0)
        && is_lw_at(words[88], 2, 16, 0x38)
        && is_move_addu(words[94], 4, 17)
        && jal_field(words[95]).is_some()
        && is_addiu(words[96], 5, 0, 0x40)
        && jal_field(words[97]).is_some()
        && is_addiu(words[98], 4, 0, 0x2b00)
        && is_lui(words[100], 4)
        && words[100] as u16 == 0x0400
        && jal_field(words[101]).is_some()
        && op(words[102]) == 0x0d
        && rt(words[102]) == 4
        && rs(words[102]) == 4
        && words[102] as u16 == 0x1000
        && is_addiu(words[106], 4, 0, 1)
        && is_lui(words[107], 5)
        && words[107] as u16 == 0x0400
        && op(words[108]) == 0x0d
        && rt(words[108]) == 5
        && rs(words[108]) == 5
        && words[108] as u16 == 0x0fc0
        && jal_field(words[110]).is_some()
        && is_addiu(words[111], 7, 0, 0x40)
        && jal_field(words[114]).is_some()
        && is_lw_at(words[119], 6, 17, 8)
        && is_lw_at(words[120], 7, 17, 12)
        && is_lui(words[121], 5)
        && words[121] as u16 == 0x0400
        && jal_field(words[122]).is_some()
        && jal_field(words[110]) == jal_field(words[122])
        && op(words[123]) == 0x0d
        && rt(words[123]) == 5
        && rs(words[123]) == 5
        && words[123] as u16 == 0x1000
        && is_jr_ra(words[129])
        && is_addiu(words[130], 29, 29, -imm(words[0]))
}

/// Return `(busy, set_status)` only after proving the complete `osSpTaskLoad`
/// dataflow.  The 1998 build keeps the prepared task pointer in saved
/// registers; the 1997 build spills both the input and helper result and
/// reloads them at every use.
fn sp_task_load_helpers(words: &[u32]) -> Option<(u32, u32)> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V {
        Unknown,
        Input,
        Task,
        Flags,
        Cleared,
        Field8,
        Field12,
        One,
        MinusTwo,
        SixtyFour,
        Status,
        Addr1000,
        AddrFc0,
    }

    if words.len() < 96 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return None;
    }
    let mut regs = [V::Unknown; 32];
    regs[4] = V::Input;
    let mut spill = BTreeMap::new();
    let mut first_call = true;
    let mut set_status = None;
    let mut busy = None;
    let mut last_call = None;
    let mut saw_bit_one = false;
    let mut saw_bit_four = false;
    let mut cleared_bit_zero = false;
    let mut task_dma = false;
    let mut boot_dma = false;
    let mut returned = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            returned = true;
            break;
        }
        if let Some(target) = jal_field(word) {
            if let Some(&slot) = words.get(index + 1) {
                match op(slot) {
                    0x09 | 0x0d => {
                        if rt(slot) != 0 {
                            regs[rt(slot) as usize] = match (rs(slot), imm(slot)) {
                                (0, 1) => V::One,
                                (0, 0x40) => V::SixtyFour,
                                (0, 0x2b00) => V::Status,
                                (r, 0x1000) if regs[r as usize] == V::Addr1000 => V::Addr1000,
                                (r, 0x0fc0) if regs[r as usize] == V::Addr1000 => V::AddrFc0,
                                _ => V::Unknown,
                            };
                        }
                    }
                    0x23 => {
                        if rt(slot) != 0 {
                            regs[rt(slot) as usize] = if rs(slot) == 29 {
                                spill.get(&imm(slot)).copied().unwrap_or(V::Unknown)
                            } else if regs[rs(slot) as usize] == V::Task {
                                match imm(slot) {
                                    8 => V::Field8,
                                    12 => V::Field12,
                                    _ => V::Unknown,
                                }
                            } else {
                                V::Unknown
                            };
                        }
                    }
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
                    _ => {}
                }
            }
            if regs[4] == V::Status {
                set_status = Some(target);
            }
            task_dma |= regs[4] == V::One
                && regs[5] == V::AddrFc0
                && regs[6] == V::Task
                && regs[7] == V::SixtyFour;
            boot_dma |= regs[4] == V::One
                && regs[5] == V::Addr1000
                && regs[6] == V::Field8
                && regs[7] == V::Field12;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                regs[caller_saved] = V::Unknown;
            }
            regs[2] = if first_call {
                first_call = false;
                V::Task
            } else {
                V::Unknown
            };
            last_call = Some(target);
            index += 2;
            continue;
        }
        match op(word) {
            0x2b => {
                let value = regs[rt(word) as usize];
                if rs(word) == 29 {
                    spill.insert(imm(word), value);
                } else if matches!(regs[rs(word) as usize], V::Input | V::Task)
                    && imm(word) == 4
                    && value == V::Cleared
                {
                    cleared_bit_zero = true;
                }
            }
            0x23 => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = if rs(word) == 29 {
                        spill.get(&imm(word)).copied().unwrap_or(V::Unknown)
                    } else if matches!(regs[rs(word) as usize], V::Input | V::Task) {
                        match imm(word) {
                            4 => V::Flags,
                            8 => V::Field8,
                            12 => V::Field12,
                            _ => V::Unknown,
                        }
                    } else {
                        V::Unknown
                    };
                }
            }
            0x0c => {
                if rt(word) != 0 {
                    if regs[rs(word) as usize] == V::Flags && imm(word) == 1 {
                        saw_bit_one = true;
                    }
                    if regs[rs(word) as usize] == V::Flags && imm(word) == 4 {
                        saw_bit_four = true;
                    }
                    regs[rt(word) as usize] = V::Unknown;
                }
            }
            0x0f => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = if words[index] as u16 == 0x0400 {
                        V::Addr1000
                    } else {
                        V::Unknown
                    };
                }
            }
            0x09 | 0x0d => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = match (rs(word), imm(word)) {
                        (0, 1) => V::One,
                        (0, -2) => V::MinusTwo,
                        (0, 0x40) => V::SixtyFour,
                        (0, 0x2b00) => V::Status,
                        (r, 0x1000) if regs[r as usize] == V::Addr1000 => V::Addr1000,
                        (r, 0x0fc0) if regs[r as usize] == V::Addr1000 => V::AddrFc0,
                        _ => V::Unknown,
                    };
                }
            }
            0x04 | 0x05 => {
                if (rs(word) == 2 && rt(word) == 0) || (rs(word) == 0 && rt(word) == 2) {
                    if let Some(target) = last_call {
                        if Some(target) != set_status {
                            busy = Some(target);
                        }
                    }
                }
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
                0x24 => {
                    let left = regs[rs(word) as usize];
                    let right = regs[rt(word) as usize];
                    if rd(word) != 0 {
                        regs[rd(word) as usize] = if matches!(
                            (left, right),
                            (V::Flags, V::MinusTwo) | (V::MinusTwo, V::Flags)
                        ) {
                            V::Cleared
                        } else {
                            V::Unknown
                        };
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
    (returned && saw_bit_one && saw_bit_four && cleared_bit_zero && task_dma && boot_dma)
        .then_some((busy?, set_status?))
}

pub(super) fn extract_sp_task_load_helpers(words: &[u32]) -> Option<(u32, u32)> {
    if let Some(helpers) = sp_task_load_helpers(words) {
        return Some(helpers);
    }
    let set_status = words.iter().enumerate().find_map(|(index, &word)| {
        let target = jal_field(word)?;
        (words
            .get(index + 1)
            .is_some_and(|&slot| is_addiu(slot, 4, 0, 0x2b00))
            || index > 0 && is_addiu(words[index - 1], 4, 0, 0x2b00))
        .then_some(target)
    })?;
    let busy = words.iter().enumerate().find_map(|(index, &word)| {
        let target = jal_field(word)?;
        let tail = &words[index + 1..words.len().min(index + 6)];
        let until_next_call = &tail[..tail
            .iter()
            .position(|&later| jal_field(later).is_some())
            .unwrap_or(tail.len())];
        (target != set_status
            && until_next_call.iter().any(|&later| {
                matches!(op(later), 0x04 | 0x05)
                    && ((rs(later) == 2 && rt(later) == 0) || (rs(later) == 0 && rt(later) == 2))
            }))
        .then_some(target)
    })?;
    Some((busy, set_status))
}

pub(super) fn is_sp_task_start_go(words: &[u32], busy: u32, set_status: u32) -> bool {
    if words.len() < 11 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let mut busy_call = false;
    let mut busy_poll = false;
    let mut status_call = false;
    let mut pending_busy = false;
    let mut returned = false;
    for (index, &word) in words.iter().enumerate().skip(1) {
        if jal_field(word) == Some(busy) {
            busy_call = true;
            pending_busy = true;
        }
        if jal_field(word) == Some(set_status) {
            status_call |= words
                .get(index + 1)
                .is_some_and(|&slot| is_addiu(slot, 4, 0, 0x125))
                || index > 0 && is_addiu(words[index - 1], 4, 0, 0x125);
        }
        if pending_busy
            && matches!(op(word), 0x04 | 0x05)
            && (rs(word) == 2 || rt(word) == 2)
            && (rs(word) == 0 || rt(word) == 0)
        {
            busy_poll = true;
        }
        if is_jr_ra(word) {
            returned = true;
            break;
        }
    }
    busy_call && busy_poll && status_call && returned
}

pub(super) fn is_sp_task_yield(words: &[u32], set_status: u32) -> bool {
    if words.len() < 7 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let calls = words
        .iter()
        .filter(|&&word| jal_field(word).is_some())
        .count();
    let status = words.iter().enumerate().any(|(index, &word)| {
        jal_field(word) == Some(set_status)
            && (words
                .get(index + 1)
                .is_some_and(|&slot| is_addiu(slot, 4, 0, 0x400))
                || index > 0 && is_addiu(words[index - 1], 4, 0, 0x400))
    });
    calls == 1 && status && words.iter().any(|&word| is_jr_ra(word))
}

pub(super) fn is_sp_task_yielded(words: &[u32]) -> bool {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum V {
        Unknown,
        Task,
        Status,
        Flags,
        Bool,
        Updated,
        Cleared,
        MinusThree,
    }
    if words.len() < 19 || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let mut regs = [V::Unknown; 32];
    regs[4] = V::Task;
    let mut spill = BTreeMap::new();
    let mut call_seen = false;
    let mut test_100 = false;
    let mut test_80 = false;
    let mut updated = false;
    let mut cleared = false;
    let mut returned_bool = false;
    let mut returned = false;
    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            returned = true;
            returned_bool |= regs[2] == V::Bool;
            break;
        }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                if op(slot) == 0x2b && rs(slot) == 29 {
                    spill.insert(imm(slot), regs[rt(slot) as usize]);
                } else if op(slot) == 0 && matches!(slot & 0x3f, 0x21 | 0x25) {
                    let src = if rt(slot) == 0 { rs(slot) } else { rt(slot) };
                    if rd(slot) != 0 {
                        regs[rd(slot) as usize] = regs[src as usize];
                    }
                }
            }
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                regs[caller_saved] = V::Unknown;
            }
            regs[2] = V::Status;
            call_seen = true;
            index += 2;
            continue;
        }
        match op(word) {
            0x2b => {
                let value = if rt(word) == 0 && test_100 {
                    V::Bool
                } else {
                    regs[rt(word) as usize]
                };
                if rs(word) == 29 {
                    spill.insert(imm(word), value);
                } else if regs[rs(word) as usize] == V::Task && imm(word) == 4 {
                    updated |= value == V::Updated;
                    cleared |= value == V::Cleared;
                }
            }
            0x23 => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = if rs(word) == 29 {
                        spill.get(&imm(word)).copied().unwrap_or(V::Unknown)
                    } else if regs[rs(word) as usize] == V::Task && imm(word) == 4 {
                        V::Flags
                    } else {
                        V::Unknown
                    };
                }
            }
            0x0c => {
                if rt(word) != 0 {
                    let source = regs[rs(word) as usize];
                    if source == V::Status && imm(word) == 0x100 {
                        test_100 = true;
                        regs[rt(word) as usize] = V::Bool;
                    } else if source == V::Status && imm(word) == 0x80 {
                        test_80 = true;
                        regs[rt(word) as usize] = V::Unknown;
                    } else {
                        regs[rt(word) as usize] = V::Unknown;
                    }
                }
            }
            0x09 => {
                if rt(word) != 0 {
                    regs[rt(word) as usize] = if rs(word) == 0 && imm(word) == -3 {
                        V::MinusThree
                    } else if rs(word) == 0 && matches!(imm(word), 0 | 1) && test_100 {
                        V::Bool
                    } else {
                        V::Unknown
                    };
                }
            }
            0 => match word & 0x3f {
                0x02 => {
                    if rd(word) != 0 {
                        regs[rd(word) as usize] =
                            if regs[rt(word) as usize] == V::Status && (word >> 6 & 31) == 8 {
                                V::Bool
                            } else {
                                V::Unknown
                            };
                    }
                }
                0x20 | 0x21 | 0x25 => {
                    let left = regs[rs(word) as usize];
                    let right = regs[rt(word) as usize];
                    if rd(word) != 0 {
                        regs[rd(word) as usize] = if rt(word) == 0 {
                            left
                        } else if rs(word) == 0 {
                            right
                        } else if matches!((left, right), (V::Flags, V::Bool) | (V::Bool, V::Flags))
                        {
                            V::Updated
                        } else {
                            V::Unknown
                        };
                    }
                }
                0x24 => {
                    let left = regs[rs(word) as usize];
                    let right = regs[rt(word) as usize];
                    if rd(word) != 0 {
                        regs[rd(word) as usize] = if matches!(
                            (left, right),
                            (V::Updated, V::MinusThree)
                                | (V::MinusThree, V::Updated)
                                | (V::Flags, V::MinusThree)
                                | (V::MinusThree, V::Flags)
                        ) {
                            V::Cleared
                        } else {
                            V::Unknown
                        };
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
        returned_bool |= regs[2] == V::Bool;
        index += 1;
    }
    (call_seen && test_100 && test_80 && updated && cleared && returned_bool && returned)
        || is_sp_task_yielded_register_resident(words)
}

fn is_sp_task_yielded_register_resident(words: &[u32]) -> bool {
    words.len() >= 19
        && is_addiu(words[0], 29, 29, -24)
        && is_sw(words[1], 16, 29, 16)
        && is_sw(words[2], 31, 29, 20)
        && jal_field(words[3]).is_some()
        && is_move_addu(words[4], 16, 4)
        && op(words[5]) == 0
        && words[5] & 0x3f == 2
        && rt(words[5]) == 2
        && rd(words[5]) == 4
        && (words[5] >> 6 & 31) == 8
        && is_andi(words[6], 2, 2, 0x80)
        && is_beq(words[7], 2, 0)
        && is_andi(words[8], 4, 4, 1)
        && is_lw_at(words[9], 2, 16, 4)
        && is_addiu(words[10], 3, 0, -3)
        && op(words[11]) == 0
        && words[11] & 0x3f == 0x25
        && rd(words[11]) == 2
        && rs(words[11]) == 2
        && rt(words[11]) == 4
        && op(words[12]) == 0
        && words[12] & 0x3f == 0x24
        && rd(words[12]) == 2
        && rs(words[12]) == 2
        && rt(words[12]) == 3
        && is_sw(words[13], 2, 16, 4)
        && is_move_addu(words[14], 2, 4)
        && is_lw_at(words[15], 31, 29, 20)
        && is_lw_at(words[16], 16, 29, 16)
        && is_jr_ra(words[17])
        && is_addiu(words[18], 29, 29, 24)
}
