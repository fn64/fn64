//! The validated-external-symbol fallback: a symbol an author's own
//! decompilation names, admitted only after this module's own structural
//! shape sanity check confirms the address behaves like the claimed role.

use super::core::{HostBindingDiscoveryError, HostBindingSymbol};
use super::io::is_si_device_busy;
use super::mesg::{is_create_mesg_queue, is_epi_start_dma, is_send_mesg, is_set_event_mesg};
use super::probe::{
    probe_overlay_recv_mesg, probe_wm_block_runtime_host_bindings, HostBindingProbeOutcome,
};
use super::sp_task::{
    extract_sp_task_load_helpers, is_sp_task_load, is_sp_task_start_go, is_sp_task_yield,
    is_sp_task_yielded,
};
use super::thread::{is_create_thread, is_get_thread_pri, is_set_thread_pri, is_start_thread};
use super::timer::is_set_timer;
use std::collections::BTreeMap;

/// How a resolved WM-block host binding's address was arrived at.
///
/// The gate's 15/15 requirement is met by "recognizer OR validated external",
/// but the provenance stays honest about which of the two actually named each
/// address, so an audit can see exactly how much of a title's catalog leans on
/// an external decompilation versus fn64's own structural recognizers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolutionProvenance {
    /// A structural recognizer produced a unique match at this address. This is
    /// the only provenance the three already-passing titles ever carry.
    ByRecognizer,
    /// No recognizer uniquely resolved this symbol, so its address was taken
    /// from an external reference table -- but only after two validations
    /// passed: it did not contradict any recognizer that *did* fire, and the
    /// routine disassembled at that address exhibits the symbol's required
    /// shape. `source` records the provenance tag of the table it came from.
    ExternalReferenceValidated { source: String },
    /// A recognizer resolved this symbol AND an external table named the same
    /// address. The agreement is itself evidence the external table describes
    /// this ROM; the binding is recognizer-authoritative but the corroboration
    /// is recorded.
    ByRecognizerConfirmedByExternal { source: String },
}

/// A WM-block host binding together with the provenance of its address.
///
/// Parallel to [`HostBinding`] rather than a field on it, so that the forty
/// existing recognizer-only call sites and their tests are untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedHostBinding {
    pub symbol: HostBindingSymbol,
    pub vram: u32,
    pub provenance: ResolutionProvenance,
}

/// A provenance-tagged external reference table: a map from host-binding symbol
/// to its VRAM address, plus a human-readable tag naming where the mapping came
/// from (e.g. an upstream decompilation's symbol dump).
///
/// This is an *optional grading input*, never an authority seed. Nothing here
/// is trusted on its face: every address it supplies is either checked for
/// agreement against a recognizer that fired, or shape-validated by
/// disassembling the routine it points at, before any binding is emitted from
/// it. That mirrors fn64's standing rule that symbol files grade and validate
/// but never silently seed authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSymbolTable {
    pub source: String,
    pub addresses: BTreeMap<HostBindingSymbol, u32>,
}

impl ExternalSymbolTable {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            addresses: BTreeMap::new(),
        }
    }

    pub fn with(mut self, symbol: HostBindingSymbol, vram: u32) -> Self {
        self.addresses.insert(symbol, vram);
        self
    }
}

/// Context extracted from the recognizers' own resolved addresses, needed to
/// shape-validate the *derived* roles (whose predicates take a prerequisite's
/// helper addresses as parameters) at an external address.
struct DerivedShapeContext {
    /// `(create, epi)` for the overlay `osRecvMesg` call-chain shape.
    overlay_recv: Option<(u32, u32)>,
    /// `(busy, set_status)` helper `jal` fields extracted from a resolved
    /// `osSpTaskLoad` body, needed by the StartGo / Yield shape predicates.
    sp_helpers: Option<(u32, u32)>,
}

