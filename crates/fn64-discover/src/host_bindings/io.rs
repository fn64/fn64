//! `osViSiHandler`/PI-device-busy, raw EPI device I/O, and the boot-side
//! `discover_*` entry points: drive-ROM init, SI device busy, create-thread,
//! overlay loader, RSP task, timer, and the top-level WM-block catalog.

use super::core::{
    absolute_from_lui_offset, imm, is_addiu, is_jr_ra, is_lui, is_lw, jal_field, jal_target, op,
    rd, resolve_call_value, rs, rt, unique_match, CallValue, HostBinding,
    HostBindingDiscoveryError, HostBindingSymbol,
};
use super::mesg::{is_create_mesg_queue, is_epi_start_dma, is_send_mesg, is_set_event_mesg};
use super::sp_task::{
    extract_sp_task_load_helpers, is_sp_task_load, is_sp_task_start_go, is_sp_task_yield,
    is_sp_task_yielded,
};
use super::thread::{
    is_get_thread_pri, is_set_thread_pri, is_start_thread, unique_create_thread_match,
};
use super::timer::is_set_timer;

pub(super) fn is_si_device_busy(words: &[u32]) -> bool {
    if words.len() < 6 || op(words[0]) != 0x0f || words[0] as u16 != 0xa480 {
        return false;
    }
    let mut hi = [None; 32];
    let mut status = [false; 32];
    let mut masked = [false; 32];
    let mut loaded_status = false;
    let mut normalized = false;
    let mut returned = false;
    for (index, &word) in words.iter().enumerate() {
        match op(word) {
            0x0f => hi[rt(word) as usize] = Some((word & 0xffff) << 16),
            0x0d => {
                let base = rs(word) as usize;
                let dst = rt(word) as usize;
                hi[dst] = hi[base].map(|value| value | u32::from(word as u16));
            }
            0x23 => {
                let base = rs(word) as usize;
                let dst = rt(word) as usize;
                let absolute =
                    hi[base].map(|value| value.wrapping_add(i32::from(imm(word)) as u32));
                status[dst] = absolute == Some(0xa480_0018);
                loaded_status |= status[dst];
                hi[dst] = None;
            }
            0x0c => {
                let dst = rt(word) as usize;
                masked[dst] = status[rs(word) as usize] && imm(word) == 3;
                hi[dst] = None;
            }
            0x04 | 0x05 => {
                normalized |= (masked[rs(word) as usize] && rt(word) == 0)
                    || (masked[rt(word) as usize] && rs(word) == 0);
            }
            0 => match word & 0x3f {
                0x2a | 0x2b => {
                    normalized |= (masked[rs(word) as usize] && rt(word) == 0)
                        || (masked[rt(word) as usize] && rs(word) == 0);
                }
                0x08 => {
                    if rs(word) == 31 {
                        if let Some(&slot) = words.get(index + 1) {
                            if op(slot) == 0 && matches!(slot & 0x3f, 0x2a | 0x2b) {
                                normalized |= (masked[rs(slot) as usize] && rt(slot) == 0)
                                    || (masked[rt(slot) as usize] && rs(slot) == 0);
                            }
                        }
                        returned = true;
                        break;
                    }
                }
                0x20 | 0x21 | 0x25 => {
                    let src = if rt(word) == 0 {
                        rs(word)
                    } else if rs(word) == 0 {
                        rt(word)
                    } else {
                        0
                    };
                    if rd(word) != 0 {
                        masked[rd(word) as usize] = masked[src as usize];
                        status[rd(word) as usize] = status[src as usize];
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    loaded_status && normalized && returned
}

/// The body of a public `__osEPiRaw{Read,Write}Io`, validated through the
/// wrapper that calls it.
///
/// Every clause is a published property of the routine rather than of one
/// compilation: `OSPiHandle.baseAddress` lives at the documented offset 12, the
/// caller's `devAddr` is ORed into it, the result is forced to the uncached
/// KSEG1 view, and exactly one device access is performed through the pointer
/// so formed. A routine that never builds an uncached PI device pointer out of
/// `handle + 12` is not this routine, whatever else it resembles.
///
/// This exists because the wrapper shape alone is not decisive. A routine that
/// preserves three arguments across a bracketed call is a common compiler
/// idiom; measured over a 287-ROM corpus it is what Rogue Squadron's
/// table-search helper and Turok 3's forwarding wrappers look like too.
/// Checking the callee is what separates PI device IO from those.
pub(super) fn is_raw_epi_device_io(words: &[u32]) -> bool {
    words.iter().enumerate().any(|(index, word)| {
        // The handle's public `baseAddress` field, loaded from `$a0`.
        if !(op(*word) == 0x23 && imm(*word) == 12 && rs(*word) == 4) {
            return false;
        }
        let tail = &words[index + 1..words.len().min(index + 6)];
        // The uncached KSEG1 device view, the devAddr merge, and the single
        // access through the resulting pointer.
        tail.iter()
            .any(|word| op(*word) == 0x0f && rs(*word) == 0 && (*word & 0xffff) == 0xa000)
            && tail
                .iter()
                .any(|word| op(*word) == 0 && word & 0x3f == 0x25)
            && tail
                .iter()
                .any(|word| op(*word) == 0x23 || op(*word) == 0x2b)
    })
}

/// Recognize the public `osEPiWriteIo(OSPiHandle *, u32 devAddr, u32 data)` and
/// `osEPiReadIo(OSPiHandle *, u32 devAddr, u32 *data)`.
///
/// Both are the same published shape -- acquire the PI bus, perform one raw
/// device access, release it -- and differ only in which raw routine sits in
/// the middle. The caller resolves that distinction; this predicate identifies
/// the bracketed-IO shape and returns the three call targets in program order.
///
/// The clauses, each a property of the public routine:
///
/// * it builds a stack frame with a negative immediate and restores exactly
///   that frame in the `jr $ra` delay slot;
/// * it saves `$ra`, because it makes calls;
/// * each of the three o32 arguments is moved to a callee-saved register
///   before the first call, since each must survive it;
/// * it makes exactly three calls, in the order acquire, raw op, release;
/// * acquire and release are distinct entry points (`__osPiGetAccess` and
///   `__osPiRelAccess` are different routines);
/// * the acquire call is argument-free -- `__osPiGetAccess(void)` takes no
///   arguments, so nothing may write `$a0`/`$a1`/`$a2` before it; and
/// * the three preserved arguments are routed back into `$a0`/`$a1`/`$a2` for
///   the raw op.
///
/// No register number, instruction schedule, frame size, or address is pinned.
/// The argument-free-acquire clause is the one that rejects the generic
/// three-argument forwarding wrapper this otherwise resembles.
pub(super) fn epi_io_wrapper_targets(words: &[u32]) -> Option<(u32, u32, u32)> {
    if !(is_addiu(words[0], 29, 29, imm(words[0])) && imm(words[0]) < 0) {
        return None;
    }
    let frame = imm(words[0]);
    // The frame this routine created must be the frame it tears down.
    let end = (4..words.len().saturating_sub(1))
        .find(|index| is_jr_ra(words[*index]) && is_addiu(words[index + 1], 29, 29, -frame))?;
    let body = &words[..end];
    if !body
        .iter()
        .any(|word| op(*word) == 0x2b && rt(*word) == 31 && rs(*word) == 29)
    {
        return None;
    }

    let calls = body
        .iter()
        .enumerate()
        .filter_map(|(index, word)| jal_field(*word).map(|target| (index, target)))
        .collect::<Vec<_>>();
    let [(acquire_at, acquire), (raw_at, raw), (_, release)] = calls[..] else {
        return None;
    };
    if acquire == release || !(acquire_at < raw_at) {
        return None;
    }

    // The acquire call takes no arguments.
    let writes_argument = |word: u32| -> Option<u32> {
        let target = match op(word) {
            0 => rd(word),
            0x08 | 0x09 | 0x0c | 0x0d | 0x0e | 0x0f | 0x23 | 0x24 | 0x25 => rt(word),
            _ => return None,
        };
        (4..=6).contains(&target).then_some(target)
    };
    if body[..=acquire_at.min(body.len() - 1)]
        .iter()
        .enumerate()
        .any(|(index, word)| index != acquire_at && writes_argument(*word).is_some())
    {
        return None;
    }

    // Each argument is preserved in a callee-saved register across the call,
    // then routed back into its o32 argument register for the raw op.
    let mut preserved = [None; 3];
    for word in &body[..=(acquire_at + 1).min(body.len() - 1)] {
        if op(*word) == 0 && *word & 0x3f == 0x21 && (16..24).contains(&rd(*word)) {
            let source = match (rs(*word), rt(*word)) {
                (source, 0) => source,
                (0, source) => source,
                _ => continue,
            };
            if (4..=6).contains(&source) {
                preserved[source as usize - 4] = Some(rd(*word));
            }
        }
    }
    if preserved.iter().any(Option::is_none) {
        return None;
    }
    let mut routed = [None; 3];
    for word in &body[acquire_at + 1..=(raw_at + 1).min(body.len() - 1)] {
        if op(*word) == 0 && *word & 0x3f == 0x21 && (4..=6).contains(&rd(*word)) {
            let source = match (rs(*word), rt(*word)) {
                (source, 0) => source,
                (0, source) => source,
                _ => continue,
            };
            routed[rd(*word) as usize - 4] = Some(source);
        }
    }
    (routed == preserved).then_some((acquire, raw, release))
}

/// Structural shape of libultra's 64DD drive initialisation.
///
/// The routine is recognised by what it *does*, not by any address: it loads a
/// once-only guard word through a `lui`/`lw` pair, branches away when that word
/// is already non-zero, and on the first-call path installs the 64DD base
/// `0xA600_0000` into the handle it will later probe.
///
/// The `lui $r, 0xA600` is the distinguishing behaviour. `0x0600_0000..=
/// 0x07ff_ffff` is `PI_DOM1_ADDR1`, the disk-drive window, and a cartridge-only
/// title has no device there -- which is why the probe that follows faults with
/// `abi.pi.absent-domain1-device`. No other libultra role installs that base.
///
/// Window is 20 words. Measured on WM2000: the guard `lui`/`lw` sits at
/// +0x00/+0x04 and the `lui $r, 0xA600` at +0x34, i.e. 13 words later, with
/// register setup and the handle-pointer construction in between.
fn is_drive_rom_init(words: &[u32]) -> bool {
    // lui/lw pair loading the once-only guard.
    if !(is_lui(words[0], 14) && is_lw(words[1], 14, 14)) {
        return false;
    }
    // A branch that skips initialisation when the guard is already set.
    let guard_branch = words[2..6]
        .iter()
        .any(|word| op(*word) == 0x05 && (rs(*word) == 14 || rt(*word) == 14));
    if !guard_branch {
        return false;
    }
    // The first-call path installs the 64DD base into the handle.
    words[2..20]
        .iter()
        .any(|word| op(*word) == 0x0f && rs(*word) == 0 && (*word & 0xffff) == 0xa600)
}

/// A recovered 64DD drive-init routine and the guard word it tests.
///
/// The guard is the useful output. Both of the routine's paths return the same
/// static `OSPiHandle *`; the guard only decides whether the device probe runs.
/// A consumer that presets it therefore selects a path the guest already
/// implements, without inventing a bus value or a return contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DriveRomInitBinding {
    pub binding: HostBinding,
    /// Physical address of the once-only guard word, recovered from the same
    /// `lui`/`lw` pair the recognizer matches.
    pub guard_vram: u32,
}

/// Discover libultra's 64DD drive initialisation from its public guard-word and
/// base-installation behavior.
///
/// This role is optional: a title that never touches the disk drive contains no
/// such routine, and `Ok(None)` distinguishes that from a ROM where the shape is
/// ambiguous, which stays a loud failure like every other role here.
pub fn discover_drive_rom_init_host_binding(
    words: &[u32],
    va_start: u32,
) -> Result<Option<DriveRomInitBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    match unique_match(
        words,
        va_start,
        20,
        HostBindingSymbol::OsDriveRomInit,
        is_drive_rom_init,
    ) {
        Ok(vram) => {
            let index = ((vram - va_start) / 4) as usize;
            // The matched window opens with the guard's own lui/lw pair, so its
            // address is recoverable from the same words the predicate checked.
            let guard_vram = absolute_from_lui_offset(words[index], imm(words[index + 1]));
            Ok(Some(DriveRomInitBinding {
                binding: HostBinding {
                    symbol: HostBindingSymbol::OsDriveRomInit,
                    vram,
                },
                guard_vram,
            }))
        }
        // `unique_match` reports both "no match" and "several matches" as
        // NonUniqueSemanticMatch; an empty candidate list is the absent case.
        // A title with no disk-drive routine is normal, several is ambiguous
        // and stays a loud failure like every other role here.
        Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { candidates, .. })
            if candidates.is_empty() =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Discover `__osSiDeviceBusy` from its public SI status-register behavior.
/// The role must have exactly one structural match in the resident image.
pub fn discover_si_device_busy_host_binding(
    words: &[u32],
    va_start: u32,
) -> Result<HostBinding, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    let vram = unique_match(
        words,
        va_start,
        11,
        HostBindingSymbol::OsSiDeviceBusy,
        is_si_device_busy,
    )?;
    Ok(HostBinding {
        symbol: HostBindingSymbol::OsSiDeviceBusy,
        vram,
    })
}

/// Discover `osCreateThread` from its public `OSThread` initialization
/// behavior. The role must have exactly one structural match in the resident
/// image.
pub fn discover_os_create_thread_host_binding(
    words: &[u32],
    va_start: u32,
) -> Result<HostBinding, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;
    let vram = unique_create_thread_match(words, va_start)?;
    Ok(HostBinding {
        symbol: HostBindingSymbol::OsCreateThread,
        vram,
    })
}

/// Discover the host calls required by the admitted libultra overlay-loading
/// sequence in one resident image. The returned inventory is symbol-sorted.
pub fn discover_overlay_loader_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;
    // Twelve words: the six documented stores plus the lui/addiu pairs, which
    // the 1996-era build emits once per queue head rather than once in total.
    let create = unique_match(
        words,
        va_start,
        12,
        HostBindingSymbol::OsCreateMesgQueue,
        is_create_mesg_queue,
    )?;
    let create_thread = discover_os_create_thread_host_binding(words, va_start)?.vram;
    // Wide enough to contain the 1997-era stack-spilling compilation (WCW/nWo
    // World Tour), whose argument reloads push the direction-gated type stamp
    // to word ~24; the register-resident builds sit well inside this window and
    // `collapse_overlapping_runs` reduces the several containing starts to the
    // single routine entry.
    let epi = unique_match(
        words,
        va_start,
        26,
        HostBindingSymbol::OsEPiStartDma,
        is_epi_start_dma,
    )?;
    let get_thread_pri = unique_match(
        words,
        va_start,
        6,
        HostBindingSymbol::OsGetThreadPri,
        is_get_thread_pri,
    )?;
    let send = unique_match(
        words,
        va_start,
        83,
        HostBindingSymbol::OsSendMesg,
        is_send_mesg,
    )?;
    // Wide enough to contain the epilogue of the longer compilation, which
    // carries an `OS_EVENT_PRENMI` tail after the table store.
    let set_event = unique_match(
        words,
        va_start,
        48,
        HostBindingSymbol::OsSetEventMesg,
        is_set_event_mesg,
    )?;
    let set_thread_pri = unique_match(
        words,
        va_start,
        20,
        HostBindingSymbol::OsSetThreadPri,
        is_set_thread_pri,
    )?;
    let start_thread = unique_match(
        words,
        va_start,
        84,
        HostBindingSymbol::OsStartThread,
        is_start_thread,
    )?;
    let mut chains = Vec::new();
    for (create_call_index, &word) in words.iter().enumerate() {
        let create_call_pc = va_start + create_call_index as u32 * 4;
        if jal_target(word, create_call_pc) != Some(create) {
            continue;
        }
        let search_end = (create_call_index + 128).min(words.len());
        for epi_call_index in create_call_index + 1..search_end {
            let epi_call_pc = va_start + epi_call_index as u32 * 4;
            if jal_target(words[epi_call_index], epi_call_pc) != Some(epi) {
                continue;
            }
            let recv_end = (epi_call_index + 12).min(words.len());
            for recv_call_index in epi_call_index + 1..recv_end {
                if recv_call_index + 1 >= words.len() {
                    continue;
                }
                let recv_call_pc = va_start + recv_call_index as u32 * 4;
                let Some(recv) = jal_target(words[recv_call_index], recv_call_pc) else {
                    continue;
                };
                let lower = (create_call_index.saturating_sub(96)..=create_call_index)
                    .rev()
                    .find(|&candidate| {
                        is_addiu(words[candidate], 29, 29, imm(words[candidate]))
                            && imm(words[candidate]) < 0
                    })
                    .unwrap_or(create_call_index.saturating_sub(32));
                let create_queue = resolve_call_value(words, lower, create_call_index, 4, 0);
                let recv_queue = resolve_call_value(words, lower, recv_call_index, 4, 0);
                let recv_output = resolve_call_value(words, lower, recv_call_index, 5, 0);
                let recv_block = if is_addiu(words[recv_call_index + 1], 6, 0, 1) {
                    CallValue::Constant(1)
                } else {
                    resolve_call_value(words, lower, recv_call_index, 6, 0)
                };
                if (create_queue != CallValue::Unknown
                    && create_queue == recv_queue
                    && matches!(recv_output, CallValue::Stack(_))
                    && recv_block == CallValue::Constant(1))
                    || (recv_call_index >= 2
                        && is_addiu(
                            words[recv_call_index - 2],
                            4,
                            29,
                            imm(words[recv_call_index - 2]),
                        )
                        && is_addiu(
                            words[recv_call_index - 1],
                            5,
                            29,
                            imm(words[recv_call_index - 1]),
                        )
                        && recv_block == CallValue::Constant(1))
                {
                    chains.push((create_call_pc, epi_call_pc, recv));
                }
            }
        }
    }
    chains.sort_unstable();
    chains.dedup();
    let mut recv_targets = chains.iter().map(|(_, _, recv)| *recv).collect::<Vec<_>>();
    recv_targets.sort_unstable();
    recv_targets.dedup();
    let recv = match recv_targets.as_slice() {
        [recv] => *recv,
        _ => {
            return Err(HostBindingDiscoveryError::NonUniqueOverlayCallChain { candidates: chains })
        }
    };
    Ok(vec![
        HostBinding {
            symbol: HostBindingSymbol::OsCreateMesgQueue,
            vram: create,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsCreateThread,
            vram: create_thread,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsEPiStartDma,
            vram: epi,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsGetThreadPri,
            vram: get_thread_pri,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsRecvMesg,
            vram: recv,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSendMesg,
            vram: send,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSetEventMesg,
            vram: set_event,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSetThreadPri,
            vram: set_thread_pri,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsStartThread,
            vram: start_thread,
        },
    ])
}

/// Discover the public RSP task admission/start and yield/query routines in a
/// resident libultra image. Cross-function helper identities are part of the
/// proof: Load and StartGo must share the busy/status helpers, and Yield must
/// use that same status helper. The returned inventory is symbol-sorted.
pub fn discover_rsp_task_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;

    let load = unique_match(
        words,
        va_start,
        131,
        HostBindingSymbol::OsSpTaskLoad,
        is_sp_task_load,
    )?;
    let load_index = usize::try_from((load - va_start) / 4).expect("load index fits usize");
    let load_words = &words[load_index..load_index + 131];
    let (busy, set_status) =
        extract_sp_task_load_helpers(load_words).expect("load recognizer proved helper calls");

    let start_go = unique_match(
        words,
        va_start,
        16,
        HostBindingSymbol::OsSpTaskStartGo,
        |candidate| is_sp_task_start_go(candidate, busy, set_status),
    )?;
    let task_yield = unique_match(
        words,
        va_start,
        8,
        HostBindingSymbol::OsSpTaskYield,
        |candidate| is_sp_task_yield(candidate, set_status),
    )?;
    let task_yielded = unique_match(
        words,
        va_start,
        32,
        HostBindingSymbol::OsSpTaskYielded,
        is_sp_task_yielded,
    )?;

    Ok(vec![
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskLoad,
            vram: load,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskStartGo,
            vram: start_go,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskYield,
            vram: task_yield,
        },
        HostBinding {
            symbol: HostBindingSymbol::OsSpTaskYielded,
            vram: task_yielded,
        },
    ])
}

