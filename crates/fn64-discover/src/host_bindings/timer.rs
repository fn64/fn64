//! `osSetTimer` recognizer.

use super::core::{imm, is_addiu, is_jr_ra, is_store_opcode, jal_field, op, rd, rs, rt};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SetTimerValue {
    Unknown,
    /// Anything provably zero, used to clear the unlinked list pointers.
    Zero,
    /// The `OSTimer *`, o32 argument one.
    Timer,
    /// High and low words of the `countdown` argument. Under o32 the first
    /// 64-bit argument after the pointer is eight-byte aligned, so it occupies
    /// `$a2`/`$a3`... which is the *interval*; `countdown` therefore arrives in
    /// the caller's argument save area. See the offsets computed below.
    CountdownHigh,
    CountdownLow,
    /// High and low words of the `interval` argument, in `$a2`/`$a3`.
    IntervalHigh,
    IntervalLow,
    /// The destination `OSMesgQueue *`, from the argument save area.
    Queue,
    /// The `OSMesg`, from the argument save area.
    Mesg,
}

/// Recognize the public
/// `osSetTimer(OSTimer *, OSTime countdown, OSTime interval, OSMesgQueue *, OSMesg)`.
///
/// Every clause is a published-ABI property of the routine:
///
/// * it is a routine with a stack frame it restores before `jr $ra`;
/// * `$a0` is the `OSTimer *`, and `$a2`/`$a3` carry the 64-bit `interval`
///   (o32 eight-byte-aligns the first 64-bit argument after the pointer, so
///   `countdown` spills to the caller's argument save area at `frame + 16`,
///   with the queue and message following at `frame + 24` and `frame + 28`);
/// * the eight documented `OSTimer` words are written at their documented
///   offsets: `next`/`prev` cleared because the timer is not yet linked,
///   `value` from `countdown`, `interval` from `interval`, then `mq` and
///   `msg`; and
/// * an `interval` of zero makes the timer one-shot, so on that path `interval`
///   is also written from the `countdown` argument.
///
/// The stack-argument offsets are derived from the frame size rather than
/// pinned, so a build that inlines the timer-list walk and a build that
/// delegates it are both accepted despite their different frames. No register
/// assignment, instruction schedule or callee address is pinned.
pub(super) fn is_set_timer(words: &[u32]) -> bool {
    use SetTimerValue as V;

    const MIN_WORDS: usize = 12;
    if words.len() < MIN_WORDS || !is_addiu(words[0], 29, 29, imm(words[0])) || imm(words[0]) >= 0 {
        return false;
    }
    let frame_size = i32::from(imm(words[0])).unsigned_abs();
    let (Ok(countdown_high_slot), Ok(countdown_low_slot), Ok(queue_slot), Ok(mesg_slot)) = (
        i16::try_from(frame_size + 16),
        i16::try_from(frame_size + 20),
        i16::try_from(frame_size + 24),
        i16::try_from(frame_size + 28),
    ) else {
        return false;
    };

    let mut registers = [V::Unknown; 32];
    registers[0] = V::Zero;
    registers[4] = V::Timer;
    registers[6] = V::IntervalHigh;
    registers[7] = V::IntervalLow;
    let mut spill: BTreeMap<i16, V> = BTreeMap::new();
    // Every value observed written to each `OSTimer` word, across all paths.
    let mut fields: BTreeMap<i16, BTreeSet<V>> = BTreeMap::new();
    let mut saw_return = false;
    let mut frame_restored = false;
    let mut post_init_call = false;

    let mut index = 1;
    while index < words.len() {
        let word = words[index];
        if is_jr_ra(word) {
            saw_return = frame_restored
                || words
                    .get(index + 1)
                    .is_some_and(|&slot| is_addiu(slot, 29, 29, imm(words[0]).wrapping_neg()));
            break;
        }
        if is_addiu(word, 29, 29, imm(words[0]).wrapping_neg()) {
            frame_restored = true;
            index += 1;
            continue;
        }
        if frame_restored && rs(word) == 29 && matches!(op(word), 0x23 | 0x2b) {
            return false;
        }
        if jal_field(word).is_some() {
            if let Some(&slot) = words.get(index + 1) {
                set_timer_step(
                    &mut registers,
                    &mut spill,
                    &mut fields,
                    slot,
                    countdown_high_slot,
                    countdown_low_slot,
                    queue_slot,
                    mesg_slot,
                );
            }
            post_init_call |= registers[4] == V::Timer;
            for caller_saved in (1usize..16).chain([24usize, 25, 31]) {
                registers[caller_saved] = V::Unknown;
            }
            index += 2;
            continue;
        }
        set_timer_step(
            &mut registers,
            &mut spill,
            &mut fields,
            word,
            countdown_high_slot,
            countdown_low_slot,
            queue_slot,
            mesg_slot,
        );
        index += 1;
    }

    let wrote = |offset: i16, value: V| {
        fields
            .get(&offset)
            .is_some_and(|values| values.contains(&value))
    };

    saw_return
        && post_init_call
        // Not yet linked into the timer list.
        && wrote(0x00, V::Zero)
        && wrote(0x04, V::Zero)
        // value = countdown
        && wrote(0x08, V::CountdownHigh)
        && wrote(0x0c, V::CountdownLow)
        // interval = interval
        && wrote(0x10, V::IntervalHigh)
        && wrote(0x14, V::IntervalLow)
        // A zero interval means one-shot at countdown.
        && wrote(0x10, V::CountdownHigh)
        && wrote(0x14, V::CountdownLow)
        // Where the expiry message is delivered.
        && wrote(0x18, V::Queue)
        && wrote(0x1c, V::Mesg)
}