/// Run the shape predicate a symbol's recognizer would apply, but anchored at a
/// specific external address rather than scanned across the image. Returns
/// `Ok(())` when the routine there is shaped like that symbol, and an
/// [`HostBindingDiscoveryError`] describing the rejection otherwise.
///
/// This is the load-bearing half of "validated, not trusted": it reuses the
/// exact predicates the recognizers encode, so an external address is accepted
/// only if it points at something fn64 would itself have recognized had the
/// address been unique.
fn shape_sanity_at(
    words: &[u32],
    va_start: u32,
    symbol: HostBindingSymbol,
    external: u32,
    context: &DerivedShapeContext,
) -> Result<(), HostBindingDiscoveryError> {
    if external < va_start || !external.is_multiple_of(4) {
        return Err(HostBindingDiscoveryError::ExternalReferenceOutOfRange { symbol, external });
    }
    let index = ((external - va_start) / 4) as usize;
    if index >= words.len() {
        return Err(HostBindingDiscoveryError::ExternalReferenceOutOfRange { symbol, external });
    }
    // Anchor a window at the external address exactly as `unique_match` presents
    // one to the recognizer: the same fixed width per symbol, so the predicate
    // sees precisely what it would have seen scanning. This matters because a
    // few predicates (notably `is_si_device_busy`) require an exact window
    // length, so slicing to end-of-image would spuriously reject them. The width
    // is clamped to the image end; a routine truncated by the image boundary
    // fails its predicate, which is the correct rejection.
    //
    // `osCreateThread` is the one recognizer that scans a length *range* rather
    // than a fixed width; anchoring at the entry, its `MAX_WORDS` upper bound is
    // the faithful window.
    let width = match symbol {
        HostBindingSymbol::OsCreateMesgQueue => 12,
        HostBindingSymbol::OsEPiStartDma => 26,
        HostBindingSymbol::OsGetThreadPri => 6,
        HostBindingSymbol::OsSendMesg => 83,
        HostBindingSymbol::OsSetEventMesg => 48,
        HostBindingSymbol::OsSetThreadPri => 20,
        HostBindingSymbol::OsStartThread => 84,
        HostBindingSymbol::OsCreateThread => 96,
        HostBindingSymbol::OsSetTimer => 100,
        HostBindingSymbol::OsSiDeviceBusy => 11,
        HostBindingSymbol::OsSpTaskLoad => 131,
        HostBindingSymbol::OsSpTaskYielded => 32,
        HostBindingSymbol::OsSpTaskStartGo => 16,
        HostBindingSymbol::OsSpTaskYield => 8,
        // osRecvMesg is validated by the overlay call chain, not a window.
        HostBindingSymbol::OsRecvMesg => 1,
        HostBindingSymbol::OsDriveRomInit
        | HostBindingSymbol::OsEPiWriteIo
        | HostBindingSymbol::OsEPiReadIo
        | HostBindingSymbol::OsFlashInit
        | HostBindingSymbol::OsFlashSectorErase
        | HostBindingSymbol::OsFlashReadArray => 1,
    };
    let body = &words[index..(index + width).min(words.len())];

    let shaped = match symbol {
        HostBindingSymbol::OsCreateMesgQueue => is_create_mesg_queue(body),
        HostBindingSymbol::OsEPiStartDma => is_epi_start_dma(body),
        HostBindingSymbol::OsGetThreadPri => is_get_thread_pri(body),
        HostBindingSymbol::OsSendMesg => is_send_mesg(body),
        HostBindingSymbol::OsSetEventMesg => is_set_event_mesg(body),
        HostBindingSymbol::OsSetThreadPri => is_set_thread_pri(body),
        HostBindingSymbol::OsStartThread => is_start_thread(body),
        HostBindingSymbol::OsCreateThread => is_create_thread(body),
        HostBindingSymbol::OsSetTimer => is_set_timer(body),
        HostBindingSymbol::OsSiDeviceBusy => is_si_device_busy(body),
        HostBindingSymbol::OsSpTaskLoad => is_sp_task_load(body),
        HostBindingSymbol::OsSpTaskYielded => is_sp_task_yielded(body),
        // Derived roles: their shape predicates require a prerequisite's helper
        // addresses. Validate only when that prerequisite recognizer resolved;
        // otherwise the external address is unvalidatable rather than trusted.
        HostBindingSymbol::OsSpTaskStartGo => {
            let Some((busy, set_status)) = context.sp_helpers else {
                return Err(HostBindingDiscoveryError::ExternalReferenceUnvalidatable {
                    symbol,
                    external,
                    needs: "osSpTaskLoad",
                });
            };
            is_sp_task_start_go(body, busy, set_status)
        }
        HostBindingSymbol::OsSpTaskYield => {
            let Some((_, set_status)) = context.sp_helpers else {
                return Err(HostBindingDiscoveryError::ExternalReferenceUnvalidatable {
                    symbol,
                    external,
                    needs: "osSpTaskLoad",
                });
            };
            is_sp_task_yield(body, set_status)
        }
        HostBindingSymbol::OsRecvMesg => {
            let Some((create, epi)) = context.overlay_recv else {
                return Err(HostBindingDiscoveryError::ExternalReferenceUnvalidatable {
                    symbol,
                    external,
                    needs: "osCreateMesgQueue + osEPiStartDma",
                });
            };
            // osRecvMesg has no standalone structural signature; the recognizer
            // identifies it as the call target of the overlay loader's blocking
            // receive. Shape-validating an external osRecvMesg address therefore
            // means proving the overlay chain names *this* address.
            match probe_overlay_recv_mesg(words, va_start, create, epi) {
                HostBindingProbeOutcome::Resolved { vram } => vram == external,
                _ => false,
            }
        }
        // Roles outside the WM-block catalog are not external-resolvable here.
        HostBindingSymbol::OsDriveRomInit
        | HostBindingSymbol::OsEPiWriteIo
        | HostBindingSymbol::OsEPiReadIo
        | HostBindingSymbol::OsFlashInit
        | HostBindingSymbol::OsFlashSectorErase
        | HostBindingSymbol::OsFlashReadArray => false,
    };

    if shaped {
        Ok(())
    } else {
        Err(HostBindingDiscoveryError::ExternalReferenceShapeMismatch { symbol, external })
    }
}