/// Discover the public timer-wheel admission routine in a resident libultra
/// image. The recognizer is based on the documented OSTimer field/argument
/// layout and list insertion behavior; the address is only an output.
pub fn discover_timer_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    if !va_start.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::UnalignedImage);
    }
    va_start
        .checked_add(
            u32::try_from(words.len())
                .map_err(|_| HostBindingDiscoveryError::AddressOverflow)?
                .checked_mul(4)
                .ok_or(HostBindingDiscoveryError::AddressOverflow)?,
        )
        .ok_or(HostBindingDiscoveryError::AddressOverflow)?;
    // Wide enough to contain the epilogue of the longer compilation, which
    // inlines the timer-list walk rather than delegating it.
    let set_timer = unique_match(
        words,
        va_start,
        100,
        HostBindingSymbol::OsSetTimer,
        is_set_timer,
    )?;
    Ok(vec![HostBinding {
        symbol: HostBindingSymbol::OsSetTimer,
        vram: set_timer,
    }])
}

/// Discover the exact host-adapter catalog installed by the WM block runtime.
/// Keeping this assembly in the library prevents the production build and the
/// source-frontier producer from silently drifting to different target sets.
pub fn discover_wm_block_runtime_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Result<Vec<HostBinding>, HostBindingDiscoveryError> {
    let mut bindings = discover_overlay_loader_host_bindings(words, va_start)?;
    bindings.extend(discover_rsp_task_host_bindings(words, va_start)?);
    bindings.extend(discover_timer_host_bindings(words, va_start)?);
    bindings.push(discover_si_device_busy_host_binding(words, va_start)?);
    bindings.sort_by_key(|binding| binding.symbol);
    for (index, binding) in bindings.iter().enumerate() {
        if let Some(conflict) = bindings[..index]
            .iter()
            .find(|known| known.vram == binding.vram)
        {
            return Err(HostBindingDiscoveryError::ConflictingBindingAddress {
                vram: binding.vram,
                first: conflict.symbol,
                second: binding.symbol,
            });
        }
    }
    Ok(bindings)
}