#[allow(clippy::too_many_arguments)]
fn set_timer_step(
    registers: &mut [SetTimerValue; 32],
    spill: &mut BTreeMap<i16, SetTimerValue>,
    fields: &mut BTreeMap<i16, BTreeSet<SetTimerValue>>,
    word: u32,
    countdown_high_slot: i16,
    countdown_low_slot: i16,
    queue_slot: i16,
    mesg_slot: i16,
) {
    use SetTimerValue as V;

    match op(word) {
        0x2b => {
            let source = registers[rt(word) as usize];
            if rs(word) == 29 {
                spill.insert(imm(word), source);
            } else if registers[rs(word) as usize] == V::Timer {
                fields.entry(imm(word)).or_default().insert(source);
            }
        }
        0x23 => {
            if rt(word) != 0 {
                registers[rt(word) as usize] = if rs(word) == 29 {
                    match imm(word) {
                        offset if offset == countdown_high_slot => V::CountdownHigh,
                        offset if offset == countdown_low_slot => V::CountdownLow,
                        offset if offset == queue_slot => V::Queue,
                        offset if offset == mesg_slot => V::Mesg,
                        offset => spill.get(&offset).copied().unwrap_or(V::Unknown),
                    }
                } else {
                    V::Unknown
                };
            }
        }
        0 => {
            let destination = rd(word) as usize;
            if destination == 0 {
                return;
            }
            match word & 0x3f {
                // Register moves propagate the tracked value.
                0x21 | 0x2d | 0x25 => {
                    registers[destination] = if rt(word) == 0 {
                        registers[rs(word) as usize]
                    } else if rs(word) == 0 {
                        registers[rt(word) as usize]
                    } else {
                        V::Unknown
                    };
                }
                0x08 | 0x09 => {}
                _ => registers[destination] = V::Unknown,
            }
        }
        // Branches and jumps write nothing we track.
        0x02..=0x07 | 0x14..=0x17 | 0x01 => {}
        _ => {
            if rt(word) != 0 && !is_store_opcode(op(word)) {
                registers[rt(word) as usize] = V::Unknown;
            }
        }
    }
}
