//! Per-role probing: which structural recognizer(s), if any, resolved each
//! member of the fifteen-symbol WM-block catalog, without failing the whole
//! discovery on a single unresolved role.

use super::core::{
    imm, is_addiu, jal_target, resolve_call_value, unique_match, CallValue,
    HostBindingDiscoveryError, HostBindingSymbol, WM_BLOCK_RUNTIME_HOST_SYMBOLS,
};
use super::io::is_si_device_busy;
use super::mesg::{is_create_mesg_queue, is_epi_start_dma, is_send_mesg, is_set_event_mesg};
use super::sp_task::{
    extract_sp_task_load_helpers, is_sp_task_load, is_sp_task_start_go, is_sp_task_yield,
    is_sp_task_yielded,
};
use super::thread::{
    is_get_thread_pri, is_set_thread_pri, is_start_thread, unique_create_thread_match,
};
use super::timer::is_set_timer;

/// Per-symbol outcome of running one recognizer independently of the chain.
///
/// [`discover_wm_block_runtime_host_bindings`] is a chain of `?`, so it aborts
/// at the first failing symbol and never evaluates the rest. Under that chain
/// "did not resolve" and "was never evaluated" are indistinguishable, which
/// has been misread as a score at least once. These variants keep them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostBindingProbeOutcome {
    /// Exactly one structural match; the role resolves standalone.
    Resolved { vram: u32 },
    /// The recognizer ran and found nothing.
    Absent,
    /// The recognizer ran and found several candidates, so the role is
    /// ambiguous rather than missing.
    Ambiguous { candidates: Vec<u32> },
    /// The recognizer ran and failed for some other reason.
    Failed { detail: String },
    /// The recognizer was never run because it is only reachable through
    /// multi-stage call-chain logic whose earlier stage did not resolve. This
    /// is *not* evidence the role is absent.
    NotReached { needs: &'static str },
}

impl HostBindingProbeOutcome {
    /// Whether this role resolved. Only [`Self::Resolved`] counts; in
    /// particular [`Self::NotReached`] is not a failure, it is an absence of
    /// measurement, and must never be scored as either.
    pub fn is_resolved(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }

    /// Whether the recognizer actually ran, and so contributes a denominator.
    pub fn was_evaluated(&self) -> bool {
        !matches!(self, Self::NotReached { .. })
    }

    fn from_unique(result: Result<u32, HostBindingDiscoveryError>) -> Self {
        match result {
            Ok(vram) => Self::Resolved { vram },
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { candidates, .. })
                if candidates.is_empty() =>
            {
                Self::Absent
            }
            Err(HostBindingDiscoveryError::NonUniqueSemanticMatch { candidates, .. }) => {
                Self::Ambiguous { candidates }
            }
            Err(error) => Self::Failed {
                detail: format!("{error:?}"),
            },
        }
    }
}