/// Discover the WM-block host-binding catalog, resolving each symbol by its
/// structural recognizer first and falling back to a VALIDATED external
/// reference address only where the recognizer did not uniquely resolve.
///
/// Resolution order, per symbol:
///
/// 1. **Recognizer first.** Every recognizer runs exactly as in
///    [`discover_wm_block_runtime_host_bindings`]. A unique structural match is
///    always preferred and always authoritative.
/// 2. **Agreement check.** If a recognizer resolved *and* the external table
///    names the same symbol, the two addresses MUST be equal. Disagreement is a
///    hard [`HostBindingDiscoveryError::ExternalReferenceDisagreement`] -- never
///    a silent preference for one over the other.
/// 3. **Validated external fallback.** If the recognizer did not uniquely
///    resolve but the external table names the symbol, the routine at that
///    address is disassembled and checked against the symbol's own recognizer
///    shape ([`shape_sanity_at`]). Only a shaped routine is bound, with
///    provenance [`ResolutionProvenance::ExternalReferenceValidated`]; a
///    mismatch is [`HostBindingDiscoveryError::ExternalReferenceShapeMismatch`].
///
/// The returned catalog is symbol-sorted and, on `Ok`, contains exactly the
/// fifteen [`WM_BLOCK_RUNTIME_HOST_SYMBOLS`]. A symbol that neither recognizes
/// nor has a validated external address leaves the result short of fifteen (it
/// is simply absent), so callers still see an honest denominator.
pub fn discover_wm_block_runtime_host_bindings_with_external_reference(
    words: &[u32],
    va_start: u32,
    external: &ExternalSymbolTable,
) -> Result<Vec<ResolvedHostBinding>, HostBindingDiscoveryError> {
    let outcomes = probe_wm_block_runtime_host_bindings(words, va_start);

    // Assemble the context the derived-role shape checks need, from whatever the
    // recognizers themselves resolved.
    let resolved_of = |symbol: HostBindingSymbol| -> Option<u32> {
        outcomes.iter().find_map(|(candidate, outcome)| {
            if *candidate == symbol {
                if let HostBindingProbeOutcome::Resolved { vram } = outcome {
                    return Some(*vram);
                }
            }
            None
        })
    };
    let overlay_recv = match (
        resolved_of(HostBindingSymbol::OsCreateMesgQueue),
        resolved_of(HostBindingSymbol::OsEPiStartDma),
    ) {
        (Some(create), Some(epi)) => Some((create, epi)),
        _ => None,
    };
    let sp_helpers = resolved_of(HostBindingSymbol::OsSpTaskLoad).and_then(|load| {
        let index = ((load - va_start) / 4) as usize;
        let load_words = words.get(index..index + 131)?;
        extract_sp_task_load_helpers(load_words)
    });
    let context = DerivedShapeContext {
        overlay_recv,
        sp_helpers,
    };

    let mut bindings = Vec::new();
    for (symbol, outcome) in &outcomes {
        let symbol = *symbol;
        let external_address = external.addresses.get(&symbol).copied();
        match outcome {
            HostBindingProbeOutcome::Resolved { vram } => {
                // AGREEMENT: a recognizer and an external address for the same
                // symbol must be equal, or it is a hard error.
                let provenance = match external_address {
                    Some(external_vram) if external_vram != *vram => {
                        return Err(HostBindingDiscoveryError::ExternalReferenceDisagreement {
                            symbol,
                            recognizer: *vram,
                            external: external_vram,
                        });
                    }
                    Some(_) => ResolutionProvenance::ByRecognizerConfirmedByExternal {
                        source: external.source.clone(),
                    },
                    None => ResolutionProvenance::ByRecognizer,
                };
                bindings.push(ResolvedHostBinding {
                    symbol,
                    vram: *vram,
                    provenance,
                });
            }
            // The recognizer did not uniquely resolve. Fall back to the external
            // address only after SHAPE-SANITY validation.
            HostBindingProbeOutcome::Absent
            | HostBindingProbeOutcome::Ambiguous { .. }
            | HostBindingProbeOutcome::NotReached { .. }
            | HostBindingProbeOutcome::Failed { .. } => {
                let Some(external_vram) = external_address else {
                    // No recognizer, no external address: the symbol is absent.
                    continue;
                };
                shape_sanity_at(words, va_start, symbol, external_vram, &context)?;
                bindings.push(ResolvedHostBinding {
                    symbol,
                    vram: external_vram,
                    provenance: ResolutionProvenance::ExternalReferenceValidated {
                        source: external.source.clone(),
                    },
                });
            }
        }
    }

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