/// Run every host-binding recognizer independently and report the per-symbol
/// outcome.
///
/// This is a *measurement* surface: it reuses the recognizers verbatim and
/// changes none of them. Its only reason to exist is that the production entry
/// point short-circuits, which makes it unable to answer "how many roles does
/// this ROM resolve".
///
/// Twelve of the fifteen roles are separable this way. The remaining three are
/// genuinely derived from an earlier stage's output rather than matched
/// standalone -- `osRecvMesg` is identified by a call chain through the
/// resolved `osCreateMesgQueue` and `osEPiStartDma` addresses, and
/// `osSpTaskStartGo`/`osSpTaskYield` are matched against helper addresses
/// extracted from a resolved `osSpTaskLoad` body. When their prerequisite
/// resolves they are evaluated for real; when it does not they are reported
/// [`HostBindingProbeOutcome::NotReached`] rather than counted as absent.
///
/// The returned vector is symbol-sorted and always contains every one of
/// [`WM_BLOCK_RUNTIME_HOST_SYMBOLS`].
pub fn probe_wm_block_runtime_host_bindings(
    words: &[u32],
    va_start: u32,
) -> Vec<(HostBindingSymbol, HostBindingProbeOutcome)> {
    use HostBindingProbeOutcome as Outcome;
    use HostBindingSymbol as Symbol;

    if !va_start.is_multiple_of(4) {
        return WM_BLOCK_RUNTIME_HOST_SYMBOLS
            .into_iter()
            .map(|symbol| {
                (
                    symbol,
                    Outcome::Failed {
                        detail: format!("{:?}", HostBindingDiscoveryError::UnalignedImage),
                    },
                )
            })
            .collect();
    }

    let unique = |width: usize, symbol: Symbol, predicate: &dyn Fn(&[u32]) -> bool| {
        Outcome::from_unique(unique_match(words, va_start, width, symbol, predicate))
    };

    // The seven overlay-loader roles that are single-window predicates.
    let create = unique(12, Symbol::OsCreateMesgQueue, &is_create_mesg_queue);
    // Width 26 so the 1997-era stack-spilling build (World Tour) fits; see the
    // matching call in `discover_wm_block_runtime_host_bindings`.
    let epi = unique(26, Symbol::OsEPiStartDma, &is_epi_start_dma);
    let get_thread_pri = unique(6, Symbol::OsGetThreadPri, &is_get_thread_pri);
    let send = unique(83, Symbol::OsSendMesg, &is_send_mesg);
    let set_event = unique(48, Symbol::OsSetEventMesg, &is_set_event_mesg);
    let set_thread_pri = unique(20, Symbol::OsSetThreadPri, &is_set_thread_pri);
    let start_thread = unique(84, Symbol::OsStartThread, &is_start_thread);

    // Roles with their own public entry point.
    let create_thread = Outcome::from_unique(unique_create_thread_match(words, va_start));
    let set_timer = unique(100, Symbol::OsSetTimer, &is_set_timer);
    let si_busy = unique(11, Symbol::OsSiDeviceBusy, &is_si_device_busy);

    // The RSP task group. Load and Yielded stand alone; StartGo and Yield are
    // matched against helper addresses that only exist once Load resolves.
    let load = unique(131, Symbol::OsSpTaskLoad, &is_sp_task_load);
    let task_yielded = unique(32, Symbol::OsSpTaskYielded, &is_sp_task_yielded);
    let (start_go, task_yield) = match &load {
        Outcome::Resolved { vram } => {
            let index = ((vram - va_start) / 4) as usize;
            let load_words = &words[index..index + 131];
            let (busy, set_status) = extract_sp_task_load_helpers(load_words)
                .expect("load recognizer proved helper calls");
            (
                unique(16, Symbol::OsSpTaskStartGo, &|candidate: &[u32]| {
                    is_sp_task_start_go(candidate, busy, set_status)
                }),
                unique(8, Symbol::OsSpTaskYield, &|candidate: &[u32]| {
                    is_sp_task_yield(candidate, set_status)
                }),
            )
        }
        _ => (
            Outcome::NotReached {
                needs: "osSpTaskLoad",
            },
            Outcome::NotReached {
                needs: "osSpTaskLoad",
            },
        ),
    };

    // osRecvMesg is identified by the overlay call chain, which needs both the
    // queue initializer and the DMA starter resolved first.
    let recv = match (&create, &epi) {
        (Outcome::Resolved { vram: create_vram }, Outcome::Resolved { vram: epi_vram }) => {
            probe_overlay_recv_mesg(words, va_start, *create_vram, *epi_vram)
        }
        _ => Outcome::NotReached {
            needs: "osCreateMesgQueue + osEPiStartDma",
        },
    };

    let mut outcomes = vec![
        (Symbol::OsCreateMesgQueue, create),
        (Symbol::OsCreateThread, create_thread),
        (Symbol::OsEPiStartDma, epi),
        (Symbol::OsGetThreadPri, get_thread_pri),
        (Symbol::OsRecvMesg, recv),
        (Symbol::OsSendMesg, send),
        (Symbol::OsSetEventMesg, set_event),
        (Symbol::OsSiDeviceBusy, si_busy),
        (Symbol::OsSetThreadPri, set_thread_pri),
        (Symbol::OsSetTimer, set_timer),
        (Symbol::OsSpTaskLoad, load),
        (Symbol::OsSpTaskStartGo, start_go),
        (Symbol::OsSpTaskYield, task_yield),
        (Symbol::OsSpTaskYielded, task_yielded),
        (Symbol::OsStartThread, start_thread),
    ];
    outcomes.sort_by_key(|(symbol, _)| *symbol);
    outcomes
}

/// The `osRecvMesg` half of the overlay call chain, factored out so the probe
/// can run it once its two prerequisites are known. This mirrors the chain
/// search in [`discover_overlay_loader_host_bindings`] exactly.
pub(super) fn probe_overlay_recv_mesg(
    words: &[u32],
    va_start: u32,
    create: u32,
    epi: u32,
) -> HostBindingProbeOutcome {
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
                if create_queue != CallValue::Unknown
                    && create_queue == recv_queue
                    && matches!(recv_output, CallValue::Stack(_))
                    && recv_block == CallValue::Constant(1)
                    || recv_call_index >= 2
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
                        && recv_block == CallValue::Constant(1)
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
    match recv_targets.as_slice() {
        [recv] => HostBindingProbeOutcome::Resolved { vram: *recv },
        [] => HostBindingProbeOutcome::Absent,
        several => HostBindingProbeOutcome::Ambiguous {
            candidates: several.to_vec(),
        },
    }
}
